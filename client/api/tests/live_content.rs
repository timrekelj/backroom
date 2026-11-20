//! End-to-end tests of emojis, reactions, channel icons, attachments and admin
//! files against the real server running in-process on a throwaway ScyllaDB
//! keyspace (needs ScyllaDB at 127.0.0.1:9042 and, for media, the S3-compatible
//! store: `docker compose up -d`).
//!
//! Tests for features the server still stubs are `#[ignore]`d; run them with
//! `cargo test -- --ignored` once the server implements them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rand::Rng;
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;
use thehideout_api::{
    AdminFileKind, ApiError, ChannelIconRef, Client, Event, Gateway, GatewayConfig, GatewayUpdate, PaletteColor,
    Permissions, ReactionEmoji,
};
use thehideout_server::config::S3Config;
use thehideout_server::snowflake::Snowflake;
use thehideout_server::storage::Storage;
use thehideout_server::{AppState, build_app, migrations};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::timeout;

const PASSWORD: &str = "correct horse battery";
const HEARTBEAT: Duration = Duration::from_millis(200);
const WAIT: Duration = Duration::from_secs(10);

// 4x4 red PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x06, 0x00, 0x00, 0x00, 0xa9, 0xf1, 0x9e, 0x7e, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x19, 0x33, 0x90, 0x2e, 0x00, 0x00, 0x3c, 0x40, 0x1f, 0xe1, 0xe0, 0x81, 0x77, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Server {
    url: String,
    state: AppState,
}

impl Server {
    async fn start() -> Self {
        let config = S3Config::from_env().expect("S3 config");
        let storage = Storage::s3(&config).expect("configure S3 storage");
        storage
            .check()
            .await
            .unwrap_or_else(|e| panic!("object storage at {} is not reachable ({e:#}); run `docker compose up -d`", config.endpoint));
        let session = SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .build()
            .await
            .expect("connect to ScyllaDB at 127.0.0.1:9042 (is `docker compose up -d` running?)");
        let keyspace = format!("test_{:016x}", rand::rng().random::<u64>());
        migrations::run(&session, &keyspace).await.expect("run migrations");
        let session = Arc::new(CachingSession::from(session, 64));
        let mut state = AppState::new(session, Arc::new(Snowflake::new(0)), keyspace, HEARTBEAT);
        state.storage = Some(storage);
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
        if let Some(storage) = &self.state.storage
            && let Ok(result) = self.state.session.execute_unpaged("SELECT key FROM media_objects", ()).await
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
            .query_unpaged(format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace), ())
            .await;
    }
}

fn spawn_gateway(client: Client) -> (thehideout_api::GatewayHandle, UnboundedReceiver<GatewayUpdate>) {
    let (tx, rx) = unbounded_channel();
    let config = GatewayConfig { heartbeat_interval: HEARTBEAT, ..Default::default() };
    let handle = Gateway::spawn_with(client, config, move |u| {
        let _ = tx.send(u);
    });
    (handle, rx)
}

async fn expect_connected(rx: &mut UnboundedReceiver<GatewayUpdate>) {
    timeout(WAIT, async {
        while !matches!(rx.recv().await.expect("gateway stopped"), GatewayUpdate::Connected) {}
    })
    .await
    .expect("gateway connects");
}

/// Waits for the first update matching `pred`, skipping others.
async fn expect_update(rx: &mut UnboundedReceiver<GatewayUpdate>, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
    timeout(WAIT, async {
        loop {
            if let GatewayUpdate::Event(event) = rx.recv().await.expect("gateway stopped")
                && pred(&event)
            {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[tokio::test]
async fn duplicate_sign_up_is_already_exists() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    assert_eq!(Client::new(&server.url).sign_up("alice", PASSWORD).await.unwrap_err(), ApiError::AlreadyExists);
    server.cleanup().await;
}

#[tokio::test]
async fn emojis_and_three_way_channel_icons() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();
    let (_handle, mut rx) = spawn_gateway(bob.clone());
    expect_connected(&mut rx).await;

    assert!(bob.list_emojis(guild.id).await.unwrap().is_empty());
    let party = alice.create_emoji(guild.id, "party", PNG.to_vec(), "image/png").await.expect("create emoji");
    assert_eq!((party.name.as_str(), party.animated, party.guild_id), ("party", false, guild.id));
    assert!(!party.image.is_empty());
    expect_update(&mut rx, "EmojisUpdated", |e| matches!(e, Event::EmojisUpdated { emojis, .. } if emojis.len() == 1)).await;
    assert!(!bob.fetch_media(&party.image).await.expect("fetch emoji image").is_empty());

    // Permissions, uniqueness, validation, local size check.
    assert_eq!(bob.create_emoji(guild.id, "nope", PNG.to_vec(), "image/png").await.unwrap_err(), ApiError::PermissionDenied);
    assert_eq!(alice.create_emoji(guild.id, "party", PNG.to_vec(), "image/png").await.unwrap_err(), ApiError::AlreadyExists);
    assert!(matches!(alice.create_emoji(guild.id, "X", PNG.to_vec(), "image/png").await, Err(ApiError::InvalidArgument(_))));
    assert_eq!(
        alice.create_emoji(guild.id, "big", vec![0; thehideout_api::MAX_EMOJI_BYTES + 1], "image/png").await.unwrap_err(),
        ApiError::TooLarge
    );

    let renamed = alice.rename_emoji(guild.id, party.id, "confetti").await.expect("rename");
    assert_eq!((renamed.id, renamed.name.as_str()), (party.id, "confetti"));
    expect_update(&mut rx, "rename event", |e| matches!(e, Event::EmojisUpdated { emojis, .. } if emojis[0].name == "confetti")).await;

    // Channel icons: Unicode, custom emoji, Phosphor, then reset.
    let uni = alice
        .create_channel_with_ref(guild.id, "games", None, &ChannelIconRef::Unicode("🎮".into()), PaletteColor::Teal)
        .await
        .expect("create channel with unicode icon");
    assert_eq!((uni.icon_ref.clone(), uni.icon.is_default()), (ChannelIconRef::Unicode("🎮".into()), true));
    let updated = alice
        .update_channel_ref(guild.id, uni.id, None, Some(&ChannelIconRef::Custom(party.id)), None, None)
        .await
        .expect("custom icon");
    assert_eq!(updated.icon_ref, ChannelIconRef::Custom(party.id));
    expect_update(&mut rx, "ChannelUpdated", |e| matches!(e, Event::ChannelUpdated(c) if c.icon_ref == ChannelIconRef::Custom(party.id))).await;
    let updated = alice
        .update_channel_ref(guild.id, uni.id, None, Some(&ChannelIconRef::Phosphor("star".into())), None, None)
        .await
        .unwrap();
    assert_eq!((updated.icon_ref, updated.icon.as_str()), (ChannelIconRef::Phosphor("star".into()), "star"));
    let (channels, _) = bob.list_channels_with_layout(guild.id).await.unwrap();
    assert_eq!(channels.iter().find(|c| c.id == uni.id).unwrap().icon_ref, ChannelIconRef::Phosphor("star".into()));
    let reset = alice.update_channel_ref(guild.id, uni.id, None, Some(&ChannelIconRef::Default), None, None).await.unwrap();
    assert_eq!(reset.icon_ref, ChannelIconRef::Default);
    assert!(matches!(
        alice.update_channel_ref(guild.id, uni.id, None, Some(&ChannelIconRef::Custom(1)), None, None).await,
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        alice.update_channel_ref(guild.id, uni.id, None, Some(&ChannelIconRef::Unicode("ab".into())), None, None).await,
        Err(ApiError::InvalidArgument(_))
    ));

    // The legacy string setters still work and fill `icon_ref`.
    let legacy = alice
        .update_channel(guild.id, uni.id, None, Some(&thehideout_api::ChannelIcon::new("hash")), None, None)
        .await
        .unwrap();
    assert_eq!(legacy.icon_ref, ChannelIconRef::Phosphor("hash".into()));

    // MANAGE_EMOJIS can be delegated through a role.
    let (role, _) = alice.create_role(guild.id, "stickers", PaletteColor::Pink, Permissions(16)).await.unwrap();
    let bob_id = bob.get_me().await.unwrap().id;
    alice.set_member_roles(guild.id, bob_id, &[role.id]).await.unwrap();
    let mine = bob.create_emoji(guild.id, "bobs", PNG.to_vec(), "image/png").await.expect("delegated create");
    alice.delete_emoji(guild.id, party.id).await.expect("delete");
    expect_update(&mut rx, "delete event", |e| matches!(e, Event::EmojisUpdated { emojis, .. } if emojis.len() == 1 && emojis[0].id == mine.id)).await;
    assert_eq!(alice.list_emojis(guild.id).await.unwrap().len(), 1);

    server.cleanup().await;
}

#[tokio::test]
async fn reactions_add_remove_and_events() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    let (_, alice_user) = alice.sign_up("alice", PASSWORD).await.unwrap();
    bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();
    let channel = alice.list_channels(guild.id).await.unwrap()[0].id;
    let (_handle, mut rx) = spawn_gateway(bob.clone());
    expect_connected(&mut rx).await;

    let msg = alice.send_message(channel, "hi", None).await.unwrap();
    assert!(msg.reactions.is_empty() && msg.attachments.is_empty() && msg.embeds.is_empty());
    let party = alice.create_emoji(guild.id, "party", PNG.to_vec(), "image/png").await.unwrap();
    let thumbs = ReactionEmoji::Unicode("👍".into());
    let custom = ReactionEmoji::Custom(party.id);

    alice.add_reaction(channel, msg.id, &thumbs).await.expect("add unicode");
    alice.add_reaction(channel, msg.id, &thumbs).await.expect("idempotent");
    bob.add_reaction(channel, msg.id, &thumbs).await.unwrap();
    bob.add_reaction(channel, msg.id, &custom).await.expect("add custom");
    let event = expect_update(&mut rx, "ReactionAdded", |e| matches!(e, Event::ReactionAdded { user_id, .. } if *user_id == alice_user.id)).await;
    assert_eq!(event, Event::ReactionAdded { channel_id: channel, message_id: msg.id, emoji: thumbs.clone(), user_id: alice_user.id });

    let page = alice.list_messages(channel, None, 10).await.unwrap();
    let reactions = &page.items[0].reactions;
    assert_eq!(reactions.len(), 2);
    assert_eq!((reactions[0].emoji.clone(), reactions[0].count, reactions[0].me), (thumbs.clone(), 2, true));
    assert_eq!((reactions[1].emoji.clone(), reactions[1].count, reactions[1].me), (custom.clone(), 1, false));
    assert_eq!(reactions[1].emoji.key(), format!("c:{}", party.id));

    bob.remove_reaction(channel, msg.id, &thumbs).await.unwrap();
    bob.remove_reaction(channel, msg.id, &thumbs).await.expect("idempotent remove");
    expect_update(&mut rx, "ReactionRemoved", |e| matches!(e, Event::ReactionRemoved { emoji, .. } if *emoji == thumbs)).await;
    let reactions = alice.list_messages(channel, None, 10).await.unwrap().items[0].reactions.clone();
    assert_eq!(reactions[0].count, 1);

    // A custom emoji is never valid in a DM; an unknown channel is not found.
    let bob_id = bob.get_me().await.unwrap().id;
    let dm = alice.open_dm(bob_id).await.unwrap();
    let dm_msg = alice.send_message(dm.id, "psst", None).await.unwrap();
    assert!(matches!(alice.add_reaction(dm.id, dm_msg.id, &custom).await, Err(ApiError::InvalidArgument(_))));
    alice.add_reaction(dm.id, dm_msg.id, &thumbs).await.expect("unicode in dm");
    assert_eq!(alice.add_reaction(999, msg.id, &thumbs).await.unwrap_err(), ApiError::NotFound);

    server.cleanup().await;
}

#[tokio::test]
#[ignore = "server: not implemented yet"]
async fn attachments_upload_send_and_download() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    let outsider = Client::new(&server.url);
    outsider.sign_up("eve", PASSWORD).await.unwrap();
    bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();
    let channel = alice.list_channels(guild.id).await.unwrap()[0].id;
    let (_handle, mut rx) = spawn_gateway(bob.clone());
    expect_connected(&mut rx).await;

    // Progress is reported as chunks are sent and ends at the total.
    let data: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
    let last = Arc::new(AtomicU64::new(0));
    let calls = Arc::new(AtomicU64::new(0));
    let (l, c) = (last.clone(), calls.clone());
    let pending = alice
        .upload_attachment(channel, "data file.bin", "application/octet-stream", data.clone(), move |sent, total| {
            assert_eq!(total, 300_000);
            l.store(sent, Ordering::SeqCst);
            c.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .expect("upload");
    assert_eq!((last.load(Ordering::SeqCst), pending.size), (300_000, 300_000));
    assert!(calls.load(Ordering::SeqCst) > 1);
    assert_eq!(pending.filename, "data file.bin");

    let msg = alice.send_message_with(channel, "", Some("n1"), &[pending.id]).await.expect("send with attachment");
    assert_eq!(msg.attachments.len(), 1);
    let att = &msg.attachments[0];
    assert_eq!((att.size, att.removed), (300_000, false));
    let key = att.file.clone().expect("file key");
    expect_update(&mut rx, "MessageCreated", |e| matches!(e, Event::MessageCreated(m) if m.attachments.len() == 1)).await;

    let mut out = Vec::new();
    let mut last_seen = 0;
    let n = bob.download(&key, &mut out, |received, total| {
        assert_eq!(total, Some(300_000));
        last_seen = received;
    })
    .await
    .expect("download");
    assert_eq!((n, last_seen, out == data), (300_000, 300_000, true));
    assert_eq!(bob.download("missing", &mut Vec::new(), |_, _| {}).await.unwrap_err(), ApiError::NotFound);

    // Errors: no token, outsider, unknown channel, reuse of a consumed upload, too large.
    let anon = Client::new(&server.url);
    assert_eq!(anon.upload_attachment(channel, "a", "text/plain", vec![1], |_, _| {}).await.unwrap_err(), ApiError::Unauthenticated);
    assert_eq!(outsider.upload_attachment(channel, "a", "text/plain", vec![1], |_, _| {}).await.unwrap_err(), ApiError::PermissionDenied);
    assert_eq!(alice.upload_attachment(12345, "a", "text/plain", vec![1], |_, _| {}).await.unwrap_err(), ApiError::NotFound);
    assert!(matches!(alice.send_message_with(channel, "x", None, &[pending.id]).await, Err(ApiError::InvalidArgument(_))));
    let lenient = Client::new(&server.url).with_token(alice.token().unwrap()).with_max_upload_bytes(u64::MAX);
    let huge = vec![0u8; 51 * 1024 * 1024];
    assert_eq!(lenient.upload_attachment(channel, "huge", "application/octet-stream", huge, |_, _| {}).await.unwrap_err(), ApiError::TooLarge);

    server.cleanup().await;
}

#[tokio::test]
#[ignore = "server: not implemented yet"]
async fn admin_files_audit_and_set_admin_precondition() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    let (_, alice_user) = alice.sign_up("alice", PASSWORD).await.unwrap();
    bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    let emoji = alice.create_emoji(guild.id, "party", PNG.to_vec(), "image/png").await.unwrap();

    // Needs `THEHIDEOUT_ADMINS=alice` in the server environment.
    assert_eq!(bob.admin_list_files(None, None, None, 10).await.unwrap_err(), ApiError::PermissionDenied);
    let overview = alice.admin_overview().await.unwrap();
    assert!(overview.storage.emojis > 0);
    let guilds = alice.admin_list_guilds("", None, 10).await.unwrap();
    assert!(guilds.items[0].storage.emojis > 0);

    let files = alice.admin_list_files(Some(guild.id), Some(AdminFileKind::Emoji), None, 10).await.unwrap();
    assert_eq!(files.items.len(), 1);
    assert_eq!((files.items[0].key.as_str(), files.items[0].kind), (emoji.image.as_str(), AdminFileKind::Emoji));
    alice.admin_delete_file(&emoji.image, "test").await.expect("delete file");
    assert!(alice.list_emojis(guild.id).await.unwrap().is_empty());
    let audit = alice.admin_list_audit_log(None, 10).await.unwrap();
    assert_eq!((audit.items[0].action.as_str(), audit.items[0].reason.as_str()), ("file.delete", "test"));
    assert_eq!(alice.admin_delete_file("missing", "").await.unwrap_err(), ApiError::NotFound);

    // Demoting yourself is a failed precondition, not "already exists".
    assert!(matches!(alice.admin_set_admin(alice_user.id, false).await, Err(ApiError::Precondition(_))));

    server.cleanup().await;
}
