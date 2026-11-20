//! Media: image processing, storage keys, accounting and the public
//! `GET /media/{key}` route. Avatars and guild icons use [`MediaKind`]'s
//! square-WebP rules; emojis fit within 128x128 and may stay animated (GIF).

mod accounting;
pub mod attachments;
mod http;
pub mod pending;
mod pipeline;
pub mod sniff;

use sha2::{Digest, Sha256};

pub use accounting::{Owner, delete_record, record, totals};
pub use http::media_route;
pub use pipeline::{
    ATTACHMENT_IMAGE_MAX_BYTES, ImageInfo, MAX_INPUT_BYTES, MediaError, Processed, acquire_decode_slot,
    inspect_attachment_image, process, EmbedThumbnail, embed_image, embed_thumbnail,
};
pub use accounting::{KindTotals, totals_by_kind};

/// What an image is used for; decides its processing rules and key prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Avatar,
    GuildIcon,
    Emoji,
}

/// How a kind's output image is shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Centre-cropped square of this edge length (upscaling allowed).
    CropSquare(u32),
    /// Fits within a square of this edge length, aspect kept, never upscaled.
    Fit(u32),
}

impl MediaKind {
    /// First key segment, also the storage "directory".
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::GuildIcon => "guild-icons",
            Self::Emoji => "emojis",
        }
    }

    /// Shape of the output image.
    pub fn shape(self) -> Shape {
        match self {
            Self::Avatar | Self::GuildIcon => Shape::CropSquare(256),
            Self::Emoji => Shape::Fit(128),
        }
    }

    /// Whether animation is preserved (animated GIF/WebP -> GIF); otherwise
    /// only the first frame is kept.
    pub fn animated(self) -> bool {
        matches!(self, Self::Emoji)
    }

    /// Largest accepted upload in bytes.
    pub fn max_input_bytes(self) -> usize {
        match self {
            Self::Avatar | Self::GuildIcon => MAX_INPUT_BYTES,
            Self::Emoji => EMOJI_MAX_INPUT_BYTES,
        }
    }

    /// Owner kind recorded in storage accounting.
    pub fn owner_kind(self) -> &'static str {
        match self {
            Self::Avatar => "user",
            Self::GuildIcon | Self::Emoji => "guild",
        }
    }

    /// Accounting label (the admin storage breakdown): `avatar`, `guild_icon`, `emoji`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::GuildIcon => "guild_icon",
            Self::Emoji => "emoji",
        }
    }
}

/// Largest accepted emoji upload.
pub const EMOJI_MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

/// Accounting label of the object stored under `key`, from its prefix.
pub fn label_of_key(key: &str) -> Option<&'static str> {
    match key.split('/').next()? {
        "avatars" => Some("avatar"),
        "guild-icons" => Some("guild_icon"),
        "emojis" => Some("emoji"),
        "attachments" => Some("attachment"),
        "embeds" => Some("embed"),
        _ => None,
    }
}

/// Content type a key's extension implies (used when the store kept none).
pub fn content_type_of_key(key: &str) -> &'static str {
    if key.ends_with(".gif") {
        "image/gif"
    } else if key.ends_with(".mp4") {
        "video/mp4"
    } else if key.ends_with(".webm") {
        "video/webm"
    } else {
        "image/webp"
    }
}

/// Key prefixes the `/media` route serves.
const KEY_PREFIXES: &[&str] = &["avatars", "guild-icons", "emojis", "embeds"];
/// Prefix of message attachment keys (`attachments/{channel_id}/{32 hex}` and
/// `…/{32 hex}-thumb.webp`); the hex part is random, not a content hash.
pub const ATTACHMENT_PREFIX: &str = "attachments";
const THUMB_SUFFIX: &str = "-thumb.webp";
const KEY_EXTENSIONS: &[&str] = &["webp", "gif", "mp4", "webm"];
const HASH_HEX_LEN: usize = 32;

/// Content-addressed key `{prefix}/{owner_id}/{hash}.{ext}` (`ext` is `webp`
/// or `gif`, see `Processed::extension`): URL-safe, unique per content and
/// owner, so stored objects are immutable.
pub fn make_key(kind: MediaKind, owner_id: u64, content: &[u8], ext: &str) -> String {
    let digest = Sha256::digest(content);
    let hash: String = digest.iter().take(HASH_HEX_LEN / 2).map(|b| format!("{b:02x}")).collect();
    format!("{}/{owner_id}/{hash}.{ext}", kind.prefix())
}

/// Random 128-bit hex id of a new attachment object.
pub fn random_attachment_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `attachments/{channel_id}/{id}`.
pub fn attachment_key(channel_id: u64, id: &str) -> String {
    format!("{ATTACHMENT_PREFIX}/{channel_id}/{id}")
}

/// `attachments/{channel_id}/{id}-thumb.webp`.
pub fn attachment_thumbnail_key(channel_id: u64, id: &str) -> String {
    format!("{ATTACHMENT_PREFIX}/{channel_id}/{id}{THUMB_SUFFIX}")
}

/// `embeds/{channel_id}/{32 hex}.webp`: a link embed's thumbnail. The hex part
/// is random (like attachment ids), so the key is unguessable.
pub fn embed_thumbnail_key(channel_id: u64) -> String {
    format!("embeds/{channel_id}/{}.webp", random_attachment_id())
}

/// `embeds/{channel_id}/{32 hex}.{ext}`: a stored image (`webp`) or video
/// (`mp4`, `webm`) of an embed.
pub fn embed_media_key(channel_id: u64, ext: &str) -> String {
    format!("embeds/{channel_id}/{}.{ext}", random_attachment_id())
}

/// Whether `key` names an attachment object (file or thumbnail).
pub fn is_attachment_key(key: &str) -> bool {
    key.starts_with("attachments/")
}

fn is_hex_id(s: &str) -> bool {
    s.len() == HASH_HEX_LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The content hash (or, for attachments, random id) of a well-formed key, or
/// `None` if `key` is not exactly a shape this server produces (this is what
/// the public route accepts: no traversal, no odd characters).
pub fn parse_key(key: &str) -> Option<&str> {
    let mut parts = key.split('/');
    let (prefix, owner, file) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    if owner.is_empty() || owner.len() > 20 || !owner.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if prefix == ATTACHMENT_PREFIX {
        let id = file.strip_suffix(THUMB_SUFFIX).unwrap_or(file);
        return is_hex_id(id).then_some(id);
    }
    if !KEY_PREFIXES.contains(&prefix) {
        return None;
    }
    let (hash, ext) = file.split_once('.')?;
    let hash_ok = is_hex_id(hash);
    // Only emojis may be animated, so only they use `.gif`; only embeds hold videos.
    let ext_ok = match ext {
        "webp" => true,
        "gif" => prefix == "emojis",
        "mp4" | "webm" => prefix == "embeds",
        _ => false,
    };
    debug_assert!(KEY_EXTENSIONS.contains(&ext) || !ext_ok);
    (hash_ok && ext_ok).then_some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip_and_are_content_addressed() {
        let a = make_key(MediaKind::Avatar, 42, b"abc", "webp");
        assert_eq!(a, make_key(MediaKind::Avatar, 42, b"abc", "webp"));
        assert_ne!(a, make_key(MediaKind::Avatar, 42, b"abd", "webp"));
        assert!(a.starts_with("avatars/42/") && a.ends_with(".webp"));
        assert!(parse_key(&a).is_some());
        assert!(parse_key(&make_key(MediaKind::GuildIcon, 7, b"x", "webp")).is_some());
        let emoji = make_key(MediaKind::Emoji, 7, b"x", "gif");
        assert!(emoji.starts_with("emojis/7/") && parse_key(&emoji).is_some());
        assert!(parse_key(&make_key(MediaKind::Emoji, 7, b"x", "webp")).is_some());
        assert_eq!(label_of_key(&emoji), Some("emoji"));
        assert_eq!(content_type_of_key(&emoji), "image/gif");
        // Only emojis may be GIFs.
        assert!(parse_key(&make_key(MediaKind::Avatar, 7, b"x", "gif")).is_none());
    }

    #[test]
    fn attachment_keys_roundtrip_and_odd_ones_are_rejected() {
        let id = random_attachment_id();
        assert_eq!(id.len(), 32);
        assert_eq!(parse_key(&attachment_key(42, &id)), Some(id.as_str()));
        assert_eq!(parse_key(&attachment_thumbnail_key(42, &id)), Some(id.as_str()));
        assert!(is_attachment_key(&attachment_key(42, &id)));
        assert_eq!(label_of_key(&attachment_key(42, &id)), Some("attachment"));
        let zeros = "0".repeat(32);
        for bad in [
            format!("attachments/42/{zeros}.webp"),
            format!("attachments/42/{zeros}.png"),
            format!("attachments/42/{zeros}-thumb.gif"),
            format!("attachments/42/{zeros}x"),
            format!("attachments/x/{zeros}"),
            format!("attachments//{zeros}"),
            format!("attachments/42/{zeros}/y"),
            format!("attachments/42/../{zeros}"),
            "attachments/42/ZZ".to_owned(),
            format!("attachments/42/{}", "0".repeat(31)),
            format!("attachments/42/{}", "A".repeat(32)),
        ] {
            assert!(parse_key(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn embed_keys_roundtrip_and_odd_ones_are_rejected() {
        let key = embed_thumbnail_key(42);
        assert!(key.starts_with("embeds/42/") && key.ends_with(".webp"));
        assert!(parse_key(&key).is_some());
        assert_eq!(label_of_key(&key), Some("embed"));
        for ext in ["mp4", "webm", "webp"] {
            let media = embed_media_key(42, ext);
            assert!(parse_key(&media).is_some(), "{media}");
            assert_eq!(label_of_key(&media), Some("embed"));
        }
        assert_eq!(content_type_of_key(&embed_media_key(42, "mp4")), "video/mp4");
        assert_eq!(content_type_of_key(&embed_media_key(42, "webm")), "video/webm");
        let zeros = "0".repeat(32);
        for bad in [
            format!("embeds/42/{zeros}.gif"),
            format!("embeds/42/{zeros}.png"),
            format!("embeds/42/{zeros}.mov"),
            format!("avatars/42/{zeros}.mp4"),
            format!("emojis/42/{zeros}.webm"),
            format!("embeds/42/{zeros}"),
            format!("embeds/x/{zeros}.webp"),
            format!("embeds/42/../{zeros}.webp"),
            format!("embeds/42/{zeros}.webp/y"),
        ] {
            assert!(parse_key(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn odd_keys_are_rejected() {
        let ok = make_key(MediaKind::Avatar, 1, b"x", "webp");
        assert!(parse_key(&ok).is_some());
        for bad in [
            "",
            "avatars",
            "avatars/1",
            "../etc/passwd",
            "avatars/../1/00000000000000000000000000000000.webp",
            "avatars/1/../00000000000000000000000000000000.webp",
            "avatars/1/0000000000000000000000000000000.webp",
            "avatars/1/0000000000000000000000000000000G.webp",
            "avatars/1/00000000000000000000000000000000.png",
            "avatars/1/00000000000000000000000000000000.gif",
            "emojis/1/00000000000000000000000000000000.png",
            "avatars/1/00000000000000000000000000000000.webp/x",
            "avatars/1/00000000000000000000000000000000.webp.webp",
            "avatars/x/00000000000000000000000000000000.webp",
            "secrets/1/00000000000000000000000000000000.webp",
            "avatars//00000000000000000000000000000000.webp",
            "avatars/1/%2e%2e.webp",
        ] {
            assert!(parse_key(bad).is_none(), "{bad}");
        }
    }
}
