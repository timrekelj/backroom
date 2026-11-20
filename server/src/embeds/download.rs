//! The media step of provider embeds: downloads the images, videos and GIFs a
//! provider found into object storage (through the safe fetcher), plus a
//! small author avatar and the legacy 400 px thumbnail.
//!
//! - Images are decoded (png/jpeg/webp/gif by signature, not by header),
//!   fit within 1600 px and stored as WebP; posters likewise.
//! - Videos are streamed into S3 as downloaded (no transcoding) after the
//!   container was recognised from the first bytes (MP4/MOV `ftyp`, WebM).
//!   A video over the cap (declared or streamed) tries the provider's other
//!   variants, and failing those is dropped: its poster stays as an image.
//! - At most `max_media` items; any piece that fails is skipped silently.
//!
//! Objects are uploaded here but accounted and referenced by the rows only
//! when the embed is stored (`super::store`); [`Media::objects`] lists them so
//! that step can account for them or delete what ends up unused.

use std::time::Duration;

use bytes::Bytes;
use futures::future::join_all;
use url::Url;

use super::fetch::{FetchError, Fetcher};
use super::provider::{AssetKind, MediaSource, Unfurled};
use crate::media::{self, EmbedThumbnail};
use crate::storage::Storage;

/// Longest edge of a stored embed image or poster.
pub const IMAGE_EDGE: u32 = 1600;
/// Longest edge of the author avatar.
pub const AVATAR_EDGE: u32 = 96;

pub struct Limits {
    /// Largest image download.
    pub image_cap: usize,
    pub max_video_bytes: usize,
    /// Deadline of one download (all redirects and the body).
    pub timeout: Duration,
    pub max_media: usize,
}

/// A downloaded, stored piece of media.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMedia {
    pub kind: AssetKind,
    pub file: String,
    pub poster: Option<String>,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u32>,
    pub size: u64,
}

#[derive(Default)]
pub struct Media {
    pub items: Vec<StoredMedia>,
    pub author_icon: Option<String>,
    /// 400 px thumbnail of the first still (back-compat `thumbnail` field), not yet stored.
    pub thumbnail: Option<EmbedThumbnail>,
    /// Every object uploaded (key, size), to account for or delete.
    pub objects: Vec<(String, u64)>,
}

/// Container of a video from its first bytes: `(key extension, content type)`.
pub fn sniff_video(head: &[u8]) -> Option<(&'static str, &'static str)> {
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        Some(("mp4", "video/mp4"))
    } else if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some(("webm", "video/webm"))
    } else {
        None
    }
}

/// Downloads all media of `unfurled` into `storage` for an embed in `channel_id`.
pub async fn fetch_media(fetcher: &Fetcher, storage: &Storage, channel_id: u64, unfurled: &Unfurled, limits: &Limits) -> Media {
    let fetcher = fetcher.with_total_timeout(limits.timeout);
    let sources: Vec<&MediaSource> = unfurled.media.iter().take(limits.max_media).collect();
    let pieces = join_all(sources.into_iter().map(|s| piece(&fetcher, storage, channel_id, s, limits))).await;
    let mut out = Media::default();
    for piece in pieces {
        out.objects.extend(piece.objects);
        if let Some(thumb) = piece.thumbnail
            && out.thumbnail.is_none()
        {
            out.thumbnail = Some(thumb);
        }
        out.items.extend(piece.item);
    }
    if let Some(icon) = &unfurled.author_icon
        && let Some((key, size)) = fetch_image(&fetcher, storage, channel_id, icon, limits.image_cap, AVATAR_EDGE).await.map(|i| (i.key, i.size))
    {
        out.objects.push((key.clone(), size));
        out.author_icon = Some(key);
    }
    out
}

#[derive(Default)]
struct Piece {
    item: Option<StoredMedia>,
    thumbnail: Option<EmbedThumbnail>,
    objects: Vec<(String, u64)>,
}

struct StoredImage {
    key: String,
    size: u64,
    width: u32,
    height: u32,
    /// The stored WebP (for deriving the thumbnail).
    bytes: Bytes,
}

async fn piece(fetcher: &Fetcher, storage: &Storage, channel_id: u64, source: &MediaSource, limits: &Limits) -> Piece {
    let mut piece = Piece::default();
    match source.kind {
        AssetKind::Image => {
            if let Some(image) = fetch_image(fetcher, storage, channel_id, &source.url, limits.image_cap, IMAGE_EDGE).await {
                piece.thumbnail = media::embed_image(image.bytes.to_vec(), 400).await;
                piece.objects.push((image.key.clone(), image.size));
                piece.item = Some(StoredMedia {
                    kind: AssetKind::Image,
                    file: image.key,
                    poster: None,
                    width: image.width,
                    height: image.height,
                    duration_ms: None,
                    size: image.size,
                });
            }
        }
        AssetKind::Video | AssetKind::Gifv => {
            let poster = match &source.poster {
                Some(p) => fetch_image(fetcher, storage, channel_id, p, limits.image_cap, IMAGE_EDGE).await,
                None => None,
            };
            if let Some(poster) = &poster {
                piece.thumbnail = media::embed_image(poster.bytes.to_vec(), 400).await;
                piece.objects.push((poster.key.clone(), poster.size));
            }
            match fetch_video(fetcher, storage, channel_id, source, limits.max_video_bytes).await {
                Some((key, size)) => {
                    piece.objects.push((key.clone(), size));
                    piece.item = Some(StoredMedia {
                        kind: source.kind,
                        file: key,
                        poster: poster.as_ref().map(|p| p.key.clone()),
                        width: source.width.or(poster.as_ref().map(|p| p.width)).unwrap_or(0),
                        height: source.height.or(poster.as_ref().map(|p| p.height)).unwrap_or(0),
                        duration_ms: source.duration_ms,
                        size,
                    });
                }
                // No video file (too large, unreachable, not a video): the still is what is left.
                None => {
                    if let Some(poster) = poster {
                        piece.item = Some(StoredMedia {
                            kind: AssetKind::Image,
                            file: poster.key,
                            poster: None,
                            width: poster.width,
                            height: poster.height,
                            duration_ms: None,
                            size: poster.size,
                        });
                    }
                }
            }
        }
    }
    piece
}

/// Downloads an image (by signature, whatever its header says), re-encodes it
/// as WebP within `edge` and stores it. `None` on any failure.
async fn fetch_image(fetcher: &Fetcher, storage: &Storage, channel_id: u64, url: &str, cap: usize, edge: u32) -> Option<StoredImage> {
    let url = Url::parse(url).ok()?;
    let body = fetcher
        .within(async {
            let reply = fetcher.get(&url).await?;
            // The signature decides in the end; CDNs sometimes call images octet-stream.
            if !(reply.content_type.starts_with("image/") || matches!(reply.content_type.as_str(), "application/octet-stream" | "binary/octet-stream")) {
                return Err(FetchError::NotAllowedType(reply.content_type));
            }
            reply.read_capped(cap).await
        })
        .await
        .map_err(|e| tracing::debug!(%url, error = %e, "embed image download failed"))
        .ok()?;
    let encoded = media::embed_image(body, edge).await?;
    let key = media::embed_media_key(channel_id, "webp");
    let size = encoded.bytes.len() as u64;
    storage
        .put(&key, encoded.bytes.clone(), "image/webp")
        .await
        .map_err(|e| tracing::warn!(error = %format!("{e:#}"), "storing embed image failed"))
        .ok()?;
    Some(StoredImage { key, size, width: encoded.width, height: encoded.height, bytes: encoded.bytes })
}

/// Streams the first acceptable variant of a video into storage.
async fn fetch_video(fetcher: &Fetcher, storage: &Storage, channel_id: u64, source: &MediaSource, cap: usize) -> Option<(String, u64)> {
    for candidate in std::iter::once(&source.url).chain(source.alternates.iter()) {
        let Ok(url) = Url::parse(candidate) else { continue };
        let result = fetcher
            .within(async {
                let reply = fetcher.get_accepting(&url, "video/mp4,video/*;q=0.9,*/*;q=0.1").await?;
                // Plain `application/octet-stream` is common for CDN video; the signature decides.
                if !(reply.content_type.starts_with("video/") || matches!(reply.content_type.as_str(), "application/octet-stream" | "binary/octet-stream" | "")) {
                    return Err(FetchError::NotAllowedType(reply.content_type));
                }
                reply
                    .download_video_to_storage(cap, storage, |head| {
                        let (ext, content_type) = sniff_video(head)?;
                        Some((media::embed_media_key(channel_id, ext), content_type))
                    })
                    .await
            })
            .await;
        match result {
            Ok(done) => return Some(done),
            Err(e) => tracing::debug!(%url, error = %e, "embed video download failed"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_containers_are_recognised_by_signature() {
        let mp4 = [0, 0, 0, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 2, 0];
        assert_eq!(sniff_video(&mp4), Some(("mp4", "video/mp4")));
        let mov = [0, 0, 0, 0x14, b'f', b't', b'y', b'p', b'q', b't', b' ', b' '];
        assert_eq!(sniff_video(&mov), Some(("mp4", "video/mp4")));
        assert_eq!(sniff_video(&[0x1A, 0x45, 0xDF, 0xA3, 1, 2, 3, 4, 5, 6, 7, 8]), Some(("webm", "video/webm")));
        for bad in [&b"<html><body>not a video at all</body>"[..], b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", b"ftyp", b""] {
            assert_eq!(sniff_video(bad), None);
        }
    }
}
