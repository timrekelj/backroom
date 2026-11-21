//! Proofs of the site-admin dashboard against the fake backend: who may open it, the overview's
//! cards and storage bar, the activity ranges, paging and filters, the delete flow (with its
//! rollback), the admin toggle and its two refusals, and the "not available" page. Driven through
//! the real view with pointer and key input, like the other runtime tests.

use rune::{Key, KeyEvent, Mods, Runtime};
use thehideout_api::AdminFileKind;

use crate::admin::{AdminPage, Confirm, LAST_ADMIN, PAGE_SIZE, SELF_DEMOTE, storage_parts};
use crate::app::App;
use crate::backend::Command;
use crate::fake::{ANA, CVETKA, FakeBackend};
use crate::scenes::booted_as;
use crate::tests::{click, has, key, rect_of, runtime, settle, signed_in};

/// A signed-in admin (cvetka) on the fake world, with replies `latency` seconds late.
fn admin_in(latency: f64) -> (Runtime<App>, FakeBackend) {
    let (app, fake) = booted_as(true, "cvetka");
    fake.set_latency(latency);
    (runtime(app), fake)
}

fn open_admin(rt: &mut Runtime<App>) {
    rt.update(|a, _| a.act(|s| s.open_admin()));
    settle(rt, 12);
}

fn go_page(rt: &mut Runtime<App>, page: AdminPage) {
    rt.update(|a, _| a.act(|s| s.admin_page(page)));
    settle(rt, 12);
}

fn cmd_shift_a(rt: &mut Runtime<App>) {
    rt.key(KeyEvent {
        key: Key::Command('a'),
        mods: Mods {
            shift: true,
            ..Mods::default()
        },
    });
}

// ---- who may open it ------------------------------------------------------------------------

#[test]
fn a_non_admin_has_no_admin_menu_item_and_the_shortcut_does_nothing() {
    let (mut rt, _) = signed_in(0.0); // tim, a plain member
    click(&mut rt, "user-menu-button");
    settle(&mut rt, 8);
    assert!(
        !rt.elements()
            .into_iter()
            .any(|e| e.label.as_deref().is_some_and(|l| l.contains("Admin"))),
        "no Admin item in the account menu for a non-admin"
    );
    cmd_shift_a(&mut rt);
    settle(&mut rt, 6);
    assert!(rt.state.state.ui.admin.is_none(), "the shortcut is ignored");
    assert!(!has(&rt, "admin"));
}

#[test]
fn an_admin_opens_the_dashboard_from_the_menu_and_the_shortcut_and_esc_leaves_it() {
    let (mut rt, _) = admin_in(0.0);
    click(&mut rt, "user-menu-button");
    settle(&mut rt, 8);
    assert!(
        rt.elements()
            .into_iter()
            .any(|e| e.label.as_deref().is_some_and(|l| l.contains("Admin"))),
        "the account menu offers Admin"
    );
    click(&mut rt, "user-menu-item2"); // Profile, Settings, Admin, Security, Sign out
    settle(&mut rt, 12);
    let s = &rt.state.state;
    assert!(s.ui.admin.is_some(), "the menu item opens the dashboard");
    assert_eq!(s.admin().unwrap().page, AdminPage::Overview);
    assert!(
        has(&rt, "admin") && !has(&rt, "shell"),
        "the dashboard replaces the shell"
    );
    assert!(has(&rt, "page-admin-overview"));
    assert!(
        s.admin().unwrap().overview.is_some(),
        "the overview data is in"
    );

    key(&mut rt, Key::Escape);
    settle(&mut rt, 6);
    assert!(
        rt.state.state.ui.admin.is_none(),
        "Esc leaves the dashboard"
    );
    assert!(has(&rt, "shell"));

    cmd_shift_a(&mut rt);
    settle(&mut rt, 12);
    assert!(
        rt.state.state.ui.admin.is_some(),
        "Cmd/Ctrl+Shift+A opens it"
    );
    assert!(has(&rt, "admin"));
}

// ---- the overview ----------------------------------------------------------------------------

#[test]
fn the_overview_shows_every_stat_card_and_a_storage_bar_matching_the_data() {
    let (mut rt, _) = admin_in(0.0);
    open_admin(&mut rt);
    settle(&mut rt, 6);
    for key in [
        "stat-users",
        "stat-servers",
        "stat-channels",
        "stat-messages",
        "stat-live",
        "stat-messages-today",
        "stat-signups-today",
        "stat-active-today",
    ] {
        assert!(has(&rt, key), "missing {key}");
    }
    assert!(has(&rt, "storage-card") && has(&rt, "activity-card"));
    let o = rt
        .state
        .state
        .admin()
        .unwrap()
        .overview
        .clone()
        .expect("the overview loaded");
    assert!(o.storage.total() > 0);

    let total = o.storage.total();
    let parts = storage_parts(&o.storage);
    let widths: Vec<(String, f32)> = parts
        .iter()
        .filter(|p| p.2 > 0)
        .map(|(key, _, _)| {
            (
                key.to_string(),
                rect_of(&rt, &format!("storage-bar-{key}"))[2],
            )
        })
        .collect();
    let drawn_total: f32 = widths.iter().map(|w| w.1).sum();
    for (key, label, n) in parts {
        assert!(has(&rt, &format!("legend-{key}")), "no legend for {label}");
        if n == 0 {
            continue;
        }
        let w = widths.iter().find(|(k, _)| k == key).unwrap().1;
        let share = w / drawn_total;
        let expected = n as f32 / total as f32;
        assert!(
            (share - expected).abs() < 0.02,
            "{key} takes {share:.3} of the bar, expected {expected:.3} ({n} of {total} bytes)"
        );
    }
    // The total the card shows is the sum of the parts.
    assert_eq!(o.storage_bytes_total, total);
}

#[test]
fn switching_the_activity_range_changes_the_point_count() {
    let (mut rt, _) = admin_in(0.0);
    open_admin(&mut rt);
    settle(&mut rt, 6);
    assert_eq!(rt.state.state.admin().unwrap().range, 30);
    assert_eq!(rt.state.state.chart_days().len(), 30);

    click(&mut rt, "admin-range/segment0");
    settle(&mut rt, 12);
    assert_eq!(rt.state.state.admin().unwrap().range, 7);
    assert_eq!(rt.state.state.chart_days().len(), 7, "7 days of points");

    click(&mut rt, "admin-range/segment2");
    settle(&mut rt, 12);
    assert_eq!(rt.state.state.admin().unwrap().range, 90);
    assert_eq!(rt.state.state.chart_days().len(), 90, "90 days of points");
    let drawn = rt
        .elements()
        .into_iter()
        .filter(|e| {
            e.key
                .as_deref()
                .is_some_and(|k| k.starts_with("activity-day-"))
        })
        .count();
    assert_eq!(drawn, 90, "every day has its column");
}

#[test]
fn the_activity_plot_fills_its_area_and_stays_inside_the_card() {
    let (mut rt, _) = admin_in(0.0);
    open_admin(&mut rt);
    settle(&mut rt, 6);
    let inside = |inner: [f32; 4], outer: [f32; 4]| {
        inner[0] >= outer[0] - 0.5
            && inner[1] >= outer[1] - 0.5
            && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
            && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
    };
    let card = rect_of(&rt, "activity-card");
    let area = rect_of(&rt, "activity-area");
    let plot = rect_of(&rt, "activity-plot");
    assert!(
        inside(area, card),
        "plot area {area:?} leaves the card {card:?}"
    );
    assert!(inside(plot, area), "plot {plot:?} leaves its area {area:?}");
    assert!(
        plot[2] > area[2] - 1.0 && plot[3] > area[3] - 1.0,
        "plot {plot:?} fills {area:?}"
    );
    for i in [0, 29] {
        let day = rect_of(&rt, &format!("activity-day-{i}"));
        assert!(inside(day, area), "day {i} {day:?} leaves {area:?}");
    }
}

// ---- lists: paging, search filters -----------------------------------------------------------

#[test]
fn the_servers_page_asks_for_its_next_page_until_every_server_is_in() {
    let (mut rt, _) = admin_in(0.0);
    open_admin(&mut rt);
    rt.update(|a, _| {
        a.state.ui.admin.as_mut().unwrap().page_size = 4;
        a.act(|s| s.admin_page(AdminPage::Servers));
    });
    settle(&mut rt, 12);
    for _ in 0..5 {
        if rt.state.state.admin().unwrap().servers.next.is_none() {
            break;
        }
        rt.update(|a, _| {
            a.act(|s| {
                s.admin_more();
            })
        });
        settle(&mut rt, 8);
    }
    let sent = rt.state.backend.sent();
    assert!(
        sent.iter().any(|c| matches!(
            c,
            Command::AdminGuilds {
                page_token: Some(_),
                ..
            }
        )),
        "a second page was asked for with a token: {sent:?}"
    );
    let ad = rt.state.state.admin().unwrap();
    assert_eq!(
        ad.servers.items.len(),
        12,
        "all three pages of four chained in"
    );
    assert!(ad.servers.next.is_none());
    assert!(has(&rt, "admin-server-rows"));
}

#[test]
fn the_audit_log_paginates() {
    let (mut rt, _) = admin_in(0.0);
    open_admin(&mut rt);
    go_page(&mut rt, AdminPage::Audit);
    let ad = rt.state.state.admin().unwrap();
    assert_eq!(ad.audit.items.len(), PAGE_SIZE as usize, "one page first");
    assert!(ad.audit.next.is_some());
    rt.update(|a, _| {
        a.act(|s| {
            s.admin_more();
        })
    });
    settle(&mut rt, 12);
    let ad = rt.state.state.admin().unwrap();
    assert_eq!(ad.audit.items.len(), 64, "the second page chained in");
    assert!(ad.audit.next.is_none(), "the log is exhausted");
    assert!(has(&rt, "admin-audit-rows"));
}

#[test]
fn the_files_page_filters_by_server_and_by_kind() {
    let (mut rt, _) = admin_in(0.0);
    open_admin(&mut rt);
    go_page(&mut rt, AdminPage::Files);
    let ad = rt.state.state.admin().unwrap();
    assert_eq!(ad.files.items.len(), PAGE_SIZE as usize);
    let guild = ad
        .files
        .items
        .iter()
        .find_map(|r| r.guild.as_ref().map(|g| g.id))
        .expect("some file has a server");

    rt.update(|a, _| a.act(|s| s.filter_files_guild(Some(guild))));
    settle(&mut rt, 12);
    let ad = rt.state.state.admin().unwrap();
    assert!(!ad.files.items.is_empty());
    assert!(
        ad.files
            .items
            .iter()
            .all(|r| r.guild.as_ref().is_some_and(|g| g.id == guild)),
        "only that server's files are listed"
    );
    assert!(
        rt.state
            .backend
            .sent()
            .iter()
            .any(|c| matches!(c, Command::AdminFiles { guild: Some(g), .. } if *g == guild)),
        "the filter went out with the request"
    );

    rt.update(|a, _| a.act(|s| s.filter_files_guild(None)));
    settle(&mut rt, 6);
    rt.update(|a, _| a.act(|s| s.filter_files_kind(Some(AdminFileKind::Emoji))));
    settle(&mut rt, 12);
    let ad = rt.state.state.admin().unwrap();
    assert!(!ad.files.items.is_empty());
    assert!(
        ad.files
            .items
            .iter()
            .all(|r| r.kind == AdminFileKind::Emoji),
        "only emojis are listed"
    );
    assert!(
        rt.state.backend.sent().iter().any(|c| matches!(c, Command::AdminFiles { kind: Some(k), guild: None, .. } if *k == AdminFileKind::Emoji)),
        "the kind filter went out with the request"
    );
}

// ---- deleting files --------------------------------------------------------------------------

#[test]
fn deleting_a_file_sends_the_reason_removes_the_row_and_a_refusal_rolls_it_back() {
    // A little latency, so the optimistic state (row gone, deletion out) is observable before
    // the reply lands.
    let (mut rt, fake) = admin_in(0.5);
    open_admin(&mut rt);
    go_page(&mut rt, AdminPage::Files);
    settle(&mut rt, 60); // let the first page (0.5 s latency) land
    let key = rt.state.state.admin().unwrap().files.items[0].key.clone();
    let count = fake.admin_file_count();

    rt.update(|a, _| a.act(|s| s.ask_delete_files(vec![key.clone()])));
    settle(&mut rt, 2);
    assert!(has(&rt, "admin-dlg-dialog"), "the confirmation opens");
    rt.update(|a, _| {
        if let Some(Confirm::DeleteFiles(d)) =
            a.state.ui.admin.as_mut().and_then(|ad| ad.confirm.as_mut())
        {
            d.reason.replace_text("spam");
        }
    });
    click(&mut rt, "admin-submit");
    settle(&mut rt, 2);
    assert!(
        rt.state.backend.sent().iter().any(|c| matches!(c, Command::AdminDeleteFile { key: k, reason } if *k == key && reason == "spam")),
        "the deletion went out with the reason"
    );
    let ad = rt.state.state.admin().unwrap();
    assert!(
        ad.files.items.iter().all(|r| r.key != key),
        "the row left the list at once"
    );
    assert!(ad.removed.contains_key(&key));

    settle(&mut rt, 80);
    assert!(!fake.admin_has_file(&key), "the server dropped it");
    assert_eq!(fake.admin_file_count(), count - 1);
    assert!(
        rt.state
            .state
            .admin()
            .unwrap()
            .files
            .items
            .iter()
            .all(|r| r.key != key),
        "it stays deleted"
    );
    assert!(
        fake.admin_audit()
            .iter()
            .any(|(a, t, r)| a == "file.delete" && t == &key && r == "spam"),
        "the audit log recorded the reason"
    );

    // A refused deletion puts the row back.
    fake.admin_fail_deletes(1);
    let key2 = rt.state.state.admin().unwrap().files.items[0].key.clone();
    rt.update(|a, _| a.act(|s| s.ask_delete_files(vec![key2.clone()])));
    settle(&mut rt, 2);
    click(&mut rt, "admin-submit");
    settle(&mut rt, 80);
    let ad = rt.state.state.admin().unwrap();
    assert!(
        ad.files.items.iter().any(|r| r.key == key2),
        "the refused row is back in the list"
    );
    assert!(fake.admin_has_file(&key2), "and still on the server");
    assert!(
        rt.state
            .state
            .ui
            .toasts
            .iter()
            .any(|t| t.message.contains("Couldn't delete")),
        "a toast says it failed: {:?}",
        rt.state
            .state
            .ui
            .toasts
            .iter()
            .map(|t| t.message.clone())
            .collect::<Vec<_>>()
    );
}

// ---- admin rights ----------------------------------------------------------------------------

#[test]
fn set_admin_grants_and_both_preconditions_show_inline_on_the_row() {
    let (mut rt, fake) = admin_in(0.0);
    open_admin(&mut rt);
    rt.update(|a, _| {
        a.state.ui.admin.as_mut().unwrap().page_size = 100;
        a.act(|s| s.admin_page(AdminPage::Users));
    });
    settle(&mut rt, 12);
    assert_eq!(
        rt.state.state.admin().unwrap().users.items.len(),
        44,
        "every user in one page"
    );

    // Grant ana the rights: the switch's confirmation, then the command.
    rt.update(|a, _| a.act(|s| s.ask_set_admin(ANA, true)));
    settle(&mut rt, 2);
    assert!(has(&rt, "admin-dlg-dialog"));
    click(&mut rt, "admin-submit");
    settle(&mut rt, 12);
    assert!(fake.admin_is_admin(ANA), "the server granted it");
    assert!(
        rt.state
            .state
            .admin()
            .unwrap()
            .users
            .items
            .iter()
            .find(|r| r.user.id == ANA)
            .unwrap()
            .user
            .is_admin,
        "the row's switch flipped"
    );

    // Demoting yourself is refused: the message sits under your own row.
    rt.update(|a, _| a.act(|s| s.ask_set_admin(CVETKA, false)));
    settle(&mut rt, 2);
    click(&mut rt, "admin-submit");
    settle(&mut rt, 12);
    let ad = rt.state.state.admin().unwrap();
    assert_eq!(
        ad.user_errors.get(&CVETKA).map(String::as_str),
        Some(SELF_DEMOTE)
    );
    assert!(
        has(&rt, &format!("user-error-{CVETKA}")),
        "the inline message is drawn"
    );

    // The last admin can't be demoted (the server refuses it as a precondition).
    fake.admin_refuse_next_set();
    let marta = rt
        .state
        .state
        .admin()
        .unwrap()
        .users
        .items
        .iter()
        .find(|r| r.user.username == "marta")
        .unwrap()
        .user
        .id;
    rt.update(|a, _| a.act(|s| s.ask_set_admin(marta, false)));
    settle(&mut rt, 2);
    click(&mut rt, "admin-submit");
    settle(&mut rt, 12);
    let ad = rt.state.state.admin().unwrap();
    assert_eq!(
        ad.user_errors.get(&marta).map(String::as_str),
        Some(LAST_ADMIN)
    );
    assert!(fake.admin_is_admin(marta), "the refusal changed nothing");
}

// ---- a server without the service ------------------------------------------------------------

#[test]
fn an_unimplemented_admin_service_shows_the_unavailable_page() {
    let (mut rt, fake) = admin_in(0.0);
    fake.admin_unavailable(true);
    open_admin(&mut rt);
    let ad = rt.state.state.admin().unwrap();
    assert!(
        ad.unavailable,
        "the Unimplemented reply marks the service unavailable"
    );
    assert!(has(&rt, "admin-unavailable"));
    assert!(
        rt.elements()
            .into_iter()
            .any(|e| e.text.as_deref() == Some("Admin service not available")),
        "the friendly page is drawn"
    );
    // "Try again" asks again (and would still be unavailable here).
    click(&mut rt, "admin-retry");
    settle(&mut rt, 12);
    assert!(rt.state.state.admin().unwrap().unavailable);
    assert!(has(&rt, "admin-unavailable"));
}
