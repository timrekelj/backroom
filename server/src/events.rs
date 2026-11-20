//! In-process event bus used by the gateway to fan events out to open streams.
//!
//! The API (`publish_to_guild`, `subscribe_guild`, `notify_user_joined_guild`,
//! `subscribe_user`) is deliberately small so the bus can later be reimplemented on top of
//! a multi-node transport such as NATS without touching the services.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::broadcast;

use crate::proto::thehideout::v1::{Channel, GuildMemberJoined, Message};

/// Messages buffered per guild channel before slow receivers start lagging.
const GUILD_CHANNEL_CAPACITY: usize = 1024;
/// Control notifications buffered per user.
const USER_CHANNEL_CAPACITY: usize = 64;

/// An event payload published to a guild; the gateway stamps it with a per-stream sequence.
#[derive(Debug, Clone)]
pub enum EventPayload {
    MessageCreated(Message),
    ChannelCreated(Channel),
    GuildMemberJoined(GuildMemberJoined),
}

/// Control notification delivered to a user's open streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserControl {
    /// The user joined this guild; streams should start listening to it.
    JoinedGuild(u64),
}

#[derive(Default)]
pub struct EventBus {
    guilds: Mutex<HashMap<u64, broadcast::Sender<Arc<EventPayload>>>>,
    users: Mutex<HashMap<u64, broadcast::Sender<UserControl>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Delivers `payload` to every current subscriber of the guild. Never fails; having no
    /// subscribers is normal.
    pub fn publish_to_guild(&self, guild_id: u64, payload: EventPayload) {
        let mut guilds = lock(&self.guilds);
        if let Some(tx) = guilds.get(&guild_id) {
            if tx.receiver_count() == 0 {
                guilds.remove(&guild_id);
            } else {
                let _ = tx.send(Arc::new(payload));
            }
        }
    }

    /// Receives events published to the guild from now on.
    pub fn subscribe_guild(&self, guild_id: u64) -> broadcast::Receiver<Arc<EventPayload>> {
        lock(&self.guilds)
            .entry(guild_id)
            .or_insert_with(|| broadcast::channel(GUILD_CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// Tells the user's open streams to start listening to `guild_id`.
    pub fn notify_user_joined_guild(&self, user_id: u64, guild_id: u64) {
        let mut users = lock(&self.users);
        if let Some(tx) = users.get(&user_id) {
            if tx.receiver_count() == 0 {
                users.remove(&user_id);
            } else {
                let _ = tx.send(UserControl::JoinedGuild(guild_id));
            }
        }
    }

    /// Receives control notifications for the user from now on.
    pub fn subscribe_user(&self, user_id: u64) -> broadcast::Receiver<UserControl> {
        lock(&self.users)
            .entry(user_id)
            .or_insert_with(|| broadcast::channel(USER_CHANNEL_CAPACITY).0)
            .subscribe()
    }

    /// Removes guild/user channels nobody listens to anymore; called when a stream ends.
    pub fn prune(&self) {
        lock(&self.guilds).retain(|_, tx| tx.receiver_count() > 0);
        lock(&self.users).retain(|_, tx| tx.receiver_count() > 0);
    }

    /// Number of guild channels currently held (for leak checks in tests).
    pub fn guild_channel_count(&self) -> usize {
        lock(&self.guilds).len()
    }

    /// Number of user control channels currently held (for leak checks in tests).
    pub fn user_channel_count(&self) -> usize {
        lock(&self.users).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel_event() -> EventPayload {
        EventPayload::ChannelCreated(Channel::default())
    }

    #[tokio::test]
    async fn publish_reaches_subscribers_and_prunes_dead_channels() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe_guild(1);
        bus.publish_to_guild(1, channel_event());
        assert!(matches!(*rx.recv().await.unwrap(), EventPayload::ChannelCreated(_)));
        assert_eq!(bus.guild_channel_count(), 1);

        drop(rx);
        bus.publish_to_guild(1, channel_event());
        assert_eq!(bus.guild_channel_count(), 0);
    }

    #[tokio::test]
    async fn user_notifications_are_delivered_and_pruned() {
        let bus = EventBus::new();
        bus.notify_user_joined_guild(7, 1); // no subscribers: no-op
        let mut rx = bus.subscribe_user(7);
        bus.notify_user_joined_guild(7, 2);
        assert_eq!(rx.recv().await.unwrap(), UserControl::JoinedGuild(2));
        drop(rx);
        bus.prune();
        assert_eq!(bus.user_channel_count(), 0);
    }
}
