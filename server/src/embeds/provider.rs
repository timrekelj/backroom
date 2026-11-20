//! Providers: how a URL turns into an [`Unfurled`] description.
//!
//! A provider is chosen from the URL ([`Provider::for_url`]) and only
//! *describes* the link: text plus the sources of media to download. It never
//! writes anything; the pipeline in `mod.rs` downloads the media (through the
//! same SSRF-safe [`Fetcher`]), stores it and accounts for it.
//!
//! Today there is one provider, [`Provider::Generic`] (OpenGraph / Twitter
//! card / `<title>` scraping, plus direct image links). To add a site-specific
//! provider (e.g. a JSON API or an embed-friendly mirror of a social network):
//! 1. add a variant to [`Provider`] and a match arm in [`Provider::for_url`]
//!    (by host) and [`Provider::unfurl`];
//! 2. fetch whatever it needs with `ctx.fetcher` (every hop is vetted; use
//!    [`Fetcher::with_total_timeout`] for slow or large requests) and return
//!    its text and [`MediaSource`]s (images and videos, in display order);
//! 3. let the pipeline's media step handle the new kinds (it currently
//!    downloads only the first image as the thumbnail; downloading to S3
//!    without buffering is [`super::fetch::Reply::download_to_storage`]).
//!
//! The result is cached per URL (`embed_cache`), so providers should be
//! deterministic for a URL and return source URLs, not stored keys.

use serde::{Deserialize, Serialize};
use url::Url;

use super::fetch::{FetchError, Fetcher};
use super::html;

/// What a piece of media is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Image,
    Video,
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
}

/// What a provider learned about a URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unfurled {
    pub title: String,
    pub description: String,
    pub site_name: String,
    /// Author or account name where the site has one (not part of `Embed` yet).
    pub author: String,
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
}

/// What providers may use.
pub struct Ctx<'a> {
    pub fetcher: &'a Fetcher,
    pub html_cap: usize,
    pub image_cap: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// OpenGraph / Twitter cards / `<title>`, and direct image links.
    Generic,
}

impl Provider {
    /// Picks the provider for `url` (by host); the generic one is the default.
    pub fn for_url(_url: &Url) -> Self {
        Self::Generic
    }

    /// Describes `url`. `Ok(None)`: nothing to show.
    pub async fn unfurl(self, ctx: &Ctx<'_>, url: &Url) -> Result<Option<Unfurled>, FetchError> {
        match self {
            Self::Generic => generic(ctx, url).await,
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
                        .map(|u| MediaSource { kind: AssetKind::Image, url: u.into(), width: None, height: None });
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
                        media: vec![MediaSource { kind: AssetKind::Image, url: url.to_string(), width: None, height: None }],
                        prefetched: Some(body),
                        ..Default::default()
                    }))
                }
                other => Err(FetchError::NotAllowedType(other.to_owned())),
            }
        })
        .await
}
