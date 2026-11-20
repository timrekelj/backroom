//! `RealBackend` against the real server running in-process on a throwaway ScyllaDB keyspace
//! (needs ScyllaDB at 127.0.0.1:9042: `docker compose up -d`). The backend is driven the way the
//! app drives it: commands in, replies out of `poll`.
//!
//! The binary has no library target, so the modules the backend (and, for the reconnect test, the
//! app state that reacts to it) needs are included by path.

#![allow(dead_code)]

#[path = "../src/apply.rs"]
mod apply;
#[path = "../src/backend.rs"]
mod backend;
#[path = "../src/forms.rs"]
mod forms;
#[path = "../src/format.rs"]
mod format;
#[path = "../src/look.rs"]
mod look;
#[path = "../src/real.rs"]
mod real;
#[path = "../src/state.rs"]
mod state;

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
use thehideout_server::snowflake::Snowflake;
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
        let session = SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .build()
            .await
            .expect("connect to ScyllaDB at 127.0.0.1:9042 (is `docker compose up -d` running?)");
        let keyspace = format!("test_{:016x}", rand::rng().random::<u64>());
        migrations::run(&session, &keyspace).await.expect("run migrations");
        let session = Arc::new(CachingSession::from(session, 64));
        let state = AppState::new(session, Arc::new(Snowflake::new(0)), keyspace, Duration::from_millis(200));
        let app = build_app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { url: format!("http://{addr}"), state }
    }

    async fn cleanup(self) {
        let _ = self.state.session.get_session().query_unpaged(format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace), ()).await;
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
        let prefs = std::env::temp_dir().join(format!("thehideout-test-{:016x}.json", rand::rng().random::<u64>()));
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
        Peer { backend, seen: Vec::new(), wakes, prefs }
    }

    /// The first reply (already seen or still to come) that `pick` takes.
    fn wait<T>(&mut self, what: &str, mut pick: impl FnMut(&Reply) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(i) = self.seen.iter().position(|r| pick(r).is_some()) {
                return pick(&self.seen.remove(i)).unwrap();
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; unclaimed: {:?}", self.seen);
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
        let cmd = Command::SignUp { username: name.into(), password: PASSWORD.into() };
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
        self.wait("the gateway to connect", |r| matches!(r, Reply::Gateway(GatewayUpdate::Connected)).then_some(()));
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
    assert_eq!((saved["server_url"].as_str(), saved["token"].as_str()), (server.url.as_str(), a.token.as_str()));
    assert!(alice.wakes.load(Ordering::SeqCst) > 0, "the waker rings when a reply lands");
    let init = alice.initial();
    assert!((init.me.clone(), init.guilds.len(), init.dms.len()) == (a.me.clone(), 0, 0));
    alice.connected();

    // A guild with its first channel, and one more.
    let guild = alice
        .ask(Command::CreateGuild { name: "friends".into() }, |r| match r {
            Reply::GuildCreated(g) => Some(g.clone()),
            _ => None,
        })
        .expect("create guild");
    let channels = alice
        .ask(Command::LoadChannels { guild: guild.id }, |r| match r {
            Reply::Channels { guild: g, result } if *g == guild.id => Some(result.clone()),
            _ => None,
        })
        .expect("channels");
    let general = channels[0].id;
    let extra = alice
        .ask(Command::CreateChannel { guild: guild.id, name: "random".into() }, |r| match r {
            Reply::ChannelCreated(c) => Some(c.clone()),
            _ => None,
        })
        .expect("create channel");
    assert_eq!((extra.guild_id, extra.name.as_str()), (guild.id, "random"));

    // Bob signs up, sees nothing, joins by invite code and loads the members.
    let b = bob.sign_up("bob");
    assert!(bob.initial().guilds.is_empty());
    bob.connected();
    let joined = bob
        .ask(Command::JoinGuild { code: guild.invite_code.clone() }, |r| match r {
            Reply::GuildJoined(g) => Some(g.clone()),
            _ => None,
        })
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
        Reply::Gateway(GatewayUpdate::Event(Event::GuildMemberJoined { guild_id, user })) if *guild_id == guild.id && user.id == b.me.id => Some(()),
        _ => None,
    });

    // A message reaches the other user's gateway with its nonce, and bob's RPC reply carries it too.
    bob.backend.submit(Command::Send { channel: general, content: "hello alice".into(), nonce: "n-1".into() });
    let live = alice.wait("bob's message on alice's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::MessageCreated(m))) if m.nonce == "n-1" => Some(m.clone()),
        _ => None,
    });
    assert_eq!((live.content.as_str(), live.author_id, live.channel_id, live.guild_id), ("hello alice", b.me.id, general, Some(guild.id)));
    let sent = bob.wait("bob's send reply", |r| match r {
        Reply::Sent { nonce, result, .. } if nonce == "n-1" => Some(result.clone()),
        _ => None,
    });
    assert_eq!(sent.expect("sent").id, live.id);

    // Pagination: more than a page, walked back with `before`.
    for i in 0..60 {
        let nonce = format!("p-{i}");
        let n = nonce.clone();
        bob.ask(Command::Send { channel: general, content: format!("m{i}"), nonce }, move |r| match r {
            Reply::Sent { nonce, result, .. } if *nonce == n => Some(result.clone().map(|_| ())),
            _ => None,
        })
        .expect("send");
    }
    let page = |peer: &mut Peer, before| {
        peer.ask(Command::LoadMessages { channel: general, before }, |r| match r {
            Reply::Messages { channel, before: b, result } if *channel == general && *b == before => Some(result.clone()),
            _ => None,
        })
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
    let mut who: Vec<_> = dm.participants.iter().map(|u| u.username.as_str()).collect();
    who.sort();
    assert_eq!(who, ["alice", "bob"]);
    bob.wait("the new DM on bob's gateway", |r| match r {
        Reply::Gateway(GatewayUpdate::Event(Event::DmOpened(d))) if d.id == dm.id => Some(()),
        _ => None,
    });

    // Errors come back as replies, not panics.
    let denied = bob.ask(Command::CreateChannel { guild: guild.id, name: "nope".into() }, |r| match r {
        Reply::ChannelCreated(c) => Some(c.clone()),
        _ => None,
    });
    assert_eq!(denied.unwrap_err(), ApiError::PermissionDenied);

    // Launch again with the saved session: it is validated and the app is signed in as alice.
    let mut again = Peer::with_prefs(alice.prefs.clone());
    let resumed = again.ask(Command::Resume, |r| match r {
        Reply::Resume(x) => Some(x.clone()),
        _ => None,
    });
    let (url, session) = resumed.expect("resume").expect("a saved session");
    assert_eq!((url.as_str(), session.me.id), (server.url.as_str(), a.me.id));
    let init = again.initial();
    assert_eq!((init.guilds.len(), init.channels.len(), init.dms.len()), (1, 2, 1));
    again.connected();

    // Sign-out forgets it, and nothing of the old session is delivered afterwards.
    alice.backend.submit(Command::SignOut);
    alice.wait("sign-out", |r| matches!(r, Reply::SignedOut).then_some(()));
    assert!(alice.saved().is_none(), "sign-out clears the saved session");
    let mut fresh = Peer::with_prefs(alice.prefs.clone());
    assert_eq!(fresh.ask(Command::Resume, |r| match r { Reply::Resume(x) => Some(x.clone()), _ => None }), Ok(None));

    // A refused token is forgotten, quietly: the form shows without an error.
    let stale = std::env::temp_dir().join(format!("thehideout-test-stale-{:016x}.json", rand::rng().random::<u64>()));
    let mut prefs = Prefs::at(&stale);
    prefs.set("session", &std::collections::HashMap::from([("server_url", server.url.as_str()), ("token", "not-a-token")])).unwrap();
    let mut old = Peer::with_prefs(stale);
    let refused = old.ask(Command::Resume, |r| match r { Reply::Resume(x) => Some(x.clone()), _ => None });
    assert_eq!(refused, Err(ApiError::Unauthenticated));
    assert!(old.saved().is_none());

    // The server field decides where the next sign-in goes; a dead one is a network error.
    let mut lost = Peer::new("http://127.0.0.1:1");
    let down = lost.ask(Command::Login { username: "alice".into(), password: PASSWORD.into() }, |r| match r { Reply::Auth(a) => Some(a.clone()), _ => None });
    assert!(matches!(down, Err(ApiError::Network(_))), "got {down:?}");
    assert!(lost.saved().is_none());
    lost.backend.set_server(&server.url);
    let wrong = lost.ask(Command::Login { username: "alice".into(), password: "wrong password!".into() }, |r| match r { Reply::Auth(a) => Some(a.clone()), _ => None });
    assert_eq!(wrong, Err(ApiError::Unauthenticated), "the new server answered");
    let ok = lost.ask(Command::Login { username: "bob".into(), password: PASSWORD.into() }, |r| match r { Reply::Auth(a) => Some(a.clone()), _ => None });
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

    fn serve(listener: TcpListener, addr: std::net::SocketAddr, target: std::net::SocketAddr) -> Self {
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
        Self { addr, target, tasks }
    }

    async fn kill(&mut self) {
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }

    async fn restart(&mut self) {
        let listener = TcpListener::bind(self.addr).await.expect("rebind proxy port");
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
        let prefs = std::env::temp_dir().join(format!("thehideout-test-{:016x}.json", rand::rng().random::<u64>()));
        let mut ui = state::AppState::new(std::time::SystemTime::now, format::Tz::Local);
        ui.auth.server.replace_text(url);
        Desk { ui, backend: RealBackend::with_prefs(Prefs::at(&prefs)), gateway: Vec::new(), prefs }
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
            assert!(Instant::now() < deadline, "timed out waiting for {what}; gateway so far {:?}", self.gateway);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn log_has(&self, channel: u64, content: &str) -> usize {
        self.ui.log(channel).map_or(0, |l| l.messages.iter().filter(|m| m.content == content).count())
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
    let mut proxy = rt.block_on(Proxy::start(server.url.trim_start_matches("http://").parse().unwrap()));
    let url = format!("http://{}", proxy.addr);

    // Alice runs the app through the proxy and makes a server.
    let mut alice = Desk::new(&url);
    alice.ui.set_auth_mode(AuthMode::SignUp);
    alice.ui.auth.username.replace_text("alice");
    alice.ui.auth.password.replace_text(PASSWORD);
    alice.ui.submit_auth();
    alice.until("the main screen", |d| d.ui.screen == Screen::Main && d.ui.ui.connection == thehideout_design::components::ConnectionStatus::Connected);
    alice.ui.open_dialog(DialogKind::CreateGuild);
    alice.ui.ui.dialog.as_mut().unwrap().input.replace_text("friends");
    alice.ui.submit_dialog();
    alice.until("her server and its first channel", |d| d.ui.current_channel().is_some_and(|c| d.ui.log(c).is_some_and(|l| l.loaded)));
    let Place::Guild(guild) = alice.ui.ui.place else { panic!("not in the new server") };
    let channel = alice.ui.current_channel().unwrap();
    let invite = alice.ui.guild(guild).unwrap().invite_code.clone();

    // The connection is cut and stays down long enough for a retry to fail.
    rt.block_on(proxy.kill());
    alice.until("a failed retry", |d| d.gateway.windows(3).any(|w| w == ["Disconnected", "Connecting", "Disconnected"]));

    // Bob, straight on the server, posts while alice is away.
    let bob = thehideout_api::Client::new(&server.url);
    rt.block_on(async {
        bob.sign_up("bob", PASSWORD).await.unwrap();
        bob.join_guild(&invite).await.unwrap();
        bob.send_message(channel, "posted while you were away", None).await.unwrap();
    });
    for _ in 0..10 {
        alice.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(alice.log_has(channel, "posted while you were away"), 0, "nothing reaches her while she is away");

    // The connection returns: the sequence ends Connecting, Connected and the refetch goes out.
    let sent_before = alice.backend.sent().len();
    rt.block_on(proxy.restart());
    alice.until("the message", |d| d.log_has(channel, "posted while you were away") > 0);
    assert!(alice.gateway.ends_with(&["Connecting", "Connected"]), "{:?}", alice.gateway);
    let sent = &alice.backend.sent()[sent_before..];
    assert!(sent.contains(&Command::LoadMessages { channel, before: None }), "the open channel is fetched again: {sent:?}");
    assert!(sent.contains(&Command::Refresh), "the lists are fetched again: {sent:?}");
    alice.until("bob in the member list", |d| d.ui.data.members.get(&guild).is_some_and(|m| m.len() == 2));
    let log = alice.ui.log(channel).unwrap();
    assert_eq!(log.messages.iter().filter(|m| m.content == "posted while you were away").count(), 1);
    let mut ids: Vec<u64> = log.messages.iter().map(|m| m.id).collect();
    ids.dedup();
    assert_eq!(ids.len(), log.messages.len(), "nothing is duplicated");
    assert_eq!(alice.ui.ui.place, Place::Guild(guild), "the selection stays");

    rt.block_on(proxy.kill());
    rt.block_on(server.cleanup());
    std::fs::remove_file(&alice.prefs).ok();
}
