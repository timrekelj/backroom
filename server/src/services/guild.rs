use std::time::Duration;

use chrono::{DateTime, Utc};
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use futures::future::try_join_all;
use rand::distr::{Alphanumeric, SampleString};
use scylla::statement::batch::{Batch, BatchType};

use crate::AppState;
use crate::access::{find_user, is_member, require_guild_member};
use crate::auth::{authenticate, internal, peer_key};
use crate::convert::{datetime_to_proto, id_from_db, id_to_db, snowflake_datetime};
use crate::events::EventPayload;
use crate::proto::thehideout::v1::*;

const GUILD_NAME_LEN: std::ops::RangeInclusive<usize> = 1..=100;
const CHANNEL_NAME_LEN: std::ops::RangeInclusive<usize> = 1..=100;
const INVITE_CODE_LEN: usize = 8;
const INVITE_CODE_ATTEMPTS: usize = 5;
const DEFAULT_CHANNEL: &str = "general";

/// Guilds one user may be a member of via creation.
pub const MAX_GUILDS_PER_USER: i64 = 100;
/// Guild creations allowed per user within [`GUILD_CREATE_WINDOW`].
pub const GUILD_CREATE_MAX: usize = 10;
const GUILD_CREATE_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Channels one guild may have.
pub const MAX_CHANNELS_PER_GUILD: i64 = 500;

/// Guild joins (invite-code lookups) allowed per peer IP within [`JOIN_WINDOW`].
pub const JOIN_IP_MAX: usize = 30;
const JOIN_WINDOW: Duration = Duration::from_secs(15 * 60);

pub struct GuildSvc {
    state: AppState,
}

impl GuildSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Trims, lowercases and joins whitespace runs with '-'; `None` unless the result matches `^[a-z0-9_-]{1,100}$`.
fn normalize_channel_name(raw: &str) -> Option<String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase();
    let valid = CHANNEL_NAME_LEN.contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    valid.then_some(name)
}

fn guild_to_proto(
    id: i64,
    name: String,
    owner_id: i64,
    invite_code: String,
    created_at: DateTime<Utc>,
) -> Guild {
    Guild {
        id: id_from_db(id),
        name,
        owner_id: id_from_db(owner_id),
        invite_code,
        created_at: datetime_to_proto(created_at).into(),
        ..Default::default()
    }
}

fn channel_to_proto(guild_id: u64, id: u64, name: String, created_at: DateTime<Utc>) -> Channel {
    Channel {
        id,
        guild_id,
        name,
        created_at: datetime_to_proto(created_at).into(),
        ..Default::default()
    }
}

impl GuildService for GuildSvc {
    async fn create_guild(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, CreateGuildRequest>,
    ) -> ServiceResult<CreateGuildResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let name = req.name.trim();
        if !GUILD_NAME_LEN.contains(&name.chars().count()) {
            return Err(ConnectError::invalid_argument("guild name must be 1-100 characters"));
        }
        self.state
            .limiter
            .check(&format!("create_guild:{user_id}"), GUILD_CREATE_MAX, GUILD_CREATE_WINDOW)?;
        self.reserve_guild_slot(user_id).await?;

        let id = self.state.snowflake.next_id();
        let db_id = id_to_db(id);
        let created_at = snowflake_datetime(id);
        let invite_code = self.claim_invite_code(db_id).await?;

        self.state
            .session
            .execute_unpaged(
                "INSERT INTO guilds (id, name, owner_id, invite_code, created_at) VALUES (?, ?, ?, ?, ?)",
                (db_id, name, id_to_db(user_id), &invite_code, created_at),
            )
            .await
            .map_err(internal)?;
        self.add_member(id, user_id).await?;
        let channel = self.insert_channel(id, DEFAULT_CHANNEL).await?;
        self.state.events.publish_to_guild(id, EventPayload::ChannelCreated(channel));
        self.state.events.notify_user_joined_guild(user_id, id);

        tracing::info!(guild_id = id, user_id, "guild created");
        Response::ok(CreateGuildResponse {
            guild: guild_to_proto(db_id, name.to_owned(), id_to_db(user_id), invite_code, created_at)
                .into(),
            ..Default::default()
        })
    }

    async fn list_my_guilds(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, ListMyGuildsRequest>,
    ) -> ServiceResult<ListMyGuildsResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let guild_ids = self
            .state
            .session
            .execute_unpaged("SELECT guild_id FROM guilds_by_user WHERE user_id = ?", (id_to_db(user_id),))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64,)>()
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;

        let mut guilds = try_join_all(guild_ids.into_iter().map(|(id,)| self.find_guild(id)))
            .await?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        guilds.sort_by_key(|g| g.id);
        Response::ok(ListMyGuildsResponse {
            guilds,
            ..Default::default()
        })
    }

    async fn join_guild(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, JoinGuildRequest>,
    ) -> ServiceResult<JoinGuildResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        self.state
            .limiter
            .check(&format!("join:ip:{}", peer_key(&ctx)), JOIN_IP_MAX, JOIN_WINDOW)?;
        let invalid = || ConnectError::not_found("invalid invite");
        let (guild_id,) = self
            .state
            .session
            .execute_unpaged(
                "SELECT guild_id FROM guilds_by_invite WHERE invite_code = ?",
                (req.invite_code.trim(),),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(i64,)>()
            .map_err(internal)?
            .ok_or_else(invalid)?;
        let guild = self.find_guild(guild_id).await?.ok_or_else(invalid)?;

        if !is_member(&self.state, user_id, guild.id).await? {
            self.add_member(guild.id, user_id).await?;
            if let Some(user) = find_user(&self.state, id_to_db(user_id)).await? {
                self.state.events.publish_to_guild(
                    guild.id,
                    EventPayload::GuildMemberJoined(GuildMemberJoined {
                        guild_id: guild.id,
                        user: user.into(),
                        ..Default::default()
                    }),
                );
            }
            self.state.events.notify_user_joined_guild(user_id, guild.id);
            tracing::info!(guild_id = guild.id, user_id, "user joined guild");
        }
        Response::ok(JoinGuildResponse {
            guild: guild.into(),
            ..Default::default()
        })
    }

    async fn list_guild_members(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, ListGuildMembersRequest>,
    ) -> ServiceResult<ListGuildMembersResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_guild_member(&self.state, user_id, req.guild_id).await?;

        let member_ids = self
            .state
            .session
            .execute_unpaged(
                "SELECT user_id FROM guild_members WHERE guild_id = ?",
                (id_to_db(req.guild_id),),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64,)>()
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;

        let mut users = try_join_all(member_ids.into_iter().map(|(id,)| find_user(&self.state, id)))
            .await?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        users.sort_by(|a, b| a.username.cmp(&b.username));
        Response::ok(ListGuildMembersResponse {
            users,
            ..Default::default()
        })
    }

    async fn create_channel(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, CreateChannelRequest>,
    ) -> ServiceResult<CreateChannelResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_guild_member(&self.state, user_id, req.guild_id).await?;
        // Only the guild owner may create channels; the schema has no roles.
        let guild = self
            .find_guild(id_to_db(req.guild_id))
            .await?
            .ok_or_else(|| ConnectError::not_found("guild not found"))?;
        if guild.owner_id != user_id {
            return Err(ConnectError::permission_denied(
                "only the guild owner can create channels",
            ));
        }
        self.reserve_channel_slot(req.guild_id).await?;
        let name = normalize_channel_name(req.name).ok_or_else(|| {
            ConnectError::invalid_argument(
                "channel name must be 1-100 characters of a-z, 0-9, '_' or '-'",
            )
        })?;

        let channel = self.insert_channel(req.guild_id, &name).await?;
        self.state
            .events
            .publish_to_guild(req.guild_id, EventPayload::ChannelCreated(channel.clone()));
        Response::ok(CreateChannelResponse {
            channel: channel.into(),
            ..Default::default()
        })
    }

    async fn list_channels(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, ListChannelsRequest>,
    ) -> ServiceResult<ListChannelsResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_guild_member(&self.state, user_id, req.guild_id).await?;

        let guild_id = req.guild_id;
        let channels = self
            .state
            .session
            .execute_unpaged(
                "SELECT id, name, created_at FROM channels WHERE guild_id = ? ORDER BY id ASC",
                (id_to_db(guild_id),),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64, String, DateTime<Utc>)>()
            .map_err(internal)?
            .map(|row| {
                row.map(|(id, name, created_at)| {
                    channel_to_proto(guild_id, id_from_db(id), name, created_at)
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        Response::ok(ListChannelsResponse {
            channels,
            ..Default::default()
        })
    }
}

impl GuildSvc {
    /// Reserves an unused invite code for `guild_db_id`, retrying on collisions.
    async fn claim_invite_code(&self, guild_db_id: i64) -> Result<String, ConnectError> {
        for _ in 0..INVITE_CODE_ATTEMPTS {
            let code = Alphanumeric.sample_string(&mut rand::rng(), INVITE_CODE_LEN);
            // An LWT returns `[applied]` plus the existing row's columns when not applied.
            let (claimed, _, _) = self
                .state
                .session
                .execute_unpaged(
                    "INSERT INTO guilds_by_invite (invite_code, guild_id) VALUES (?, ?) IF NOT EXISTS",
                    (&code, guild_db_id),
                )
                .await
                .map_err(internal)?
                .into_rows_result()
                .map_err(internal)?
                .first_row::<(bool, Option<String>, Option<i64>)>()
                .map_err(internal)?;
            if claimed {
                return Ok(code);
            }
        }
        Err(internal("could not allocate a unique invite code"))
    }

    /// Writes both membership tables in one logged batch so they stay consistent.
    async fn add_member(&self, guild_id: u64, user_id: u64) -> Result<(), ConnectError> {
        let (guild_id, user_id) = (id_to_db(guild_id), id_to_db(user_id));
        let joined_at = Utc::now();
        let mut batch = Batch::new(BatchType::Logged);
        batch.append_statement("INSERT INTO guild_members (guild_id, user_id, joined_at) VALUES (?, ?, ?)");
        batch.append_statement("INSERT INTO guilds_by_user (user_id, guild_id, joined_at) VALUES (?, ?, ?)");
        self.state
            .session
            .batch(
                &batch,
                ((guild_id, user_id, joined_at), (user_id, guild_id, joined_at)),
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    /// Writes `channels` and `channels_by_id` in one logged batch.
    async fn insert_channel(&self, guild_id: u64, name: &str) -> Result<Channel, ConnectError> {
        let id = self.state.snowflake.next_id();
        let created_at = snowflake_datetime(id);
        let (db_guild_id, db_id) = (id_to_db(guild_id), id_to_db(id));
        let mut batch = Batch::new(BatchType::Logged);
        batch.append_statement("INSERT INTO channels (guild_id, id, name, created_at) VALUES (?, ?, ?, ?)");
        batch.append_statement("INSERT INTO channels_by_id (id, guild_id) VALUES (?, ?)");
        self.state
            .session
            .batch(&batch, ((db_guild_id, db_id, name, created_at), (db_id, db_guild_id)))
            .await
            .map_err(internal)?;
        Ok(channel_to_proto(guild_id, id, name.to_owned(), created_at))
    }

    /// Atomically reserves one guild-creation slot for `user_id` via the
    /// `user_guild_quota` counter and rejects past [`MAX_GUILDS_PER_USER`]. The
    /// increment is atomic, so concurrent creates cannot all pass the cap; a
    /// rejected create compensates its increment.
    async fn reserve_guild_slot(&self, user_id: u64) -> Result<(), ConnectError> {
        let key = id_to_db(user_id);
        self.state
            .session
            .execute_unpaged(
                "UPDATE user_guild_quota SET created = created + 1 WHERE user_id = ?",
                (key,),
            )
            .await
            .map_err(internal)?;
        let (created,): (scylla::value::Counter,) = self
            .state
            .session
            .execute_unpaged("SELECT created FROM user_guild_quota WHERE user_id = ?", (key,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .first_row::<(scylla::value::Counter,)>()
            .map_err(internal)?;
        if created.0 > MAX_GUILDS_PER_USER {
            let _ = self
                .state
                .session
                .execute_unpaged(
                    "UPDATE user_guild_quota SET created = created - 1 WHERE user_id = ?",
                    (key,),
                )
                .await;
            return Err(ConnectError::resource_exhausted("guild limit reached"));
        }
        Ok(())
    }

    /// Atomically reserves one channel-creation slot for `guild_id` via the
    /// `guild_channel_quota` counter and rejects past [`MAX_CHANNELS_PER_GUILD`].
    async fn reserve_channel_slot(&self, guild_id: u64) -> Result<(), ConnectError> {
        let key = id_to_db(guild_id);
        self.state
            .session
            .execute_unpaged(
                "UPDATE guild_channel_quota SET created = created + 1 WHERE guild_id = ?",
                (key,),
            )
            .await
            .map_err(internal)?;
        let (created,): (scylla::value::Counter,) = self
            .state
            .session
            .execute_unpaged("SELECT created FROM guild_channel_quota WHERE guild_id = ?", (key,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .first_row::<(scylla::value::Counter,)>()
            .map_err(internal)?;
        if created.0 > MAX_CHANNELS_PER_GUILD {
            let _ = self
                .state
                .session
                .execute_unpaged(
                    "UPDATE guild_channel_quota SET created = created - 1 WHERE guild_id = ?",
                    (key,),
                )
                .await;
            return Err(ConnectError::resource_exhausted("channel limit reached"));
        }
        Ok(())
    }

    async fn find_guild(&self, db_id: i64) -> Result<Option<Guild>, ConnectError> {
        Ok(self
            .state
            .session
            .execute_unpaged(
                "SELECT name, owner_id, invite_code, created_at FROM guilds WHERE id = ?",
                (db_id,),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(String, i64, String, DateTime<Utc>)>()
            .map_err(internal)?
            .map(|(name, owner_id, invite_code, created_at)| {
                guild_to_proto(db_id, name, owner_id, invite_code, created_at)
            }))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_normalized() {
        assert_eq!(normalize_channel_name("  My   Channel ").as_deref(), Some("my-channel"));
        assert_eq!(normalize_channel_name("a_b-1").as_deref(), Some("a_b-1"));
        assert_eq!(normalize_channel_name(""), None);
        assert_eq!(normalize_channel_name("   "), None);
        assert_eq!(normalize_channel_name("bad!"), None);
        assert_eq!(normalize_channel_name("émile"), None);
        assert_eq!(normalize_channel_name(&"a".repeat(101)), None);
        assert!(normalize_channel_name(&"a".repeat(100)).is_some());
    }
}
