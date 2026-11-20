//! Avatars and guild icons: processing, storage, persistence, events.

use std::time::Duration;

use bytes::Bytes;
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::auth::{authenticate, internal};
use crate::convert::{id_from_db, id_to_db};
use crate::events::EventPayload;
use crate::media::{self, MAX_INPUT_BYTES, MediaError, MediaKind, Owner};
use crate::permissions::{MANAGE_SERVER, require_permission};
use crate::proto::thehideout::v1::*;
use crate::services::guild::load_guild;
use crate::storage::Storage;

/// Uploads allowed per user within [`UPLOAD_WINDOW`].
pub const UPLOAD_MAX: usize = 10;
const UPLOAD_WINDOW: Duration = Duration::from_secs(60);

pub struct MediaSvc {
    state: AppState,
}

impl MediaSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn storage(&self) -> Result<&Storage, ConnectError> {
        self.state
            .storage
            .as_ref()
            .ok_or_else(|| ConnectError::unavailable("media storage is not available"))
    }

    fn throttle(&self, user_id: u64) -> Result<(), ConnectError> {
        self.state.limiter.check(&format!("upload:{user_id}"), UPLOAD_MAX, UPLOAD_WINDOW)
    }

    /// Processes and stores an image for `owner_id`, returning its key and size.
    async fn store(&self, kind: MediaKind, owner_id: u64, data: &[u8], content_type: &str) -> Result<(String, u64), ConnectError> {
        let storage = self.storage()?;
        // Reject oversized bodies before copying them.
        if data.len() > MAX_INPUT_BYTES {
            return Err(ConnectError::invalid_argument(MediaError::TooLarge.message()));
        }
        let processed = media::process(kind, data.to_vec(), content_type.to_owned())
            .await
            .map_err(|e| match e {
                MediaError::Internal(msg) => {
                    tracing::error!(error = %msg, "image processing failed");
                    ConnectError::internal("image processing failed")
                }
                other => ConnectError::invalid_argument(other.message()),
            })?;
        let key = media::make_key(kind, owner_id, &processed.bytes, processed.extension());
        let size = processed.bytes.len() as u64;
        storage
            .put(&key, Bytes::clone(&processed.bytes), processed.content_type)
            .await
            .map_err(unavailable)?;
        Ok((key, size))
    }

    /// Drops a replaced or removed object, best effort.
    async fn discard(&self, key: &str) {
        if let Some(storage) = &self.state.storage
            && let Err(e) = storage.delete(key).await
        {
            tracing::warn!(key, error = %format!("{e:#}"), "deleting old media object failed");
        }
        media::delete_record(&self.state, key).await;
    }

    pub(crate) async fn set_avatar(&self, user_id: u64, new: Option<(String, u64)>) -> Result<User, ConnectError> {
        let db_id = id_to_db(user_id);
        let old = self.current_media("SELECT avatar FROM users WHERE id = ?", db_id).await?;
        let new_key = new.as_ref().map(|(k, _)| k.as_str());
        if let Some((key, size)) = &new
            && old.as_deref() != Some(key)
        {
            let owner = Owner { kind: MediaKind::Avatar.owner_kind(), id: user_id, guild_id: None, label: MediaKind::Avatar.label() };
            media::record_with(&self.state, key, owner, *size, media::Meta { user_id: Some(user_id), ..Default::default() }).await?;
        }
        self.state
            .session
            .execute_unpaged("UPDATE users SET avatar = ? WHERE id = ?", (new_key.unwrap_or(""), db_id))
            .await
            .map_err(internal)?;
        if let Some(old) = old
            && Some(old.as_str()) != new_key
        {
            self.discard(&old).await;
        }
        let user = crate::admin::load_user(&self.state, user_id)
            .await?
            .ok_or_else(|| ConnectError::unauthenticated("user no longer exists"))?;
        self.publish_user_updated(user_id, &user).await;
        Ok(user)
    }

    pub(crate) async fn set_icon(&self, guild_id: u64, new: Option<(String, u64)>) -> Result<Guild, ConnectError> {
        let db_id = id_to_db(guild_id);
        let old = self.current_media("SELECT icon FROM guilds WHERE id = ?", db_id).await?;
        let new_key = new.as_ref().map(|(k, _)| k.as_str());
        if let Some((key, size)) = &new
            && old.as_deref() != Some(key)
        {
            let owner = Owner { kind: MediaKind::GuildIcon.owner_kind(), id: guild_id, guild_id: Some(guild_id), label: MediaKind::GuildIcon.label() };
            media::record(&self.state, key, owner, *size).await?;
        }
        self.state
            .session
            .execute_unpaged("UPDATE guilds SET icon = ? WHERE id = ?", (new_key.unwrap_or(""), db_id))
            .await
            .map_err(internal)?;
        if let Some(old) = old
            && Some(old.as_str()) != new_key
        {
            self.discard(&old).await;
        }
        let guild = load_guild(&self.state, db_id)
            .await?
            .ok_or_else(|| ConnectError::not_found("guild not found"))?;
        self.state
            .events
            .publish_to_guild(guild_id, EventPayload::GuildUpdated(guild.clone()));
        Ok(guild)
    }

    /// The media key currently stored in the single text column `query` selects.
    async fn current_media(&self, query: &'static str, db_id: i64) -> Result<Option<String>, ConnectError> {
        Ok(self
            .state
            .session
            .execute_unpaged(query, (db_id,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(Option<String>,)>()
            .map_err(internal)?
            .and_then(|(key,)| key)
            .filter(|k| !k.is_empty()))
    }

    /// Sends `user_updated` to the user and everyone sharing a guild or DM with
    /// them. Only the user themself gets `is_admin`.
    pub(crate) async fn publish_user_updated(&self, user_id: u64, user: &User) {
        let mut public = user.clone();
        public.is_admin = false;
        match self.audience(user_id).await {
            Ok(ids) => {
                for id in ids {
                    let shown = if id == user_id { user } else { &public };
                    self.state.events.notify_user_profile_updated(id, shown.clone());
                }
            }
            Err(e) => tracing::warn!(user_id, error = %e.message.as_deref().unwrap_or("?"), "user_updated audience lookup failed"),
        }
    }

    async fn audience(&self, user_id: u64) -> Result<Vec<u64>, ConnectError> {
        let db_id = id_to_db(user_id);
        let mut ids = std::collections::HashSet::from([user_id]);
        let guilds = self
            .state
            .session
            .execute_unpaged("SELECT guild_id FROM guilds_by_user WHERE user_id = ?", (db_id,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64,)>()
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (guild_id,) in guilds {
            let members = self
                .state
                .session
                .execute_unpaged("SELECT user_id FROM guild_members WHERE guild_id = ?", (guild_id,))
                .await
                .map_err(internal)?
                .into_rows_result()
                .map_err(internal)?
                .rows::<(i64,)>()
                .map_err(internal)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?;
            ids.extend(members.into_iter().map(|(m,)| id_from_db(m)));
        }
        let dms = self
            .state
            .session
            .execute_unpaged("SELECT dm_id FROM dms_by_user WHERE user_id = ?", (db_id,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64,)>()
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        for (dm_id,) in dms {
            if let Some((a, b)) = self
                .state
                .session
                .execute_unpaged("SELECT user_a, user_b FROM dm_channels WHERE id = ?", (dm_id,))
                .await
                .map_err(internal)?
                .into_rows_result()
                .map_err(internal)?
                .maybe_first_row::<(i64, i64)>()
                .map_err(internal)?
            {
                ids.extend([id_from_db(a), id_from_db(b)]);
            }
        }
        Ok(ids.into_iter().collect())
    }
}

fn unavailable(e: anyhow::Error) -> ConnectError {
    tracing::warn!(error = %format!("{e:#}"), "media storage error");
    ConnectError::unavailable("media storage is unavailable")
}

impl MediaService for MediaSvc {
    async fn upload_avatar(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, UploadAvatarRequest>,
    ) -> ServiceResult<UploadAvatarResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        self.storage()?;
        self.throttle(user_id)?;
        let stored = self.store(MediaKind::Avatar, user_id, req.data, req.content_type).await?;
        let user = self.set_avatar(user_id, Some(stored)).await?;
        Response::ok(UploadAvatarResponse { user: user.into(), ..Default::default() })
    }

    async fn remove_avatar(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, RemoveAvatarRequest>,
    ) -> ServiceResult<RemoveAvatarResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        self.throttle(user_id)?;
        let user = self.set_avatar(user_id, None).await?;
        Response::ok(RemoveAvatarResponse { user: user.into(), ..Default::default() })
    }

    async fn upload_guild_icon(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, UploadGuildIconRequest>,
    ) -> ServiceResult<UploadGuildIconResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_permission(&self.state, user_id, req.guild_id, MANAGE_SERVER).await?;
        self.storage()?;
        self.throttle(user_id)?;
        let stored = self.store(MediaKind::GuildIcon, req.guild_id, req.data, req.content_type).await?;
        let guild = self.set_icon(req.guild_id, Some(stored)).await?;
        Response::ok(UploadGuildIconResponse { guild: guild.into(), ..Default::default() })
    }

    async fn remove_guild_icon(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, RemoveGuildIconRequest>,
    ) -> ServiceResult<RemoveGuildIconResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_permission(&self.state, user_id, req.guild_id, MANAGE_SERVER).await?;
        self.throttle(user_id)?;
        let guild = self.set_icon(req.guild_id, None).await?;
        Response::ok(RemoveGuildIconResponse { guild: guild.into(), ..Default::default() })
    }
}
