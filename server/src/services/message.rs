use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use futures::future::try_join_all;
use scylla::value::{CqlValue, Row};

use crate::AppState;
use crate::access::{ChannelAccess, require_channel_access};
use crate::auth::{authenticate, internal};
use crate::convert::{datetime_to_proto, id_from_db, id_to_db, snowflake_datetime};
use crate::events::EventPayload;
use crate::proto::thehideout::v1::*;
use crate::services::emoji::emoji_exists;
use crate::snowflake::{bucket_of, current_bucket};
use crate::validation::validate_unicode_emoji;

const MAX_CONTENT_CHARS: usize = 4000;
const MAX_NONCE_CHARS: usize = 64;
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 100;

/// Distinct emojis one message may carry.
pub const MAX_REACTION_EMOJIS: usize = 20;
/// Reaction adds/removes allowed per user within [`REACTION_WINDOW`].
pub const REACTION_MAX: usize = 120;
const REACTION_WINDOW: Duration = Duration::from_secs(60);

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
        let access = require_channel_access(&self.state, user_id, req.channel_id).await?;
        // DM messages carry guild_id = 0; guild channels carry their guild's ID.
        let guild_id = match &access {
            ChannelAccess::Guild(guild_id) => *guild_id,
            ChannelAccess::Dm => 0,
        };

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
        let event = EventPayload::MessageCreated(message.clone());
        match &access {
            ChannelAccess::Guild(guild_id) => self.state.events.publish_to_guild(*guild_id, event),
            ChannelAccess::Dm => self.state.events.publish_to_dm(req.channel_id, event),
        }
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
        require_channel_access(&self.state, user_id, req.channel_id).await?;

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
        let mut messages: Vec<Message> = rows
            .into_iter()
            .map(|row| row_to_message(req.channel_id, row))
            .collect();
        let mut reactions = self.load_reactions(req.channel_id, user_id, &messages).await?;
        for message in &mut messages {
            message.reactions = reactions.remove(&message.id).unwrap_or_default();
        }
        let next_before = has_more.then(|| messages.last().map(|m| m.id)).flatten();
        Response::ok(ListMessagesResponse {
            messages,
            next_before,
            has_more,
            ..Default::default()
        })
    }

    async fn add_reaction(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, AddReactionRequest>,
    ) -> ServiceResult<AddReactionResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        self.throttle_reaction(user_id)?;
        let access = require_channel_access(&self.state, user_id, req.channel_id).await?;
        let kind = req.emoji.as_option().and_then(|e| e.kind.clone());
        let key = self.reaction_key(&access, kind, true).await?;
        self.require_message(req.channel_id, req.message_id).await?;
        let (channel, bucket, message) = (id_to_db(req.channel_id), bucket_of(req.message_id), id_to_db(req.message_id));

        // A new distinct emoji counts against the cap; adding to an emoji the
        // message already has never does. Racing adds of different new emojis
        // can overshoot the cap by a few (see migration 0006); the cap bounds
        // growth rather than being a hard invariant.
        // Every row of an emoji carries the emoji's first-use stamp (`at`), so
        // the order of emojis survives individual users removing theirs.
        let first_use = self
            .state
            .session
            .execute_unpaged(
                "SELECT at FROM reactions WHERE channel_id = ? AND bucket = ? AND message_id = ? \
                 AND emoji_key = ? LIMIT 1",
                (channel, bucket, message, &key),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .maybe_first_row::<(Option<i64>,)>()
            .map_err(internal)?;
        let present = first_use.is_some();
        let at = first_use.and_then(|(at,)| at).unwrap_or_else(|| id_to_db(self.state.snowflake.next_id()));
        if !present {
            let distinct: HashSet<String> = self
                .state
                .session
                .execute_unpaged(
                    "SELECT emoji_key FROM reactions WHERE channel_id = ? AND bucket = ? AND message_id = ?",
                    (channel, bucket, message),
                )
                .await
                .map_err(internal)?
                .into_rows_result()
                .map_err(internal)?
                .rows::<(String,)>()
                .map_err(internal)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?
                .into_iter()
                .map(|(k,)| k)
                .collect();
            if distinct.len() >= MAX_REACTION_EMOJIS {
                return Err(ConnectError::resource_exhausted("a message can have at most 20 distinct reactions"));
            }
        }

        let result = self
            .state
            .session
            .execute_unpaged(
                "INSERT INTO reactions (channel_id, bucket, message_id, emoji_key, user_id, at) \
                 VALUES (?, ?, ?, ?, ?, ?) IF NOT EXISTS",
                (channel, bucket, message, &key, id_to_db(user_id), at),
            )
            .await
            .map_err(internal)?;
        if applied(result)? {
            self.publish_reaction(&access, &req.emoji.as_option().and_then(|e| e.kind.clone()), req.channel_id, req.message_id, user_id, true);
        }
        Response::ok(AddReactionResponse::default())
    }

    async fn remove_reaction(
        &self,
        ctx: RequestContext,
        req: ServiceRequest<'_, RemoveReactionRequest>,
    ) -> ServiceResult<RemoveReactionResponse> {
        let user_id = authenticate(&self.state, &ctx).await?;
        self.throttle_reaction(user_id)?;
        let access = require_channel_access(&self.state, user_id, req.channel_id).await?;
        let kind = req.emoji.as_option().and_then(|e| e.kind.clone());
        // The emoji may have been deleted since: only its shape is checked so
        // users can still remove their own reaction.
        let key = self.reaction_key(&access, kind.clone(), false).await?;
        self.require_message(req.channel_id, req.message_id).await?;
        let result = self
            .state
            .session
            .execute_unpaged(
                "DELETE FROM reactions WHERE channel_id = ? AND bucket = ? AND message_id = ? \
                 AND emoji_key = ? AND user_id = ? IF EXISTS",
                (id_to_db(req.channel_id), bucket_of(req.message_id), id_to_db(req.message_id), &key, id_to_db(user_id)),
            )
            .await
            .map_err(internal)?;
        if applied(result)? {
            self.publish_reaction(&access, &kind, req.channel_id, req.message_id, user_id, false);
        }
        Response::ok(RemoveReactionResponse::default())
    }
}

/// Whether an LWT was applied (`[applied]` is its first column).
fn applied(result: scylla::response::query_result::QueryResult) -> Result<bool, ConnectError> {
    let row = result
        .into_rows_result()
        .map_err(internal)?
        .first_row::<Row>()
        .map_err(internal)?;
    Ok(matches!(row.columns.first(), Some(Some(CqlValue::Boolean(true)))))
}

/// `"u:<unicode>"` / `"c:<id>"` back to the proto emoji.
fn key_to_emoji(key: &str) -> Option<ReactionEmoji> {
    let kind = match key.split_at_checked(2)? {
        ("u:", unicode) => reaction_emoji::Kind::Unicode(unicode.to_owned()),
        ("c:", id) => reaction_emoji::Kind::CustomEmojiId(id.parse().ok()?),
        _ => return None,
    };
    Some(ReactionEmoji {
        kind: Some(kind),
        ..Default::default()
    })
}

impl MessageSvc {
    fn throttle_reaction(&self, user_id: u64) -> Result<(), ConnectError> {
        self.state
            .limiter
            .check(&format!("reaction:{user_id}"), REACTION_MAX, REACTION_WINDOW)
    }

    /// Validates the emoji for the channel and returns its storage key.
    /// `must_exist` additionally requires a custom emoji to exist in the guild.
    async fn reaction_key(
        &self,
        access: &ChannelAccess,
        kind: Option<reaction_emoji::KindView<'_>>,
        must_exist: bool,
    ) -> Result<String, ConnectError> {
        match kind {
            None => Err(ConnectError::invalid_argument("emoji is required")),
            Some(reaction_emoji::KindView::Unicode(unicode)) => {
                validate_unicode_emoji(unicode)?;
                Ok(format!("u:{unicode}"))
            }
            Some(reaction_emoji::KindView::CustomEmojiId(id)) => match access {
                ChannelAccess::Dm => Err(ConnectError::invalid_argument("custom emojis cannot be used in direct messages")),
                ChannelAccess::Guild(guild_id) => {
                    if id == 0 || (must_exist && !emoji_exists(&self.state, *guild_id, id).await?) {
                        return Err(ConnectError::invalid_argument("custom emoji does not exist in this guild"));
                    }
                    Ok(format!("c:{id}"))
                }
            },
        }
    }

    async fn require_message(&self, channel_id: u64, message_id: u64) -> Result<(), ConnectError> {
        let found = self
            .state
            .session
            .execute_unpaged(
                "SELECT id FROM messages WHERE channel_id = ? AND bucket = ? AND id = ?",
                (id_to_db(channel_id), bucket_of(message_id), id_to_db(message_id)),
            )
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows_num()
            > 0;
        if found { Ok(()) } else { Err(ConnectError::not_found("message not found")) }
    }

    /// Publishes to the same audience as `message_created`.
    fn publish_reaction(
        &self,
        access: &ChannelAccess,
        kind: &Option<reaction_emoji::KindView<'_>>,
        channel_id: u64,
        message_id: u64,
        user_id: u64,
        added: bool,
    ) {
        let kind = kind.as_ref().map(|k| match k {
            reaction_emoji::KindView::Unicode(u) => reaction_emoji::Kind::Unicode((*u).to_owned()),
            reaction_emoji::KindView::CustomEmojiId(id) => reaction_emoji::Kind::CustomEmojiId(*id),
        });
        let change = ReactionChange {
            channel_id,
            message_id,
            emoji: ReactionEmoji { kind, ..Default::default() }.into(),
            user_id,
            ..Default::default()
        };
        let event = if added { EventPayload::ReactionAdded(change) } else { EventPayload::ReactionRemoved(change) };
        match access {
            ChannelAccess::Guild(guild_id) => self.state.events.publish_to_guild(*guild_id, event),
            ChannelAccess::Dm => self.state.events.publish_to_dm(channel_id, event),
        }
    }

    /// The reactions of every message of a page, with `me` for `user_id`:
    /// one query per bucket the page touches. Counts come from the rows
    /// themselves, so the cost is proportional to the reactions on the page;
    /// emojis are ordered by first use (`at`, see `add_reaction`).
    async fn load_reactions(
        &self,
        channel_id: u64,
        user_id: u64,
        messages: &[Message],
    ) -> Result<HashMap<u64, Vec<Reaction>>, ConnectError> {
        let mut per_bucket: BTreeMap<i32, Vec<i64>> = BTreeMap::new();
        for m in messages {
            per_bucket.entry(bucket_of(m.id)).or_default().push(id_to_db(m.id));
        }
        let me = id_to_db(user_id);
        let fetched = try_join_all(per_bucket.into_iter().map(|(bucket, ids)| async move {
            self.state
                .session
                .execute_unpaged(
                    "SELECT message_id, emoji_key, user_id, at FROM reactions \
                     WHERE channel_id = ? AND bucket = ? AND message_id IN ?",
                    (id_to_db(channel_id), bucket, ids),
                )
                .await
                .map_err(internal)?
                .into_rows_result()
                .map_err(internal)?
                .rows::<(i64, String, i64, Option<i64>)>()
                .map_err(internal)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)
        }))
        .await?;

        // message -> emoji key -> (count, me, first use)
        let mut agg: HashMap<u64, HashMap<String, (u32, bool, i64)>> = HashMap::new();
        for (message_id, key, reactor, at) in fetched.into_iter().flatten() {
            let entry = agg
                .entry(id_from_db(message_id))
                .or_default()
                .entry(key)
                .or_insert((0, false, i64::MAX));
            entry.0 += 1;
            entry.1 |= reactor == me;
            entry.2 = entry.2.min(at.unwrap_or(0));
        }
        Ok(agg
            .into_iter()
            .map(|(message_id, emojis)| {
                let mut emojis: Vec<_> = emojis.into_iter().collect();
                emojis.sort_by(|a, b| a.1.2.cmp(&b.1.2).then_with(|| a.0.cmp(&b.0)));
                let reactions = emojis
                    .into_iter()
                    .filter_map(|(key, (count, me, _))| {
                        Some(Reaction {
                            emoji: key_to_emoji(&key)?.into(),
                            count,
                            me,
                            ..Default::default()
                        })
                    })
                    .collect();
                (message_id, reactions)
            })
            .collect())
    }

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
