//! The size-ordered file index (`files_by_size`) and the object details the
//! admin file list shows. Written by `accounting` wherever an object is
//! recorded or forgotten; read here.

use chrono::{DateTime, Utc};
use connectrpc::ConnectError;

use super::accounting::{MAX_SIZE_BUCKET, index, label_of_row, size_bucket};
use crate::AppState;
use crate::auth::internal;
use crate::convert::id_from_db;

/// Where a listing resumes: the last row of the previous page (size, key).
pub type Cursor = (i64, String);

/// Up to `limit` `(bytes, key)` rows of `scope` (a `files_by_size` scope),
/// largest first with ties by key, strictly after `after`. Walks the size
/// buckets from the one `after` is in (or the top) downwards.
pub async fn page(state: &AppState, scope: &str, after: Option<&Cursor>, limit: usize) -> Result<Vec<(i64, String)>, ConnectError> {
    let mut out: Vec<(i64, String)> = Vec::new();
    let start = after.map_or(MAX_SIZE_BUCKET, |(bytes, _)| size_bucket(*bytes));
    for bucket in (0..=start).rev() {
        let want = (limit - out.len()) as i32;
        if let (Some((bytes, key)), true) = (after, bucket == start) {
            out.extend(
                query(
                    state,
                    "SELECT bytes, key FROM files_by_size WHERE scope = ? AND bucket = ? AND bytes = ? AND key > ? LIMIT ?",
                    (scope, bucket, *bytes, key.as_str(), want),
                )
                .await?,
            );
            let want = (limit - out.len()) as i32;
            if want > 0 {
                out.extend(
                    query(
                        state,
                        "SELECT bytes, key FROM files_by_size WHERE scope = ? AND bucket = ? AND bytes < ? LIMIT ?",
                        (scope, bucket, *bytes, want),
                    )
                    .await?,
                );
            }
        } else {
            out.extend(
                query(state, "SELECT bytes, key FROM files_by_size WHERE scope = ? AND bucket = ? LIMIT ?", (scope, bucket, want)).await?,
            );
        }
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

async fn query(state: &AppState, cql: &'static str, values: impl scylla::serialize::row::SerializeRow) -> Result<Vec<(i64, String)>, ConnectError> {
    state
        .session
        .execute_unpaged(cql, values)
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .rows::<(i64, String)>()
        .map_err(internal)?
        .collect::<Result<_, _>>()
        .map_err(internal)
}

/// What the `media_objects` row of a key says.
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: String,
    /// Accounting label (`attachment`, `emoji`, `guild_icon`, `avatar`, `embed`).
    pub label: &'static str,
    pub filename: String,
    pub content_type: String,
    pub bytes: i64,
    pub guild_id: Option<u64>,
    /// `"user"` or `"guild"`, and its id (see `Owner`).
    pub owner_kind: String,
    pub owner_id: u64,
    pub channel_id: Option<u64>,
    pub message_id: Option<u64>,
    pub user_id: Option<u64>,
    pub created_at: DateTime<Utc>,
}

/// The entry of `key`, if it is accounted.
pub async fn entry(state: &AppState, key: &str) -> Result<Option<Entry>, ConnectError> {
    type Flat = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<DateTime<Utc>>,
        Option<i64>,
    );
    let row = state
        .session
        .execute_unpaged(
            "SELECT kind, filename, content_type, bytes, guild_id, channel_id, message_id, user_id, owner_kind, created_at, owner_id \
             FROM media_objects WHERE key = ?",
            (key,),
        )
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .maybe_first_row::<Flat>()
        .map_err(internal)?;
    Ok(row.map(
        |(kind, filename, content_type, bytes, guild_id, channel_id, message_id, user_id, owner_kind, created_at, owner_id)| Entry {
            key: key.to_owned(),
            label: label_of_row(key, kind.as_deref()),
            filename: filename.unwrap_or_default(),
            content_type: content_type.unwrap_or_else(|| super::content_type_of_key(key).to_owned()),
            bytes: bytes.unwrap_or(0),
            guild_id: guild_id.map(id_from_db),
            owner_kind: owner_kind.unwrap_or_default(),
            owner_id: owner_id.map(id_from_db).unwrap_or(0),
            channel_id: channel_id.map(id_from_db),
            message_id: message_id.map(id_from_db),
            user_id: user_id.map(id_from_db),
            created_at: created_at.unwrap_or_default(),
        },
    ))
}

/// The entries of `keys` that are still accounted, in order.
pub async fn entries(state: &AppState, keys: &[String]) -> Result<Vec<Entry>, ConnectError> {
    let mut found: Vec<Option<Entry>> = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(16) {
        found.extend(futures::future::try_join_all(chunk.iter().map(|key| entry(state, key))).await?);
    }
    Ok(found.into_iter().flatten().collect())
}

/// Fills `files_by_size` from `media_objects` if the index is empty, as after
/// upgrading a server that already had stored files. Returns the rows written.
pub async fn backfill_index(state: &AppState) -> Result<usize, ConnectError> {
    let empty = state
        .session
        .execute_unpaged("SELECT key FROM files_by_size LIMIT 1", ())
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .rows_num()
        == 0;
    if !empty {
        return Ok(0);
    }
    type ObjectRow = (String, Option<i64>, Option<i64>, Option<String>);
    let rows: Vec<ObjectRow> = state
        .session
        .execute_unpaged("SELECT key, guild_id, bytes, kind FROM media_objects", ())
        .await
        .map_err(internal)?
        .into_rows_result()
        .map_err(internal)?
        .rows::<ObjectRow>()
        .map_err(internal)?
        .collect::<Result<_, _>>()
        .map_err(internal)?;
    let count = rows.len();
    for (key, guild_id, bytes, kind) in rows {
        let label = label_of_row(&key, kind.as_deref());
        index(state, guild_id.map(id_from_db), label, bytes.unwrap_or(0), &key, true).await?;
    }
    if count > 0 {
        tracing::info!(count, "backfilled the file size index from media_objects");
    }
    Ok(count)
}
