//! Message reactions. Custom emojis are inserted straight into the `emojis`
//! table, so these tests need ScyllaDB only (no object storage).
mod common;

use std::time::Duration;

use common::TestApp;
use common::fixtures::*;
use connectrpc::{ConnectError, ErrorCode};
use thehideout_server::convert::{id_to_db, snowflake_datetime};
use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::*;
use thehideout_server::snowflake::{BUCKET_MS, bucket_of, make_id};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

fn unicode(s: &str) -> ReactionEmoji {
    ReactionEmoji {
        kind: Some(reaction_emoji::Kind::Unicode(s.into())),
        ..Default::default()
    }
}

fn custom(id: u64) -> ReactionEmoji {
    ReactionEmoji {
        kind: Some(reaction_emoji::Kind::CustomEmojiId(id)),
        ..Default::default()
    }
}

fn messages(app: &TestApp, who: &Account) -> MessageServiceClient<connectrpc::client::HttpClient> {
    app.client(MessageServiceClient::new, Some(&who.token))
}

async fn send(app: &TestApp, who: &Account, channel_id: u64, text: &str) -> u64 {
    messages(app, who)
        .send_message(SendMessageRequest {
            channel_id,
            content: text.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .message
        .unwrap()
        .id
}

async fn add(app: &TestApp, who: &Account, channel_id: u64, message_id: u64, emoji: ReactionEmoji) -> Result<(), ConnectError> {
    messages(app, who)
        .add_reaction(AddReactionRequest {
            channel_id,
            message_id,
            emoji: emoji.into(),
            ..Default::default()
        })
        .await?;
    Ok(())
}

async fn remove(app: &TestApp, who: &Account, channel_id: u64, message_id: u64, emoji: ReactionEmoji) -> Result<(), ConnectError> {
    messages(app, who)
        .remove_reaction(RemoveReactionRequest {
            channel_id,
            message_id,
            emoji: emoji.into(),
            ..Default::default()
        })
        .await?;
    Ok(())
}

async fn list(app: &TestApp, who: &Account, channel_id: u64, limit: u32) -> Vec<Message> {
    messages(app, who)
        .list_messages(ListMessagesRequest {
            channel_id,
            limit,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .messages
}

/// `(emoji, count, me)` of the reactions of `message_id` as `who` sees them.
async fn reactions_of(app: &TestApp, who: &Account, channel_id: u64, message_id: u64) -> Vec<(ReactionEmoji, u32, bool)> {
    list(app, who, channel_id, 100)
        .await
        .into_iter()
        .find(|m| m.id == message_id)
        .expect("message listed")
        .reactions
        .iter()
        .map(|r| (r.emoji.as_option().cloned().unwrap(), r.count, r.me))
        .collect()
}

async fn insert_emoji(app: &TestApp, guild_id: u64, name: &str) -> u64 {
    let id = app.state.snowflake.next_id();
    app.state
        .session
        .execute_unpaged(
            "INSERT INTO emojis (guild_id, id, name, animated, image, creator_id) VALUES (?, ?, ?, ?, ?, ?)",
            (id_to_db(guild_id), id_to_db(id), name, false, "", 0i64),
        )
        .await
        .unwrap();
    id
}

fn assert_code<T: std::fmt::Debug>(result: Result<T, ConnectError>, code: ErrorCode) {
    match result {
        Err(e) => assert_eq!(e.code, code, "{e:?}"),
        Ok(v) => panic!("expected {code:?}, got Ok({v:?})"),
    }
}

struct Setup {
    app: TestApp,
    alice: Account,
    bob: Account,
    guild: Guild,
    channel: u64,
}

async fn setup() -> Setup {
    let app = TestApp::start_with_heartbeat(Duration::from_millis(200)).await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let channel = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    Setup { app, alice, bob, guild, channel }
}

macro_rules! next_payload {
    ($stream:expr) => {{
        loop {
            let msg = timeout(WAIT, $stream.message())
                .await
                .expect("timed out waiting for an event")
                .expect("stream error")
                .expect("stream ended");
            let event = msg.to_owned_message().event.into_option().expect("event");
            match event.payload {
                Some(Payload::Heartbeat(_)) | None => continue,
                Some(p) => break p,
            }
        }
    }};
}

macro_rules! assert_quiet {
    ($stream:expr) => {
        assert!(
            timeout(Duration::from_millis(500), async {
                loop {
                    let msg = $stream.message().await.unwrap().unwrap().to_owned_message();
                    if !matches!(msg.event.into_option().and_then(|e| e.payload), Some(Payload::Heartbeat(_)) | None) {
                        break;
                    }
                }
            })
            .await
            .is_err(),
            "unexpected event"
        )
    };
}

#[tokio::test]
async fn add_remove_counts_and_me_per_caller() {
    let Setup { app, alice, bob, channel, .. } = setup().await;
    let msg = send(&app, &alice, channel, "hello").await;
    let other = send(&app, &alice, channel, "plain").await;
    assert!(reactions_of(&app, &alice, channel, msg).await.is_empty());

    add(&app, &alice, channel, msg, unicode("👍")).await.unwrap();
    add(&app, &bob, channel, msg, unicode("🎉")).await.unwrap();
    add(&app, &bob, channel, msg, unicode("👍")).await.unwrap();

    // Ordered by first use; `me` depends on the caller.
    assert_eq!(
        reactions_of(&app, &alice, channel, msg).await,
        vec![(unicode("👍"), 2, true), (unicode("🎉"), 1, false)]
    );
    assert_eq!(
        reactions_of(&app, &bob, channel, msg).await,
        vec![(unicode("👍"), 2, true), (unicode("🎉"), 1, true)]
    );
    assert!(reactions_of(&app, &alice, channel, other).await.is_empty());

    // Adding twice is a no-op.
    add(&app, &alice, channel, msg, unicode("👍")).await.unwrap();
    assert_eq!(reactions_of(&app, &alice, channel, msg).await[0], (unicode("👍"), 2, true));

    // Removing is idempotent too, including a reaction that was never there.
    remove(&app, &alice, channel, msg, unicode("👍")).await.unwrap();
    remove(&app, &alice, channel, msg, unicode("👍")).await.unwrap();
    remove(&app, &alice, channel, msg, unicode("🔥")).await.unwrap();
    assert_eq!(
        reactions_of(&app, &alice, channel, msg).await,
        vec![(unicode("👍"), 1, false), (unicode("🎉"), 1, false)]
    );
    remove(&app, &bob, channel, msg, unicode("👍")).await.unwrap();
    remove(&app, &bob, channel, msg, unicode("🎉")).await.unwrap();
    assert!(reactions_of(&app, &alice, channel, msg).await.is_empty());
    // Multi-codepoint emoji (ZWJ family, flag, keycap, skin tone) work as keys.
    for e in ["👨\u{200d}👩\u{200d}👧", "🇸🇮", "1\u{FE0F}\u{20E3}", "👍🏽"] {
        add(&app, &alice, channel, msg, unicode(e)).await.unwrap();
    }
    assert_eq!(reactions_of(&app, &alice, channel, msg).await.len(), 4);
    app.cleanup().await;
}

#[tokio::test]
async fn invalid_emoji_message_and_access_are_rejected() {
    let Setup { app, alice, guild, channel, .. } = setup().await;
    let eve = user(&app, "eve").await;
    let msg = send(&app, &alice, channel, "hello").await;
    let invalid = ErrorCode::InvalidArgument;
    // No emoji at all, text, two emojis, a custom id of nobody.
    let empty = AddReactionRequest {
        channel_id: channel,
        message_id: msg,
        ..Default::default()
    };
    assert_code(messages(&app, &alice).add_reaction(empty).await, invalid);
    for bad in ["", "a", "ab", "😀😀", "😀a", &"😀".repeat(9)] {
        assert_code(add(&app, &alice, channel, msg, unicode(bad)).await, invalid);
        assert_code(remove(&app, &alice, channel, msg, unicode(bad)).await, invalid);
    }
    assert_code(add(&app, &alice, channel, msg, custom(424242)).await, invalid);
    assert_code(add(&app, &alice, channel, msg, custom(0)).await, invalid);
    // Emojis of another guild are not usable here.
    let other_guild = common::fixtures::guild(&app, &alice).await;
    let foreign = insert_emoji(&app, other_guild.id, "foreign").await;
    assert_code(add(&app, &alice, channel, msg, custom(foreign)).await, invalid);
    // Own guild's are.
    let own = insert_emoji(&app, guild.id, "own").await;
    add(&app, &alice, channel, msg, custom(own)).await.unwrap();
    assert_eq!(reactions_of(&app, &alice, channel, msg).await, vec![(custom(own), 1, true)]);

    // A deleted emoji's reactions stay (clients render text) and can be removed.
    app.state
        .session
        .execute_unpaged("DELETE FROM emojis WHERE guild_id = ? AND id = ?", (id_to_db(guild.id), id_to_db(own)))
        .await
        .unwrap();
    assert_eq!(reactions_of(&app, &alice, channel, msg).await, vec![(custom(own), 1, true)]);
    remove(&app, &alice, channel, msg, custom(own)).await.unwrap();
    assert!(reactions_of(&app, &alice, channel, msg).await.is_empty());

    // Unknown message / channel and outsiders.
    assert_code(add(&app, &alice, channel, 424242, unicode("👍")).await, ErrorCode::NotFound);
    assert_code(remove(&app, &alice, channel, 424242, unicode("👍")).await, ErrorCode::NotFound);
    assert_code(add(&app, &alice, 424242, msg, unicode("👍")).await, ErrorCode::NotFound);
    assert_code(add(&app, &eve, channel, msg, unicode("👍")).await, ErrorCode::NotFound);
    assert_code(remove(&app, &eve, channel, msg, unicode("👍")).await, ErrorCode::NotFound);
    app.cleanup().await;
}

#[tokio::test]
async fn at_most_20_distinct_emojis_per_message() {
    let Setup { app, alice, bob, channel, .. } = setup().await;
    let msg = send(&app, &alice, channel, "hello").await;
    let emojis: Vec<String> = ('\u{1F600}'..='\u{1F613}').map(String::from).collect(); // 20 emoji
    assert_eq!(emojis.len(), 20);
    for e in &emojis {
        add(&app, &alice, channel, msg, unicode(e)).await.unwrap();
    }
    assert_code(add(&app, &alice, channel, msg, unicode("🔥")).await, ErrorCode::ResourceExhausted);
    // Another user joining an existing emoji, or the same user repeating, is fine at the cap.
    add(&app, &bob, channel, msg, unicode(&emojis[3])).await.unwrap();
    add(&app, &alice, channel, msg, unicode(&emojis[0])).await.unwrap();
    assert_eq!(reactions_of(&app, &alice, channel, msg).await.len(), 20);
    // A message of its own has its own budget.
    let second = send(&app, &alice, channel, "second").await;
    add(&app, &alice, channel, second, unicode("🔥")).await.unwrap();
    // Removing the last reaction of an emoji frees a slot.
    remove(&app, &alice, channel, msg, unicode(&emojis[19])).await.unwrap();
    add(&app, &alice, channel, msg, unicode("🔥")).await.unwrap();
    assert_code(add(&app, &alice, channel, msg, unicode("🚀")).await, ErrorCode::ResourceExhausted);
    app.cleanup().await;
}

#[tokio::test]
async fn guild_events_reach_members_once_per_change() {
    let Setup { app, alice, bob, guild, channel } = setup().await;
    let eve = user(&app, "eve").await;
    let own = insert_emoji(&app, guild.id, "own").await;
    let msg = send(&app, &alice, channel, "hello").await;
    let mut bob_stream = app.client(GatewayServiceClient::new, Some(&bob.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    let mut eve_stream = app.client(GatewayServiceClient::new, Some(&eve.token)).subscribe(SubscribeRequest::default()).await.unwrap();

    add(&app, &alice, channel, msg, unicode("👍")).await.unwrap();
    add(&app, &alice, channel, msg, unicode("👍")).await.unwrap(); // no-op: no second event
    add(&app, &alice, channel, msg, custom(own)).await.unwrap();
    remove(&app, &alice, channel, msg, unicode("👍")).await.unwrap();
    remove(&app, &alice, channel, msg, unicode("👍")).await.unwrap(); // no-op

    let Payload::ReactionAdded(ev) = next_payload!(bob_stream) else { panic!("expected ReactionAdded") };
    assert_eq!((ev.channel_id, ev.message_id, ev.user_id), (channel, msg, alice.id));
    assert_eq!(ev.emoji.as_option().cloned(), Some(unicode("👍")));
    let Payload::ReactionAdded(ev) = next_payload!(bob_stream) else { panic!("expected ReactionAdded") };
    assert_eq!(ev.emoji.as_option().cloned(), Some(custom(own)));
    let Payload::ReactionRemoved(ev) = next_payload!(bob_stream) else { panic!("expected ReactionRemoved") };
    assert_eq!((ev.channel_id, ev.message_id, ev.user_id), (channel, msg, alice.id));
    assert_eq!(ev.emoji.as_option().cloned(), Some(unicode("👍")));
    assert_quiet!(bob_stream);
    assert_quiet!(eve_stream);
    app.cleanup().await;
}

#[tokio::test]
async fn direct_messages_allow_unicode_only_and_reach_participants() {
    let Setup { app, alice, bob, guild, .. } = setup().await;
    let eve = user(&app, "eve").await;
    let dm = app
        .client(DmServiceClient::new, Some(&alice.token))
        .open_dm(OpenDmRequest {
            user_id: bob.id,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    let msg = send(&app, &alice, dm.id, "psst").await;
    let mut bob_stream = app.client(GatewayServiceClient::new, Some(&bob.token)).subscribe(SubscribeRequest::default()).await.unwrap();
    let mut eve_stream = app.client(GatewayServiceClient::new, Some(&eve.token)).subscribe(SubscribeRequest::default()).await.unwrap();

    add(&app, &bob, dm.id, msg, unicode("❤\u{FE0F}")).await.unwrap();
    let Payload::ReactionAdded(ev) = next_payload!(bob_stream) else { panic!("expected ReactionAdded") };
    assert_eq!((ev.channel_id, ev.message_id, ev.user_id), (dm.id, msg, bob.id));
    assert_eq!(reactions_of(&app, &alice, dm.id, msg).await, vec![(unicode("❤\u{FE0F}"), 1, false)]);
    assert_eq!(reactions_of(&app, &bob, dm.id, msg).await, vec![(unicode("❤\u{FE0F}"), 1, true)]);

    // Even a real emoji of a guild both users are in is invalid in a DM.
    let own = insert_emoji(&app, guild.id, "own").await;
    assert_code(add(&app, &alice, dm.id, msg, custom(own)).await, ErrorCode::InvalidArgument);
    assert_code(remove(&app, &alice, dm.id, msg, custom(own)).await, ErrorCode::InvalidArgument);
    // Outsiders can neither react nor listen.
    assert_code(add(&app, &eve, dm.id, msg, unicode("👍")).await, ErrorCode::NotFound);
    assert_quiet!(eve_stream);
    remove(&app, &bob, dm.id, msg, unicode("❤\u{FE0F}")).await.unwrap();
    assert!(matches!(next_payload!(bob_stream), Payload::ReactionRemoved(_)));
    app.cleanup().await;
}

#[tokio::test]
async fn list_messages_includes_reactions_across_buckets() {
    let Setup { app, alice, bob, guild, .. } = setup().await;
    let now_bucket = bucket_of(app.state.snowflake.next_id());
    assert!(now_bucket >= 10, "test assumes the clock is well past the epoch");
    let base = now_bucket as u64 - 8;
    // Random worker and sequence: the shared keyspace keeps the data of earlier runs.
    let (worker, sequence) = (rand::random::<u16>() & 0x3ff, rand::random::<u16>() & 0xfff);
    let in_bucket = |bucket: u64, offset_ms: u64| make_id(bucket * BUCKET_MS + offset_ms, worker, sequence);
    let channel_id = in_bucket(base, 0);
    let session = &app.state.session;
    session
        .execute_unpaged(
            "INSERT INTO channels (guild_id, id, name, created_at) VALUES (?, ?, ?, ?)",
            (id_to_db(guild.id), id_to_db(channel_id), "old", snowflake_datetime(channel_id)),
        )
        .await
        .unwrap();
    session
        .execute_unpaged("INSERT INTO channels_by_id (id, guild_id) VALUES (?, ?)", (id_to_db(channel_id), id_to_db(guild.id)))
        .await
        .unwrap();
    // Three buckets with an empty one between; the last message is the newest.
    let ids = [
        in_bucket(base, 1_000),
        in_bucket(base, 2_000),
        in_bucket(base + 3, 5),
        in_bucket(now_bucket as u64, 10),
    ];
    for id in ids {
        session
            .execute_unpaged(
                "INSERT INTO messages (channel_id, bucket, id, guild_id, author_id, content, nonce) VALUES (?, ?, ?, ?, ?, ?, ?)",
                (id_to_db(channel_id), bucket_of(id), id_to_db(id), id_to_db(guild.id), 0i64, format!("m{id}"), ""),
            )
            .await
            .unwrap();
    }
    add(&app, &alice, channel_id, ids[0], unicode("👍")).await.unwrap();
    add(&app, &bob, channel_id, ids[0], unicode("👍")).await.unwrap();
    add(&app, &bob, channel_id, ids[2], unicode("🎉")).await.unwrap();
    add(&app, &alice, channel_id, ids[3], unicode("🔥")).await.unwrap();
    add(&app, &alice, channel_id, ids[3], unicode("🚀")).await.unwrap();

    let page = list(&app, &alice, channel_id, 100).await;
    assert_eq!(page.iter().map(|m| m.id).collect::<Vec<_>>(), [ids[3], ids[2], ids[1], ids[0]]);
    let summary = |m: &Message| m.reactions.iter().map(|r| (r.emoji.as_option().cloned().unwrap(), r.count, r.me)).collect::<Vec<_>>();
    assert_eq!(summary(&page[0]), vec![(unicode("🔥"), 1, true), (unicode("🚀"), 1, true)]);
    assert_eq!(summary(&page[1]), vec![(unicode("🎉"), 1, false)]);
    assert!(page[2].reactions.is_empty());
    assert_eq!(summary(&page[3]), vec![(unicode("👍"), 2, true)]);

    // The same, paginated one message at a time: each page carries its own reactions.
    let mut before = None;
    let mut seen = Vec::new();
    loop {
        let resp = messages(&app, &bob)
            .list_messages(ListMessagesRequest {
                channel_id,
                before,
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap()
            .into_owned();
        seen.extend(resp.messages.iter().map(|m| (m.id, summary(m))));
        match resp.next_before {
            Some(b) => before = Some(b),
            None => break,
        }
    }
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[1].1, vec![(unicode("🎉"), 1, true)]);
    assert_eq!(seen[3].1, vec![(unicode("👍"), 2, true)]);
    app.cleanup().await;
}

#[tokio::test]
async fn reactions_are_rate_limited() {
    let Setup { app, alice, channel, .. } = setup().await;
    let msg = send(&app, &alice, channel, "hello").await;
    for i in 0..120 {
        let result = if i % 2 == 0 {
            add(&app, &alice, channel, msg, unicode("👍")).await
        } else {
            remove(&app, &alice, channel, msg, unicode("👍")).await
        };
        result.unwrap_or_else(|e| panic!("call {i}: {e:?}"));
    }
    assert_code(add(&app, &alice, channel, msg, unicode("👍")).await, ErrorCode::ResourceExhausted);
    app.cleanup().await;
}
