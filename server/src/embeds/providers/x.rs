//! X / Twitter through FxTwitter's JSON API (`api.fxtwitter.com/status/{id}`),
//! falling back to VxTwitter's API and then to the OpenGraph page of an
//! `fxtwitter.com`-style mirror.
//!
//! FxTwitter's response (checked against a live post): `tweet.text`,
//! `tweet.author {name, screen_name, url, avatar_url}`, `tweet.created_timestamp`
//! and `tweet.media.all[]`, each `{type: "photo"|"video"|"gif", url, width,
//! height, thumbnail_url, duration (seconds), variants[{url, bitrate,
//! content_type}]}`. Photos come as `…?name=orig` (huge); the provider asks
//! for `name=large` since stored images are fit to 1600 px anyway. GIFs are
//! MP4 files on X (`gif`) and become silent looping clips.

use serde_json::Value;
use url::Url;

use super::{Page, absolute, clean_text, fetch_json, fetch_page, join, json_str, json_u32, order_variants, seconds_to_ms, TEXT_MAX};
use crate::embeds::fetch::FetchError;
use crate::embeds::provider::{AssetKind, Ctx, MediaSource, Unfurled};

pub async fn unfurl(ctx: &Ctx<'_>, id: &str, user: Option<&str>) -> Result<Option<Unfurled>, FetchError> {
    let mut last_error = None;
    for base in ctx.mirrors.get("x_api") {
        let Some(url) = join(base, &format!("/status/{id}")) else { continue };
        match fetch_json(ctx, &url).await {
            Ok(json) => {
                if let Some(found) = from_fx(ctx, &json) {
                    return Ok(Some(found));
                }
            }
            Err(e) => {
                tracing::debug!(%url, error = %e, "fxtwitter API failed");
                last_error = Some(e);
            }
        }
    }
    let user = user.unwrap_or("i");
    for base in ctx.mirrors.get("x_vx_api") {
        let Some(url) = join(base, &format!("/{user}/status/{id}")) else { continue };
        match fetch_json(ctx, &url).await {
            Ok(json) => {
                if let Some(found) = from_vx(ctx, &json) {
                    return Ok(Some(found));
                }
            }
            Err(e) => {
                tracing::debug!(%url, error = %e, "vxtwitter API failed");
                last_error = Some(e);
            }
        }
    }
    for base in ctx.mirrors.get("x_html") {
        let Some(url) = join(base, &format!("/{user}/status/{id}")) else { continue };
        match fetch_page(ctx, &url).await {
            Ok(Page::Html(meta, final_url)) => {
                if let Some(found) = from_page(ctx, &meta, &final_url, user) {
                    return Ok(Some(found));
                }
            }
            Ok(Page::Video(_)) => {}
            Err(e) => {
                tracing::debug!(%url, error = %e, "twitter mirror failed");
                last_error = Some(e);
            }
        }
    }
    last_error.map_or(Ok(None), Err)
}

fn author_name(name: Option<&str>, handle: Option<&str>) -> String {
    match (name, handle) {
        (Some(n), Some(h)) => format!("{n} (@{h})"),
        (Some(n), None) => n.to_owned(),
        (None, Some(h)) => format!("@{h}"),
        (None, None) => String::new(),
    }
}

/// `pbs.twimg.com/media/…?name=orig` → `name=large`.
fn photo_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut u) if u.host_str() == Some("pbs.twimg.com") && u.path().starts_with("/media/") => {
            let others: Vec<(String, String)> = u.query_pairs().filter(|(k, _)| k != "name").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            u.query_pairs_mut().clear().extend_pairs(others).append_pair("name", "large");
            u.into()
        }
        _ => url.to_owned(),
    }
}

fn from_fx(ctx: &Ctx<'_>, json: &Value) -> Option<Unfurled> {
    let tweet = json.get("tweet").filter(|t| t.is_object())?;
    let author = tweet.get("author");
    let handle = author.and_then(|a| json_str(a, "screen_name"));
    let mut media = Vec::new();
    for item in tweet.pointer("/media/all").and_then(Value::as_array).into_iter().flatten() {
        if media.len() >= ctx.max_media {
            break;
        }
        let Some(url) = json_str(item, "url") else { continue };
        let (width, height) = (json_u32(item, "width"), json_u32(item, "height"));
        let duration = item.get("duration").and_then(Value::as_f64);
        let source = match json_str(item, "type") {
            Some("photo") => MediaSource::new(AssetKind::Image, photo_url(url)).size(width, height),
            Some(kind @ ("video" | "gif")) => {
                let variants: Vec<(String, u64)> = item
                    .get("variants")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|v| json_str(v, "content_type") == Some("video/mp4"))
                    .filter_map(|v| Some((json_str(v, "url")?.to_owned(), v.get("bitrate").and_then(Value::as_u64).unwrap_or(0))))
                    .collect();
                let mut urls = order_variants(variants, duration, ctx.max_video_bytes);
                if urls.is_empty() && json_str(item, "format") == Some("video/mp4") {
                    urls.push(url.to_owned());
                }
                if urls.is_empty() {
                    continue;
                }
                let first = urls.remove(0);
                let mut source = MediaSource::new(if kind == "gif" { AssetKind::Gifv } else { AssetKind::Video }, first).size(width, height);
                source.alternates = urls;
                source.poster = json_str(item, "thumbnail_url").map(str::to_owned);
                source.duration_ms = duration.and_then(seconds_to_ms);
                source
            }
            _ => continue,
        };
        media.push(source);
    }
    let text = json_str(tweet, "text").map(|t| clean_text(t, TEXT_MAX)).unwrap_or_default();
    let unfurled = Unfurled {
        description: text,
        site_name: "X".to_owned(),
        author: author_name(author.and_then(|a| json_str(a, "name")), handle),
        author_url: author.and_then(|a| json_str(a, "url")).map(str::to_owned).or_else(|| handle.map(|h| format!("https://x.com/{h}"))),
        author_icon: author.and_then(|a| json_str(a, "avatar_url")).map(str::to_owned),
        published_at: tweet.get("created_timestamp").and_then(Value::as_i64),
        media,
        ..Default::default()
    };
    unfurled.has_content().then_some(unfurled)
}

fn from_vx(ctx: &Ctx<'_>, json: &Value) -> Option<Unfurled> {
    if !json.is_object() || json_str(json, "tweetID").is_none() && json_str(json, "text").is_none() {
        return None;
    }
    let handle = json_str(json, "user_screen_name");
    let mut media = Vec::new();
    for item in json.get("media_extended").and_then(Value::as_array).into_iter().flatten() {
        if media.len() >= ctx.max_media {
            break;
        }
        let Some(url) = json_str(item, "url") else { continue };
        let size = item.get("size");
        let (width, height) = (size.and_then(|s| json_u32(s, "width")), size.and_then(|s| json_u32(s, "height")));
        let source = match json_str(item, "type") {
            Some("image") => MediaSource::new(AssetKind::Image, photo_url(url)).size(width, height),
            Some(kind @ ("video" | "gif")) => {
                let mut source = MediaSource::new(if kind == "gif" { AssetKind::Gifv } else { AssetKind::Video }, url).size(width, height);
                source.poster = json_str(item, "thumbnail_url").map(str::to_owned);
                source.duration_ms = json_u32(item, "duration_millis");
                source
            }
            _ => continue,
        };
        media.push(source);
    }
    let unfurled = Unfurled {
        description: json_str(json, "text").map(|t| clean_text(t, TEXT_MAX)).unwrap_or_default(),
        site_name: "X".to_owned(),
        author: author_name(json_str(json, "user_name"), handle),
        author_url: handle.map(|h| format!("https://x.com/{h}")),
        author_icon: json_str(json, "user_profile_image_url").map(str::to_owned),
        published_at: json.get("date_epoch").and_then(Value::as_i64),
        media,
        ..Default::default()
    };
    unfurled.has_content().then_some(unfurled)
}

/// Last resort: the OpenGraph tags of the mirror's page.
fn from_page(ctx: &Ctx<'_>, meta: &crate::embeds::html::Metadata, page: &Url, user: &str) -> Option<Unfurled> {
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
    let handle = (user != "i").then_some(user);
    let unfurled = Unfurled {
        description: clean_text(&meta.description, TEXT_MAX),
        site_name: "X".to_owned(),
        author: if meta.title.is_empty() { author_name(None, handle) } else { meta.title.clone() },
        author_url: handle.map(|h| format!("https://x.com/{h}")),
        media,
        ..Default::default()
    };
    unfurled.has_content().then_some(unfurled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twitter_photos_are_requested_in_large_not_original_size() {
        assert_eq!(photo_url("https://pbs.twimg.com/media/HFURRWmXYAEcq7c.jpg?name=orig"), "https://pbs.twimg.com/media/HFURRWmXYAEcq7c.jpg?name=large");
        assert_eq!(photo_url("https://pbs.twimg.com/media/a.jpg?format=jpg&name=small"), "https://pbs.twimg.com/media/a.jpg?format=jpg&name=large");
        assert_eq!(photo_url("https://pbs.twimg.com/media/a.jpg"), "https://pbs.twimg.com/media/a.jpg?name=large");
        // Other hosts are left alone.
        assert_eq!(photo_url("https://example.com/a.png?name=orig"), "https://example.com/a.png?name=orig");
        assert_eq!(photo_url("not a url"), "not a url");
    }
}
