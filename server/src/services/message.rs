use chrono::{DateTime, Utc};
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::access::require_channel_member;
use crate::auth::{authenticate, internal};
use crate::convert::{datetime_to_proto, id_from_db, id_to_db, snowflake_datetime};
use crate::events::EventPayload;
use crate::proto::thehideout::v1::*;
use crate::snowflake::{bucket_of, current_bucket};

const MAX_CONTENT_CHARS: usize = 4000;
const MAX_NONCE_CHARS: usize = 64;
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 100;

pub struct MessageSvc {
    state: AppState,
}

impl MessageSvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

type MessageRow = (i64, i64, i64, String, Option<String>, Option<DateTime<Utc>>);

fn row_to_message(channel_id: u64, row: MessageRow) -> Message {
    let (id, guild_id, author_id, content, nonce, edited_at) = row;
    let id = id_from_db(id);
    Message {
        id,
        channel_id,
        guild_id: id_from_db(guild_id),
        author_id: id_from_db(author_id),
        content,
        created_at: datetime_to_proto(snowflake_datetime(id)).into(),
        edited_at: edited_at.map(datetime_to_proto).into(),
        nonce: nonce.unwrap_or_default(),
        ..Default::default()
    }
}

impl MessageService for MessageSvc {
    async fn send_message(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, SendMessageRequest>,
    ) -> ServiceResult<SendMessageResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let guild_id = require_channel_member(&self.state, user_id, req.channel_id).await?;

        if req.content.trim().is_empty() {
            return Err(ConnectError::invalid_argument("message content must not be empty"));
        }
        if req.content.chars().count() > MAX_CONTENT_CHARS {
            return Err(ConnectError::invalid_argument(
                "message content must be at most 4000 characters",
            ));
        }
        // TODO: nonce-based deduplication of retried sends is not implemented; the nonce is only echoed.
        let nonce = req.nonce.unwrap_or_default();
        if nonce.chars().count() > MAX_NONCE_CHARS {
            return Err(ConnectError::invalid_argument("nonce must be at most 64 characters"));
        }

        let id = self.state.snowflake.next_id();
        self.state
            .session
            .execute_unpaged(
                "INSERT INTO messages (channel_id, bucket, id, guild_id, author_id, content, nonce) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                (
                    id_to_db(req.channel_id),
                    bucket_of(id),
                    id_to_db(id),
                    id_to_db(guild_id),
                    id_to_db(user_id),
                    req.content,
                    nonce,
                ),
            )
            .await
            .map_err(internal)?;

        let message = row_to_message(
            req.channel_id,
            (
                id_to_db(id),
                id_to_db(guild_id),
                id_to_db(user_id),
                req.content.to_owned(),
                Some(nonce.to_owned()),
                None,
            ),
        );
        self.state
            .events
            .publish_to_guild(guild_id, EventPayload::MessageCreated(message.clone()));
        Response::ok(SendMessageResponse {
            message: message.into(),
            ..Default::default()
        })
    }

    async fn list_messages(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, ListMessagesRequest>,
    ) -> ServiceResult<ListMessagesResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        require_channel_member(&self.state, user_id, req.channel_id).await?;

        let limit = match req.limit as usize {
            0 => DEFAULT_PAGE_SIZE,
            n => n.min(MAX_PAGE_SIZE),
        };
        if req.before == Some(0) {
            return Response::ok(ListMessagesResponse::default());
        }

        let mut rows = self.fetch_older(req.channel_id, req.before, limit + 1).await?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let messages: Vec<Message> = rows
            .into_iter()
            .map(|row| row_to_message(req.channel_id, row))
            .collect();
        let next_before = has_more.then(|| messages.last().map(|m| m.id)).flatten();
        Response::ok(ListMessagesResponse {
            messages,
            next_before,
            has_more,
            ..Default::default()
        })
    }
}

impl MessageSvc {
    /// Returns up to `want` messages of `channel_id` with an id below `before` (or the newest
    /// ones when `before` is `None`), newest first.
    ///
    /// Messages are partitioned by `(channel_id, bucket)`, so a page cannot be read with one
    /// query. Starting at the bucket of `before` (clamped to the current bucket), this walks
    /// buckets downward, asking each for the rows still needed, until `want` rows are collected
    /// or it has passed `bucket_of(channel_id)`: a message can't predate its channel, so no
    /// lower bucket can hold anything.
    ///
    /// Cost: one query per bucket visited, so a channel idle for many buckets scans that many
    /// empty partitions. Idea for later: remember each channel's newest non-empty bucket.
    async fn fetch_older(
        &self,
        channel_id: u64,
        before: Option<u64>,
        want: usize,
    ) -> Result<Vec<MessageRow>, ConnectError> {
        // IDs are stored as positive i64, so any larger bound is equivalent to i64::MAX.
        let bound = before.unwrap_or(i64::MAX as u64).min(i64::MAX as u64);
        let current_bucket = current_bucket();
        let lowest_bucket = bucket_of(channel_id);

        let mut rows = Vec::with_capacity(want);
        let mut bucket = bucket_of(bound).min(current_bucket);
        while bucket >= lowest_bucket && rows.len() < want {
            let remaining = (want - rows.len()) as i32;
            let found = self
                .state
                .session
                .execute_unpaged(
                    "SELECT id, guild_id, author_id, content, nonce, edited_at FROM messages \
                     WHERE channel_id = ? AND bucket = ? AND id < ? LIMIT ?",
                    (id_to_db(channel_id), bucket, id_to_db(bound), remaining),
                )
                .await
                .map_err(internal)?
                .into_rows_result()
                .map_err(internal)?
                .rows::<MessageRow>()
                .map_err(internal)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?;
            rows.extend(found);
            bucket -= 1;
        }
        Ok(rows)
    }
}
