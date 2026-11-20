//! Helpers for the roles / layout / events tests.
#![allow(dead_code)]

use connectrpc::ConnectError;
use connectrpc::client::HttpClient;
use thehideout_server::proto::thehideout::v1::*;

use super::TestApp;

pub const PASSWORD: &str = "correct horse battery";

pub const ADMINISTRATOR: u64 = 1;
pub const MANAGE_SERVER: u64 = 2;
pub const MANAGE_CHANNELS: u64 = 4;
pub const MANAGE_ROLES: u64 = 8;
pub const MANAGE_EMOJIS: u64 = 16;

pub struct Account {
    pub token: String,
    pub id: u64,
}

pub async fn user(app: &TestApp, username: &str) -> Account {
    let resp = app
        .client(AuthServiceClient::new, None)
        .sign_up(SignUpRequest {
            username: username.into(),
            password: PASSWORD.into(),
            ..Default::default()
        })
        .await
        .expect("sign up")
        .into_owned();
    Account {
        token: resp.token,
        id: resp.user.as_option().expect("user").id,
    }
}

pub fn guilds(app: &TestApp, who: &Account) -> GuildServiceClient<HttpClient> {
    app.client(GuildServiceClient::new, Some(&who.token))
}

pub fn roles(app: &TestApp, who: &Account) -> RoleServiceClient<HttpClient> {
    app.client(RoleServiceClient::new, Some(&who.token))
}

pub async fn guild(app: &TestApp, owner: &Account) -> Guild {
    guilds(app, owner)
        .create_guild(CreateGuildRequest {
            name: "guild".into(),
            ..Default::default()
        })
        .await
        .expect("create guild")
        .into_owned()
        .guild
        .unwrap()
}

pub async fn join(app: &TestApp, who: &Account, guild: &Guild) {
    guilds(app, who)
        .join_guild(JoinGuildRequest {
            invite_code: guild.invite_code.clone(),
            ..Default::default()
        })
        .await
        .expect("join guild");
}

pub async fn role(
    app: &TestApp,
    who: &Account,
    guild_id: u64,
    name: &str,
    permissions: u64,
) -> Result<Role, ConnectError> {
    Ok(roles(app, who)
        .create_role(CreateRoleRequest {
            guild_id,
            name: name.into(),
            permissions,
            ..Default::default()
        })
        .await?
        .into_owned()
        .role
        .unwrap())
}

pub async fn list_roles(app: &TestApp, who: &Account, guild_id: u64) -> Result<ListRolesResponse, ConnectError> {
    Ok(roles(app, who)
        .list_roles(ListRolesRequest {
            guild_id,
            ..Default::default()
        })
        .await?
        .into_owned())
}

pub async fn set_roles(
    app: &TestApp,
    who: &Account,
    guild_id: u64,
    user_id: u64,
    role_ids: &[u64],
) -> Result<Member, ConnectError> {
    Ok(roles(app, who)
        .set_member_roles(SetMemberRolesRequest {
            guild_id,
            user_id,
            role_ids: role_ids.to_vec(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .member
        .unwrap())
}

pub async fn channels(app: &TestApp, who: &Account, guild_id: u64) -> Result<ListChannelsResponse, ConnectError> {
    Ok(guilds(app, who)
        .list_channels(ListChannelsRequest {
            guild_id,
            ..Default::default()
        })
        .await?
        .into_owned())
}

pub async fn channel(app: &TestApp, who: &Account, guild_id: u64, name: &str) -> Result<Channel, ConnectError> {
    Ok(guilds(app, who)
        .create_channel(CreateChannelRequest {
            guild_id,
            name: name.into(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .channel
        .unwrap())
}

pub async fn section(app: &TestApp, who: &Account, guild_id: u64, name: &str) -> Result<Section, ConnectError> {
    Ok(guilds(app, who)
        .create_section(CreateSectionRequest {
            guild_id,
            name: name.into(),
            ..Default::default()
        })
        .await?
        .into_owned()
        .section
        .unwrap())
}

/// The channel ids of `section_id` (0 = top level) in `layout`.
pub fn entry(layout: &GuildLayout, section_id: u64) -> Vec<u64> {
    layout
        .entries
        .iter()
        .find(|e| e.section_id == section_id)
        .map(|e| e.channel_ids.clone())
        .unwrap_or_default()
}

pub fn layout_entry(section_id: u64, channel_ids: &[u64]) -> LayoutEntry {
    LayoutEntry {
        section_id,
        channel_ids: channel_ids.to_vec(),
        ..Default::default()
    }
}

pub async fn update_layout(
    app: &TestApp,
    who: &Account,
    guild_id: u64,
    expected_version: u64,
    section_ids: &[u64],
    entries: Vec<LayoutEntry>,
) -> Result<GuildLayout, ConnectError> {
    Ok(guilds(app, who)
        .update_layout(UpdateLayoutRequest {
            guild_id,
            expected_version,
            section_ids: section_ids.to_vec(),
            entries,
            ..Default::default()
        })
        .await?
        .into_owned()
        .layout
        .unwrap())
}
