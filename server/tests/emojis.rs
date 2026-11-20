//! Custom emojis against the real S3-compatible service from
//! `docker-compose.yml` (`docker compose up -d` must be running).
mod common;

use std::collections::HashMap;
use std::io::Cursor;
use std::time::Duration;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::png::PngEncoder;
use image::{AnimationDecoder, Delay, ExtendedColorType, Frame, ImageEncoder, ImageFormat, Rgba, RgbaImage};
use thehideout_server::convert::id_to_db;
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

/// An animated GIF with `frames` frames of 100 ms (frame i has a distinct colour).
fn animated_gif(w: u32, h: u32, frames: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.set_repeat(Repeat::Infinite).unwrap();
        let frames = (0..frames).map(|i| {
            let img = RgbaImage::from_pixel(w, h, Rgba([(i * 40 % 256) as u8, 100, 200, 255]));
            Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1))
        });
        enc.encode_frames(frames).unwrap();
    }
    out
}

fn emojis(app: &TestApp, who: &Account) -> EmojiServiceClient<connectrpc::client::HttpClient> {
    app.client(EmojiServiceClient::new, Some(&who.token))
}

async fn create(
    app: &TestApp,
    who: &Account,
    guild_id: u64,
    name: &str,
    data: Vec<u8>,
    content_type: &str,
) -> Result<Emoji, ConnectError> {
    Ok(emojis(app, who)
        .create_emoji(CreateEmojiRequest {
            guild_id,
            name: name.into(),
            data,
            content_type: content_type.into(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .emoji
        .unwrap())
}

async fn list(app: &TestApp, who: &Account, guild_id: u64) -> Result<Vec<Emoji>, ConnectError> {
    Ok(emojis(app, who)
        .list_emojis(ListEmojisRequest {
            guild_id,
            ..Default::default()
        })
        .await?
        .into_owned()
        .emojis)
}

async fn rename(app: &TestApp, who: &Account, guild_id: u64, emoji_id: u64, name: &str) -> Result<Emoji, ConnectError> {
    Ok(emojis(app, who)
        .rename_emoji(RenameEmojiRequest {
            guild_id,
            emoji_id,
            name: name.into(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .emoji
        .unwrap())
}

async fn delete(app: &TestApp, who: &Account, guild_id: u64, emoji_id: u64) -> Result<(), ConnectError> {
    emojis(app, who)
        .delete_emoji(DeleteEmojiRequest {
            guild_id,
            emoji_id,
            ..Default::default()
        })
        .await?;
    Ok(())
}

struct HttpReply {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn http_get(app: &TestApp, path: &str) -> HttpReply {
    let addr = app.base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
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
async fn static_and_animated_emojis_are_stored_and_served() {
    let app = TestApp::builder().storage().isolated().start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;

    // Static: fitted (not cropped) within 128x128 and stored as WebP.
    let wide = create(&app, &alice, guild.id, "wide", png(300, 200, 1), "image/png").await.unwrap();
    assert!(!wide.animated);
    assert_eq!((wide.guild_id, wide.creator_id), (guild.id, alice.id));
    assert!(wide.image.starts_with(&format!("emojis/{}/", guild.id)) && wide.image.ends_with(".webp"), "{}", wide.image);
    let reply = http_get(&app, &format!("/media/{}", wide.image)).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.headers["content-type"], "image/webp");
    let img = image::load_from_memory_with_format(&reply.body, ImageFormat::WebP).unwrap();
    assert_eq!((img.width(), img.height()), (128, 85));

    // Never upscaled.
    let small = create(&app, &alice, guild.id, "small", png(32, 20, 2), "image/png").await.unwrap();
    let reply = http_get(&app, &format!("/media/{}", small.image)).await;
    let img = image::load_from_memory_with_format(&reply.body, ImageFormat::WebP).unwrap();
    assert_eq!((img.width(), img.height()), (32, 20));

    // Animated GIF: animation and delays survive, resized to fit 128x128, stored as GIF.
    let party = create(&app, &alice, guild.id, "party", animated_gif(256, 128, 4), "image/gif").await.unwrap();
    assert!(party.animated);
    assert!(party.image.ends_with(".gif"), "{}", party.image);
    let reply = http_get(&app, &format!("/media/{}", party.image)).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.headers["content-type"], "image/gif");
    let frames = GifDecoder::new(Cursor::new(&reply.body)).unwrap().into_frames().collect_frames().unwrap();
    assert_eq!(frames.len(), 4);
    for frame in &frames {
        assert_eq!(frame.buffer().dimensions(), (128, 64));
        assert_eq!(frame.delay().numer_denom_ms(), (100, 1));
    }

    // A single-frame GIF is just a static image.
    let still = create(&app, &alice, guild.id, "still", animated_gif(10, 10, 1), "image/gif").await.unwrap();
    assert!(!still.animated && still.image.ends_with(".webp"));

    // Equal images in one guild still get distinct objects.
    let a = create(&app, &alice, guild.id, "twin_a", png(8, 8, 3), "image/png").await.unwrap();
    let b = create(&app, &alice, guild.id, "twin_b", png(8, 8, 3), "image/png").await.unwrap();
    assert_ne!(a.image, b.image);

    let listed = list(&app, &alice, guild.id).await.unwrap();
    let names: Vec<_> = listed.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["party", "small", "still", "twin_a", "twin_b", "wide"]);

    // Accounted as guild storage of kind emoji.
    let kinds = media::totals_by_kind(&app.state, Some(guild.id)).await.unwrap();
    let emoji = kinds.iter().find(|k| k.kind == "emoji").unwrap();
    assert_eq!(emoji.objects, 6);
    assert!(emoji.bytes > 0);
    assert!(kinds.iter().filter(|k| k.kind != "emoji").all(|k| (k.bytes, k.objects) == (0, 0)));
    let (bytes, objects) = media::totals(&app.state, &format!("guild:{}", guild.id)).await.unwrap();
    assert_eq!((bytes, objects), (emoji.bytes, 6));
    let (global_bytes, _) = media::totals(&app.state, "global").await.unwrap();
    assert_eq!(global_bytes, bytes);
    app.cleanup().await;
}

#[tokio::test]
async fn names_are_validated_unique_and_renameable() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let other_guild = common::fixtures::guild(&app, &alice).await;
    let invalid = ErrorCode::InvalidArgument;

    for bad in ["", "a", "Ab", "a-b", "a b", "é_é", &"a".repeat(33)] {
        assert_code(create(&app, &alice, guild.id, bad, png(8, 8, 1), "image/png").await, invalid);
    }
    let ok = create(&app, &alice, guild.id, &"a".repeat(32), png(8, 8, 1), "image/png").await.unwrap();
    let one = create(&app, &alice, guild.id, "one", png(8, 8, 1), "image/png").await.unwrap();
    assert_code(create(&app, &alice, guild.id, "one", png(9, 9, 1), "image/png").await, ErrorCode::AlreadyExists);
    // Names are per guild.
    create(&app, &alice, other_guild.id, "one", png(8, 8, 1), "image/png").await.unwrap();
    // A failed create leaves no name claim behind.
    assert_code(create(&app, &alice, guild.id, "bad_img", b"nope".to_vec(), "image/png").await, invalid);
    create(&app, &alice, guild.id, "bad_img", png(8, 8, 1), "image/png").await.unwrap();

    // Rename: invalid, taken, same name (no-op), free, old name reusable.
    assert_code(rename(&app, &alice, guild.id, one.id, "Bad").await, invalid);
    assert_code(rename(&app, &alice, guild.id, one.id, &ok.name).await, ErrorCode::AlreadyExists);
    assert_eq!(rename(&app, &alice, guild.id, one.id, "one").await.unwrap().name, "one");
    let renamed = rename(&app, &alice, guild.id, one.id, "uno").await.unwrap();
    assert_eq!((renamed.id, renamed.name.as_str(), renamed.image.as_str()), (one.id, "uno", one.image.as_str()));
    create(&app, &alice, guild.id, "one", png(8, 8, 2), "image/png").await.unwrap();
    assert_code(rename(&app, &alice, guild.id, 424242, "nope").await, ErrorCode::NotFound);
    // An emoji id from another guild is not found here.
    assert_code(rename(&app, &alice, other_guild.id, one.id, "x1").await, ErrorCode::NotFound);

    // Delete: gone from the list, object removed, name free again, counters back.
    let before = media::totals(&app.state, &format!("guild:{}", guild.id)).await.unwrap();
    delete(&app, &alice, guild.id, one.id).await.unwrap();
    assert_code(delete(&app, &alice, guild.id, one.id).await, ErrorCode::NotFound);
    assert!(!list(&app, &alice, guild.id).await.unwrap().iter().any(|e| e.id == one.id));
    assert_eq!(http_get(&app, &format!("/media/{}", one.image)).await.status, 404);
    let after = media::totals(&app.state, &format!("guild:{}", guild.id)).await.unwrap();
    assert_eq!(after.1, before.1 - 1);
    assert!(after.0 < before.0);
    create(&app, &alice, guild.id, "uno", png(8, 8, 9), "image/png").await.unwrap();
    app.cleanup().await;
}

#[tokio::test]
async fn bad_images_are_invalid_argument() {
    let app = TestApp::builder().storage().isolated().start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let invalid = ErrorCode::InvalidArgument;
    assert_code(create(&app, &alice, guild.id, "aa", b"not an image".to_vec(), "image/png").await, invalid);
    assert_code(create(&app, &alice, guild.id, "aa", vec![], "image/png").await, invalid);
    assert_code(create(&app, &alice, guild.id, "aa", png(8, 8, 1), "image/bmp").await, invalid);
    assert_code(create(&app, &alice, guild.id, "aa", png(8, 8, 1), "image/gif").await, invalid); // mismatch
    // Over 2 MiB.
    assert_code(create(&app, &alice, guild.id, "aa", vec![0; 2 * 1024 * 1024 + 1], "image/png").await, invalid);
    // Too many frames (201 > 200) and a bomb-ish canvas.
    assert_code(create(&app, &alice, guild.id, "aa", animated_gif(1, 1, 201), "image/gif").await, invalid);
    assert_code(create(&app, &alice, guild.id, "aa", png(9000, 1, 1), "image/png").await, invalid);
    assert_eq!(list(&app, &alice, guild.id).await.unwrap().len(), 0);
    let (bytes, objects) = media::totals(&app.state, "global").await.unwrap();
    assert_eq!((bytes, objects), (0, 0));
    app.cleanup().await;
}

#[tokio::test]
async fn emoji_cap_is_200_per_guild() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    for i in 0..200u64 {
        let id = app.state.snowflake.next_id();
        app.state
            .session
            .execute_unpaged(
                "INSERT INTO emojis (guild_id, id, name, animated, image, creator_id) VALUES (?, ?, ?, ?, ?, ?)",
                (id_to_db(guild.id), id_to_db(id), format!("e{i}"), false, "", id_to_db(alice.id)),
            )
            .await
            .unwrap();
        app.state
            .session
            .execute_unpaged(
                "INSERT INTO emoji_names (guild_id, name, emoji_id) VALUES (?, ?, ?)",
                (id_to_db(guild.id), format!("e{i}"), id_to_db(id)),
            )
            .await
            .unwrap();
    }
    assert_code(create(&app, &alice, guild.id, "overflow", png(8, 8, 1), "image/png").await, ErrorCode::ResourceExhausted);
    // The rejected create left no name claim, and deleting one makes room.
    let first = list(&app, &alice, guild.id).await.unwrap()[0].clone();
    delete(&app, &alice, guild.id, first.id).await.unwrap();
    create(&app, &alice, guild.id, "overflow", png(8, 8, 1), "image/png").await.unwrap();
    assert_eq!(list(&app, &alice, guild.id).await.unwrap().len(), 200);
    assert_code(create(&app, &alice, guild.id, "again", png(8, 8, 1), "image/png").await, ErrorCode::ResourceExhausted);
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_creates_at_the_cap_do_not_exceed_it() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    for i in 0..198u64 {
        let id = app.state.snowflake.next_id();
        app.state
            .session
            .execute_unpaged(
                "INSERT INTO emojis (guild_id, id, name, animated, image, creator_id) VALUES (?, ?, ?, ?, ?, ?)",
                (id_to_db(guild.id), id_to_db(id), format!("e{i}"), false, "", id_to_db(alice.id)),
            )
            .await
            .unwrap();
    }
    let results = futures::future::join_all((0..5).map(|i| {
        let (app, alice) = (&app, &alice);
        async move { create(app, alice, guild.id, &format!("race{i}"), png(8, 8, i as u8), "image/png").await }
    }))
    .await;
    // Racers at the cap may all back off, but never more than the two free slots survive.
    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert!(ok <= 2, "{results:?}");
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|e| e.code == ErrorCode::ResourceExhausted));
    assert_eq!(list(&app, &alice, guild.id).await.unwrap().len(), 198 + ok);
    app.cleanup().await;
}

#[tokio::test]
async fn permission_matrix() {
    let app = TestApp::start_with_storage().await;
    let (alice, bob, carol, dave, eve) = (
        user(&app, "alice").await,
        user(&app, "bob").await,
        user(&app, "carol").await,
        user(&app, "dave").await,
        user(&app, "eve").await,
    );
    let guild = guild(&app, &alice).await;
    for who in [&bob, &carol, &dave] {
        join(&app, who, &guild).await;
    }
    let manager = role(&app, &alice, guild.id, "emoji-mgr", MANAGE_EMOJIS).await.unwrap();
    set_roles(&app, &alice, guild.id, carol.id, &[manager.id]).await.unwrap();
    let admin = role(&app, &alice, guild.id, "admin", ADMINISTRATOR).await.unwrap();
    set_roles(&app, &alice, guild.id, dave.id, &[admin.id]).await.unwrap();
    // MANAGE_EMOJIS is a valid role bit; MANAGE_ROLES alone does not grant it.
    let other = role(&app, &alice, guild.id, "other", MANAGE_ROLES | MANAGE_CHANNELS | MANAGE_SERVER).await.unwrap();
    set_roles(&app, &alice, guild.id, bob.id, &[other.id]).await.unwrap();

    let target = create(&app, &alice, guild.id, "target", png(8, 8, 1), "image/png").await.unwrap();
    for (who, may) in [(&alice, true), (&carol, true), (&dave, true), (&bob, false)] {
        // Every member can list.
        assert_eq!(list(&app, who, guild.id).await.unwrap().iter().filter(|e| e.name.starts_with("t")).count(), 1);
        let name = format!("by_{}", who.id);
        let made = create(&app, who, guild.id, &name, png(8, 8, 2), "image/png").await;
        let renamed = rename(&app, who, guild.id, target.id, &format!("t{}", who.id)).await;
        if may {
            let made = made.unwrap_or_else(|e| panic!("create as {}: {e:?}", who.id));
            renamed.unwrap();
            delete(&app, who, guild.id, made.id).await.unwrap();
        } else {
            assert_code(made, ErrorCode::PermissionDenied);
            assert_code(renamed, ErrorCode::PermissionDenied);
            assert_code(delete(&app, who, guild.id, target.id).await, ErrorCode::PermissionDenied);
        }
    }
    // Non-members cannot see the guild at all.
    assert_code(list(&app, &eve, guild.id).await, ErrorCode::NotFound);
    assert_code(create(&app, &eve, guild.id, "nope", png(8, 8, 1), "image/png").await, ErrorCode::NotFound);
    app.cleanup().await;
}

#[tokio::test]
async fn creates_are_rate_limited() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    for i in 0..20u8 {
        create(&app, &alice, guild.id, &format!("e{i:02}"), png(4, 4, i), "image/png").await.unwrap();
    }
    assert_code(create(&app, &alice, guild.id, "e99", png(4, 4, 99), "image/png").await, ErrorCode::ResourceExhausted);
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
            let event = msg.to_owned_message().event.into_option().expect("event");
            match event.payload {
                Some(Payload::Heartbeat(_)) | None => continue,
                Some(p) => break p,
            }
        }
    }};
}

#[tokio::test]
async fn mutations_publish_the_full_list_to_members_only() {
    let app = TestApp::start_with_storage().await;
    let (alice, bob, eve) = (user(&app, "alice").await, user(&app, "bob").await, user(&app, "eve").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let mut bob_stream = app.client(GatewayServiceClient::new, Some(&bob.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    let mut eve_stream = app.client(GatewayServiceClient::new, Some(&eve.token)).subscribe(SubscribeRequest::default()).await.unwrap();

    let a = create(&app, &alice, guild.id, "aa", png(8, 8, 1), "image/png").await.unwrap();
    let Payload::EmojisUpdated(ev) = next_payload!(bob_stream) else { panic!("expected EmojisUpdated") };
    assert_eq!(ev.guild_id, guild.id);
    assert_eq!(ev.emojis.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["aa"]);

    let b = create(&app, &alice, guild.id, "bb", png(8, 8, 2), "image/png").await.unwrap();
    let Payload::EmojisUpdated(ev) = next_payload!(bob_stream) else { panic!() };
    assert_eq!(ev.emojis.len(), 2);

    rename(&app, &alice, guild.id, a.id, "cc").await.unwrap();
    let Payload::EmojisUpdated(ev) = next_payload!(bob_stream) else { panic!() };
    assert_eq!(ev.emojis.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["bb", "cc"]);

    delete(&app, &alice, guild.id, b.id).await.unwrap();
    let Payload::EmojisUpdated(ev) = next_payload!(bob_stream) else { panic!() };
    assert_eq!(ev.emojis.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["cc"]);

    // A rejected mutation publishes nothing, and outsiders hear nothing at all.
    let _ = create(&app, &alice, guild.id, "cc", png(8, 8, 3), "image/png").await;
    assert!(timeout(Duration::from_millis(500), async {
        loop {
            let msg = eve_stream.message().await.unwrap().unwrap().to_owned_message();
            if !matches!(msg.event.into_option().and_then(|e| e.payload), Some(Payload::Heartbeat(_)) | None) {
                break;
            }
        }
    })
    .await
    .is_err());
    app.cleanup().await;
}

#[tokio::test]
async fn deleting_an_emoji_resets_channels_using_it_as_icon() {
    let app = TestApp::start_with_storage().await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let emoji = create(&app, &alice, guild.id, "star", png(8, 8, 1), "image/png").await.unwrap();
    let keep = create(&app, &alice, guild.id, "keep", png(8, 8, 2), "image/png").await.unwrap();
    let icon = |id: u64| {
        Some(ChannelIcon {
            kind: Some(channel_icon::Kind::CustomEmojiId(id)),
            ..Default::default()
        })
    };
    let make = |name: &str, id: u64| {
        let client = guilds(&app, &alice);
        let req = CreateChannelRequest {
            guild_id: guild.id,
            name: name.into(),
            icon_ref: icon(id).into(),
            ..Default::default()
        };
        async move { client.create_channel(req).await.map(|r| r.into_owned().channel.unwrap()) }
    };
    let using = make("using", emoji.id).await.unwrap();
    let other = make("other", keep.id).await.unwrap();
    assert_eq!(using.icon_ref.as_option().and_then(|i| i.kind.clone()), Some(channel_icon::Kind::CustomEmojiId(emoji.id)));
    assert_eq!(using.icon, "");

    let mut stream = app.client(GatewayServiceClient::new, Some(&bob.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    delete(&app, &alice, guild.id, emoji.id).await.unwrap();
    let Payload::ChannelUpdated(c) = next_payload!(stream) else { panic!("expected ChannelUpdated") };
    assert_eq!(c.id, using.id);
    assert!(c.icon_ref.as_option().is_none() && c.icon.is_empty());
    assert!(matches!(next_payload!(stream), Payload::EmojisUpdated(_)));

    let listing = channels(&app, &alice, guild.id).await.unwrap();
    let find = |id: u64| listing.channels.iter().find(|c| c.id == id).unwrap();
    assert!(find(using.id).icon_ref.as_option().is_none());
    assert_eq!(
        find(other.id).icon_ref.as_option().and_then(|i| i.kind.clone()),
        Some(channel_icon::Kind::CustomEmojiId(keep.id))
    );
    app.cleanup().await;
}

#[tokio::test]
async fn custom_channel_icons_must_be_emojis_of_the_same_guild() {
    let app = TestApp::start_with_storage().await;
    let alice = user(&app, "alice").await;
    let (g1, g2) = (guild(&app, &alice).await, guild(&app, &alice).await);
    let foreign = create(&app, &alice, g2.id, "star", png(8, 8, 1), "image/png").await.unwrap();
    let general = channels(&app, &alice, g1.id).await.unwrap().channels[0].id;
    let update = |emoji_id: u64| {
        let client = guilds(&app, &alice);
        let req = UpdateChannelRequest {
            guild_id: g1.id,
            channel_id: general,
            icon_ref: Some(ChannelIcon {
                kind: Some(channel_icon::Kind::CustomEmojiId(emoji_id)),
                ..Default::default()
            })
            .into(),
            ..Default::default()
        };
        async move { client.update_channel(req).await.map(|r| r.into_owned().channel.unwrap()) }
    };
    assert_code(update(foreign.id).await, ErrorCode::InvalidArgument);
    assert_code(update(424242).await, ErrorCode::InvalidArgument);
    let mine = create(&app, &alice, g1.id, "mine", png(8, 8, 2), "image/png").await.unwrap();
    let channel = update(mine.id).await.unwrap();
    assert_eq!(channel.icon_ref.as_option().and_then(|i| i.kind.clone()), Some(channel_icon::Kind::CustomEmojiId(mine.id)));
    app.cleanup().await;
}
