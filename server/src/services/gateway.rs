use std::sync::Arc;
use std::time::{Duration, SystemTime};

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::time::{Instant, Interval, MissedTickBehavior};
use tokio_stream::{StreamExt, StreamMap};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::AppState;
use crate::auth::{authenticate, bearer_token, hash_token, internal, session_exists};
use crate::convert::{id_from_db, id_to_db, system_time_to_proto};
use crate::events::{EventPayload, UserControl};
use crate::proto::thehideout::v1::event::Payload;
use crate::proto::thehideout::v1::*;

pub struct GatewaySvc {
    state: AppState,
}

impl GatewaySvc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl GatewayService for GatewaySvc {
    /// Streams events for every guild the caller belongs to and every DM channel they
    /// participate in, plus any guild they join or DM opened with them while the stream is
    /// open.
    ///
    /// Each stream numbers its events from 1 and sends a heartbeat immediately and then every
    /// heartbeat interval. Events are not replayed: anything published before the stream
    /// opened (or while it was disconnected) must be fetched with the `List*` RPCs.
    ///
    /// Slow clients: each guild buffers a bounded number of events per receiver. If a client
    /// falls behind and events are dropped, the stream ends with `ResourceExhausted` rather
    /// than silently skipping events; the client should reconnect and resync via the `List*`
    /// RPCs.
    async fn subscribe(
        &self,
        ctx: RequestContext,
        _req: ServiceRequest<'_, SubscribeRequest>,
    ) -> ServiceResult<ServiceStream<SubscribeResponse>> {
        let user_id = authenticate(&self.state, &ctx).await?;
        let token_hash = hash_token(bearer_token(&ctx).expect("authenticate succeeded"));
        let bus = &self.state.events;

        // Subscribe to the control channel before reading memberships/DMs so a guild joined
        // or a DM opened in between is not missed (re-adding an already-subscribed channel is
        // harmless).
        let control = bus.subscribe_user(user_id);
        let guild_ids = self
            .state
            .session
            .execute_unpaged("SELECT guild_id FROM guilds_by_user WHERE user_id = ?", (id_to_db(user_id),))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64,)>()
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        let dm_ids = self
            .state
            .session
            .execute_unpaged("SELECT dm_id FROM dms_by_user WHERE user_id = ?", (id_to_db(user_id),))
            .await
            .map_err(internal)?
            .into_rows_result()
            .map_err(internal)?
            .rows::<(i64,)>()
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        let mut guilds = StreamMap::new();
        for (id,) in guild_ids {
            let id = id_from_db(id);
            guilds.insert(id, BroadcastStream::new(bus.subscribe_guild(id)));
        }
        let mut dms = StreamMap::new();
        for (id,) in dm_ids {
            let id = id_from_db(id);
            dms.insert(id, BroadcastStream::new(bus.subscribe_dm(id)));
        }

        let mut heartbeat = tokio::time::interval_at(
            Instant::now() + self.state.heartbeat_interval,
            self.state.heartbeat_interval,
        );
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let session = EventStream {
            state: self.state.clone(),
            control: Some(control),
            guilds,
            dms,
            heartbeat,
            sequence: 0,
            token_hash,
            last_check: Instant::now(),
            recheck_interval: self.state.heartbeat_interval.min(Duration::from_secs(60)),
        };
        Response::stream_ok(futures::stream::unfold(Some(session), |session| async move {
            let mut session = session?;
            match session.next_event().await {
                Ok(event) => Some((Ok(SubscribeResponse { event: event.into(), ..Default::default() }), Some(session))),
                Err(err) => Some((Err(err), None)),
            }
        }))
    }
}

/// State of one open Subscribe stream.
struct EventStream {
    state: AppState,
    /// `None` only while dropping.
    control: Option<broadcast::Receiver<UserControl>>,
    guilds: StreamMap<u64, BroadcastStream<Arc<EventPayload>>>,
    dms: StreamMap<u64, BroadcastStream<Arc<EventPayload>>>,
    heartbeat: Interval,
    sequence: u64,
    /// Hash of the token the stream was opened with, re-checked periodically.
    token_hash: String,
    last_check: Instant,
    recheck_interval: Duration,
}

impl EventStream {
    /// Waits for the next outgoing event. The first call returns a heartbeat immediately.
    async fn next_event(&mut self) -> Result<Event, ConnectError> {
        let payload = if self.sequence == 0 {
            heartbeat_payload()
        } else {
            self.next_payload().await?
        };
        self.sequence += 1;
        Ok(Event {
            sequence: self.sequence,
            payload: Some(payload),
            ..Default::default()
        })
    }

    async fn next_payload(&mut self) -> Result<Payload, ConnectError> {
        loop {
            let control = self.control.as_mut().expect("control receiver present while streaming");
            tokio::select! {
                _ = self.heartbeat.tick() => {
                    // Re-verify the session so a token that expired (Scylla TTL) or was
                    // revoked cannot keep an open stream alive past its validity.
                    if self.last_check.elapsed() >= self.recheck_interval {
                        self.last_check = Instant::now();
                        if !session_exists(&self.state, &self.token_hash).await? {
                            return Err(ConnectError::unauthenticated("session expired"));
                        }
                    }
                    return Ok(heartbeat_payload());
                }
                msg = control.recv() => match msg {
                    Ok(UserControl::JoinedGuild(guild_id)) => {
                        let rx = self.state.events.subscribe_guild(guild_id);
                        self.guilds.insert(guild_id, BroadcastStream::new(rx));
                    }
                    Ok(UserControl::OpenedDm(channel)) => {
                        // Subscribe to the DM channel, then emit the event itself so the
                        // client learns about the new conversation on this stream.
                        let rx = self.state.events.subscribe_dm(channel.id);
                        self.dms.insert(channel.id, BroadcastStream::new(rx));
                        return Ok(Payload::DmOpened(channel.into()));
                    }
                    Ok(UserControl::ProfileUpdated(user)) => return Ok(Payload::UserUpdated(user.into())),
                    Err(RecvError::Lagged(_)) => return Err(lagged()),
                    Err(RecvError::Closed) => return Err(internal("user control channel closed")),
                },
                Some((_, item)) = self.guilds.next(), if !self.guilds.is_empty() => {
                    return match item {
                        Ok(event) => Ok(to_payload(&event)),
                        Err(BroadcastStreamRecvError::Lagged(_)) => Err(lagged()),
                    };
                }
                Some((_, item)) = self.dms.next(), if !self.dms.is_empty() => {
                    return match item {
                        Ok(event) => Ok(to_payload(&event)),
                        Err(BroadcastStreamRecvError::Lagged(_)) => Err(lagged()),
                    };
                }
            }
        }
    }
}

impl Drop for EventStream {
    /// Releases every receiver first so the bus can drop channels nobody listens to.
    fn drop(&mut self) {
        self.guilds.clear();
        self.dms.clear();
        self.control = None;
        self.state.events.prune();
    }
}

fn heartbeat_payload() -> Payload {
    Payload::Heartbeat(
        Heartbeat {
            server_time: system_time_to_proto(SystemTime::now()).into(),
            ..Default::default()
        }
        .into(),
    )
}

fn to_payload(event: &EventPayload) -> Payload {
    match event.clone() {
        EventPayload::MessageCreated(m) => Payload::MessageCreated(m.into()),
        EventPayload::ChannelCreated(c) => Payload::ChannelCreated(c.into()),
        EventPayload::GuildMemberJoined(j) => Payload::GuildMemberJoined(j.into()),
        EventPayload::GuildUpdated(g) => Payload::GuildUpdated(g.into()),
        EventPayload::ChannelUpdated(c) => Payload::ChannelUpdated(c.into()),
        EventPayload::LayoutUpdated(l) => Payload::LayoutUpdated(l.into()),
        EventPayload::RolesUpdated(r) => Payload::RolesUpdated(r.into()),
        EventPayload::MemberUpdated(m) => Payload::MemberUpdated(m.into()),
        EventPayload::EmojisUpdated(e) => Payload::EmojisUpdated(e.into()),
        EventPayload::ReactionAdded(r) => Payload::ReactionAdded(r.into()),
        EventPayload::ReactionRemoved(r) => Payload::ReactionRemoved(r.into()),
    }
}

fn lagged() -> ConnectError {
    ConnectError::resource_exhausted(
        "event stream fell behind and events were dropped; reconnect and resync via the List* RPCs",
    )
}
