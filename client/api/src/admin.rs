//! Site-admin domain types (see `Client::admin_*` methods).

use std::time::SystemTime;

use crate::proto::thehideout::v1 as pb;
use crate::types::{time, Guild, User};

/// Site-wide totals; "today" is the current UTC day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    pub users: u64,
    pub guilds: u64,
    pub channels: u64,
    pub messages: u64,
    pub storage_bytes_total: u64,
    pub storage_bytes_avatars: u64,
    pub storage_bytes_guild_icons: u64,
    pub live_connections: u64,
    pub messages_today: u64,
    pub signups_today: u64,
    pub active_users_today: u64,
}

/// One day of activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityPoint {
    /// UTC date, `YYYY-MM-DD`.
    pub date: String,
    pub messages: u64,
    pub signups: u64,
    pub active_users: u64,
}

/// A guild with usage numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminGuildRow {
    pub guild: Guild,
    pub member_count: u64,
    pub channel_count: u64,
    pub message_count: u64,
    pub storage_bytes: u64,
}

/// A user with usage numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUserRow {
    pub user: User,
    pub guild_count: u64,
    pub message_count: u64,
    pub storage_bytes: u64,
    /// `None` if the user was never active.
    pub last_active_at: Option<SystemTime>,
}

/// One page of admin rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// Pass as `page_token` for the next page; `None` on the last page.
    pub next_page_token: Option<String>,
}

pub(crate) fn next_token(token: String) -> Option<String> {
    (!token.is_empty()).then_some(token)
}

impl From<pb::GetOverviewResponse> for Overview {
    fn from(o: pb::GetOverviewResponse) -> Self {
        Self {
            users: o.users,
            guilds: o.guilds,
            channels: o.channels,
            messages: o.messages,
            storage_bytes_total: o.storage_bytes_total,
            storage_bytes_avatars: o.storage_bytes_avatars,
            storage_bytes_guild_icons: o.storage_bytes_guild_icons,
            live_connections: o.live_connections,
            messages_today: o.messages_today,
            signups_today: o.signups_today,
            active_users_today: o.active_users_today,
        }
    }
}

impl From<pb::ActivityPoint> for ActivityPoint {
    fn from(p: pb::ActivityPoint) -> Self {
        Self {
            date: p.date,
            messages: p.messages,
            signups: p.signups,
            active_users: p.active_users,
        }
    }
}

impl AdminGuildRow {
    pub(crate) fn from_proto(r: pb::AdminGuildRow) -> Option<Self> {
        Some(Self {
            guild: r.guild.into_option().map(Guild::from)?,
            member_count: r.member_count,
            channel_count: r.channel_count,
            message_count: r.message_count,
            storage_bytes: r.storage_bytes,
        })
    }
}

impl AdminUserRow {
    pub(crate) fn from_proto(r: pb::AdminUserRow) -> Option<Self> {
        Some(Self {
            user: r.user.into_option().map(User::from)?,
            guild_count: r.guild_count,
            message_count: r.message_count,
            storage_bytes: r.storage_bytes,
            last_active_at: r.last_active_at.as_option().map(|ts| time(Some(ts))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_token_empty_is_none() {
        assert_eq!(next_token(String::new()), None);
        assert_eq!(next_token("abc".into()), Some("abc".into()));
    }

    #[test]
    fn overview_copies_fields() {
        let o: Overview = pb::GetOverviewResponse {
            users: 1,
            guilds: 2,
            active_users_today: 9,
            ..Default::default()
        }
        .into();
        assert_eq!((o.users, o.guilds, o.active_users_today), (1, 2, 9));
    }

    #[test]
    fn rows_require_their_entity_and_map_optional_time() {
        assert!(AdminGuildRow::from_proto(pb::AdminGuildRow::default()).is_none());
        assert!(AdminUserRow::from_proto(pb::AdminUserRow::default()).is_none());
        let row = AdminUserRow::from_proto(pb::AdminUserRow {
            user: pb::User {
                id: 3,
                is_admin: true,
                ..Default::default()
            }
            .into(),
            guild_count: 2,
            ..Default::default()
        })
        .unwrap();
        assert!(row.user.is_admin);
        assert_eq!(row.last_active_at, None);
        assert_eq!(row.guild_count, 2);
    }

    #[test]
    fn activity_point_copies() {
        let p: ActivityPoint = pb::ActivityPoint {
            date: "2026-01-02".into(),
            messages: 4,
            signups: 1,
            active_users: 2,
            ..Default::default()
        }
        .into();
        assert_eq!(p.date, "2026-01-02");
        assert_eq!(p.active_users, 2);
    }
}
