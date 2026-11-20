//! Storage accounting for the admin dashboard: one row per stored object and
//! per-guild / global byte totals (counters live in their own table because
//! Scylla counters cannot share a table with regular columns).
//!
//! `media_totals` scopes: `global` and `guild:<id>` are sums over every kind;
//! `global:<label>` and `guild:<id>:<label>` break them down by kind
//! (`avatar`, `guild_icon`, `emoji`, later `attachment`, `embed`), which is
//! what the admin storage breakdown reads via [`totals_by_kind`]. Avatars
//! have no guild, so they only appear in the global scopes.
//!
//! Every object is also written to `files_by_size` under the same scopes (see
//! [`size_bucket`]), the index behind the admin file list, and carries where it
//! belongs ([`Meta`]) on its `media_objects` row.

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

/// Where an object belongs, beyond its [`Owner`]: what the admin file list
/// shows. Everything is optional; unset fields are stored as null.
#[derive(Debug, Clone, Copy, Default)]
pub struct Meta<'a> {
    /// Original file name (attachments).
    pub filename: Option<&'a str>,
    /// MIME type; derived from the key's extension when unset.
    pub content_type: Option<&'a str>,
    /// Channel and message of an attachment or embed.
    pub channel_id: Option<u64>,
    pub message_id: Option<u64>,
    /// The user the object is attributed to: avatar owner, attachment or embed
    /// author, emoji creator.
    pub user_id: Option<u64>,
}

/// Records a stored object and adds its size to the totals.
pub async fn record(state: &AppState, key: &str, owner: Owner<'_>, bytes: u64) -> Result<(), ConnectError> {
    record_with(state, key, owner, bytes, Meta::default()).await
}

/// Like [`record`], with the object's [`Meta`].
pub async fn record_with(state: &AppState, key: &str, owner: Owner<'_>, bytes: u64, meta: Meta<'_>) -> Result<(), ConnectError> {
    state
        .session
        .execute_unpaged(
            "INSERT INTO media_objects (key, owner_kind, owner_id, guild_id, bytes, created_at, kind, filename, \
             content_type, channel_id, message_id, user_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                key,
                owner.kind,
                id_to_db(owner.id),
                owner.guild_id.map(id_to_db),
                bytes as i64,
                Utc::now(),
                owner.label,
                meta.filename,
                meta.content_type.unwrap_or_else(|| super::content_type_of_key(key)),
                meta.channel_id.map(id_to_db),
                meta.message_id.map(id_to_db),
                meta.user_id.map(id_to_db),
            ),
        )
        .await
        .map_err(internal)?;
    index(state, owner.guild_id, owner.label, bytes as i64, key, true).await?;
    adjust(state, owner.guild_id, owner.label, bytes as i64).await
}

/// Highest `files_by_size` bucket (2^40 bytes = 1 TiB and up share it).
pub const MAX_SIZE_BUCKET: i32 = 40;

/// The `files_by_size` bucket of an object: floor(log2(bytes)), capped, 0 for empty.
pub fn size_bucket(bytes: i64) -> i32 {
    ((63 - (bytes.max(1) as u64).leading_zeros()) as i32).min(MAX_SIZE_BUCKET)
}

/// The `files_by_size` scopes an object appears in.
pub fn index_scopes(guild_id: Option<u64>, label: &str) -> Vec<String> {
    let mut scopes = vec!["global".to_owned(), format!("global:{label}")];
    if let Some(guild_id) = guild_id {
        scopes.push(format!("guild:{guild_id}"));
        scopes.push(format!("guild:{guild_id}:{label}"));
    }
    scopes
}

/// Adds (`present`) or removes an object in the size index.
pub async fn index(state: &AppState, guild_id: Option<u64>, label: &str, bytes: i64, key: &str, present: bool) -> Result<(), ConnectError> {
    let query = if present {
        "INSERT INTO files_by_size (scope, bucket, bytes, key) VALUES (?, ?, ?, ?)"
    } else {
        "DELETE FROM files_by_size WHERE scope = ? AND bucket = ? AND bytes = ? AND key = ?"
    };
    for scope in index_scopes(guild_id, label) {
        state
            .session
            .execute_unpaged(query, (scope, size_bucket(bytes), bytes, key))
            .await
            .map_err(internal)?;
    }
    Ok(())
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
        let label = label_of_row(key, kind.as_deref());
        index(state, guild_id, label, bytes.unwrap_or(0), key, false).await?;
        adjust(state, guild_id, label, -bytes.unwrap_or(0)).await
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(key, error = %e.message.as_deref().unwrap_or("?"), "media accounting update failed");
    }
}

/// Accounting label of an object row: its `kind` column, or (rows from before
/// the column existed) the key's prefix.
pub fn label_of_row(key: &str, kind: Option<&str>) -> &'static str {
    kind.and_then(|k| KINDS.iter().find(|l| **l == k))
        .copied()
        .or_else(|| super::label_of_key(key))
        .unwrap_or("other")
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
