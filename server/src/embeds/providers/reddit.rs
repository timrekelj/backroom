//! Reddit posts through a `vxreddit.com`-style mirror and Reddit's own public
//! JSON (`{base}/comments/{id}.json`).
//!
//! Reddit hosts video as DASH: the video and the audio are separate streams
//! and the post JSON only links the video-only `fallback_url`. The mirror
//! muxes them into one MP4 (`og:video`), so it is asked first and its video
//! wins. If only the JSON is available the video-only file is stored and
//! marked as a silent looping clip (`GIFV`); documented as a limitation.
//!
//! Images and galleries: the mirror exposes a single `og:image`; when a mirror
//! gave no video the JSON is consulted too, because it lists every gallery
//! item (`media_metadata`) and the post text. The JSON endpoint refuses
//! requests from some networks (HTTP 403, or a login redirect for generic
//! user agents), so it is a fallback, never the only plan.
//!
//! Share links (`/r/{sub}/s/{code}`) are resolved by following their redirect.

use serde_json::Value;
use url::Url;

use super::{Page, absolute, clean_text, detect_landing, fetch_json, fetch_page, join, json_str, json_u32, seconds_to_ms, TEXT_MAX, Target};
use crate::embeds::fetch::FetchError;
use crate::embeds::provider::{AssetKind, Ctx, MediaSource, Provider, Unfurled};

pub async fn unfurl(ctx: &Ctx<'_>, path: String, id: Option<String>, original: Url) -> Result<Option<Unfurled>, FetchError> {
    let (path, id) = match id {
        Some(id) => (path, id),
        None => match resolve(ctx, &original).await? {
            Some(found) => found,
            None => return Ok(None),
        },
    };
    let mut last_error = None;
    let mut from_mirror = None;
    for base in ctx.mirrors.get("reddit") {
        let Some(url) = join(base, &path) else { continue };
        match fetch_page(ctx, &url).await {
            Ok(Page::Html(meta, page)) => {
                if let Some(found) = from_page(ctx, &meta, &page) {
                    from_mirror = Some(found);
                    break;
                }
            }
            Ok(Page::Video(_)) => {}
            Err(e) => {
                tracing::debug!(%url, error = %e, "reddit mirror failed");
                last_error = Some(e);
            }
        }
    }
    // A muxed video from the mirror is the best there is.
    if let Some(found) = &from_mirror
        && found.media.iter().any(|m| m.kind == AssetKind::Video)
    {
        return Ok(from_mirror);
    }
    for base in ctx.mirrors.get("reddit_json") {
        let Some(url) = join(base, &format!("/comments/{id}.json?limit=1&raw_json=1")) else { continue };
        match fetch_json(ctx, &url).await {
            Ok(json) => {
                if let Some(found) = from_json(ctx, &json) {
                    return Ok(Some(found));
                }
            }
            Err(e) => {
                tracing::debug!(%url, error = %e, "reddit JSON failed");
                last_error = Some(e);
            }
        }
    }
    match from_mirror {
        Some(found) => Ok(Some(found)),
        None => last_error.map_or(Ok(None), Err),
    }
}

async fn resolve(ctx: &Ctx<'_>, share: &Url) -> Result<Option<(String, String)>, FetchError> {
    let fetcher = ctx.mirror_fetcher.with_total_timeout(ctx.attempt_timeout);
    let landed = fetcher
        .within(async {
            let reply = fetcher.get(share).await?;
            Ok(reply.url.clone())
        })
        .await?;
    Ok(match detect_landing(&landed, Provider::Reddit) {
        Some(Target::Reddit { path, id: Some(id), .. }) => Some((path, id)),
        _ => None,
    })
}

/// `og:site_name` is `u/{author} on r/{sub} - ⬆️ {score} | 💬 {comments}`.
fn split_site_name(site_name: &str) -> (Option<String>, Option<String>) {
    let head = site_name.split(" - ").next().unwrap_or(site_name);
    match head.split_once(" on ") {
        Some((author, sub)) => (Some(author.trim().to_owned()), Some(sub.trim().to_owned())),
        None => (None, None),
    }
}

fn user_url(author: &str) -> Option<String> {
    let name = author.strip_prefix("u/")?;
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')).then(|| format!("https://www.reddit.com/user/{name}/"))
}

fn from_page(ctx: &Ctx<'_>, meta: &crate::embeds::html::Metadata, page: &Url) -> Option<Unfurled> {
    let poster = meta.image.as_deref().and_then(|i| absolute(page, i)).map(String::from);
    let mut media = Vec::new();
    if let Some(video) = meta.video.as_deref().and_then(|v| absolute(page, v)) {
        let mut source = MediaSource::new(AssetKind::Video, video).size(meta.video_width, meta.video_height);
        source.poster = poster;
        media.push(source);
    } else if let Some(image) = poster {
        media.push(MediaSource::new(AssetKind::Image, image));
    }
    media.truncate(ctx.max_media);
    let (author, sub) = split_site_name(&meta.site_name);
    let unfurled = Unfurled {
        title: meta.title.clone(),
        description: clean_text(&meta.description, TEXT_MAX),
        site_name: sub.unwrap_or_else(|| "Reddit".to_owned()),
        author_url: author.as_deref().and_then(user_url),
        author: author.unwrap_or_default(),
        media,
        ..Default::default()
    };
    // A mirror that errored out in HTML has no title and no media.
    (!unfurled.title.is_empty() || !unfurled.media.is_empty()).then_some(unfurled)
}

fn from_json(ctx: &Ctx<'_>, json: &Value) -> Option<Unfurled> {
    let post = json.pointer("/0/data/children/0/data").filter(|p| p.is_object())?;
    let title = json_str(post, "title")?;
    let mut media: Vec<MediaSource> = Vec::new();

    if let Some(video) = post.pointer("/media/reddit_video").or_else(|| post.pointer("/secure_media/reddit_video")) {
        if let Some(url) = json_str(video, "fallback_url") {
            // The fallback file has no audio track (DASH keeps it apart), GIF or not.
            let mut source = MediaSource::new(AssetKind::Gifv, url).size(json_u32(video, "width"), json_u32(video, "height"));
            source.duration_ms = video.get("duration").and_then(Value::as_f64).and_then(seconds_to_ms);
            source.poster = preview_image(post).map(|(u, _, _)| u);
            media.push(source);
        }
    } else if post.get("is_gallery").and_then(Value::as_bool).unwrap_or(false) {
        let metadata = post.get("media_metadata");
        for item in post.pointer("/gallery_data/items").and_then(Value::as_array).into_iter().flatten() {
            if media.len() >= ctx.max_media {
                break;
            }
            let Some(entry) = json_str(item, "media_id").and_then(|id| metadata?.get(id)) else { continue };
            if json_str(entry, "status").is_some_and(|s| s != "valid") {
                continue;
            }
            let Some(source) = entry.get("s") else { continue };
            if let Some(url) = json_str(source, "u").or_else(|| json_str(source, "gif")) {
                media.push(MediaSource::new(AssetKind::Image, url).size(json_u32(source, "x"), json_u32(source, "y")));
            }
        }
    } else if let Some((url, w, h)) = preview_image(post) {
        // A link post to an image, or a link with a preview picture.
        media.push(MediaSource::new(AssetKind::Image, url).size(w, h));
    }
    media.truncate(ctx.max_media);

    let author = json_str(post, "author").filter(|a| *a != "[deleted]").map(|a| format!("u/{a}"));
    let unfurled = Unfurled {
        title: clean_text(title, 300),
        description: json_str(post, "selftext").map(|t| clean_text(t, TEXT_MAX)).unwrap_or_default(),
        site_name: json_str(post, "subreddit_name_prefixed").unwrap_or("Reddit").to_owned(),
        author_url: author.as_deref().and_then(user_url),
        author: author.unwrap_or_default(),
        published_at: post.get("created_utc").and_then(Value::as_f64).map(|t| t as i64),
        media,
        ..Default::default()
    };
    Some(unfurled)
}

/// The best still of a post: `(url, width, height)`.
fn preview_image(post: &Value) -> Option<(String, Option<u32>, Option<u32>)> {
    let source = post.pointer("/preview/images/0/source")?;
    Some((json_str(source, "url")?.to_owned(), json_u32(source, "width"), json_u32(source, "height")))
}
