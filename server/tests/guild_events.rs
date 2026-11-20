//! Gateway events for guild settings, layout, roles and member changes.
mod common;

use std::time::Duration;

use common::TestApp;
use common::fixtures::*;
use thehideout_server::proto::thehideout::v1::event::Payload;
use thehideout_server::proto::thehideout::v1::*;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

async fn start() -> TestApp {
    TestApp::start_with_heartbeat(Duration::from_millis(200)).await
}

macro_rules! subscribe {
    ($app:expr, $who:expr) => {
        $app.client(GatewayServiceClient::new, Some(&$who.token))
            .subscribe(SubscribeRequest::default())
            .await
            .expect("subscribe")
    };
}

/// Next non-heartbeat event payload.
macro_rules! next_payload {
    ($stream:expr) => {{
        loop {
            let msg = timeout(WAIT, $stream.message())
                .await
                .expect("timed out waiting for an event")
                .expect("stream error")
                .expect("stream ended");
            let event = msg.to_owned_message().event.into_option().expect("event");
            match event.payload {
                Some(Payload::Heartbeat(_)) | None => continue,
                Some(p) => break p,
            }
        }
    }};
}

/// Every non-heartbeat payload arriving within `window`.
macro_rules! payloads_within {
    ($stream:expr, $window:expr) => {{
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + $window;
        while let Ok(res) = tokio::time::timeout_at(deadline, $stream.message()).await {
            let event = res.unwrap().unwrap().to_owned_message().event.into_option().unwrap();
            if !matches!(event.payload, Some(Payload::Heartbeat(_)) | None) {
                out.push(event.payload.unwrap());
            }
        }
        out
    }};
}

#[tokio::test]
async fn guild_and_channel_updates_reach_other_members() {
    let app = start().await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let mut stream = subscribe!(app, bob);

    guilds(&app, &alice)
        .update_guild(UpdateGuildRequest {
            guild_id: guild.id,
            name: Some("Renamed".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let Payload::GuildUpdated(g) = next_payload!(stream) else { panic!("expected GuildUpdated") };
    assert_eq!((g.id, g.name.as_str()), (guild.id, "Renamed"));

    guilds(&app, &alice)
        .update_channel(UpdateChannelRequest {
            guild_id: guild.id,
            channel_id: general,
            icon: Some("hash".into()),
            color: Some("pink".into()),
            topic: Some("welcome".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let Payload::ChannelUpdated(c) = next_payload!(stream) else { panic!("expected ChannelUpdated") };
    assert_eq!((c.id, c.icon.as_str(), c.color.as_str(), c.topic.as_str()), (general, "hash", "pink", "welcome"));
    app.cleanup().await;
}

#[tokio::test]
async fn layout_changes_publish_layout_updated() {
    let app = start().await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let mut stream = subscribe!(app, bob);

    // CreateSection
    let s = section(&app, &alice, guild.id, "s").await.unwrap();
    let Payload::LayoutUpdated(u) = next_payload!(stream) else { panic!("expected LayoutUpdated") };
    let layout = u.layout.unwrap();
    assert_eq!((u.guild_id, layout.version), (guild.id, 1));
    assert_eq!(layout.sections[0].id, s.id);

    // UpdateSection
    guilds(&app, &alice)
        .update_section(UpdateSectionRequest { guild_id: guild.id, section_id: s.id, name: "t".into(), ..Default::default() })
        .await
        .unwrap();
    let Payload::LayoutUpdated(u) = next_payload!(stream) else { panic!("expected LayoutUpdated") };
    let layout = u.layout.unwrap();
    assert_eq!((layout.version, layout.sections[0].name.as_str()), (2, "t"));

    // CreateChannel: ChannelCreated then LayoutUpdated carrying the new channel.
    let c = channel(&app, &alice, guild.id, "new").await.unwrap();
    let Payload::ChannelCreated(created) = next_payload!(stream) else { panic!("expected ChannelCreated") };
    assert_eq!(created.id, c.id);
    let Payload::LayoutUpdated(u) = next_payload!(stream) else { panic!("expected LayoutUpdated") };
    let layout = u.layout.unwrap();
    assert_eq!(layout.version, 3);
    assert_eq!(entry(&layout, 0), vec![general, c.id]);

    // UpdateLayout
    update_layout(
        &app,
        &alice,
        guild.id,
        3,
        &[s.id],
        vec![layout_entry(0, &[general]), layout_entry(s.id, &[c.id])],
    )
    .await
    .unwrap();
    let Payload::LayoutUpdated(u) = next_payload!(stream) else { panic!("expected LayoutUpdated") };
    let layout = u.layout.unwrap();
    assert_eq!(layout.version, 4);
    assert_eq!(entry(&layout, s.id), vec![c.id]);

    // DeleteSection
    guilds(&app, &alice)
        .delete_section(DeleteSectionRequest { guild_id: guild.id, section_id: s.id, ..Default::default() })
        .await
        .unwrap();
    let Payload::LayoutUpdated(u) = next_payload!(stream) else { panic!("expected LayoutUpdated") };
    let layout = u.layout.unwrap();
    assert_eq!(layout.version, 5);
    assert!(layout.sections.is_empty());
    assert_eq!(entry(&layout, 0), vec![general, c.id]);

    // A rejected edit publishes nothing.
    let _ = update_layout(&app, &alice, guild.id, 1, &[], vec![]).await.unwrap_err();
    assert!(payloads_within!(stream, Duration::from_millis(500)).is_empty());
    app.cleanup().await;
}

#[tokio::test]
async fn role_changes_publish_roles_and_member_events() {
    let app = start().await;
    let (alice, bob, carol) = (user(&app, "alice").await, user(&app, "bob").await, user(&app, "carol").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    join(&app, &carol, &guild).await;
    let mut stream = subscribe!(app, bob);

    // CreateRole -> RolesUpdated with the full list and version.
    let a = role(&app, &alice, guild.id, "a", MANAGE_CHANNELS).await.unwrap();
    let Payload::RolesUpdated(r) = next_payload!(stream) else { panic!("expected RolesUpdated") };
    assert_eq!((r.guild_id, r.version), (guild.id, 1));
    assert_eq!(r.roles.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a.id]);
    let b = role(&app, &alice, guild.id, "b", 0).await.unwrap();
    let Payload::RolesUpdated(r) = next_payload!(stream) else { panic!("expected RolesUpdated") };
    assert_eq!((r.version, r.roles.iter().map(|x| x.id).collect::<Vec<_>>()), (2, vec![a.id, b.id]));

    // UpdateRole
    roles(&app, &alice)
        .update_role(UpdateRoleRequest { guild_id: guild.id, role_id: b.id, name: Some("b2".into()), ..Default::default() })
        .await
        .unwrap();
    let Payload::RolesUpdated(r) = next_payload!(stream) else { panic!("expected RolesUpdated") };
    assert_eq!((r.version, r.roles[1].name.as_str()), (2, "b2"));

    // ReorderRoles
    roles(&app, &alice)
        .reorder_roles(ReorderRolesRequest { guild_id: guild.id, expected_version: 2, role_ids: vec![b.id, a.id], ..Default::default() })
        .await
        .unwrap();
    let Payload::RolesUpdated(r) = next_payload!(stream) else { panic!("expected RolesUpdated") };
    assert_eq!((r.version, r.roles.iter().map(|x| x.id).collect::<Vec<_>>()), (3, vec![b.id, a.id]));

    // SetMemberRoles -> MemberUpdated (for the whole guild, including bystanders).
    set_roles(&app, &alice, guild.id, carol.id, &[a.id, b.id]).await.unwrap();
    let Payload::MemberUpdated(m) = next_payload!(stream) else { panic!("expected MemberUpdated") };
    let member = m.member.unwrap();
    assert_eq!((m.guild_id, member.user.as_option().unwrap().id), (guild.id, carol.id));
    let mut expected = vec![a.id, b.id];
    expected.sort_unstable();
    assert_eq!(member.role_ids, expected);

    // DeleteRole -> RolesUpdated, then MemberUpdated for each affected member.
    roles(&app, &alice)
        .delete_role(DeleteRoleRequest { guild_id: guild.id, role_id: a.id, ..Default::default() })
        .await
        .unwrap();
    let Payload::RolesUpdated(r) = next_payload!(stream) else { panic!("expected RolesUpdated") };
    assert_eq!((r.version, r.roles.iter().map(|x| (x.id, x.position)).collect::<Vec<_>>()), (4, vec![(b.id, 0)]));
    let Payload::MemberUpdated(m) = next_payload!(stream) else { panic!("expected MemberUpdated") };
    let member = m.member.unwrap();
    assert_eq!((member.user.as_option().unwrap().id, member.role_ids), (carol.id, vec![b.id]));
    app.cleanup().await;
}

#[tokio::test]
async fn events_stay_inside_their_guild() {
    let app = start().await;
    let (alice, bob, eve) = (user(&app, "alice").await, user(&app, "bob").await, user(&app, "eve").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let eves_guild = self::guild(&app, &eve).await;
    let mut eve_stream = subscribe!(app, eve);
    let mut bob_stream = subscribe!(app, bob);

    section(&app, &alice, guild.id, "s").await.unwrap();
    role(&app, &alice, guild.id, "r", 0).await.unwrap();
    guilds(&app, &alice)
        .update_guild(UpdateGuildRequest { guild_id: guild.id, name: Some("x".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(payloads_within!(bob_stream, Duration::from_millis(800)).len(), 3);
    assert!(payloads_within!(eve_stream, Duration::from_millis(100)).is_empty());
    let _ = eves_guild;
    app.cleanup().await;
}
