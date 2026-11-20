//! End-to-end tests of roles, layout, channel look, media and admin against the
//! real server running in-process on a throwaway ScyllaDB keyspace (needs
//! ScyllaDB at 127.0.0.1:9042: `docker compose up -d`).
//!
//! Tests for RPCs the server still stubs as `unimplemented` are `#[ignore]`d;
//! run them with `cargo test -- --ignored` once the server implements them.

use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use scylla::client::caching_session::CachingSession;
use scylla::client::session_builder::SessionBuilder;
use thehideout_api::{
    ApiError, ChannelIcon, Client, Event, Gateway, GatewayConfig, GatewayUpdate, LayoutEntry, PaletteColor,
    Permissions,
};
use thehideout_server::snowflake::Snowflake;
use thehideout_server::{AppState, build_app, migrations};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::timeout;

const PASSWORD: &str = "correct horse battery";
const HEARTBEAT: Duration = Duration::from_millis(200);
const WAIT: Duration = Duration::from_secs(10);

struct Server {
    url: String,
    state: AppState,
}

impl Server {
    async fn start() -> Self {
        let session = SessionBuilder::new()
            .known_node("127.0.0.1:9042")
            .build()
            .await
            .expect("connect to ScyllaDB at 127.0.0.1:9042 (is `docker compose up -d` running?)");
        let keyspace = format!("test_{:016x}", rand::rng().random::<u64>());
        migrations::run(&session, &keyspace).await.expect("run migrations");
        let session = Arc::new(CachingSession::from(session, 64));
        let state = AppState::new(session, Arc::new(Snowflake::new(0)), keyspace, HEARTBEAT);
        let app = build_app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .unwrap();
        });
        Self { url: format!("http://{addr}"), state }
    }

    async fn cleanup(self) {
        let _ = self
            .state
            .session
            .get_session()
            .query_unpaged(format!("DROP KEYSPACE IF EXISTS {}", self.state.keyspace), ())
            .await;
    }
}

fn spawn_gateway(client: Client) -> (thehideout_api::GatewayHandle, UnboundedReceiver<GatewayUpdate>) {
    let (tx, rx) = unbounded_channel();
    let config = GatewayConfig { heartbeat_interval: HEARTBEAT, ..Default::default() };
    let handle = Gateway::spawn_with(client, config, move |u| {
        let _ = tx.send(u);
    });
    (handle, rx)
}

/// Waits for the first update matching `pred`, skipping others.
async fn expect_update(rx: &mut UnboundedReceiver<GatewayUpdate>, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
    timeout(WAIT, async {
        loop {
            if let GatewayUpdate::Event(event) = rx.recv().await.expect("gateway stopped")
                && pred(&event)
            {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

#[tokio::test]
async fn roles_and_member_roles() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    let (_, bob_user) = bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();

    let (_alice_gateway, mut alice_rx) = spawn_gateway(alice.clone());
    let (_bob_gateway, mut bob_rx) = spawn_gateway(bob.clone());
    expect_update_connected(&mut alice_rx).await;
    expect_update_connected(&mut bob_rx).await;

    let initial = alice.list_roles(guild.id).await.expect("list roles");
    let (mods, v1) = alice
        .create_role(guild.id, "mods", PaletteColor::Red, Permissions::MANAGE_CHANNELS)
        .await
        .expect("create role");
    assert_eq!((mods.color, mods.permissions), (PaletteColor::Red, Permissions::MANAGE_CHANNELS));
    assert!(v1 > initial.version);
    let event = expect_update(&mut bob_rx, "RolesUpdated", |e| matches!(e, Event::RolesUpdated { .. })).await;
    let Event::RolesUpdated { guild_id, roles, version } = event else { unreachable!() };
    assert_eq!((guild_id, version), (guild.id, v1));
    assert!(roles.iter().any(|r| r.id == mods.id));

    let helpers = alice.create_role(guild.id, "helpers", PaletteColor::None, Permissions::NONE).await.unwrap().0;
    let renamed = alice
        .update_role(guild.id, helpers.id, Some("crew"), Some(PaletteColor::Teal), Some(Permissions::MANAGE_ROLES))
        .await
        .unwrap();
    assert_eq!((renamed.name.as_str(), renamed.color), ("crew", PaletteColor::Teal));
    assert!(renamed.permissions.contains(Permissions::MANAGE_ROLES));

    // Reorder: stale version conflicts, current one succeeds.
    let listed = alice.list_roles(guild.id).await.unwrap();
    let mut order: Vec<u64> = listed.roles.iter().map(|r| r.id).collect();
    order.reverse();
    assert_eq!(alice.reorder_roles(guild.id, listed.version + 100, &order).await.unwrap_err(), ApiError::Conflict);
    let reordered = alice.reorder_roles(guild.id, listed.version, &order).await.expect("reorder");
    assert_eq!(reordered.roles.iter().map(|r| r.id).collect::<Vec<_>>(), order);

    // Assign to bob; members expose role ids and bob gets MemberUpdated.
    let member = alice.set_member_roles(guild.id, bob_user.id, &[mods.id]).await.expect("set member roles");
    assert_eq!(member.role_ids, vec![mods.id]);
    expect_update(&mut bob_rx, "MemberUpdated", |e| matches!(e, Event::MemberUpdated { member, .. } if member.user.id == bob_user.id)).await;
    let members = alice.list_members(guild.id).await.unwrap();
    assert_eq!(members.iter().find(|m| m.user.id == bob_user.id).unwrap().role_ids, vec![mods.id]);
    assert_eq!(alice.list_guild_members(guild.id).await.unwrap().len(), 2);

    // Bob now has MANAGE_CHANNELS but not MANAGE_ROLES.
    assert_eq!(
        bob.create_role(guild.id, "x", PaletteColor::None, Permissions::NONE).await.unwrap_err(),
        ApiError::PermissionDenied
    );
    assert!(matches!(
        alice.create_role(guild.id, "bad", PaletteColor::None, Permissions(1 << 40)).await,
        Err(ApiError::InvalidArgument(_))
    ));

    let version = alice.delete_role(guild.id, helpers.id).await.expect("delete role");
    assert!(version > listed.version);
    assert!(alice.list_roles(guild.id).await.unwrap().roles.iter().all(|r| r.id != helpers.id));

    server.cleanup().await;
}

async fn expect_update_connected(rx: &mut UnboundedReceiver<GatewayUpdate>) {
    timeout(WAIT, async {
        while rx.recv().await != Some(GatewayUpdate::Connected) {}
    })
    .await
    .expect("gateway connects");
}

#[tokio::test]
async fn layout_sections_and_channel_look() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    bob.sign_up("bob", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    bob.join_guild(&guild.invite_code).await.unwrap();
    let (_handle, mut rx) = spawn_gateway(bob.clone());
    expect_update_connected(&mut rx).await;

    // Guild rename.
    let renamed = alice.update_guild(guild.id, Some("pals")).await.expect("update guild");
    assert_eq!(renamed.name, "pals");
    expect_update(&mut rx, "GuildUpdated", |e| matches!(e, Event::GuildUpdated(g) if g.name == "pals")).await;
    assert_eq!(bob.update_guild(guild.id, Some("mine")).await.unwrap_err(), ApiError::PermissionDenied);

    // Sections.
    let (channels, layout) = alice.list_channels_with_layout(guild.id).await.unwrap();
    assert_eq!(channels.len(), 1);
    assert!(layout.sections.is_empty());
    assert_eq!(layout.entries.iter().find(|e| e.section_id.is_none()).unwrap().channel_ids, vec![channels[0].id]);

    let (section, layout) = alice.create_section(guild.id, "voice").await.expect("create section");
    assert_eq!(section.name, "voice");
    expect_update(&mut rx, "LayoutUpdated", |e| matches!(e, Event::LayoutUpdated { layout: l, .. } if l.sections.len() == 1)).await;
    let (section, layout2) = alice.update_section(guild.id, section.id, "talk").await.unwrap();
    assert_eq!(section.name, "talk");
    assert!(layout2.version > layout.version);

    // New channel in the section, with look.
    let icon = ChannelIcon::new("game-controller");
    let games = alice
        .create_channel_with(guild.id, "games", Some(section.id), &icon, PaletteColor::Indigo)
        .await
        .expect("create channel with look");
    assert_eq!((games.section_id, games.icon.clone(), games.color), (Some(section.id), icon.clone(), PaletteColor::Indigo));

    let updated = alice
        .update_channel(guild.id, games.id, Some("gaming"), Some(&ChannelIcon::default()), Some(PaletteColor::None), Some("play"))
        .await
        .expect("update channel");
    assert_eq!((updated.name.as_str(), updated.topic.as_str(), updated.color), ("gaming", "play", PaletteColor::None));
    assert!(updated.icon.is_default());
    expect_update(&mut rx, "ChannelUpdated", |e| matches!(e, Event::ChannelUpdated(c) if c.topic == "play")).await;
    assert!(matches!(
        alice.update_channel(guild.id, games.id, None, Some(&ChannelIcon::new("Bad Icon")), None, None).await,
        Err(ApiError::InvalidArgument(_))
    ));
    assert_eq!(
        bob.update_channel(guild.id, games.id, Some("hax"), None, None, None).await.unwrap_err(),
        ApiError::PermissionDenied
    );

    // Move the first channel into the section and back out of order; stale version conflicts.
    let (channels, mut current) = alice.list_channels_with_layout(guild.id).await.unwrap();
    let lobby = channels.iter().find(|c| c.id != games.id).unwrap().id;
    let stale = current.clone();
    current.entries = vec![
        LayoutEntry { section_id: None, channel_ids: vec![] },
        LayoutEntry { section_id: Some(section.id), channel_ids: vec![games.id, lobby] },
    ];
    let applied = alice.update_layout(guild.id, &current).await.expect("update layout");
    assert!(applied.version > current.version);
    assert_eq!(applied.entries.iter().find(|e| e.section_id == Some(section.id)).unwrap().channel_ids, vec![games.id, lobby]);
    assert_eq!(alice.update_layout(guild.id, &stale).await.unwrap_err(), ApiError::Conflict);
    let (channels, _) = alice.list_channels_with_layout(guild.id).await.unwrap();
    assert!(channels.iter().all(|c| c.section_id == Some(section.id)));

    // Deleting the section moves its channels to the top level.
    let layout = alice.delete_section(guild.id, section.id).await.expect("delete section");
    assert!(layout.sections.is_empty());
    assert_eq!(layout.entries.iter().find(|e| e.section_id.is_none()).unwrap().channel_ids, vec![games.id, lobby]);
    assert_eq!(
        bob.create_section(guild.id, "nope").await.unwrap_err(),
        ApiError::PermissionDenied
    );

    server.cleanup().await;
}

// 1x1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35, 0x81, 0x84,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test]
#[ignore = "server: not implemented yet"]
async fn media_upload_and_fetch() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    let guild = alice.create_guild("friends").await.unwrap();
    let (_handle, mut rx) = spawn_gateway(alice.clone());
    expect_update_connected(&mut rx).await;

    let user = alice.upload_avatar(PNG.to_vec(), "image/png").await.expect("upload avatar");
    let key = user.avatar.clone().expect("avatar key");
    expect_update(&mut rx, "UserUpdated", |e| matches!(e, Event::UserUpdated(u) if u.avatar.as_deref() == Some(&key))).await;
    assert!(alice.media_url(&key).ends_with(&format!("/media/{key}")));
    assert!(!alice.fetch_media(&key).await.expect("fetch avatar").is_empty());
    assert_eq!(alice.fetch_media("missing").await.unwrap_err(), ApiError::NotFound);
    assert_eq!(alice.remove_avatar().await.unwrap().avatar, None);

    let guild = alice.upload_guild_icon(guild.id, PNG.to_vec(), "image/png").await.expect("upload icon");
    assert!(guild.icon.is_some());
    assert_eq!(alice.remove_guild_icon(guild.id).await.unwrap().icon, None);
    assert!(matches!(alice.upload_avatar(b"not an image".to_vec(), "image/png").await, Err(ApiError::InvalidArgument(_))));

    server.cleanup().await;
}

#[tokio::test]
#[ignore = "server: not implemented yet"]
async fn admin_dashboard() {
    let server = Server::start().await;
    let alice = Client::new(&server.url);
    let bob = Client::new(&server.url);
    alice.sign_up("alice", PASSWORD).await.unwrap();
    let (_, bob_user) = bob.sign_up("bob", PASSWORD).await.unwrap();

    // Not an admin unless THEHIDEOUT_ADMINS lists the user; bob never is.
    assert_eq!(bob.admin_overview().await.unwrap_err(), ApiError::PermissionDenied);

    // Needs `THEHIDEOUT_ADMINS=alice` in the server environment.
    let overview = alice.admin_overview().await.expect("overview");
    assert!(overview.users >= 2);
    assert_eq!(alice.admin_activity(7).await.unwrap().len(), 7);
    let users = alice.admin_list_users("bo", None, 10).await.unwrap();
    assert!(users.items.iter().any(|r| r.user.id == bob_user.id));
    let guilds = alice.admin_list_guilds("", None, 10).await.unwrap();
    assert!(guilds.next_page_token.is_none());
    assert!(alice.admin_set_admin(bob_user.id, true).await.unwrap().is_admin);

    server.cleanup().await;
}
