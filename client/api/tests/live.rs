//! End-to-end tests against the real server running in-process on a throwaway
//! ScyllaDB keyspace (needs ScyllaDB at 127.0.0.1:9042: `docker compose up -d`).

use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;
use thehideout_api::{ApiError, Client, Event, Gateway, GatewayConfig, GatewayUpdate};
use thehideout_server::snowflake::Snowflake;
use thehideout_server::{AppState, build_app, migrations};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinSet;
use tokio::time::timeout;

const PASSWORD: &str = "correct horse battery";
const HEARTBEAT: Duration = Duration::from_millis(200);
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
        let state = AppState::new(session, Arc::new(Snowflake::new(0)), keyspace, HEARTBEAT);
        let app = build_app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .unwrap();
        });
        Self { url: format!("http://{addr}"), state }
    }

    async fn cleanup(self) {
        let _ = self
            .state
            .session
            .get_session()
            .query_unpaged(format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace), ())
            .await;
    }
}

/// A TCP forwarder that can be killed (cutting every open connection, like a
/// server restart would) and started again on the same port.
struct Proxy {
    addr: std::net::SocketAddr,
    target: std::net::SocketAddr,
    tasks: JoinSet<()>,
}

impl Proxy {
    async fn start(target: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        Self::serve(listener, addr, target)
    }

    fn serve(listener: TcpListener, addr: std::net::SocketAddr, target: std::net::SocketAddr) -> Self {
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let mut conns = JoinSet::new();
            while let Ok((mut inbound, _)) = listener.accept().await {
                conns.spawn(async move {
                    if let Ok(mut outbound) = TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });
        Self { addr, target, tasks }
    }

    /// Kills the listener and every connection through it.
    async fn kill(&mut self) {
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
    }

    async fn restart(&mut self) {
        let listener = TcpListener::bind(self.addr).await.expect("rebind proxy port");
        *self = Self::serve(listener, self.addr, self.target);
    }
}

fn host(url: &str) -> std::net::SocketAddr {
    url.trim_start_matches("http://").parse().unwrap()
}

fn spawn_gateway(client: Client) -> (thehideout_api::GatewayHandle, UnboundedReceiver<GatewayUpdate>) {
    let (tx, rx) = unbounded_channel();
    let config = GatewayConfig {
        initial_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(500),
        heartbeat_interval: HEARTBEAT,
        ..Default::default()
    };
    let handle = Gateway::spawn_with(client, config, move |u| {
        let _ = tx.send(u);
    });
    (handle, rx)
}

/// Waits for the first update matching `pred`, skipping others.
async fn expect_update(rx: &mut UnboundedReceiver<GatewayUpdate>, what: &str, pred: impl Fn(&GatewayUpdate) -> bool) -> GatewayUpdate {
    timeout(WAIT, async {
        loop {
            let update = rx.recv().await.expect("gateway stopped");
            if pred(&update) {
                return update;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[tokio::test]
async fn rpc_round_trip() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);

    let (token, me) = alice.sign_up("alice", PASSWORD).await.expect("sign up");
    assert_eq!(alice.token().as_deref(), Some(token.as_str()));
    assert_eq!(me.username, "alice");
    assert!(me.created_at > std::time::UNIX_EPOCH);
    assert_eq!(alice.sign_up("alice", PASSWORD).await.unwrap_err(), ApiError::AlreadyExists);
    assert_eq!(alice.login("alice", "wrong password!").await.unwrap_err(), ApiError::Unauthenticated);

    let fresh = Client::new(&server.url);
    let (_, logged_in) = fresh.login("alice", PASSWORD).await.expect("login");
    assert_eq!(logged_in.id, me.id);
    assert_eq!(fresh.get_me().await.unwrap(), me);
    assert_eq!(Client::new(&server.url).get_me().await.unwrap_err(), ApiError::Unauthenticated);

    let guild = alice.create_guild("friends").await.expect("create guild");
    assert_eq!(guild.owner_id, me.id);
    assert_eq!(alice.list_guilds().await.unwrap(), vec![guild.clone()]);
    let general = alice.create_channel(guild.id, "random").await.expect("create channel");
    let channels = alice.list_channels(guild.id).await.unwrap();
    assert!(channels.contains(&general));
    let channel_id = channels[0].id;

    let (_, bob_user) = bob.sign_up("bob", PASSWORD).await.unwrap();
    assert_eq!(bob.join_guild("nope-nope").await.unwrap_err(), ApiError::NotFound);
    assert_eq!(bob.join_guild(&guild.invite_code).await.unwrap().id, guild.id);
    let members = alice.list_guild_members(guild.id).await.unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(bob.create_channel(guild.id, "nope").await.unwrap_err(), ApiError::PermissionDenied);

    // Sending and pagination.
    assert!(matches!(alice.send_message(channel_id, "", None).await, Err(ApiError::InvalidArgument(_))));
    let too_long = "x".repeat(thehideout_api::MAX_MESSAGE_CHARS + 1);
    assert!(matches!(alice.send_message(channel_id, &too_long, None).await, Err(ApiError::InvalidArgument(_))));
    let first = alice.send_message(channel_id, "m0", Some("n0")).await.unwrap();
    assert_eq!((first.content.as_str(), first.nonce.as_str()), ("m0", "n0"));
    assert_eq!(first.guild_id, Some(guild.id));
    assert_eq!(first.edited_at, None);
    for i in 1..5 {
        bob.send_message(channel_id, &format!("m{i}"), None).await.unwrap();
    }
    let page1 = alice.list_messages(channel_id, None, 2).await.unwrap();
    assert_eq!(contents(&page1.items), ["m4", "m3"]);
    assert!(page1.has_more);
    let page2 = alice.list_messages(channel_id, page1.next_before, 2).await.unwrap();
    assert_eq!(contents(&page2.items), ["m2", "m1"]);
    let page3 = alice.list_messages(channel_id, page2.next_before, 2).await.unwrap();
    assert_eq!(contents(&page3.items), ["m0"]);
    assert!(!page3.has_more);
    assert_eq!(page3.next_before, None);

    // Direct messages.
    let dm = alice.open_dm(bob_user.id).await.expect("open dm");
    assert_eq!(dm.participants.len(), 2);
    assert_eq!(bob.open_dm(me.id).await.unwrap().id, dm.id, "opening is idempotent");
    assert_eq!(alice.open_dm(u64::MAX).await.unwrap_err(), ApiError::NotFound);
    assert_eq!(alice.list_dms().await.unwrap().len(), 1);
    let sent = bob.send_message(dm.id, "psst", None).await.unwrap();
    assert_eq!(sent.guild_id, None);
    let history = alice.list_messages(dm.id, None, 10).await.unwrap();
    assert_eq!(contents(&history.items), ["psst"]);

    server.cleanup().await;
}

fn contents(messages: &[thehideout_api::Message]) -> Vec<&str> {
    messages.iter().map(|m| m.content.as_str()).collect()
}

#[tokio::test]
async fn gateway_delivers_events() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    let (_, bob_user) = bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();
    let channel_id = alice.list_channels(guild.id).await.unwrap()[0].id;

    let (_handle, mut rx) = spawn_gateway(bob.clone());
    assert_eq!(rx.recv().await, Some(GatewayUpdate::Connecting));
    expect_update(&mut rx, "Connected", |u| *u == GatewayUpdate::Connected).await;
    expect_update(&mut rx, "heartbeat", |u| *u == GatewayUpdate::Event(Event::Heartbeat)).await;

    alice.send_message(channel_id, "hello bob", Some("n-1")).await.unwrap();
    let update = expect_update(&mut rx, "MessageCreated", |u| matches!(u, GatewayUpdate::Event(Event::MessageCreated(_)))).await;
    let GatewayUpdate::Event(Event::MessageCreated(m)) = update else { unreachable!() };
    assert_eq!((m.content.as_str(), m.nonce.as_str(), m.channel_id), ("hello bob", "n-1", channel_id));

    alice.create_channel(guild.id, "extra").await.unwrap();
    expect_update(&mut rx, "ChannelCreated", |u| matches!(u, GatewayUpdate::Event(Event::ChannelCreated(c)) if c.name == "extra")).await;

    alice.open_dm(bob_user.id).await.unwrap();
    expect_update(&mut rx, "DmOpened", |u| matches!(u, GatewayUpdate::Event(Event::DmOpened(_)))).await;

    server.cleanup().await;
}

#[tokio::test]
async fn gateway_reconnects_after_connection_loss() {
    let server = Server::start().await;
    let mut proxy = Proxy::start(host(&server.url)).await;
    let url = format!("http://{}", proxy.addr);
    let alice = Client::new(&url);
    let bob = Client::new(&url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();
    let channel_id = alice.list_channels(guild.id).await.unwrap()[0].id;

    let (_handle, mut rx) = spawn_gateway(bob.clone());
    expect_update(&mut rx, "Connected", |u| *u == GatewayUpdate::Connected).await;

    proxy.kill().await;
    let down = expect_update(&mut rx, "Disconnected", |u| matches!(u, GatewayUpdate::Disconnected { .. })).await;
    let GatewayUpdate::Disconnected { retry_in, .. } = down else { unreachable!() };
    assert!(retry_in <= Duration::from_millis(500));

    proxy.restart().await;
    expect_update(&mut rx, "Connected again", |u| *u == GatewayUpdate::Connected).await;
    alice.send_message(channel_id, "after restart", None).await.unwrap();
    expect_update(&mut rx, "MessageCreated after reconnect", |u| {
        matches!(u, GatewayUpdate::Event(Event::MessageCreated(m)) if m.content == "after restart")
    })
    .await;

    proxy.kill().await;
    server.cleanup().await;
}

#[tokio::test]
async fn gateway_stops_when_token_is_rejected() {
    let server = Server::start().await;
    let client = Client::new(&server.url).with_token("not-a-real-token");
    let (handle, mut rx) = spawn_gateway(client);
    expect_update(&mut rx, "AuthExpired", |u| *u == GatewayUpdate::AuthExpired).await;
    timeout(WAIT, async {
        while !handle.is_stopped() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("gateway task ends after AuthExpired");
    server.cleanup().await;
}

#[tokio::test]
async fn gateway_declares_silent_connection_dead() {
    let server = Server::start().await;
    let client = Client::new(&server.url);
    client.sign_up("alice", PASSWORD).await.unwrap();
    // The server heartbeats every 200ms; a watchdog window far below that
    // must trip and reconnect.
    let (tx, mut rx) = unbounded_channel();
    let config = GatewayConfig {
        initial_backoff: Duration::from_millis(50),
        heartbeat_interval: Duration::from_millis(1),
        silence_factor: 2.5,
        ..Default::default()
    };
    let _handle = Gateway::spawn_with(client, config, move |u| {
        let _ = tx.send(u);
    });
    expect_update(&mut rx, "Disconnected", |u| matches!(u, GatewayUpdate::Disconnected { .. })).await;
    server.cleanup().await;
}
