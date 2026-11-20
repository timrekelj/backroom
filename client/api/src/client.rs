//! The RPC client.

use std::sync::{Arc, RwLock};

use connectrpc::client::{ClientConfig, HttpClient};

use crate::error::ApiError;
use crate::proto::thehideout::v1 as pb;
use crate::structure::{ChannelIcon, GuildLayout, Member, PaletteColor, Section};
use crate::types::{Channel, DmChannel, Event, Guild, Message, Page, User};

type Result<T> = std::result::Result<T, ApiError>;

/// Typed client for the thehideout server.
///
/// Cheap to clone; clones share the connection pool and the session token, so
/// a token set on one clone (e.g. after login) is used by all of them,
/// including a running [`Gateway`](crate::Gateway).
#[derive(Clone)]
pub struct Client {
    pub(crate) http: HttpClient,
    pub(crate) base_url: String,
    token: Arc<RwLock<Option<String>>>,
}

impl Client {
    /// Creates a client for `base_url` (e.g. `http://127.0.0.1:8080`), without a token.
    ///
    /// Only plaintext `http://` is supported for now.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: HttpClient::plaintext(),
            base_url: base_url.into(),
            token: Arc::default(),
        }
    }

    /// Builder-style [`set_token`](Self::set_token).
    pub fn with_token(self, token: impl Into<String>) -> Self {
        self.set_token(Some(token.into()));
        self
    }

    /// Sets or clears the session token sent as `Authorization: Bearer <token>`.
    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap() = token;
    }

    /// The current session token, if any.
    pub fn token(&self) -> Option<String> {
        self.token.read().unwrap().clone()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn config(&self) -> Result<ClientConfig> {
        let uri = self
            .base_url
            .parse()
            .map_err(|e| ApiError::InvalidArgument(format!("bad server url {:?}: {e}", self.base_url)))?;
        let mut config = ClientConfig::new(uri);
        if let Some(token) = self.token() {
            config = config.with_default_header("authorization", format!("Bearer {token}"));
        }
        Ok(config)
    }

    fn auth(&self) -> Result<pb::AuthServiceClient<HttpClient>> {
        Ok(pb::AuthServiceClient::new(self.http.clone(), self.config()?))
    }

    pub(crate) fn guilds(&self) -> Result<pb::GuildServiceClient<HttpClient>> {
        Ok(pb::GuildServiceClient::new(self.http.clone(), self.config()?))
    }

    fn messages(&self) -> Result<pb::MessageServiceClient<HttpClient>> {
        Ok(pb::MessageServiceClient::new(self.http.clone(), self.config()?))
    }

    fn dms(&self) -> Result<pb::DmServiceClient<HttpClient>> {
        Ok(pb::DmServiceClient::new(self.http.clone(), self.config()?))
    }

    /// Creates an account. Returns the session token and the user, and also
    /// stores the token on this client.
    ///
    /// Fails with [`ApiError::AlreadyExists`] if the username is taken.
    pub async fn sign_up(&self, username: &str, password: &str) -> Result<(String, User)> {
        let res = self
            .auth()?
            .sign_up(pb::SignUpRequest {
                username: username.into(),
                password: password.into(),
                ..Default::default()
            })
            .await?
            .into_owned();
        self.finish_auth(res.token, res.user.into_option())
    }

    /// Logs in. Returns the session token and the user, and also stores the
    /// token on this client.
    pub async fn login(&self, username: &str, password: &str) -> Result<(String, User)> {
        let res = self
            .auth()?
            .login(pb::LoginRequest {
                username: username.into(),
                password: password.into(),
                ..Default::default()
            })
            .await?
            .into_owned();
        self.finish_auth(res.token, res.user.into_option())
    }

    fn finish_auth(&self, token: String, user: Option<pb::User>) -> Result<(String, User)> {
        let user = user.ok_or_else(|| ApiError::Other("server returned no user".into()))?;
        self.set_token(Some(token.clone()));
        Ok((token, user.into()))
    }

    /// The user the current token belongs to.
    pub async fn get_me(&self) -> Result<User> {
        let res = self.auth()?.get_me(pb::GetMeRequest::default()).await?.into_owned();
        required(res.user.into_option())
    }

    /// Creates a guild owned by the caller (the server also creates its first channel).
    pub async fn create_guild(&self, name: &str) -> Result<Guild> {
        let res = self
            .guilds()?
            .create_guild(pb::CreateGuildRequest { name: name.into(), ..Default::default() })
            .await?
            .into_owned();
        required(res.guild.into_option())
    }

    /// The guilds the caller is a member of.
    pub async fn list_guilds(&self) -> Result<Vec<Guild>> {
        let res = self.guilds()?.list_my_guilds(pb::ListMyGuildsRequest::default()).await?.into_owned();
        Ok(res.guilds.into_iter().map(Into::into).collect())
    }

    /// Joins a guild by invite code. [`ApiError::NotFound`] means the code is invalid.
    pub async fn join_guild(&self, invite_code: &str) -> Result<Guild> {
        let res = self
            .guilds()?
            .join_guild(pb::JoinGuildRequest { invite_code: invite_code.into(), ..Default::default() })
            .await?
            .into_owned();
        required(res.guild.into_option())
    }

    /// The members of a guild the caller belongs to, with their roles.
    pub async fn list_members(&self, guild_id: u64) -> Result<Vec<Member>> {
        let res = self
            .guilds()?
            .list_guild_members(pb::ListGuildMembersRequest { guild_id, ..Default::default() })
            .await?
            .into_owned();
        Ok(res.members.into_iter().map(Into::into).collect())
    }

    /// The users that are members of a guild (see [`list_members`](Self::list_members) for roles).
    pub async fn list_guild_members(&self, guild_id: u64) -> Result<Vec<User>> {
        let res = self
            .guilds()?
            .list_guild_members(pb::ListGuildMembersRequest { guild_id, ..Default::default() })
            .await?
            .into_owned();
        Ok(res.users.into_iter().map(Into::into).collect())
    }

    /// Creates a text channel in a guild.
    pub async fn create_channel(&self, guild_id: u64, name: &str) -> Result<Channel> {
        let res = self
            .guilds()?
            .create_channel(pb::CreateChannelRequest { guild_id, name: name.into(), ..Default::default() })
            .await?
            .into_owned();
        required(res.channel.into_option())
    }

    /// Creates a channel with an optional section, icon and colour.
    pub async fn create_channel_with(
        &self,
        guild_id: u64,
        name: &str,
        section_id: Option<u64>,
        icon: &ChannelIcon,
        color: PaletteColor,
    ) -> Result<Channel> {
        let res = self
            .guilds()?
            .create_channel(pb::CreateChannelRequest {
                guild_id,
                name: name.into(),
                section_id,
                icon: icon.0.clone(),
                color: color.as_key().into(),
                ..Default::default()
            })
            .await?
            .into_owned();
        required(res.channel.into_option())
    }

    /// The channels of a guild the caller belongs to.
    pub async fn list_channels(&self, guild_id: u64) -> Result<Vec<Channel>> {
        Ok(self.list_channels_with_layout(guild_id).await?.0)
    }

    /// The channels of a guild together with its section/channel layout.
    pub async fn list_channels_with_layout(&self, guild_id: u64) -> Result<(Vec<Channel>, GuildLayout)> {
        let res = self
            .guilds()?
            .list_channels(pb::ListChannelsRequest { guild_id, ..Default::default() })
            .await?
            .into_owned();
        let layout = res.layout.into_option().map(GuildLayout::from).unwrap_or_default();
        Ok((res.channels.into_iter().map(Into::into).collect(), layout))
    }

    /// Renames a guild (needs `MANAGE_SERVER`).
    pub async fn update_guild(&self, guild_id: u64, name: Option<&str>) -> Result<Guild> {
        let res = self
            .guilds()?
            .update_guild(pb::UpdateGuildRequest { guild_id, name: name.map(str::to_owned), ..Default::default() })
            .await?
            .into_owned();
        required(res.guild.into_option())
    }

    /// Updates a channel; `None` fields stay unchanged, an empty icon/topic
    /// or [`PaletteColor::None`] clears it. Needs `MANAGE_CHANNELS`.
    pub async fn update_channel(
        &self,
        guild_id: u64,
        channel_id: u64,
        name: Option<&str>,
        icon: Option<&ChannelIcon>,
        color: Option<PaletteColor>,
        topic: Option<&str>,
    ) -> Result<Channel> {
        let res = self
            .guilds()?
            .update_channel(pb::UpdateChannelRequest {
                guild_id,
                channel_id,
                name: name.map(str::to_owned),
                icon: icon.map(|i| i.0.clone()),
                color: color.map(|c| c.as_key().to_owned()),
                topic: topic.map(str::to_owned),
                ..Default::default()
            })
            .await?
            .into_owned();
        required(res.channel.into_option())
    }

    /// Creates a section (appended last). Returns it with the resulting layout.
    pub async fn create_section(&self, guild_id: u64, name: &str) -> Result<(Section, GuildLayout)> {
        let res = self
            .guilds()?
            .create_section(pb::CreateSectionRequest { guild_id, name: name.into(), ..Default::default() })
            .await?
            .into_owned();
        Ok((required(res.section.into_option())?, required(res.layout.into_option())?))
    }

    /// Renames a section. Returns it with the resulting layout.
    pub async fn update_section(&self, guild_id: u64, section_id: u64, name: &str) -> Result<(Section, GuildLayout)> {
        let res = self
            .guilds()?
            .update_section(pb::UpdateSectionRequest { guild_id, section_id, name: name.into(), ..Default::default() })
            .await?
            .into_owned();
        Ok((required(res.section.into_option())?, required(res.layout.into_option())?))
    }

    /// Deletes a section; its channels move to the top level. Returns the new layout.
    pub async fn delete_section(&self, guild_id: u64, section_id: u64) -> Result<GuildLayout> {
        let res = self
            .guilds()?
            .delete_section(pb::DeleteSectionRequest { guild_id, section_id, ..Default::default() })
            .await?
            .into_owned();
        required(res.layout.into_option())
    }

    /// Replaces the section order and the per-section channel order.
    ///
    /// `layout` is the desired state based on `layout.version`; a stale
    /// version fails with [`ApiError::Conflict`] (refetch and retry). Returns
    /// the new layout.
    pub async fn update_layout(&self, guild_id: u64, layout: &GuildLayout) -> Result<GuildLayout> {
        let res = self
            .guilds()?
            .update_layout(pb::UpdateLayoutRequest {
                guild_id,
                expected_version: layout.version,
                section_ids: layout.sections.iter().map(|s| s.id).collect(),
                entries: layout.entries.iter().map(Into::into).collect(),
                ..Default::default()
            })
            .await?
            .into_owned();
        required(res.layout.into_option())
    }

    /// Posts a message to a guild channel or DM channel.
    ///
    /// The server rejects empty content and content over
    /// [`MAX_MESSAGE_CHARS`] with [`ApiError::InvalidArgument`]; a `nonce`
    /// is echoed back on the message and on its gateway event.
    pub async fn send_message(&self, channel_id: u64, content: &str, nonce: Option<&str>) -> Result<Message> {
        let res = self
            .messages()?
            .send_message(pb::SendMessageRequest {
                channel_id,
                content: content.into(),
                nonce: nonce.map(str::to_owned),
                ..Default::default()
            })
            .await?
            .into_owned();
        required(res.message.into_option())
    }

    /// Lists messages newest first. Pass the previous page's `next_before` as
    /// `before` to go further back; `limit` is capped by the server at 100.
    pub async fn list_messages(&self, channel_id: u64, before: Option<u64>, limit: u32) -> Result<Page<Message>> {
        let res = self
            .messages()?
            .list_messages(pb::ListMessagesRequest { channel_id, before, limit, ..Default::default() })
            .await?
            .into_owned();
        Ok(Page {
            items: res.messages.into_iter().map(Into::into).collect(),
            next_before: res.next_before,
            has_more: res.has_more,
        })
    }

    /// Opens (or returns the existing) DM channel with `user_id`.
    pub async fn open_dm(&self, user_id: u64) -> Result<DmChannel> {
        let res = self
            .dms()?
            .open_dm(pb::OpenDmRequest { user_id, ..Default::default() })
            .await?
            .into_owned();
        required(res.channel.into_option())
    }

    /// The caller's DM channels, newest first.
    pub async fn list_dms(&self) -> Result<Vec<DmChannel>> {
        let res = self.dms()?.list_dms(pb::ListDmsRequest::default()).await?.into_owned();
        Ok(res.channels.into_iter().map(Into::into).collect())
    }

    /// Opens one gateway stream with the current token.
    pub(crate) async fn open_stream(&self) -> Result<EventStream> {
        let client = pb::GatewayServiceClient::new(self.http.clone(), self.config()?);
        let inner = client.subscribe(pb::SubscribeRequest::default()).await?;
        Ok(EventStream { inner })
    }
}

/// Largest image upload the server accepts (8 MiB); bigger ones are rejected locally.
///
/// The Connect client imposes no request-size limit of its own (its
/// `max_message_size` only bounds responses), so no client config change is needed.
pub const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;

/// Maximum message length in characters accepted by the server.
pub const MAX_MESSAGE_CHARS: usize = 4000;

pub(crate) fn required<T, U: From<T>>(value: Option<T>) -> Result<U> {
    value
        .map(Into::into)
        .ok_or_else(|| ApiError::Other("server returned an incomplete response".into()))
}

/// An open `Subscribe` stream yielding domain events.
pub(crate) struct EventStream {
    inner: connectrpc::client::ServerStream<hyper::body::Incoming, pb::SubscribeResponseView<'static>>,
}

impl EventStream {
    /// Next event; `Ok(None)` when the server ended the stream. Events with
    /// no known payload are skipped.
    pub(crate) async fn next(&mut self) -> Result<Option<Event>> {
        loop {
            let Some(msg) = self.inner.message().await? else {
                return Ok(None);
            };
            let event = msg.to_owned_message().event.into_option();
            if let Some(event) = event.and_then(Event::from_proto) {
                return Ok(Some(event));
            }
        }
    }
}
