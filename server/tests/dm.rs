mod common;

use std::time::Duration;

use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::{
    AuthServiceClient, DmChannel, DmServiceClient, Event, GatewayServiceClient, GetMeRequest,
    ListDmsRequest, ListMessagesRequest, Message, MessageServiceClient, OpenDmRequest,
    SendMessageRequest, SignUpRequest, SubscribeRequest,
};
use common::TestApp;
use connectrpc::{ConnectError, ErrorCode};
use tokio::time::{sleep, timeout};

const PASSWORD: &str = "correct horse battery";
const HEARTBEAT: Duration = Duration::from_millis(200);
const WAIT: Duration = Duration::from_secs(5);

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

/// The signed-up user's ID.
async fn user_id(app: &TestApp, token: &str) -> u64 {
    app.client(AuthServiceClient::new, Some(token))
        .get_me(GetMeRequest::default())
        .await
        .expect("get me")
        .into_owned()
        .user
        .expect("user")
        .id
}

fn dms(app: &TestApp, token: Option<&str>) -> DmServiceClient<connectrpc::client::HttpClient> {
    app.client(DmServiceClient::new, token)
}

async fn open_dm(app: &TestApp, token: &str, other: u64) -> Result<DmChannel, ConnectError> {
    let resp = dms(app, Some(token))
        .open_dm(OpenDmRequest {
            user_id: other,
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().channel.expect("channel"))
}

async fn list_dms(app: &TestApp, token: &str) -> Vec<DmChannel> {
    dms(app, Some(token))
        .list_dms(ListDmsRequest::default())
        .await
        .expect("list dms")
        .into_owned()
        .channels
}

async fn send(
    app: &TestApp,
    token: &str,
    channel_id: u64,
    content: &str,
) -> Result<Message, ConnectError> {
    let resp = app
        .client(MessageServiceClient::new, Some(token))
        .send_message(SendMessageRequest {
            channel_id,
            content: content.into(),
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().message.unwrap())
}

fn code(err: ConnectError) -> ErrorCode {
    err.code
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
        msg.to_owned_message()
            .event
            .into_option()
            .expect("response carries an event")
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
async fn open_dm_is_idempotent_and_symmetric() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let (alice_id, bob_id) = (user_id(&app, &alice).await, user_id(&app, &bob).await);

    let ab = open_dm(&app, &alice, bob_id).await.expect("alice opens dm");
    assert_eq!(ab.participants.len(), 2);
    let participant_ids: Vec<u64> = ab.participants.iter().map(|u| u.id).collect();
    assert_eq!(
        participant_ids,
        vec![alice_id.min(bob_id), alice_id.max(bob_id)]
    );
    assert!(ab.created_at.is_set());

    // Opening from the other side returns the same channel, not a new one.
    let ba = open_dm(&app, &bob, alice_id).await.expect("bob opens dm");
    assert_eq!(ba.id, ab.id);
    // Opening again is still the same channel.
    let again = open_dm(&app, &alice, bob_id)
        .await
        .expect("alice re-opens dm");
    assert_eq!(again.id, ab.id);

    // Both users see the DM in their list.
    for token in [&alice, &bob] {
        let channels = list_dms(&app, token).await;
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].id, ab.id);
        assert_eq!(channels[0].participants.len(), 2);
    }

    app.cleanup().await;
}

#[tokio::test]
async fn open_dm_rejects_self_unknown_user_and_missing_auth() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let alice_id = user_id(&app, &alice).await;

    let err = open_dm(&app, &alice, alice_id).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::InvalidArgument);

    // A well-formed but never-issued user ID.
    let err = open_dm(&app, &alice, 1u64 << 62).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);

    let err = dms(&app, None)
        .open_dm(OpenDmRequest {
            user_id: alice_id,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::Unauthenticated);
    let err = dms(&app, None)
        .list_dms(ListDmsRequest::default())
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::Unauthenticated);

    app.cleanup().await;
}

#[tokio::test]
async fn dm_messages_flow_between_participants_only() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let carol = sign_up(&app, "carol").await;
    let bob_id = user_id(&app, &bob).await;
    let dm = open_dm(&app, &alice, bob_id).await.unwrap();

    // Messages in a DM channel carry guild_id = 0.
    let sent = send(&app, &alice, dm.id, "hi bob").await.unwrap();
    assert_eq!(sent.channel_id, dm.id);
    assert_eq!(sent.guild_id, 0);

    // The other participant can read them.
    let page = app
        .client(MessageServiceClient::new, Some(&bob))
        .list_messages(ListMessagesRequest {
            channel_id: dm.id,
            ..Default::default()
        })
        .await
        .expect("bob lists dm messages")
        .into_owned();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].id, sent.id);
    assert_eq!(page.messages[0].content, "hi bob");
    assert_eq!(page.messages[0].guild_id, 0);

    // A stranger gets NotFound, indistinguishable from a nonexistent channel.
    let err = send(&app, &carol, dm.id, "intruder").await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);
    let err = app
        .client(MessageServiceClient::new, Some(&carol))
        .list_messages(ListMessagesRequest {
            channel_id: dm.id,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);
    let err = send(&app, &alice, 12345, "hi").await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);

    // DM channels are not mixed into guild channel listings: alice's DM is not a guild channel.
    let err = app
        .client(
            thehideout_server::proto::thehideout::v1::GuildServiceClient::new,
            Some(&alice),
        )
        .list_channels(thehideout_server::proto::thehideout::v1::ListChannelsRequest {
            guild_id: dm.id,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);

    app.cleanup().await;
}

#[tokio::test]
async fn gateway_delivers_dm_opened_and_dm_messages_to_both_participants() {
    let app = TestApp::start_with_heartbeat(HEARTBEAT).await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let carol = sign_up(&app, "carol").await;
    let (alice_id, bob_id) = (user_id(&app, &alice).await, user_id(&app, &bob).await);

    let mut stream_a = subscribe!(app, &alice);
    let mut stream_b = subscribe!(app, &bob);
    let mut stream_c = subscribe!(app, &carol);

    let dm = open_dm(&app, &alice, bob_id).await.unwrap();

    // Both participants' streams learn about the new DM via dm_opened.
    for stream in [&mut stream_a, &mut stream_b] {
        let event = next_real_event!(stream);
        let Some(Payload::DmOpened(channel)) = event.payload else {
            panic!("expected DmOpened, got {event:?}");
        };
        assert_eq!(channel.id, dm.id);
        assert_eq!(channel.participants.len(), 2);
    }

    // A message in the DM reaches both participants' streams, marked guild_id = 0.
    let sent = send(&app, &alice, dm.id, "hello over dm").await.unwrap();
    for stream in [&mut stream_a, &mut stream_b] {
        let event = next_real_event!(stream);
        let Some(Payload::MessageCreated(message)) = event.payload else {
            panic!("expected MessageCreated, got {event:?}");
        };
        assert_eq!(message.id, sent.id);
        assert_eq!(message.channel_id, dm.id);
        assert_eq!(message.guild_id, 0);
        assert_eq!(message.content, "hello over dm");
    }

    // A bystander's stream hears nothing about the DM (only heartbeats).
    let events = events_within!(stream_c, Duration::from_millis(600));
    assert!(
        events
            .iter()
            .all(|e| matches!(e.payload, Some(Payload::Heartbeat(_))))
    );

    // A DM opened while a stream is already running is picked up live: carol opens a DM
    // with alice, and alice's open stream receives dm_opened for it.
    let dm2 = open_dm(&app, &carol, alice_id).await.unwrap();
    let event = next_real_event!(stream_a);
    let Some(Payload::DmOpened(channel)) = event.payload else {
        panic!("expected DmOpened, got {event:?}");
    };
    assert_eq!(channel.id, dm2.id);

    // Messages in the new DM reach alice's stream too.
    send(&app, &carol, dm2.id, "hi alice").await.unwrap();
    let event = next_real_event!(stream_a);
    let Some(Payload::MessageCreated(message)) = event.payload else {
        panic!("expected MessageCreated, got {event:?}");
    };
    assert_eq!(message.channel_id, dm2.id);
    assert_eq!(message.content, "hi alice");

    // Sequences keep increasing across event kinds.
    let e1 = next_event!(stream_a);
    let e2 = next_event!(stream_a);
    assert_eq!(e2.sequence, e1.sequence + 1);

    sleep(Duration::from_millis(50)).await;
    app.cleanup().await;
}
