//! The site-admin service against Scylla and the S3-compatible store from
//! `docker-compose.yml`. Tests that read site-wide numbers use their own
//! keyspace; the rest share one and only look at what they created.
mod common;

use std::time::Duration;

use common::fixtures::*;
use common::TestApp;
use connectrpc::{ConnectError, ErrorCode};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use thehideout_server::admin;
use thehideout_server::convert::id_to_db;
use thehideout_server::media::{self, Meta, Owner};
use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::*;
use thehideout_server::snowflake::bucket_of;
use thehideout_server::stats;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let image = RgbaImage::from_fn(w, h, |x, y| Rgba([(x as u8).wrapping_add(seed), (y as u8).wrapping_mul(3), seed, 255]));
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(image.as_raw(), w, h, ExtendedColorType::Rgba8).unwrap();
    out
}

fn svc(app: &TestApp, who: &Account) -> AdminServiceClient<connectrpc::client::HttpClient> {
    app.client(AdminServiceClient::new, Some(&who.token))
}

/// A signed-up user made site admin.
async fn admin_user(app: &TestApp, name: &str) -> Account {
    let who = user(app, name).await;
    admin::set_flag(&app.state, who.id, true).await.unwrap();
    who
}

fn code<T>(r: Result<T, ConnectError>) -> ErrorCode {
    r.err().expect("expected an error").code
}

async fn overview(app: &TestApp, who: &Account) -> GetOverviewResponse {
    svc(app, who).get_overview(GetOverviewRequest::default()).await.unwrap().into_owned()
}

async fn me(app: &TestApp, who: &Account) -> User {
    app.client(AuthServiceClient::new, Some(&who.token)).get_me(GetMeRequest::default()).await.unwrap().into_owned().user.unwrap()
}

async fn send(app: &TestApp, who: &Account, channel_id: u64, content: &str, attachment_ids: Vec<u64>) -> Message {
    app.client(MessageServiceClient::new, Some(&who.token))
        .send_message(SendMessageRequest { channel_id, content: content.into(), attachment_ids, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .message
        .unwrap()
}

async fn list_messages(app: &TestApp, who: &Account, channel_id: u64) -> Vec<Message> {
    app.client(MessageServiceClient::new, Some(&who.token))
        .list_messages(ListMessagesRequest { channel_id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .messages
}

/// Uploads a file through `POST /attachments`; returns the pending id.
async fn upload(app: &TestApp, who: &Account, channel_id: u64, filename: &str, content_type: &str, data: &[u8]) -> u64 {
    let addr = app.base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /attachments?channel_id={channel_id}&filename={filename} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Authorization: Bearer {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        who.token,
        data.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(data).await.unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw).await;
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    let json: serde_json::Value = serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap().trim()).unwrap();
    json["id"].as_str().unwrap().parse().unwrap()
}

async fn upload_avatar(app: &TestApp, who: &Account, seed: u8) -> User {
    app.client(MediaServiceClient::new, Some(&who.token))
        .upload_avatar(UploadAvatarRequest { data: png(64, 64, seed), content_type: "image/png".into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .user
        .unwrap()
}

async fn upload_icon(app: &TestApp, who: &Account, guild_id: u64, seed: u8) -> Guild {
    app.client(MediaServiceClient::new, Some(&who.token))
        .upload_guild_icon(UploadGuildIconRequest { guild_id, data: png(80, 80, seed), content_type: "image/png".into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .guild
        .unwrap()
}

async fn create_emoji(app: &TestApp, who: &Account, guild_id: u64, name: &str, seed: u8) -> Emoji {
    app.client(EmojiServiceClient::new, Some(&who.token))
        .create_emoji(CreateEmojiRequest { guild_id, name: name.into(), data: png(32, 32, seed), content_type: "image/png".into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .emoji
        .unwrap()
}

/// Subscribes and waits for the first heartbeat (the subscription is live).
macro_rules! subscribe {
    ($app:expr, $who:expr) => {{
        let mut stream = $app.client(GatewayServiceClient::new, Some(&$who.token)).subscribe(SubscribeRequest::default()).await.unwrap();
        let first = timeout(WAIT, stream.message()).await.unwrap().unwrap().unwrap().to_owned_message();
        assert!(matches!(first.event.into_option().and_then(|e| e.payload), Some(Payload::Heartbeat(_))));
        stream
    }};
}

macro_rules! next_payload {
    ($stream:expr) => {{
        loop {
            let msg = timeout(WAIT, $stream.message()).await.expect("timed out waiting for an event").expect("stream error").expect("stream ended");
            match msg.to_owned_message().event.into_option().and_then(|e| e.payload) {
                Some(Payload::Heartbeat(_)) | None => continue,
                Some(payload) => break payload,
            }
        }
    }};
}

async fn files(app: &TestApp, who: &Account, request: ListFilesRequest) -> ListFilesResponse {
    svc(app, who).list_files(request).await.unwrap().into_owned()
}

async fn audit(app: &TestApp, who: &Account) -> Vec<AuditEntry> {
    svc(app, who).list_audit_log(ListAuditLogRequest::default()).await.unwrap().into_owned().entries
}

async fn object_gone(app: &TestApp, key: &str) -> bool {
    app.state.storage.as_ref().unwrap().head(key).await.unwrap().is_none()
        && media::accounting::totals(&app.state, "global").await.is_ok()
        && app
            .state
            .session
            .execute_unpaged("SELECT key FROM media_objects WHERE key = ?", (key,))
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .rows_num()
            == 0
}

#[tokio::test]
async fn every_call_requires_an_admin() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let anon = app.client(AdminServiceClient::new, None);
    assert_eq!(code(anon.get_overview(GetOverviewRequest::default()).await), ErrorCode::Unauthenticated);
    assert_eq!(code(anon.list_audit_log(ListAuditLogRequest::default()).await), ErrorCode::Unauthenticated);

    let c = svc(&app, &alice);
    let denied = ErrorCode::PermissionDenied;
    assert_eq!(code(c.get_overview(GetOverviewRequest::default()).await), denied);
    assert_eq!(code(c.get_activity(GetActivityRequest::default()).await), denied);
    assert_eq!(code(c.list_guilds(ListGuildsRequest::default()).await), denied);
    assert_eq!(code(c.list_users(ListUsersRequest::default()).await), denied);
    assert_eq!(code(c.set_admin(SetAdminRequest { user_id: alice.id, is_admin: true, ..Default::default() }).await), denied);
    assert_eq!(code(c.list_files(ListFilesRequest::default()).await), denied);
    assert_eq!(code(c.delete_file(DeleteFileRequest { key: "x".into(), ..Default::default() }).await), denied);
    assert_eq!(code(c.list_audit_log(ListAuditLogRequest::default()).await), denied);
    assert!(!me(&app, &alice).await.is_admin);

    let boss = admin_user(&app, "boss").await;
    assert!(me(&app, &boss).await.is_admin);
    overview(&app, &boss).await;
    app.cleanup().await;
}

#[tokio::test]
async fn configured_admins_are_promoted_at_sign_up_and_at_startup() {
    let app = TestApp::builder().admins(&["Boss", " early "]).start().await;
    // Names are normalised like usernames: lower-case, trimmed.
    let early = user(&app, "early").await;
    assert!(me(&app, &early).await.is_admin, "promoted at sign-up");
    let sign_up = app
        .client(AuthServiceClient::new, None)
        .sign_up(SignUpRequest { username: app.name("boss"), password: "correct horse battery".into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned();
    assert!(sign_up.user.as_option().unwrap().is_admin);
    let login = app
        .client(AuthServiceClient::new, None)
        .login(LoginRequest { username: app.name("boss"), password: "correct horse battery".into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned();
    assert!(login.user.as_option().unwrap().is_admin);
    let other = user(&app, "other").await;
    assert!(!me(&app, &other).await.is_admin);

    // A restart re-promotes a configured admin whose flag was cleared.
    admin::set_flag(&app.state, early.id, false).await.unwrap();
    assert!(!me(&app, &early).await.is_admin);
    admin::startup(&app.state).await.unwrap();
    assert!(me(&app, &early).await.is_admin);
    assert!(!me(&app, &other).await.is_admin);
    assert_eq!(admin::parse_admins(" A, ,b ,"), ["a", "b"]);
    app.cleanup().await;
}

#[tokio::test]
async fn is_admin_is_only_visible_to_the_user_and_admins() {
    let app = TestApp::start().await;
    let boss = admin_user(&app, "boss").await;
    let alice = user(&app, "alice").await;
    let g = guild(&app, &boss).await;
    join(&app, &alice, &g).await;
    let members = app
        .client(GuildServiceClient::new, Some(&alice.token))
        .list_guild_members(ListGuildMembersRequest { guild_id: g.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .users;
    assert!(members.iter().all(|u| !u.is_admin), "other users never see the flag");
    let listed = svc(&app, &boss).list_users(ListUsersRequest { query: app.name(""), ..Default::default() }).await.unwrap().into_owned().users;
    let flags: Vec<(String, bool)> = listed.iter().map(|r| (r.user.as_option().unwrap().username.clone(), r.user.as_option().unwrap().is_admin)).collect();
    assert_eq!(flags, [(app.name("boss"), true), (app.name("alice"), false)]);
    app.cleanup().await;
}

#[tokio::test]
async fn overview_and_activity_count_what_happened() {
    let app = TestApp::builder().storage().isolated().start().await;
    let boss = admin_user(&app, "boss").await;
    let alice = user(&app, "alice").await;
    let bob = user(&app, "bob").await;
    let g = guild(&app, &boss).await;
    join(&app, &alice, &g).await;
    let general = channels(&app, &boss, g.id).await.unwrap().channels[0].id;
    channel(&app, &boss, g.id, "extra").await.unwrap();
    send(&app, &boss, general, "one", vec![]).await;
    send(&app, &boss, general, "two", vec![]).await;
    send(&app, &alice, general, "three", vec![]).await;
    // A DM counts as a message but not as a guild message.
    let dm = app
        .client(DmServiceClient::new, Some(&alice.token))
        .open_dm(OpenDmRequest { user_id: bob.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    send(&app, &alice, dm.id, "psst", vec![]).await;
    upload_avatar(&app, &alice, 1).await;
    upload_icon(&app, &boss, g.id, 2).await;

    let o = overview(&app, &boss).await;
    assert_eq!((o.users, o.guilds, o.channels, o.messages), (3, 1, 2, 4));
    assert_eq!((o.messages_today, o.signups_today), (4, 3));
    assert_eq!(o.active_users_today, 2, "boss and alice sent messages; bob did not");
    let storage = o.storage.as_option().unwrap();
    assert!(storage.avatars > 0 && storage.guild_icons > 0);
    assert_eq!((storage.attachments, storage.emojis, storage.embeds), (0, 0, 0));
    assert_eq!(o.storage_bytes_avatars, storage.avatars);
    assert_eq!(o.storage_bytes_guild_icons, storage.guild_icons);
    assert_eq!(o.storage_bytes_total, storage.avatars + storage.guild_icons);
    assert_eq!(o.live_connections, 0);

    // Another message by the same user does not count them twice.
    send(&app, &alice, general, "again", vec![]).await;
    assert_eq!(overview(&app, &boss).await.active_users_today, 2);

    // A gateway stream counts as live and makes bob active.
    let stream = subscribe!(app, bob);
    let o = overview(&app, &boss).await;
    assert_eq!((o.live_connections, o.active_users_today), (1, 3));
    drop(stream);
    for _ in 0..50 {
        if overview(&app, &boss).await.live_connections == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(overview(&app, &boss).await.live_connections, 0);

    // Activity.
    let activity = svc(&app, &boss).get_activity(GetActivityRequest::default()).await.unwrap().into_owned().points;
    assert_eq!(activity.len(), 30);
    let today = stats::today();
    let last = activity.last().unwrap();
    assert_eq!((last.date.as_str(), last.messages, last.signups, last.active_users), (today.as_str(), 5, 3, 3));
    assert!(activity[..29].iter().all(|p| (p.messages, p.signups, p.active_users) == (0, 0, 0)), "gaps are zero-filled");
    assert!(activity.windows(2).all(|w| w[0].date < w[1].date), "oldest first");
    let one = svc(&app, &boss).get_activity(GetActivityRequest { days: 1, ..Default::default() }).await.unwrap().into_owned().points;
    assert_eq!(one.len(), 1);
    assert_eq!(svc(&app, &boss).get_activity(GetActivityRequest { days: 90, ..Default::default() }).await.unwrap().into_owned().points.len(), 90);
    assert_eq!(code(svc(&app, &boss).get_activity(GetActivityRequest { days: 91, ..Default::default() }).await), ErrorCode::InvalidArgument);

    // Per-guild and per-user numbers.
    let guilds = svc(&app, &boss).list_guilds(ListGuildsRequest::default()).await.unwrap().into_owned().guilds;
    assert_eq!(guilds.len(), 1);
    let row = &guilds[0];
    assert_eq!((row.member_count, row.channel_count, row.message_count), (2, 2, 4));
    assert_eq!(row.storage_bytes, storage.guild_icons);
    assert_eq!(row.storage.as_option().unwrap().guild_icons, storage.guild_icons);
    assert_eq!(row.storage.as_option().unwrap().avatars, 0);
    let users = svc(&app, &boss).list_users(ListUsersRequest::default()).await.unwrap().into_owned().users;
    let by_name = |n: &str| users.iter().find(|r| r.user.as_option().unwrap().username == app.name(n)).unwrap();
    assert_eq!((by_name("alice").guild_count, by_name("alice").message_count), (1, 3));
    assert_eq!(by_name("alice").storage_bytes, storage.avatars);
    assert_eq!((by_name("boss").guild_count, by_name("boss").message_count, by_name("boss").storage_bytes), (1, 2, 0));
    assert!(by_name("alice").last_active_at.is_set() && by_name("bob").last_active_at.is_set());
    assert_eq!((by_name("bob").guild_count, by_name("bob").message_count), (0, 0));
    app.cleanup().await;
}

#[tokio::test]
async fn counters_are_rebuilt_from_existing_data_once() {
    let app = TestApp::builder().isolated().start().await;
    let boss = admin_user(&app, "boss").await;
    let alice = user(&app, "alice").await;
    let g = guild(&app, &boss).await;
    join(&app, &alice, &g).await;
    let general = channels(&app, &boss, g.id).await.unwrap().channels[0].id;
    send(&app, &boss, general, "one", vec![]).await;
    send(&app, &alice, general, "two", vec![]).await;
    let before = overview(&app, &boss).await;
    let guilds_before = svc(&app, &boss).list_guilds(ListGuildsRequest::default()).await.unwrap().into_owned().guilds;

    // Counters of an older server: nothing there.
    for table in ["counters", "server_meta", "daily_active"] {
        app.state.session.execute_unpaged(format!("TRUNCATE {table}"), ()).await.unwrap();
    }
    let empty = overview(&app, &boss).await;
    assert_eq!((empty.users, empty.guilds, empty.channels, empty.messages), (0, 0, 0, 0));

    assert!(stats::backfill(&app.state).await.unwrap(), "runs when the marker is missing");
    let after = overview(&app, &boss).await;
    assert_eq!(
        (after.users, after.guilds, after.channels, after.messages, after.messages_today, after.signups_today, after.active_users_today),
        (before.users, before.guilds, before.channels, before.messages, before.messages_today, before.signups_today, before.active_users_today)
    );
    let guilds_after = svc(&app, &boss).list_guilds(ListGuildsRequest::default()).await.unwrap().into_owned().guilds;
    assert_eq!(
        (guilds_after[0].member_count, guilds_after[0].channel_count, guilds_after[0].message_count),
        (guilds_before[0].member_count, guilds_before[0].channel_count, guilds_before[0].message_count)
    );
    assert!(!stats::backfill(&app.state).await.unwrap(), "only once");
    assert_eq!(overview(&app, &boss).await.users, before.users);

    // Activity after the backfill is not counted twice for authors it already counted.
    send(&app, &boss, general, "three", vec![]).await;
    assert_eq!(overview(&app, &boss).await.active_users_today, before.active_users_today);
    app.cleanup().await;
}

#[tokio::test]
async fn guilds_and_users_paginate_and_filter() {
    let app = TestApp::start().await;
    let boss = admin_user(&app, "boss").await;
    let tag = format!("zq{:08x}", rand::random::<u32>());
    let mut ids = Vec::new();
    for i in 0..5 {
        let g = app
            .client(GuildServiceClient::new, Some(&boss.token))
            .create_guild(CreateGuildRequest { name: format!("{tag} Club {i}"), ..Default::default() })
            .await
            .unwrap()
            .into_owned()
            .guild
            .unwrap();
        ids.push(g.id);
    }
    let query = tag.to_uppercase();
    let mut seen = Vec::new();
    let mut token = String::new();
    let mut pages = 0;
    loop {
        let page = svc(&app, &boss)
            .list_guilds(ListGuildsRequest { page_token: token.clone(), limit: 2, query: query.clone(), ..Default::default() })
            .await
            .unwrap()
            .into_owned();
        assert!(page.guilds.len() <= 2);
        seen.extend(page.guilds.iter().map(|r| r.guild.as_option().unwrap().id));
        pages += 1;
        token = page.next_page_token;
        if token.is_empty() {
            break;
        }
    }
    assert_eq!((pages, seen.as_slice()), (3, ids.as_slice()), "ascending by id, case-insensitive filter");
    let none = svc(&app, &boss).list_guilds(ListGuildsRequest { query: format!("{tag}-nothing"), ..Default::default() }).await.unwrap().into_owned();
    assert!(none.guilds.is_empty() && none.next_page_token.is_empty());
    assert_eq!(code(svc(&app, &boss).list_guilds(ListGuildsRequest { page_token: "junk".into(), ..Default::default() }).await), ErrorCode::InvalidArgument);

    // Users: three with a shared tag.
    let names = ["ann", "bea", "cyd"];
    let mut users = Vec::new();
    for n in names {
        users.push(user(&app, n).await);
    }
    let first = svc(&app, &boss).list_users(ListUsersRequest { limit: 2, query: app.name("").to_uppercase(), ..Default::default() }).await.unwrap().into_owned();
    // The admin itself matches too (same tag): 4 users in total.
    assert_eq!(first.users.len(), 2);
    assert!(!first.next_page_token.is_empty());
    let second = svc(&app, &boss)
        .list_users(ListUsersRequest { limit: 2, page_token: first.next_page_token.clone(), query: app.name(""), ..Default::default() })
        .await
        .unwrap()
        .into_owned();
    assert_eq!(second.users.len(), 2);
    assert!(second.next_page_token.is_empty());
    let all: Vec<u64> = first.users.iter().chain(&second.users).map(|r| r.user.as_option().unwrap().id).collect();
    let mut expected: Vec<u64> = users.iter().map(|u| u.id).chain([boss.id]).collect();
    expected.sort();
    assert_eq!(all, expected);
    // Default limit 50 and the cap of 200 do not reject.
    svc(&app, &boss).list_users(ListUsersRequest { limit: 100_000, ..Default::default() }).await.unwrap();
    app.cleanup().await;
}

/// Fabricates the rows of a link embed that owns `thumbnail`, `file` and `poster`.
async fn fake_embed(app: &TestApp, guild_id: u64, channel_id: u64, message_id: u64, author: u64, sizes: [usize; 3]) -> [String; 3] {
    let storage = app.state.storage.as_ref().unwrap();
    let keys = [media::embed_thumbnail_key(channel_id), media::embed_media_key(channel_id, "mp4"), media::embed_thumbnail_key(channel_id)];
    let owner = Owner { kind: "guild", id: guild_id, guild_id: Some(guild_id), label: "embed" };
    for (key, size) in keys.iter().zip(sizes) {
        let ct = media::content_type_of_key(key);
        storage.put(key, vec![7u8; size].into(), ct).await.unwrap();
        let meta = Meta { channel_id: Some(channel_id), message_id: Some(message_id), user_id: Some(author), ..Default::default() };
        media::record_with(&app.state, key, owner, size as u64, meta).await.unwrap();
    }
    let (channel, bucket, message) = (id_to_db(channel_id), bucket_of(message_id), id_to_db(message_id));
    app.state
        .session
        .execute_unpaged(
            "INSERT INTO message_embed_media (channel_id, bucket, message_id, position, idx, kind, file, poster, width, height, size) \
             VALUES (?, ?, ?, 0, 0, 'video', ?, ?, 10, 10, ?)",
            (channel, bucket, message, &keys[1], &keys[2], sizes[1] as i64),
        )
        .await
        .unwrap();
    app.state
        .session
        .execute_unpaged(
            "INSERT INTO message_embeds (channel_id, bucket, message_id, position, url, title, description, site_name, thumbnail, \
             thumbnail_width, thumbnail_height, kind, provider) VALUES (?, ?, ?, 0, 'https://example.com/x', 't', 'd', 's', ?, 5, 5, 'video', 'x')",
            (channel, bucket, message, &keys[0]),
        )
        .await
        .unwrap();
    keys
}

#[tokio::test]
async fn files_are_listed_largest_first_with_guild_and_kind_filters() {
    let app = TestApp::builder().storage().isolated().start().await;
    let boss = admin_user(&app, "boss").await;
    let alice = user(&app, "alice").await;
    let (g1, g2) = (guild(&app, &boss).await, guild(&app, &alice).await);
    let c1 = channels(&app, &boss, g1.id).await.unwrap().channels[0].id;
    let c2 = channels(&app, &alice, g2.id).await.unwrap().channels[0].id;

    // Guild 1: icon, emoji, two attachments (one an image with a thumbnail), an embed with three objects.
    upload_icon(&app, &boss, g1.id, 1).await;
    let emoji = create_emoji(&app, &boss, g1.id, "wave", 2).await;
    let big = vec![b'x'; 70_000];
    let (a_big, a_png) = (upload(&app, &boss, c1, "big.txt", "text/plain", &big).await, upload(&app, &boss, c1, "pic.png", "image/png", &png(300, 200, 3)).await);
    let msg = send(&app, &boss, c1, "files", vec![a_big, a_png]).await;
    let embed_keys = fake_embed(&app, g1.id, c1, msg.id, boss.id, [3000, 9000, 1200]).await;
    // Guild 2 and an avatar.
    upload_icon(&app, &alice, g2.id, 4).await;
    let alice_avatar = upload_avatar(&app, &alice, 5).await.avatar;
    let a2 = upload(&app, &alice, c2, "small.txt", "text/plain", b"hello").await;
    send(&app, &alice, c2, "small", vec![a2]).await;

    let all = files(&app, &boss, ListFilesRequest::default()).await;
    assert!(all.next_page_token.is_empty());
    let sizes: Vec<u64> = all.files.iter().map(|f| f.bytes).collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "largest first: {sizes:?}");
    assert_eq!(all.files[0].filename, "big.txt");
    assert_eq!(all.files.len(), 11, "icon, emoji, 2 files + thumbnail, 3 embed objects, g2 icon, avatar, small file");
    let big_row = &all.files[0];
    assert_eq!((big_row.kind.as_known(), big_row.bytes, big_row.content_type.as_str()), (Some(AdminFileKind::ADMIN_FILE_KIND_ATTACHMENT), 70_000, "text/plain"));
    assert_eq!(big_row.guild.as_option().unwrap().id, g1.id);
    assert_eq!(big_row.owner.as_option().unwrap().id, boss.id);
    assert_eq!((big_row.channel_id, big_row.message_id), (Some(c1), Some(msg.id)));
    assert!(big_row.created_at.is_set());

    // Guild filter: nothing of guild 2 or the avatar.
    let g1_files = files(&app, &boss, ListFilesRequest { guild_id: Some(g1.id), ..Default::default() }).await;
    assert!(g1_files.files.iter().all(|f| f.guild.as_option().map(|g| g.id) == Some(g1.id)));
    assert_eq!(g1_files.files.len(), 1 + 1 + 3 + 3);
    let g2_files = files(&app, &boss, ListFilesRequest { guild_id: Some(g2.id), ..Default::default() }).await;
    assert_eq!(g2_files.files.len(), 2, "icon and the small file");

    // Kind filters.
    let kind = |k: AdminFileKind, guild_id: Option<u64>| ListFilesRequest { kind: Some(k.into()), guild_id, ..Default::default() };
    let attachments = files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_ATTACHMENT, None)).await.files;
    assert_eq!(attachments.len(), 4, "two files, one thumbnail, one DM-less small file");
    assert!(attachments.iter().all(|f| f.kind.as_known() == Some(AdminFileKind::ADMIN_FILE_KIND_ATTACHMENT)));
    let thumb = attachments.iter().find(|f| f.content_type == "image/webp").expect("the thumbnail");
    assert_eq!(thumb.filename, "pic.png");
    let avatars = files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_AVATAR, None)).await.files;
    assert_eq!(avatars.len(), 1);
    assert_eq!((avatars[0].key.as_str(), avatars[0].owner.as_option().unwrap().id, avatars[0].guild.is_set()), (alice_avatar.as_str(), alice.id, false));
    assert!(files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_AVATAR, Some(g1.id))).await.files.is_empty(), "avatars belong to no guild");
    let emojis = files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_EMOJI, Some(g1.id))).await.files;
    assert_eq!((emojis.len(), emojis[0].key.as_str()), (1, emoji.image.as_str()));
    assert_eq!(emojis[0].owner.as_option().unwrap().id, boss.id, "creator");
    assert_eq!(files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_GUILD_ICON, None)).await.files.len(), 2);
    let embeds = files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_EMBED_THUMBNAIL, Some(g1.id))).await.files;
    assert_eq!(embeds.iter().map(|f| f.bytes).collect::<Vec<_>>(), [9000, 3000, 1200]);
    assert_eq!(files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_EMBED_THUMBNAIL, Some(g2.id))).await.files.len(), 0);
    assert_eq!(embeds[0].key, embed_keys[1]);
    assert_eq!(embeds[0].content_type, "video/mp4");
    // UNSPECIFIED = all kinds.
    assert_eq!(files(&app, &boss, kind(AdminFileKind::ADMIN_FILE_KIND_UNSPECIFIED, None)).await.files.len(), all.files.len());

    // Paging in twos gives the same order.
    let mut paged = Vec::new();
    let mut token = String::new();
    loop {
        let page = files(&app, &boss, ListFilesRequest { page_token: token, limit: 2, ..Default::default() }).await;
        assert!(page.files.len() <= 2);
        paged.extend(page.files.into_iter().map(|f| f.key));
        token = page.next_page_token;
        if token.is_empty() {
            break;
        }
    }
    assert_eq!(paged, all.files.iter().map(|f| f.key.clone()).collect::<Vec<_>>());
    assert_eq!(code(svc(&app, &boss).list_files(ListFilesRequest { page_token: "nope".into(), ..Default::default() }).await), ErrorCode::InvalidArgument);

    // The index can be rebuilt from media_objects.
    app.state.session.execute_unpaged("TRUNCATE files_by_size", ()).await.unwrap();
    assert!(files(&app, &boss, ListFilesRequest::default()).await.files.is_empty());
    assert_eq!(media::files::backfill_index(&app.state).await.unwrap(), all.files.len());
    let rebuilt = files(&app, &boss, ListFilesRequest::default()).await;
    assert_eq!(rebuilt.files.iter().map(|f| f.key.clone()).collect::<Vec<_>>(), paged);
    assert_eq!(media::files::backfill_index(&app.state).await.unwrap(), 0, "not when the index has rows");
    app.cleanup().await;
}

#[tokio::test]
async fn deleting_an_attachment_marks_it_removed_and_publishes_message_updated() {
    let app = TestApp::start_with_storage().await;
    let boss = admin_user(&app, "boss").await;
    let alice = user(&app, "alice").await;
    let g = guild(&app, &boss).await;
    join(&app, &alice, &g).await;
    let channel_id = channels(&app, &boss, g.id).await.unwrap().channels[0].id;
    let id = upload(&app, &alice, channel_id, "pic.png", "image/png", &png(300, 100, 6)).await;
    let msg = send(&app, &alice, channel_id, "look", vec![id]).await;
    let (file, thumb) = (msg.attachments[0].file.clone(), msg.attachments[0].thumbnail.clone().expect("thumbnail"));
    let storage = app.state.storage.clone().unwrap();
    let guild_total = |app: &TestApp| {
        let state = app.state.clone();
        let scope = format!("guild:{}:attachment", g.id);
        async move { media::totals(&state, &scope).await.unwrap() }
    };
    assert_eq!(guild_total(&app).await.1, 2);
    let mut stream = subscribe!(app, alice);

    svc(&app, &boss).delete_file(DeleteFileRequest { key: file.clone(), reason: "illegal".into(), ..Default::default() }).await.unwrap();
    let Payload::MessageUpdated(updated) = next_payload!(stream) else { panic!("expected message_updated") };
    let updated = updated.message.into_option().unwrap();
    assert_eq!(updated.id, msg.id);
    let a = &updated.attachments[0];
    assert!(a.removed && a.file.is_empty() && a.thumbnail.is_none(), "{a:?}");
    assert_eq!((a.filename.as_str(), a.content_type.as_str()), ("pic.png", "image/png"), "the metadata stays");
    let listed = &list_messages(&app, &alice, channel_id).await[0].attachments[0];
    assert!(listed.removed && listed.file.is_empty());
    // The thumbnail went with it.
    for key in [&file, &thumb] {
        assert!(storage.head(key).await.unwrap().is_none());
        assert!(object_gone(&app, key).await);
    }
    assert_eq!(guild_total(&app).await, (0, 0));
    assert_eq!(code(svc(&app, &boss).delete_file(DeleteFileRequest { key: file.clone(), ..Default::default() }).await), ErrorCode::NotFound);

    let log = audit(&app, &boss).await;
    let entry = log.iter().find(|e| e.target == file).expect("audit entry");
    assert_eq!((entry.action.as_str(), entry.reason.as_str(), entry.actor.as_option().unwrap().id), ("file.delete", "illegal", boss.id));
    assert!(entry.actor.as_option().unwrap().is_admin);
    app.cleanup().await;
}

#[tokio::test]
async fn deleting_embed_objects_clears_them_and_publishes_message_updated() {
    let app = TestApp::start_with_storage().await;
    let boss = admin_user(&app, "boss").await;
    let g = guild(&app, &boss).await;
    let channel_id = channels(&app, &boss, g.id).await.unwrap().channels[0].id;
    let msg = send(&app, &boss, channel_id, "https://example.com/x", vec![]).await;
    let [thumbnail, video, poster] = fake_embed(&app, g.id, channel_id, msg.id, boss.id, [3000, 9000, 1200]).await;
    let storage = app.state.storage.clone().unwrap();
    let mut stream = subscribe!(app, boss);
    let embed = |m: &Message| m.embeds[0].clone();
    let before = list_messages(&app, &boss, channel_id).await.remove(0);
    assert_eq!(embed(&before).thumbnail.as_deref(), Some(thumbnail.as_str()));
    assert_eq!(embed(&before).media.len(), 1);

    // The thumbnail.
    svc(&app, &boss).delete_file(DeleteFileRequest { key: thumbnail.clone(), ..Default::default() }).await.unwrap();
    let Payload::MessageUpdated(u) = next_payload!(stream) else { panic!() };
    let m = u.message.into_option().unwrap();
    assert_eq!((embed(&m).thumbnail, embed(&m).media.len()), (None, 1));
    assert!(object_gone(&app, &thumbnail).await);

    // The poster: the video stays, without a poster.
    svc(&app, &boss).delete_file(DeleteFileRequest { key: poster.clone(), ..Default::default() }).await.unwrap();
    let Payload::MessageUpdated(u) = next_payload!(stream) else { panic!() };
    let m = u.message.into_option().unwrap();
    assert_eq!(embed(&m).media.len(), 1);
    assert!(embed(&m).media[0].poster.is_none() || embed(&m).media[0].poster.as_deref() == Some(""));
    assert!(object_gone(&app, &poster).await);
    assert!(storage.head(&video).await.unwrap().is_some());

    // The video itself: the media item disappears.
    svc(&app, &boss).delete_file(DeleteFileRequest { key: video.clone(), reason: "nsfw".into(), ..Default::default() }).await.unwrap();
    let Payload::MessageUpdated(u) = next_payload!(stream) else { panic!() };
    assert!(embed(&u.message.into_option().unwrap()).media.is_empty());
    assert!(object_gone(&app, &video).await);
    assert_eq!(media::totals(&app.state, &format!("guild:{}:embed", g.id)).await.unwrap(), (0, 0));
    assert_eq!(audit(&app, &boss).await.iter().filter(|e| [&thumbnail, &video, &poster].contains(&&e.target)).count(), 3);
    app.cleanup().await;
}

#[tokio::test]
async fn deleting_an_emoji_image_deletes_the_emoji_and_resets_channel_icons() {
    let app = TestApp::start_with_storage().await;
    let boss = admin_user(&app, "boss").await;
    let g = guild(&app, &boss).await;
    let emoji = create_emoji(&app, &boss, g.id, "wave", 7).await;
    let ch = channels(&app, &boss, g.id).await.unwrap().channels[0].clone();
    app.client(GuildServiceClient::new, Some(&boss.token))
        .update_channel(UpdateChannelRequest {
            guild_id: g.id,
            channel_id: ch.id,
            icon_ref: ChannelIcon { kind: Some(channel_icon::Kind::CustomEmojiId(emoji.id)), ..Default::default() }.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut stream = subscribe!(app, boss);

    svc(&app, &boss).delete_file(DeleteFileRequest { key: emoji.image.clone(), ..Default::default() }).await.unwrap();
    let mut saw_channel_reset = false;
    let list = loop {
        match next_payload!(stream) {
            Payload::ChannelUpdated(c) => {
                assert_eq!(c.id, ch.id);
                saw_channel_reset = true;
            }
            Payload::EmojisUpdated(e) => break e,
            _ => {}
        }
    };
    assert!(saw_channel_reset);
    assert_eq!(list.guild_id, g.id);
    assert!(list.emojis.is_empty());
    assert!(object_gone(&app, &emoji.image).await);
    assert_eq!(media::totals(&app.state, &format!("guild:{}", g.id)).await.unwrap(), (0, 0));
    let listed = app
        .client(EmojiServiceClient::new, Some(&boss.token))
        .list_emojis(ListEmojisRequest { guild_id: g.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .emojis;
    assert!(listed.is_empty());
    app.cleanup().await;
}

#[tokio::test]
async fn deleting_a_guild_icon_or_avatar_clears_it_and_publishes_updates() {
    let app = TestApp::start_with_storage().await;
    let boss = admin_user(&app, "boss").await;
    let alice = user(&app, "alice").await;
    let g = guild(&app, &boss).await;
    join(&app, &alice, &g).await;
    let icon = upload_icon(&app, &boss, g.id, 8).await.icon;
    let avatar = upload_avatar(&app, &alice, 9).await.avatar;
    assert!(!icon.is_empty() && !avatar.is_empty());
    let mut stream = subscribe!(app, boss);

    svc(&app, &boss).delete_file(DeleteFileRequest { key: icon.clone(), reason: "x".into(), ..Default::default() }).await.unwrap();
    let Payload::GuildUpdated(updated) = next_payload!(stream) else { panic!("expected guild_updated") };
    assert_eq!((updated.id, updated.icon.as_str()), (g.id, ""));
    assert!(object_gone(&app, &icon).await);

    svc(&app, &boss).delete_file(DeleteFileRequest { key: avatar.clone(), ..Default::default() }).await.unwrap();
    // Boss shares the guild with alice, so boss hears about alice's new profile, without the admin flag.
    let Payload::UserUpdated(updated) = next_payload!(stream) else { panic!("expected user_updated") };
    let updated = *updated;
    assert_eq!((updated.id, updated.avatar.as_str(), updated.is_admin), (alice.id, "", false));
    assert!(object_gone(&app, &avatar).await);
    assert_eq!(me(&app, &alice).await.avatar, "");
    assert_eq!(media::totals(&app.state, &format!("guild:{}", g.id)).await.unwrap(), (0, 0));
    assert_eq!(code(svc(&app, &boss).delete_file(DeleteFileRequest { key: "avatars/1/nope.webp".into(), ..Default::default() }).await), ErrorCode::NotFound);
    app.cleanup().await;
}

#[tokio::test]
async fn set_admin_rules_and_audit_log() {
    let app = TestApp::start().await;
    let boss = admin_user(&app, "boss").await;
    let second = user(&app, "second").await;
    let plain = user(&app, "plain").await;
    let c = svc(&app, &boss);

    // Unknown user.
    assert_eq!(code(c.set_admin(SetAdminRequest { user_id: 1, is_admin: true, ..Default::default() }).await), ErrorCode::NotFound);
    // Grant.
    let granted = c.set_admin(SetAdminRequest { user_id: second.id, is_admin: true, ..Default::default() }).await.unwrap().into_owned().user.unwrap();
    assert!(granted.is_admin && granted.id == second.id);
    assert!(me(&app, &second).await.is_admin);
    overview(&app, &second).await;
    // Granting again is a no-op that still answers.
    assert!(c.set_admin(SetAdminRequest { user_id: second.id, is_admin: true, ..Default::default() }).await.unwrap().into_owned().user.unwrap().is_admin);
    // Cannot demote yourself, even with another admin around.
    let own = svc(&app, &second).set_admin(SetAdminRequest { user_id: second.id, is_admin: false, ..Default::default() }).await;
    assert_eq!(code(own), ErrorCode::FailedPrecondition);
    // Demote another admin.
    let revoked = svc(&app, &second).set_admin(SetAdminRequest { user_id: boss.id, is_admin: false, ..Default::default() }).await.unwrap().into_owned().user.unwrap();
    assert!(!revoked.is_admin);
    assert_eq!(code(c.get_overview(GetOverviewRequest::default()).await), ErrorCode::PermissionDenied);
    assert_eq!(code(svc(&app, &plain).set_admin(SetAdminRequest { user_id: plain.id, is_admin: true, ..Default::default() }).await), ErrorCode::PermissionDenied);
    app.cleanup().await;

    // The last admin cannot be demoted, not even by someone who is not one.
    let app = TestApp::builder().isolated().start().await;
    let only = admin_user(&app, "only").await;
    let other = user(&app, "other").await;
    assert_eq!(code(svc(&app, &only).set_admin(SetAdminRequest { user_id: only.id, is_admin: false, ..Default::default() }).await), ErrorCode::FailedPrecondition);
    svc(&app, &only).set_admin(SetAdminRequest { user_id: other.id, is_admin: true, ..Default::default() }).await.unwrap();
    // Concurrent mutual demotions leave one admin.
    let (c_only, c_other) = (svc(&app, &only), svc(&app, &other));
    let (a, b) = tokio::join!(
        c_only.set_admin(SetAdminRequest { user_id: other.id, is_admin: false, ..Default::default() }),
        c_other.set_admin(SetAdminRequest { user_id: only.id, is_admin: false, ..Default::default() }),
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
    assert_eq!(admin::admin_ids(&app.state).await.unwrap().len(), 1);

    // The audit log lists newest first, with pagination.
    let winner = if a_is_ok_marker(&app, &only).await { only } else { other };
    let log = audit(&app, &winner).await;
    let actions: Vec<(&str, &str)> = log.iter().map(|e| (e.action.as_str(), e.target.as_str())).collect();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[1].0, "admin.grant");
    assert_eq!(actions[0].0, "admin.revoke");
    assert!(log[0].id > log[1].id && log[0].created_at.is_set());
    let first = svc(&app, &winner).list_audit_log(ListAuditLogRequest { limit: 1, ..Default::default() }).await.unwrap().into_owned();
    assert_eq!((first.entries.len(), first.entries[0].id), (1, log[0].id));
    assert!(!first.next_page_token.is_empty());
    let second = svc(&app, &winner).list_audit_log(ListAuditLogRequest { limit: 1, page_token: first.next_page_token, ..Default::default() }).await.unwrap().into_owned();
    assert_eq!((second.entries.len(), second.entries[0].id, second.next_page_token.as_str()), (1, log[1].id, ""));
    assert_eq!(code(svc(&app, &winner).list_audit_log(ListAuditLogRequest { page_token: "x".into(), ..Default::default() }).await), ErrorCode::InvalidArgument);
    app.cleanup().await;
}

/// Whether `who` is still an admin.
async fn a_is_ok_marker(app: &TestApp, who: &Account) -> bool {
    me(app, who).await.is_admin
}
