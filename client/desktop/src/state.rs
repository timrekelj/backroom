//! `AppState`: everything the UI draws and every mode it can be in. Plain data and pure
//! transitions: handlers mutate it and push [`Command`]s on `outbox`; replies come back through
//! `apply::apply`. Nothing here touches Rune's runtime, so it is unit-testable.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::SystemTime;

use rune::TextEdit;
use thehideout_api::{Channel, ChannelIcon, DmChannel, Guild, GuildLayout, MAX_MESSAGE_CHARS, Message, PaletteColor, Permissions, Role, User};
use thehideout_design::components::{ConnectionStatus, ToastKind};

use crate::backend::{Command, Session};
pub use crate::forms::{Auth, AuthError, AuthField, AuthMode, ChannelForm, DialogKind, DialogState, Phase, auth_error, check_name, sentence};
use crate::layout::{self, Item};
use crate::format::{Tz, unix};
use crate::look::Look;

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
/// How long a loading placeholder waits before it shows, so a fast load never flashes it.
pub const PLACEHOLDER_DELAY: f64 = 0.15;
const TOAST_SECS: f64 = 3.5;
const TOAST_QUEUE: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Launch with a saved session to check: neither the form nor the shell yet.
    Loading,
    Auth,
    Main,
}


// ---- data ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fetch {
    Idle,
    /// The first page. `since` is stamped by the next poll (the clock a handler sees can be stale)
    /// and `shown` flips once the load has outlasted [`PLACEHOLDER_DELAY`].
    Initial { since: Option<f64>, shown: bool },
    Older,
}

/// One channel's loaded history, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelLog {
    pub messages: Vec<Message>,
    pub oldest_id: Option<u64>,
    pub has_more: bool,
    pub loaded: bool,
    pub fetch: Fetch,
    /// Ids of messages that start a new group no matter who wrote the one before: the oldest
    /// message of each page that older pages were later loaded above. A row that merged into an
    /// older group when the page arrived would change its place in the tree, and Rune holds the
    /// reader's row by that place.
    pub breaks: Vec<u64>,
    /// Loaded before the gateway dropped, and not refetched since: the next visit asks for the
    /// newest page again and merges it.
    pub stale: bool,
}

impl Default for ChannelLog {
    fn default() -> Self {
        ChannelLog { messages: Vec::new(), oldest_id: None, has_more: false, loaded: false, fetch: Fetch::Idle, breaks: Vec::new(), stale: false }
    }
}

impl ChannelLog {
    pub fn newest_id(&self) -> Option<u64> {
        self.messages.last().map(|m| m.id)
    }

    /// Adds `m` in id order unless it is already here (the RPC reply and the gateway echo both
    /// bring a sent message). Returns whether the log changed.
    pub fn insert(&mut self, m: Message) -> bool {
        match self.messages.binary_search_by_key(&m.id, |x| x.id) {
            Ok(_) => false,
            Err(at) => {
                self.messages.insert(at, m);
                self.oldest_id = self.messages.first().map(|m| m.id);
                true
            }
        }
    }

    /// Merges a page (newest first, as the API returns it).
    pub fn insert_page(&mut self, mut items: Vec<Message>) {
        items.sort_by_key(|m| m.id);
        for m in items {
            self.insert(m);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Pending,
    Failed,
}

/// A message this client sent that the server hasn't confirmed yet (or refused).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub nonce: String,
    pub channel: u64,
    pub content: String,
    pub at: SystemTime,
    pub state: Delivery,
}

#[derive(Default)]
pub struct Data {
    pub guilds: Vec<Guild>,
    pub channels: HashMap<u64, Vec<Channel>>,
    pub dms: Vec<DmChannel>,
    pub users: HashMap<u64, User>,
    /// Member ids per guild, once loaded.
    pub members: HashMap<u64, Vec<u64>>,
    /// Guilds whose members were asked for and not yet refused, so one question is ever in flight.
    pub members_asked: HashSet<u64>,
    pub logs: HashMap<u64, ChannelLog>,
    pub outgoing: Vec<Outgoing>,
    /// Each guild's sections and channel order.
    pub layouts: HashMap<u64, GuildLayout>,
    pub roles: HashMap<u64, Vec<Role>>,
    /// Role ids per member per guild.
    pub member_roles: HashMap<u64, HashMap<u64, Vec<u64>>>,
    /// Guilds whose access was asked for and not yet refused.
    pub access_asked: HashSet<u64>,
    /// The layout as it was before an optimistic edit that the server hasn't answered. While one
    /// is out, further structure edits wait (they would be based on a version the server is about to retire).
    pub layout_undo: HashMap<u64, GuildLayout>,
}

// ---- ui --------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Home: the direct-message list.
    Home,
    Guild(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Unread {
    pub count: u32,
    pub mentions: u32,
}

/// Where the "New" divider sits in the channel being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    None,
    /// Entered with `unread` new messages before the history arrived.
    Pending { channel: u64, unread: u32 },
    /// The divider goes after message `id` (`None`: before the first).
    After { channel: u64, id: Option<u64> },
}

/// A page of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Appearance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    None,
    /// The rail's "+": create or join a server.
    Add,
    /// The sidebar header's menu.
    Guild,
    /// The user strip's gear: settings and sign out.
    User,
    /// A channel's menu (the row's "..." button, or a right-click at `at`).
    Channel { id: u64, at: Option<(i32, i32)> },
    /// A section header's menu.
    Section { id: u64, at: Option<(i32, i32)> },
    /// The channel menu's "Move to section" submenu.
    MoveTo { id: u64, at: Option<(i32, i32)> },
}

/// A sidebar row held by the pointer: dragged `dy` points from where it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub guild: u64,
    pub item: Item,
    pub dy: f32,
}

/// Points between rows (32 for a row, 2 for the gap): what a drag of `dy` points steps over.
pub const ROW_PITCH: f32 = 34.0;

/// Whether the event stream has been whole since the last `Connected`. The gateway doesn't
/// replay what it missed, and `ConnectionStatus` can't say: `Connecting` arrives before every
/// retry and overwrites `Reconnecting`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Live,
    /// A `Disconnected` came after the last `Connected`: events may have been missed.
    Interrupted,
}

pub struct ToastEntry {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
    /// Stamped by the poll that first shows it.
    pub until: Option<f64>,
}

pub struct Ui {
    pub place: Place,
    pub guild_channel: HashMap<u64, u64>,
    pub home_dm: Option<u64>,
    pub unread: HashMap<u64, Unread>,
    pub marker: Marker,
    pub dialog: Option<DialogState>,
    pub menu: Menu,
    pub toasts: VecDeque<ToastEntry>,
    pub members_open: bool,
    pub composers: HashMap<u64, TextEdit>,
    /// The field shown while no channel is open.
    pub idle_composer: TextEdit,
    pub connection: ConnectionStatus,
    pub stream: Stream,
    pub at_bottom: bool,
    /// Messages that arrived below the fold while the reader was scrolled up.
    pub unseen_below: u32,
    pub dms_open: bool,
    /// Sections the user folded, `(guild, section)`; saved to the prefs (`collapsed_dirty`).
    pub collapsed: HashSet<(u64, u64)>,
    pub collapsed_dirty: bool,
    pub drag: Option<Drag>,
    /// The saved look (accent, background, appearance, tint, contrast, reduce motion). Scenes and
    /// tests force an appearance through it.
    pub look: Look,
    /// A change to `look` the saver has not been given yet (`App::flush` hands it over).
    pub look_dirty: bool,
    /// The settings page that replaces the shell, when open.
    pub settings: Option<Section>,
    /// Text for the next poll to put on the clipboard (a press handler has no `Tx`).
    pub copy_request: Option<String>,
    pub toast_seq: u64,
}

impl Ui {
    fn new() -> Ui {
        Ui {
            place: Place::Home,
            guild_channel: HashMap::new(),
            home_dm: None,
            unread: HashMap::new(),
            marker: Marker::None,
            dialog: None,
            menu: Menu::None,
            toasts: VecDeque::new(),
            members_open: false,
            composers: HashMap::new(),
            idle_composer: TextEdit::new(""),
            connection: ConnectionStatus::Connecting,
            stream: Stream::Live,
            at_bottom: true,
            unseen_below: 0,
            dms_open: true,
            collapsed: HashSet::new(),
            collapsed_dirty: false,
            drag: None,
            look: Look::default(),
            look_dirty: false,
            settings: None,
            copy_request: None,
            toast_seq: 0,
        }
    }
}

pub struct AppState {
    pub screen: Screen,
    pub auth: Auth,
    pub session: Option<Session>,
    pub data: Data,
    pub ui: Ui,
    pub outbox: Vec<Command>,
    /// The app clock as of the last poll.
    pub clock: f64,
    /// The wall-clock day number "Today" means.
    pub today: i64,
    pub tz: Tz,
    /// The wall clock; scenes and tests swap in a fixed one.
    pub now: fn() -> SystemTime,
    nonce: u64,
}

impl AppState {
    pub fn new(now: fn() -> SystemTime, tz: Tz) -> AppState {
        AppState {
            screen: Screen::Auth,
            auth: Auth::new(DEFAULT_SERVER),
            session: None,
            data: Data::default(),
            ui: Ui::new(),
            outbox: Vec::new(),
            clock: 0.0,
            today: crate::format::day_of(tz, unix(now())),
            tz,
            now,
            nonce: 0,
        }
    }

    /// The URL in the "Server" field.
    pub fn server_url(&self) -> String {
        self.auth.server.text.trim().to_owned()
    }

    /// Back to a fresh signed-out app, keeping the server URL and the look.
    pub fn reset(&mut self, error: Option<AuthError>) {
        let (server, look, today, tz, now) = (self.server_url(), self.ui.look, self.today, self.tz, self.now);
        let collapsed = std::mem::take(&mut self.ui.collapsed);
        *self = AppState::new(now, tz);
        self.ui.collapsed = collapsed;
        self.today = today;
        self.auth = Auth::new(&server);
        self.auth.error = error;
        self.ui.look = look;
    }

    // ---- queries -----------------------------------------------------------------------------

    pub fn me(&self) -> Option<&User> {
        self.session.as_ref().map(|s| &s.me)
    }

    pub fn my_id(&self) -> u64 {
        self.me().map_or(0, |u| u.id)
    }

    pub fn user_name(&self, id: u64) -> String {
        self.data.users.get(&id).map_or_else(|| format!("user {id}"), |u| u.username.clone())
    }

    pub fn guild(&self, id: u64) -> Option<&Guild> {
        self.data.guilds.iter().find(|g| g.id == id)
    }

    pub fn channels_of(&self, guild: u64) -> &[Channel] {
        self.data.channels.get(&guild).map_or(&[], Vec::as_slice)
    }

    /// A guild's channels in sidebar order: the layout's, then any the layout doesn't list yet.
    pub fn ordered_channels(&self, guild: u64) -> Vec<&Channel> {
        let all = self.channels_of(guild);
        let Some(l) = self.data.layouts.get(&guild) else { return all.iter().collect() };
        let mut out: Vec<&Channel> = layout::channel_order(l).into_iter().filter_map(|id| all.iter().find(|c| c.id == id)).collect();
        let unlisted: Vec<&Channel> = all.iter().filter(|c| !out.iter().any(|o| o.id == c.id)).collect();
        out.extend(unlisted);
        out
    }

    pub fn channel(&self, guild: u64, id: u64) -> Option<&Channel> {
        self.channels_of(guild).iter().find(|c| c.id == id)
    }

    /// What the signed-in user may do in `guild` (nothing until its roles have arrived, unless they own it).
    pub fn perms(&self, guild: u64) -> Permissions {
        let Some(g) = self.guild(guild) else { return Permissions::NONE };
        let mine = self.data.member_roles.get(&guild).and_then(|m| m.get(&self.my_id())).map_or(&[][..], Vec::as_slice);
        crate::perms::effective(g, self.my_id(), mine, self.data.roles.get(&guild).map_or(&[][..], Vec::as_slice))
    }

    /// Whether the structure controls (drag, menus, create section, settings) are offered.
    pub fn can_manage_channels(&self, guild: u64) -> bool {
        self.perms(guild).contains(Permissions::MANAGE_CHANNELS)
    }

    pub fn section_open(&self, guild: u64, section: u64) -> bool {
        !self.ui.collapsed.contains(&(guild, section))
    }

    pub fn dm(&self, id: u64) -> Option<&DmChannel> {
        self.data.dms.iter().find(|d| d.id == id)
    }

    /// The other participant of a DM.
    pub fn dm_peer<'a>(&self, dm: &'a DmChannel) -> Option<&'a User> {
        let me = self.my_id();
        dm.participants.iter().find(|u| u.id != me).or(dm.participants.first())
    }

    /// The channel (or DM) being read.
    pub fn current_channel(&self) -> Option<u64> {
        match self.ui.place {
            Place::Home => self.ui.home_dm,
            Place::Guild(g) => self.ui.guild_channel.get(&g).copied(),
        }
    }

    pub fn current_guild(&self) -> Option<&Guild> {
        match self.ui.place {
            Place::Guild(g) => self.guild(g),
            Place::Home => None,
        }
    }

    /// The open guild channel, whole (for its icon, colour and topic).
    pub fn current_guild_channel(&self) -> Option<&Channel> {
        match self.ui.place {
            Place::Guild(g) => self.current_channel().and_then(|c| self.channel(g, c)),
            Place::Home => None,
        }
    }

    pub fn current_channel_name(&self) -> Option<String> {
        let id = self.current_channel()?;
        match self.ui.place {
            Place::Home => self.dm(id).and_then(|d| self.dm_peer(d)).map(|u| u.username.clone()),
            Place::Guild(g) => self.channels_of(g).iter().find(|c| c.id == id).map(|c| c.name.clone()),
        }
    }

    pub fn log(&self, channel: u64) -> Option<&ChannelLog> {
        self.data.logs.get(&channel)
    }

    pub fn unread_of(&self, channel: u64) -> Unread {
        self.ui.unread.get(&channel).copied().unwrap_or_default()
    }

    pub fn guild_unread(&self, guild: u64) -> Unread {
        self.channels_of(guild).iter().fold(Unread::default(), |a, c| {
            let u = self.unread_of(c.id);
            Unread { count: a.count + u.count, mentions: a.mentions + u.mentions }
        })
    }

    pub fn home_unread(&self) -> Unread {
        self.data.dms.iter().fold(Unread::default(), |a, d| {
            let u = self.unread_of(d.id);
            Unread { count: a.count + u.count, mentions: a.mentions + u.mentions }
        })
    }

    /// Everyone the user could start a DM with: members of their servers and DM partners.
    pub fn known_users(&self) -> Vec<&User> {
        let me = self.my_id();
        let mut v: Vec<&User> = self.data.users.values().filter(|u| u.id != me).collect();
        v.sort_by_key(|u| u.username.to_lowercase());
        v
    }

    /// Whether the composer may send: the gateway is up (or still coming up).
    pub fn composer_block(&self) -> Option<String> {
        match self.ui.connection {
            ConnectionStatus::Reconnecting(_) => Some("Reconnecting\u{2026} you can send again once you're back online".into()),
            ConnectionStatus::Offline => Some("You're offline. Messages can't be sent right now".into()),
            ConnectionStatus::Connected | ConnectionStatus::Connecting => None,
        }
    }

    /// The composer's field for the open channel (drafts are kept per channel).
    pub fn composer_mut(&mut self) -> &mut TextEdit {
        match self.current_channel() {
            Some(c) => self.ui.composers.entry(c).or_insert_with(|| TextEdit::new("")),
            None => &mut self.ui.idle_composer,
        }
    }

    pub fn composer(&self) -> &TextEdit {
        self.current_channel().and_then(|c| self.ui.composers.get(&c)).unwrap_or(&self.ui.idle_composer)
    }

    // ---- settings ----------------------------------------------------------------------------

    pub fn open_settings(&mut self) {
        self.ui.menu = Menu::None;
        self.ui.settings = Some(Section::Appearance);
    }

    pub fn close_settings(&mut self) {
        self.ui.settings = None;
    }

    /// Changes the look; a real change is queued for the saver.
    pub fn edit_look(&mut self, f: impl FnOnce(&mut Look)) {
        let before = self.ui.look;
        f(&mut self.ui.look);
        if self.ui.look != before {
            self.ui.look_dirty = true;
        }
    }

    // ---- navigation --------------------------------------------------------------------------

    pub fn go_home(&mut self) {
        self.ui.menu = Menu::None;
        if self.ui.place == Place::Home {
            return;
        }
        self.ui.place = Place::Home;
        self.arrive();
    }

    pub fn go_guild(&mut self, id: u64) {
        self.ui.menu = Menu::None;
        if self.ui.place == Place::Guild(id) || self.guild(id).is_none() {
            return;
        }
        self.ui.place = Place::Guild(id);
        if !self.ui.guild_channel.contains_key(&id)
            && let Some(first) = self.ordered_channels(id).first().map(|c| c.id)
        {
            self.ui.guild_channel.insert(id, first);
        }
        self.want_members(id);
        self.want_access(id);
        if !self.data.channels.contains_key(&id) {
            self.outbox.push(Command::LoadChannels { guild: id });
        }
        self.arrive();
    }

    /// Asks for a guild's members unless they are loaded or already asked for.
    pub fn want_members(&mut self, guild: u64) {
        if !self.data.members.contains_key(&guild) && self.data.members_asked.insert(guild) {
            self.outbox.push(Command::LoadMembers { guild });
        }
    }

    /// Asks for a guild's roles and role assignments unless that is already done or asked.
    pub fn want_access(&mut self, guild: u64) {
        if !self.data.roles.contains_key(&guild) && self.data.access_asked.insert(guild) {
            self.outbox.push(Command::LoadAccess { guild });
        }
    }

    /// Asks for the members of the guild `m` was posted in when its author is a stranger.
    pub fn want_author(&mut self, m: &Message) {
        if let Some(guild) = m.guild_id
            && !self.data.users.contains_key(&m.author_id)
        {
            self.want_members(guild);
        }
    }

    /// Opens `channel` in the current place.
    pub fn select_channel(&mut self, channel: u64) {
        if self.current_channel() == Some(channel) {
            return;
        }
        match self.ui.place {
            Place::Home => self.ui.home_dm = Some(channel),
            Place::Guild(g) => {
                self.ui.guild_channel.insert(g, channel);
            }
        }
        self.arrive();
    }

    /// Alt+Up / Alt+Down: the previous or next channel (or DM) in the sidebar.
    pub fn step_channel(&mut self, delta: i32) {
        let ids: Vec<u64> = match self.ui.place {
            Place::Home => self.data.dms.iter().map(|d| d.id).collect(),
            Place::Guild(g) => self.ordered_channels(g).iter().map(|c| c.id).collect(),
        };
        if ids.is_empty() {
            return;
        }
        let at = self.current_channel().and_then(|c| ids.iter().position(|&i| i == c));
        let next = match at {
            Some(i) => (i as i32 + delta).clamp(0, ids.len() as i32 - 1) as usize,
            None => 0,
        };
        self.select_channel(ids[next]);
    }

    /// The reader just arrived at the current channel: fix the "New" divider, clear its unread
    /// count and fetch its history if it hasn't been.
    fn arrive(&mut self) {
        self.ui.at_bottom = true;
        self.ui.unseen_below = 0;
        self.ui.marker = Marker::None;
        let Some(channel) = self.current_channel() else { return };
        let unread = self.ui.unread.remove(&channel).filter(|u| u.count > 0);
        let log = self.data.logs.entry(channel).or_default();
        if let Some(u) = unread {
            self.ui.marker = if log.loaded {
                let at = log.messages.len().checked_sub(u.count as usize + 1);
                Marker::After { channel, id: at.map(|i| log.messages[i].id) }
            } else {
                Marker::Pending { channel, unread: u.count }
            };
        }
        if !log.loaded && log.fetch == Fetch::Idle {
            log.fetch = Fetch::Initial { since: None, shown: false };
            self.outbox.push(Command::LoadMessages { channel, before: None });
        } else if log.loaded && log.stale {
            // Keeps what is shown; the newest page merges in when it lands.
            log.stale = false;
            self.outbox.push(Command::LoadMessages { channel, before: None });
        }
    }

    /// The gateway is back after a gap and doesn't replay: fetch the lists again, the open
    /// channel's newest page now, and every other loaded channel's on its next visit. Nothing
    /// is cleared and the selection stays.
    pub fn resync(&mut self) {
        self.outbox.push(Command::Refresh);
        let current = self.current_channel();
        for (&id, log) in &mut self.data.logs {
            log.stale = log.loaded && Some(id) != current;
        }
        if let Some(channel) = current {
            self.outbox.push(Command::LoadMessages { channel, before: None });
        }
    }

    /// Tries the open channel's first page again after a failed load.
    pub fn reload_current(&mut self) {
        let Some(channel) = self.current_channel() else { return };
        let log = self.data.logs.entry(channel).or_default();
        if !log.loaded && log.fetch == Fetch::Idle {
            log.fetch = Fetch::Initial { since: None, shown: false };
            self.outbox.push(Command::LoadMessages { channel, before: None });
        }
    }

    /// The reply button: starts the draft with an @mention of the message's author.
    pub fn reply_to(&mut self, chat_id: &str) {
        let Some(channel) = self.current_channel() else { return };
        let me = self.my_id();
        let author = self.log(channel).and_then(|l| l.messages.iter().find(|m| chat_id_of(m, me) == chat_id)).map(|m| m.author_id);
        if let Some(author) = author {
            let name = self.user_name(author);
            let edit = self.composer_mut();
            let draft = format!("@{name} {}", edit.text);
            edit.replace_text(&draft);
            edit.focused = true;
        }
    }

    /// The copy button: the message text goes to the clipboard on the next poll.
    pub fn copy_message(&mut self, chat_id: &str) {
        let Some(channel) = self.current_channel() else { return };
        let me = self.my_id();
        let found = self.log(channel).and_then(|l| l.messages.iter().find(|m| chat_id_of(m, me) == chat_id)).map(|m| m.content.clone());
        let found = found.or_else(|| self.data.outgoing.iter().find(|o| format!("n:{}", o.nonce) == chat_id).map(|o| o.content.clone()));
        self.ui.copy_request = found;
    }

    /// Asks for the page older than what's loaded, once.
    pub fn load_older(&mut self, channel: u64) {
        let Some(log) = self.data.logs.get_mut(&channel) else { return };
        if !log.loaded || !log.has_more || log.fetch != Fetch::Idle {
            return;
        }
        log.fetch = Fetch::Older;
        self.outbox.push(Command::LoadMessages { channel, before: log.oldest_id });
    }

    // ---- sending -----------------------------------------------------------------------------

    /// Queues `text` as a pending message in the open channel. False (and nothing queued) when
    /// there is no channel or the text is over the limit.
    pub fn send(&mut self, text: &str) -> bool {
        let content = text.trim();
        let Some(channel) = self.current_channel() else { return false };
        if content.is_empty() || content.chars().count() > MAX_MESSAGE_CHARS {
            return false;
        }
        self.nonce += 1;
        let nonce = format!("{}-{}", self.my_id(), self.nonce);
        self.data.outgoing.push(Outgoing { nonce: nonce.clone(), channel, content: content.to_owned(), at: (self.now)(), state: Delivery::Pending });
        self.outbox.push(Command::Send { channel, content: content.to_owned(), nonce });
        true
    }

    pub fn retry(&mut self, nonce: &str) {
        if let Some(o) = self.data.outgoing.iter_mut().find(|o| o.nonce == nonce && o.state == Delivery::Failed) {
            o.state = Delivery::Pending;
            self.outbox.push(Command::Send { channel: o.channel, content: o.content.clone(), nonce: o.nonce.clone() });
        }
    }

    // ---- auth --------------------------------------------------------------------------------

    pub fn set_auth_mode(&mut self, mode: AuthMode) {
        if self.auth.phase == Phase::Idle {
            self.auth.mode = mode;
            self.auth.error = None;
        }
    }

    pub fn submit_auth(&mut self) {
        if self.auth.phase == Phase::Busy {
            return;
        }
        let username = self.auth.username.text.trim().to_owned();
        let password = self.auth.password.text.clone();
        let invalid = if username.is_empty() {
            Some((AuthField::Username, "Enter your username."))
        } else if password.is_empty() {
            Some((AuthField::Password, "Enter your password."))
        } else {
            None
        };
        if let Some((field, message)) = invalid {
            self.auth.error = Some(AuthError { field, message: message.into() });
            return;
        }
        self.auth.error = None;
        self.auth.phase = Phase::Busy;
        self.outbox.push(match self.auth.mode {
            AuthMode::Login => Command::Login { username, password },
            AuthMode::SignUp => Command::SignUp { username, password },
        });
    }

    pub fn sign_out(&mut self) {
        // `reset` starts from a fresh state, outbox included, so the command goes in after it.
        self.reset(None);
        self.outbox.push(Command::SignOut);
    }

    // ---- dialogs -----------------------------------------------------------------------------

    pub fn open_dialog(&mut self, kind: DialogKind) {
        self.ui.menu = Menu::None;
        let (text, channel) = match kind {
            DialogKind::CreateChannel { .. } => (String::new(), Some(ChannelForm::new(None))),
            DialogKind::RenameSection { guild, section } => {
                (self.data.layouts.get(&guild).and_then(|l| l.sections.iter().find(|s| s.id == section)).map_or_else(String::new, |s| s.name.clone()), None)
            }
            DialogKind::ChannelSettings { guild, channel } => match self.channel(guild, channel) {
                Some(c) => (c.name.clone(), Some(ChannelForm::of(c))),
                None => return,
            },
            _ => (String::new(), None),
        };
        let mut input = TextEdit::new(&text);
        input.focused = true;
        self.ui.dialog = Some(DialogState { kind, input, channel, error: None, phase: Phase::Idle });
    }

    /// "Create channel here": the create dialog with the section already chosen.
    pub fn open_create_channel_in(&mut self, guild: u64, section: u64) {
        self.open_dialog(DialogKind::CreateChannel { guild });
        if let Some(f) = self.ui.dialog.as_mut().and_then(|d| d.channel.as_mut()) {
            f.section = Some(section);
        }
    }

    pub fn close_dialog(&mut self) {
        self.ui.dialog = None;
    }

    /// Enter or the primary button. Validates first; an invalid form shows its error and sends
    /// nothing.
    pub fn submit_dialog(&mut self) {
        let Some(d) = &mut self.ui.dialog else { return };
        if d.phase == Phase::Busy {
            return;
        }
        let value = d.input.text.trim().to_owned();
        let cmd = match d.kind {
            DialogKind::Invite { .. } => {
                self.ui.dialog = None;
                return;
            }
            DialogKind::CreateGuild => match check_name(&value, 2, 32, "Give the server a name") {
                Ok(()) => Command::CreateGuild { name: value },
                Err(e) => return fail(d, e),
            },
            DialogKind::JoinGuild => {
                if value.is_empty() {
                    return fail(d, "Paste an invite code".into());
                }
                Command::JoinGuild { code: value }
            }
            DialogKind::CreateChannel { guild } => {
                let name = value.to_lowercase().replace(' ', "-");
                let f = d.channel.as_ref();
                match check_name(&name, 1, 32, "Give the channel a name") {
                    Ok(()) => Command::CreateChannel {
                        guild,
                        name,
                        section: f.and_then(|f| f.section),
                        icon: ChannelIcon::new(f.map_or("", |f| f.icon.as_str())),
                        color: PaletteColor::from_key(f.map_or("", |f| f.color)),
                    },
                    Err(e) => return fail(d, e),
                }
            }
            DialogKind::CreateSection { guild } => match check_name(&value, 1, 32, "Give the section a name") {
                Ok(()) => Command::CreateSection { guild, name: value },
                Err(e) => return fail(d, e),
            },
            DialogKind::RenameSection { guild, section } => match check_name(&value, 1, 32, "Give the section a name") {
                Ok(()) => Command::RenameSection { guild, section, name: value },
                Err(e) => return fail(d, e),
            },
            DialogKind::DeleteSection { guild, section } => Command::DeleteSection { guild, section },
            DialogKind::ChannelSettings { guild, channel } => {
                let name = value.to_lowercase().replace(' ', "-");
                if let Err(e) = check_name(&name, 1, 32, "Give the channel a name") {
                    return fail(d, e);
                }
                let Some(f) = d.channel.as_ref() else { return };
                let topic = f.topic.text.trim().to_owned();
                if topic.chars().count() > 256 {
                    return fail(d, "Use up to 256 characters for the topic".into());
                }
                let old = f.original.as_ref();
                let cmd = Command::UpdateChannel {
                    guild,
                    channel,
                    name: (old.map(|c| c.name.as_str()) != Some(name.as_str())).then_some(name),
                    icon: (old.map(|c| c.icon.as_str()) != Some(f.icon.as_str())).then(|| ChannelIcon::new(f.icon.as_str())),
                    color: (old.map(|c| c.color.as_key()) != Some(f.color)).then(|| PaletteColor::from_key(f.color)),
                    topic: (old.map(|c| c.topic.as_str()) != Some(topic.as_str())).then_some(topic),
                };
                if matches!(&cmd, Command::UpdateChannel { name: None, icon: None, color: None, topic: None, .. }) {
                    self.ui.dialog = None;
                    return;
                }
                cmd
            }
            DialogKind::NewDm => {
                let wanted = value.trim_start_matches('@').to_lowercase();
                if wanted.is_empty() {
                    return fail(d, "Enter a username".into());
                }
                let me = self.session.as_ref().map_or(0, |s| s.me.id);
                match self.data.users.values().find(|u| u.id != me && u.username.to_lowercase() == wanted) {
                    Some(u) => Command::OpenDm { user_id: u.id },
                    None => return fail(d, format!("No one called \"{wanted}\" in your servers")),
                }
            }
        };
        d.error = None;
        d.phase = Phase::Busy;
        self.outbox.push(cmd);
    }

    // ---- toasts ------------------------------------------------------------------------------

    pub fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.ui.toast_seq += 1;
        self.ui.toasts.push_back(ToastEntry { id: self.ui.toast_seq, kind, message: message.into(), until: None });
        while self.ui.toasts.len() > TOAST_QUEUE {
            self.ui.toasts.pop_front();
        }
    }

    /// Time-driven state, called from `poll`: toast expiry and the loading placeholder's delay.
    /// Returns whether something visible changed and when to be woken next.
    pub fn tick(&mut self, clock: f64) -> (bool, Option<f64>) {
        self.clock = clock;
        let mut changed = false;
        let mut next: Option<f64> = None;
        let mut soon = |t: f64| next = Some(next.map_or(t, |n| n.min(t)));
        // "Today" follows the wall clock past local midnight, and wakes the app there.
        let now = unix((self.now)());
        let today = crate::format::day_of(self.tz, now);
        if today != self.today {
            self.today = today;
            changed = true;
        }
        soon(clock + crate::format::until_next_day(self.tz, now) as f64);
        while let Some(front) = self.ui.toasts.front_mut() {
            match front.until {
                None => front.until = Some(clock + TOAST_SECS),
                Some(t) if t <= clock => {
                    self.ui.toasts.pop_front();
                    changed = true;
                    continue;
                }
                Some(_) => {}
            }
            soon(front.until.unwrap_or(clock));
            break;
        }
        if let Some(log) = self.current_channel().and_then(|c| self.data.logs.get_mut(&c))
            && let Fetch::Initial { since, shown } = &mut log.fetch
            && !*shown
        {
            let start = *since.get_or_insert(clock);
            if clock - start >= PLACEHOLDER_DELAY {
                *shown = true;
                changed = true;
            } else {
                soon(start + PLACEHOLDER_DELAY);
            }
        }
        (changed, next)
    }

    // ---- derived -----------------------------------------------------------------------------

    /// Whether `content` mentions the signed-in user.
    pub fn mentions_me(&self, content: &str) -> bool {
        self.me().is_some_and(|m| mentions(content, &m.username))
    }

    pub fn day_of(&self, unix: i64) -> i64 {
        crate::format::day_of(self.tz, unix)
    }
}

fn fail(d: &mut DialogState, message: String) {
    d.error = Some(message);
}

/// Whether `content` contains an `@username` mention (case-insensitive).
pub fn mentions(content: &str, username: &str) -> bool {
    use thehideout_design::components::chat::body::{Block, Inline, parse};
    let want = format!("@{}", username.to_lowercase());
    parse(content).iter().any(|b| match b {
        Block::Text { text, marks } => marks.iter().any(|&(a, z, k)| k == Inline::Mention && text[a..z].to_lowercase() == want),
        Block::Code { .. } => false,
    })
}

/// The stable element id of a message row: a sent message keeps the id of its pending self (its
/// nonce) so the row doesn't remount when the server confirms it.
pub fn chat_id_of(m: &Message, me: u64) -> String {
    if m.author_id == me && !m.nonce.is_empty() { format!("n:{}", m.nonce) } else { format!("m:{}", m.id) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_ignore_case_and_code() {
        assert!(mentions("hi @Tim, ok?", "tim"));
        assert!(!mentions("hi @timothy", "tim"));
        assert!(!mentions("run `@tim`", "tim"));
        assert!(!mentions("mail me@tim.com", "tim"));
    }

    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn clock() -> SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(CLOCK.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[test]
    fn today_follows_the_wall_clock_across_local_midnight() {
        use crate::format::utc;
        // 23:59:50 on the 19th in UTC+1 is 22:59:50 UTC.
        CLOCK.store(utc(2026, 11, 19, 22, 59) as u64 + 50, std::sync::atomic::Ordering::SeqCst);
        let mut s = AppState::new(clock, Tz::Fixed(3600));
        let before = s.today;
        let (changed, wake) = s.tick(100.0);
        assert!(!changed);
        assert_eq!(wake, Some(110.0), "woken at the turn of the day");
        CLOCK.store(utc(2026, 11, 19, 23, 0) as u64, std::sync::atomic::Ordering::SeqCst);
        let (changed, _) = s.tick(110.0);
        assert!(changed);
        assert_eq!(s.today, before + 1);
    }

    #[test]
    fn sentences() {
        assert_eq!(sentence("username too short"), "Username too short.");
        assert_eq!(sentence("done!"), "Done!");
    }
}
