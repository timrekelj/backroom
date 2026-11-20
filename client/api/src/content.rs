//! Emojis, reactions, attachments and embeds: the rich-content domain types of a message.

use std::time::SystemTime;

use crate::proto::thehideout::v1 as pb;
use crate::types::time;

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// A custom emoji owned by a guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emoji {
    pub id: u64,
    pub guild_id: u64,
    /// Matches `^[a-z0-9_]{2,32}$`, unique per guild.
    pub name: String,
    /// Animated emojis are GIFs; static ones are WebP.
    pub animated: bool,
    /// Media key of the image (see [`Client::media_url`](crate::Client::media_url)).
    pub image: String,
    pub creator_id: u64,
    pub created_at: SystemTime,
}

impl Emoji {
    /// The message token that renders this emoji: `<:name:id>` or `<a:name:id>`.
    pub fn token(&self) -> String {
        format!(
            "<{}:{}:{}>",
            if self.animated { "a" } else { "" },
            self.name,
            self.id
        )
    }
}

/// The emoji of a reaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReactionEmoji {
    /// A single Unicode emoji grapheme cluster.
    Unicode(String),
    /// A custom emoji of the channel's guild.
    Custom(u64),
}

impl ReactionEmoji {
    /// A stable string key (`u:<emoji>` / `c:<id>`), e.g. for map keys.
    pub fn key(&self) -> String {
        match self {
            Self::Unicode(e) => format!("u:{e}"),
            Self::Custom(id) => format!("c:{id}"),
        }
    }

    /// Parses a [`key`](Self::key).
    pub fn from_key(key: &str) -> Option<Self> {
        if let Some(e) = key.strip_prefix("u:") {
            (!e.is_empty()).then(|| Self::Unicode(e.to_owned()))
        } else {
            key.strip_prefix("c:")?.parse().ok().map(Self::Custom)
        }
    }

    pub(crate) fn from_proto(e: pb::ReactionEmoji) -> Option<Self> {
        use pb::reaction_emoji::Kind;
        Some(match e.kind? {
            Kind::Unicode(s) => Self::Unicode(s),
            Kind::CustomEmojiId(id) => Self::Custom(id),
        })
    }

    pub(crate) fn to_proto(&self) -> pb::ReactionEmoji {
        use pb::reaction_emoji::Kind;
        pb::ReactionEmoji {
            kind: Some(match self {
                Self::Unicode(s) => Kind::Unicode(s.clone()),
                Self::Custom(id) => Kind::CustomEmojiId(*id),
            }),
            ..Default::default()
        }
    }
}

/// The reactions of one emoji on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub emoji: ReactionEmoji,
    pub count: u32,
    /// Whether the caller reacted. Always `false` in gateway events: maintain it
    /// from reaction events carrying your own user id.
    pub me: bool,
}

impl Reaction {
    pub(crate) fn from_proto(r: pb::Reaction) -> Option<Self> {
        Some(Self {
            emoji: ReactionEmoji::from_proto(r.emoji.into_option()?)?,
            count: r.count,
            me: r.me,
        })
    }
}

/// A file attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: u64,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    /// Media key of the file; `None` if `removed`.
    pub file: Option<String>,
    /// Pixel size (images only).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Media key of a WebP thumbnail (images only); `None` if absent or `removed`.
    pub thumbnail: Option<String>,
    /// A site admin deleted the file; show a placeholder.
    pub removed: bool,
}

impl From<pb::Attachment> for Attachment {
    fn from(a: pb::Attachment) -> Self {
        Self {
            id: a.id,
            filename: a.filename,
            content_type: a.content_type,
            size: a.size,
            file: non_empty(a.file),
            width: a.width,
            height: a.height,
            thumbnail: a.thumbnail.and_then(non_empty),
            removed: a.removed,
        }
    }
}

/// A link preview resolved by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embed {
    pub url: String,
    pub title: String,
    pub description: String,
    pub site_name: String,
    /// Media key of the preview image.
    pub thumbnail: Option<String>,
    pub thumbnail_width: Option<u32>,
    pub thumbnail_height: Option<u32>,
}

impl From<pb::Embed> for Embed {
    fn from(e: pb::Embed) -> Self {
        Self {
            url: e.url,
            title: e.title,
            description: e.description,
            site_name: e.site_name,
            thumbnail: e.thumbnail.and_then(non_empty),
            thumbnail_width: e.thumbnail_width,
            thumbnail_height: e.thumbnail_height,
        }
    }
}

/// An uploaded file waiting to be linked by
/// [`Client::send_message_with`](crate::Client::send_message_with); it expires after an hour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAttachment {
    /// Pass in `attachment_ids`.
    pub id: u64,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    pub expires_at: SystemTime,
}

impl From<pb::PendingAttachment> for PendingAttachment {
    fn from(p: pb::PendingAttachment) -> Self {
        Self {
            id: p.id,
            filename: p.filename,
            content_type: p.content_type,
            size: p.size,
            expires_at: time(p.expires_at.as_option()),
        }
    }
}

impl From<pb::Emoji> for Emoji {
    fn from(e: pb::Emoji) -> Self {
        Self {
            id: e.id,
            guild_id: e.guild_id,
            name: e.name,
            animated: e.animated,
            image: e.image,
            creator_id: e.creator_id,
            created_at: time(e.created_at.as_option()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaction_emoji_key_round_trips() {
        for e in [
            ReactionEmoji::Unicode("👍".into()),
            ReactionEmoji::Custom(42),
        ] {
            assert_eq!(ReactionEmoji::from_key(&e.key()), Some(e));
        }
        assert_eq!(ReactionEmoji::Unicode("👍".into()).key(), "u:👍");
        assert_eq!(ReactionEmoji::Custom(7).key(), "c:7");
        for bad in ["", "u:", "c:x", "x:1", "c:"] {
            assert_eq!(ReactionEmoji::from_key(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn reaction_emoji_proto_round_trips_and_unset_kind_is_dropped() {
        for e in [
            ReactionEmoji::Unicode("🎉".into()),
            ReactionEmoji::Custom(9),
        ] {
            assert_eq!(ReactionEmoji::from_proto(e.to_proto()), Some(e));
        }
        assert_eq!(
            ReactionEmoji::from_proto(pb::ReactionEmoji::default()),
            None
        );
        assert!(Reaction::from_proto(pb::Reaction {
            count: 2,
            ..Default::default()
        })
        .is_none());
        let r = Reaction::from_proto(pb::Reaction {
            emoji: ReactionEmoji::Custom(3).to_proto().into(),
            count: 2,
            me: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            (r.emoji, r.count, r.me),
            (ReactionEmoji::Custom(3), 2, true)
        );
    }

    #[test]
    fn attachment_maps_optional_fields_and_removal() {
        let a: Attachment = pb::Attachment {
            id: 1,
            filename: "a.png".into(),
            content_type: "image/png".into(),
            size: 10,
            file: "k".into(),
            width: Some(3),
            height: Some(4),
            thumbnail: Some("t".into()),
            ..Default::default()
        }
        .into();
        assert_eq!(
            (
                a.file.as_deref(),
                a.width,
                a.height,
                a.thumbnail.as_deref(),
                a.removed
            ),
            (Some("k"), Some(3), Some(4), Some("t"), false)
        );
        let a: Attachment = pb::Attachment {
            removed: true,
            thumbnail: Some(String::new()),
            ..Default::default()
        }
        .into();
        assert_eq!((a.file, a.thumbnail, a.removed), (None, None, true));
    }

    #[test]
    fn embed_and_pending_and_emoji_convert() {
        let e: Embed = pb::Embed {
            url: "u".into(),
            thumbnail: Some("t".into()),
            thumbnail_width: Some(5),
            ..Default::default()
        }
        .into();
        assert_eq!(
            (
                e.url.as_str(),
                e.thumbnail.as_deref(),
                e.thumbnail_width,
                e.thumbnail_height
            ),
            ("u", Some("t"), Some(5), None)
        );
        let p: PendingAttachment = pb::PendingAttachment {
            id: 8,
            size: 3,
            filename: "f".into(),
            ..Default::default()
        }
        .into();
        assert_eq!((p.id, p.size, p.filename.as_str()), (8, 3, "f"));
        let m: Emoji = pb::Emoji {
            id: 5,
            name: "party".into(),
            animated: true,
            image: "i".into(),
            ..Default::default()
        }
        .into();
        assert_eq!(m.token(), "<a:party:5>");
        assert_eq!(
            Emoji {
                animated: false,
                ..m
            }
            .token(),
            "<:party:5>"
        );
    }
}
