//! Providers: how a URL turns into an [`Unfurled`] description.
//!
//! A provider is chosen from the URL ([`Provider::for_url`]) and only
//! *describes* the link: text plus the sources of media to download. It never
//! writes anything; the pipeline in `mod.rs` downloads the media (through the
//! same SSRF-safe [`Fetcher`]), stores it and accounts for it.
//!
//! [`Provider::Generic`] is OpenGraph / Twitter card / `<title>` scraping plus
//! direct image links. The site-specific providers (X, TikTok, Instagram,
//! Reddit; see [`super::providers`]) fetch through embed-friendly mirrors and
//! public APIs, never from the site itself, and describe the post with text
//! plus [`MediaSource`]s (images, videos, GIFs, in display order). To add one:
//! 1. add a variant to [`Provider`], detect its URLs in
//!    [`super::providers::detect`] and dispatch in [`Provider::unfurl`];
//! 2. fetch with `ctx.mirror_fetcher` (every hop is vetted) and return the
//!    text and sources; the pipeline downloads them (`super::media`).
//!
//! The result is cached per URL (`embed_cache`), so providers should be
//! deterministic for a URL and return source URLs, not stored keys.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use super::fetch::{FetchError, Fetcher};
use super::html;
use super::providers::{self, Mirrors};

/// What a piece of media is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Image,
    Video,
    /// A silent looping clip stored as MP4 (a GIF, or a video without sound).
    Gifv,
}

/// Media a provider found, still to be downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSource {
    pub kind: AssetKind,
    /// Absolute http(s) URL to download from.
    pub url: String,
    /// Dimensions if the provider knows them (hints only; the stored file is measured).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A still of a video (image URL).
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u32>,
    /// Other URLs of the same video (e.g. lower bitrates), tried in order when
    /// `url` fails or is over the size cap.
    #[serde(default)]
    pub alternates: Vec<String>,
}

impl MediaSource {
    pub fn new(kind: AssetKind, url: impl Into<String>) -> Self {
        Self { kind, url: url.into(), width: None, height: None, poster: None, duration_ms: None, alternates: Vec::new() }
    }

    pub fn size(mut self, width: Option<u32>, height: Option<u32>) -> Self {
        self.width = width.filter(|v| *v > 0);
        self.height = height.filter(|v| *v > 0);
        self
    }
}

/// What a provider learned about a URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unfurled {
    pub title: String,
    pub description: String,
    pub site_name: String,
    /// Author or account name where the site has one.
    pub author: String,
    /// Profile URL of the author (a link only; never fetched).
    pub author_url: Option<String>,
    /// Image URL of a small avatar, downloaded as the author icon.
    pub author_icon: Option<String>,
    /// Unix seconds the post was published.
    pub published_at: Option<i64>,
    /// Media in display order; the first image is the thumbnail today.
    pub media: Vec<MediaSource>,
    /// Body of `media[0]` if the provider already downloaded it (a direct
    /// image link is fetched once, not twice). Never cached.
    pub prefetched: Option<Vec<u8>>,
}

impl Unfurled {
    /// Whether there is anything to show without downloading media.
    pub fn has_text(&self) -> bool {
        !self.title.is_empty() || !self.description.is_empty()
    }

    /// Whether there is anything at all to show.
    pub fn has_content(&self) -> bool {
        self.has_text() || !self.media.is_empty() || !self.author.is_empty()
    }
}

/// What providers may use.
pub struct Ctx<'a> {
    pub fetcher: &'a Fetcher,
    /// The fetcher for mirrors and APIs: same safety rules, a browser-style
    /// user agent (some mirrors only answer those).
    pub mirror_fetcher: &'a Fetcher,
    pub mirrors: &'a Mirrors,
    pub html_cap: usize,
    pub image_cap: usize,
    /// Deadline of one provider run (all mirrors and redirects).
    pub provider_timeout: Duration,
    /// Deadline of one request to one mirror.
    pub attempt_timeout: Duration,
    /// Largest video; used to pick a variant that fits.
    pub max_video_bytes: usize,
    pub max_media: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// OpenGraph / Twitter cards / `<title>`, and direct image links.
    Generic,
    X,
    TikTok,
    Instagram,
    Reddit,
}

impl Provider {
    /// Picks the provider for `url` (by host and path); the generic one is the default.
    pub fn for_url(url: &Url) -> Self {
        providers::detect(url).map_or(Self::Generic, |t| t.provider())
    }

    /// `Embed.provider`: "" for a generic link.
    pub fn name(self) -> &'static str {
        match self {
            Self::Generic => "",
            Self::X => "x",
            Self::TikTok => "tiktok",
            Self::Instagram => "instagram",
            Self::Reddit => "reddit",
        }
    }

    /// Describes `url`. `Ok(None)`: nothing to show.
    pub async fn unfurl(self, ctx: &Ctx<'_>, url: &Url) -> Result<Option<Unfurled>, FetchError> {
        match self {
            Self::Generic => generic(ctx, url).await,
            _ => {
                let Some(target) = providers::detect(url) else { return Ok(None) };
                tokio::time::timeout(ctx.provider_timeout, providers::unfurl(ctx, target))
                    .await
                    .map_err(|_| FetchError::Timeout)?
            }
        }
    }
}

async fn generic(ctx: &Ctx<'_>, url: &Url) -> Result<Option<Unfurled>, FetchError> {
    ctx.fetcher
        .within(async {
            let reply = ctx.fetcher.get(url).await?;
            let final_url = reply.url.clone();
            match reply.content_type.as_str() {
                "text/html" | "application/xhtml+xml" => {
                    let body = reply.read_html_prefix(ctx.html_cap).await?;
                    let meta = html::parse(&body);
                    let image = meta
                        .image
                        .as_deref()
                        .and_then(|i| final_url.join(i).ok())
                        .filter(|u| matches!(u.scheme(), "http" | "https"))
                        .map(|u| MediaSource::new(AssetKind::Image, u));
                    let site_name = if meta.site_name.is_empty() {
                        final_url.host_str().map(|h| html::clean(h, html::SITE_NAME_MAX)).unwrap_or_default()
                    } else {
                        meta.site_name
                    };
                    let unfurled = Unfurled {
                        title: meta.title,
                        description: meta.description,
                        site_name,
                        media: image.into_iter().collect(),
                        ..Default::default()
                    };
                    Ok((unfurled.has_text() || !unfurled.media.is_empty()).then_some(unfurled))
                }
                t if t.starts_with("image/") => {
                    let body = reply.read_capped(ctx.image_cap).await?;
                    Ok(Some(Unfurled {
                        media: vec![MediaSource::new(AssetKind::Image, url.to_string())],
                        prefetched: Some(body),
                        ..Default::default()
                    }))
                }
                other => Err(FetchError::NotAllowedType(other.to_owned())),
            }
        })
        .await
}
