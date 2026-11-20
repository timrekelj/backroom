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
use std::time::Duration;

use thehideout_api::{
    ApiError, Client, Event, Gateway, GatewayHandle, GatewayUpdate, LoginOutcome, SecondFactor, User,
};

use crate::backend::{
    Backend, Command, GuildAccess, Initial, InviteAction, Reply, SecurityInfo, Session, device_label,
};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8080";
pub const SERVER_ENV: &str = "THEHIDEOUT_SERVER_URL";
const APP_ID: &str = "thehideout";
const PAGE: u32 = 50;
/// A passkey ceremony is polled once a second for up to two minutes.
const PASSKEY_POLL: Duration = Duration::from_secs(1);
const PASSKEY_POLLS: u32 = 120;

/// The server to talk to when the auth form's field is empty: the environment, else the default.
pub fn default_server() -> String {
    std::env::var(SERVER_ENV)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_owned())
}

/// `http://host:port` without a trailing slash; a bare `host:port` gets `http://`.
fn normalize(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
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
        Some((
            saved.get("server_url")?.clone(),
            saved.get("token")?.clone(),
        ))
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
    /// Uploads under way by the client's id, so one can be stopped.
    uploads: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
    /// Bumped by every passkey ceremony and by `CancelPasskey`: a poll of an older one stops.
    passkey: AtomicU64,
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

    /// The form's server, normalised, and a client for it without a token (sign-in calls).
    fn signed_out_client(&self) -> (String, Client) {
        let url = normalize(&self.server.lock().unwrap());
        (url.clone(), Client::new(url))
    }

    /// A sign-in landed: `client` becomes the session's, the previous user's gateway is gone and
    /// the session is saved for the next launch.
    fn establish(&self, client: Client, url: &str, token: String, me: User) -> Session {
        self.new_generation();
        *self.client.lock().unwrap() = client;
        self.store.save(url, &token);
        Session { token, me }
    }

    /// Polls a passkey ceremony page's result once a second, until it is there, fails, is
    /// cancelled (`None`) or two minutes pass.
    async fn wait_ceremony<T, Fut>(&self, ticket: u64, mut finish: impl FnMut() -> Fut) -> Option<Result<T, ApiError>>
    where
        Fut: Future<Output = Result<Option<T>, ApiError>>,
    {
        for _ in 0..PASSKEY_POLLS {
            tokio::time::sleep(PASSKEY_POLL).await;
            if self.passkey.load(Ordering::SeqCst) != ticket {
                return None;
            }
            let result = finish().await;
            if self.passkey.load(Ordering::SeqCst) != ticket {
                return None;
            }
            match result {
                Ok(Some(done)) => return Some(Ok(done)),
                Ok(None) => {}
                Err(e) => return Some(Err(e)),
            }
        }
        Some(Err(ApiError::Other("the passkey wasn't confirmed within two minutes".into())))
    }

    /// Starts the gateway for the current session unless one is already running.
    fn ensure_gateway(self: &Arc<Self>) {
        let generation = self.generation();
        let mut slot = self.gateway.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|(g, h)| *g == generation && !h.is_stopped())
        {
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
                uploads: Mutex::new(HashMap::new()),
                passkey: AtomicU64::new(0),
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

    /// Streams an upload on the runtime and reports its progress (a step per percent) and end. The
    /// task is kept so `CancelUpload` can stop it; a stopped upload reports nothing more.
    fn upload(&mut self, id: u64, source: UploadSource, filename: String, content_type: String, channel: u64) {
        let shared = Arc::clone(&self.shared);
        let generation = shared.generation();
        let client = shared.client();
        let handle = self.runtime().spawn({
            let shared = Arc::clone(&shared);
            async move {
                let steps = Arc::new(AtomicU64::new(u64::MAX));
                let report = {
                    let (shared, steps) = (Arc::clone(&shared), Arc::clone(&steps));
                    move |sent: u64, total: u64| {
                        let step = (sent * 100).checked_div(total).unwrap_or(100);
                        if steps.swap(step, Ordering::Relaxed) != step {
                            shared.push(Reply::UploadProgress { id, sent, total });
                        }
                    }
                };
                let result = match source {
                    UploadSource::Bytes(bytes) => client.upload_attachment(channel, &filename, &content_type, bytes, report).await,
                    UploadSource::Path { path, size } => {
                        // The thumbnail of a picture is read beside the upload, off the UI thread.
                        if content_type.starts_with("image/") && size <= thehideout_api::MAX_MEDIA_BYTES as u64 {
                            let p = path.clone();
                            if let Ok(Ok(bytes)) = tokio::task::spawn_blocking(move || std::fs::read(p)).await {
                                shared.push(Reply::UploadPreview { id, bytes });
                            }
                        }
                        match tokio::task::spawn_blocking(move || std::fs::File::open(path)).await {
                            Ok(Ok(file)) => client.upload_attachment_reader(channel, &filename, &content_type, size, file, report).await,
                            Ok(Err(e)) => Err(ApiError::InvalidArgument(format!("can't read that file: {e}"))),
                            Err(e) => Err(ApiError::Other(e.to_string())),
                        }
                    }
                };
                shared.uploads.lock().unwrap().remove(&id);
                if shared.generation() != generation {
                    return;
                }
                if matches!(result, Err(ApiError::Unauthenticated)) {
                    shared.expire();
                } else {
                    shared.push(Reply::Uploaded { id, result });
                }
            }
        });
        self.shared.uploads.lock().unwrap().insert(id, handle.abort_handle());
    }

    /// A sign-in step on a fresh client for the form's server: a session (saved, and the client
    /// becomes the session's), a second-factor challenge, or the error.
    fn sign_in<Fut>(&mut self, run: impl FnOnce(Client) -> Fut + Send + 'static)
    where
        Fut: Future<Output = Result<LoginOutcome, ApiError>> + Send + 'static,
    {
        let shared = Arc::clone(&self.shared);
        self.runtime().spawn(async move {
            let (url, client) = shared.signed_out_client();
            let reply = match run(client.clone()).await {
                Ok(LoginOutcome::Session { token, user }) => Reply::Auth(Ok(shared.establish(client, &url, token, user))),
                Ok(LoginOutcome::SecondFactor { challenge, methods }) => Reply::SecondFactor { challenge, methods },
                Err(e) => Reply::Auth(Err(e)),
            };
            shared.push(reply);
        });
    }

    /// A call that needs no session (codes, invites) on the form's server.
    fn signed_out<T: Send + 'static, Fut>(
        &mut self,
        run: impl FnOnce(Client) -> Fut + Send + 'static,
        make: impl FnOnce(Result<T, ApiError>) -> Reply + Send + 'static,
    ) where
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let shared = Arc::clone(&self.shared);
        self.runtime().spawn(async move {
            let (_, client) = shared.signed_out_client();
            shared.push(make(run(client).await));
        });
    }

    /// `call` for a signed-in check of a code or password: the server answers a wrong code with
    /// `Unauthenticated` too, so the session only ends when `GetMe` agrees it is gone.
    fn call_checked<T: Send + 'static>(
        &mut self,
        fut: impl Future<Output = Result<T, ApiError>> + Send + 'static,
        make: impl FnOnce(Result<T, ApiError>) -> Reply + Send + 'static,
    ) {
        let shared = Arc::clone(&self.shared);
        let generation = shared.generation();
        let client = shared.client();
        self.runtime().spawn(async move {
            let result = fut.await;
            if matches!(result, Err(ApiError::Unauthenticated)) && client.get_me().await == Err(ApiError::Unauthenticated) {
                if shared.generation() == generation {
                    shared.expire();
                }
                return;
            }
            if shared.generation() == generation {
                shared.push(make(result));
            }
        });
    }

    /// A passkey sign-in: the ceremony page goes to the app to open, then its result is polled.
    fn passkey_login(&mut self, username: String) {
        let shared = Arc::clone(&self.shared);
        let ticket = shared.passkey.fetch_add(1, Ordering::SeqCst) + 1;
        self.runtime().spawn(async move {
            let (url, client) = shared.signed_out_client();
            let ceremony = match client.begin_passkey_login(&username, &device_label(), None).await {
                Ok(c) => c,
                Err(e) => return shared.push(Reply::PasskeyCeremony(Err(e))),
            };
            shared.push(Reply::PasskeyCeremony(Ok(ceremony.url.clone())));
            let done = shared.wait_ceremony(ticket, || client.finish_passkey_login(&ceremony.id, None)).await;
            match done {
                None => {}
                Some(Ok((token, me))) => {
                    let session = shared.establish(client.clone(), &url, token, me);
                    shared.push(Reply::Auth(Ok(session)));
                }
                Some(Err(e)) => shared.push(Reply::Auth(Err(e))),
            }
        });
    }

    /// Adds a passkey to the signed-in account through the browser ceremony.
    fn passkey_register(&mut self, name: String) {
        let shared = Arc::clone(&self.shared);
        let ticket = shared.passkey.fetch_add(1, Ordering::SeqCst) + 1;
        let generation = shared.generation();
        let client = shared.client();
        self.runtime().spawn(async move {
            let ceremony = match client.begin_passkey_registration(&name, None).await {
                Ok(c) => c,
                Err(ApiError::Unauthenticated) => return shared.expire(),
                Err(e) => return shared.push(Reply::PasskeyCeremony(Err(e))),
            };
            shared.push(Reply::PasskeyCeremony(Ok(ceremony.url.clone())));
            let done = shared.wait_ceremony(ticket, || client.finish_passkey_registration(&ceremony.id, None)).await;
            if shared.generation() != generation {
                return;
            }
            match done {
                None => {}
                Some(Err(ApiError::Unauthenticated)) => shared.expire(),
                Some(result) => shared.push(Reply::PasskeyAdded(result)),
            }
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
    Ok(Initial {
        me,
        guilds,
        channels: by_guild.into_iter().flatten().collect(),
        layouts,
        dms,
    })
}

enum UploadSource {
    Path { path: std::path::PathBuf, size: u64 },
    Bytes(Vec<u8>),
}

impl Backend for RealBackend {
    fn submit(&mut self, cmd: Command) {
        self.sent.push(cmd.clone());
        let client = self.shared.client();
        match cmd {
            Command::Login { username, password } => {
                self.sign_in(move |c| async move { c.login_on(&username, &password, &device_label()).await })
            }
            Command::RequestEmailCode { email } => {
                self.signed_out(move |c| async move { c.request_email_code(&email).await }, Reply::CodeSent)
            }
            Command::VerifyEmailCode { email, code } => {
                self.sign_in(move |c| async move { c.verify_email_code(&email, &code, &device_label()).await })
            }
            Command::CompleteSecondFactor { challenge, code, recovery } => self.sign_in(move |c| async move {
                let answer = if recovery { SecondFactor::RecoveryCode(&code) } else { SecondFactor::Totp(&code) };
                let (token, user) = c.complete_second_factor(&challenge, answer).await?;
                Ok(LoginOutcome::Session { token, user })
            }),
            Command::RequestPasswordReset { email } => {
                self.signed_out(move |c| async move { c.request_password_reset(&email).await }, Reply::CodeSent)
            }
            Command::ResetPassword { email, code, new_password } => self.signed_out(
                move |c| async move { c.reset_password(&email, &code, &new_password).await },
                Reply::PasswordReset,
            ),
            Command::BeginPasskeyLogin { username } => self.passkey_login(username),
            Command::CancelPasskey => {
                self.shared.passkey.fetch_add(1, Ordering::SeqCst);
            }
            Command::LookupInvite { code } => {
                self.signed_out(move |c| async move { c.lookup_invite(&code).await }, Reply::InviteLookup)
            }
            Command::AcceptInvite { code, username, password } => self.sign_in(move |c| async move {
                let (token, user) = c.accept_invite(&code, &username, password.as_deref(), &device_label()).await?;
                Ok(LoginOutcome::Session { token, user })
            }),
            Command::LoadSecurity => {
                self.call(
                    async move {
                        let (passkeys, sessions) = tokio::join!(client.list_passkeys(), client.list_sessions());
                        // The API has no "is an authenticator on?": starting a setup answers it
                        // (refused when one is on; a setup that is never confirmed lapses).
                        let totp = match client.begin_totp_setup().await {
                            Ok(_) => Some(false),
                            Err(ApiError::Precondition(_)) => Some(true),
                            Err(_) => None,
                        };
                        Ok(SecurityInfo { passkeys: passkeys?, sessions: sessions?, totp })
                    },
                    Reply::Security,
                );
            }
            Command::ChangePassword { current, new } => {
                self.call(async move { client.change_password(&current, &new).await }, Reply::PasswordChanged);
            }
            Command::ChangeEmail { email, password } => {
                self.call(async move { client.change_email(&email, &password).await }, Reply::EmailChangeStarted);
            }
            Command::ConfirmEmailChange { code } => {
                self.call_checked(async move { client.confirm_email_change(&code).await }, Reply::EmailChanged);
            }
            Command::BeginTotpSetup => self.call(async move { client.begin_totp_setup().await }, Reply::TotpSetup),
            Command::ConfirmTotp { code } => self.call_checked(
                async move { client.confirm_totp(&code).await },
                |result| Reply::RecoveryCodes { regenerated: false, result },
            ),
            Command::DisableTotp { code } => {
                self.call_checked(async move { client.disable_totp(&code).await }, Reply::TotpDisabled);
            }
            Command::RegenerateRecoveryCodes { code } => self.call_checked(
                async move { client.regenerate_recovery_codes(&code).await },
                |result| Reply::RecoveryCodes { regenerated: true, result },
            ),
            Command::BeginPasskeyRegistration { name } => self.passkey_register(name),
            Command::RenamePasskey { id, name } => {
                self.call(async move { client.rename_passkey(id, &name).await }, Reply::PasskeyRenamed);
            }
            Command::DeletePasskey { id } => {
                self.call(async move { client.delete_passkey(id).await }, move |result| Reply::PasskeyDeleted { id, result });
            }
            Command::RevokeSession { id } => {
                self.call(async move { client.revoke_session(id).await }, move |result| Reply::SessionRevoked { id, result });
            }
            Command::RevokeOtherSessions => {
                self.call(async move { client.revoke_other_sessions().await }, Reply::OthersRevoked);
            }
            Command::CreateInvite { invite } => {
                self.call(async move { client.create_invite(&invite).await }, Reply::InviteCreated);
            }
            Command::ListInvites { epoch, guild, page_token, limit } => self.call(
                async move { client.list_invites(guild, page_token.as_deref(), limit).await },
                move |result| Reply::Invites { epoch, guild, result },
            ),
            Command::RevokeInvite { id } => self.call(
                async move { client.revoke_invite(id).await },
                move |result| Reply::InviteSaved { id, action: InviteAction::Revoke, result },
            ),
            Command::ResendInvite { id } => self.call(
                async move { client.resend_invite(id).await },
                move |result| Reply::InviteSaved { id, action: InviteAction::Resend, result },
            ),
            Command::Resume => self.resume(),
            Command::LoadInitial => self.load_initial(),
            Command::Refresh => {
                self.call(
                    async move { fetch_initial(&client).await },
                    Reply::Refreshed,
                );
            }
            Command::LoadChannels { guild } => {
                self.call(
                    async move { client.list_channels_with_layout(guild).await },
                    move |result| Reply::Channels { guild, result },
                );
            }
            Command::LoadMessages { channel, before } => {
                self.call(
                    async move { client.list_messages(channel, before, PAGE).await },
                    move |result| Reply::Messages {
                        channel,
                        before,
                        result,
                    },
                );
            }
            Command::Send {
                channel,
                content,
                nonce,
            } => {
                let n = nonce.clone();
                self.call(
                    async move { client.send_message(channel, &content, Some(&n)).await },
                    move |result| Reply::Sent {
                        channel,
                        nonce,
                        result,
                    },
                );
            }
            Command::SendWith {
                channel,
                content,
                nonce,
                attachments,
            } => {
                let n = nonce.clone();
                self.call(
                    async move {
                        client
                            .send_message_with(channel, &content, Some(&n), &attachments)
                            .await
                    },
                    move |result| Reply::Sent {
                        channel,
                        nonce,
                        result,
                    },
                );
            }
            Command::UploadFile {
                channel,
                id,
                path,
                filename,
                content_type,
                size,
            } => self.upload(id, UploadSource::Path { path, size }, filename, content_type, channel),
            Command::UploadBytes {
                channel,
                id,
                filename,
                content_type,
                bytes,
            } => self.upload(id, UploadSource::Bytes(bytes), filename, content_type, channel),
            Command::CancelUpload { id } => {
                if let Some(h) = self.shared.uploads.lock().unwrap().remove(&id) {
                    h.abort();
                }
            }
            Command::Download { key, path } => {
                let shared = Arc::clone(&self.shared);
                let generation = shared.generation();
                self.runtime().spawn(async move {
                    let steps = AtomicU64::new(u64::MAX);
                    let result = match std::fs::File::create(&path) {
                        Ok(file) => {
                            let mut file = std::io::BufWriter::new(file);
                            let progress = |received: u64, total: Option<u64>| {
                                let step = total.filter(|t| *t > 0).map_or(received >> 18, |t| received * 100 / t);
                                if steps.swap(step, Ordering::Relaxed) != step {
                                    shared.push(Reply::DownloadProgress { key: key.clone(), received, total });
                                }
                            };
                            client.download(&key, &mut file, progress).await
                        }
                        Err(e) => Err(ApiError::Other(format!("can't write {}: {e}", path.display()))),
                    };
                    // A broken download leaves no half file behind.
                    if result.is_err() {
                        std::fs::remove_file(&path).ok();
                    }
                    if shared.generation() == generation {
                        shared.push(Reply::Downloaded { key, path, result });
                    }
                });
            }
            Command::CreateGuild { name } => {
                self.call(
                    async move { client.create_guild(&name).await },
                    Reply::GuildCreated,
                );
            }
            Command::JoinGuild { code } => {
                self.call(
                    async move { client.join_guild(&code).await },
                    Reply::GuildJoined,
                );
            }
            Command::CreateChannel {
                guild,
                name,
                section,
                icon,
                color,
            } => {
                self.call(
                    async move {
                        client
                            .create_channel_with_ref(guild, &name, section, &icon, color)
                            .await
                    },
                    Reply::ChannelCreated,
                );
            }
            Command::LoadAccess { guild } => {
                self.call(
                    async move {
                        let (roles, members) =
                            tokio::join!(client.list_roles(guild), client.list_members(guild));
                        let member_roles = members?
                            .into_iter()
                            .map(|m| (m.user.id, m.role_ids))
                            .collect();
                        let list = roles?;
                        Ok(GuildAccess {
                            roles: list.roles,
                            version: list.version,
                            member_roles,
                        })
                    },
                    move |result| Reply::Access { guild, result },
                );
            }
            Command::CreateSection { guild, name } => {
                self.call(
                    async move { client.create_section(guild, &name).await },
                    move |result| Reply::SectionSaved { guild, result },
                );
            }
            Command::RenameSection {
                guild,
                section,
                name,
            } => {
                self.call(
                    async move { client.update_section(guild, section, &name).await },
                    move |result| Reply::SectionSaved { guild, result },
                );
            }
            Command::DeleteSection { guild, section } => {
                self.call(
                    async move { client.delete_section(guild, section).await },
                    move |result| Reply::SectionDeleted { guild, result },
                );
            }
            Command::UpdateLayout { guild, layout } => {
                self.call(
                    async move { client.update_layout(guild, &layout).await },
                    move |result| Reply::LayoutSaved { guild, result },
                );
            }
            Command::UpdateChannel {
                guild,
                channel,
                name,
                icon,
                color,
                topic,
            } => {
                self.call(
                    async move {
                        client
                            .update_channel_ref(
                                guild,
                                channel,
                                name.as_deref(),
                                icon.as_ref(),
                                color,
                                topic.as_deref(),
                            )
                            .await
                    },
                    move |result| Reply::ChannelSaved { guild, result },
                );
            }
            Command::LoadMembers { guild } => {
                self.call(
                    async move { client.list_guild_members(guild).await },
                    move |result| Reply::Members { guild, result },
                );
            }
            Command::OpenDm { user_id } => {
                self.call(
                    async move { client.open_dm(user_id).await },
                    Reply::DmOpened,
                );
            }
            Command::FetchMedia { key } => {
                let fetched = key.clone();
                self.call(
                    async move { client.fetch_media(&fetched).await },
                    move |result| Reply::Media { key, result },
                );
            }
            Command::ReadImage { path, purpose } => {
                // The file is read on a blocking thread of the runtime, never where frames are drawn.
                self.call(
                    async move {
                        tokio::task::spawn_blocking(move || crate::media::read_image(&path))
                            .await
                            .map_err(|e| ApiError::Other(e.to_string()))?
                    },
                    move |result| Reply::ImageRead { purpose, result },
                );
            }
            Command::UploadAvatar {
                bytes,
                content_type,
            } => {
                self.call(
                    async move { client.upload_avatar(bytes, &content_type).await },
                    Reply::AvatarSaved,
                );
            }
            Command::RemoveAvatar => {
                self.call(
                    async move { client.remove_avatar().await },
                    Reply::AvatarSaved,
                );
            }
            Command::UploadGuildIcon {
                guild,
                bytes,
                content_type,
            } => {
                self.call(
                    async move { client.upload_guild_icon(guild, bytes, &content_type).await },
                    move |result| Reply::GuildSaved { guild, result },
                );
            }
            Command::RemoveGuildIcon { guild } => {
                self.call(
                    async move { client.remove_guild_icon(guild).await },
                    move |result| Reply::GuildSaved { guild, result },
                );
            }
            Command::RenameGuild { guild, name } => {
                self.call(
                    async move { client.update_guild(guild, Some(&name)).await },
                    move |result| Reply::GuildSaved { guild, result },
                );
            }
            Command::CreateRole {
                guild,
                name,
                color,
                permissions,
            } => {
                self.call(
                    async move { client.create_role(guild, &name, color, permissions).await },
                    move |result| Reply::RoleCreated { guild, result },
                );
            }
            Command::UpdateRole {
                guild,
                role,
                name,
                color,
                permissions,
            } => {
                self.call(
                    async move {
                        client
                            .update_role(guild, role, name.as_deref(), color, permissions)
                            .await
                    },
                    move |result| Reply::RoleUpdated { guild, result },
                );
            }
            Command::DeleteRole { guild, role } => {
                self.call(
                    async move { client.delete_role(guild, role).await },
                    move |result| Reply::RoleDeleted {
                        guild,
                        role,
                        result,
                    },
                );
            }
            Command::ReorderRoles {
                guild,
                expected_version,
                role_ids,
            } => {
                self.call(
                    async move {
                        client
                            .reorder_roles(guild, expected_version, &role_ids)
                            .await
                    },
                    move |result| Reply::RolesReordered { guild, result },
                );
            }
            Command::SetMemberRoles {
                guild,
                user,
                role_ids,
            } => {
                self.call(
                    async move { client.set_member_roles(guild, user, &role_ids).await },
                    move |result| Reply::MemberRolesSet {
                        guild,
                        user,
                        result,
                    },
                );
            }
            Command::LoadEmojis { guild } => {
                self.call(
                    async move { client.list_emojis(guild).await },
                    move |result| Reply::Emojis { guild, result },
                );
            }
            Command::CreateEmoji {
                guild,
                name,
                bytes,
                content_type,
            } => {
                self.call(
                    async move {
                        client
                            .create_emoji(guild, &name, bytes, &content_type)
                            .await
                    },
                    move |result| Reply::EmojiSaved { guild, result },
                );
            }
            Command::RenameEmoji { guild, emoji, name } => {
                self.call(
                    async move { client.rename_emoji(guild, emoji, &name).await },
                    move |result| Reply::EmojiSaved { guild, result },
                );
            }
            Command::DeleteEmoji { guild, emoji } => {
                self.call(
                    async move { client.delete_emoji(guild, emoji).await },
                    move |result| Reply::EmojiDeleted {
                        guild,
                        emoji,
                        result,
                    },
                );
            }
            Command::AddReaction {
                channel,
                message,
                emoji,
            } => {
                let sent = emoji.clone();
                self.call(
                    async move { client.add_reaction(channel, message, &sent).await },
                    move |result| Reply::ReactionSaved {
                        channel,
                        message,
                        emoji,
                        add: true,
                        result,
                    },
                );
            }
            Command::RemoveReaction {
                channel,
                message,
                emoji,
            } => {
                let sent = emoji.clone();
                self.call(
                    async move { client.remove_reaction(channel, message, &sent).await },
                    move |result| Reply::ReactionSaved {
                        channel,
                        message,
                        emoji,
                        add: false,
                        result,
                    },
                );
            }
            Command::AdminOverview => {
                self.call(
                    async move { client.admin_overview().await },
                    Reply::AdminOverview,
                );
            }
            Command::AdminActivity { days } => {
                self.call(
                    async move { client.admin_activity(days).await },
                    move |result| Reply::AdminActivity { days, result },
                );
            }
            Command::AdminGuilds {
                epoch,
                query,
                page_token,
                limit,
            } => {
                self.call(
                    async move {
                        client
                            .admin_list_guilds(&query, page_token.as_deref(), limit)
                            .await
                    },
                    move |result| Reply::AdminGuilds { epoch, result },
                );
            }
            Command::AdminUsers {
                epoch,
                query,
                page_token,
                limit,
            } => {
                self.call(
                    async move {
                        client
                            .admin_list_users(&query, page_token.as_deref(), limit)
                            .await
                    },
                    move |result| Reply::AdminUsers { epoch, result },
                );
            }
            Command::AdminFiles {
                epoch,
                guild,
                kind,
                page_token,
                limit,
            } => {
                self.call(
                    async move {
                        client
                            .admin_list_files(guild, kind, page_token.as_deref(), limit)
                            .await
                    },
                    move |result| Reply::AdminFiles { epoch, result },
                );
            }
            Command::AdminAudit {
                epoch,
                page_token,
                limit,
            } => {
                self.call(
                    async move {
                        client
                            .admin_list_audit_log(page_token.as_deref(), limit)
                            .await
                    },
                    move |result| Reply::AdminAudit { epoch, result },
                );
            }
            Command::AdminSetAdmin { user, is_admin } => {
                self.call(
                    async move { client.admin_set_admin(user, is_admin).await },
                    move |result| Reply::AdminAdminSet {
                        user,
                        is_admin,
                        result,
                    },
                );
            }
            Command::AdminDeleteFile { key, reason } => {
                let sent = key.clone();
                self.call(
                    async move { client.admin_delete_file(&sent, &reason).await },
                    move |result| Reply::AdminFileDeleted { key, result },
                );
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
        *self.shared.server.lock().unwrap() = if url.is_empty() {
            default_server()
        } else {
            url.to_owned()
        };
    }

    fn sent(&self) -> Vec<Command> {
        self.sent.clone()
    }
}
