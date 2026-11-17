mod common;

use backroom_server::convert::{id_to_db, snowflake_datetime};
use backroom_server::proto::backroom::v1::{
    AuthServiceClient, CreateGuildRequest, GuildServiceClient, ListChannelsRequest,
    ListMessagesRequest, ListMessagesResponse, Message, MessageServiceClient, SendMessageRequest,
    SignUpRequest,
};
use backroom_server::snowflake::{BUCKET_MS, bucket_of, make_id};
use common::TestApp;
use connectrpc::{ConnectError, ErrorCode};

const PASSWORD: &str = "correct horse battery";

async fn sign_up(app: &TestApp, username: &str) -> String {
    let client = app.client(AuthServiceClient::new, None);
    client
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

/// Creates a guild for `token` and returns `(guild_id, general_channel_id)`.
async fn guild_with_channel(app: &TestApp, token: &str) -> (u64, u64) {
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
    (guild.id, channels[0].id)
}

fn messages(app: &TestApp, token: Option<&str>) -> MessageServiceClient<connectrpc::client::HttpClient> {
    app.client(MessageServiceClient::new, token)
}

async fn send(
    app: &TestApp,
    token: &str,
    channel_id: u64,
    content: &str,
    nonce: Option<&str>,
) -> Result<Message, ConnectError> {
    let resp = messages(app, Some(token))
        .send_message(SendMessageRequest {
            channel_id,
            content: content.into(),
            nonce: nonce.map(str::to_owned),
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().message.unwrap())
}

async fn list(
    app: &TestApp,
    token: &str,
    channel_id: u64,
    before: Option<u64>,
    limit: u32,
) -> Result<ListMessagesResponse, ConnectError> {
    let resp = messages(app, Some(token))
        .list_messages(ListMessagesRequest {
            channel_id,
            before,
            limit,
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned())
}

fn code(err: ConnectError) -> ErrorCode {
    err.code
}

/// Inserts a channel with a hand-built id (so it can be old) into `guild_id`.
async fn insert_channel(app: &TestApp, guild_id: u64, channel_id: u64) {
    let session = &app.state.session;
    session
        .execute_unpaged(
            "INSERT INTO channels (guild_id, id, name, created_at) VALUES (?, ?, ?, ?)",
            (id_to_db(guild_id), id_to_db(channel_id), "old", snowflake_datetime(channel_id)),
        )
        .await
        .unwrap();
    session
        .execute_unpaged(
            "INSERT INTO channels_by_id (id, guild_id) VALUES (?, ?)",
            (id_to_db(channel_id), id_to_db(guild_id)),
        )
        .await
        .unwrap();
}

async fn insert_message(app: &TestApp, channel_id: u64, guild_id: u64, id: u64) {
    app.state
        .session
        .execute_unpaged(
            "INSERT INTO messages (channel_id, bucket, id, guild_id, author_id, content, nonce) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                id_to_db(channel_id),
                bucket_of(id),
                id_to_db(id),
                id_to_db(guild_id),
                0i64,
                format!("m{id}"),
                "",
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn send_then_list_returns_message() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let (guild_id, channel_id) = guild_with_channel(&app, &alice).await;

    let sent = send(&app, &alice, channel_id, "  hello  ", Some("n-1")).await.unwrap();
    assert_eq!(sent.content, "  hello  ");
    assert_eq!(sent.nonce, "n-1");
    assert_eq!(sent.channel_id, channel_id);
    assert_eq!(sent.guild_id, guild_id);

    let page = list(&app, &alice, channel_id, None, 0).await.unwrap();
    assert_eq!(page.messages.len(), 1);
    assert!(!page.has_more);
    assert_eq!(page.next_before, None);
    let got = &page.messages[0];
    assert_eq!(got.id, sent.id);
    assert_eq!(got.channel_id, channel_id);
    assert_eq!(got.guild_id, guild_id);
    assert_eq!(got.author_id, sent.author_id);
    assert_ne!(got.author_id, 0);
    assert_eq!(got.content, "  hello  ");
    assert_eq!(got.nonce, "n-1");
    assert!(!got.edited_at.is_set());
    assert!(got.created_at.is_set());
    assert_eq!(got.created_at, sent.created_at);
    let expected = backroom_server::convert::datetime_to_proto(snowflake_datetime(got.id));
    assert_eq!(*got.created_at, expected);

    let no_nonce = send(&app, &alice, channel_id, "x", None).await.unwrap();
    assert_eq!(no_nonce.nonce, "");

    app.cleanup().await;
}

#[tokio::test]
async fn paginates_newest_first_without_gaps() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let (_, channel_id) = guild_with_channel(&app, &alice).await;

    let mut sent = Vec::new();
    for i in 0..25 {
        sent.push(send(&app, &alice, channel_id, &format!("msg {i}"), None).await.unwrap().id);
    }
    sent.reverse();

    let mut seen = Vec::new();
    let mut before = None;
    let mut sizes = Vec::new();
    let mut flags = Vec::new();
    loop {
        let page = list(&app, &alice, channel_id, before, 10).await.unwrap();
        sizes.push(page.messages.len());
        flags.push(page.has_more);
        seen.extend(page.messages.iter().map(|m| m.id));
        if page.has_more {
            assert_eq!(page.next_before, page.messages.last().map(|m| m.id));
            before = page.next_before;
        } else {
            assert_eq!(page.next_before, None);
            break;
        }
    }
    assert_eq!(sizes, vec![10, 10, 5]);
    assert_eq!(flags, vec![true, true, false]);
    assert_eq!(seen, sent);

    app.cleanup().await;
}

#[tokio::test]
async fn paginates_across_buckets() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let (guild_id, _) = guild_with_channel(&app, &alice).await;

    let now_bucket = bucket_of(app.state.snowflake.next_id());
    assert!(now_bucket >= 10, "test assumes the clock is well past the epoch");
    let base = now_bucket as u64 - 8;
    let in_bucket = |bucket: u64, offset_ms: u64| make_id(bucket * BUCKET_MS + offset_ms, 1, 0);

    // The channel is created at the start of `base`; messages sit in `base`, `base + 3`
    // (with empty buckets between) and the current bucket.
    let channel_id = in_bucket(base, 0);
    insert_channel(&app, guild_id, channel_id).await;
    let mut expected = vec![
        in_bucket(base, 1_000),
        in_bucket(base, 2_000),
        in_bucket(base + 3, 5),
        in_bucket(base + 3, 6),
        in_bucket(base + 3, 7),
        in_bucket(now_bucket as u64, 10),
        in_bucket(now_bucket as u64, 20),
    ];
    for id in &expected {
        insert_message(&app, channel_id, guild_id, *id).await;
    }
    expected.reverse();

    for limit in [1u32, 2, 3, 100] {
        let mut seen = Vec::new();
        let mut before = None;
        loop {
            let page = list(&app, &alice, channel_id, before, limit).await.unwrap();
            seen.extend(page.messages.iter().map(|m| m.id));
            if !page.has_more {
                assert_eq!(page.next_before, None);
                break;
            }
            before = page.next_before;
        }
        assert_eq!(seen, expected, "limit {limit}");
    }

    // A cursor inside an empty bucket still finds older messages.
    let page = list(&app, &alice, channel_id, Some(in_bucket(base + 2, 0)), 10).await.unwrap();
    assert_eq!(page.messages.len(), 2);
    assert!(!page.has_more);

    // A cursor in the far future is clamped to the present.
    let page = list(&app, &alice, channel_id, Some(u64::MAX), 100).await.unwrap();
    assert_eq!(page.messages.len(), expected.len());

    // `before = 0` means nothing is older.
    let page = list(&app, &alice, channel_id, Some(0), 10).await.unwrap();
    assert!(page.messages.is_empty());
    assert!(!page.has_more);

    app.cleanup().await;
}

#[tokio::test]
async fn limit_defaults_and_caps() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let (guild_id, channel_id) = guild_with_channel(&app, &alice).await;

    let now = app.state.snowflake.next_id();
    for i in 0..120u64 {
        insert_message(&app, channel_id, guild_id, now + 1 + i).await;
    }

    let page = list(&app, &alice, channel_id, None, 0).await.unwrap();
    assert_eq!(page.messages.len(), 50);
    assert!(page.has_more);

    let page = list(&app, &alice, channel_id, None, 500).await.unwrap();
    assert_eq!(page.messages.len(), 100);
    assert!(page.has_more);
    assert_eq!(page.next_before, page.messages.last().map(|m| m.id));

    app.cleanup().await;
}

#[tokio::test]
async fn rejects_invalid_content_and_nonce() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let (_, channel_id) = guild_with_channel(&app, &alice).await;

    for content in ["", "   \n\t "] {
        let err = send(&app, &alice, channel_id, content, None).await.unwrap_err();
        assert_eq!(code(err), ErrorCode::InvalidArgument, "content {content:?}");
    }
    let err = send(&app, &alice, channel_id, &"a".repeat(4001), None).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::InvalidArgument);
    // Length counts characters, not bytes.
    send(&app, &alice, channel_id, &"é".repeat(4000), None).await.expect("4000 chars is allowed");

    let err = send(&app, &alice, channel_id, "hi", Some(&"n".repeat(65))).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::InvalidArgument);
    send(&app, &alice, channel_id, "hi", Some(&"n".repeat(64))).await.expect("64 chars is allowed");

    app.cleanup().await;
}

#[tokio::test]
async fn access_control() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let (_, channel_id) = guild_with_channel(&app, &alice).await;
    send(&app, &alice, channel_id, "secret", None).await.unwrap();

    let err = send(&app, &bob, channel_id, "hi", None).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);
    let err = list(&app, &bob, channel_id, None, 10).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);

    let err = send(&app, &alice, 12345, "hi", None).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);
    let err = list(&app, &alice, 12345, None, 10).await.unwrap_err();
    assert_eq!(code(err), ErrorCode::NotFound);

    let err = messages(&app, None)
        .send_message(SendMessageRequest {
            channel_id,
            content: "hi".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::Unauthenticated);
    let err = messages(&app, None)
        .list_messages(ListMessagesRequest {
            channel_id,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(code(err), ErrorCode::Unauthenticated);

    app.cleanup().await;
}
