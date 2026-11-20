mod common;

use chrono::Utc;
use common::TestApp;
use common::fixtures::*;
use connectrpc::ErrorCode;
use thehideout_server::layout::MAX_SECTIONS_PER_GUILD;
use thehideout_server::proto::thehideout::v1::*;

#[tokio::test]
async fn fresh_guild_has_a_top_level_general() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let listing = channels(&app, &alice, guild.id).await.unwrap();
    let layout = listing.layout.as_option().expect("layout");
    assert_eq!(layout.version, 0);
    assert!(layout.sections.is_empty());
    assert_eq!(layout.entries.len(), 1);
    assert_eq!(entry(layout, 0), vec![listing.channels[0].id]);
    let general = &listing.channels[0];
    assert_eq!((general.section_id, general.position), (None, 0));
    assert_eq!((general.icon.as_str(), general.color.as_str(), general.topic.as_str()), ("", "", ""));
    app.cleanup().await;
}

#[tokio::test]
async fn create_channel_with_section_icon_and_color() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let sec = section(&app, &alice, guild.id, "Games").await.unwrap();

    let create = |name: &str, section_id: Option<u64>, icon: &str, color: &str| {
        let client = guilds(&app, &alice);
        let req = CreateChannelRequest {
            guild_id: guild.id,
            name: name.into(),
            section_id,
            icon: icon.into(),
            color: color.into(),
            ..Default::default()
        };
        async move { client.create_channel(req).await.map(|r| r.into_owned().channel.unwrap()) }
    };
    let chess = create("chess", Some(sec.id), "crown-simple", "amber").await.unwrap();
    assert_eq!(chess.section_id, Some(sec.id));
    assert_eq!((chess.icon.as_str(), chess.color.as_str()), ("crown-simple", "amber"));
    let go = create("go", Some(sec.id), "", "").await.unwrap();
    assert_eq!((go.section_id, go.position), (Some(sec.id), 1));
    let top = create("top", None, "", "").await.unwrap();
    assert_eq!((top.section_id, top.position), (None, 1));

    assert_eq!(create("x", Some(424242), "", "").await.unwrap_err().code, ErrorCode::NotFound);
    assert_eq!(create("x", None, "Bad Icon", "").await.unwrap_err().code, ErrorCode::InvalidArgument);
    assert_eq!(create("x", None, "", "magenta").await.unwrap_err().code, ErrorCode::InvalidArgument);
    // A rejected create leaves nothing behind.
    let listing = channels(&app, &alice, guild.id).await.unwrap();
    assert_eq!(listing.channels.len(), 4);
    let layout = listing.layout.unwrap();
    assert_eq!(entry(&layout, sec.id), vec![chess.id, go.id]);
    // section (1) + chess + go + top = 4 structural bumps.
    assert_eq!(layout.version, 4);
    app.cleanup().await;
}

#[tokio::test]
async fn section_crud_and_delete_moves_channels_to_top_level() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;

    let one = section(&app, &alice, guild.id, "  One ").await.unwrap();
    let two = section(&app, &alice, guild.id, "Two").await.unwrap();
    assert_eq!((one.name.as_str(), one.position, two.position), ("One", 0, 1));
    assert_eq!(section(&app, &alice, guild.id, " ").await.unwrap_err().code, ErrorCode::InvalidArgument);
    assert_eq!(
        section(&app, &alice, guild.id, &"s".repeat(101)).await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );

    // Rename bumps the version and keeps everything else.
    let renamed = guilds(&app, &alice)
        .update_section(UpdateSectionRequest {
            guild_id: guild.id,
            section_id: one.id,
            name: "Uno".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    assert_eq!(renamed.section.unwrap().name, "Uno");
    let layout = renamed.layout.unwrap();
    assert_eq!(layout.version, 3);
    assert_eq!(layout.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Uno", "Two"]);
    let err = guilds(&app, &alice)
        .update_section(UpdateSectionRequest {
            guild_id: guild.id,
            section_id: 999,
            name: "x".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // Put channels into section "Uno", then delete it.
    let a = guilds(&app, &alice)
        .create_channel(CreateChannelRequest {
            guild_id: guild.id,
            name: "a".into(),
            section_id: Some(one.id),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    let b = guilds(&app, &alice)
        .create_channel(CreateChannelRequest {
            guild_id: guild.id,
            name: "b".into(),
            section_id: Some(one.id),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    let top = channel(&app, &alice, guild.id, "top").await.unwrap();
    let before = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap();
    assert_eq!(entry(&before, 0), vec![general, top.id]);

    let after = guilds(&app, &alice)
        .delete_section(DeleteSectionRequest {
            guild_id: guild.id,
            section_id: one.id,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .layout
        .unwrap();
    assert_eq!(after.version, before.version + 1);
    assert_eq!(entry(&after, 0), vec![general, top.id, a.id, b.id]);
    assert_eq!(after.sections.len(), 1);
    assert_eq!((after.sections[0].id, after.sections[0].position), (two.id, 0));
    let listing = channels(&app, &alice, guild.id).await.unwrap();
    for c in &listing.channels {
        assert_eq!(c.section_id, None);
    }
    let positions: Vec<(u64, i32)> = {
        let mut p: Vec<_> = listing.channels.iter().map(|c| (c.id, c.position)).collect();
        p.sort_by_key(|(_, pos)| *pos);
        p
    };
    assert_eq!(positions.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![general, top.id, a.id, b.id]);

    let err = guilds(&app, &alice)
        .delete_section(DeleteSectionRequest {
            guild_id: guild.id,
            section_id: one.id,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    app.cleanup().await;
}

#[tokio::test]
async fn update_layout_reorders_and_moves_channels_across_sections() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let s1 = section(&app, &alice, guild.id, "s1").await.unwrap();
    let s2 = section(&app, &alice, guild.id, "s2").await.unwrap();
    let c1 = channel(&app, &alice, guild.id, "c1").await.unwrap();
    let c2 = channel(&app, &alice, guild.id, "c2").await.unwrap();
    let c3 = channel(&app, &alice, guild.id, "c3").await.unwrap();
    let v = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap().version;

    let layout = update_layout(
        &app,
        &alice,
        guild.id,
        v,
        &[s2.id, s1.id],
        vec![
            layout_entry(0, &[c3.id]),
            layout_entry(s1.id, &[c2.id, general]),
            layout_entry(s2.id, &[c1.id]),
        ],
    )
    .await
    .expect("update layout");
    assert_eq!(layout.version, v + 1);
    assert_eq!(layout.sections.iter().map(|s| (s.id, s.position)).collect::<Vec<_>>(), vec![(s2.id, 0), (s1.id, 1)]);
    assert_eq!(entry(&layout, 0), vec![c3.id]);
    assert_eq!(entry(&layout, s1.id), vec![c2.id, general]);
    assert_eq!(entry(&layout, s2.id), vec![c1.id]);

    // The persisted state matches, and channels mirror it.
    let listing = channels(&app, &alice, guild.id).await.unwrap();
    assert_eq!(listing.layout.unwrap(), layout);
    let find = |id: u64| listing.channels.iter().find(|c| c.id == id).unwrap();
    assert_eq!((find(c3.id).section_id, find(c3.id).position), (None, 0));
    assert_eq!((find(c2.id).section_id, find(c2.id).position), (Some(s1.id), 0));
    assert_eq!((find(general).section_id, find(general).position), (Some(s1.id), 1));
    assert_eq!((find(c1.id).section_id, find(c1.id).position), (Some(s2.id), 0));

    // A missing entry means "no channels": empty a section by moving everything out.
    let layout = update_layout(
        &app,
        &alice,
        guild.id,
        layout.version,
        &[s1.id, s2.id],
        vec![layout_entry(0, &[general, c1.id, c2.id, c3.id])],
    )
    .await
    .expect("move everything to the top level");
    assert_eq!(entry(&layout, 0), vec![general, c1.id, c2.id, c3.id]);
    assert!(entry(&layout, s1.id).is_empty() && entry(&layout, s2.id).is_empty());
    assert_eq!(layout.entries.len(), 3, "every section has an entry");
    app.cleanup().await;
}

#[tokio::test]
async fn update_layout_rejects_non_permutations_and_stale_versions() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let s1 = section(&app, &alice, guild.id, "s1").await.unwrap();
    let c1 = channel(&app, &alice, guild.id, "c1").await.unwrap();
    let v = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap().version;
    let invalid = ErrorCode::InvalidArgument;
    let try_layout = |sections: Vec<u64>, entries: Vec<LayoutEntry>| {
        let (app, alice) = (&app, &alice);
        async move { update_layout(app, alice, guild.id, v, &sections, entries).await }
    };

    let cases: Vec<(&str, Vec<u64>, Vec<LayoutEntry>)> = vec![
        ("missing channel", vec![s1.id], vec![layout_entry(0, &[general])]),
        ("duplicate channel", vec![s1.id], vec![layout_entry(0, &[general, c1.id]), layout_entry(s1.id, &[c1.id])]),
        ("duplicate in one entry", vec![s1.id], vec![layout_entry(0, &[general, c1.id, c1.id])]),
        ("unknown channel", vec![s1.id], vec![layout_entry(0, &[general, c1.id, 999])]),
        ("unknown section entry", vec![s1.id], vec![layout_entry(0, &[general, c1.id]), layout_entry(777, &[])]),
        ("duplicate entry", vec![s1.id], vec![layout_entry(0, &[general]), layout_entry(0, &[c1.id])]),
        ("missing section", vec![], vec![layout_entry(0, &[general, c1.id])]),
        ("unknown section", vec![s1.id, 777], vec![layout_entry(0, &[general, c1.id])]),
        ("duplicate section", vec![s1.id, s1.id], vec![layout_entry(0, &[general, c1.id])]),
        ("section 0 in section_ids", vec![0], vec![layout_entry(0, &[general, c1.id])]),
    ];
    for (what, sections, entries) in cases {
        let err = try_layout(sections, entries).await.unwrap_err();
        assert_eq!(err.code, invalid, "{what}");
    }
    // Nothing was applied by any of the rejected calls.
    let layout = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap();
    assert_eq!(layout.version, v);
    assert_eq!(entry(&layout, 0), vec![general, c1.id]);

    // A stale version is ABORTED and applies nothing; a retry from the fresh one works.
    let swap = vec![layout_entry(0, &[c1.id, general]), layout_entry(s1.id, &[])];
    update_layout(&app, &alice, guild.id, v, &[s1.id], swap.clone()).await.expect("first");
    let err = update_layout(&app, &alice, guild.id, v, &[s1.id], vec![layout_entry(0, &[general, c1.id])])
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Aborted);
    let layout = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap();
    assert_eq!((layout.version, entry(&layout, 0)), (v + 1, vec![c1.id, general]));
    // Structural changes also invalidate stale clients.
    section(&app, &alice, guild.id, "s2").await.unwrap();
    let err = update_layout(&app, &alice, guild.id, v + 1, &[s1.id], swap).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Aborted);
    // A wrong future version is rejected too.
    let err = update_layout(&app, &alice, guild.id, v + 50, &[s1.id], vec![layout_entry(0, &[c1.id, general])])
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Aborted);
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_layout_updates_have_exactly_one_winner() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let c1 = channel(&app, &alice, guild.id, "c1").await.unwrap();
    let v = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap().version;
    let attempts = (0..6).map(|i| {
        let order = if i % 2 == 0 { [general, c1.id] } else { [c1.id, general] };
        update_layout(&app, &alice, guild.id, v, &[], vec![layout_entry(0, &order)])
    });
    let results = futures::future::join_all(attempts).await;
    let wins = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(wins, 1, "{results:?}");
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|e| e.code == ErrorCode::Aborted));
    assert_eq!(channels(&app, &alice, guild.id).await.unwrap().layout.unwrap().version, v + 1);
    app.cleanup().await;
}

#[tokio::test]
async fn update_channel_validates_and_applies_partial_changes() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    let update = |req: UpdateChannelRequest| {
        let client = guilds(&app, &alice);
        let req = UpdateChannelRequest {
            guild_id: guild.id,
            channel_id: if req.channel_id == 0 { general } else { req.channel_id },
            ..req
        };
        async move { client.update_channel(req).await.map(|r| r.into_owned().channel.unwrap()) }
    };
    let invalid = ErrorCode::InvalidArgument;

    for bad in ["Game", "-game", "game-", "ga--me", "ga me", "ga_me", &"a".repeat(49)] {
        let err = update(UpdateChannelRequest { icon: Some(bad.into()), ..Default::default() }).await.unwrap_err();
        assert_eq!(err.code, invalid, "icon {bad}");
    }
    for bad in ["Red", "#ff0000", "grey"] {
        let err = update(UpdateChannelRequest { color: Some(bad.into()), ..Default::default() }).await.unwrap_err();
        assert_eq!(err.code, invalid, "color {bad}");
    }
    let err = update(UpdateChannelRequest { topic: Some("t".repeat(257)), ..Default::default() }).await.unwrap_err();
    assert_eq!(err.code, invalid);
    let err = update(UpdateChannelRequest { name: Some("bad name!".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!(err.code, invalid);
    let err = update(UpdateChannelRequest { channel_id: 999, topic: Some("x".into()), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // A bad field rejects the whole request: nothing applied.
    let err = update(UpdateChannelRequest {
        name: Some("renamed".into()),
        color: Some("nope".into()),
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert_eq!(err.code, invalid);
    assert_eq!(channels(&app, &alice, guild.id).await.unwrap().channels[0].name, "general");

    let updated = update(UpdateChannelRequest {
        name: Some("  Lobby Chat ".into()),
        icon: Some("game-controller".into()),
        color: Some("teal".into()),
        topic: Some("t".repeat(256)),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        (updated.name.as_str(), updated.icon.as_str(), updated.color.as_str(), updated.topic.len()),
        ("lobby-chat", "game-controller", "teal", 256)
    );
    // Unset fields stay; "" clears.
    let updated = update(UpdateChannelRequest { topic: Some("hello".into()), ..Default::default() }).await.unwrap();
    assert_eq!((updated.name.as_str(), updated.icon.as_str(), updated.topic.as_str()), ("lobby-chat", "game-controller", "hello"));
    let updated = update(UpdateChannelRequest {
        icon: Some(String::new()),
        color: Some(String::new()),
        topic: Some(String::new()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!((updated.icon.as_str(), updated.color.as_str(), updated.topic.as_str()), ("", "", ""));
    let listing = channels(&app, &alice, guild.id).await.unwrap();
    assert_eq!(listing.channels[0], updated);
    // Look edits are not structural.
    assert_eq!(listing.layout.unwrap().version, 0);
    app.cleanup().await;
}

#[tokio::test]
async fn update_channel_in_another_guild_is_not_found() {
    let app = TestApp::start().await;
    let (alice, bob) = (user(&app, "alice").await, user(&app, "bob").await);
    let g1 = guild(&app, &alice).await;
    let g2 = guild(&app, &bob).await;
    let g1_general = channels(&app, &alice, g1.id).await.unwrap().channels[0].id;
    let err = guilds(&app, &bob)
        .update_channel(UpdateChannelRequest {
            guild_id: g2.id,
            channel_id: g1_general,
            topic: Some("hijack".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(channels(&app, &alice, g1.id).await.unwrap().channels[0].topic, "");
    app.cleanup().await;
}

#[tokio::test]
async fn update_guild_renames_and_validates() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let update = |name: Option<&str>| {
        let client = guilds(&app, &alice);
        let req = UpdateGuildRequest {
            guild_id: guild.id,
            name: name.map(str::to_owned),
            ..Default::default()
        };
        async move { client.update_guild(req).await.map(|r| r.into_owned().guild.unwrap()) }
    };
    let renamed = update(Some("  New Name ")).await.unwrap();
    assert_eq!((renamed.name.as_str(), renamed.id, renamed.invite_code.as_str()), ("New Name", guild.id, guild.invite_code.as_str()));
    let mine = guilds(&app, &alice).list_my_guilds(ListMyGuildsRequest::default()).await.unwrap().into_owned().guilds;
    assert_eq!(mine[0].name, "New Name");
    assert_eq!(update(Some("   ")).await.unwrap_err().code, ErrorCode::InvalidArgument);
    assert_eq!(update(Some(&"n".repeat(101))).await.unwrap_err().code, ErrorCode::InvalidArgument);
    // Unset name is a no-op.
    assert_eq!(update(None).await.unwrap().name, "New Name");
    app.cleanup().await;
}

#[tokio::test]
async fn legacy_channels_without_layout_columns_get_a_valid_layout() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;

    // Rows as written before migration 0004: only the original columns.
    let mut legacy = Vec::new();
    for name in ["old-a", "old-b", "old-c"] {
        let id = app.state.snowflake.next_id();
        app.state
            .session
            .execute_unpaged(
                "INSERT INTO channels (guild_id, id, name, created_at) VALUES (?, ?, ?, ?)",
                (guild.id as i64, id as i64, name, Utc::now()),
            )
            .await
            .unwrap();
        app.state
            .session
            .execute_unpaged("INSERT INTO channels_by_id (id, guild_id) VALUES (?, ?)", (id as i64, guild.id as i64))
            .await
            .unwrap();
        legacy.push(id);
    }
    let listing = channels(&app, &alice, guild.id).await.unwrap();
    let layout = listing.layout.unwrap();
    let mut expected = vec![general];
    expected.extend(&legacy);
    // `general` has position 0 like the legacy rows, so creation order (id) decides.
    assert_eq!(entry(&layout, 0), expected);
    assert_eq!(layout.entries.len(), 1);
    for (i, id) in expected.iter().enumerate() {
        let c = listing.channels.iter().find(|c| c.id == *id).unwrap();
        assert_eq!((c.section_id, c.position, c.topic.as_str()), (None, i as i32, ""));
    }
    // New channels append after them, and the layout is editable.
    let fresh = channel(&app, &alice, guild.id, "fresh").await.unwrap();
    assert_eq!(fresh.position, 4);
    let layout = channels(&app, &alice, guild.id).await.unwrap().layout.unwrap();
    let reversed: Vec<u64> = entry(&layout, 0).into_iter().rev().collect();
    let layout = update_layout(&app, &alice, guild.id, layout.version, &[], vec![layout_entry(0, &reversed)])
        .await
        .unwrap();
    assert_eq!(entry(&layout, 0), reversed);
    // Look edits work on legacy rows too.
    let updated = guilds(&app, &alice)
        .update_channel(UpdateChannelRequest {
            guild_id: guild.id,
            channel_id: legacy[0],
            topic: Some("legacy".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned()
        .channel
        .unwrap();
    assert_eq!(updated.topic, "legacy");
    app.cleanup().await;
}

#[tokio::test]
async fn section_count_is_capped() {
    let app = TestApp::start().await;
    let alice = user(&app, "alice").await;
    let guild = guild(&app, &alice).await;
    for i in 0..MAX_SECTIONS_PER_GUILD {
        section(&app, &alice, guild.id, &format!("s{i}")).await.expect("under the cap");
    }
    assert_eq!(section(&app, &alice, guild.id, "extra").await.unwrap_err().code, ErrorCode::ResourceExhausted);
    app.cleanup().await;
}

#[tokio::test]
async fn layout_edits_need_membership_and_manage_channels() {
    let app = TestApp::start().await;
    let (alice, bob, dave) = (user(&app, "alice").await, user(&app, "bob").await, user(&app, "dave").await);
    let guild = guild(&app, &alice).await;
    join(&app, &bob, &guild).await;
    let sec = section(&app, &alice, guild.id, "s").await.unwrap();
    let general = channels(&app, &alice, guild.id).await.unwrap().channels[0].id;
    for (who, expected) in [(&bob, ErrorCode::PermissionDenied), (&dave, ErrorCode::NotFound)] {
        let g = guilds(&app, who);
        assert_eq!(
            g.update_section(UpdateSectionRequest { guild_id: guild.id, section_id: sec.id, name: "x".into(), ..Default::default() })
                .await.unwrap_err().code,
            expected
        );
        assert_eq!(
            g.delete_section(DeleteSectionRequest { guild_id: guild.id, section_id: sec.id, ..Default::default() })
                .await.unwrap_err().code,
            expected
        );
        assert_eq!(
            update_layout(&app, who, guild.id, 1, &[sec.id], vec![layout_entry(0, &[general])]).await.unwrap_err().code,
            expected
        );
        assert_eq!(
            g.update_channel(UpdateChannelRequest { guild_id: guild.id, channel_id: general, topic: Some("x".into()), ..Default::default() })
                .await.unwrap_err().code,
            expected
        );
    }
    // Members can still read the layout.
    assert!(channels(&app, &bob, guild.id).await.unwrap().layout.is_set());
    assert_eq!(channels(&app, &dave, guild.id).await.unwrap_err().code, ErrorCode::NotFound);
    app.cleanup().await;
}
