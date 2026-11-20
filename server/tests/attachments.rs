//! Message attachments against the real S3-compatible service from
//! `docker-compose.yml`: `docker compose up -d` must be running.
mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, ImageFormat, Rgba, RgbaImage};
use rand::RngCore;
use thehideout_server::convert::id_to_db;
use thehideout_server::media::{self, attachments::AttachmentSettings, pending};
use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::*;
use thehideout_server::services::message::publish_message_updated;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, timeout};

const WAIT: Duration = Duration::from_secs(5);

struct Reply {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).unwrap_or_else(|| panic!("no {name} header in {:?}", self.headers))
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", String::from_utf8_lossy(&self.body)))
    }
}

/// Minimal raw HTTP/1.1 request: the body is written as given, `Content-Length`
/// only if `length` says so (so tests can lie about it or use none).
async fn http(app: &TestApp, method: &str, path: &str, headers: &[(&str, String)], length: Option<usize>, body: &[u8]) -> Reply {
    let addr = app.base_url.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(length) = length {
        request.push_str(&format!("Content-Length: {length}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    // The server may answer (and close) before reading the whole body.
    let _ = stream.write_all(body).await;
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw).await;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("response header end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Reply { status, headers, body: raw[split + 4..].to_vec() }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn upload_raw(app: &TestApp, token: Option<&str>, channel_id: u64, filename: &str, content_type: &str, data: &[u8]) -> Reply {
    let mut headers = vec![("Content-Type", content_type.to_owned())];
    if let Some(token) = token {
        headers.push(("Authorization", format!("Bearer {token}")));
    }
    http(app, "POST", &format!("/attachments?channel_id={channel_id}&filename={}", encode(filename)), &headers, Some(data.len()), data).await
}

/// Uploads, expects success and returns the pending attachment's JSON.
async fn upload(app: &TestApp, who: &Account, channel_id: u64, filename: &str, content_type: &str, data: &[u8]) -> serde_json::Value {
    let reply = upload_raw(app, Some(&who.token), channel_id, filename, content_type, data).await;
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
    reply.json()
}

fn pending_id(p: &serde_json::Value) -> u64 {
    p["id"].as_str().expect("id is a JSON string").parse().unwrap()
}

fn messages(app: &TestApp, who: &Account) -> MessageServiceClient<connectrpc::client::HttpClient> {
    app.client(MessageServiceClient::new, Some(&who.token))
}

async fn send(app: &TestApp, who: &Account, channel_id: u64, content: &str, ids: &[u64]) -> Result<Message, ConnectError> {
    Ok(messages(app, who)
        .send_message(SendMessageRequest {
            channel_id,
            content: content.into(),
            attachment_ids: ids.to_vec(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .message
        .unwrap())
}

async fn list(app: &TestApp, who: &Account, channel_id: u64) -> Vec<Message> {
    messages(app, who)
        .list_messages(ListMessagesRequest {
            channel_id,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .messages
}

fn assert_code<T: std::fmt::Debug>(result: Result<T, ConnectError>, code: ErrorCode) {
    match result {
        Err(e) => assert_eq!(e.code, code, "{e:?}"),
        Ok(v) => panic!("expected {code:?}, got Ok({v:?})"),
    }
}

async fn download(app: &TestApp, key: &str, headers: &[(&str, String)]) -> Reply {
    http(app, "GET", &format!("/media/{key}"), headers, None, &[]).await
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbaImage::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255]));
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8).unwrap();
    out
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut data = vec![0u8; n];
    rand::rng().fill_bytes(&mut data);
    data
}

struct Setup {
    app: TestApp,
    alice: Account,
    bob: Account,
    guild: Guild,
    channel: u64,
}

async fn setup_with(settings: AttachmentSettings) -> Setup {
    let app = TestApp::start_with_attachments(settings).await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let channel = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    Setup { app, alice, bob, guild, channel }
}

async fn setup() -> Setup {
    setup_with(AttachmentSettings::default()).await
}

/// The keys of the caller's pending uploads, read straight from the table.
async fn pending_keys(app: &TestApp, uploader: u64) -> Vec<String> {
    app.state
        .session
        .execute_unpaged("SELECT key FROM pending_attachments WHERE uploader = ?", (id_to_db(uploader),))
        .await
        .unwrap()
        .into_rows_result()
        .unwrap()
        .rows::<(String,)>()
        .unwrap()
        .map(|r| r.unwrap().0)
        .collect()
}

async fn sweep_rows(app: &TestApp) -> usize {
    app.state
        .session
        .execute_unpaged("SELECT id FROM pending_sweep", ())
        .await
        .unwrap()
        .into_rows_result()
        .unwrap()
        .rows_num()
}

async fn exists(app: &TestApp, key: &str) -> bool {
    app.state.storage.as_ref().unwrap().head(key).await.unwrap().is_some()
}

#[tokio::test]
async fn upload_text_file_returns_the_pending_json() {
    let Setup { app, alice, channel, .. } = setup().await;
    let reply = upload_raw(&app, Some(&alice.token), channel, "notes.txt", "text/plain; charset=utf-8", b"hello attachments").await;
    assert_eq!(reply.status, 200);
    assert!(reply.header("content-type").starts_with("application/json"));
    let json = reply.json();
    // proto3 JSON: camelCase, 64-bit integers as strings, RFC 3339 timestamps.
    assert!(json["id"].as_str().unwrap().parse::<u64>().unwrap() > 0);
    assert_eq!(json["filename"], "notes.txt");
    assert_eq!(json["contentType"], "text/plain");
    assert_eq!(json["size"], "17");
    let expires = json["expiresAt"].as_str().expect("expiresAt");
    assert!(expires.ends_with('Z') && expires.contains('T'), "{expires}");
    // Typed round trip through the generated message.
    let typed: PendingAttachment = serde_json::from_value(json).unwrap();
    assert_eq!((typed.size, typed.filename.as_str()), (17, "notes.txt"));
    assert_eq!(pending_keys(&app, alice.id).await.len(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn filenames_are_sanitised_and_types_sniffed() {
    let Setup { app, alice, channel, .. } = setup().await;
    // A PNG declared as something else is recognised from its bytes.
    let json = upload(&app, &alice, channel, "../../etc/pass\u{0}wd.png", "application/x-bogus", &png(4, 4)).await;
    assert_eq!(json["filename"], "passwd.png");
    assert_eq!(json["contentType"], "image/png");
    // Unknown bytes keep a sane declared type, an insane one becomes octet-stream.
    assert_eq!(upload(&app, &alice, channel, "a.bin", "application/foo", b"\x01\x02\x03").await["contentType"], "application/foo");
    assert_eq!(upload(&app, &alice, channel, "a.bin", "not a mime", b"\x01\x02\x03").await["contentType"], "application/octet-stream");
    app.cleanup().await;
}

#[tokio::test]
async fn png_gets_dimensions_and_a_thumbnail() {
    let Setup { app, alice, channel, .. } = setup().await;
    let data = png(800, 200);
    let pending = upload(&app, &alice, channel, "wide.png", "image/png", &data).await;
    let message = send(&app, &alice, channel, "", &[pending_id(&pending)]).await.unwrap();
    assert_eq!(message.attachments.len(), 1);
    for a in [&message.attachments[0], &list(&app, &alice, channel).await[0].attachments[0]] {
        assert_eq!((a.width, a.height), (Some(800), Some(200)));
        assert_eq!((a.filename.as_str(), a.content_type.as_str(), a.size), ("wide.png", "image/png", data.len() as u64));
        assert!(!a.removed && a.file.starts_with(&format!("attachments/{channel}/")));
        let thumb_key = a.thumbnail.clone().expect("thumbnail key");
        assert_eq!(thumb_key, format!("{}-thumb.webp", a.file));
        let thumb = download(&app, &thumb_key, &[]).await;
        assert_eq!(thumb.status, 200);
        assert_eq!(thumb.header("content-type"), "image/webp");
        assert!(thumb.header("content-disposition").starts_with("inline"));
        let decoded = image::load_from_memory_with_format(&thumb.body, ImageFormat::WebP).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (400, 100));
    }
    // A non-image has neither.
    let text = upload(&app, &alice, channel, "a.txt", "text/plain", b"x").await;
    let message = send(&app, &alice, channel, "", &[pending_id(&text)]).await.unwrap();
    let a = &message.attachments[0];
    assert_eq!((a.width, a.height, a.thumbnail.clone()), (None, None, None));
    app.cleanup().await;
}

#[tokio::test]
async fn large_binary_streams_and_serves_ranges() {
    let Setup { app, alice, channel, .. } = setup().await;
    // 12 MB: three 5 MiB parts through the multipart path.
    let data = random_bytes(12 * 1000 * 1000);
    let pending = upload(&app, &alice, channel, "blob.bin", "application/octet-stream", &data).await;
    assert_eq!(pending["size"], data.len().to_string());
    let message = send(&app, &alice, channel, "", &[pending_id(&pending)]).await.unwrap();
    let key = message.attachments[0].file.clone();

    let full = download(&app, &key, &[]).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.header("accept-ranges"), "bytes");
    assert!(full.body == data, "downloaded bytes differ");

    let part = download(&app, &key, &[("Range", "bytes=100-199".into())]).await;
    assert_eq!(part.status, 206);
    assert_eq!(part.header("content-range"), format!("bytes 100-199/{}", data.len()));
    assert_eq!(part.body, &data[100..200]);
    let tail = download(&app, &key, &[("Range", "bytes=-10".into())]).await;
    assert_eq!((tail.status, tail.body.as_slice()), (206, &data[data.len() - 10..]));
    let open = download(&app, &key, &[("Range", format!("bytes={}-", data.len() - 5))]).await;
    assert_eq!((open.status, open.body.as_slice()), (206, &data[data.len() - 5..]));
    let beyond = download(&app, &key, &[("Range", format!("bytes={}-", data.len()))]).await;
    assert_eq!(beyond.status, 416);
    assert_eq!(beyond.header("content-range"), format!("bytes */{}", data.len()));
    app.cleanup().await;
}

#[tokio::test]
async fn upload_requires_a_valid_token_and_channel_access() {
    let Setup { app, alice, channel, .. } = setup().await;
    assert_eq!(upload_raw(&app, None, channel, "a.txt", "text/plain", b"x").await.status, 401);
    assert_eq!(upload_raw(&app, Some("nope"), channel, "a.txt", "text/plain", b"x").await.status, 401);
    // A user outside the guild, and a channel that does not exist: both 404.
    let eve = user(&app, "eve").await;
    assert_eq!(upload_raw(&app, Some(&eve.token), channel, "a.txt", "text/plain", b"x").await.status, 404);
    assert_eq!(upload_raw(&app, Some(&alice.token), 12345, "a.txt", "text/plain", b"x").await.status, 404);
    let missing = http(&app, "POST", "/attachments", &[("Authorization", format!("Bearer {}", alice.token))], Some(1), b"x").await;
    assert_eq!(missing.status, 400);
    assert!(pending_keys(&app, eve.id).await.is_empty());
    app.cleanup().await;
}

#[tokio::test]
async fn size_limits_are_enforced_before_reading_the_body() {
    let settings = AttachmentSettings { max_upload_bytes: 1000, ..Default::default() };
    let Setup { app, alice, channel, .. } = setup_with(settings).await;
    let auth = vec![("Authorization", format!("Bearer {}", alice.token))];
    let path = format!("/attachments?channel_id={channel}&filename=big.bin");
    // Declared too large, body never sent: answered right away.
    let reply = http(&app, "POST", &path, &auth, Some(1001), &[]).await;
    assert_eq!(reply.status, 413);
    assert_eq!(reply.json()["code"], "invalid_argument");
    // Exactly at the limit is fine.
    assert_eq!(upload_raw(&app, Some(&alice.token), channel, "ok.bin", "application/octet-stream", &[7u8; 1000]).await.status, 200);
    // No Content-Length (chunked): 411. Empty body: 400.
    let mut chunked = auth.clone();
    chunked.push(("Transfer-Encoding", "chunked".into()));
    assert_eq!(http(&app, "POST", &path, &chunked, None, b"5\r\nhello\r\n0\r\n\r\n").await.status, 411);
    assert_eq!(http(&app, "POST", &path, &auth, Some(0), &[]).await.status, 400);
    assert_eq!(pending_keys(&app, alice.id).await.len(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn disconnecting_mid_upload_cleans_up() {
    let Setup { app, alice, channel, .. } = setup().await;
    let addr = app.base_url.trim_start_matches("http://").to_owned();
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let head = format!(
        "POST /attachments?channel_id={channel}&filename=cut.bin HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nContent-Length: 20000000\r\n\r\n",
        alice.token
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&random_bytes(6 * 1024 * 1024)).await.unwrap();
    // Wait until the server has registered the upload, then hang up.
    timeout(WAIT, async {
        while sweep_rows(&app).await == 0 {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("upload registered");
    drop(stream);
    timeout(Duration::from_secs(10), async {
        while sweep_rows(&app).await != 0 {
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("registry row removed after the abort");
    assert!(pending_keys(&app, alice.id).await.is_empty());
    app.cleanup().await;
}

#[tokio::test]
async fn pending_uploads_are_rate_limited() {
    let settings = AttachmentSettings { max_pending_per_user: 2, ..Default::default() };
    let Setup { app, alice, bob, channel, .. } = setup_with(settings).await;
    upload(&app, &alice, channel, "1.txt", "text/plain", b"1").await;
    upload(&app, &alice, channel, "2.txt", "text/plain", b"2").await;
    assert_eq!(upload_raw(&app, Some(&alice.token), channel, "3.txt", "text/plain", b"3").await.status, 429);
    // The cap is per user.
    assert_eq!(upload_raw(&app, Some(&bob.token), channel, "b.txt", "text/plain", b"b").await.status, 200);
    app.cleanup().await;

    let settings = AttachmentSettings { uploads_per_minute: 2, ..Default::default() };
    let Setup { app, alice, channel, .. } = setup_with(settings).await;
    upload(&app, &alice, channel, "1.txt", "text/plain", b"1").await;
    upload(&app, &alice, channel, "2.txt", "text/plain", b"2").await;
    assert_eq!(upload_raw(&app, Some(&alice.token), channel, "3.txt", "text/plain", b"3").await.status, 429);
    app.cleanup().await;
}

#[tokio::test]
async fn attachments_are_listed_and_served_with_safe_headers() {
    let Setup { app, alice, bob, channel, .. } = setup().await;
    let pdf = b"%PDF-1.4\n1 0 obj<<>>endobj\n".to_vec();
    let uploads = [
        upload(&app, &alice, channel, "résumé ž.pdf", "application/pdf", &pdf).await,
        upload(&app, &alice, channel, "notes.txt", "text/plain", b"plain text").await,
        upload(&app, &alice, channel, "logo.svg", "image/svg+xml", b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>").await,
        upload(&app, &alice, channel, "page.html", "text/html", b"<script>alert(1)</script>").await,
        upload(&app, &alice, channel, "pic.png", "image/png", &png(10, 10)).await,
        upload(&app, &alice, channel, "a.zip", "application/zip", b"PK\x03\x04rest").await,
        // Declared as an image but not one: never rendered inline.
        upload(&app, &alice, channel, "fake.png", "image/png", b"<html>not a png</html>").await,
    ];
    let ids: Vec<u64> = uploads.iter().map(pending_id).collect();
    assert_eq!(uploads[2]["contentType"], "image/svg+xml");
    let sent = send(&app, &alice, channel, "files", &ids).await.unwrap();
    assert_eq!(sent.attachments.iter().map(|a| a.id).collect::<Vec<_>>(), ids, "order is kept");

    // Another member sees the same list from ListMessages.
    let listed = list(&app, &bob, channel).await;
    assert_eq!(listed[0].attachments, sent.attachments);

    let get = |i: usize| {
        let key = sent.attachments[i].file.clone();
        let app = &app;
        async move { download(app, &key, &[]).await }
    };
    let pdf_reply = get(0).await;
    assert_eq!(pdf_reply.status, 200);
    assert_eq!(pdf_reply.body, pdf);
    assert_eq!(pdf_reply.header("content-type"), "application/pdf");
    assert_eq!(pdf_reply.header("content-disposition"), "inline; filename*=UTF-8''r%C3%A9sum%C3%A9%20%C5%BE.pdf");
    assert_eq!(pdf_reply.header("x-content-type-options"), "nosniff");

    let txt = get(1).await;
    assert_eq!(txt.header("content-type"), "text/plain");
    assert_eq!(txt.header("content-disposition"), "attachment; filename*=UTF-8''notes.txt");
    assert_eq!(txt.header("x-content-type-options"), "nosniff");

    for i in [2, 3] {
        let r = get(i).await;
        assert_eq!(r.header("content-type"), "application/octet-stream", "svg/html are opaque downloads");
        assert!(r.header("content-disposition").starts_with("attachment;"), "{:?}", r.headers);
        assert_eq!(r.header("x-content-type-options"), "nosniff");
    }
    let image = get(4).await;
    assert_eq!(image.header("content-type"), "image/png");
    assert!(image.header("content-disposition").starts_with("inline;"));
    let zip = get(5).await;
    assert!(zip.header("content-disposition").starts_with("attachment;"));
    let fake = get(6).await;
    assert!(fake.header("content-disposition").starts_with("attachment;"), "unverified image types are not inline");

    // Conditional requests and unknown keys.
    let etag = image.header("etag").to_owned();
    let cached = download(&app, &sent.attachments[4].file, &[("If-None-Match", etag)]).await;
    assert_eq!(cached.status, 304);
    assert_eq!(download(&app, &format!("attachments/{channel}/{}", "0".repeat(32)), &[]).await.status, 404);
    assert_eq!(download(&app, &format!("attachments/{channel}/../x"), &[]).await.status, 404);
    app.cleanup().await;
}

#[tokio::test]
async fn pending_uploads_can_only_be_used_once_by_their_owner_for_their_channel() {
    let Setup { app, alice, bob, guild, channel } = setup().await;
    let other_channel = channel_in(&app, &alice, guild.id).await;
    let pending = upload(&app, &alice, channel, "a.txt", "text/plain", b"a").await;
    let id = pending_id(&pending);

    // Someone else's pending upload, another channel's, unknown ones.
    assert_code(send(&app, &bob, channel, "x", &[id]).await, ErrorCode::InvalidArgument);
    assert_code(send(&app, &alice, other_channel, "x", &[id]).await, ErrorCode::InvalidArgument);
    assert_code(send(&app, &alice, channel, "x", &[id + 1]).await, ErrorCode::InvalidArgument);
    assert_code(send(&app, &alice, channel, "x", &[0]).await, ErrorCode::InvalidArgument);
    assert_code(send(&app, &alice, channel, "x", &[id, id]).await, ErrorCode::InvalidArgument);
    // A failed attempt consumes nothing.
    assert_eq!(pending_keys(&app, alice.id).await.len(), 1);

    let ok = send(&app, &alice, channel, "x", &[id]).await.unwrap();
    assert_eq!(ok.attachments.len(), 1);
    // Consumed: gone for good.
    assert_code(send(&app, &alice, channel, "again", &[id]).await, ErrorCode::InvalidArgument);
    assert!(pending_keys(&app, alice.id).await.is_empty());
    assert_eq!(sweep_rows(&app).await, 0, "consuming removes the registry row");
    app.cleanup().await;
}

async fn channel_in(app: &TestApp, who: &Account, guild_id: u64) -> u64 {
    channel(app, who, guild_id, "other").await.unwrap().id
}

#[tokio::test]
async fn concurrent_sends_of_one_pending_upload_apply_once() {
    let Setup { app, alice, channel, .. } = setup().await;
    let id = pending_id(&upload(&app, &alice, channel, "a.txt", "text/plain", b"a").await);
    let ids = [id];
    let results = futures::future::join_all((0..4).map(|_| send(&app, &alice, channel, "", &ids))).await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
    assert_eq!(list(&app, &alice, channel).await.len(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn attachment_count_and_content_rules() {
    let Setup { app, alice, channel, .. } = setup().await;
    let mut ids = Vec::new();
    for i in 0..11 {
        ids.push(pending_id(&upload(&app, &alice, channel, &format!("{i}.txt"), "text/plain", b"x").await));
    }
    assert_code(send(&app, &alice, channel, "too many", &ids).await, ErrorCode::InvalidArgument);
    // Ten are fine, and content may then be empty.
    let sent = send(&app, &alice, channel, "", &ids[..10]).await.unwrap();
    assert_eq!((sent.content.as_str(), sent.attachments.len()), ("", 10));
    // Without attachments, empty content is still refused; the leftover pending upload is untouched.
    assert_code(send(&app, &alice, channel, "  ", &[]).await, ErrorCode::InvalidArgument);
    assert_eq!(send(&app, &alice, channel, "", &ids[10..]).await.unwrap().attachments.len(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn expired_uploads_are_swept_and_used_ones_kept() {
    let settings = AttachmentSettings {
        pending_ttl: Duration::from_secs(2),
        sweep_grace: Duration::ZERO,
        ..Default::default()
    };
    let Setup { app, alice, channel, .. } = setup_with(settings).await;
    let png_pending = upload(&app, &alice, channel, "p.png", "image/png", &png(20, 20)).await;
    let unused = upload(&app, &alice, channel, "unused.txt", "text/plain", b"never sent").await;
    let used = upload(&app, &alice, channel, "used.txt", "text/plain", b"sent").await;
    let mut keys = pending_keys(&app, alice.id).await;
    keys.sort();
    assert_eq!(keys.len(), 3);
    for key in &keys {
        assert!(exists(&app, key).await);
    }
    let sent = send(&app, &alice, channel, "", &[pending_id(&used)]).await.unwrap();
    let used_key = sent.attachments[0].file.clone();
    // Nothing is due yet.
    assert_eq!(pending::sweep_once(&app.state).await.unwrap(), 0);

    sleep(Duration::from_millis(3500)).await;
    assert_eq!(pending::sweep_once(&app.state).await.unwrap(), 2, "unused text file and png");
    for key in keys.iter().filter(|k| **k != used_key) {
        assert!(!exists(&app, key).await, "{key} should be deleted");
        assert!(!exists(&app, &format!("{key}-thumb.webp")).await);
    }
    assert!(exists(&app, &used_key).await, "consumed upload survives");
    assert_eq!(sweep_rows(&app).await, 0);
    // Expired ones cannot be sent any more.
    assert_code(send(&app, &alice, channel, "late", &[pending_id(&unused)]).await, ErrorCode::InvalidArgument);
    assert_code(send(&app, &alice, channel, "late", &[pending_id(&png_pending)]).await, ErrorCode::InvalidArgument);
    // A second pass finds nothing.
    assert_eq!(pending::sweep_once(&app.state).await.unwrap(), 0);
    app.cleanup().await;
}

#[tokio::test]
async fn bytes_are_counted_when_sent_not_when_uploaded() {
    let Setup { app, alice, guild, channel, .. } = setup().await;
    let scope = format!("guild:{}:attachment", guild.id);
    let guild_all = format!("guild:{}", guild.id);
    let total = |scope: String| {
        let state = app.state.clone();
        async move { media::totals(&state, &scope).await.unwrap() }
    };
    assert_eq!(total("global:attachment".into()).await, (0, 0));

    let text = b"twelve bytes";
    let a = upload(&app, &alice, channel, "t.txt", "text/plain", text).await;
    assert_eq!(total("global:attachment".into()).await, (0, 0), "pending uploads are not counted");

    let image = png(500, 300);
    let b = upload(&app, &alice, channel, "i.png", "image/png", &image).await;
    let sent = send(&app, &alice, channel, "", &[pending_id(&a), pending_id(&b)]).await.unwrap();
    let thumb_key = sent.attachments[1].thumbnail.clone().unwrap();
    let thumb_len = download(&app, &thumb_key, &[]).await.body.len() as i64;
    let expected = (text.len() + image.len()) as i64 + thumb_len;
    assert_eq!(total("global:attachment".into()).await, (expected, 3));
    assert_eq!(total(scope.clone()).await, (expected, 3));
    assert_eq!(total(guild_all.clone()).await, (expected, 3));
    assert_eq!(total("global".into()).await.0, expected);

    // The per-kind breakdown the admin overview reads.
    let by_kind = media::totals_by_kind(&app.state, Some(guild.id)).await.unwrap();
    assert_eq!(by_kind.iter().find(|k| k.kind == "attachment").unwrap().bytes, expected);
    app.cleanup().await;
}

#[tokio::test]
async fn direct_message_attachments_work_and_count_globally() {
    let Setup { app, alice, bob, guild, .. } = setup().await;
    let dm = app
        .client(DmServiceClient::new, Some(&alice.token))
        .open_dm(OpenDmRequest { user_id: bob.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    let eve = user(&app, "eve").await;
    assert_eq!(upload_raw(&app, Some(&eve.token), dm.id, "x.txt", "text/plain", b"x").await.status, 404, "not a participant");

    let pending = upload(&app, &alice, dm.id, "dm.txt", "text/plain", b"secret").await;
    let sent = send(&app, &alice, dm.id, "", &[pending_id(&pending)]).await.unwrap();
    assert_eq!(sent.guild_id, 0);
    let seen_by_bob = list(&app, &bob, dm.id).await;
    assert_eq!(seen_by_bob[0].attachments[0].filename, "dm.txt");
    let file = download(&app, &seen_by_bob[0].attachments[0].file, &[]).await;
    assert_eq!((file.status, file.body.as_slice()), (200, b"secret".as_slice()));

    assert_eq!(media::totals(&app.state, "global:attachment").await.unwrap(), (6, 1));
    assert_eq!(media::totals(&app.state, &format!("guild:{}:attachment", guild.id)).await.unwrap(), (0, 0));
    // Bob can send his own file into the DM; Alice's pending upload is not his.
    let other = upload(&app, &bob, dm.id, "b.txt", "text/plain", b"b").await;
    assert_code(send(&app, &alice, dm.id, "x", &[pending_id(&other)]).await, ErrorCode::InvalidArgument);
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
async fn message_updated_helper_publishes_the_full_message() {
    let app = TestApp::start_with_attachments(AttachmentSettings::default()).await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let channel = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let pending = upload(&app, &alice, channel, "a.txt", "text/plain", b"hello").await;
    let sent = send(&app, &alice, channel, "hi", &[pending_id(&pending)]).await.unwrap();
    app.client(MessageServiceClient::new, Some(&alice.token))
        .add_reaction(AddReactionRequest {
            channel_id: channel,
            message_id: sent.id,
            emoji: ReactionEmoji { kind: Some(reaction_emoji::Kind::Unicode("👍".into())), ..Default::default() }.into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let mut stream = app
        .client(GatewayServiceClient::new, Some(&bob.token))
        .subscribe(SubscribeRequest::default())
        .await
        .unwrap();
    // Subscribing is asynchronous on the server side: wait for the first heartbeat.
    let first = timeout(WAIT, stream.message()).await.unwrap().unwrap().unwrap().to_owned_message();
    assert!(matches!(first.event.into_option().and_then(|e| e.payload), Some(Payload::Heartbeat(_))));

    publish_message_updated(&app.state, channel, sent.id).await.unwrap();
    let Payload::MessageUpdated(updated) = next_payload!(stream) else { panic!("expected MessageUpdated") };
    let message = updated.message.into_option().expect("message");
    assert_eq!((message.id, message.content.as_str()), (sent.id, "hi"));
    assert_eq!(message.attachments, sent.attachments);
    assert_eq!(message.reactions.len(), 1);
    assert!(!message.reactions[0].me, "broadcasts carry no per-viewer state");
    assert_code(publish_message_updated(&app.state, channel, sent.id + 1).await.map(|_| ()), ErrorCode::NotFound);
    app.cleanup().await;
}
