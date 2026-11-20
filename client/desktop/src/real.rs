//! The network backend: every [`Command`] becomes an RPC on a small tokio runtime, and its
//! [`Reply`] waits in a shared queue until `poll` drains it (the shell's waker is rung when one
//! lands). The gateway runs on the same runtime and feeds the same queue.
//!
//! The runtime is multi-threaded with two workers: the gateway stream is a long-lived task and a
//! slow decode or a stalled RPC must not hold it up. It is built on the first command, so
//! constructing the backend (headless scenes build one and drop it) costs nothing.
//!
//! The session (`server_url`, `token`) is kept with Rune's `Prefs` (`prefs.json` in the app's
//! config directory). All of it is read and written on the runtime, never in `poll`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rune::Waker;
use rune::prefs::Prefs;
use thehideout_api::{ApiError, Client, Event, Gateway, GatewayHandle, GatewayUpdate};

use crate::backend::{Backend, Command, GuildAccess, Initial, Reply, Session};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
pub const SERVER_ENV: &str = "THEHIDEOUT_SERVER_URL";
const APP_ID: &str = "thehideout";
const PAGE: u32 = 50;

/// The server to talk to when the auth form's field is empty: the environment, else the default.
pub fn default_server() -> String {
    std::env::var(SERVER_ENV).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SERVER.to_owned())
}

/// `http://host:port` without a trailing slash; a bare `host:port` gets `http://`.
fn normalize(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.contains("://") { url.to_owned() } else { format!("http://{url}") }
}

/// The app's one `Prefs`, shared with the look's saver thread: two stores on one file would each
/// rewrite it whole from their own copy and lose the other's keys.
pub type SharedPrefs = Arc<Mutex<Prefs>>;

/// Opens the app's prefs file. Read once at launch (before the first frame), never in a frame.
pub fn open_prefs() -> SharedPrefs {
    Arc::new(Mutex::new(Prefs::open(APP_ID)))
}

struct Store(SharedPrefs);

impl Store {
    fn with<T>(&self, f: impl FnOnce(&mut Prefs) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn load(&self) -> Option<(String, String)> {
        let saved: HashMap<String, String> = self.with(|p| p.get("session"))?;
        Some((saved.get("server_url")?.clone(), saved.get("token")?.clone()))
    }

    fn save(&self, server_url: &str, token: &str) {
        let saved = HashMap::from([("server_url", server_url), ("token", token)]);
        // A session that can't be written only costs the next launch's auto-login.
        self.with(|p| p.set("session", &saved)).ok();
    }

    fn clear(&self) {
        self.with(|p| p.remove("session")).ok();
    }
}

struct Shared {
    queue: Mutex<Vec<Reply>>,
    waker: Mutex<Option<Waker>>,
    client: Mutex<Client>,
    /// The server URL the next login uses (the auth form's field).
    server: Mutex<String>,
    /// Bumped by sign-in and sign-out: answers and gateway updates of an older session are dropped.
    generation: AtomicU64,
    gateway: Mutex<Option<(u64, GatewayHandle)>>,
    store: Store,
}

impl Shared {
    fn push(&self, reply: Reply) {
        self.queue.lock().unwrap().push(reply);
        if let Some(wake) = self.waker.lock().unwrap().as_ref() {
            wake();
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn client(&self) -> Client {
        self.client.lock().unwrap().clone()
    }

    fn stop_gateway(&self) {
        self.gateway.lock().unwrap().take();
    }

    /// A new session begins (or the old one ends): nothing from before may land afterwards.
    fn new_generation(&self) -> u64 {
        self.stop_gateway();
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The server refused the token: forget the session and send the app back to the form.
    fn expire(&self) {
        self.new_generation();
        self.client().set_token(None);
        self.store.clear();
        self.push(Reply::Gateway(GatewayUpdate::AuthExpired));
    }

    /// Starts the gateway for the current session unless one is already running.
    fn ensure_gateway(self: &Arc<Self>) {
        let generation = self.generation();
        let mut slot = self.gateway.lock().unwrap();
        if slot.as_ref().is_some_and(|(g, h)| *g == generation && !h.is_stopped()) {
            return;
        }
        let shared = Arc::clone(self);
        let handle = Gateway::spawn(self.client(), move |update| {
            if shared.generation() != generation {
                return;
            }
            match update {
                GatewayUpdate::Event(Event::Heartbeat) => {}
                GatewayUpdate::AuthExpired => {
                    shared.client().set_token(None);
                    shared.store.clear();
                    shared.push(Reply::Gateway(GatewayUpdate::AuthExpired));
                }
                other => shared.push(Reply::Gateway(other)),
            }
        });
        *slot = Some((generation, handle));
    }
}

pub struct RealBackend {
    runtime: Option<tokio::runtime::Runtime>,
    shared: Arc<Shared>,
    sent: Vec<Command>,
}

impl RealBackend {
    /// Sessions are kept in `prefs`, the store the rest of the app shares.
    #[allow(dead_code)]
    pub fn with_shared(prefs: SharedPrefs) -> RealBackend {
        RealBackend::with_store(Store(prefs))
    }

    /// Sessions are kept in `prefs` (tests give it a temporary file or none).
    #[allow(dead_code)]
    pub fn with_prefs(prefs: Prefs) -> RealBackend {
        RealBackend::with_store(Store(Arc::new(Mutex::new(prefs))))
    }

    fn with_store(store: Store) -> RealBackend {
        let server = default_server();
        RealBackend {
            runtime: None,
            shared: Arc::new(Shared {
                queue: Mutex::new(Vec::new()),
                waker: Mutex::new(None),
                client: Mutex::new(Client::new(normalize(&server))),
                server: Mutex::new(server),
                generation: AtomicU64::new(0),
                gateway: Mutex::new(None),
                store,
            }),
            sent: Vec::new(),
        }
    }

    fn runtime(&mut self) -> &tokio::runtime::Runtime {
        self.runtime.get_or_insert_with(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("thehideout-net")
                .enable_all()
                .build()
                .expect("start the network runtime")
        })
    }

    /// Runs `fut` and queues `make(result)`, unless the session changed meanwhile. A refused
    /// token ends the session instead of surfacing as an error on whatever was asked.
    fn call<T: Send + 'static>(
        &mut self,
        fut: impl Future<Output = Result<T, ApiError>> + Send + 'static,
        make: impl FnOnce(Result<T, ApiError>) -> Reply + Send + 'static,
    ) {
        let shared = Arc::clone(&self.shared);
        let generation = shared.generation();
        self.runtime().spawn(async move {
            let result = fut.await;
            if shared.generation() != generation {
                return;
            }
            if matches!(result, Err(ApiError::Unauthenticated)) {
                shared.expire();
            } else {
                shared.push(make(result));
            }
        });
    }

    /// A sign-in: a fresh client for the form's server, then `auth` on it; on success the session
    /// is saved and the previous user's gateway is gone.
    fn authenticate(&mut self, username: String, password: String, sign_up: bool) {
        let shared = Arc::clone(&self.shared);
        self.runtime().spawn(async move {
            let url = normalize(&shared.server.lock().unwrap());
            let client = Client::new(url.clone());
            let result = if sign_up { client.sign_up(&username, &password).await } else { client.login(&username, &password).await };
            let reply = result.map(|(token, me)| {
                shared.new_generation();
                *shared.client.lock().unwrap() = client;
                shared.store.save(&url, &token);
                Session { token, me }
            });
            shared.push(Reply::Auth(reply));
        });
    }

    /// Launch: validate the saved session (if any) with `GetMe`.
    fn resume(&mut self) {
        let shared = Arc::clone(&self.shared);
        self.runtime().spawn(async move {
            let Some((url, token)) = shared.store.load() else {
                return shared.push(Reply::Resume(Ok(None)));
            };
            let client = Client::new(normalize(&url)).with_token(token.clone());
            match client.get_me().await {
                Ok(me) => {
                    shared.new_generation();
                    *shared.client.lock().unwrap() = client;
                    shared.push(Reply::Resume(Ok(Some((url, Session { token, me })))));
                }
                Err(e) => {
                    // Only a refused token ends the saved session; an unreachable server doesn't.
                    if e == ApiError::Unauthenticated {
                        shared.store.clear();
                    }
                    shared.push(Reply::Resume(Err(e)));
                }
            }
        });
    }

    fn load_initial(&mut self) {
        let shared = Arc::clone(&self.shared);
        let generation = shared.generation();
        let client = shared.client();
        self.runtime().spawn(async move {
            let result = fetch_initial(&client).await;
            if shared.generation() != generation {
                return;
            }
            match &result {
                Ok(_) => shared.ensure_gateway(),
                Err(ApiError::Unauthenticated) => {
                    shared.new_generation();
                    shared.store.clear();
                }
                Err(_) => {}
            }
            shared.push(Reply::Initial(result));
        });
    }
}

async fn fetch_initial(client: &Client) -> Result<Initial, ApiError> {
    let (me, guilds, dms) = tokio::join!(client.get_me(), client.list_guilds(), client.list_dms());
    let (me, guilds, dms) = (me?, guilds?, dms?);
    let mut lists = tokio::task::JoinSet::new();
    for (i, g) in guilds.iter().enumerate() {
        let (client, id) = (client.clone(), g.id);
        lists.spawn(async move { (i, client.list_channels_with_layout(id).await) });
    }
    let mut by_guild = vec![Vec::new(); guilds.len()];
    let mut layouts = HashMap::new();
    while let Some(done) = lists.join_next().await {
        let (i, list) = done.map_err(|e| ApiError::Other(e.to_string()))?;
        let (channels, layout) = list?;
        by_guild[i] = channels;
        layouts.insert(guilds[i].id, layout);
    }
    Ok(Initial { me, guilds, channels: by_guild.into_iter().flatten().collect(), layouts, dms })
}

impl Backend for RealBackend {
    fn submit(&mut self, cmd: Command) {
        self.sent.push(cmd.clone());
        let client = self.shared.client();
        match cmd {
            Command::SignUp { username, password } => self.authenticate(username, password, true),
            Command::Login { username, password } => self.authenticate(username, password, false),
            Command::Resume => self.resume(),
            Command::LoadInitial => self.load_initial(),
            Command::Refresh => {
                self.call(async move { fetch_initial(&client).await }, Reply::Refreshed);
            }
            Command::LoadChannels { guild } => {
                self.call(async move { client.list_channels_with_layout(guild).await }, move |result| Reply::Channels { guild, result });
            }
            Command::LoadMessages { channel, before } => {
                self.call(async move { client.list_messages(channel, before, PAGE).await }, move |result| Reply::Messages { channel, before, result });
            }
            Command::Send { channel, content, nonce } => {
                let n = nonce.clone();
                self.call(async move { client.send_message(channel, &content, Some(&n)).await }, move |result| Reply::Sent { channel, nonce, result });
            }
            Command::CreateGuild { name } => {
                self.call(async move { client.create_guild(&name).await }, Reply::GuildCreated);
            }
            Command::JoinGuild { code } => {
                self.call(async move { client.join_guild(&code).await }, Reply::GuildJoined);
            }
            Command::CreateChannel { guild, name, section, icon, color } => {
                self.call(async move { client.create_channel_with(guild, &name, section, &icon, color).await }, Reply::ChannelCreated);
            }
            Command::LoadAccess { guild } => {
                self.call(
                    async move {
                        let (roles, members) = tokio::join!(client.list_roles(guild), client.list_members(guild));
                        let member_roles = members?.into_iter().map(|m| (m.user.id, m.role_ids)).collect();
                        let list = roles?;
                        Ok(GuildAccess { roles: list.roles, version: list.version, member_roles })
                    },
                    move |result| Reply::Access { guild, result },
                );
            }
            Command::CreateSection { guild, name } => {
                self.call(async move { client.create_section(guild, &name).await }, move |result| Reply::SectionSaved { guild, result });
            }
            Command::RenameSection { guild, section, name } => {
                self.call(async move { client.update_section(guild, section, &name).await }, move |result| Reply::SectionSaved { guild, result });
            }
            Command::DeleteSection { guild, section } => {
                self.call(async move { client.delete_section(guild, section).await }, move |result| Reply::SectionDeleted { guild, result });
            }
            Command::UpdateLayout { guild, layout } => {
                self.call(async move { client.update_layout(guild, &layout).await }, move |result| Reply::LayoutSaved { guild, result });
            }
            Command::UpdateChannel { guild, channel, name, icon, color, topic } => {
                self.call(
                    async move { client.update_channel(guild, channel, name.as_deref(), icon.as_ref(), color, topic.as_deref()).await },
                    move |result| Reply::ChannelSaved { guild, result },
                );
            }
            Command::LoadMembers { guild } => {
                self.call(async move { client.list_guild_members(guild).await }, move |result| Reply::Members { guild, result });
            }
            Command::OpenDm { user_id } => {
                self.call(async move { client.open_dm(user_id).await }, Reply::DmOpened);
            }
            Command::FetchMedia { key } => {
                let fetched = key.clone();
                self.call(async move { client.fetch_media(&fetched).await }, move |result| Reply::Media { key, result });
            }
            Command::ReadImage { path, purpose } => {
                // The file is read on a blocking thread of the runtime, never where frames are drawn.
                self.call(
                    async move { tokio::task::spawn_blocking(move || crate::media::read_image(&path)).await.map_err(|e| ApiError::Other(e.to_string()))? },
                    move |result| Reply::ImageRead { purpose, result },
                );
            }
            Command::UploadAvatar { bytes, content_type } => {
                self.call(async move { client.upload_avatar(bytes, &content_type).await }, Reply::AvatarSaved);
            }
            Command::RemoveAvatar => {
                self.call(async move { client.remove_avatar().await }, Reply::AvatarSaved);
            }
            Command::UploadGuildIcon { guild, bytes, content_type } => {
                self.call(async move { client.upload_guild_icon(guild, bytes, &content_type).await }, move |result| Reply::GuildSaved { guild, result });
            }
            Command::RemoveGuildIcon { guild } => {
                self.call(async move { client.remove_guild_icon(guild).await }, move |result| Reply::GuildSaved { guild, result });
            }
            Command::RenameGuild { guild, name } => {
                self.call(async move { client.update_guild(guild, Some(&name)).await }, move |result| Reply::GuildSaved { guild, result });
            }
            Command::CreateRole { guild, name, color, permissions } => {
                self.call(async move { client.create_role(guild, &name, color, permissions).await }, move |result| Reply::RoleCreated { guild, result });
            }
            Command::UpdateRole { guild, role, name, color, permissions } => {
                self.call(
                    async move { client.update_role(guild, role, name.as_deref(), color, permissions).await },
                    move |result| Reply::RoleUpdated { guild, result },
                );
            }
            Command::DeleteRole { guild, role } => {
                self.call(async move { client.delete_role(guild, role).await }, move |result| Reply::RoleDeleted { guild, role, result });
            }
            Command::ReorderRoles { guild, expected_version, role_ids } => {
                self.call(async move { client.reorder_roles(guild, expected_version, &role_ids).await }, move |result| Reply::RolesReordered { guild, result });
            }
            Command::SetMemberRoles { guild, user, role_ids } => {
                self.call(async move { client.set_member_roles(guild, user, &role_ids).await }, move |result| Reply::MemberRolesSet { guild, user, result });
            }
            Command::SignOut => {
                self.shared.new_generation();
                client.set_token(None);
                // On the runtime: the prefs file is written there, not on the caller's thread.
                let shared = Arc::clone(&self.shared);
                self.runtime().spawn(async move {
                    shared.store.clear();
                    shared.push(Reply::SignedOut);
                });
            }
        }
    }

    fn poll(&mut self, _now: f64) -> Vec<Reply> {
        std::mem::take(&mut *self.shared.queue.lock().unwrap())
    }

    fn set_waker(&mut self, waker: Waker) {
        *self.shared.waker.lock().unwrap() = Some(waker);
    }

    fn set_server(&mut self, url: &str) {
        let url = url.trim();
        *self.shared.server.lock().unwrap() = if url.is_empty() { default_server() } else { url.to_owned() };
    }

    fn sent(&self) -> Vec<Command> {
        self.sent.clone()
    }
}
