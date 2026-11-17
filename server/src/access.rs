//! Membership checks shared by the guild and message services.

use connectrpc::ConnectError;

use crate::AppState;
use crate::auth::internal;
use crate::convert::{id_from_db, id_to_db};

/// Succeeds if `user_id` belongs to `guild_id`; otherwise `NotFound` so a guild's existence is not leaked.
pub async fn require_guild_member(
    state: &AppState,
    user_id: u64,
    guild_id: u64,
) -> Result<(), ConnectError> {
    if is_member(state, user_id, guild_id).await? {
        Ok(())
    } else {
        Err(ConnectError::not_found("guild not found"))
    }
}

/// Resolves `channel_id` to its guild and checks `user_id` is a member of it.
pub async fn require_channel_member(
    state: &AppState,
    user_id: u64,
    channel_id: u64,
) -> Result<u64, ConnectError> {
    let not_found = || ConnectError::not_found("channel not found");
    let guild_id = state
        .session
        .execute_unpaged("SELECT guild_id FROM channels_by_id WHERE id = ?", (id_to_db(channel_id),))
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<(i64,)>()
        .map_err(internal)?
        .map(|(id,)| id_from_db(id))
        .ok_or_else(not_found)?;
    if is_member(state, user_id, guild_id).await? {
        Ok(guild_id)
    } else {
        Err(not_found())
    }
}

pub(crate) async fn is_member(
    state: &AppState,
    user_id: u64,
    guild_id: u64,
) -> Result<bool, ConnectError> {
    Ok(state
        .session
        .execute_unpaged(
            "SELECT user_id FROM guild_members WHERE guild_id = ? AND user_id = ?",
            (id_to_db(guild_id), id_to_db(user_id)),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<(i64,)>()
        .map_err(internal)?
        .is_some())
}
