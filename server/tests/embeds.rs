//! Link embeds against the real Scylla + S3 services from `docker-compose.yml`.
//! The pages come from a local axum site started by each test; the fetcher
//! may reach IPv4 loopback only because the tests set
//! `allow_loopback_for_tests` (private and link-local targets stay blocked).
mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use common::TestApp;
use common::fixtures::*;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use thehideout_server::embeds::EmbedSettings;
use thehideout_server::media;
use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::*;
use tokio::time::{sleep, timeout};

const WAIT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------- test site

#[derive(Default)]
struct Site {
    hits: Mutex<HashMap<String, usize>>,
    /// Bytes streamed by the endless pages.
    sent: AtomicUsize,
    /// Port of the `[::1]` listener (0 if IPv6 is unavailable) and what reached it.
    v6_port: AtomicU16,
    v6_hits: AtomicUsize,
    port: AtomicU16,
}

impl Site {
    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }

    fn total_hits(&self) -> usize {
        self.hits.lock().unwrap().values().sum()
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port.load(Ordering::SeqCst))
    }
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbaImage::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255]));
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8).unwrap();
    out
}

fn page(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

fn og(image: &str) -> Response {
    page(format!(
        "<!doctype html><html><head><title>Fallback title</title>\
         <meta property=\"og:title\" content=\"OG &amp; Title\">\
         <meta property=\"og:description\" content=\"A   description\">\
         <meta property=\"og:site_name\" content=\"Test Site\">\
         <meta property=\"og:image\" content=\"{image}\"></head><body>hello</body></html>"
    ))
}

fn redirect(status: u16, to: &str) -> Response {
    (StatusCode::from_u16(status).unwrap(), [(header::LOCATION, to.to_owned())]).into_response()
}

fn typed(content_type: &'static str, body: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// A body that never ends (and stalls when `stall`): `prefix`, then filler.
fn endless(site: Arc<Site>, prefix: &'static str, stall: bool) -> Response {
    let stream = futures::stream::unfold((site, true), move |(site, first)| async move {
        if !first && stall {
            sleep(Duration::from_secs(60)).await;
        }
        let chunk = if first { Bytes::from(prefix) } else { Bytes::from(vec![b' '; 64 * 1024]) };
        site.sent.fetch_add(chunk.len(), Ordering::SeqCst);
        Some((Ok::<_, std::io::Error>(chunk), (site, false)))
    });
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    response
}

async fn site_handler(State(site): State<Arc<Site>>, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().to_owned();
    *site.hits.lock().unwrap().entry(path.clone()).or_default() += 1;
    let header_of = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("none").to_owned();
    match path.as_str() {
        "/og" | "/cached" => og("/img.png"),
        "/og-title-only" => page("<head><title>Only a title</title><meta name=\"description\" content=\"Meta &lt;description&gt;\"></head>".into()),
        "/og-no-site" => page("<head><meta property=\"og:title\" content=\"No site\"></head>".into()),
        "/img.png" | "/direct.png" => typed("image/png", png(800, 600)),
        "/r1" => redirect(302, "/r2"),
        "/r2" => redirect(301, "/r3"),
        "/r3" => redirect(307, "/og"),
        "/r4a" => redirect(302, "/r4b"),
        "/r4b" => redirect(302, "/r4c"),
        "/r4c" => redirect(302, "/r4d"),
        "/r4d" => redirect(302, "/og"),
        "/loop" => redirect(302, "/loop"),
        "/to-10" => redirect(302, "http://10.0.0.1/og"),
        "/to-169" => redirect(302, "http://169.254.169.254/latest/meta-data/"),
        "/to-192" => redirect(302, "http://192.168.1.1/og"),
        "/to-cgnat" => redirect(302, "http://100.64.0.1/og"),
        "/to-zero" => redirect(302, "http://0.0.0.0/og"),
        "/to-mapped" => redirect(302, "http://[::ffff:10.0.0.1]/og"),
        "/to-decimal" => redirect(302, "http://167772161/og"),
        "/to-v6" => redirect(302, &format!("http://[::1]:{}/og", site.v6_port.load(Ordering::SeqCst))),
        "/to-file" => redirect(302, "file:///etc/passwd"),
        "/to-ftp" => redirect(302, "ftp://127.0.0.1/og"),
        "/to-10-after-ok" => redirect(302, "/to-169"),
        "/huge-title" => endless(site, "<html><head><title>Huge page</title>", false),
        "/huge-late" => endless(site, "<html><head>", false),
        "/slow" => {
            sleep(Duration::from_secs(4)).await;
            og("/img.png")
        }
        "/slow-body" => endless(site, "<html><head><title>Partial</title>", true),
        "/priv-img" => page("<head><title>Private image</title><meta property=\"og:image\" content=\"http://10.0.0.1/x.png\"></head>".into()),
        "/priv-img-v6" => {
            page("<head><title>Private image</title><meta property=\"og:image\" content=\"http://[::1]/x.png\"></head>".into())
        }
        "/big-img" => page("<head><title>Big image</title><meta property=\"og:image\" content=\"/big.png\"></head>".into()),
        "/big.png" => typed("image/png", vec![0u8; 6 * 1024 * 1024]),
        "/html-img" => page("<head><title>HTML image</title><meta property=\"og:image\" content=\"/notimg\"></head>".into()),
        "/notimg" => page("<html>not an image</html>".into()),
        "/bad-img" => page("<head><title>Bad image</title><meta property=\"og:image\" content=\"/bad.png\"></head>".into()),
        "/bad.png" => typed("image/png", b"definitely not a png".to_vec()),
        "/ua" => page(format!(
            "<head><meta property=\"og:title\" content=\"{}\"><meta property=\"og:description\" content=\"cookie:{}\"></head>",
            header_of("user-agent"),
            header_of("cookie")
        )),
        "/c1" => ([(header::SET_COOKIE, "a=b; Path=/")], redirect(302, "/c2")).into_response(),
        "/c2" => page(format!("<head><title>cookie:{}</title></head>", header_of("cookie"))),
        "/text" => typed("text/plain", b"<title>not html</title>".to_vec()),
        "/json" => typed("application/json", b"{\"title\":\"x\"}".to_vec()),
        "/gone" => StatusCode::NOT_FOUND.into_response(),
        "/empty" => page("<html><head></head><body>nothing</body></html>".into()),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn start_site() -> Arc<Site> {
    let site = Arc::new(Site::default());
    let router = Router::new().fallback(site_handler).with_state(site.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    site.port.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Something listening on ::1 shows whether a redirect to it was (wrongly) followed.
    if let Ok(v6) = tokio::net::TcpListener::bind("[::1]:0").await {
        site.v6_port.store(v6.local_addr().unwrap().port(), Ordering::SeqCst);
        let counter = site.clone();
        let router = Router::new().fallback(|State(s): State<Arc<Site>>| async move {
            s.v6_hits.fetch_add(1, Ordering::SeqCst);
            og("")
        });
        tokio::spawn(async move { axum::serve(v6, router.with_state(counter)).await.unwrap() });
    }
    site
}

// ------------------------------------------------------------------ harness

fn settings() -> EmbedSettings {
    let mut e = EmbedSettings::default();
    e.allow_loopback_for_tests = true;
    e
}

struct Harness {
    app: TestApp,
    alice: Account,
    guild: Guild,
    channel: u64,
    site: Arc<Site>,
}

async fn harness_with(embeds: EmbedSettings) -> Harness {
    let site = start_site().await;
    let app = TestApp::start_with_embeds(embeds).await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let channel = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    Harness { app, alice, guild, channel, site }
}

async fn harness() -> Harness {
    harness_with(settings()).await
}

/// Subscribes as `who` and waits for the first heartbeat (the subscription is live).
macro_rules! stream {
    ($app:expr, $who:expr) => {{
        let mut stream = $app.client(GatewayServiceClient::new, Some(&$who.token)).subscribe(SubscribeRequest::default()).await.unwrap();
        let first = timeout(WAIT, stream.message()).await.unwrap().unwrap().unwrap().to_owned_message();
        assert!(matches!(first.event.into_option().and_then(|e| e.payload), Some(Payload::Heartbeat(_))));
        stream
    }};
}

/// The next `message_updated` message (other events are skipped).
macro_rules! next_updated {
    ($stream:expr) => {{
        loop {
            let msg = timeout(WAIT, $stream.message()).await.expect("timed out waiting for message_updated").expect("stream error").expect("stream ended");
            if let Some(Payload::MessageUpdated(u)) = msg.to_owned_message().event.into_option().and_then(|e| e.payload) {
                break u.message.into_option().expect("message");
            }
        }
    }};
}

/// Waits for the unfurl jobs to finish, then asserts no `message_updated` was published.
macro_rules! assert_no_update {
    ($app:expr, $stream:expr) => {{
        settle(&$app).await;
        while let Ok(Ok(Some(msg))) = timeout(Duration::from_millis(300), $stream.message()).await {
            let payload = msg.to_owned_message().event.into_option().and_then(|e| e.payload);
            assert!(!matches!(payload, Some(Payload::MessageUpdated(_))), "unexpected message_updated: {payload:?}");
        }
    }};
}

async fn settle(app: &TestApp) {
    for _ in 0..800 {
        if app.state.embeds.in_flight() == 0 {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("embed jobs did not finish");
}

fn messages(app: &TestApp, who: &Account) -> MessageServiceClient<connectrpc::client::HttpClient> {
    app.client(MessageServiceClient::new, Some(&who.token))
}

async fn send(app: &TestApp, who: &Account, channel_id: u64, content: &str) -> Message {
    messages(app, who)
        .send_message(SendMessageRequest { channel_id, content: content.into(), ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .message
        .unwrap()
}

async fn list(app: &TestApp, who: &Account, channel_id: u64) -> Vec<Message> {
    messages(app, who)
        .list_messages(ListMessagesRequest { channel_id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .messages
}

async fn media_get(app: &TestApp, key: &str) -> (u16, Option<String>, Vec<u8>) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client.get(format!("{}/media/{key}", app.base_url)).send().await.unwrap();
    let status = response.status().as_u16();
    let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_owned);
    (status, content_type, response.bytes().await.unwrap().to_vec())
}

impl Harness {
    /// Sends `content` and asserts nothing is embedded (once the jobs are done).
    async fn expect_no_embed(&self, content: &str) {
        let mut stream = stream!(self.app, self.alice);
        let sent = send(&self.app, &self.alice, self.channel, content).await;
        assert_no_update!(self.app, stream);
        let listed = list(&self.app, &self.alice, self.channel).await;
        let message = listed.iter().find(|m| m.id == sent.id).unwrap();
        assert!(message.embeds.is_empty(), "{content}: {:?}", message.embeds);
    }

    async fn expect_embed(&self, content: &str) -> Message {
        let mut stream = stream!(self.app, self.alice);
        let sent = send(&self.app, &self.alice, self.channel, content).await;
        assert!(sent.embeds.is_empty(), "SendMessage does not wait for embeds");
        let updated = next_updated!(stream);
        assert_eq!(updated.id, sent.id);
        let listed = list(&self.app, &self.alice, self.channel).await;
        let from_list = listed.iter().find(|m| m.id == sent.id).unwrap();
        assert_eq!(from_list.embeds, updated.embeds, "ListMessages and message_updated agree");
        updated
    }
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn embed_arrives_via_message_updated_and_list_with_a_served_thumbnail() {
    let h = harness().await;
    let og_url = h.site.url("/og");
    let updated = h.expect_embed(&format!("look at {og_url} please")).await;
    assert_eq!(updated.content, format!("look at {og_url} please"));
    assert_eq!(updated.embeds.len(), 1);
    let embed = &updated.embeds[0];
    assert_eq!(embed.url, og_url);
    assert_eq!(embed.title, "OG & Title");
    assert_eq!(embed.description, "A description");
    assert_eq!(embed.site_name, "Test Site");
    // 800x600 fits within 400x400 as 400x300.
    assert_eq!((embed.thumbnail_width, embed.thumbnail_height), (Some(400), Some(300)));
    let key = embed.thumbnail.clone().expect("thumbnail");
    assert!(key.starts_with(&format!("embeds/{}/", h.channel)) && key.ends_with(".webp"), "{key}");

    let (status, content_type, body) = media_get(&h.app, &key).await;
    assert_eq!((status, content_type.as_deref()), (200, Some("image/webp")));
    let decoded = image::load_from_memory_with_format(&body, image::ImageFormat::WebP).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (400, 300));

    // Accounting: one object of kind `embed`, counted for the guild and globally.
    let bytes = body.len() as i64;
    assert_eq!(media::totals(&h.app.state, &format!("guild:{}:embed", h.guild.id)).await.unwrap(), (bytes, 1));
    assert_eq!(media::totals(&h.app.state, "global:embed").await.unwrap(), (bytes, 1));
    assert_eq!(media::totals(&h.app.state, &format!("guild:{}", h.guild.id)).await.unwrap(), (bytes, 1));
    let by_kind = media::totals_by_kind(&h.app.state, Some(h.guild.id)).await.unwrap();
    assert_eq!(by_kind.iter().find(|k| k.kind == "embed").unwrap().bytes, bytes);
    let (owner_kind, guild_id, kind): (String, Option<i64>, String) = h
        .app
        .state
        .session
        .execute_unpaged("SELECT owner_kind, guild_id, kind FROM media_objects WHERE key = ?", (&key,))
        .await
        .unwrap()
        .into_rows_result()
        .unwrap()
        .first_row()
        .unwrap();
    assert_eq!((owner_kind.as_str(), guild_id, kind.as_str()), ("guild", Some(h.guild.id as i64), "embed"));
    h.app.cleanup().await;
}

#[tokio::test]
async fn title_and_description_fallbacks_and_host_as_site_name() {
    let h = harness().await;
    let updated = h.expect_embed(&h.site.url("/og-title-only")).await;
    let embed = &updated.embeds[0];
    assert_eq!((embed.title.as_str(), embed.description.as_str()), ("Only a title", "Meta <description>"));
    assert_eq!(embed.site_name, "127.0.0.1");
    assert_eq!((embed.thumbnail.clone(), embed.thumbnail_width, embed.thumbnail_height), (None, None, None));
    assert_eq!(media::totals(&h.app.state, "global:embed").await.unwrap(), (0, 0));
    // A page with an empty head yields nothing.
    h.expect_no_embed(&h.site.url("/empty")).await;
    h.app.cleanup().await;
}

#[tokio::test]
async fn direct_image_links_embed_just_the_image() {
    let h = harness().await;
    let url = h.site.url("/direct.png");
    let embed = h.expect_embed(&url).await.embeds.remove(0);
    assert_eq!((embed.url.as_str(), embed.title.as_str(), embed.description.as_str()), (url.as_str(), "", ""));
    assert_eq!((embed.thumbnail_width, embed.thumbnail_height), (Some(400), Some(300)));
    let (status, _, _) = media_get(&h.app, embed.thumbnail.as_deref().unwrap()).await;
    assert_eq!(status, 200);
    // One fetch: the image of the first response is the thumbnail source.
    assert_eq!(h.site.hits("/direct.png"), 1);
    h.app.cleanup().await;
}

#[tokio::test]
async fn up_to_three_redirects_are_followed_a_fourth_is_not() {
    let h = harness().await;
    let embed = h.expect_embed(&h.site.url("/r1")).await.embeds.remove(0);
    assert_eq!((embed.url.as_str(), embed.title.as_str()), (h.site.url("/r1").as_str(), "OG & Title"));
    let og_hits = h.site.hits("/og");
    h.expect_no_embed(&h.site.url("/r4a")).await;
    assert_eq!(h.site.hits("/r4d"), 1, "the third redirect target was requested");
    assert_eq!(h.site.hits("/og"), og_hits, "the fourth redirect was not followed");
    h.expect_no_embed(&h.site.url("/loop")).await;
    assert!(h.site.hits("/loop") <= 4);
    h.app.cleanup().await;
}

#[tokio::test]
async fn redirects_to_private_targets_are_blocked_even_with_loopback_allowed() {
    let h = harness().await;
    for path in [
        "/to-10", "/to-169", "/to-192", "/to-cgnat", "/to-zero", "/to-mapped", "/to-decimal", "/to-v6", "/to-file", "/to-ftp",
        "/to-10-after-ok",
    ] {
        h.expect_no_embed(&h.site.url(path)).await;
    }
    assert_eq!(h.site.v6_hits.load(Ordering::SeqCst), 0, "::1 was never contacted");
    assert_eq!(h.site.hits("/og"), 0);
    // The same hops are fine when the target is public-or-loopback-for-tests: control.
    h.expect_embed(&h.site.url("/r1")).await;
    // Direct private URLs in messages.
    h.expect_no_embed("http://10.0.0.1/ http://169.254.169.254/latest/meta-data/ http://[::1]:8080/").await;
    assert_eq!(h.site.v6_hits.load(Ordering::SeqCst), 0);
    h.app.cleanup().await;
}

#[tokio::test]
async fn loopback_is_blocked_without_the_test_flag() {
    let h = harness_with(EmbedSettings::default()).await;
    h.expect_no_embed(&h.site.url("/og")).await;
    h.expect_no_embed("http://localhost:8080/ http://127.0.0.1/ http://[::1]/").await;
    assert_eq!(h.site.total_hits(), 0, "the site was never contacted");
    h.app.cleanup().await;
}

#[tokio::test]
async fn embeds_can_be_switched_off() {
    let h = harness_with({
        let mut e = settings();
        e.enabled = false;
        e
    })
    .await;
    h.expect_no_embed(&h.site.url("/og")).await;
    assert_eq!(h.site.total_hits(), 0);
    h.app.cleanup().await;
}

#[tokio::test]
async fn huge_pages_are_read_only_up_to_the_cap() {
    let h = harness().await;
    // The title is early: read, then the connection is dropped at 1 MiB.
    let embed = h.expect_embed(&h.site.url("/huge-title")).await.embeds.remove(0);
    assert_eq!(embed.title, "Huge page");
    let sent = h.site.sent.load(Ordering::SeqCst);
    assert!((1024 * 1024..8 * 1024 * 1024).contains(&sent), "streamed {sent} bytes");
    // Nothing useful inside the first MiB.
    h.site.sent.store(0, Ordering::SeqCst);
    h.expect_no_embed(&h.site.url("/huge-late")).await;
    let sent = h.site.sent.load(Ordering::SeqCst);
    assert!(sent < 8 * 1024 * 1024, "streamed {sent} bytes");
    h.app.cleanup().await;
}

#[tokio::test]
async fn slow_pages_time_out() {
    let h = harness_with({
        let mut e = settings();
        e.total_timeout = Duration::from_millis(700);
        e
    })
    .await;
    let started = Instant::now();
    h.expect_no_embed(&h.site.url("/slow")).await;
    h.expect_no_embed(&h.site.url("/slow-body")).await;
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(h.site.hits("/slow"), 1);
    h.app.cleanup().await;
}

#[tokio::test]
async fn a_saturated_queue_drops_new_jobs() {
    let h = harness_with({
        let mut e = settings().with_max_concurrent(1);
        e.total_timeout = Duration::from_millis(1500);
        e
    })
    .await;
    let mut stream = stream!(h.app, h.alice);
    send(&h.app, &h.alice, h.channel, &h.site.url("/slow")).await;
    send(&h.app, &h.alice, h.channel, &h.site.url("/og")).await;
    assert_no_update!(h.app, stream);
    assert_eq!(h.site.hits("/og"), 0, "second job was dropped, not queued");
    // Slots are free again afterwards.
    h.expect_embed(&h.site.url("/og")).await;
    h.app.cleanup().await;
}

#[tokio::test]
async fn unsafe_preview_images_are_dropped_but_the_text_stays() {
    let h = harness().await;
    for path in ["/priv-img", "/priv-img-v6", "/big-img", "/html-img", "/bad-img"] {
        let embed = h.expect_embed(&h.site.url(path)).await.embeds.remove(0);
        assert!(!embed.title.is_empty(), "{path}");
        assert_eq!((embed.thumbnail.as_deref(), embed.thumbnail_width, embed.thumbnail_height), (None, None, None), "{path}");
    }
    assert_eq!(h.site.v6_hits.load(Ordering::SeqCst), 0);
    assert_eq!(media::totals(&h.app.state, "global:embed").await.unwrap(), (0, 0));
    h.app.cleanup().await;
}

#[tokio::test]
async fn non_html_and_failed_pages_get_no_embed() {
    let h = harness().await;
    for path in ["/text", "/json", "/gone", "/nowhere"] {
        h.expect_no_embed(&h.site.url(path)).await;
    }
    // Unresolvable host.
    h.expect_no_embed("https://this-host-does-not-exist.invalid/").await;
    h.app.cleanup().await;
}

#[tokio::test]
async fn url_rules_code_is_ignored_brackets_are_not_and_the_cap_is_three() {
    let h = harness().await;
    let (og, plain, nosite) = (h.site.url("/og"), h.site.url("/og-title-only"), h.site.url("/og-no-site"));
    // Only code excludes a URL.
    h.expect_no_embed(&format!("`{og}`")).await;
    h.expect_no_embed(&format!("```\n{og}\n```")).await;
    h.expect_no_embed(&format!("```\n{og}")).await;
    h.expect_no_embed(&format!("``{og}``")).await;
    assert_eq!(h.site.total_hits(), 0, "none of those URLs was fetched");

    // `<url>` and masked links are embedded like bare URLs.
    let embeds = h.expect_embed(&format!("<{og}>")).await.embeds;
    assert_eq!(embeds.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), [og.as_str()]);
    let embeds = h.expect_embed(&format!("[look]({plain})")).await.embeds;
    assert_eq!(embeds.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), [plain.as_str()]);
    let embeds = h.expect_embed(&format!("[look](<{nosite}>) and `{og}`")).await.embeds;
    assert_eq!(embeds.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), [nosite.as_str()]);
    assert_eq!(h.site.hits("/og"), 1, "only the first <url> message fetched it (the code span did not)");

    // Five URLs (one repeated): at most three, in order, each once.
    let (a, b) = (h.site.url("/og?a"), h.site.url("/og?b"));
    let embeds = h.expect_embed(&format!("{a} {b} {a} {plain} {nosite} {og}")).await.embeds;
    assert_eq!(embeds.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), [a.as_str(), b.as_str(), plain.as_str()]);
    assert_eq!(h.site.hits("/og-no-site"), 1, "the fourth distinct URL was not fetched again");
    h.app.cleanup().await;
}

#[tokio::test]
async fn cached_metadata_is_reused_but_every_embed_owns_its_thumbnail() {
    let h = harness().await;
    let url = h.site.url("/cached");
    let first = h.expect_embed(&url).await.embeds.remove(0);
    let second = h.expect_embed(&url).await.embeds.remove(0);
    assert_eq!(h.site.hits("/cached"), 1, "the page was fetched once");
    assert_eq!(h.site.hits("/img.png"), 2, "the image is fetched per embed");
    assert_eq!((first.title.as_str(), first.site_name.as_str()), (second.title.as_str(), second.site_name.as_str()));
    let (k1, k2) = (first.thumbnail.unwrap(), second.thumbnail.unwrap());
    assert_ne!(k1, k2);
    assert_eq!(media::totals(&h.app.state, "global:embed").await.unwrap().1, 2);
    assert_eq!(media::totals(&h.app.state, &format!("guild:{}:embed", h.guild.id)).await.unwrap().1, 2);
    // Deleting one record only uncounts that object.
    media::delete_record(&h.app.state, &k1).await;
    assert_eq!(media::totals(&h.app.state, "global:embed").await.unwrap().1, 1);
    h.app.cleanup().await;
}

#[tokio::test]
async fn failed_unfurls_are_cached_negatively() {
    let h = harness().await;
    h.expect_no_embed(&h.site.url("/empty")).await;
    h.expect_no_embed(&h.site.url("/empty")).await;
    assert_eq!(h.site.hits("/empty"), 1);
    h.app.cleanup().await;
}

#[tokio::test]
async fn direct_messages_count_embeds_globally_only() {
    let h = harness().await;
    let bob = user(&h.app, "bob").await;
    let dm = h
        .app
        .client(DmServiceClient::new, Some(&h.alice.token))
        .open_dm(OpenDmRequest { user_id: bob.id, ..Default::default() })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    let mut stream = stream!(h.app, bob);
    let sent = send(&h.app, &h.alice, dm.id, &h.site.url("/og")).await;
    let updated = next_updated!(stream);
    assert_eq!((updated.id, updated.guild_id, updated.embeds.len()), (sent.id, 0, 1));
    let listed = list(&h.app, &bob, dm.id).await;
    assert_eq!(listed[0].embeds, updated.embeds);
    let key = updated.embeds[0].thumbnail.clone().unwrap();
    assert!(key.starts_with(&format!("embeds/{}/", dm.id)));
    assert_eq!(media::totals(&h.app.state, "global:embed").await.unwrap().1, 1);
    assert_eq!(media::totals(&h.app.state, &format!("guild:{}:embed", h.guild.id)).await.unwrap(), (0, 0));
    h.app.cleanup().await;
}

#[tokio::test]
async fn the_bot_identifies_itself_and_sends_no_cookies() {
    let h = harness().await;
    let embed = h.expect_embed(&h.site.url("/ua")).await.embeds.remove(0);
    assert!(embed.title.starts_with("thehideout-embed/0.1"), "{}", embed.title);
    assert_eq!(embed.description, "cookie:none");
    // A cookie set on the way is not sent on the next hop.
    let embed = h.expect_embed(&h.site.url("/c1")).await.embeds.remove(0);
    assert_eq!(embed.title, "cookie:none");
    h.app.cleanup().await;
}

#[tokio::test]
async fn media_route_serves_only_well_formed_embed_keys() {
    let h = harness().await;
    let zeros = "0".repeat(32);
    for key in [
        format!("embeds/1/{zeros}.webp"),
        format!("embeds/1/{zeros}.gif"),
        format!("embeds/1/{zeros}"),
        format!("embeds/x/{zeros}.webp"),
        format!("embeds/1/../{zeros}.webp"),
    ] {
        let (status, _, _) = media_get(&h.app, &key).await;
        assert_eq!(status, 404, "{key}");
    }
    h.app.cleanup().await;
}

#[tokio::test]
async fn the_safe_client_streams_large_downloads_into_storage_with_a_per_call_cap() {
    use thehideout_server::embeds::fetch::{FetchError, Fetcher};
    use thehideout_server::embeds::ssrf::{Blocked, Policy};
    use thehideout_server::storage::Storage;

    let site = start_site().await;
    let storage = Storage::in_memory();
    let fetcher = Fetcher {
        policy: Policy { allow_loopback_for_tests: true },
        connect_timeout: Duration::from_secs(2),
        total_timeout: Duration::from_secs(5),
        max_redirects: 3,
        user_agent: "test".into(),
    };
    let big = url::Url::parse(&site.url("/big.png")).unwrap();
    let size = fetcher
        .get(&big)
        .await
        .unwrap()
        .download_to_storage(8 * 1024 * 1024, &storage, "embeds/1/big", "image/png")
        .await
        .unwrap();
    assert_eq!(size, 6 * 1024 * 1024);
    assert_eq!(storage.head("embeds/1/big").await.unwrap().unwrap().size, size);

    // The cap is per call; an over-large body (declared or not) leaves no object behind.
    let err = fetcher.get(&big).await.unwrap().download_to_storage(1024 * 1024, &storage, "embeds/1/small", "image/png").await;
    assert!(matches!(err, Err(FetchError::TooLarge)), "{err:?}");
    assert!(storage.head("embeds/1/small").await.unwrap().is_none());
    let endless = url::Url::parse(&site.url("/huge-title")).unwrap();
    let err = fetcher.get(&endless).await.unwrap().download_to_storage(512 * 1024, &storage, "embeds/1/endless", "text/html").await;
    assert!(matches!(err, Err(FetchError::TooLarge)), "{err:?}");
    assert!(storage.head("embeds/1/endless").await.unwrap().is_none());

    // The same vetting applies to every download, and deadlines are per client.
    let private = url::Url::parse("http://10.0.0.1/video.mp4").unwrap();
    assert!(matches!(fetcher.get(&private).await, Err(FetchError::Blocked(Blocked::Address(_)))));
    let slow = fetcher.with_total_timeout(Duration::from_millis(300));
    let result = slow.within(async { slow.get(&url::Url::parse(&site.url("/slow")).unwrap()).await.map(|_| ()) }).await;
    assert!(matches!(result, Err(FetchError::Timeout)), "{result:?}");
}
