//! Storage accounting for the admin dashboard: one row per stored object and
//! per-guild / global byte totals (counters live in their own table because
//! Scylla counters cannot share a table with regular columns).

use chrono::Utc;
use connectrpc::ConnectError;
use scylla::value::Counter;

use crate::AppState;
use crate::auth::internal;
use crate::convert::id_to_db;

/// Whose quota an object counts against.
#[derive(Debug, Clone, Copy)]
pub struct Owner<'a> {
    /// `"user"` or `"guild"` (see `MediaKind::owner_kind`).
    pub kind: &'a str,
    pub id: u64,
    /// Guild the object belongs to, if any (counted in the per-guild total).
    pub guild_id: Option<u64>,
}

/// Records a stored object and adds its size to the totals.
pub async fn record(state: &AppState, key: &str, owner: Owner<'_>, bytes: u64) -> Result<(), ConnectError> {
    state
        .session
        .execute_unpaged(
            "INSERT INTO media_objects (key, owner_kind, owner_id, guild_id, bytes, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            (key, owner.kind, id_to_db(owner.id), owner.guild_id.map(id_to_db), bytes as i64, Utc::now()),
        )
        .await
        .map_err(internal)?;
    adjust(state, owner.guild_id, bytes as i64).await
}

/// Forgets a stored object and subtracts its size from the totals. Best
/// effort: failures are logged, not returned, since the caller has already
/// swapped in the new object.
pub async fn delete_record(state: &AppState, key: &str) {
    let result: Result<(), ConnectError> = async {
        let Some((guild_id, bytes)) = state
            .session
            .execute_unpaged("SELECT guild_id, bytes FROM media_objects WHERE key = ?", (key,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(Option<i64>, Option<i64>)>()
            .map_err(internal)?
        else {
            return Ok(());
        };
        state
            .session
            .execute_unpaged("DELETE FROM media_objects WHERE key = ?", (key,))
            .await
            .map_err(internal)?;
        let guild_id = guild_id.map(crate::convert::id_from_db);
        adjust(state, guild_id, -bytes.unwrap_or(0)).await
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(key, error = %e.message.as_deref().unwrap_or("?"), "media accounting update failed");
    }
}

async fn adjust(state: &AppState, guild_id: Option<u64>, delta: i64) -> Result<(), ConnectError> {
    state
        .session
        .execute_unpaged(
            "UPDATE media_totals SET bytes = bytes + ?, objects = objects + ? WHERE scope = ?",
            (Counter(delta), Counter(delta.signum()), "global"),
        )
        .await
        .map_err(internal)?;
    if let Some(guild_id) = guild_id {
        state
            .session
            .execute_unpaged(
                "UPDATE media_totals SET bytes = bytes + ?, objects = objects + ? WHERE scope = ?",
                (Counter(delta), Counter(delta.signum()), format!("guild:{guild_id}")),
            )
            .await
            .map_err(internal)?;
    }
    Ok(())
}

/// `(bytes, objects)` for `"global"` or `"guild:<id>"`; zero if nothing was stored.
pub async fn totals(state: &AppState, scope: &str) -> Result<(i64, i64), ConnectError> {
    Ok(state
        .session
        .execute_unpaged("SELECT bytes, objects FROM media_totals WHERE scope = ?", (scope,))
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<(Option<Counter>, Option<Counter>)>()
        .map_err(internal)?
        .map_or((0, 0), |(b, o)| (b.map_or(0, |c| c.0), o.map_or(0, |c| c.0))))
}
