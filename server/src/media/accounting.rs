//! Storage accounting for the admin dashboard: one row per stored object and
//! per-guild / global byte totals (counters live in their own table because
//! Scylla counters cannot share a table with regular columns).
//!
//! `media_totals` scopes: `global` and `guild:<id>` are sums over every kind;
//! `global:<label>` and `guild:<id>:<label>` break them down by kind
//! (`avatar`, `guild_icon`, `emoji`, later `attachment`, `embed`), which is
//! what the admin storage breakdown reads via [`totals_by_kind`]. Avatars
//! have no guild, so they only appear in the global scopes.

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
    /// Accounting label of the object (see `MediaKind::label`).
    pub label: &'a str,
}

/// Records a stored object and adds its size to the totals.
pub async fn record(state: &AppState, key: &str, owner: Owner<'_>, bytes: u64) -> Result<(), ConnectError> {
    state
        .session
        .execute_unpaged(
            "INSERT INTO media_objects (key, owner_kind, owner_id, guild_id, bytes, created_at, kind) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            (key, owner.kind, id_to_db(owner.id), owner.guild_id.map(id_to_db), bytes as i64, Utc::now(), owner.label),
        )
        .await
        .map_err(internal)?;
    adjust(state, owner.guild_id, owner.label, bytes as i64).await
}

/// Forgets a stored object and subtracts its size from the totals. Best
/// effort: failures are logged, not returned, since the caller has already
/// swapped in the new object.
pub async fn delete_record(state: &AppState, key: &str) {
    let result: Result<(), ConnectError> = async {
        let Some((guild_id, bytes, kind)) = state
            .session
            .execute_unpaged("SELECT guild_id, bytes, kind FROM media_objects WHERE key = ?", (key,))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(Option<i64>, Option<i64>, Option<String>)>()
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
        // Rows from before the `kind` column derive it from the key prefix.
        let label = kind
            .as_deref()
            .and_then(|k| KINDS.iter().find(|l| **l == k))
            .copied()
            .or_else(|| super::label_of_key(key))
            .unwrap_or("other");
        adjust(state, guild_id, label, -bytes.unwrap_or(0)).await
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(key, error = %e.message.as_deref().unwrap_or("?"), "media accounting update failed");
    }
}

/// Kinds with a per-kind breakdown, in the order the admin overview lists them.
pub const KINDS: [&str; 5] = ["attachment", "emoji", "guild_icon", "avatar", "embed"];

async fn adjust(state: &AppState, guild_id: Option<u64>, label: &str, delta: i64) -> Result<(), ConnectError> {
    let mut scopes = vec!["global".to_owned(), format!("global:{label}")];
    if let Some(guild_id) = guild_id {
        scopes.push(format!("guild:{guild_id}"));
        scopes.push(format!("guild:{guild_id}:{label}"));
    }
    for scope in scopes {
        state
            .session
            .execute_unpaged(
                "UPDATE media_totals SET bytes = bytes + ?, objects = objects + ? WHERE scope = ?",
                (Counter(delta), Counter(delta.signum()), scope),
            )
            .await
            .map_err(internal)?;
    }
    Ok(())
}

/// Bytes and objects of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindTotals {
    pub kind: &'static str,
    pub bytes: i64,
    pub objects: i64,
}

/// Per-kind totals (every kind in [`KINDS`], zero if none) for one guild, or
/// globally with `None`.
pub async fn totals_by_kind(state: &AppState, guild_id: Option<u64>) -> Result<Vec<KindTotals>, ConnectError> {
    let mut out = Vec::with_capacity(KINDS.len());
    for kind in KINDS {
        let scope = match guild_id {
            Some(id) => format!("guild:{id}:{kind}"),
            None => format!("global:{kind}"),
        };
        let (bytes, objects) = totals(state, &scope).await?;
        out.push(KindTotals { kind, bytes, objects });
    }
    Ok(out)
}

/// `(bytes, objects)` for a scope (`"global"`, `"guild:<id>"`, or their per-kind variants); zero if nothing was stored.
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
