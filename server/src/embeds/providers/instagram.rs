//! Instagram posts and reels through InstaFix-style mirrors.
//!
//! Mirrors are tried in order and are often flaky (they are blocked or
//! rate-limited by Instagram from time to time), so failover is the norm. A
//! mirror answers `{base}/{p|reel}/{code}/` with OpenGraph tags (`og:video`,
//! possibly relative like `/videos/{code}/1`; `og:image`; `og:description`;
//! the account in `og:title` / `twitter:title`), or redirects straight to the
//! video file (`kkinstagram.com` does that for reels). Pages without any
//! media are skipped as failures.
//!
//! Mirrors whose redirects lead off to ad networks are not in the default
//! list. Only public posts work; a mirror cannot see private ones.

use super::{Page, absolute, clean_text, fetch_page, join, TEXT_MAX};
use crate::embeds::fetch::FetchError;
use crate::embeds::provider::{AssetKind, Ctx, MediaSource, Unfurled};

pub async fn unfurl(ctx: &Ctx<'_>, kind: &str, code: &str) -> Result<Option<Unfurled>, FetchError> {
    let mut last_error = None;
    for base in ctx.mirrors.get("instagram") {
        let Some(url) = join(base, &format!("/{kind}/{code}/")) else { continue };
        match fetch_page(ctx, &url).await {
            Ok(Page::Html(meta, page)) => {
                let poster = meta.image.as_deref().and_then(|i| absolute(&page, i)).map(String::from);
                let mut media = Vec::new();
                if let Some(video) = meta.video.as_deref().and_then(|v| absolute(&page, v)) {
                    let mut source = MediaSource::new(AssetKind::Video, video).size(meta.video_width, meta.video_height);
                    source.poster = poster;
                    media.push(source);
                } else if let Some(image) = poster {
                    media.push(MediaSource::new(AssetKind::Image, image));
                }
                let handle = meta.title.trim().trim_start_matches('@').to_owned();
                let unfurled = Unfurled {
                    description: clean_text(&meta.description, TEXT_MAX),
                    site_name: "Instagram".to_owned(),
                    author_url: is_handle(&handle).then(|| format!("https://www.instagram.com/{handle}/")),
                    author: meta.title.clone(),
                    media,
                    ..Default::default()
                };
                // A login wall or notice page has text but no media: try the next mirror.
                if !unfurled.media.is_empty() {
                    return Ok(Some(unfurled));
                }
            }
            Ok(Page::Video(video)) => {
                return Ok(Some(Unfurled {
                    site_name: "Instagram".to_owned(),
                    media: vec![MediaSource::new(AssetKind::Video, video)],
                    ..Default::default()
                }));
            }
            Err(e) => {
                tracing::debug!(%url, error = %e, "instagram mirror failed");
                last_error = Some(e);
            }
        }
    }
    last_error.map_or(Ok(None), Err)
}

fn is_handle(s: &str) -> bool {
    !s.is_empty() && s.len() <= 30 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}
