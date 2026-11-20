//! Per-guild optimistic-concurrency counters (`guild_versions`), only ever
//! written with lightweight transactions. A missing row is version 0.

use connectrpc::ConnectError;
use scylla::value::{CqlValue, Row};

use crate::AppState;
use crate::auth::internal;
use crate::convert::id_to_db;

pub const LAYOUT: &str = "layout";
pub const ROLES: &str = "roles";

const RETRIES: usize = 16;

pub async fn get(state: &AppState, guild_id: u64, kind: &str) -> Result<u64, ConnectError> {
    Ok(state
        .session
        .execute_unpaged(
            "SELECT version FROM guild_versions WHERE guild_id = ? AND kind = ?",
            (id_to_db(guild_id), kind),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<(i64,)>()
        .map_err(internal)?
        .map_or(0, |(v,)| v as u64))
}

/// Compare-and-set: moves the counter from `current` to `current + 1`; `false` if someone else got there first.
async fn cas(state: &AppState, guild_id: u64, kind: &str, current: u64) -> Result<bool, ConnectError> {
    let guild_id = id_to_db(guild_id);
    let result = if current == 0 {
        state
            .session
            .execute_unpaged(
                "INSERT INTO guild_versions (guild_id, kind, version) VALUES (?, ?, 1) IF NOT EXISTS",
                (guild_id, kind),
            )
            .await
    } else {
        state
            .session
            .execute_unpaged(
                "UPDATE guild_versions SET version = ? WHERE guild_id = ? AND kind = ? IF version = ?",
                ((current + 1) as i64, guild_id, kind, current as i64),
            )
            .await
    };
    // An LWT answers with `[applied]` as its first column.
    let row = result
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .first_row::<Row>()
        .map_err(internal)?;
    Ok(matches!(row.columns.first(), Some(Some(CqlValue::Boolean(true)))))
}

/// Bumps the counter and returns the new version. With `expected` the bump
/// only succeeds from exactly that version (`Aborted` otherwise); without it
/// the bump retries until it wins.
pub async fn bump(
    state: &AppState,
    guild_id: u64,
    kind: &str,
    expected: Option<u64>,
) -> Result<u64, ConnectError> {
    if let Some(expected) = expected {
        return if cas(state, guild_id, kind, expected).await? {
            Ok(expected + 1)
        } else {
            Err(ConnectError::aborted("version conflict: refetch and retry"))
        };
    }
    for _ in 0..RETRIES {
        let current = get(state, guild_id, kind).await?;
        if cas(state, guild_id, kind, current).await? {
            return Ok(current + 1);
        }
    }
    Err(ConnectError::aborted("too much contention, retry"))
}
