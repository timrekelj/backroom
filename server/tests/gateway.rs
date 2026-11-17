mod common;

use std::time::Duration;

use backroom_server::proto::backroom::v1::event::Payload;
use backroom_server::proto::backroom::v1::{
    AuthServiceClient, CreateChannelRequest, CreateGuildRequest, Event, GatewayServiceClient, Guild,
    GuildServiceClient, JoinGuildRequest, ListChannelsRequest, MessageServiceClient,
    SendMessageRequest, SignUpRequest, SubscribeRequest,
};
use common::TestApp;
use connectrpc::{ConnectError, ErrorCode};
use tokio::time::{sleep, timeout};

const PASSWORD: &str = "correct horse battery";
const HEARTBEAT: Duration = Duration::from_millis(200);
const WAIT: Duration = Duration::from_secs(5);

async fn start() -> TestApp {
    TestApp::start_with_heartbeat(HEARTBEAT).await
}

async fn sign_up(app: &TestApp, username: &str) -> String {
    app.client(AuthServiceClient::new, None)
        .sign_up(SignUpRequest {
            username: username.into(),
            password: PASSWORD.into(),
            ..Default::default()
        })
        .await
        .expect("sign up")
        .into_owned()
        .token
}

/// Creates a guild for `token` and returns it with its `general` channel id.
async fn create_guild(app: &TestApp, token: &str) -> (Guild, u64) {
    let client = app.client(GuildServiceClient::new, Some(token));
    let guild = client
        .create_guild(CreateGuildRequest {
            name: "guild".into(),
            ..Default::default()
        })
        .await
        .expect("create guild")
        .into_owned()
        .guild
        .unwrap();
    let channels = client
        .list_channels(ListChannelsRequest {
            guild_id: guild.id,
            ..Default::default()
        })
        .await
        .expect("list channels")
        .into_owned()
        .channels;
    (guild, channels[0].id)
}

async fn join(app: &TestApp, token: &str, invite_code: &str) {
    app.client(GuildServiceClient::new, Some(token))
        .join_guild(JoinGuildRequest {
            invite_code: invite_code.into(),
            ..Default::default()
        })
        .await
        .expect("join guild");
}

async fn send(app: &TestApp, token: &str, channel_id: u64, content: &str, nonce: Option<&str>) {
    app.client(MessageServiceClient::new, Some(token))
        .send_message(SendMessageRequest {
            channel_id,
            content: content.into(),
            nonce: nonce.map(str::to_owned),
            ..Default::default()
        })
        .await
        .expect("send message");
}

/// Opens a Subscribe stream for `token`.
macro_rules! subscribe {
    ($app:expr, $token:expr) => {
        $app.client(GatewayServiceClient::new, Some($token))
            .subscribe(SubscribeRequest::default())
            .await
            .expect("subscribe")
    };
}

/// Next event on the stream (heartbeats included), failing the test on timeout or stream end.
macro_rules! next_event {
    ($stream:expr) => {{
        let msg = timeout(WAIT, $stream.message())
            .await
            .expect("timed out waiting for an event")
            .expect("stream error")
            .expect("stream ended");
        msg.to_owned_message().event.into_option().expect("response carries an event")
    }};
}

/// Next event that is not a heartbeat.
macro_rules! next_real_event {
    ($stream:expr) => {{
        loop {
            let event: Event = next_event!($stream);
            if !matches!(event.payload, Some(Payload::Heartbeat(_))) {
                break event;
            }
        }
    }};
}

/// Collects every event arriving within `window`.
macro_rules! events_within {
    ($stream:expr, $window:expr) => {{
        let mut events: Vec<Event> = Vec::new();
        let deadline = tokio::time::Instant::now() + $window;
        while let Ok(res) = tokio::time::timeout_at(deadline, $stream.message()).await {
            let msg = res.expect("stream error").expect("stream ended");
            events.push(msg.to_owned_message().event.into_option().unwrap());
        }
        events
    }};
}

#[tokio::test]
async fn first_event_is_heartbeat_and_sequences_increase_by_one() {
    let app = start().await;
    let token = sign_up(&app, "alice").await;
    let mut stream = subscribe!(app, &token);

    let first = next_event!(stream);
    assert_eq!(first.sequence, 1);
    assert!(matches!(first.payload, Some(Payload::Heartbeat(_))));

    for expected in 2..=4 {
        let event = next_event!(stream);
        assert_eq!(event.sequence, expected);
        assert!(matches!(event.payload, Some(Payload::Heartbeat(_))));
    }
    app.cleanup().await;
}

#[tokio::test]
async fn message_reaches_other_members_and_the_sender() {
    let app = start().await;
    let (a, b) = (sign_up(&app, "alice").await, sign_up(&app, "bob").await);
    let (guild, channel_id) = create_guild(&app, &a).await;
    join(&app, &b, &guild.invite_code).await;

    let mut stream_a = subscribe!(app, &a);
    let mut stream_b = subscribe!(app, &b);
    send(&app, &a, channel_id, "hello bob", Some("n-1")).await;

    for stream in [&mut stream_b, &mut stream_a] {
        let event = next_real_event!(stream);
        let Some(Payload::MessageCreated(message)) = event.payload else {
            panic!("expected MessageCreated, got {event:?}");
        };
        assert_eq!(message.content, "hello bob");
        assert_eq!(message.nonce, "n-1");
        assert_eq!(message.channel_id, channel_id);
        assert_eq!(message.guild_id, guild.id);
    }
    app.cleanup().await;
}

#[tokio::test]
async fn create_channel_is_broadcast() {
    let app = start().await;
    let (a, b) = (sign_up(&app, "alice").await, sign_up(&app, "bob").await);
    let (guild, _) = create_guild(&app, &a).await;
    join(&app, &b, &guild.invite_code).await;

    let mut stream_b = subscribe!(app, &b);
    app.client(GuildServiceClient::new, Some(&a))
        .create_channel(CreateChannelRequest {
            guild_id: guild.id,
            name: "random".into(),
            ..Default::default()
        })
        .await
        .expect("create channel");

    let event = next_real_event!(stream_b);
    let Some(Payload::ChannelCreated(channel)) = event.payload else {
        panic!("expected ChannelCreated, got {event:?}");
    };
    assert_eq!(channel.name, "random");
    assert_eq!(channel.guild_id, guild.id);
    app.cleanup().await;
}

#[tokio::test]
async fn member_join_is_broadcast() {
    let app = start().await;
    let (a, b, c) = (
        sign_up(&app, "alice").await,
        sign_up(&app, "bob").await,
        sign_up(&app, "carol").await,
    );
    let (guild, _) = create_guild(&app, &a).await;
    join(&app, &b, &guild.invite_code).await;

    let mut stream_b = subscribe!(app, &b);
    join(&app, &c, &guild.invite_code).await;

    let event = next_real_event!(stream_b);
    let Some(Payload::GuildMemberJoined(joined)) = event.payload else {
        panic!("expected GuildMemberJoined, got {event:?}");
    };
    assert_eq!(joined.guild_id, guild.id);
    assert_eq!(joined.user.as_option().expect("user").username, "carol");

    // Joining again is not a new join.
    join(&app, &c, &guild.invite_code).await;
    let events = events_within!(stream_b, Duration::from_millis(600));
    assert!(events.iter().all(|e| matches!(e.payload, Some(Payload::Heartbeat(_)))));
    app.cleanup().await;
}

#[tokio::test]
async fn stream_opened_before_joining_receives_the_new_guild() {
    let app = start().await;
    let (a, c) = (sign_up(&app, "alice").await, sign_up(&app, "carol").await);
    let (guild, channel_id) = create_guild(&app, &a).await;

    // Carol is in no guild yet.
    let mut stream_c = subscribe!(app, &c);
    assert!(matches!(next_event!(stream_c).payload, Some(Payload::Heartbeat(_))));
    join(&app, &c, &guild.invite_code).await;

    // The stream picks up the new guild asynchronously, so keep sending until it shows.
    let mut received = None;
    for attempt in 0..50 {
        send(&app, &a, channel_id, &format!("ping {attempt}"), None).await;
        let events = events_within!(stream_c, Duration::from_millis(100));
        received = events.into_iter().find_map(|e| match e.payload {
            Some(Payload::MessageCreated(m)) => Some(m),
            _ => None,
        });
        if received.is_some() {
            break;
        }
    }
    let message = received.expect("carol never received a message after joining");
    assert!(message.content.starts_with("ping "));
    assert_eq!(message.guild_id, guild.id);
    app.cleanup().await;
}

#[tokio::test]
async fn stream_opened_before_creating_a_guild_receives_it() {
    let app = start().await;
    let a = sign_up(&app, "alice").await;
    let mut stream_a = subscribe!(app, &a);
    assert!(matches!(next_event!(stream_a).payload, Some(Payload::Heartbeat(_))));

    let (guild, channel_id) = create_guild(&app, &a).await;
    let mut received = None;
    for attempt in 0..50 {
        send(&app, &a, channel_id, &format!("ping {attempt}"), None).await;
        let events = events_within!(stream_a, Duration::from_millis(100));
        received = events.into_iter().find_map(|e| match e.payload {
            Some(Payload::MessageCreated(m)) => Some(m),
            _ => None,
        });
        if received.is_some() {
            break;
        }
    }
    assert_eq!(received.expect("no message after creating a guild").guild_id, guild.id);
    app.cleanup().await;
}

#[tokio::test]
async fn non_members_receive_only_heartbeats() {
    let app = start().await;
    let (a, d) = (sign_up(&app, "alice").await, sign_up(&app, "dave").await);
    let (guild, channel_id) = create_guild(&app, &a).await;
    let mut stream_d = subscribe!(app, &d);

    send(&app, &a, channel_id, "secret", None).await;
    app.client(GuildServiceClient::new, Some(&a))
        .create_channel(CreateChannelRequest {
            guild_id: guild.id,
            name: "private".into(),
            ..Default::default()
        })
        .await
        .expect("create channel");

    let events = events_within!(stream_d, Duration::from_millis(900));
    assert!(!events.is_empty(), "expected heartbeats");
    for event in events {
        assert!(matches!(event.payload, Some(Payload::Heartbeat(_))), "unexpected {event:?}");
    }
    app.cleanup().await;
}

#[tokio::test]
async fn unauthenticated_subscribe_is_rejected() {
    let app = start().await;
    let client = app.client(GatewayServiceClient::new, None);
    let err: ConnectError = match client.subscribe(SubscribeRequest::default()).await {
        Err(err) => err,
        Ok(mut stream) => stream.message().await.expect_err("stream must fail"),
    };
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    app.cleanup().await;
}

#[tokio::test]
async fn dropped_streams_leave_no_channels_behind() {
    let app = start().await;
    let (a, b) = (sign_up(&app, "alice").await, sign_up(&app, "bob").await);
    let (guild, channel_id) = create_guild(&app, &a).await;
    join(&app, &b, &guild.invite_code).await;

    let mut stream_a = subscribe!(app, &a);
    let mut stream_b = subscribe!(app, &b);
    next_event!(stream_a);
    next_event!(stream_b);
    assert_eq!(app.state.events.guild_channel_count(), 1);
    assert_eq!(app.state.events.user_channel_count(), 2);
    send(&app, &a, channel_id, "bye", None).await;
    next_real_event!(stream_a);

    drop(stream_a);
    drop(stream_b);
    // The server notices the disconnect on its next write (the next heartbeat).
    for _ in 0..50 {
        if app.state.events.guild_channel_count() == 0 && app.state.events.user_channel_count() == 0 {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(app.state.events.guild_channel_count(), 0);
    assert_eq!(app.state.events.user_channel_count(), 0);
    app.cleanup().await;
}
