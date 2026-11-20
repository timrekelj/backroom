//! `RealBackend` against the real server running in-process on a throwaway ScyllaDB keyspace
//! (needs ScyllaDB at 127.0.0.1:9042: `docker compose up -d`). The backend is driven the way the
//! app drives it: commands in, replies out of `poll`.
//!
//! The binary has no library target, so the modules the backend (and, for the reconnect test, the
//! app state that reacts to it) needs are included by path.

#![allow(dead_code)]

#[path = "../src/admin.rs"]
mod admin;
#[path = "../src/apply.rs"]
mod apply;
#[path = "../src/backend.rs"]
mod backend;
#[path = "../src/emoji.rs"]
mod emoji;
#[path = "../src/format.rs"]
mod format;
#[path = "../src/forms.rs"]
mod forms;
#[path = "../src/layout.rs"]
mod layout;
#[path = "../src/look.rs"]
mod look;
#[path = "../src/media.rs"]
mod media;
#[path = "../src/perms.rs"]
mod perms;
#[path = "../src/real.rs"]
mod real;
#[path = "../src/server_settings.rs"]
mod server_settings;
#[path = "../src/state.rs"]
mod state;
#[path = "../src/structure.rs"]
mod structure;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use backend::{Backend, Command, Reply, Session};
use rand::Rng;
use real::RealBackend;
use rune::prefs::Prefs;
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;
use thehideout_api::{ApiError, Event, GatewayUpdate};
use thehideout_server::config::S3Config;
use thehideout_server::snowflake::Snowflake;
use thehideout_server::storage::Storage;
use thehideout_server::{AppState, build_app, migrations};
use tokio::net::TcpListener;

const PASSWORD: &str = "correct horse battery";
const WAIT: Duration = Duration::from_secs(10);

struct Server {
    url: String,
    state: AppState,
}

impl Server {
    async fn start() -> Self {
        Self::boot(false).await
    }

    /// With the S3-compatible store attached, for pictures (`docker compose up -d`).
    async fn start_with_media() -> Self {
        Self::boot(true).await
    }

    async fn boot(media: bool) -> Self {
        let storage = if media {
            let config = S3Config::from_env().expect("S3 config");
            let storage = Storage::s3(&config).expect("configure S3 storage");
            storage.check().await.unwrap_or_else(|e| {
                panic!(
                    "object storage at {} is not reachable ({e:#}); run `docker compose up -d`",
                    config.endpoint
                )
            });
            Some(storage)
        } else {
            None
        };
        let session = SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .build()
            .await
            .expect("connect to ScyllaDB at 127.0.0.1:9042 (is `docker compose up -d` running?)");
        let keyspace = format!("test_{:016x}", rand::rng().random::<u64>());
        migrations::run(&session, &keyspace)
            .await
            .expect("run migrations");
        let session = Arc::new(CachingSession::from(session, 64));
        let mut state = AppState::new(
            session,
            Arc::new(Snowflake::new(0)),
            keyspace,
            Duration::from_millis(200),
        );
        state.storage = storage;
        let app = build_app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            url: format!("http://{addr}"),
            state,
        }
    }

    async fn cleanup(self) {
        if let Some(storage) = &self.state.storage
            && let Ok(result) = self
                .state
                .session
                .execute_unpaged("SELECT key FROM media_objects", ())
                .await
            && let Ok(rows) = result.into_rows_result()
            && let Ok(rows) = rows.rows::<(String,)>()
        {
            for (key,) in rows.flatten() {
                let _ = storage.delete(&key).await;
            }
        }
        let _ = self
            .state
            .session
            .get_session()
            .query_unpaged(
                format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace),
                (),
            )
            .await;
    }
}

/// A user's backend plus every reply it has produced that no one has asked for yet.
struct Peer {
    backend: RealBackend,
    seen: Vec<Reply>,
    wakes: Arc<AtomicUsize>,
    prefs: std::path::PathBuf,
}

impl Peer {
    fn new(url: &str) -> Peer {
        let prefs = std::env::temp_dir().join(format!(
            "thehideout-test-{:016x}.json",
            rand::rng().random::<u64>()
        ));
        let mut peer = Peer::with_prefs(prefs);
        peer.backend.set_server(url);
        peer
    }

    fn with_prefs(prefs: std::path::PathBuf) -> Peer {
        let mut backend = RealBackend::with_prefs(Prefs::at(&prefs));
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        backend.set_waker(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        Peer {
            backend,
            seen: Vec::new(),
            wakes,
            prefs,
        }
    }

    /// The first reply (already seen or still to come) that `pick` takes.
    fn wait<T>(&mut self, what: &str, mut pick: impl FnMut(&Reply) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(i) = self.seen.iter().position(|r| pick(r).is_some()) {
                return pick(&self.seen.remove(i)).unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; unclaimed: {:?}",
                self.seen
            );
            self.seen.extend(self.backend.poll(0.0));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ask<T>(&mut self, cmd: Command, pick: impl FnMut(&Reply) -> Option<T>) -> T {
        let what = format!("the answer to {cmd:?}");
        self.backend.submit(cmd);
        self.wait(&what, pick)
    }

    fn sign_up(&mut self, name: &str) -> Session {
        let cmd = Command::SignUp {
            username: name.into(),
            password: PASSWORD.into(),
        };
        let session = self.ask(cmd, |r| match r {
            Reply::Auth(a) => Some(a.clone()),
            _ => None,
        });
        session.expect("sign up")
    }

    fn initial(&mut self) -> backend::Initial {
        self.ask(Command::LoadInitial, |r| match r {
            Reply::Initial(i) => Some(i.clone()),
            _ => None,
        })
        .expect("initial load")
    }

    fn connected(&mut self) {
        self.wait("the gateway to connect", |r| {
            matches!(r, Reply::Gateway(GatewayUpdate::Connected)).then_some(())
        });
    }

    fn saved(&self) -> Option<std::collections::HashMap<String, String>> {
        Prefs::at(&self.prefs).get("session")
    }
}

#[test]
fn drives_the_real_server() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start());

    let mut alice = Peer::new(&server.url);
    let mut bob = Peer::new(&server.url);

    // Sign up, and the session lands in the prefs file.
    let a = alice.sign_up("alice");
    assert_eq!(a.me.username, "alice");
    let saved = alice.saved().expect("the session is saved after sign-up");
    assert_eq!(
        (saved["server_url"].as_str(), saved["token"].as_str()),
        (server.url.as_str(), a.token.as_str())
    );
    assert!(
        alice.wakes.load(Ordering::SeqCst) > 0,
        "the waker rings when a reply lands"
    );
    let init = alice.initial();
    assert!((init.me.clone(), init.guilds.len(), init.dms.len()) == (a.me.clone(), 0, 0));
    alice.connected();

    // A guild with its first channel, and one more.
    let guild = alice
        .ask(
            Command::CreateGuild {
                name: "friends".into(),
            },
            |r| match r {
                Reply::GuildCreated(g) => Some(g.clone()),
                _ => None,
            },
        )
        .expect("create guild");
    let channels = alice
        .ask(Command::LoadChannels { guild: guild.id }, |r| match r {
            Reply::Channels { guild: g, result } if *g == guild.id => Some(result.clone()),
            _ => None,
        })
        .expect("channels")
        .0;
    let general = channels[0].id;
    let extra = alice
        .ask(
            Command::CreateChannel {
                guild: guild.id,
                name: "random".into(),
                section: None,
                icon: Default::default(),
                color: Default::default(),
            },
            |r| match r {
                Reply::ChannelCreated(c) => Some(c.clone()),
                _ => None,
            },
        )
        .expect("create channel");
    assert_eq!((extra.guild_id, extra.name.as_str()), (guild.id, "random"));

    // Bob signs up, sees nothing, joins by invite code and loads the members.
    let b = bob.sign_up("bob");
    assert!(bob.initial().guilds.is_empty());
    bob.connected();
    let joined = bob
        .ask(
            Command::JoinGuild {
                code: guild.invite_code.clone(),
            },
            |r| match r {
                Reply::GuildJoined(g) => Some(g.clone()),
                _ => None,
            },
        )
        .expect("join guild");
    assert_eq!(joined.id, guild.id);
    let members = bob
        .ask(Command::LoadMembers { guild: guild.id }, |r| match r {
            Reply::Members { guild: g, result } if *g == guild.id => Some(result.clone()),
            _ => None,
        })
        .expect("members");
    let mut names: Vec<_> = members.iter().map(|u| u.username.as_str()).collect();
    names.sort();
    assert_eq!(names, ["alice", "bob"]);
    alice.wait("bob's join on alice's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::GuildMemberJoined { guild_id, user }))
            if *guild_id == guild.id && user.id == b.me.id =>
        {
            Some(())
        }
        _ => None,
    });

    // A message reaches the other user's gateway with its nonce, and bob's RPC reply carries it too.
    bob.backend.submit(Command::Send {
        channel: general,
        content: "hello alice".into(),
        nonce: "n-1".into(),
    });
    let live = alice.wait("bob's message on alice's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::MessageCreated(m))) if m.nonce == "n-1" => {
            Some(m.clone())
        }
        _ => None,
    });
    assert_eq!(
        (
            live.content.as_str(),
            live.author_id,
            live.channel_id,
            live.guild_id
        ),
        ("hello alice", b.me.id, general, Some(guild.id))
    );
    let sent = bob.wait("bob's send reply", |r| match r {
        Reply::Sent { nonce, result, .. } if nonce == "n-1" => Some(result.clone()),
        _ => None,
    });
    assert_eq!(sent.expect("sent").id, live.id);

    // Pagination: more than a page, walked back with `before`.
    for i in 0..60 {
        let nonce = format!("p-{i}");
        let n = nonce.clone();
        bob.ask(
            Command::Send {
                channel: general,
                content: format!("m{i}"),
                nonce,
            },
            move |r| match r {
                Reply::Sent { nonce, result, .. } if *nonce == n => {
                    Some(result.clone().map(|_| ()))
                }
                _ => None,
            },
        )
        .expect("send");
    }
    let page = |peer: &mut Peer, before| {
        peer.ask(
            Command::LoadMessages {
                channel: general,
                before,
            },
            |r| match r {
                Reply::Messages {
                    channel,
                    before: b,
                    result,
                } if *channel == general && *b == before => Some(result.clone()),
                _ => None,
            },
        )
        .expect("messages")
    };
    let first = page(&mut alice, None);
    assert_eq!((first.items.len(), first.has_more), (50, true));
    assert_eq!(first.items[0].content, "m59", "newest first");
    let second = page(&mut alice, first.next_before);
    assert_eq!((second.items.len(), second.has_more), (11, false));
    assert_eq!(second.items.last().unwrap().content, "hello alice");

    // Direct messages: bob hears of the channel on his gateway and the participants carry names.
    let dm = alice
        .ask(Command::OpenDm { user_id: b.me.id }, |r| match r {
            Reply::DmOpened(d) => Some(d.clone()),
            _ => None,
        })
        .expect("open dm");
    let mut who: Vec<_> = dm
        .participants
        .iter()
        .map(|u| u.username.as_str())
        .collect();
    who.sort();
    assert_eq!(who, ["alice", "bob"]);
    bob.wait("the new DM on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::DmOpened(d))) if d.id == dm.id => Some(()),
        _ => None,
    });

    // Errors come back as replies, not panics.
    let denied = bob.ask(
        Command::CreateChannel {
            guild: guild.id,
            name: "nope".into(),
            section: None,
            icon: Default::default(),
            color: Default::default(),
        },
        |r| match r {
            Reply::ChannelCreated(c) => Some(c.clone()),
            _ => None,
        },
    );
    assert_eq!(denied.unwrap_err(), ApiError::PermissionDenied);

    // Launch again with the saved session: it is validated and the app is signed in as alice.
    let mut again = Peer::with_prefs(alice.prefs.clone());
    let resumed = again.ask(Command::Resume, |r| match r {
        Reply::Resume(x) => Some(x.clone()),
        _ => None,
    });
    let (url, session) = resumed.expect("resume").expect("a saved session");
    assert_eq!(
        (url.as_str(), session.me.id),
        (server.url.as_str(), a.me.id)
    );
    let init = again.initial();
    assert_eq!(
        (init.guilds.len(), init.channels.len(), init.dms.len()),
        (1, 2, 1)
    );
    again.connected();

    // Sign-out forgets it, and nothing of the old session is delivered afterwards.
    alice.backend.submit(Command::SignOut);
    alice.wait("sign-out", |r| matches!(r, Reply::SignedOut).then_some(()));
    assert!(alice.saved().is_none(), "sign-out clears the saved session");
    let mut fresh = Peer::with_prefs(alice.prefs.clone());
    assert_eq!(
        fresh.ask(Command::Resume, |r| match r {
            Reply::Resume(x) => Some(x.clone()),
            _ => None,
        }),
        Ok(None)
    );

    // A refused token is forgotten, quietly: the form shows without an error.
    let stale = std::env::temp_dir().join(format!(
        "thehideout-test-stale-{:016x}.json",
        rand::rng().random::<u64>()
    ));
    let mut prefs = Prefs::at(&stale);
    prefs
        .set(
            "session",
            &std::collections::HashMap::from([
                ("server_url", server.url.as_str()),
                ("token", "not-a-token"),
            ]),
        )
        .unwrap();
    let mut old = Peer::with_prefs(stale);
    let refused = old.ask(Command::Resume, |r| match r {
        Reply::Resume(x) => Some(x.clone()),
        _ => None,
    });
    assert_eq!(refused, Err(ApiError::Unauthenticated));
    assert!(old.saved().is_none());

    // The server field decides where the next sign-in goes; a dead one is a network error.
    let mut lost = Peer::new("http://127.0.0.1:1");
    let down = lost.ask(
        Command::Login {
            username: "alice".into(),
            password: PASSWORD.into(),
        },
        |r| match r {
            Reply::Auth(a) => Some(a.clone()),
            _ => None,
        },
    );
    assert!(matches!(down, Err(ApiError::Network(_))), "got {down:?}");
    assert!(lost.saved().is_none());
    lost.backend.set_server(&server.url);
    let wrong = lost.ask(
        Command::Login {
            username: "alice".into(),
            password: "wrong password!".into(),
        },
        |r| match r {
            Reply::Auth(a) => Some(a.clone()),
            _ => None,
        },
    );
    assert_eq!(
        wrong,
        Err(ApiError::Unauthenticated),
        "the new server answered"
    );
    let ok = lost.ask(
        Command::Login {
            username: "bob".into(),
            password: PASSWORD.into(),
        },
        |r| match r {
            Reply::Auth(a) => Some(a.clone()),
            _ => None,
        },
    );
    assert_eq!(ok.expect("login").me.id, b.me.id);

    rt.block_on(server.cleanup());
    for p in [&alice, &bob, &again, &fresh, &old, &lost] {
        std::fs::remove_file(&p.prefs).ok();
    }
}

// ---- reconnect --------------------------------------------------------------------------------

/// A TCP forwarder that can be killed (cutting every open connection, like a server restart
/// would) and started again on the same port, as `client/api/tests/live.rs` does.
struct Proxy {
    addr: std::net::SocketAddr,
    target: std::net::SocketAddr,
    tasks: tokio::task::JoinSet<()>,
}

impl Proxy {
    async fn start(target: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        Self::serve(listener, addr, target)
    }

    fn serve(
        listener: TcpListener,
        addr: std::net::SocketAddr,
        target: std::net::SocketAddr,
    ) -> Self {
        let mut tasks = tokio::task::JoinSet::new();
        tasks.spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            while let Ok((mut inbound, _)) = listener.accept().await {
                conns.spawn(async move {
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });
        Self {
            addr,
            target,
            tasks,
        }
    }

    async fn kill(&mut self) {
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }

    async fn restart(&mut self) {
        let listener = TcpListener::bind(self.addr)
            .await
            .expect("rebind proxy port");
        *self = Self::serve(listener, self.addr, self.target);
    }
}

/// The app's state wired to the real backend the way `App::pump` wires it to any backend, and the
/// names of the gateway updates it has seen.
struct Desk {
    ui: state::AppState,
    backend: RealBackend,
    gateway: Vec<&'static str>,
    prefs: std::path::PathBuf,
}

impl Desk {
    fn new(url: &str) -> Desk {
        let prefs = std::env::temp_dir().join(format!(
            "thehideout-test-{:016x}.json",
            rand::rng().random::<u64>()
        ));
        let mut ui = state::AppState::new(std::time::SystemTime::now, format::Tz::Local);
        ui.auth.server.replace_text(url);
        Desk {
            ui,
            backend: RealBackend::with_prefs(Prefs::at(&prefs)),
            gateway: Vec::new(),
            prefs,
        }
    }

    fn pump(&mut self) {
        for cmd in std::mem::take(&mut self.ui.outbox) {
            if matches!(cmd, Command::Login { .. } | Command::SignUp { .. }) {
                self.backend.set_server(&self.ui.server_url());
            }
            self.backend.submit(cmd);
        }
        for reply in self.backend.poll(0.0) {
            if let Reply::Gateway(u) = &reply {
                self.gateway.push(match u {
                    GatewayUpdate::Connecting => "Connecting",
                    GatewayUpdate::Connected => "Connected",
                    GatewayUpdate::Disconnected { .. } => "Disconnected",
                    GatewayUpdate::AuthExpired => "AuthExpired",
                    GatewayUpdate::Event(_) => continue,
                });
            }
            apply::apply(&mut self.ui, reply, &mut rune::Tx::default());
        }
    }

    fn until(&mut self, what: &str, cond: impl Fn(&Desk) -> bool) {
        let deadline = Instant::now() + WAIT;
        loop {
            self.pump();
            if cond(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; gateway so far {:?}",
                self.gateway
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn log_has(&self, channel: u64, content: &str) -> usize {
        self.ui.log(channel).map_or(0, |l| {
            l.messages.iter().filter(|m| m.content == content).count()
        })
    }
}

/// The gateway drops and retries (`Connecting` before every attempt, as the real one does), a
/// message is posted meanwhile, and once it is back the app asks for what it missed and shows it
/// once.
#[test]
fn a_dropped_gateway_refetches_what_was_missed() {
    use state::{AuthMode, DialogKind, Place, Screen};

    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start());
    let mut proxy = rt.block_on(Proxy::start(
        server.url.trim_start_matches("http://").parse().unwrap(),
    ));
    let url = format!("http://{}", proxy.addr);

    // Alice runs the app through the proxy and makes a server.
    let mut alice = Desk::new(&url);
    alice.ui.set_auth_mode(AuthMode::SignUp);
    alice.ui.auth.username.replace_text("alice");
    alice.ui.auth.password.replace_text(PASSWORD);
    alice.ui.submit_auth();
    alice.until("the main screen", |d| {
        d.ui.screen == Screen::Main
            && d.ui.ui.connection == thehideout_design::components::ConnectionStatus::Connected
    });
    alice.ui.open_dialog(DialogKind::CreateGuild);
    alice
        .ui
        .ui
        .dialog
        .as_mut()
        .unwrap()
        .input
        .replace_text("friends");
    alice.ui.submit_dialog();
    alice.until("her server and its first channel", |d| {
        d.ui.current_channel()
            .is_some_and(|c| d.ui.log(c).is_some_and(|l| l.loaded))
    });
    let Place::Guild(guild) = alice.ui.ui.place else {
        panic!("not in the new server")
    };
    let channel = alice.ui.current_channel().unwrap();
    let invite = alice.ui.guild(guild).unwrap().invite_code.clone();

    // The connection is cut and stays down long enough for a retry to fail.
    rt.block_on(proxy.kill());
    alice.until("a failed retry", |d| {
        d.gateway
            .windows(3)
            .any(|w| w == ["Disconnected", "Connecting", "Disconnected"])
    });

    // Bob, straight on the server, posts while alice is away.
    let bob = thehideout_api::Client::new(&server.url);
    rt.block_on(async {
        bob.sign_up("bob", PASSWORD).await.unwrap();
        bob.join_guild(&invite).await.unwrap();
        bob.send_message(channel, "posted while you were away", None)
            .await
            .unwrap();
    });
    for _ in 0..10 {
        alice.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        alice.log_has(channel, "posted while you were away"),
        0,
        "nothing reaches her while she is away"
    );

    // The connection returns: the sequence ends Connecting, Connected and the refetch goes out.
    let sent_before = alice.backend.sent().len();
    rt.block_on(proxy.restart());
    alice.until("the message", |d| {
        d.log_has(channel, "posted while you were away") > 0
    });
    assert!(
        alice.gateway.ends_with(&["Connecting", "Connected"]),
        "{:?}",
        alice.gateway
    );
    let sent = &alice.backend.sent()[sent_before..];
    assert!(
        sent.contains(&Command::LoadMessages {
            channel,
            before: None
        }),
        "the open channel is fetched again: {sent:?}"
    );
    assert!(
        sent.contains(&Command::Refresh),
        "the lists are fetched again: {sent:?}"
    );
    alice.until("bob in the member list", |d| {
        d.ui.data.members.get(&guild).is_some_and(|m| m.len() == 2)
    });
    let log = alice.ui.log(channel).unwrap();
    assert_eq!(
        log.messages
            .iter()
            .filter(|m| m.content == "posted while you were away")
            .count(),
        1
    );
    let mut ids: Vec<u64> = log.messages.iter().map(|m| m.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), log.messages.len(), "nothing is duplicated");
    assert_eq!(
        alice.ui.ui.place,
        Place::Guild(guild),
        "the selection stays"
    );

    rt.block_on(proxy.kill());
    rt.block_on(server.cleanup());
    std::fs::remove_file(&alice.prefs).ok();
}

/// Sections, a channel's icon and colour, and the layout, through the real server and a second
/// client's gateway: what one user edits, the other sees as events.
#[test]
fn sections_icons_and_layout_reach_a_second_client() {
    use thehideout_api::{ChannelIconRef, PaletteColor};

    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start());
    let mut alice = Peer::new(&server.url);
    let mut bob = Peer::new(&server.url);
    alice.sign_up("alice");
    alice.initial();
    alice.connected();
    let guild = alice
        .ask(
            Command::CreateGuild {
                name: "friends".into(),
            },
            |r| match r {
                Reply::GuildCreated(g) => Some(g.clone()),
                _ => None,
            },
        )
        .expect("create guild");
    bob.sign_up("bob");
    bob.initial();
    bob.connected();
    bob.ask(
        Command::JoinGuild {
            code: guild.invite_code.clone(),
        },
        |r| matches!(r, Reply::GuildJoined(Ok(_))).then_some(()),
    );
    // Both clients listen for the layout; drop what the setup produced.
    alice.seen.clear();
    bob.seen.clear();
    let guild = guild.id;
    let is_layout = |r: &Reply| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::LayoutUpdated { guild_id, layout }))
            if *guild_id == guild =>
        {
            Some(layout.clone())
        }
        _ => None,
    };

    // Alice creates a section: her reply carries the layout, bob's gateway announces it.
    let (section, layout) = alice
        .ask(
            Command::CreateSection {
                guild,
                name: "Projects".into(),
            },
            |r| match r {
                Reply::SectionSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("create section");
    assert_eq!(section.name, "Projects");
    let seen = bob.wait("the new section on bob's gateway", |r| {
        is_layout(r).filter(|l| l.sections.iter().any(|s| s.id == section.id))
    });
    assert_eq!(seen.version, layout.version);

    // A channel with its own icon and colour, created straight into the section.
    let dev = alice
        .ask(
            Command::CreateChannel {
                guild,
                name: "dev".into(),
                section: Some(section.id),
                icon: ChannelIconRef::Phosphor("code".into()),
                color: PaletteColor::Blue,
            },
            |r| match r {
                Reply::ChannelCreated(c) => Some(c.clone()),
                _ => None,
            },
        )
        .expect("create channel");
    assert_eq!(
        (dev.section_id, dev.icon.as_str(), dev.color),
        (Some(section.id), "code", PaletteColor::Blue)
    );
    let announced = bob.wait("the channel on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::ChannelCreated(c))) if c.id == dev.id => {
            Some(c.clone())
        }
        _ => None,
    });
    assert_eq!(
        (announced.icon.as_str(), announced.color),
        ("code", PaletteColor::Blue)
    );
    let after_create = bob.wait("the layout after the channel", |r| {
        is_layout(r).filter(|l| layout::channels_in(l, Some(section.id)).contains(&dev.id))
    });

    // Alice moves the channel to the top level with the version she holds; bob sees the new order.
    let mine = layout::normalized(&after_create);
    let moved = layout::move_channel_to(&mine, dev.id, None, 0).expect("a real move");
    let saved = alice
        .ask(
            Command::UpdateLayout {
                guild,
                layout: moved.clone(),
            },
            |r| match r {
                Reply::LayoutSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("update layout");
    assert_eq!(layout::channels_in(&saved, None).first(), Some(&dev.id));
    assert!(
        saved.version > mine.version,
        "the server bumped the version"
    );
    let seen = bob.wait("the move on bob's gateway", |r| {
        is_layout(r).filter(|l| l.version == saved.version)
    });
    assert_eq!(layout::channels_in(&seen, None).first(), Some(&dev.id));

    // The same stale version again is refused: refetch and retry.
    let stale = alice.ask(
        Command::UpdateLayout {
            guild,
            layout: moved,
        },
        |r| match r {
            Reply::LayoutSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(stale, Err(ApiError::Conflict));

    // Icon, colour and topic change; bob's gateway carries the new channel.
    let updated = alice
        .ask(
            Command::UpdateChannel {
                guild,
                channel: dev.id,
                name: None,
                icon: Some(ChannelIconRef::Phosphor("rocket".into())),
                color: Some(PaletteColor::Pink),
                topic: Some("ship it".into()),
            },
            |r| match r {
                Reply::ChannelSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("update channel");
    assert_eq!(
        (updated.icon.as_str(), updated.color, updated.topic.as_str()),
        ("rocket", PaletteColor::Pink, "ship it")
    );
    let seen = bob.wait("the update on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::ChannelUpdated(c))) if c.id == dev.id => {
            Some(c.clone())
        }
        _ => None,
    });
    assert_eq!(
        (
            seen.icon.as_str(),
            seen.color,
            seen.topic.as_str(),
            seen.name.as_str()
        ),
        ("rocket", PaletteColor::Pink, "ship it", "dev")
    );

    // Bob may not: he has no role with MANAGE_CHANNELS.
    let denied = bob.ask(
        Command::UpdateChannel {
            guild,
            channel: dev.id,
            name: Some("nope".into()),
            icon: None,
            color: None,
            topic: None,
        },
        |r| match r {
            Reply::ChannelSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(denied, Err(ApiError::PermissionDenied));
    let access = bob
        .ask(Command::LoadAccess { guild }, |r| match r {
            Reply::Access { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("access");
    assert!(
        access.member_roles.contains_key(&2) || !access.member_roles.is_empty(),
        "members are listed with their role ids"
    );

    // Deleting the section leaves its channels at the top level.
    let after = alice
        .ask(
            Command::DeleteSection {
                guild,
                section: section.id,
            },
            |r| match r {
                Reply::SectionDeleted { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("delete section");
    assert!(after.sections.is_empty());
    assert!(layout::channels_in(&after, None).contains(&dev.id));

    rt.block_on(server.cleanup());
    std::fs::remove_file(&alice.prefs).ok();
    std::fs::remove_file(&bob.prefs).ok();
}

/// A role with a colour and MANAGE_CHANNELS, handed to a second user: who could not touch the
/// layout before now reorders it. Role order goes through the real server too, stale version and all.
#[test]
fn a_role_with_manage_channels_lets_a_second_user_reorder_the_layout() {
    use thehideout_api::{PaletteColor, Permissions};

    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start());
    let mut alice = Peer::new(&server.url);
    let mut bob = Peer::new(&server.url);
    alice.sign_up("alice");
    alice.initial();
    alice.connected();
    let guild = alice
        .ask(
            Command::CreateGuild {
                name: "friends".into(),
            },
            |r| match r {
                Reply::GuildCreated(g) => Some(g.clone()),
                _ => None,
            },
        )
        .expect("create guild");
    let b = bob.sign_up("bob");
    bob.initial();
    bob.connected();
    bob.ask(
        Command::JoinGuild {
            code: guild.invite_code.clone(),
        },
        |r| matches!(r, Reply::GuildJoined(Ok(_))).then_some(()),
    );
    let guild = guild.id;
    // A second channel, so there is an order to change.
    let extra = alice
        .ask(
            Command::CreateChannel {
                guild,
                name: "random".into(),
                section: None,
                icon: Default::default(),
                color: Default::default(),
            },
            |r| match r {
                Reply::ChannelCreated(c) => Some(c.clone()),
                _ => None,
            },
        )
        .expect("create channel");
    let channels = |peer: &mut Peer| {
        peer.ask(Command::LoadChannels { guild }, |r| match r {
            Reply::Channels { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("channels")
    };

    // Bob has no role: the layout is not his to change.
    let (_, layout) = channels(&mut bob);
    let mine = layout::normalized(&layout);
    let moved = layout::step_channel(&mine, extra.id, -1).expect("a real move");
    let refused = bob.ask(
        Command::UpdateLayout {
            guild,
            layout: moved.clone(),
        },
        |r| match r {
            Reply::LayoutSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(refused, Err(ApiError::PermissionDenied));

    // Alice makes the role with a colour and MANAGE_CHANNELS; bob's gateway carries the new list.
    bob.seen.clear();
    let (role, version) = alice
        .ask(
            Command::CreateRole {
                guild,
                name: "Builders".into(),
                color: PaletteColor::Blue,
                permissions: Permissions::MANAGE_CHANNELS,
            },
            |r| match r {
                Reply::RoleCreated { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("create role");
    assert_eq!(
        (role.name.as_str(), role.color, role.permissions),
        ("Builders", PaletteColor::Blue, Permissions::MANAGE_CHANNELS)
    );
    let announced = bob.wait("the roles on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::RolesUpdated {
            guild_id,
            roles,
            version: v,
        })) if *guild_id == guild && roles.iter().any(|x| x.id == role.id) => {
            Some((roles.clone(), *v))
        }
        _ => None,
    });
    assert_eq!(announced.1, version);

    // She hands it to bob; his gateway says his roles changed, and now the same move works.
    let member = alice
        .ask(
            Command::SetMemberRoles {
                guild,
                user: b.me.id,
                role_ids: vec![role.id],
            },
            |r| match r {
                Reply::MemberRolesSet { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("set member roles");
    assert_eq!(member.role_ids, vec![role.id]);
    bob.wait("bob's roles on his gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::MemberUpdated { member, .. }))
            if member.user.id == b.me.id && member.role_ids == [role.id] =>
        {
            Some(())
        }
        _ => None,
    });
    let access = bob
        .ask(Command::LoadAccess { guild }, |r| match r {
            Reply::Access { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("access");
    assert_eq!(access.version, version);
    assert_eq!(access.member_roles.get(&b.me.id), Some(&vec![role.id]));
    let saved = bob
        .ask(
            Command::UpdateLayout {
                guild,
                layout: moved,
            },
            |r| match r {
                Reply::LayoutSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("bob may reorder now");
    assert_eq!(layout::channels_in(&saved, None).first(), Some(&extra.id));
    // He still can't manage roles: that was not in the role.
    let denied = bob.ask(
        Command::CreateRole {
            guild,
            name: "Mine".into(),
            color: PaletteColor::None,
            permissions: Permissions::NONE,
        },
        |r| match r {
            Reply::RoleCreated { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(denied.map(|_| ()), Err(ApiError::PermissionDenied));

    // Role order: a second role, then a reorder on the current version, then the same one again (stale).
    let (guests, v2) = alice
        .ask(
            Command::CreateRole {
                guild,
                name: "Guests".into(),
                color: PaletteColor::Slate,
                permissions: Permissions::NONE,
            },
            |r| match r {
                Reply::RoleCreated { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("second role");
    let reorder = |peer: &mut Peer, expected_version: u64, ids: Vec<u64>| {
        peer.ask(
            Command::ReorderRoles {
                guild,
                expected_version,
                role_ids: ids.clone(),
            },
            |r| match r {
                Reply::RolesReordered { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
    };
    // New roles go to the bottom: Builders, Guests. Put Guests on top.
    let done = reorder(&mut alice, v2, vec![guests.id, role.id]).expect("reorder");
    assert_eq!(
        done.roles.iter().map(|r| r.id).collect::<Vec<_>>(),
        [guests.id, role.id]
    );
    assert!(done.version > v2);
    let stale = reorder(&mut alice, v2, vec![role.id, guests.id]);
    assert_eq!(
        stale.map(|_| ()),
        Err(ApiError::Conflict),
        "ABORTED is Conflict"
    );

    rt.block_on(server.cleanup());
    std::fs::remove_file(&alice.prefs).ok();
    std::fs::remove_file(&bob.prefs).ok();
}

/// An avatar uploaded by one user reaches the other's gateway as `UserUpdated`, and the picture
/// can be fetched by its key; a picked file is read through the backend; a server icon and a
/// removal go the same way.
#[test]
fn an_uploaded_avatar_reaches_a_second_client_and_can_be_fetched() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start_with_media());
    let mut alice = Peer::new(&server.url);
    let mut bob = Peer::new(&server.url);
    let a = alice.sign_up("alice");
    alice.initial();
    alice.connected();
    let guild = alice
        .ask(
            Command::CreateGuild {
                name: "friends".into(),
            },
            |r| match r {
                Reply::GuildCreated(g) => Some(g.clone()),
                _ => None,
            },
        )
        .expect("create guild");
    bob.sign_up("bob");
    bob.initial();
    bob.connected();
    bob.ask(
        Command::JoinGuild {
            code: guild.invite_code.clone(),
        },
        |r| matches!(r, Reply::GuildJoined(Ok(_))).then_some(()),
    );
    bob.seen.clear();

    // A picked file is read off-thread by the backend: its bytes and type come back as they were.
    let png = media::generated_png(3, 64);
    let path = std::env::temp_dir().join(format!(
        "thehideout-avatar-{:016x}.png",
        rand::rng().random::<u64>()
    ));
    std::fs::write(&path, &png).unwrap();
    let read = alice
        .ask(
            Command::ReadImage {
                path: path.clone(),
                purpose: backend::ImagePurpose::Avatar,
            },
            |r| match r {
                Reply::ImageRead { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("read the picked file");
    assert_eq!(read, (png.clone(), "image/png".to_owned()));
    std::fs::remove_file(&path).ok();
    let refused = alice.ask(
        Command::ReadImage {
            path: path.with_extension("txt"),
            purpose: backend::ImagePurpose::Avatar,
        },
        |r| match r {
            Reply::ImageRead { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert!(
        matches!(refused, Err(ApiError::InvalidArgument(_))),
        "only images are read"
    );

    // Upload: alice's reply has the key; bob's gateway announces the user with it.
    let user = alice
        .ask(
            Command::UploadAvatar {
                bytes: read.0,
                content_type: read.1,
            },
            |r| match r {
                Reply::AvatarSaved(u) => Some(u.clone()),
                _ => None,
            },
        )
        .expect("upload avatar");
    let key = user.avatar.clone().expect("the user has an avatar key now");
    let seen = bob.wait("alice's new avatar on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::UserUpdated(u)))
            if u.id == a.me.id && u.avatar.is_some() =>
        {
            Some(u.clone())
        }
        _ => None,
    });
    assert_eq!(seen.avatar.as_deref(), Some(key.as_str()));

    // Bob fetches the picture by key: real image bytes, resized by the server.
    let bytes = bob
        .ask(Command::FetchMedia { key: key.clone() }, |r| match r {
            Reply::Media { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("fetch the avatar");
    assert!(!bytes.is_empty());
    let cache = media::MediaCache::default();
    let side = cache
        .preview(bytes)
        .wait()
        .expect("the fetched bytes decode");
    assert_eq!(side[0], side[1], "the server stores it square");
    let unknown = bob.ask(
        Command::FetchMedia {
            key: "no-such-key".into(),
        },
        |r| match r {
            Reply::Media { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(unknown, Err(ApiError::NotFound));

    // A guild icon, announced as GuildUpdated.
    let guild_after = alice
        .ask(
            Command::UploadGuildIcon {
                guild: guild.id,
                bytes: png.clone(),
                content_type: "image/png".into(),
            },
            |r| match r {
                Reply::GuildSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("upload icon");
    let icon = guild_after.icon.clone().expect("the guild has an icon key");
    let seen = bob.wait("the new icon on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::GuildUpdated(g)))
            if g.id == guild.id && g.icon.is_some() =>
        {
            Some(g.clone())
        }
        _ => None,
    });
    assert_eq!(seen.icon.as_deref(), Some(icon.as_str()));
    // Bob is no manager: his try is refused.
    let denied = bob.ask(
        Command::UploadGuildIcon {
            guild: guild.id,
            bytes: png,
            content_type: "image/png".into(),
        },
        |r| match r {
            Reply::GuildSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(denied.map(|_| ()), Err(ApiError::PermissionDenied));

    // Removing the avatar is announced too.
    alice
        .ask(Command::RemoveAvatar, |r| match r {
            Reply::AvatarSaved(u) => Some(u.clone()),
            _ => None,
        })
        .expect("remove avatar");
    bob.wait("the removal on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::UserUpdated(u)))
            if u.id == a.me.id && u.avatar.is_none() =>
        {
            Some(())
        }
        _ => None,
    });

    rt.block_on(server.cleanup());
    std::fs::remove_file(&alice.prefs).ok();
    std::fs::remove_file(&bob.prefs).ok();
}

/// Custom emojis, reactions and an emoji channel icon through the real server: an upload reaches
/// the second client as `EmojisUpdated` and its picture can be fetched; one user's reaction is seen
/// by the other; the channel's `icon_ref` can be a custom emoji, and deleting the emoji says so.
#[test]
fn emojis_reactions_and_an_emoji_channel_icon_reach_a_second_client() {
    use thehideout_api::{ChannelIconRef, ReactionEmoji};

    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(Server::start_with_media());
    let mut alice = Peer::new(&server.url);
    let mut bob = Peer::new(&server.url);
    let a = alice.sign_up("alice");
    alice.initial();
    alice.connected();
    let guild = alice
        .ask(
            Command::CreateGuild {
                name: "friends".into(),
            },
            |r| match r {
                Reply::GuildCreated(g) => Some(g.clone()),
                _ => None,
            },
        )
        .expect("create guild");
    let b = bob.sign_up("bob");
    bob.initial();
    bob.connected();
    bob.ask(
        Command::JoinGuild {
            code: guild.invite_code.clone(),
        },
        |r| matches!(r, Reply::GuildJoined(Ok(_))).then_some(()),
    );
    let g = guild.id;
    bob.seen.clear();

    // Nothing yet. Bob (a plain member) may not add one: MANAGE_EMOJIS is the owner's here.
    let none = alice
        .ask(Command::LoadEmojis { guild: g }, |r| match r {
            Reply::Emojis { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("list emojis");
    assert!(none.is_empty());
    let png = media::generated_emoji(5, 64);
    let denied = bob.ask(
        Command::CreateEmoji {
            guild: g,
            name: "nope".into(),
            bytes: png.clone(),
            content_type: "image/png".into(),
        },
        |r| match r {
            Reply::EmojiSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(denied.map(|_| ()), Err(ApiError::PermissionDenied));
    // A bad name is the server's `INVALID_ARGUMENT` too, whatever the client checks first.
    let bad = alice.ask(
        Command::CreateEmoji {
            guild: g,
            name: "Bad Name".into(),
            bytes: png.clone(),
            content_type: "image/png".into(),
        },
        |r| match r {
            Reply::EmojiSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert!(matches!(bad, Err(ApiError::InvalidArgument(_))));

    // Alice uploads a static one and an animated one; bob's gateway carries the full list each time.
    let cat = alice
        .ask(
            Command::CreateEmoji {
                guild: g,
                name: "smug_cat".into(),
                bytes: png.clone(),
                content_type: "image/png".into(),
            },
            |r| match r {
                Reply::EmojiSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("create emoji");
    assert_eq!(
        (
            cat.name.as_str(),
            cat.animated,
            cat.guild_id,
            cat.creator_id
        ),
        ("smug_cat", false, g, a.me.id)
    );
    let seen = bob.wait("the new emoji on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::EmojisUpdated { guild_id, emojis }))
            if *guild_id == g && emojis.iter().any(|e| e.id == cat.id) =>
        {
            Some(emojis.clone())
        }
        _ => None,
    });
    assert_eq!(seen.len(), 1);
    let gif = media::generated_animated_emoji(6, 64, 6);
    let parrot = alice
        .ask(
            Command::CreateEmoji {
                guild: g,
                name: "party_parrot".into(),
                bytes: gif,
                content_type: "image/gif".into(),
            },
            |r| match r {
                Reply::EmojiSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("create the animated emoji");
    assert!(parrot.animated, "a GIF with several frames is animated");
    bob.wait("the second emoji on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::EmojisUpdated { emojis, .. }))
            if emojis.len() == 2 =>
        {
            Some(())
        }
        _ => None,
    });
    // A taken name is refused.
    let dup = alice.ask(
        Command::CreateEmoji {
            guild: g,
            name: "smug_cat".into(),
            bytes: png.clone(),
            content_type: "image/png".into(),
        },
        |r| match r {
            Reply::EmojiSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert_eq!(dup.map(|_| ()), Err(ApiError::AlreadyExists));

    // Bob lists them and fetches the picture by its key: it decodes, and the animated one is a GIF.
    let listed = bob
        .ask(Command::LoadEmojis { guild: g }, |r| match r {
            Reply::Emojis { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("bob lists emojis");
    assert_eq!(
        listed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["party_parrot", "smug_cat"],
        "by name"
    );
    let fetch = |peer: &mut Peer, key: &str| {
        peer.ask(
            Command::FetchMedia {
                key: key.to_owned(),
            },
            |r| match r {
                Reply::Media { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("fetch the emoji")
    };
    let bytes = fetch(&mut bob, &cat.image);
    let cache = media::MediaCache::default();
    assert!(
        cache.preview(bytes).wait().is_ok(),
        "the fetched picture decodes"
    );
    let moving = fetch(&mut bob, &parrot.image);
    assert!(
        moving.starts_with(b"GIF8"),
        "animated emojis are served as GIFs"
    );
    assert!(
        media::first_frame(&moving).is_some(),
        "so the client can make its still"
    );

    // A rename and its event.
    let renamed = alice
        .ask(
            Command::RenameEmoji {
                guild: g,
                emoji: cat.id,
                name: "smug_dog".into(),
            },
            |r| match r {
                Reply::EmojiSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("rename");
    assert_eq!(renamed.name, "smug_dog");
    bob.wait("the rename on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::EmojisUpdated { emojis, .. }))
            if emojis
                .iter()
                .any(|e| e.id == cat.id && e.name == "smug_dog") =>
        {
            Some(())
        }
        _ => None,
    });

    // A message, and reactions: alice's thumbs-up and a custom one reach bob; bob adds his own.
    let (channels, _) = alice
        .ask(Command::LoadChannels { guild: g }, |r| match r {
            Reply::Channels { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("channels");
    let channel = channels[0].id;
    let message = alice
        .ask(
            Command::Send {
                channel,
                content: "look <:smug_dog:1>".replace('1', &cat.id.to_string()),
                nonce: "n1".into(),
            },
            |r| match r {
                Reply::Sent { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("send")
        .id;
    bob.seen.clear();
    let thumbs = ReactionEmoji::Unicode("\u{1f44d}".into());
    let react = |peer: &mut Peer, emoji: ReactionEmoji, add: bool| {
        let cmd = if add {
            Command::AddReaction {
                channel,
                message,
                emoji,
            }
        } else {
            Command::RemoveReaction {
                channel,
                message,
                emoji,
            }
        };
        peer.ask(cmd, |r| match r {
            Reply::ReactionSaved { result, .. } => Some(result.clone()),
            _ => None,
        })
    };
    react(&mut alice, thumbs.clone(), true).expect("add");
    react(&mut alice, ReactionEmoji::Custom(cat.id), true).expect("add custom");
    let user = bob.wait("alice's reaction on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::ReactionAdded {
            message_id,
            emoji,
            user_id,
            ..
        })) if *message_id == message && *emoji == thumbs => Some(*user_id),
        _ => None,
    });
    assert_eq!(user, a.me.id, "the event carries who reacted");
    // Idempotent: the same again changes nothing and is not an error.
    react(&mut alice, thumbs.clone(), true).expect("idempotent add");
    react(&mut bob, thumbs.clone(), true).expect("bob adds the same emoji");
    let page = bob
        .ask(
            Command::LoadMessages {
                channel,
                before: None,
            },
            |r| match r {
                Reply::Messages { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("messages");
    let m = page.items.iter().find(|m| m.id == message).unwrap();
    let t = m.reactions.iter().find(|r| r.emoji == thumbs).unwrap();
    assert_eq!(
        (t.count, t.me),
        (2, true),
        "two reacted, and bob is one of them"
    );
    let c = m
        .reactions
        .iter()
        .find(|r| r.emoji == ReactionEmoji::Custom(cat.id))
        .unwrap();
    assert_eq!((c.count, c.me), (1, false));
    alice.wait("bob's reaction on alice's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::ReactionAdded { emoji, user_id, .. }))
            if *emoji == thumbs && *user_id == b.me.id =>
        {
            Some(())
        }
        _ => None,
    });
    react(&mut bob, thumbs.clone(), false).expect("remove");
    alice.wait("bob taking it back", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::ReactionRemoved { emoji, user_id, .. }))
            if *emoji == thumbs && *user_id == b.me.id =>
        {
            Some(())
        }
        _ => None,
    });
    // A custom emoji from nowhere is refused.
    let foreign = react(&mut bob, ReactionEmoji::Custom(1), true);
    assert!(
        matches!(
            foreign,
            Err(ApiError::InvalidArgument(_)) | Err(ApiError::NotFound)
        ),
        "{foreign:?}"
    );

    // The channel's icon is the custom emoji; bob sees the update; it can go back to the default.
    let saved = alice
        .ask(
            Command::UpdateChannel {
                guild: g,
                channel,
                name: None,
                icon: Some(ChannelIconRef::Custom(cat.id)),
                color: None,
                topic: None,
            },
            |r| match r {
                Reply::ChannelSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("set the icon");
    assert_eq!(saved.icon_ref, ChannelIconRef::Custom(cat.id));
    bob.wait("the new icon on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::ChannelUpdated(c)))
            if c.id == channel && c.icon_ref == ChannelIconRef::Custom(cat.id) =>
        {
            Some(())
        }
        _ => None,
    });
    let uni = alice
        .ask(
            Command::UpdateChannel {
                guild: g,
                channel,
                name: None,
                icon: Some(ChannelIconRef::Unicode("\u{1f680}".into())),
                color: None,
                topic: None,
            },
            |r| match r {
                Reply::ChannelSaved { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("set a Unicode icon");
    assert_eq!(uni.icon_ref, ChannelIconRef::Unicode("\u{1f680}".into()));
    let created = alice
        .ask(
            Command::CreateChannel {
                guild: g,
                name: "party".into(),
                section: None,
                icon: ChannelIconRef::Custom(parrot.id),
                color: Default::default(),
            },
            |r| match r {
                Reply::ChannelCreated(c) => Some(c.clone()),
                _ => None,
            },
        )
        .expect("a channel created with an emoji icon");
    assert_eq!(created.icon_ref, ChannelIconRef::Custom(parrot.id));
    // An emoji of another guild (here: one that doesn't exist) is not a valid icon.
    let bogus = alice.ask(
        Command::UpdateChannel {
            guild: g,
            channel,
            name: None,
            icon: Some(ChannelIconRef::Custom(1)),
            color: None,
            topic: None,
        },
        |r| match r {
            Reply::ChannelSaved { result, .. } => Some(result.clone()),
            _ => None,
        },
    );
    assert!(
        matches!(bogus, Err(ApiError::InvalidArgument(_))),
        "{bogus:?}"
    );

    // Delete: bob's gateway carries the shorter list.
    alice
        .ask(
            Command::DeleteEmoji {
                guild: g,
                emoji: cat.id,
            },
            |r| match r {
                Reply::EmojiDeleted { result, .. } => Some(result.clone()),
                _ => None,
            },
        )
        .expect("delete");
    bob.wait("the deletion on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::EmojisUpdated { emojis, .. }))
            if emojis.len() == 1 && emojis[0].id == parrot.id =>
        {
            Some(())
        }
        _ => None,
    });

    rt.block_on(server.cleanup());
    std::fs::remove_file(&alice.prefs).ok();
    std::fs::remove_file(&bob.prefs).ok();
}
