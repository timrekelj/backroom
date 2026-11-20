//! What the UI asks for and what comes back. The UI only pushes [`Command`]s and applies
//! [`Reply`]s (see `apply.rs`); a [`Backend`] turns one into the other. The fake backend in
//! `fake.rs` answers deterministically; `real.rs` talks to the server through `thehideout-api`.

use thehideout_api::{ApiError, Channel, DmChannel, GatewayUpdate, Guild, Message, Page, User};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SignUp {
        username: String,
        password: String,
    },
    Login {
        username: String,
        password: String,
    },
    /// Me, guilds, DMs and every guild's channels, in one answer.
    LoadInitial,
    /// `LoadInitial` again after a gateway gap: same data, but it must not reset the UI.
    Refresh,
    LoadChannels {
        guild: u64,
    },
    /// Newest page when `before` is `None`, else the page older than that message id.
    LoadMessages {
        channel: u64,
        before: Option<u64>,
    },
    Send {
        channel: u64,
        content: String,
        nonce: String,
    },
    CreateGuild {
        name: String,
    },
    JoinGuild {
        code: String,
    },
    CreateChannel {
        guild: u64,
        name: String,
    },
    LoadMembers {
        guild: u64,
    },
    OpenDm {
        user_id: u64,
    },
    SignOut,
    /// Launch: validate the saved session, if there is one.
    Resume,
}

/// A signed-in session: the token the server issued and who it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub token: String,
    pub me: User,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Initial {
    pub me: User,
    pub guilds: Vec<Guild>,
    pub channels: Vec<Channel>,
    pub dms: Vec<DmChannel>,
}

type R<T> = Result<T, ApiError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Answer to `SignUp` and `Login`.
    Auth(R<Session>),
    Initial(R<Initial>),
    /// Answer to `Refresh`.
    Refreshed(R<Initial>),
    Channels {
        guild: u64,
        result: R<Vec<Channel>>,
    },
    Messages {
        channel: u64,
        before: Option<u64>,
        result: R<Page<Message>>,
    },
    Sent {
        channel: u64,
        nonce: String,
        result: R<Message>,
    },
    GuildCreated(R<Guild>),
    GuildJoined(R<Guild>),
    ChannelCreated(R<Channel>),
    Members {
        guild: u64,
        result: R<Vec<User>>,
    },
    DmOpened(R<DmChannel>),
    SignedOut,
    /// Answer to `Resume`: no saved session, a valid one (its server URL and the session), or why
    /// it couldn't be used (`Unauthenticated`: the token was refused and is forgotten).
    Resume(R<Option<(String, Session)>>),
    Gateway(GatewayUpdate),
}

pub trait Backend {
    fn submit(&mut self, cmd: Command);
    /// Replies that are due at `now` (the app clock, seconds). Called from `poll`.
    fn poll(&mut self, now: f64) -> Vec<Reply>;
    /// When the next queued reply is due, so the app can schedule a poll.
    fn next_due(&self) -> Option<f64> {
        None
    }
    /// Called once with the shell's waker: wake it whenever a reply becomes available.
    fn set_waker(&mut self, _waker: rune::Waker) {}
    /// The server the next sign-in talks to (the auth form's field; empty: the default).
    fn set_server(&mut self, _url: &str) {}
    /// Every command submitted so far (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    fn sent(&self) -> Vec<Command> {
        Vec::new()
    }
}
