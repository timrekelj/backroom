mod common;

use thehideout_server::proto::thehideout::v1::{
    AuthServiceClient, Channel, CreateChannelRequest, CreateGuildRequest, Guild, GuildServiceClient,
    JoinGuildRequest, ListChannelsRequest, ListGuildMembersRequest, ListMyGuildsRequest,
    SignUpRequest,
};
use thehideout_server::services::guild::{GUILD_CREATE_MAX, JOIN_IP_MAX};
use common::TestApp;
use connectrpc::{ConnectError, ErrorCode};

const PASSWORD: &str = "correct horse battery";

/// Signs up `username` and returns its session token.
async fn sign_up(app: &TestApp, username: &str) -> String {
    let client = app.client(AuthServiceClient::new, None);
    client
        .sign_up(SignUpRequest {
            username: app.name(username),
            password: PASSWORD.into(),
            ..Default::default()
        })
        .await
        .expect("sign up")
        .into_owned()
        .token
}

fn guilds(app: &TestApp, token: &str) -> GuildServiceClient<connectrpc::client::HttpClient> {
    app.client(GuildServiceClient::new, Some(token))
}

async fn create_guild(app: &TestApp, token: &str, name: &str) -> Result<Guild, ConnectError> {
    let resp = guilds(app, token)
        .create_guild(CreateGuildRequest {
            name: name.into(),
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().guild.unwrap())
}

async fn create_channel(
    app: &TestApp,
    token: &str,
    guild_id: u64,
    name: &str,
) -> Result<Channel, ConnectError> {
    let resp = guilds(app, token)
        .create_channel(CreateChannelRequest {
            guild_id,
            name: name.into(),
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().channel.unwrap())
}

async fn list_channels(app: &TestApp, token: &str, guild_id: u64) -> Result<Vec<Channel>, ConnectError> {
    let resp = guilds(app, token)
        .list_channels(ListChannelsRequest {
            guild_id,
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().channels)
}

async fn member_names(app: &TestApp, token: &str, guild_id: u64) -> Result<Vec<String>, ConnectError> {
    let resp = guilds(app, token)
        .list_guild_members(ListGuildMembersRequest {
            guild_id,
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().users.into_iter().map(|u| u.username).collect())
}

async fn join(app: &TestApp, token: &str, code: &str) -> Result<Guild, ConnectError> {
    let resp = guilds(app, token)
        .join_guild(JoinGuildRequest {
            invite_code: code.into(),
            ..Default::default()
        })
        .await?;
    Ok(resp.into_owned().guild.unwrap())
}

#[tokio::test]
async fn create_guild_has_invite_and_general_channel() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;

    let guild = create_guild(&app, &alice, "  TheHideout  ").await.expect("create guild");
    assert_ne!(guild.id, 0);
    assert_eq!(guild.name, "TheHideout");
    assert_eq!(guild.invite_code.len(), 8);
    assert!(guild.invite_code.bytes().all(|b| b.is_ascii_alphanumeric()));
    assert!(guild.created_at.is_set());

    let channels = list_channels(&app, &alice, guild.id).await.expect("list channels");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "general");
    assert_eq!(channels[0].guild_id, guild.id);

    app.cleanup().await;
}

#[tokio::test]
async fn list_my_guilds_for_creator() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;

    let first = create_guild(&app, &alice, "one").await.unwrap();
    let second = create_guild(&app, &alice, "two").await.unwrap();

    let mine = guilds(&app, &alice)
        .list_my_guilds(ListMyGuildsRequest::default())
        .await
        .expect("list")
        .into_owned()
        .guilds;
    assert_eq!(mine, vec![first, second]);

    let theirs = guilds(&app, &bob)
        .list_my_guilds(ListMyGuildsRequest::default())
        .await
        .expect("list")
        .into_owned()
        .guilds;
    assert!(theirs.is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn join_via_invite_and_list_members() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let guild = create_guild(&app, &alice, "club").await.unwrap();

    let joined = join(&app, &bob, &format!(" {} ", guild.invite_code)).await.expect("join");
    assert_eq!(joined, guild);

    for token in [&alice, &bob] {
        let names = member_names(&app, token, guild.id).await.expect("members");
        assert_eq!(names, [app.name("alice"), app.name("bob")]);
    }

    let bobs = guilds(&app, &bob)
        .list_my_guilds(ListMyGuildsRequest::default())
        .await
        .unwrap()
        .into_owned()
        .guilds;
    assert_eq!(bobs, vec![guild.clone()]);
    // Members can see channels too.
    assert_eq!(list_channels(&app, &bob, guild.id).await.unwrap().len(), 1);

    app.cleanup().await;
}

#[tokio::test]
async fn joining_twice_is_idempotent() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let guild = create_guild(&app, &alice, "club").await.unwrap();

    let first = join(&app, &bob, &guild.invite_code).await.unwrap();
    let second = join(&app, &bob, &guild.invite_code).await.unwrap();
    assert_eq!(first, second);
    // The creator re-joining their own guild is a no-op as well.
    assert_eq!(join(&app, &alice, &guild.invite_code).await.unwrap(), guild);

    assert_eq!(member_names(&app, &alice, guild.id).await.unwrap(), [app.name("alice"), app.name("bob")]);

    app.cleanup().await;
}

#[tokio::test]
async fn invalid_invite_is_not_found() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;

    for code in ["nope1234", "", "   "] {
        let err = join(&app, &alice, code).await.expect_err("invalid invite");
        assert_eq!(err.code, ErrorCode::NotFound, "code {code:?}");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn non_members_get_not_found() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let mallory = sign_up(&app, "mallory").await;
    let guild = create_guild(&app, &alice, "private").await.unwrap();

    let err = list_channels(&app, &mallory, guild.id).await.expect_err("channels");
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = member_names(&app, &mallory, guild.id).await.expect_err("members");
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = create_channel(&app, &mallory, guild.id, "sneaky").await.expect_err("create channel");
    assert_eq!(err.code, ErrorCode::NotFound);

    // A nonexistent guild looks identical to one the caller isn't in.
    let err = list_channels(&app, &mallory, 12345).await.expect_err("missing guild");
    assert_eq!(err.code, ErrorCode::NotFound);

    // And nothing was created.
    assert_eq!(list_channels(&app, &alice, guild.id).await.unwrap().len(), 1);

    app.cleanup().await;
}

#[tokio::test]
async fn channel_names_are_normalized_and_validated() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let guild = create_guild(&app, &alice, "club").await.unwrap();

    let channel = create_channel(&app, &alice, guild.id, "  My   Channel ").await.expect("create");
    assert_eq!(channel.name, "my-channel");
    assert_eq!(channel.guild_id, guild.id);
    assert!(channel.created_at.is_set());
    create_channel(&app, &alice, guild.id, "dev_chat-2").await.expect("valid chars");

    let too_long = "a".repeat(101);
    for name in ["", "   ", "bad!", "émile", "a/b", too_long.as_str()] {
        let err = create_channel(&app, &alice, guild.id, name).await.expect_err("invalid name");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "name {name:?}");
    }

    let channels = list_channels(&app, &alice, guild.id).await.unwrap();
    let names: Vec<_> = channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["general", "my-channel", "dev_chat-2"]);
    assert!(channels.windows(2).all(|w| w[0].id < w[1].id));

    app.cleanup().await;
}

#[tokio::test]
async fn invalid_guild_names_are_rejected() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;

    for name in ["", "   "] {
        let err = create_guild(&app, &alice, name).await.expect_err("empty name");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "name {name:?}");
    }
    let err = create_guild(&app, &alice, &"g".repeat(101)).await.expect_err("too long");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    create_guild(&app, &alice, &"g".repeat(100)).await.expect("max length");

    app.cleanup().await;
}

#[tokio::test]
async fn only_the_owner_can_create_channels() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;
    let bob = sign_up(&app, "bob").await;
    let guild = create_guild(&app, &alice, "club").await.unwrap();
    join(&app, &bob, &guild.invite_code).await.expect("join");

    let err = create_channel(&app, &bob, guild.id, "nope").await.expect_err("non-owner");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    // The owner still can, and nothing was created for bob.
    create_channel(&app, &alice, guild.id, "yes").await.expect("owner");
    assert_eq!(list_channels(&app, &alice, guild.id).await.unwrap().len(), 2);

    app.cleanup().await;
}

#[tokio::test]
async fn guild_creation_is_rate_limited_per_user() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;

    for i in 0..GUILD_CREATE_MAX {
        create_guild(&app, &alice, &format!("g{i}")).await.expect("within limit");
    }
    let err = create_guild(&app, &alice, "one-too-many").await.expect_err("over the limit");
    assert_eq!(err.code, ErrorCode::ResourceExhausted);

    app.cleanup().await;
}

#[tokio::test]
async fn join_is_rate_limited_per_ip() {
    let app = TestApp::start().await;
    let alice = sign_up(&app, "alice").await;

    for i in 0..JOIN_IP_MAX {
        let err = join(&app, &alice, &format!("bad{i:05}")).await.expect_err("invalid invite");
        assert_eq!(err.code, ErrorCode::NotFound, "attempt {i}");
    }
    let err = join(&app, &alice, "badzzzzz").await.expect_err("over the limit");
    assert_eq!(err.code, ErrorCode::ResourceExhausted);

    app.cleanup().await;
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let app = TestApp::start().await;
    let client = app.client(GuildServiceClient::new, None);

    let err = client.create_guild(CreateGuildRequest::default()).await.expect_err("create guild");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let err = client.list_my_guilds(ListMyGuildsRequest::default()).await.expect_err("list guilds");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let err = client.join_guild(JoinGuildRequest::default()).await.expect_err("join");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let err = client.list_guild_members(ListGuildMembersRequest::default()).await.expect_err("members");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let err = client.create_channel(CreateChannelRequest::default()).await.expect_err("create channel");
    assert_eq!(err.code, ErrorCode::Unauthenticated);
    let err = client.list_channels(ListChannelsRequest::default()).await.expect_err("channels");
    assert_eq!(err.code, ErrorCode::Unauthenticated);

    let client = app.client(GuildServiceClient::new, Some("garbage"));
    let err = client.list_my_guilds(ListMyGuildsRequest::default()).await.expect_err("bad token");
    assert_eq!(err.code, ErrorCode::Unauthenticated);

    app.cleanup().await;
}
