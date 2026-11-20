//! Media: image processing, storage keys, accounting and the public
//! `GET /media/{key}` route. Avatars and guild icons use [`MediaKind`]'s
//! square-WebP rules; emojis later add a kind with their own rules.

mod accounting;
mod http;
mod pipeline;

use sha2::{Digest, Sha256};

pub use accounting::{Owner, delete_record, record, totals};
pub use http::media_route;
pub use pipeline::{MAX_INPUT_BYTES, MediaError, Processed, process};

/// What an image is used for; decides its processing rules and key prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Avatar,
    GuildIcon,
}

impl MediaKind {
    /// First key segment, also the storage "directory".
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::GuildIcon => "guild-icons",
        }
    }

    /// Edge length of the square output image.
    pub fn size(self) -> u32 {
        match self {
            Self::Avatar | Self::GuildIcon => 256,
        }
    }

    /// Whether animation is preserved. Every current kind keeps the first frame only.
    pub fn animated(self) -> bool {
        false
    }

    /// Owner kind recorded in storage accounting.
    pub fn owner_kind(self) -> &'static str {
        match self {
            Self::Avatar => "user",
            Self::GuildIcon => "guild",
        }
    }
}

/// Key prefixes the `/media` route serves.
const KEY_PREFIXES: &[&str] = &["avatars", "guild-icons"];
const KEY_EXTENSIONS: &[&str] = &["webp"];
const HASH_HEX_LEN: usize = 32;

/// Content-addressed key `{prefix}/{owner_id}/{hash}.{ext}`: URL-safe, unique
/// per content and owner, so stored objects are immutable.
pub fn make_key(kind: MediaKind, owner_id: u64, content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let hash: String = digest.iter().take(HASH_HEX_LEN / 2).map(|b| format!("{b:02x}")).collect();
    format!("{}/{owner_id}/{hash}.webp", kind.prefix())
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
    (hash_ok && KEY_EXTENSIONS.contains(&ext)).then_some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip_and_are_content_addressed() {
        let a = make_key(MediaKind::Avatar, 42, b"abc");
        assert_eq!(a, make_key(MediaKind::Avatar, 42, b"abc"));
        assert_ne!(a, make_key(MediaKind::Avatar, 42, b"abd"));
        assert!(a.starts_with("avatars/42/") && a.ends_with(".webp"));
        assert!(parse_key(&a).is_some());
        assert!(parse_key(&make_key(MediaKind::GuildIcon, 7, b"x")).is_some());
    }

    #[test]
    fn odd_keys_are_rejected() {
        let ok = make_key(MediaKind::Avatar, 1, b"x");
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
