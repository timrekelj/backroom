//! Provider embeds (X, TikTok, Instagram, Reddit) against the real Scylla + S3
//! services. No test reaches the network: each test starts a local site that
//! plays every mirror and API (the mirror lists point at it) with small
//! synthetic JSON/HTML fixtures shaped like the real responses, and tiny fake
//! MP4 files. Live checks against the real mirrors are `#[ignore]`d at the end
//! (`cargo test --test embed_providers -- --ignored`).
mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use common::TestApp;
use common::fixtures::*;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use serde_json::{Value, json};
use thehideout_server::embeds::EmbedSettings;
use thehideout_server::embeds::providers::Mirrors;
use thehideout_server::proto::thehideout::v1::*;
use tokio::time::sleep;

/// Cap used by the tests: the "hi" video is over it, the "lo" one under.
const VIDEO_CAP: usize = 16 * 1024;

// ---------------------------------------------------------------- test site

#[derive(Default)]
struct Site {
    hits: Mutex<HashMap<String, usize>>,
    agents: Mutex<Vec<String>>,
    port: AtomicU16,
}

impl Site {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port.load(Ordering::SeqCst))
    }

    fn hits(&self, prefix: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|(p, _)| p.starts_with(prefix)).map(|(_, n)| n).sum()
    }
}

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = RgbaImage::from_fn(w, h, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, seed, 255]));
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8).unwrap();
    out
}

/// A fake MP4: a valid `ftyp` box followed by `len - 32` zero bytes.
fn mp4(len: usize) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 2, 0];
    v.extend_from_slice(b"isomiso2avc1mp41");
    v.resize(len, 0);
    v
}

fn page(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

fn json_response(value: Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

fn redirect(to: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, to.to_owned())]).into_response()
}

fn typed(content_type: &'static str, body: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn og(tags: &[(&str, &str)]) -> Response {
    let metas: String = tags.iter().map(|(k, v)| format!("<meta property=\"{k}\" content=\"{v}\">")).collect();
    page(format!("<!doctype html><html><head>{metas}</head><body></body></html>"))
}

/// An FxTwitter-shaped response.
fn fx(site: &Site, text: &str, media: Vec<Value>) -> Response {
    json_response(json!({
        "code": 200, "message": "OK",
        "tweet": {
            "url": "https://x.com/nasa/status/1", "id": "1", "text": text,
            "author": {"screen_name": "nasa", "name": "NASA", "url": "https://x.com/nasa", "avatar_url": site.url("/avatar.png")},
            "created_timestamp": 1666809958,
            "media": {"all": media},
        }
    }))
}

fn fx_photo(site: &Site, n: usize) -> Value {
    json!({"type": "photo", "url": site.url(&format!("/img/{n}.png?name=orig")), "width": 640, "height": 480})
}

async fn site_handler(State(site): State<Arc<Site>>, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().to_owned();
    *site.hits.lock().unwrap().entry(path.clone()).or_default() += 1;
    site.agents.lock().unwrap().push(headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned());
    let site = &*site;
    let id_of = |p: &str| path.strip_prefix(p).map(|rest| rest.split('/').next().unwrap_or("").to_owned()).unwrap_or_default();
    match path.as_str() {
        // Shared media.
        "/avatar.png" => typed("image/png", png(64, 64, 10)),
        "/poster.png" => typed("image/png", png(320, 180, 20)),
        "/cdn/lo.mp4" | "/cdn/tt.mp4" | "/cdn/ig.mp4" | "/cdn/reddit.mp4" | "/cdn/silent.mp4" | "/cdn/gif.mp4" => typed("video/mp4", mp4(8_000)),
        "/cdn/hi.mp4" | "/cdn/huge.mp4" => typed("video/mp4", mp4(30_000)),
        "/cdn/notvideo.mp4" => typed("video/mp4", b"<html>this is not a video, just a page that claims to be one</html>".to_vec()),
        "/cdn/octet.mp4" => typed("application/octet-stream", mp4(8_000)),
        "/gen/tiktok.mp4" => redirect("/cdn/tt.mp4"),
        "/redditvideo.mp4" => redirect("/cdn/reddit.mp4"),
        p if p.starts_with("/img/") => typed("image/png", png(640, 480, p.len() as u8)),

        // X: FxTwitter-style API.
        p if p.starts_with("/fx/status/") => match id_of("/fx/status/").as_str() {
            "100" => fx(site, "Hello &amp; welcome\n\n\n\nto the show", vec![fx_photo(site, 1), fx_photo(site, 2)]),
            "101" => fx(
                site,
                "A video",
                vec![json!({
                    "type": "video", "url": site.url("/cdn/hi.mp4"), "width": 1280, "height": 720, "duration": 10.0,
                    "thumbnail_url": site.url("/poster.png"),
                    "variants": [
                        {"url": site.url("/x.m3u8"), "bitrate": 0, "content_type": "application/x-mpegURL"},
                        {"url": site.url("/cdn/hi.mp4"), "bitrate": 40000, "content_type": "video/mp4"},
                        {"url": site.url("/cdn/lo.mp4"), "bitrate": 8000, "content_type": "video/mp4"},
                    ],
                })],
            ),
            "102" => fx(site, "", vec![json!({"type": "gif", "url": site.url("/cdn/gif.mp4"), "format": "video/mp4", "width": 320, "height": 240, "thumbnail_url": site.url("/poster.png")})]),
            "103" => fx(site, "six photos", (1..=6).map(|n| fx_photo(site, n)).collect()),
            "107" => fx(site, "too big", vec![json!({"type": "video", "url": site.url("/cdn/huge.mp4"), "format": "video/mp4", "thumbnail_url": site.url("/poster.png"), "width": 1280, "height": 720})]),
            "108" => fx(site, "fake video", vec![json!({"type": "video", "url": site.url("/cdn/notvideo.mp4"), "format": "video/mp4"})]),
            "109" => fx(site, "private media", vec![json!({"type": "photo", "url": "http://10.0.0.1/a.png"}), json!({"type": "video", "url": "http://192.168.1.1/a.mp4", "format": "video/mp4"})]),
            "110" => fx(site, "octet stream video", vec![json!({"type": "video", "url": site.url("/cdn/octet.mp4"), "format": "video/mp4"})]),
            "104" | "105" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            _ => json_response(json!({"code": 404, "message": "NOT_FOUND", "tweet": null})),
        },
        // X: VxTwitter-style API.
        p if p.starts_with("/vx/") => match p.rsplit('/').next().unwrap_or("") {
            "104" => json_response(json!({
                "tweetID": "104", "text": "from the second API", "user_name": "NASA", "user_screen_name": "nasa",
                "user_profile_image_url": site.url("/avatar.png"), "date_epoch": 1666809958,
                "media_extended": [
                    {"type": "image", "url": site.url("/img/1.png"), "size": {"width": 640, "height": 480}},
                    {"type": "video", "url": site.url("/cdn/lo.mp4"), "thumbnail_url": site.url("/poster.png"), "duration_millis": 4200, "size": {"width": 640, "height": 480}},
                ],
            })),
            _ => StatusCode::BAD_GATEWAY.into_response(),
        },
        // X: OpenGraph page of a mirror.
        p if p.starts_with("/xh/") => match p.rsplit('/').next().unwrap_or("") {
            "105" => og(&[("og:title", "NASA (@nasa)"), ("og:description", "page only"), ("og:image", "/img/1.png")]),
            _ => StatusCode::NOT_FOUND.into_response(),
        },

        // TikTok: a mirror that is only a notice, a broken one, a good one.
        p if p.starts_with("/ttnotice/") => og(&[("og:title", "Notice"), ("og:description", "Due to a legal request, this service is no longer available.")]),
        p if p.starts_with("/ttbroken/") => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        p if p.starts_with("/ttgood/") => {
            let mut tags = vec![
                ("og:title", "Scout &amp; Co (@scout)"),
                ("og:description", "Scramble up ur name"),
                ("og:video", "/gen/tiktok.mp4"),
                ("og:video:width", "576"),
                ("og:video:height", "1024"),
            ];
            if p.ends_with("/2") {
                tags.push(("og:image", "/poster.png"));
            }
            og(&tags)
        }
        // Short links: vm → landing URL with the real shape.
        "/vm/abc" => redirect("/landed/vm2"),
        "/landed/vm2" => redirect("/@scout/video/1?utm=x"),
        p if p.starts_with("/@") => page("<html>landed</html>".into()),
        "/vm/loop" => redirect("/vm/loop"),
        "/vm/private" => redirect("http://10.0.0.1/@a/video/1"),
        "/vm/elsewhere" => redirect("/landed/profile"),
        p if p.starts_with("/landed/") => page("<html>landed</html>".into()),

        // Instagram.
        p if p.starts_with("/igdead/") => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        p if p.starts_with("/ightml/") => {
            let code = p.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            og(&[("twitter:title", "@nasa"), ("og:description", "A reel"), ("og:video", &format!("/videos/{code}/1")), ("og:image", "/poster.png")])
        }
        p if p.starts_with("/videos/") => redirect("/cdn/ig.mp4"),
        p if p.starts_with("/igredir/") => redirect("/cdn/ig.mp4"),
        p if p.starts_with("/iglogin/") => og(&[("og:title", "Login • Instagram"), ("og:description", "Log in to see this")]),

        // Reddit: mirror and JSON.
        p if p.starts_with("/rdmirror/") => match p.trim_end_matches('/').rsplit('/').next().unwrap_or("") {
            "vid123" => og(&[
                ("og:site_name", "u/alice on r/pics - ⬆️ 10 | 💬 2"),
                ("og:title", "A video post"),
                ("og:video", &site.url("/redditvideo.mp4?video_url=a&audio_url=b")),
                ("og:video:width", "720"),
                ("og:video:height", "1280"),
                ("og:image", "/poster.png"),
            ]),
            "gal001" => og(&[("og:site_name", "u/bob on r/pics - ⬆️ 1 | 💬 0"), ("og:title", "A gallery"), ("og:image", "/img/1.png")]),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        p if p.starts_with("/rj/comments/") => {
            let id = p.trim_start_matches("/rj/comments/").trim_end_matches(".json");
            let post = match id {
                "gal001" => json!({
                    "title": "A gallery", "author": "bob", "subreddit_name_prefixed": "r/pics", "created_utc": 1666809958.0, "is_gallery": true,
                    "gallery_data": {"items": [{"media_id": "a"}, {"media_id": "b"}, {"media_id": "c"}]},
                    "media_metadata": {
                        "a": {"status": "valid", "s": {"u": site.url("/img/1.png"), "x": 640, "y": 480}},
                        "b": {"status": "valid", "s": {"u": site.url("/img/2.png"), "x": 640, "y": 480}},
                        "c": {"status": "failed", "s": {"u": site.url("/img/3.png")}},
                    },
                }),
                "sil001" => json!({
                    "title": "A silent clip", "author": "carol", "subreddit_name_prefixed": "r/videos", "is_video": true,
                    "media": {"reddit_video": {"fallback_url": site.url("/cdn/silent.mp4"), "width": 720, "height": 1280, "duration": 11, "has_audio": true, "is_gif": false}},
                    "preview": {"images": [{"source": {"url": site.url("/poster.png"), "width": 320, "height": 180}}]},
                }),
                "txt001" => json!({"title": "A text post", "author": "dave", "subreddit_name_prefixed": "r/askreddit", "selftext": "Line one\n\nLine two"}),
                _ => return StatusCode::NOT_FOUND.into_response(),
            };
            json_response(json!([{"data": {"children": [{"data": post}]}}, {"data": {"children": []}}]))
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn start_site() -> Arc<Site> {
    let site = Arc::new(Site::default());
    let router = Router::new().fallback(site_handler).with_state(site.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    site.port.store(listener.local_addr().unwrap().port(), Ordering::SeqCst);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    site
}

// ------------------------------------------------------------------ harness

struct Harness {
    app: TestApp,
    alice: Account,
    channel: u64,
    guild_id: u64,
    site: Arc<Site>,
}

fn settings_for(site: &Site) -> EmbedSettings {
    let mut e = EmbedSettings::default();
    e.allow_loopback_for_tests = true;
    e.max_video_bytes = VIDEO_CAP;
    e.provider_timeout = Duration::from_secs(8);
    e.attempt_timeout = Duration::from_secs(4);
    e.mirrors = Mirrors::none()
        .with("x_api", &[&site.url("/fx")])
        .with("x_vx_api", &[&site.url("/vx")])
        .with("x_html", &[&site.url("/xh")])
        .with("tiktok", &[&site.url("/ttbroken"), &site.url("/ttnotice"), &site.url("/ttgood")])
        .with("instagram", &[&site.url("/igdead"), &site.url("/iglogin"), &site.url("/ightml")])
        .with("reddit", &[&site.url("/rdmirror")])
        .with("reddit_json", &[&site.url("/rj")]);
    e
}

async fn harness() -> Harness {
    harness_with(|_, e| e).await
}

async fn harness_with(tweak: impl FnOnce(&Site, EmbedSettings) -> EmbedSettings) -> Harness {
    let site = start_site().await;
    let settings = tweak(&site, settings_for(&site));
    let app = TestApp::start_with_embeds(settings).await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let channel = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    Harness { app, alice, channel, guild_id: guild.id, site }
}

fn messages(app: &TestApp, who: &Account) -> MessageServiceClient<connectrpc::client::HttpClient> {
    app.client(MessageServiceClient::new, Some(&who.token))
}

async fn settle(app: &TestApp) {
    for _ in 0..1200 {
        if app.state.embeds.in_flight() == 0 {
            return;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("embed jobs did not finish");
}

impl Harness {
    /// Sends `content`, waits for the unfurl job and returns the listed embeds.
    async fn embeds(&self, content: &str) -> Vec<Embed> {
        let sent = messages(&self.app, &self.alice)
            .send_message(SendMessageRequest { channel_id: self.channel, content: content.into(), ..Default::default() })
            .await
            .unwrap()
            .into_owned()
            .message
            .unwrap();
        settle(&self.app).await;
        let listed = messages(&self.app, &self.alice)
            .list_messages(ListMessagesRequest { channel_id: self.channel, ..Default::default() })
            .await
            .unwrap()
            .into_owned()
            .messages;
        listed.into_iter().find(|m| m.id == sent.id).unwrap().embeds
    }

    async fn one(&self, content: &str) -> Embed {
        let mut embeds = self.embeds(content).await;
        assert_eq!(embeds.len(), 1, "{content}: {embeds:?}");
        embeds.remove(0)
    }

    /// `(key, kind, bytes)` of every accounted object.
    async fn objects(&self) -> Vec<(String, String, i64)> {
        self.app
            .state
            .session
            .execute_unpaged("SELECT key, kind, bytes FROM media_objects", ())
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .rows::<(String, Option<String>, Option<i64>)>()
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(k, kind, b)| (k, kind.unwrap_or_default(), b.unwrap_or(0)))
            .filter(|(k, _, _)| k.starts_with(&format!("embeds/{}/", self.channel)))
            .collect()
    }
}

async fn media_get(app: &TestApp, key: &str, range: Option<&str>) -> reqwest::Response {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut request = client.get(format!("{}/media/{key}", app.base_url));
    if let Some(range) = range {
        request = request.header("range", range);
    }
    request.send().await.unwrap()
}

fn kinds(embed: &Embed) -> Vec<EmbedMediaKind> {
    embed.media.iter().map(|m| m.kind.as_known().unwrap()).collect()
}

fn kind_of(embed: &Embed) -> EmbedKind {
    embed.kind.as_known().unwrap()
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn x_photos_become_a_rich_embed_with_stored_webp_images() {
    let h = harness().await;
    let url = "https://x.com/NASA/status/100?s=20";
    let embed = h.one(url).await;
    assert_eq!(embed.url, url, "the message keeps the original link");
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_RICH);
    assert_eq!(embed.provider, "x");
    assert_eq!(embed.description, "Hello & welcome\n\nto the show");
    assert_eq!(embed.site_name, "X");
    assert_eq!(embed.author_name, "NASA (@nasa)");
    assert_eq!(embed.author_url.as_deref(), Some("https://x.com/nasa"));
    assert_eq!(embed.published_at.as_option().map(|t| t.seconds), Some(1666809958));
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_IMAGE; 2]);
    for m in &embed.media {
        assert!(m.file.starts_with(&format!("embeds/{}/", h.channel)) && m.file.ends_with(".webp"));
        assert_eq!((m.width, m.height), (640, 480), "measured from the stored file");
        let response = media_get(&h.app, &m.file, None).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/webp");
        assert_eq!(response.bytes().await.unwrap().len() as u64, m.size);
    }
    // Legacy thumbnail and the author's small avatar.
    let thumb = embed.thumbnail.clone().expect("legacy thumbnail");
    assert_eq!((embed.thumbnail_width, embed.thumbnail_height), (Some(400), Some(300)));
    assert_eq!(media_get(&h.app, &thumb, None).await.status(), 200);
    let icon = embed.author_icon.clone().expect("author icon");
    assert_eq!(media_get(&h.app, &icon, None).await.status(), 200);
    // Accounting: one `embed` row per stored file (2 images + thumbnail + icon), summing to their sizes.
    let objects = h.objects().await;
    assert_eq!(objects.len(), 4, "{objects:?}");
    assert!(objects.iter().all(|(_, kind, _)| kind == "embed"));
    h.app.cleanup().await;
}

#[tokio::test]
async fn mirror_requests_use_the_browser_style_bot_user_agent() {
    let h = harness().await;
    h.one("https://x.com/NASA/status/100").await;
    let agents = h.site.agents.lock().unwrap().clone();
    assert!(!agents.is_empty());
    assert!(agents.iter().all(|a| a.starts_with("Mozilla/5.0 (compatible; thehideout-embed/0.1;")), "{agents:?}");
    h.app.cleanup().await;
}

#[tokio::test]
async fn x_video_picks_the_best_variant_under_the_cap_and_is_served_with_ranges() {
    let h = harness().await;
    let embed = h.one("https://twitter.com/NASA/status/101").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
    assert_eq!(embed.media.len(), 1);
    let video = &embed.media[0];
    assert_eq!(video.kind.as_known(), Some(EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO));
    assert!(video.file.ends_with(".mp4"));
    assert_eq!(video.size, 8_000, "the lo variant: the hi one is estimated over the cap");
    assert_eq!((video.width, video.height), (1280, 720));
    assert_eq!(video.duration_ms, Some(10_000));
    assert_eq!(h.site.hits("/cdn/hi.mp4"), 0, "the over-cap variant is never downloaded");
    let poster = video.poster.clone().expect("poster");
    assert!(poster.ends_with(".webp"));
    assert!(embed.thumbnail.is_some(), "the poster doubles as the legacy thumbnail");

    // Served as video/mp4, inline, with ranges.
    let whole = media_get(&h.app, &video.file, None).await;
    assert_eq!(whole.status(), 200);
    assert_eq!(whole.headers()["content-type"], "video/mp4");
    assert_eq!(whole.headers()["accept-ranges"], "bytes");
    assert!(whole.headers().get("content-disposition").is_none());
    assert_eq!(whole.bytes().await.unwrap().len(), 8_000);
    let part = media_get(&h.app, &video.file, Some("bytes=0-15")).await;
    assert_eq!(part.status(), 206);
    assert_eq!(part.headers()["content-range"], "bytes 0-15/8000");
    let bytes = part.bytes().await.unwrap();
    assert_eq!(&bytes[4..8], b"ftyp");
    assert_eq!(media_get(&h.app, &video.file, Some("bytes=9000-")).await.status(), 416);
    assert_eq!(media_get(&h.app, &poster, None).await.status(), 200);

    // Accounting: video, poster, thumbnail and avatar, each an `embed` object.
    let objects = h.objects().await;
    assert_eq!(objects.len(), 4, "{objects:?}");
    assert!(objects.iter().all(|(_, kind, _)| kind == "embed"));
    assert!(objects.iter().any(|(k, _, b)| *k == video.file && *b == 8_000));
    h.app.cleanup().await;
}

#[tokio::test]
async fn a_video_over_the_cap_keeps_its_poster_and_the_link_but_no_file() {
    let h = harness().await;
    let embed = h.one("https://x.com/NASA/status/107").await;
    assert_eq!(embed.description, "too big");
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_RICH, "no video file, so not a video embed");
    assert_eq!(embed.media.len(), 1);
    assert_eq!(embed.media[0].kind.as_known(), Some(EmbedMediaKind::EMBED_MEDIA_KIND_IMAGE), "the poster");
    assert!(embed.media[0].file.ends_with(".webp") && embed.media[0].poster.is_none());
    assert!(h.objects().await.iter().all(|(k, _, _)| !k.ends_with(".mp4")));
    h.app.cleanup().await;
}

#[tokio::test]
async fn a_larger_cap_stores_what_the_default_would_not() {
    let h = harness_with(|_, mut e| {
        e.max_video_bytes = 64 * 1024;
        e
    })
    .await;
    let embed = h.one("https://x.com/NASA/status/107").await;
    assert_eq!(embed.media[0].kind.as_known(), Some(EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO));
    assert_eq!(embed.media[0].size, 30_000);
    h.app.cleanup().await;
}

#[tokio::test]
async fn x_gifs_are_stored_as_silent_looping_clips() {
    let h = harness().await;
    let embed = h.one("https://x.com/NASA/status/102").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_GIFV]);
    assert!(embed.media[0].file.ends_with(".mp4"));
    h.app.cleanup().await;
}

#[tokio::test]
async fn at_most_four_media_are_stored() {
    let h = harness().await;
    let embed = h.one("https://x.com/NASA/status/103").await;
    assert_eq!(embed.media.len(), 4);
    assert_eq!(h.site.hits("/img/5.png"), 0);
    h.app.cleanup().await;
}

#[tokio::test]
async fn x_fails_over_from_fxtwitter_to_vxtwitter_to_the_html_mirror() {
    let h = harness().await;
    // 104: the first API is down, the second answers.
    let embed = h.one("https://x.com/NASA/status/104").await;
    assert_eq!(embed.description, "from the second API");
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_IMAGE, EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO]);
    assert_eq!(embed.media[1].duration_ms, Some(4200));
    assert!(h.site.hits("/fx/status/104") >= 1 && h.site.hits("/vx/NASA/status/104") == 1);
    // 105: both APIs are down, the OpenGraph page answers.
    let embed = h.one("https://x.com/NASA/status/105").await;
    assert_eq!(embed.description, "page only");
    assert_eq!(embed.author_name, "NASA (@nasa)");
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_IMAGE]);
    // 106: nothing anywhere.
    assert!(h.embeds("https://x.com/NASA/status/106").await.is_empty());
    h.app.cleanup().await;
}

#[tokio::test]
async fn media_that_is_unsafe_or_not_what_it_claims_is_dropped_but_the_text_stays() {
    let h = harness().await;
    // A page served as video/mp4.
    let embed = h.one("https://x.com/NASA/status/108").await;
    assert_eq!((embed.description.as_str(), embed.media.len()), ("fake video", 0));
    assert!(h.objects().await.iter().all(|(k, _, _)| !k.ends_with(".mp4")), "no object is left behind");
    // Private addresses are never fetched.
    let embed = h.one("https://x.com/NASA/status/109").await;
    assert_eq!((embed.description.as_str(), embed.media.len()), ("private media", 0));
    // application/octet-stream is fine when the signature says MP4.
    let embed = h.one("https://x.com/NASA/status/110").await;
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO]);
    h.app.cleanup().await;
}

#[tokio::test]
async fn tiktok_fails_over_past_broken_and_notice_mirrors_and_downloads_the_redirected_mp4() {
    let h = harness().await;
    let url = "https://www.tiktok.com/@scout/video/1";
    let embed = h.one(url).await;
    assert_eq!((embed.provider.as_str(), embed.url.as_str()), ("tiktok", url));
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
    assert_eq!(embed.author_name, "Scout & Co (@scout)");
    assert_eq!(embed.author_url.as_deref(), Some("https://www.tiktok.com/@scout"));
    assert_eq!(embed.description, "Scramble up ur name");
    assert_eq!(embed.site_name, "TikTok");
    assert_eq!(embed.media.len(), 1);
    let video = &embed.media[0];
    assert_eq!((video.size, video.width, video.height), (8_000, 576, 1024));
    assert!(video.poster.is_none(), "this mirror has no poster");
    assert_eq!(h.site.hits("/ttbroken/"), 1);
    assert_eq!(h.site.hits("/ttnotice/"), 1);
    assert_eq!(h.site.hits("/gen/tiktok.mp4"), 1, "the og:video redirect was followed");
    // A mirror that has a poster gives one.
    let embed = h.one("https://www.tiktok.com/@scout/video/2").await;
    assert!(embed.media[0].poster.is_some() && embed.thumbnail.is_some());
    h.app.cleanup().await;
}

#[tokio::test]
async fn tiktok_short_links_are_resolved_through_safe_redirects() {
    use thehideout_server::embeds::fetch::FetchError;
    use thehideout_server::embeds::providers::{self, Target};
    use thehideout_server::embeds::provider::Ctx;
    let site = start_site().await;
    let settings = settings_for(&site);
    let (fetcher, mirror_fetcher) = (settings.fetcher(), settings.mirror_fetcher());
    let ctx = Ctx {
        fetcher: &fetcher,
        mirror_fetcher: &mirror_fetcher,
        mirrors: &settings.mirrors,
        html_cap: settings.html_cap,
        image_cap: settings.image_cap,
        provider_timeout: Duration::from_secs(8),
        attempt_timeout: Duration::from_secs(4),
        max_video_bytes: VIDEO_CAP,
        max_media: 4,
    };
    let short = |path: &str| Target::TikTok { user: None, id: None, original: url::Url::parse(&site.url(path)).unwrap() };
    // vm → two redirects → /@scout/video/1 → the mirror is asked for exactly that.
    let found = providers::unfurl(&ctx, short("/vm/abc")).await.unwrap().expect("resolved");
    assert_eq!(found.author_url.as_deref(), Some("https://www.tiktok.com/@scout"));
    assert_eq!(found.media.len(), 1);
    assert_eq!(site.hits("/ttgood/@scout/video/1"), 1);
    // A redirect loop, a private target and a landing page of the wrong shape give nothing.
    assert!(matches!(providers::unfurl(&ctx, short("/vm/loop")).await, Err(FetchError::TooManyRedirects)));
    assert!(matches!(providers::unfurl(&ctx, short("/vm/private")).await, Err(FetchError::Blocked(_))));
    assert!(providers::unfurl(&ctx, short("/vm/elsewhere")).await.unwrap().is_none());
}

#[tokio::test]
async fn instagram_fails_over_past_dead_and_login_pages_and_resolves_relative_video_urls() {
    let h = harness().await;
    let embed = h.one("https://www.instagram.com/reel/DKS4Zklsbzb/?igsh=abc").await;
    assert_eq!(embed.provider, "instagram");
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
    assert_eq!(embed.author_name, "@nasa");
    assert_eq!(embed.author_url.as_deref(), Some("https://www.instagram.com/nasa/"));
    assert_eq!(embed.media.len(), 1);
    assert_eq!((embed.media[0].size, embed.media[0].kind.as_known()), (8_000, Some(EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO)));
    assert!(embed.media[0].poster.is_some());
    assert_eq!((h.site.hits("/igdead/"), h.site.hits("/iglogin/")), (1, 1));
    assert_eq!(h.site.hits("/ightml/reel/DKS4Zklsbzb/"), 1);
    assert_eq!(h.site.hits("/videos/DKS4Zklsbzb/1"), 1, "relative og:video resolved against the mirror");
    h.app.cleanup().await;
}

#[tokio::test]
async fn instagram_mirrors_that_redirect_straight_to_the_video_work() {
    let site_only = harness_with(|site, mut e| {
        e.mirrors = e.mirrors.with("instagram", &[&site.url("/igredir")]);
        e
    })
    .await;
    let embed = site_only.one("https://www.instagram.com/p/C-6mYVRS5hD/").await;
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO]);
    assert_eq!(embed.media[0].size, 8_000);
    site_only.app.cleanup().await;
}

#[tokio::test]
async fn reddit_prefers_the_mirrors_muxed_video() {
    let h = harness().await;
    let url = "https://old.reddit.com/r/pics/comments/vid123/a_video_post/";
    let embed = h.one(url).await;
    assert_eq!(embed.provider, "reddit");
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
    assert_eq!((embed.title.as_str(), embed.site_name.as_str(), embed.author_name.as_str()), ("A video post", "r/pics", "u/alice"));
    assert_eq!(embed.author_url.as_deref(), Some("https://www.reddit.com/user/alice/"));
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_VIDEO], "muxed video with sound, not the silent clip");
    assert_eq!(h.site.hits("/rj/"), 0, "the JSON is not needed when the mirror has the video");
    h.app.cleanup().await;
}

#[tokio::test]
async fn reddit_galleries_come_from_the_json_when_the_mirror_only_has_one_image() {
    let h = harness().await;
    let embed = h.one("https://www.reddit.com/r/pics/comments/gal001/a_gallery/").await;
    assert_eq!(embed.title, "A gallery");
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_RICH);
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_IMAGE; 2], "the third item has no valid media");
    assert_eq!(embed.published_at.as_option().map(|t| t.seconds), Some(1666809958));
    h.app.cleanup().await;
}

#[tokio::test]
async fn reddit_json_video_without_a_muxed_mirror_is_stored_as_a_silent_clip() {
    let h = harness().await;
    let embed = h.one("https://redd.it/sil001").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
    assert_eq!(kinds(&embed), [EmbedMediaKind::EMBED_MEDIA_KIND_GIFV]);
    assert_eq!(embed.media[0].duration_ms, Some(11_000));
    assert!(embed.media[0].poster.is_some());
    // Text-only posts embed too.
    let embed = h.one("https://www.reddit.com/comments/txt001/").await;
    assert_eq!((embed.title.as_str(), embed.description.as_str()), ("A text post", "Line one\n\nLine two"));
    assert!(embed.media.is_empty());
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_RICH);
    h.app.cleanup().await;
}

#[tokio::test]
async fn cached_provider_results_are_reused_but_every_embed_owns_its_files() {
    let h = harness().await;
    let first = h.one("https://x.com/NASA/status/101").await;
    let api_hits = h.site.hits("/fx/status/101");
    let second = h.one("https://x.com/NASA/status/101").await;
    assert_eq!(h.site.hits("/fx/status/101"), api_hits, "the API is not asked again");
    assert_eq!(second.media[0].size, 8_000);
    assert_ne!(first.media[0].file, second.media[0].file);
    assert_ne!(first.media[0].poster, second.media[0].poster);
    h.app.cleanup().await;
}

#[tokio::test]
async fn generic_links_keep_the_legacy_thumbnail_and_no_provider_fields() {
    let h = harness().await;
    // A link on the test site itself is a plain page: `/avatar.png` is a direct image link.
    let embed = h.one(&h.site.url("/poster.png")).await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_IMAGE);
    assert_eq!((embed.provider.as_str(), embed.author_name.as_str()), ("", ""));
    assert!(embed.media.is_empty() && embed.author_url.is_none() && embed.author_icon.is_none());
    let thumb = embed.thumbnail.clone().expect("thumbnail");
    assert_eq!((embed.thumbnail_width, embed.thumbnail_height), (Some(320), Some(180)));
    assert_eq!(media_get(&h.app, &thumb, None).await.status(), 200);
    let objects = h.objects().await;
    assert_eq!(objects.len(), 1);
    h.app.cleanup().await;
}

#[tokio::test]
async fn embed_storage_counts_against_the_guild() {
    let h = harness().await;
    h.one("https://x.com/NASA/status/101").await;
    let total: i64 = h.objects().await.iter().map(|(_, _, b)| b).sum();
    let counted = thehideout_server::media::totals_by_kind(&h.app.state, Some(h.guild_id)).await.unwrap();
    let embeds = counted.into_iter().find(|t| t.kind == "embed").expect("embed total");
    assert_eq!(embeds.bytes, total);
    h.app.cleanup().await;
}

// ------------------------------------------------------- live (not default)

/// The same flow against the real mirrors; run by hand:
/// `cargo test --test embed_providers -- --ignored --nocapture`.
async fn live(url: &str) -> Embed {
    let mut e = EmbedSettings::default();
    e.provider_timeout = Duration::from_secs(25);
    e.media_timeout = Duration::from_secs(60);
    let app = TestApp::start_with_embeds(e).await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let channel = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let h = Harness { app, alice, channel, guild_id: guild.id, site: Arc::new(Site::default()) };
    let embed = h.one(url).await;
    eprintln!("{embed:#?}");
    h.app.cleanup().await;
    embed
}

#[tokio::test]
#[ignore = "needs the network"]
async fn live_x_video() {
    let embed = live("https://x.com/elonmusk/status/1585341984679469056").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
}

#[tokio::test]
#[ignore = "needs the network"]
async fn live_x_photos() {
    let embed = live("https://x.com/NASA/status/2041557036274475228").await;
    assert_eq!(embed.media.len(), 4);
}

#[tokio::test]
#[ignore = "needs the network"]
async fn live_tiktok() {
    let embed = live("https://www.tiktok.com/@scout2015/video/6718335390845095173").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
}

#[tokio::test]
#[ignore = "needs the network"]
async fn live_instagram_reel() {
    let embed = live("https://www.instagram.com/reel/DKS4Zklsbzb/").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
}

#[tokio::test]
#[ignore = "needs the network"]
async fn live_reddit_video() {
    let embed = live("https://www.reddit.com/r/Unexpected/comments/1xhqbt0/either_way_counts/").await;
    assert_eq!(kind_of(&embed), EmbedKind::EMBED_KIND_VIDEO);
}
