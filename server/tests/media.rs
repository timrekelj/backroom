//! Avatars and guild icons against the real S3-compatible service from
//! `docker-compose.yml`: `docker compose up -d` must be running (the tests
//! panic with a clear message if the object store is unreachable).
mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat, Rgba, RgbaImage};
use thehideout_server::media;
use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

fn sample(w: u32, h: u32, seed: u8) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| Rgba([(x as u8).wrapping_add(seed), (y as u8).wrapping_mul(3), seed, 255]))
}

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(sample(w, h, seed).as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let rgb = DynamicImage::ImageRgba8(sample(w, h, 9)).into_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new(&mut out).write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8).unwrap();
    out
}

fn gif(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    GifEncoder::new(&mut out).encode(sample(w, h, 5).as_raw(), w, h, ExtendedColorType::Rgba8).unwrap();
    out
}

fn webp(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(sample(w, h, 7).as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn media_client(app: &TestApp, who: &Account) -> MediaServiceClient<connectrpc::client::HttpClient> {
    app.client(MediaServiceClient::new, Some(&who.token))
}

async fn upload_avatar(app: &TestApp, who: &Account, data: Vec<u8>, content_type: &str) -> Result<User, ConnectError> {
    Ok(media_client(app, who)
        .upload_avatar(UploadAvatarRequest {
            data,
            content_type: content_type.into(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .user
        .unwrap())
}

async fn upload_icon(app: &TestApp, who: &Account, guild_id: u64, data: Vec<u8>) -> Result<Guild, ConnectError> {
    Ok(media_client(app, who)
        .upload_guild_icon(UploadGuildIconRequest {
            guild_id,
            data,
            content_type: "image/png".into(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .guild
        .unwrap())
}

struct HttpReply {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// Minimal raw HTTP/1.1 request, so odd paths are sent exactly as written.
async fn http(app: &TestApp, method: &str, path: &str, extra: &[(&str, &str)]) -> HttpReply {
    let addr = app.base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in extra {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("header end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    HttpReply {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

fn assert_code<T: std::fmt::Debug>(result: Result<T, ConnectError>, code: ErrorCode) {
    match result {
        Err(e) => assert_eq!(e.code, code, "{e:?}"),
        Ok(v) => panic!("expected {code:?}, got Ok({v:?})"),
    }
}

#[tokio::test]
async fn uploads_of_every_format_are_stored_as_256_webp() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let inputs = [
        (png(300, 120, 1), "image/png"),
        (jpeg(80, 200), "image/jpeg"),
        (gif(33, 33), "image/gif"),
        (webp(10, 10), "image/webp"),
    ];
    for (data, content_type) in inputs {
        let user = upload_avatar(&app, &alice, data, content_type).await.expect(content_type);
        assert!(user.avatar.starts_with(&format!("avatars/{}/", alice.id)), "{}", user.avatar);
        assert!(user.avatar.ends_with(".webp"));

        let reply = http(&app, "GET", &format!("/media/{}", user.avatar), &[]).await;
        assert_eq!(reply.status, 200, "{content_type}");
        assert_eq!(reply.headers["content-type"], "image/webp");
        assert_eq!(reply.headers["cache-control"], "public, max-age=31536000, immutable");
        assert_eq!(reply.headers["content-length"], reply.body.len().to_string());
        let hash = user.avatar.rsplit('/').next().unwrap().trim_end_matches(".webp");
        assert_eq!(reply.headers["etag"], format!("\"{hash}\""));
        let img = image::load_from_memory_with_format(&reply.body, ImageFormat::WebP).unwrap();
        assert_eq!((img.width(), img.height()), (256, 256), "{content_type}");

        let head = http(&app, "HEAD", &format!("/media/{}", user.avatar), &[]).await;
        assert_eq!(head.status, 200);
        assert!(head.body.is_empty());
    }
    // The avatar shows up wherever the user does.
    let me = app.client(AuthServiceClient::new, Some(&alice.token)).get_me(GetMeRequest::default()).await.unwrap().into_owned();
    assert!(!me.user.as_option().unwrap().avatar.is_empty());
    app.cleanup().await;
}

#[tokio::test]
async fn conditional_get_returns_304() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let key = upload_avatar(&app, &alice, png(16, 16, 1), "image/png").await.unwrap().avatar;
    let first = http(&app, "GET", &format!("/media/{key}"), &[]).await;
    let etag = first.headers["etag"].clone();
    let second = http(&app, "GET", &format!("/media/{key}"), &[("If-None-Match", &etag)]).await;
    assert_eq!(second.status, 304);
    assert!(second.body.is_empty());
    assert_eq!(second.headers["cache-control"], "public, max-age=31536000, immutable");
    app.cleanup().await;
}

#[tokio::test]
async fn bad_uploads_are_invalid_argument() {
    let app = TestApp::builder().storage().isolated().start().await;
    let alice = user(&app, "alice").await;
    let invalid = ErrorCode::InvalidArgument;
    assert_code(upload_avatar(&app, &alice, b"definitely not an image".to_vec(), "image/png").await, invalid);
    assert_code(upload_avatar(&app, &alice, vec![], "image/png").await, invalid);
    assert_code(upload_avatar(&app, &alice, png(8, 8, 1), "image/bmp").await, invalid);
    assert_code(upload_avatar(&app, &alice, png(8, 8, 1), "image/jpeg").await, invalid); // type mismatch
    let mut truncated = png(64, 64, 1);
    truncated.truncate(truncated.len() / 2);
    assert_code(upload_avatar(&app, &alice, truncated, "image/png").await, invalid);
    // Just over 8 MiB.
    assert_code(upload_avatar(&app, &alice, vec![0; 8 * 1024 * 1024 + 1], "image/png").await, invalid);
    // Decompression-bomb-ish dimensions in a tiny file.
    assert_code(upload_avatar(&app, &alice, png(9000, 1, 1), "image/png").await, invalid);
    assert_code(upload_avatar(&app, &alice, png(1, 9000, 1), "image/png").await, invalid);
    // Nothing was stored.
    let (bytes, objects) = media::totals(&app.state, "global").await.unwrap();
    assert_eq!((bytes, objects), (0, 0));
    app.cleanup().await;
}

#[tokio::test]
async fn far_oversized_body_is_refused_cleanly() {
    let app = TestApp::builder().storage().isolated().start().await;
    let alice = user(&app, "alice").await;
    // Over the media RPCs' transport limit (8 MiB plus base64 and field overhead): refused before
    // any processing. (Bodies far beyond it get the connection reset by the server instead.)
    let result = upload_avatar(&app, &alice, vec![0; 12 * 1024 * 1024], "image/png").await;
    let err = result.expect_err("must be rejected");
    // Usually a clean error; if the server closes the connection while the client is still
    // sending, the client reports a transport failure instead.
    let transport_reset = err.code == ErrorCode::Internal
        && err.message.as_deref().is_some_and(|m| m.contains("failed to read response body"));
    assert!(
        matches!(err.code, ErrorCode::InvalidArgument | ErrorCode::ResourceExhausted) || transport_reset,
        "{err:?}"
    );
    assert_eq!(media::totals(&app.state, "global").await.unwrap(), (0, 0));
    app.cleanup().await;
}

#[tokio::test]
async fn other_rpcs_keep_a_small_body_limit() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let result = app
        .client(GuildServiceClient::new, Some(&alice.token))
        .create_guild(CreateGuildRequest {
            name: "x".repeat(5 * 1024 * 1024),
            ..Default::default()
        })
        .await;
    assert!(result.is_err());
    app.cleanup().await;
}

#[tokio::test]
async fn replacing_and_removing_an_avatar_deletes_objects_and_updates_counters() {
    let app = TestApp::builder().storage().isolated().start().await;
    let storage = app.state.storage.clone().unwrap();
    let alice = user(&app, "alice").await;

    let first = upload_avatar(&app, &alice, png(64, 64, 1), "image/png").await.unwrap().avatar;
    let (bytes1, objects) = media::totals(&app.state, "global").await.unwrap();
    assert_eq!(objects, 1);
    assert_eq!(bytes1, storage.head(&first).await.unwrap().unwrap().size as i64);

    // Re-uploading identical content yields the same key and no double count.
    let again = upload_avatar(&app, &alice, png(64, 64, 1), "image/png").await.unwrap().avatar;
    assert_eq!(again, first);
    assert_eq!(media::totals(&app.state, "global").await.unwrap(), (bytes1, 1));
    assert!(storage.head(&first).await.unwrap().is_some());

    let second = upload_avatar(&app, &alice, png(64, 64, 2), "image/png").await.unwrap().avatar;
    assert_ne!(second, first);
    assert!(storage.head(&first).await.unwrap().is_none(), "old object must be deleted");
    assert_eq!(http(&app, "GET", &format!("/media/{first}"), &[]).await.status, 404);
    assert_eq!(http(&app, "GET", &format!("/media/{second}"), &[]).await.status, 200);
    let (bytes2, objects) = media::totals(&app.state, "global").await.unwrap();
    assert_eq!(objects, 1);
    assert_eq!(bytes2, storage.head(&second).await.unwrap().unwrap().size as i64);

    let removed = media_client(&app, &alice)
        .remove_avatar(RemoveAvatarRequest::default())
        .await
        .unwrap()
        .into_owned()
        .user
        .unwrap();
    assert_eq!(removed.avatar, "");
    assert!(storage.head(&second).await.unwrap().is_none());
    assert_eq!(media::totals(&app.state, "global").await.unwrap(), (0, 0));
    // Removing again is a no-op.
    let again = media_client(&app, &alice).remove_avatar(RemoveAvatarRequest::default()).await.unwrap().into_owned();
    assert_eq!(again.user.as_option().unwrap().avatar, "");
    let me = app.client(AuthServiceClient::new, Some(&alice.token)).get_me(GetMeRequest::default()).await.unwrap().into_owned();
    assert_eq!(me.user.as_option().unwrap().avatar, "");
    app.cleanup().await;
}

#[tokio::test]
async fn guild_icon_needs_manage_server_and_is_counted_per_guild() {
    let app = TestApp::builder().storage().isolated().start().await;
    let owner = user(&app, "owner").await;
    let member = user(&app, "member").await;
    let outsider = user(&app, "outsider").await;
    let guild = guild(&app, &owner).await;
    join(&app, &member, &guild).await;

    assert_code(upload_icon(&app, &member, guild.id, png(32, 32, 1)).await, ErrorCode::PermissionDenied);
    assert_code(upload_icon(&app, &outsider, guild.id, png(32, 32, 1)).await, ErrorCode::NotFound);
    assert_code(
        media_client(&app, &member).remove_guild_icon(RemoveGuildIconRequest { guild_id: guild.id, ..Default::default() }).await,
        ErrorCode::PermissionDenied,
    );

    // A role with MANAGE_SERVER is enough.
    let role = role(&app, &owner, guild.id, "mods", MANAGE_SERVER).await.unwrap();
    set_roles(&app, &owner, guild.id, member.id, &[role.id]).await.unwrap();
    let updated = upload_icon(&app, &member, guild.id, png(32, 32, 1)).await.unwrap();
    assert!(updated.icon.starts_with(&format!("guild-icons/{}/", guild.id)));
    assert_eq!(http(&app, "GET", &format!("/media/{}", updated.icon), &[]).await.status, 200);

    let listed = guilds(&app, &owner).list_my_guilds(ListMyGuildsRequest::default()).await.unwrap().into_owned();
    assert_eq!(listed.guilds[0].icon, updated.icon);
    let (guild_bytes, guild_objects) = media::totals(&app.state, &format!("guild:{}", guild.id)).await.unwrap();
    assert_eq!(guild_objects, 1);
    assert!(guild_bytes > 0);
    assert_eq!(media::totals(&app.state, "global").await.unwrap(), (guild_bytes, 1));

    let removed = media_client(&app, &owner)
        .remove_guild_icon(RemoveGuildIconRequest { guild_id: guild.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .guild
        .unwrap();
    assert_eq!(removed.icon, "");
    assert_eq!(http(&app, "GET", &format!("/media/{}", updated.icon), &[]).await.status, 404);
    assert_eq!(media::totals(&app.state, &format!("guild:{}", guild.id)).await.unwrap(), (0, 0));
    app.cleanup().await;
}

macro_rules! next_payload {
    ($stream:expr) => {{
        loop {
            let msg = timeout(WAIT, $stream.message())
                .await
                .expect("timed out waiting for an event")
                .expect("stream error")
                .expect("stream ended");
            match msg.to_owned_message().event.into_option().expect("event").payload {
                Some(Payload::Heartbeat(_)) | None => continue,
                Some(p) => break p,
            }
        }
    }};
}

#[tokio::test]
async fn events_reach_guild_mates_dm_partners_and_members() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let bob = user(&app, "bob").await; // shares a guild with alice
    let carol = user(&app, "carol").await; // DM only
    let dave = user(&app, "dave").await; // unrelated
    let g = guild(&app, &alice).await;
    join(&app, &bob, &g).await;
    app.client(DmServiceClient::new, Some(&alice.token))
        .open_dm(OpenDmRequest { user_id: carol.id, ..Default::default() })
        .await
        .expect("open dm");

    let mut bob_stream = app.client(GatewayServiceClient::new, Some(&bob.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    let mut carol_stream = app.client(GatewayServiceClient::new, Some(&carol.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    let mut dave_stream = app.client(GatewayServiceClient::new, Some(&dave.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    let mut alice_stream = app.client(GatewayServiceClient::new, Some(&alice.token)).subscribe(SubscribeRequest::default()).await.unwrap();

    let key = upload_avatar(&app, &alice, png(40, 40, 3), "image/png").await.unwrap().avatar;
    for stream in [&mut bob_stream, &mut carol_stream, &mut alice_stream] {
        match next_payload!(stream) {
            Payload::UserUpdated(u) => assert_eq!((u.id, u.avatar.as_str()), (alice.id, key.as_str())),
            other => panic!("expected user_updated, got {other:?}"),
        }
    }

    let icon = upload_icon(&app, &alice, g.id, png(40, 40, 4)).await.unwrap().icon;
    for stream in [&mut bob_stream, &mut alice_stream] {
        match next_payload!(stream) {
            Payload::GuildUpdated(gu) => assert_eq!((gu.id, gu.icon.as_str()), (g.id, icon.as_str())),
            other => panic!("expected guild_updated, got {other:?}"),
        }
    }

    // Dave shares nothing with alice and sees neither event.
    let leaked = timeout(Duration::from_millis(600), async {
        loop {
            let msg = dave_stream.message().await.unwrap().unwrap();
            if !matches!(msg.to_owned_message().event.into_option().unwrap().payload, Some(Payload::Heartbeat(_)) | None) {
                return true;
            }
        }
    })
    .await;
    assert!(leaked.is_err(), "unrelated user received an event");
    app.cleanup().await;
}

#[tokio::test]
async fn avatar_is_carried_in_members_and_dm_participants() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let bob = user(&app, "bob").await;
    let g = guild(&app, &alice).await;
    join(&app, &bob, &g).await;
    let key = upload_avatar(&app, &alice, png(40, 40, 3), "image/png").await.unwrap().avatar;

    let members = guilds(&app, &bob)
        .list_guild_members(ListGuildMembersRequest { guild_id: g.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned();
    assert_eq!(members.users.iter().find(|u| u.id == alice.id).unwrap().avatar, key);

    let dm = app
        .client(DmServiceClient::new, Some(&bob.token))
        .open_dm(OpenDmRequest { user_id: alice.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned();
    let dm = dm.channel.unwrap();
    assert_eq!(dm.participants.iter().find(|u| u.id == alice.id).unwrap().avatar, key);
    app.cleanup().await;
}

#[tokio::test]
async fn media_route_rejects_unknown_and_malicious_keys() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let key = upload_avatar(&app, &alice, png(16, 16, 1), "image/png").await.unwrap().avatar;
    assert_eq!(http(&app, "GET", &format!("/media/{key}"), &[]).await.status, 200);

    let well_formed_unknown = "/media/avatars/1/00000000000000000000000000000000.webp";
    assert_eq!(http(&app, "GET", well_formed_unknown, &[]).await.status, 404);
    for path in [
        "/media/",
        "/media/avatars",
        "/media/..",
        "/media/../health",
        "/media/avatars/../../etc/passwd",
        "/media/avatars/%2e%2e/%2e%2e/etc/passwd",
        "/media/avatars%2F1%2F00000000000000000000000000000000.webp",
        &format!("/media/{key}/extra"),
        &format!("/media/{key}.png"),
        &format!("/media/{}?x=1", key.replace("avatars", "secrets")),
        "/media/avatars/1/.healthcheck",
        "/media/avatars/1/0000000000000000000000000000000%00.webp",
    ] {
        let status = http(&app, "GET", path, &[]).await.status;
        assert!(status == 404 || status == 400, "{path} -> {status}");
    }
    // A key with a query string is the same key.
    assert_eq!(http(&app, "GET", &format!("/media/{key}?v=1"), &[]).await.status, 200);
    app.cleanup().await;
}

#[tokio::test]
async fn uploads_are_rate_limited_per_user() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    for i in 0..10 {
        upload_avatar(&app, &alice, png(8, 8, i), "image/png").await.expect("within limit");
    }
    assert_code(upload_avatar(&app, &alice, png(8, 8, 99), "image/png").await, ErrorCode::ResourceExhausted);
    app.cleanup().await;
}

#[tokio::test]
async fn media_rpcs_are_unavailable_without_storage_but_the_rest_works() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    assert_code(upload_avatar(&app, &alice, png(8, 8, 1), "image/png").await, ErrorCode::Unavailable);
    assert_eq!(http(&app, "GET", "/media/avatars/1/00000000000000000000000000000000.webp", &[]).await.status, 503);
    guild(&app, &alice).await;
    assert_eq!(http(&app, "GET", "/health", &[]).await.status, 200);
    app.cleanup().await;
}

#[tokio::test]
async fn unauthenticated_uploads_are_refused() {
    let app = TestApp::start_with_storage().await;
    let result = app
        .client(MediaServiceClient::new, None)
        .upload_avatar(UploadAvatarRequest { data: png(8, 8, 1), content_type: "image/png".into(), ..Default::default() })
        .await;
    assert_eq!(result.err().unwrap().code, ErrorCode::Unauthenticated);
    app.cleanup().await;
}
