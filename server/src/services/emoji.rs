//! Custom guild emojis: processing, storage, persistence, events.
//!
//! Consistency model (Scylla has no multi-table transactions):
//! - Names are unique per guild through `emoji_names`, a claim table written
//!   with `INSERT ... IF NOT EXISTS` (LWT); the claim is taken before the
//!   emoji row and released on every failure path, on rename and on delete.
//! - The 200-emoji cap is checked up front (cheap rejection) and again right
//!   after the insert: a creator that then sees more than 200 emojis rolls
//!   itself back. Whoever checks last sees every surviving row, so the cap is
//!   never exceeded; the price is that creators racing exactly at the cap may
//!   all back off (RESOURCE_EXHAUSTED) and have to retry.
//! - Image objects are content-addressed *with the emoji id mixed in*, so two
//!   emojis never share an object and deleting one cannot break another.
//! - Object deletion and accounting are best effort (logged on failure).

use std::time::Duration;

use bytes::Bytes;
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use scylla::value::{CqlValue, Row};

use crate::AppState;
use crate::access::require_guild_member;
use crate::auth::{authenticate, internal};
use crate::convert::{datetime_to_proto, id_from_db, id_to_db, snowflake_datetime};
use crate::events::EventPayload;
use crate::layout::{self, IconRef};
use crate::media::{self, MediaError, MediaKind, Owner};
use crate::permissions::{MANAGE_EMOJIS, require_permission};
use crate::proto::thehideout::v1::*;
use crate::storage::Storage;
use crate::validation::validate_emoji_name;

/// Emojis one guild may have.
pub const MAX_EMOJIS_PER_GUILD: usize = 200;
/// Emoji creations allowed per user within [`CREATE_WINDOW`].
pub const EMOJI_CREATE_MAX: usize = 20;
const CREATE_WINDOW: Duration = Duration::from_secs(60);
/// Renames and deletes allowed per user within [`CREATE_WINDOW`].
pub const EMOJI_EDIT_MAX: usize = 60;

pub struct EmojiSvc {
    state: AppState,
}

impl EmojiSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

type EmojiRow = (i64, Option<String>, Option<bool>, Option<String>, Option<i64>);
const EMOJI_COLUMNS: &str = "id, name, animated, image, creator_id";

fn row_to_proto(guild_id: u64, (id, name, animated, image, creator_id): EmojiRow) -> Emoji {
    let id = id_from_db(id);
    Emoji {
        id,
        guild_id,
        name: name.unwrap_or_default(),
        animated: animated.unwrap_or(false),
        image: image.unwrap_or_default(),
        creator_id: creator_id.map_or(0, id_from_db),
        created_at: datetime_to_proto(snowflake_datetime(id)).into(),
        ..Default::default()
    }
}

/// All emojis of the guild, ordered by name.
pub async fn list_emojis(state: &AppState, guild_id: u64) -> Result<Vec<Emoji>, ConnectError> {
    let mut emojis = state
        .session
        .execute_unpaged(
            format!("SELECT {EMOJI_COLUMNS} FROM emojis WHERE guild_id = ?"),
            (id_to_db(guild_id),),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .rows::<EmojiRow>()
        .map_err(internal)?
        .map(|r| r.map(|row| row_to_proto(guild_id, row)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    emojis.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(emojis)
}

async fn find_emoji(state: &AppState, guild_id: u64, emoji_id: u64) -> Result<Option<Emoji>, ConnectError> {
    Ok(state
        .session
        .execute_unpaged(
            format!("SELECT {EMOJI_COLUMNS} FROM emojis WHERE guild_id = ? AND id = ?"),
            (id_to_db(guild_id), id_to_db(emoji_id)),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<EmojiRow>()
        .map_err(internal)?
        .map(|row| row_to_proto(guild_id, row)))
}

/// Whether `emoji_id` is an emoji of `guild_id`.
pub async fn emoji_exists(state: &AppState, guild_id: u64, emoji_id: u64) -> Result<bool, ConnectError> {
    Ok(state
        .session
        .execute_unpaged(
            "SELECT id FROM emojis WHERE guild_id = ? AND id = ?",
            (id_to_db(guild_id), id_to_db(emoji_id)),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<(i64,)>()
        .map_err(internal)?
        .is_some())
}

/// Number of emojis the guild has.
async fn count_emojis(state: &AppState, guild_id: u64) -> Result<usize, ConnectError> {
    Ok(state
        .session
        .execute_unpaged("SELECT id FROM emojis WHERE guild_id = ?", (id_to_db(guild_id),))
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .rows_num())
}

/// Tries to claim `name` for `emoji_id`; `false` if another emoji holds it.
async fn claim_name(state: &AppState, guild_id: u64, name: &str, emoji_id: u64) -> Result<bool, ConnectError> {
    let row = state
        .session
        .execute_unpaged(
            "INSERT INTO emoji_names (guild_id, name, emoji_id) VALUES (?, ?, ?) IF NOT EXISTS",
            (id_to_db(guild_id), name, id_to_db(emoji_id)),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .first_row::<Row>()
        .map_err(internal)?;
    Ok(matches!(row.columns.first(), Some(Some(CqlValue::Boolean(true)))))
}

/// Releases `name` if `emoji_id` still holds it. Best effort.
async fn release_name(state: &AppState, guild_id: u64, name: &str, emoji_id: u64) {
    if let Err(e) = state
        .session
        .execute_unpaged(
            "DELETE FROM emoji_names WHERE guild_id = ? AND name = ? IF emoji_id = ?",
            (id_to_db(guild_id), name, id_to_db(emoji_id)),
        )
        .await
    {
        tracing::warn!(guild_id, name, error = %e, "releasing emoji name failed");
    }
}

async fn publish_list(state: &AppState, guild_id: u64) -> Result<Vec<Emoji>, ConnectError> {
    let emojis = list_emojis(state, guild_id).await?;
    state.events.publish_to_guild(
        guild_id,
        EventPayload::EmojisUpdated(EmojisUpdated {
            guild_id,
            emojis: emojis.clone(),
            ..Default::default()
        }),
    );
    Ok(emojis)
}

impl EmojiSvc {
    fn storage(&self) -> Result<&Storage, ConnectError> {
        self.state
            .storage
            .as_ref()
            .ok_or_else(|| ConnectError::unavailable("media storage is not available"))
    }

    fn throttle_edit(&self, user_id: u64) -> Result<(), ConnectError> {
        self.state
            .limiter
            .check(&format!("emoji_edit:{user_id}"), EMOJI_EDIT_MAX, CREATE_WINDOW)
    }

    /// Removes everything a half-created emoji left behind. Best effort.
    async fn roll_back(&self, guild_id: u64, id: u64, name: &str, key: Option<&str>, row_inserted: bool) {
        if row_inserted
            && let Err(e) = self
                .state
                .session
                .execute_unpaged("DELETE FROM emojis WHERE guild_id = ? AND id = ?", (id_to_db(guild_id), id_to_db(id)))
                .await
        {
            tracing::warn!(guild_id, id, error = %e, "rolling back emoji row failed");
        }
        release_name(&self.state, guild_id, name, id).await;
        if let Some(key) = key {
            self.discard(key).await;
        }
    }

    /// Drops a stored object and its accounting, best effort.
    async fn discard(&self, key: &str) {
        if let Some(storage) = &self.state.storage
            && let Err(e) = storage.delete(key).await
        {
            tracing::warn!(key, error = %format!("{e:#}"), "deleting emoji object failed");
        }
        media::delete_record(&self.state, key).await;
    }

    /// Resets channels that use the deleted emoji as icon to the default and publishes `channel_updated`.
    async fn reset_channel_icons(&self, guild_id: u64, emoji_id: u64) -> Result<(), ConnectError> {
        let data = layout::load(&self.state, guild_id).await?;
        let affected: Vec<u64> = data
            .channels
            .iter()
            .filter(|c| c.icon == Some(IconRef::Custom(emoji_id)))
            .map(|c| c.id)
            .collect();
        if affected.is_empty() {
            return Ok(());
        }
        let (kind, value) = IconRef::columns_of(None);
        for id in &affected {
            self.state
                .session
                .execute_unpaged(
                    "UPDATE channels SET icon = '', icon_kind = ?, icon_value = ? WHERE guild_id = ? AND id = ?",
                    (kind, &value, id_to_db(guild_id), id_to_db(*id)),
                )
                .await
                .map_err(internal)?;
        }
        let data = layout::load(&self.state, guild_id).await?;
        for channel in layout::build(guild_id, &data, 0).channels.into_iter().filter(|c| affected.contains(&c.id)) {
            self.state.events.publish_to_guild(guild_id, EventPayload::ChannelUpdated(channel));
        }
        Ok(())
    }
}

impl EmojiService for EmojiSvc {
    async fn list_emojis(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, ListEmojisRequest>,
    ) -> ServiceResult<ListEmojisResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_guild_member(&self.state, user_id, req.guild_id).await?;
        let emojis = list_emojis(&self.state, req.guild_id).await?;
        Response::ok(ListEmojisResponse {
            emojis,
            ..Default::default()
        })
    }

    async fn create_emoji(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, CreateEmojiRequest>,
    ) -> ServiceResult<CreateEmojiResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let guild_id = req.guild_id;
        require_permission(&self.state, user_id, guild_id, MANAGE_EMOJIS).await?;
        let storage = self.storage()?;
        self.state
            .limiter
            .check(&format!("emoji_create:{user_id}"), EMOJI_CREATE_MAX, CREATE_WINDOW)?;
        validate_emoji_name(req.name)?;
        let cap_reached = || ConnectError::resource_exhausted("emoji limit reached (200 per guild)");
        if count_emojis(&self.state, guild_id).await? >= MAX_EMOJIS_PER_GUILD {
            return Err(cap_reached());
        }

        let kind = MediaKind::Emoji;
        if req.data.len() > kind.max_input_bytes() {
            return Err(ConnectError::invalid_argument(MediaError::TooLarge.message()));
        }
        let processed = media::process(kind, req.data.to_vec(), req.content_type.to_owned())
            .await
            .map_err(|e| match e {
                MediaError::Internal(msg) => {
                    tracing::error!(error = %msg, "image processing failed");
                    ConnectError::internal("image processing failed")
                }
                other => ConnectError::invalid_argument(other.message()),
            })?;

        let id = self.state.snowflake.next_id();
        if !claim_name(&self.state, guild_id, req.name, id).await? {
            return Err(ConnectError::already_exists("an emoji with this name already exists"));
        }
        // Mix the id in so equal images of different emojis get different objects.
        let mut salted = processed.bytes.to_vec();
        salted.extend_from_slice(&id.to_be_bytes());
        let key = media::make_key(kind, guild_id, &salted, processed.extension());
        let size = processed.bytes.len() as u64;
        if let Err(e) = storage.put(&key, Bytes::clone(&processed.bytes), processed.content_type).await {
            tracing::warn!(error = %format!("{e:#}"), "media storage error");
            self.roll_back(guild_id, id, req.name, None, false).await;
            return Err(ConnectError::unavailable("media storage is unavailable"));
        }

        let inserted = self
            .state
            .session
            .execute_unpaged(
                "INSERT INTO emojis (guild_id, id, name, animated, image, creator_id) VALUES (?, ?, ?, ?, ?, ?)",
                (id_to_db(guild_id), id_to_db(id), req.name, processed.animated, &key, id_to_db(user_id)),
            )
            .await;
        if let Err(e) = inserted {
            self.roll_back(guild_id, id, req.name, Some(&key), false).await;
            return Err(internal(e));
        }
        // Post-insert check, see the module docs.
        match count_emojis(&self.state, guild_id).await {
            Ok(total) if total <= MAX_EMOJIS_PER_GUILD => {}
            Ok(_) => {
                self.roll_back(guild_id, id, req.name, Some(&key), true).await;
                return Err(cap_reached());
            }
            Err(e) => {
                self.roll_back(guild_id, id, req.name, Some(&key), true).await;
                return Err(e);
            }
        }
        let owner = Owner {
            kind: kind.owner_kind(),
            id: guild_id,
            guild_id: Some(guild_id),
            label: kind.label(),
        };
        if let Err(e) = media::record(&self.state, &key, owner, size).await {
            self.roll_back(guild_id, id, req.name, Some(&key), true).await;
            return Err(e);
        }

        let emoji = find_emoji(&self.state, guild_id, id)
            .await?
            .ok_or_else(|| internal("emoji vanished after insert"))?;
        publish_list(&self.state, guild_id).await?;
        tracing::info!(guild_id, emoji_id = id, "emoji created");
        Response::ok(CreateEmojiResponse {
            emoji: emoji.into(),
            ..Default::default()
        })
    }

    async fn rename_emoji(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, RenameEmojiRequest>,
    ) -> ServiceResult<RenameEmojiResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let guild_id = req.guild_id;
        require_permission(&self.state, user_id, guild_id, MANAGE_EMOJIS).await?;
        self.throttle_edit(user_id)?;
        validate_emoji_name(req.name)?;
        let current = find_emoji(&self.state, guild_id, req.emoji_id)
            .await?
            .ok_or_else(|| ConnectError::not_found("emoji not found"))?;
        if current.name == req.name {
            return Response::ok(RenameEmojiResponse {
                emoji: current.into(),
                ..Default::default()
            });
        }
        if !claim_name(&self.state, guild_id, req.name, req.emoji_id).await? {
            return Err(ConnectError::already_exists("an emoji with this name already exists"));
        }
        if let Err(e) = self
            .state
            .session
            .execute_unpaged(
                "UPDATE emojis SET name = ? WHERE guild_id = ? AND id = ? IF EXISTS",
                (req.name, id_to_db(guild_id), id_to_db(req.emoji_id)),
            )
            .await
        {
            release_name(&self.state, guild_id, req.name, req.emoji_id).await;
            return Err(internal(e));
        }
        release_name(&self.state, guild_id, &current.name, req.emoji_id).await;
        let emoji = find_emoji(&self.state, guild_id, req.emoji_id)
            .await?
            .ok_or_else(|| ConnectError::not_found("emoji not found"))?;
        publish_list(&self.state, guild_id).await?;
        Response::ok(RenameEmojiResponse {
            emoji: emoji.into(),
            ..Default::default()
        })
    }

    async fn delete_emoji(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, DeleteEmojiRequest>,
    ) -> ServiceResult<DeleteEmojiResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let guild_id = req.guild_id;
        require_permission(&self.state, user_id, guild_id, MANAGE_EMOJIS).await?;
        self.throttle_edit(user_id)?;
        let emoji = find_emoji(&self.state, guild_id, req.emoji_id)
            .await?
            .ok_or_else(|| ConnectError::not_found("emoji not found"))?;
        self.state
            .session
            .execute_unpaged(
                "DELETE FROM emojis WHERE guild_id = ? AND id = ?",
                (id_to_db(guild_id), id_to_db(req.emoji_id)),
            )
            .await
            .map_err(internal)?;
        release_name(&self.state, guild_id, &emoji.name, req.emoji_id).await;
        if !emoji.image.is_empty() {
            self.discard(&emoji.image).await;
        }
        // Channels using the emoji as their icon fall back to the default
        // icon (and clients are told); reactions and message tokens keep the
        // id and render as text, as documented for DeleteEmoji.
        self.reset_channel_icons(guild_id, req.emoji_id).await?;
        publish_list(&self.state, guild_id).await?;
        Response::ok(DeleteEmojiResponse::default())
    }
}
