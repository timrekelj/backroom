//! TikTok through fxTikTok-style mirrors.
//!
//! Short links (`vm.tiktok.com/…`, `vt.tiktok.com/…`, `tiktok.com/t/…`) are
//! resolved first by following their redirects (every hop vetted) to the real
//! `/@user/video/{id}` URL; the mirror is then asked for that. A mirror page
//! carries `og:title` (`Name (@user)`), `og:description` (the caption),
//! `og:video` (an MP4; on `tnktok.com` a redirect to TikTok's CDN, followed by
//! the download) and, on some mirrors, `og:image` (the poster). Mirrors that
//! only answer with a notice or an error page are skipped.

use url::Url;

use super::{Page, absolute, clean_text, detect_landing, fetch_page, join, TEXT_MAX, Target};
use crate::embeds::fetch::FetchError;
use crate::embeds::provider::{AssetKind, Ctx, MediaSource, Provider, Unfurled};

pub async fn unfurl(ctx: &Ctx<'_>, user: Option<String>, id: Option<String>, original: Url) -> Result<Option<Unfurled>, FetchError> {
    let (user, id) = match id {
        Some(id) => (user, id),
        None => match resolve(ctx, &original).await? {
            Some(found) => found,
            None => return Ok(None),
        },
    };
    let user_path = user.as_deref().unwrap_or("_");
    let mut last_error = None;
    for base in ctx.mirrors.get("tiktok") {
        let Some(url) = join(base, &format!("/@{user_path}/video/{id}")) else { continue };
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
                let unfurled = Unfurled {
                    description: clean_text(&meta.description, TEXT_MAX),
                    site_name: "TikTok".to_owned(),
                    author: meta.title.clone(),
                    author_url: user.as_deref().map(|u| format!("https://www.tiktok.com/@{u}")),
                    media,
                    ..Default::default()
                };
                // A mirror that is only a notice ("no longer available") has a caption but no media.
                if !unfurled.media.is_empty() {
                    return Ok(Some(unfurled));
                }
            }
            Ok(Page::Video(video)) => {
                return Ok(Some(Unfurled {
                    site_name: "TikTok".to_owned(),
                    author_url: user.as_deref().map(|u| format!("https://www.tiktok.com/@{u}")),
                    media: vec![MediaSource::new(AssetKind::Video, video)],
                    ..Default::default()
                }));
            }
            Err(e) => {
                tracing::debug!(%url, error = %e, "tiktok mirror failed");
                last_error = Some(e);
            }
        }
    }
    last_error.map_or(Ok(None), Err)
}

/// Follows a short link to the video it names: `(user, video id)`.
async fn resolve(ctx: &Ctx<'_>, short: &Url) -> Result<Option<(Option<String>, String)>, FetchError> {
    let fetcher = ctx.mirror_fetcher.with_total_timeout(ctx.attempt_timeout);
    let landed = fetcher
        .within(async {
            // Only the final URL matters; the body is dropped unread.
            let reply = fetcher.get(short).await?;
            Ok(reply.url.clone())
        })
        .await?;
    Ok(match detect_landing(&landed, Provider::TikTok) {
        Some(Target::TikTok { user, id: Some(id), .. }) => Some((user, id)),
        _ => None,
    })
}
