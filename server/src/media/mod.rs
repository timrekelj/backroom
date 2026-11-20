//! Media: image processing, storage keys, accounting and the public
//! `GET /media/{key}` route. Avatars and guild icons use [`MediaKind`]'s
//! square-WebP rules; emojis fit within 128x128 and may stay animated (GIF).

mod accounting;
mod http;
mod pipeline;

use sha2::{Digest, Sha256};

pub use accounting::{Owner, delete_record, record, totals};
pub use http::media_route;
pub use pipeline::{MAX_INPUT_BYTES, MediaError, Processed, process};
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
        _ => None,
    }
}

/// Content type a key's extension implies (used when the store kept none).
pub fn content_type_of_key(key: &str) -> &'static str {
    if key.ends_with(".gif") { "image/gif" } else { "image/webp" }
}

/// Key prefixes the `/media` route serves.
const KEY_PREFIXES: &[&str] = &["avatars", "guild-icons", "emojis"];
const KEY_EXTENSIONS: &[&str] = &["webp", "gif"];
const HASH_HEX_LEN: usize = 32;

/// Content-addressed key `{prefix}/{owner_id}/{hash}.{ext}` (`ext` is `webp`
/// or `gif`, see `Processed::extension`): URL-safe, unique per content and
/// owner, so stored objects are immutable.
pub fn make_key(kind: MediaKind, owner_id: u64, content: &[u8], ext: &str) -> String {
    let digest = Sha256::digest(content);
    let hash: String = digest.iter().take(HASH_HEX_LEN / 2).map(|b| format!("{b:02x}")).collect();
    format!("{}/{owner_id}/{hash}.{ext}", kind.prefix())
}

/// The content hash of a well-formed key, or `None` if `key` is not exactly
/// the shape [`make_key`] produces (this is what the public route accepts:
/// no traversal, no odd characters).
pub fn parse_key(key: &str) -> Option<&str> {
    let mut parts = key.split('/');
    let (prefix, owner, file) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !KEY_PREFIXES.contains(&prefix) {
        return None;
    }
    if owner.is_empty() || owner.len() > 20 || !owner.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (hash, ext) = file.split_once('.')?;
    let hash_ok = hash.len() == HASH_HEX_LEN && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    // Only emojis may be animated, so only they use `.gif`.
    let ext_ok = KEY_EXTENSIONS.contains(&ext) && (ext == "webp" || prefix == "emojis");
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
