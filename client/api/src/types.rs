//! Plain domain types returned by the API; none of them expose generated proto types.

use std::time::SystemTime;

use buffa_types::google::protobuf::Timestamp;

use crate::proto::thehideout::v1 as pb;
use crate::structure::{ChannelIcon, GuildLayout, Member, PaletteColor, Role};

/// A registered user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: u64,
    pub username: String,
    pub created_at: SystemTime,
    /// Media key of the avatar (see [`Client::media_url`](crate::Client::media_url)); `None` = no avatar.
    pub avatar: Option<String>,
    /// Whether the user is a site administrator.
    pub is_admin: bool,
}

/// A guild (server) that groups members and channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guild {
    pub id: u64,
    pub name: String,
    pub owner_id: u64,
    /// Code other users pass to [`Client::join_guild`](crate::Client::join_guild).
    pub invite_code: String,
    pub created_at: SystemTime,
    /// Media key of the guild icon; `None` = no icon.
    pub icon: Option<String>,
}

/// A text channel inside a guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: u64,
    pub guild_id: u64,
    pub name: String,
    pub created_at: SystemTime,
    /// Section the channel is in; `None` = top level.
    pub section_id: Option<u64>,
    /// 0-based position within its section (the layout is authoritative).
    pub position: i32,
    pub icon: ChannelIcon,
    pub color: PaletteColor,
    /// Free-text topic; empty = none.
    pub topic: String,
}

/// A 1:1 direct-message channel. Its `id` is the `channel_id` for messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmChannel {
    pub id: u64,
    /// Both participants, ordered by user id ascending.
    pub participants: Vec<User>,
    pub created_at: SystemTime,
}

/// A chat message in a guild channel or a DM channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: u64,
    pub channel_id: u64,
    /// The owning guild; `None` for DM channels.
    pub guild_id: Option<u64>,
    pub author_id: u64,
    pub content: String,
    pub created_at: SystemTime,
    pub edited_at: Option<SystemTime>,
    /// The nonce given to `send_message`; empty if none was given.
    pub nonce: String,
}

/// One page of messages, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Pass as `before` to fetch the next (older) page; `None` when there are no more.
    pub next_before: Option<u64>,
    pub has_more: bool,
}

/// A real-time event from the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    MessageCreated(Message),
    ChannelCreated(Channel),
    GuildMemberJoined { guild_id: u64, user: User },
    /// A DM channel was opened with the caller.
    DmOpened(DmChannel),
    /// Keep-alive from the server.
    Heartbeat,
    /// A guild's name or icon changed.
    GuildUpdated(Guild),
    /// A channel's name, icon, colour or topic changed.
    ChannelUpdated(Channel),
    /// A guild's sections and/or channel order changed.
    LayoutUpdated { guild_id: u64, layout: GuildLayout },
    /// A guild's roles changed; always the full list, so replace yours.
    RolesUpdated { guild_id: u64, roles: Vec<Role>, version: u64 },
    /// A member's roles changed.
    MemberUpdated { guild_id: u64, member: Member },
    /// A user changed their profile (e.g. avatar).
    UserUpdated(User),
}

pub(crate) fn time(ts: Option<&Timestamp>) -> SystemTime {
    ts.and_then(|ts| SystemTime::try_from(ts.clone()).ok())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn media_key(key: String) -> Option<String> {
    (!key.is_empty()).then_some(key)
}

impl From<pb::User> for User {
    fn from(u: pb::User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            created_at: time(u.created_at.as_option()),
            avatar: media_key(u.avatar),
            is_admin: u.is_admin,
        }
    }
}

impl From<pb::Guild> for Guild {
    fn from(g: pb::Guild) -> Self {
        Self {
            id: g.id,
            name: g.name,
            owner_id: g.owner_id,
            invite_code: g.invite_code,
            created_at: time(g.created_at.as_option()),
            icon: media_key(g.icon),
        }
    }
}

impl From<pb::Channel> for Channel {
    fn from(c: pb::Channel) -> Self {
        Self {
            id: c.id,
            guild_id: c.guild_id,
            name: c.name,
            created_at: time(c.created_at.as_option()),
            section_id: c.section_id.filter(|&id| id != 0),
            position: c.position,
            icon: ChannelIcon(c.icon),
            color: PaletteColor::from_key(&c.color),
            topic: c.topic,
        }
    }
}

impl From<pb::DmChannel> for DmChannel {
    fn from(d: pb::DmChannel) -> Self {
        Self {
            id: d.id,
            participants: d.participants.into_iter().map(User::from).collect(),
            created_at: time(d.created_at.as_option()),
        }
    }
}

impl From<pb::Message> for Message {
    fn from(m: pb::Message) -> Self {
        Self {
            id: m.id,
            channel_id: m.channel_id,
            guild_id: (m.guild_id != 0).then_some(m.guild_id),
            author_id: m.author_id,
            content: m.content,
            created_at: time(m.created_at.as_option()),
            edited_at: m.edited_at.as_option().map(|ts| time(Some(ts))),
            nonce: m.nonce,
        }
    }
}

impl Event {
    /// Converts a wire event; `None` if it carries no known payload.
    pub(crate) fn from_proto(event: pb::Event) -> Option<Self> {
        use pb::event::Payload;
        Some(match event.payload? {
            Payload::MessageCreated(m) => Self::MessageCreated((*m).into()),
            Payload::ChannelCreated(c) => Self::ChannelCreated((*c).into()),
            Payload::GuildMemberJoined(j) => Self::GuildMemberJoined {
                guild_id: j.guild_id,
                user: j.user.into_option().map(User::from)?,
            },
            Payload::Heartbeat(_) => Self::Heartbeat,
            Payload::DmOpened(d) => Self::DmOpened((*d).into()),
            Payload::GuildUpdated(g) => Self::GuildUpdated((*g).into()),
            Payload::ChannelUpdated(c) => Self::ChannelUpdated((*c).into()),
            Payload::LayoutUpdated(l) => Self::LayoutUpdated {
                guild_id: l.guild_id,
                layout: l.layout.into_option().map(GuildLayout::from)?,
            },
            Payload::RolesUpdated(r) => Self::RolesUpdated {
                guild_id: r.guild_id,
                roles: r.roles.into_iter().map(Role::from).collect(),
                version: r.version,
            },
            Payload::MemberUpdated(m) => Self::MemberUpdated {
                guild_id: m.guild_id,
                member: m.member.into_option().map(Member::from)?,
            },
            Payload::UserUpdated(u) => Self::UserUpdated((*u).into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_message_has_no_guild_and_unset_edit_time() {
        let m: Message = pb::Message { id: 1, guild_id: 0, ..Default::default() }.into();
        assert_eq!(m.guild_id, None);
        assert_eq!(m.edited_at, None);
        let m: Message = pb::Message { guild_id: 7, ..Default::default() }.into();
        assert_eq!(m.guild_id, Some(7));
    }

    #[test]
    fn user_and_guild_media_keys_empty_is_none() {
        let u: User = pb::User { avatar: "k1".into(), is_admin: true, ..Default::default() }.into();
        assert_eq!(u.avatar.as_deref(), Some("k1"));
        assert!(u.is_admin);
        let u: User = pb::User::default().into();
        assert_eq!((u.avatar, u.is_admin), (None, false));
        let g: Guild = pb::Guild { icon: "i".into(), ..Default::default() }.into();
        assert_eq!(g.icon.as_deref(), Some("i"));
        assert_eq!(Guild::from(pb::Guild::default()).icon, None);
    }

    #[test]
    fn channel_section_zero_is_top_level_and_look_converts() {
        let c: Channel = pb::Channel {
            section_id: Some(0),
            icon: "hash".into(),
            color: "teal".into(),
            topic: "t".into(),
            position: 2,
            ..Default::default()
        }
        .into();
        assert_eq!(c.section_id, None);
        assert_eq!((c.icon.as_str(), c.color, c.topic.as_str(), c.position), ("hash", PaletteColor::Teal, "t", 2));
        let c: Channel = pb::Channel { section_id: Some(5), color: "bogus".into(), ..Default::default() }.into();
        assert_eq!(c.section_id, Some(5));
        assert_eq!(c.color, PaletteColor::None);
        assert!(c.icon.is_default());
        assert_eq!(Channel::from(pb::Channel::default()).section_id, None);
    }

    fn event(payload: pb::event::Payload) -> Option<Event> {
        Event::from_proto(pb::Event { payload: Some(payload), ..Default::default() })
    }

    #[test]
    fn new_gateway_payloads_map_to_events() {
        use pb::event::Payload;
        let guild = pb::Guild { id: 1, icon: "x".into(), ..Default::default() };
        assert!(matches!(event(Payload::GuildUpdated(Box::new(guild))), Some(Event::GuildUpdated(g)) if g.icon.as_deref() == Some("x")));
        assert!(matches!(
            event(Payload::ChannelUpdated(Box::new(pb::Channel { id: 2, ..Default::default() }))),
            Some(Event::ChannelUpdated(c)) if c.id == 2
        ));
        assert!(matches!(
            event(Payload::UserUpdated(Box::new(pb::User { id: 3, ..Default::default() }))),
            Some(Event::UserUpdated(u)) if u.id == 3
        ));

        let layout = pb::LayoutUpdated { guild_id: 4, layout: pb::GuildLayout { version: 6, ..Default::default() }.into(), ..Default::default() };
        assert!(matches!(
            event(Payload::LayoutUpdated(Box::new(layout))),
            Some(Event::LayoutUpdated { guild_id: 4, layout }) if layout.version == 6
        ));
        // A layout event without a layout is malformed and dropped.
        assert!(event(Payload::LayoutUpdated(Box::default())).is_none());

        let roles = pb::RolesUpdated {
            guild_id: 5,
            roles: vec![pb::Role { id: 1, ..Default::default() }],
            version: 8,
            ..Default::default()
        };
        assert!(matches!(
            event(Payload::RolesUpdated(Box::new(roles))),
            Some(Event::RolesUpdated { guild_id: 5, roles, version: 8 }) if roles.len() == 1
        ));

        let member = pb::MemberUpdated {
            guild_id: 6,
            member: pb::Member { role_ids: vec![9], ..Default::default() }.into(),
            ..Default::default()
        };
        assert!(matches!(
            event(Payload::MemberUpdated(Box::new(member))),
            Some(Event::MemberUpdated { guild_id: 6, member }) if member.role_ids == vec![9]
        ));
        assert!(event(Payload::MemberUpdated(Box::default())).is_none());
    }
}
