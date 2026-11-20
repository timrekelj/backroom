//! A deterministic backend with seeded data: no network, no randomness, no wall clock. It
//! answers each command with the reply the real server would give (including its errors),
//! queued until the next poll. Scenes and tests also drive it directly: incoming events,
//! connection changes, failing sends.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, UNIX_EPOCH};

use thehideout_api::{
    ApiError, Channel, ChannelIcon, DmChannel, Event, GatewayUpdate, Guild, Message, Page, PaletteColor, User,
};

use crate::backend::{Backend, Command, Initial, Reply, Session};
use crate::fake_lines::{DEV_LINES, DM_ANA_LINES, DM_BRUNO_LINES, GENERAL_LINES};
use crate::format::{utc, DAY};

const PAGE: usize = 50;
const MAX_CHARS: usize = thehideout_api::MAX_MESSAGE_CHARS;

/// The instant the fake world calls "now": Thursday 19 Nov 2026, 11:30 UTC.
pub fn now_unix() -> i64 {
    utc(2026, 11, 19, 11, 30)
}

pub const HIDEOUT: u64 = 1;
pub const GENERAL: u64 = 10;
pub const DEV: u64 = 11;
pub const RANDOM: u64 = 12;
/// Always empty.
pub const PLANS: u64 = 13;
pub const DM_ANA: u64 = 50;
pub const DM_BRUNO: u64 = 51;
pub const TIM: u64 = 1;
pub const ANA: u64 = 2;
pub const BRUNO: u64 = 3;
pub const CVETKA: u64 = 4;

fn time(unix: i64) -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(unix.max(0) as u64)
}

fn user(id: u64, name: &str) -> User {
    User {
        id,
        username: name.into(),
        created_at: time(utc(2026, 1, 5, 12, 0)),
        avatar: None,
        is_admin: false,
    }
}

struct World {
    me: User,
    users: Vec<User>,
    guilds: Vec<Guild>,
    channels: Vec<Channel>,
    dms: Vec<DmChannel>,
    members: HashMap<u64, Vec<u64>>,
    messages: HashMap<u64, Vec<Message>>,
    next_id: u64,
    sent_at: i64,
    queue: Vec<(f64, Reply)>,
    now: f64,
    latency: f64,
    waker: Option<rune::Waker>,
    log: Vec<Command>,
    fail_sends: u32,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

/// A tiny deterministic generator for message gaps.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

impl World {
    fn new() -> World {
        let users = vec![
            user(TIM, "tim"),
            user(ANA, "ana"),
            user(BRUNO, "bruno"),
            user(CVETKA, "cvetka"),
        ];
        let ch = |id: u64, guild: u64, position: i32, name: &str| Channel {
            id,
            guild_id: guild,
            name: name.into(),
            created_at: time(utc(2026, 1, 6, 9, 0)),
            section_id: None,
            position,
            icon: ChannelIcon::default(),
            color: PaletteColor::None,
            topic: String::new(),
        };
        let guild = |id: u64, name: &str, owner: u64, code: &str| Guild {
            id,
            name: name.into(),
            owner_id: owner,
            invite_code: code.into(),
            created_at: time(utc(2026, 1, 6, 9, 0)),
            icon: None,
        };
        let dm = |id: u64, a: u64, b: u64| DmChannel {
            id,
            participants: vec![users[a as usize - 1].clone(), users[b as usize - 1].clone()],
            created_at: time(utc(2026, 3, 2, 18, 0)),
        };
        let mut f = World {
            me: users[0].clone(),
            guilds: vec![
                guild(HIDEOUT, "The Hideout", TIM, "HIDEOUT1"),
                guild(2, "Garage Band", ANA, "GARAGE22"),
                guild(3, "Cooking Club", BRUNO, "COOK333"),
            ],
            channels: vec![
                ch(GENERAL, 1, 0, "general"),
                ch(DEV, 1, 1, "dev"),
                ch(RANDOM, 1, 2, "random"),
                ch(PLANS, 1, 3, "plans"),
                ch(20, 2, 0, "general"),
                ch(21, 2, 1, "gigs"),
                ch(30, 3, 0, "recipes"),
                ch(31, 3, 1, "shopping"),
            ],
            dms: vec![dm(DM_ANA, TIM, ANA), dm(DM_BRUNO, TIM, BRUNO)],
            members: HashMap::from([
                (1, vec![TIM, ANA, BRUNO, CVETKA]),
                (2, vec![TIM, ANA, CVETKA]),
                (3, vec![TIM, BRUNO]),
            ]),
            users,
            messages: HashMap::new(),
            next_id: 10_000,
            sent_at: now_unix(),
            queue: Vec::new(),
            now: 0.0,
            latency: 0.0,
            waker: None,
            log: Vec::new(),
            fail_sends: 0,
        };
        f.seed();
        f
    }

    fn push(&mut self, channel: u64, author: u64, at: i64, content: &str) {
        self.next_id += 1;
        let guild_id = self
            .channels
            .iter()
            .find(|c| c.id == channel)
            .map(|c| c.guild_id);
        let m = Message {
            id: self.next_id,
            channel_id: channel,
            guild_id,
            author_id: author,
            content: content.into(),
            created_at: time(at),
            edited_at: None,
            nonce: String::new(),
        };
        self.messages.entry(channel).or_default().push(m);
    }

    fn seed(&mut self) {
        // #general: 150 messages over four days, authors in runs so groups form.
        const WHO: [u64; 12] = [
            ANA, ANA, BRUNO, TIM, TIM, TIM, CVETKA, ANA, BRUNO, BRUNO, TIM, CVETKA,
        ];
        let mut rng = Lcg(7);
        // Gaps first, so the last day can be laid out backwards and end just before "now".
        let day_of = |i: usize| match i {
            0..40 => -3,
            40..82 => -2,
            82..120 => -1,
            _ => 0,
        };
        let authors: Vec<u64> = (0..150).map(|i| WHO[i % WHO.len()]).collect();
        let gaps: Vec<i64> = (0..150)
            .map(|i| {
                if i % 17 == 0 {
                    5400
                } else if i > 0 && authors[i] == authors[i - 1] {
                    20 + rng.next(130) as i64
                } else {
                    70 + rng.next(800) as i64
                }
            })
            .collect();
        let mut t = 0;
        for i in 0..150usize {
            let d = day_of(i);
            if i == 0 || day_of(i - 1) != d {
                let span: i64 = (i + 1..150)
                    .take_while(|&j| day_of(j) == d)
                    .map(|j| gaps[j])
                    .sum();
                t = if d == 0 {
                    now_unix() - 18 * 60 - span
                } else {
                    utc(2026, 11, 19, 8, 40) + d * DAY + (i as i64 % 7) * 180
                };
            } else {
                t += gaps[i];
            }
            self.push(
                GENERAL,
                authors[i],
                t,
                GENERAL_LINES[(i * 17 + i / 60) % GENERAL_LINES.len()],
            );
        }
        for (i, line) in DEV_LINES.iter().enumerate() {
            let author = [BRUNO, TIM, ANA][i % 3];
            self.push(DEV, author, utc(2026, 11, 18, 14, 0) + i as i64 * 400, line);
        }
        self.push(
            RANDOM,
            CVETKA,
            utc(2026, 11, 17, 20, 12),
            "found this at the flea market, 5 euros",
        );
        for (i, (who, line)) in DM_ANA_LINES.iter().enumerate() {
            self.push(DM_ANA, *who, utc(2026, 11, 18, 21, 0) + i as i64 * 90, line);
        }
        for (i, (who, line)) in DM_BRUNO_LINES.iter().enumerate() {
            self.push(
                DM_BRUNO,
                *who,
                utc(2026, 11, 17, 19, 30) + i as i64 * 120,
                line,
            );
        }
        self.push(
            20,
            ANA,
            utc(2026, 11, 16, 18, 0),
            "rehearsal moved to Thursday",
        );
        self.push(
            30,
            BRUNO,
            utc(2026, 11, 15, 10, 0),
            "sourdough starter is alive",
        );
    }

    // ---- knobs for scenes and tests ----------------------------------------------------------

    /// Seconds each reply waits before it is due (default 0: due at the next poll).
    fn set_latency(&mut self, secs: f64) {
        self.latency = secs;
    }

    /// The next `n` sends are refused with a network error.
    #[cfg_attr(not(test), allow(dead_code))]
    fn set_fail_sends(&mut self, n: u32) {
        self.fail_sends = n;
    }

    /// `author` posts `content` in `channel`, as the gateway would deliver it.
    fn incoming(&mut self, channel: u64, author: u64, content: &str) {
        self.sent_at += 20;
        self.push(channel, author, self.sent_at, content);
        let m = self.messages[&channel].last().cloned().unwrap();
        self.enqueue(Reply::Gateway(GatewayUpdate::Event(Event::MessageCreated(
            m,
        ))));
    }

    /// Like `incoming`, but the gateway never delivers it: the client is away and has to fetch it.
    fn missed(&mut self, channel: u64, author: u64, content: &str) {
        self.sent_at += 20;
        self.push(channel, author, self.sent_at, content);
    }

    fn gateway(&mut self, update: GatewayUpdate) {
        self.enqueue(Reply::Gateway(update));
    }

    fn disconnect(&mut self, retry_secs: u64) {
        self.gateway(GatewayUpdate::Disconnected {
            retry_in: Duration::from_secs(retry_secs),
            reason: "simulated".into(),
        });
    }

    /// One retry that fails: the real gateway announces `Connecting` before each attempt.
    fn failed_retry(&mut self, retry_secs: u64) {
        self.gateway(GatewayUpdate::Connecting);
        self.disconnect(retry_secs);
    }

    /// One retry that succeeds.
    fn reconnect(&mut self) {
        self.gateway(GatewayUpdate::Connecting);
        self.gateway(GatewayUpdate::Connected);
    }

    fn initial(&self) -> Initial {
        let mine: Vec<Guild> = self
            .guilds
            .iter()
            .filter(|g| {
                self.members
                    .get(&g.id)
                    .is_some_and(|m| m.contains(&self.me.id))
            })
            .cloned()
            .collect();
        let channels = self
            .channels
            .iter()
            .filter(|c| mine.iter().any(|g| g.id == c.guild_id))
            .cloned()
            .collect();
        let dms = self
            .dms
            .iter()
            .filter(|d| d.participants.iter().any(|p| p.id == self.me.id))
            .cloned()
            .collect();
        Initial {
            me: self.me.clone(),
            guilds: mine,
            channels,
            dms,
        }
    }

    fn enqueue(&mut self, reply: Reply) {
        self.queue.push((self.now + self.latency, reply));
        if let Some(w) = &self.waker {
            w();
        }
    }

    // ---- commands ----------------------------------------------------------------------------

    fn find_user(&self, name: &str) -> Option<&User> {
        self.users
            .iter()
            .find(|u| u.username.eq_ignore_ascii_case(name))
    }

    fn session_for(&mut self, username: &str) -> Session {
        let me = match self.find_user(username) {
            Some(u) => u.clone(),
            None => {
                let u = user(100 + self.users.len() as u64, username);
                self.users.push(u.clone());
                u
            }
        };
        self.me = me.clone();
        Session {
            token: format!("fake-token-{}", me.id),
            me,
        }
    }

    fn page(&self, channel: u64, before: Option<u64>) -> Page<Message> {
        let all = self.messages.get(&channel).map_or(&[][..], Vec::as_slice);
        let end = before.map_or(all.len(), |b| all.partition_point(|m| m.id < b));
        let start = end.saturating_sub(PAGE);
        let items: Vec<Message> = all[start..end].iter().rev().cloned().collect();
        let has_more = start > 0;
        Page {
            next_before: has_more.then(|| items.last().map(|m| m.id)).flatten(),
            items,
            has_more,
        }
    }

    fn handle(&mut self, cmd: Command) -> Vec<Reply> {
        let me = self.me.id;
        match cmd {
            Command::SignUp { username, password } => {
                let r = if username.chars().count() < 3 {
                    Err(ApiError::InvalidArgument(
                        "username must be at least 3 characters".into(),
                    ))
                } else if username.eq_ignore_ascii_case("taken")
                    || self.find_user(&username).is_some()
                {
                    Err(ApiError::AlreadyExists)
                } else if password.chars().count() < 6 {
                    Err(ApiError::InvalidArgument(
                        "password must be at least 6 characters".into(),
                    ))
                } else {
                    Ok(self.session_for(&username))
                };
                vec![Reply::Auth(r)]
            }
            Command::Login { username, password } => {
                let r = if password == "wrong" {
                    Err(ApiError::Unauthenticated)
                } else {
                    Ok(self.session_for(&username))
                };
                vec![Reply::Auth(r)]
            }
            Command::Resume => vec![Reply::Resume(Ok(None))],
            Command::LoadInitial => vec![
                Reply::Initial(Ok(self.initial())),
                Reply::Gateway(GatewayUpdate::Connected),
            ],
            Command::Refresh => vec![Reply::Refreshed(Ok(self.initial()))],
            Command::LoadChannels { guild } => {
                vec![Reply::Channels {
                    guild,
                    result: Ok(self
                        .channels
                        .iter()
                        .filter(|c| c.guild_id == guild)
                        .cloned()
                        .collect()),
                }]
            }
            Command::LoadMessages { channel, before } => vec![Reply::Messages {
                channel,
                before,
                result: Ok(self.page(channel, before)),
            }],
            Command::Send {
                channel,
                content,
                nonce,
            } => {
                let result = if self.fail_sends > 0 {
                    self.fail_sends -= 1;
                    Err(ApiError::Network("simulated outage".into()))
                } else if content.trim().is_empty() || content.chars().count() > MAX_CHARS {
                    Err(ApiError::InvalidArgument(
                        "message must be 1 to 4000 characters".into(),
                    ))
                } else {
                    self.sent_at += 15;
                    self.push(channel, me, self.sent_at, &content);
                    let m = self
                        .messages
                        .get_mut(&channel)
                        .and_then(|v| v.last_mut())
                        .unwrap();
                    m.nonce = nonce.clone();
                    Ok(m.clone())
                };
                // The server answers the sender and broadcasts to everyone, the sender included.
                let echo = result
                    .clone()
                    .ok()
                    .map(|m| Reply::Gateway(GatewayUpdate::Event(Event::MessageCreated(m))));
                std::iter::once(Reply::Sent {
                    channel,
                    nonce,
                    result,
                })
                .chain(echo)
                .collect()
            }
            Command::CreateGuild { name } => {
                let n = name.chars().count();
                if !(2..=32).contains(&n) {
                    return vec![Reply::GuildCreated(Err(ApiError::InvalidArgument(
                        "name must be 2 to 32 characters".into(),
                    )))];
                }
                let id = 100 + self.guilds.len() as u64;
                let g = Guild {
                    id,
                    name,
                    owner_id: me,
                    invite_code: format!("INVITE{id}"),
                    created_at: time(now_unix()),
                    icon: None,
                };
                self.guilds.push(g.clone());
                self.channels.push(Channel {
                    id: id * 10,
                    guild_id: id,
                    name: "general".into(),
                    created_at: time(now_unix()),
                    section_id: None,
                    position: 0,
                    icon: ChannelIcon::default(),
                    color: PaletteColor::None,
                    topic: String::new(),
                });
                self.members.insert(id, vec![me]);
                vec![Reply::GuildCreated(Ok(g))]
            }
            Command::JoinGuild { code } => {
                let found = self
                    .guilds
                    .iter()
                    .find(|g| g.invite_code.eq_ignore_ascii_case(code.trim()))
                    .cloned();
                let r = match found {
                    None => Err(ApiError::NotFound),
                    Some(g) if self.members.get(&g.id).is_some_and(|m| m.contains(&me)) => {
                        Err(ApiError::AlreadyExists)
                    }
                    Some(g) => {
                        self.members.entry(g.id).or_default().push(me);
                        Ok(g)
                    }
                };
                vec![Reply::GuildJoined(r)]
            }
            Command::CreateChannel { guild, name } => {
                let owner = self
                    .guilds
                    .iter()
                    .find(|g| g.id == guild)
                    .map(|g| g.owner_id);
                let r = match owner {
                    None => Err(ApiError::NotFound),
                    Some(o) if o != me => Err(ApiError::PermissionDenied),
                    Some(_)
                        if self
                            .channels
                            .iter()
                            .any(|c| c.guild_id == guild && c.name == name) =>
                    {
                        Err(ApiError::AlreadyExists)
                    }
                    Some(_) => {
                        let c = Channel {
                            id: 1000 + self.channels.len() as u64,
                            guild_id: guild,
                            name,
                            created_at: time(now_unix()),
                            section_id: None,
                            position: self.channels.iter().filter(|c| c.guild_id == guild).count() as i32,
                            icon: ChannelIcon::default(),
                            color: PaletteColor::None,
                            topic: String::new(),
                        };
                        self.channels.push(c.clone());
                        Ok(c)
                    }
                };
                vec![Reply::ChannelCreated(r)]
            }
            Command::LoadMembers { guild } => {
                let ids = self.members.get(&guild).cloned().unwrap_or_default();
                let users = ids
                    .iter()
                    .filter_map(|i| self.users.iter().find(|u| u.id == *i).cloned())
                    .collect();
                vec![Reply::Members {
                    guild,
                    result: Ok(users),
                }]
            }
            Command::OpenDm { user_id } => {
                let existing = self
                    .dms
                    .iter()
                    .find(|d| {
                        d.participants.iter().any(|p| p.id == user_id)
                            && d.participants.iter().any(|p| p.id == me)
                    })
                    .cloned();
                let r = match (
                    existing,
                    self.users.iter().find(|u| u.id == user_id).cloned(),
                ) {
                    (Some(d), _) => Ok(d),
                    (None, None) => Err(ApiError::NotFound),
                    (None, Some(other)) => {
                        let mut participants = vec![self.me.clone(), other];
                        participants.sort_by_key(|u| u.id);
                        let d = DmChannel {
                            id: 500 + self.dms.len() as u64,
                            participants,
                            created_at: time(now_unix()),
                        };
                        self.dms.push(d.clone());
                        Ok(d)
                    }
                };
                vec![Reply::DmOpened(r)]
            }
            Command::SignOut => vec![Reply::SignedOut],
        }
    }
}

impl World {
    fn submit(&mut self, cmd: Command) {
        self.log.push(cmd.clone());
        for r in self.handle(cmd) {
            self.enqueue(r);
        }
    }

    fn poll_due(&mut self, now: f64) -> Vec<Reply> {
        self.now = now;
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.queue)
            .into_iter()
            .partition(|(t, _)| *t <= now);
        self.queue = later;
        due.into_iter().map(|(_, r)| r).collect()
    }

    fn due(&self) -> Option<f64> {
        self.queue.iter().map(|(t, _)| *t).reduce(f64::min)
    }
}

/// A handle to the fake world. Clones share it, so a scene or test keeps one to drive incoming
/// events while the app owns the boxed [`Backend`].
#[derive(Clone, Default)]
pub struct FakeBackend(Rc<RefCell<World>>);

impl FakeBackend {
    pub fn new() -> FakeBackend {
        FakeBackend::default()
    }

    /// Seconds each reply waits before it is due (default 0: due at the next poll).
    pub fn with_latency(self, secs: f64) -> Self {
        self.0.borrow_mut().set_latency(secs);
        self
    }

    /// Changes the reply latency of a shared handle.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_latency(&self, secs: f64) {
        self.0.borrow_mut().set_latency(secs);
    }

    /// The next `n` sends are refused with a network error.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fail_sends(&self, n: u32) {
        self.0.borrow_mut().set_fail_sends(n);
    }

    /// `author` posts `content` in `channel`, as the gateway would deliver it.
    pub fn incoming(&self, channel: u64, author: u64, content: &str) {
        self.0.borrow_mut().incoming(channel, author, content);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn gateway(&self, update: GatewayUpdate) {
        self.0.borrow_mut().gateway(update);
    }

    /// The gateway drops; it will retry in `retry_secs`.
    pub fn disconnect(&self, retry_secs: u64) {
        self.0.borrow_mut().disconnect(retry_secs);
    }

    /// `author` posts in `channel` while the client is away: only a fetch finds it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn missed(&self, channel: u64, author: u64, content: &str) {
        self.0.borrow_mut().missed(channel, author, content);
    }

    /// A retry that fails: `Connecting`, then `Disconnected` again.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn failed_retry(&self, retry_secs: u64) {
        self.0.borrow_mut().failed_retry(retry_secs);
    }

    /// A retry that works: `Connecting`, then `Connected`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn reconnect(&self) {
        self.0.borrow_mut().reconnect();
    }
}

impl Backend for FakeBackend {
    fn submit(&mut self, cmd: Command) {
        self.0.borrow_mut().submit(cmd);
    }

    fn poll(&mut self, now: f64) -> Vec<Reply> {
        self.0.borrow_mut().poll_due(now)
    }

    fn next_due(&self) -> Option<f64> {
        self.0.borrow().due()
    }

    fn set_waker(&mut self, waker: rune::Waker) {
        self.0.borrow_mut().waker = Some(waker);
    }

    fn sent(&self) -> Vec<Command> {
        self.0.borrow().log.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: &mut World, cmd: Command) -> Vec<Reply> {
        f.submit(cmd);
        f.poll_due(1.0)
    }

    #[test]
    fn seed_is_deterministic_and_busy() {
        let (a, b) = (World::new(), World::new());
        assert_eq!(a.messages[&GENERAL], b.messages[&GENERAL]);
        assert_eq!(a.messages[&GENERAL].len(), 150);
        assert!(a.messages[&GENERAL]
            .windows(2)
            .all(|w| w[0].id < w[1].id && w[0].created_at <= w[1].created_at));
        assert!(!a.messages.contains_key(&PLANS));
    }

    #[test]
    fn pages_walk_back_to_the_start() {
        let f = World::new();
        let first = f.page(GENERAL, None);
        assert_eq!((first.items.len(), first.has_more), (PAGE, true));
        let mut before = first.next_before;
        let mut total = first.items.len();
        while let Some(b) = before {
            let p = f.page(GENERAL, Some(b));
            total += p.items.len();
            before = p.next_before;
        }
        assert_eq!(total, 150);
    }

    #[test]
    fn auth_rules() {
        let mut f = World::new();
        let r = run(
            &mut f,
            Command::Login {
                username: "tim".into(),
                password: "wrong".into(),
            },
        );
        assert_eq!(r, vec![Reply::Auth(Err(ApiError::Unauthenticated))]);
        let r = run(
            &mut f,
            Command::SignUp {
                username: "taken".into(),
                password: "secret1".into(),
            },
        );
        assert_eq!(r, vec![Reply::Auth(Err(ApiError::AlreadyExists))]);
        let r = run(
            &mut f,
            Command::Login {
                username: "tim".into(),
                password: "anything".into(),
            },
        );
        assert!(matches!(&r[0], Reply::Auth(Ok(s)) if s.me.id == TIM));
    }

    #[test]
    fn a_send_answers_with_the_reply_and_an_echo_carrying_the_nonce() {
        let mut f = World::new();
        let r = run(
            &mut f,
            Command::Send {
                channel: GENERAL,
                content: "hi".into(),
                nonce: "n1".into(),
            },
        );
        assert_eq!(r.len(), 2);
        assert!(matches!(&r[0], Reply::Sent { result: Ok(m), .. } if m.nonce == "n1"));
        assert!(
            matches!(&r[1], Reply::Gateway(GatewayUpdate::Event(Event::MessageCreated(m))) if m.nonce == "n1")
        );
    }

    #[test]
    fn latency_defers_replies() {
        let mut f = World::new();
        f.set_latency(0.5);
        f.submit(Command::SignOut);
        assert!(f.poll_due(0.2).is_empty());
        assert_eq!(f.due(), Some(0.5));
        assert_eq!(f.poll_due(0.6).len(), 1);
    }
}
