//! Proofs of server settings, roles and pictures against the fake backend: role CRUD and reorder
//! (with its conflict), the hierarchy rules that grey controls out, handing roles to members,
//! name colours and member-panel groups, and the avatar and icon flows. Mostly state-level (the
//! reducers and commands); a few drive the real view with pointer input, a file dialog and a drop.

use std::path::PathBuf;

use rune::Runtime;
use thehideout_api::{ApiError, Event, GatewayUpdate, PaletteColor, Permissions};
use thehideout_design::components::ToastKind;

use crate::app::App;
use crate::backend::{Backend, Command, ImagePurpose, Reply};
use crate::fake::{
    FakeBackend, ANA, BAND_DRUMMER, BAND_GUEST, BAND_ROADIE, BRUNO, CVETKA, HIDEOUT_ADMINS,
    HIDEOUT_DESIGNERS, HIDEOUT_MEMBERS, HIDEOUT_MODS, TIM,
};
use crate::forms::{DialogKind, Phase};
use crate::scenes::{booted, booted_as};
use crate::server_settings::{ServerPage, ROLES_CONFLICT_TOAST};
use crate::tests::{click, has, runtime, settle};

const G: u64 = 1;
const BAND: u64 = 2;
/// The band's Fan role (nothing special about it but its place at the bottom).
const BAND_FAN: u64 = crate::fake::BAND_FAN;

/// Tim, owner of The Hideout, with its settings page open on `page`.
fn owner(page: ServerPage) -> (App, FakeBackend) {
    let (mut app, fake) = booted(true);
    app.act(|s| {
        s.open_server_settings(G);
        s.server_page(page);
    });
    app.pump();
    (app, fake)
}

/// Tim, a Roadie (MANAGE_ROLES and MANAGE_EMOJIS, position 2 of 4) in Garage Band.
fn roadie() -> (App, FakeBackend) {
    let (mut app, fake) = booted(true);
    app.act(|s| s.go_guild(BAND));
    app.pump();
    app.act(|s| s.open_server_settings(BAND));
    app.pump();
    (app, fake)
}

fn names(app: &App, guild: u64) -> Vec<String> {
    app.state
        .roles_of(guild)
        .iter()
        .map(|r| r.name.clone())
        .collect()
}

fn list(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

fn tmp_png(name: &str, seed: u64) -> (PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("thehideout-roles-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = crate::media::generated_png(seed, 32);
    let path = dir.join(name);
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

// ---- who sees the page -----------------------------------------------------------------------

#[test]
fn server_settings_are_offered_to_those_who_may_manage_the_server_or_its_roles() {
    let (mut app, _) = booted(true);
    // Roles arrive when a server is first opened.
    app.act(|s| {
        s.go_guild(BAND);
        s.go_guild(3);
    });
    app.pump();
    assert!(app.state.can_open_server_settings(G), "the owner always");
    assert!(
        app.state.can_open_server_settings(BAND),
        "a Roadie may manage roles"
    );
    assert!(
        !app.state.can_open_server_settings(3),
        "a channel moderator may not"
    );
    let (mut app, _) = booted_as(true, "ana");
    assert!(
        !app.state.can_open_server_settings(G),
        "a Designer with no permissions may not"
    );
    app.act(|s| s.open_server_settings(G));
    assert!(
        app.state.ui.server_settings.is_none(),
        "refused, not opened"
    );
}

#[test]
fn the_roadies_page_starts_on_roles_because_overview_needs_manage_server() {
    let (mut app, _) = roadie();
    let ss = app.state.ui.server_settings.as_ref().unwrap();
    assert_eq!(ss.page, ServerPage::Roles);
    assert!(!app.state.settings_page_allowed(BAND, ServerPage::Overview));
    app.act(|s| s.server_page(ServerPage::Overview));
    assert_eq!(
        app.state.ui.server_settings.as_ref().unwrap().page,
        ServerPage::Roles,
        "can't be navigated to"
    );
    app.act(|s| s.server_page(ServerPage::Members));
    assert_eq!(
        app.state.ui.server_settings.as_ref().unwrap().page,
        ServerPage::Members
    );
}

// ---- role CRUD -------------------------------------------------------------------------------

#[test]
fn creating_a_role_with_a_colour_and_permissions_sends_it_and_selects_it() {
    let (mut app, fake) = owner(ServerPage::Roles);
    app.act(|s| {
        s.new_role();
        let f = s.role_form().unwrap();
        f.name.replace_text("Builders");
        s.set_role_color("orange");
        s.toggle_permission(Permissions::MANAGE_CHANNELS);
        s.save_role();
    });
    assert!(fake.sent().iter().any(|c| matches!(c, Command::CreateRole { guild: 1, name, color: PaletteColor::Orange, permissions } if name == "Builders" && *permissions == Permissions::MANAGE_CHANNELS)));
    assert!(
        app.state
            .ui
            .server_settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .busy
    );
    app.pump();
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Moderator", "Designer", "Regular", "Builders"]),
        "a new role goes to the bottom"
    );
    let f = app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .form
        .as_ref()
        .unwrap();
    assert!(
        f.role.is_some() && !f.busy && !f.dirty(),
        "the editor now shows the saved role"
    );
    assert_eq!(
        fake.roles(G).last().map(|r| (r.color, r.permissions)),
        Some((PaletteColor::Orange, Permissions::MANAGE_CHANNELS))
    );
    assert_eq!(
        app.state.role_version(G),
        5,
        "the new version came with the reply"
    );
    assert!(app
        .state
        .ui
        .toasts
        .iter()
        .any(|t| t.message == "Created Builders"));
}

#[test]
fn an_empty_or_over_long_role_name_sends_nothing() {
    let (mut app, fake) = owner(ServerPage::Roles);
    let before = fake.sent().len();
    app.act(|s| {
        s.new_role();
        s.role_form().unwrap().name.replace_text("   ");
        s.save_role();
    });
    assert!(app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .form
        .as_ref()
        .unwrap()
        .error
        .is_some());
    app.act(|s| {
        s.role_form().unwrap().name.replace_text("x".repeat(33));
        s.save_role();
    });
    assert_eq!(fake.sent().len(), before, "validation stays on the client");
}

#[test]
fn editing_a_role_sends_only_what_changed() {
    let (mut app, fake) = owner(ServerPage::Roles);
    app.act(|s| {
        s.select_role(HIDEOUT_MODS);
        // Nothing changed yet: Save does nothing.
        s.save_role();
    });
    assert!(!fake
        .sent()
        .iter()
        .any(|c| matches!(c, Command::UpdateRole { .. })));
    app.act(|s| {
        s.set_role_color("indigo");
        s.save_role();
    });
    assert!(fake.sent().iter().any(|c| matches!(c, Command::UpdateRole { role, name: None, color: Some(PaletteColor::Indigo), permissions: None, .. } if *role == HIDEOUT_MODS)));
    app.pump();
    assert_eq!(
        app.state.role(G, HIDEOUT_MODS).unwrap().color,
        PaletteColor::Indigo
    );
    assert_eq!(
        fake.roles(G)
            .iter()
            .find(|r| r.id == HIDEOUT_MODS)
            .unwrap()
            .color,
        PaletteColor::Indigo
    );
    assert!(
        !app.state
            .ui
            .server_settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .dirty(),
        "the saved role is the new baseline"
    );
    // A permission switch and a rename.
    app.act(|s| {
        s.role_form().unwrap().name.replace_text("Mods");
        s.toggle_permission(Permissions::MANAGE_EMOJIS);
        s.save_role();
    });
    assert!(fake.sent().iter().any(|c| matches!(c, Command::UpdateRole { name: Some(n), color: None, permissions: Some(p), .. } if n == "Mods" && p.bits() == (4 | 16))));
}

#[test]
fn a_refused_save_shows_the_servers_words_in_the_editor() {
    let (mut app, _fake) = owner(ServerPage::Roles);
    app.act(|s| {
        s.select_role(HIDEOUT_MODS);
        s.role_form().unwrap().name.replace_text("Mods");
        s.save_role();
    });
    // Whatever the client allowed, the server has the last word.
    let mut tx = rune::Tx::default();
    crate::apply::apply(
        &mut app.state,
        Reply::RoleUpdated {
            guild: G,
            result: Err(ApiError::PermissionDenied),
        },
        &mut tx,
    );
    let f = app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .form
        .as_ref()
        .unwrap();
    assert_eq!(
        f.error.as_deref(),
        Some("You don't have permission to do that.")
    );
    assert!(!f.busy, "the form is usable again");
    assert_eq!(f.name.text, "Mods", "what was typed stays");
}

#[test]
fn deleting_a_role_asks_first_then_removes_it_from_everyone() {
    let (mut app, fake) = owner(ServerPage::Roles);
    app.act(|s| {
        s.select_role(HIDEOUT_MODS);
        s.ask_delete_role(HIDEOUT_MODS);
    });
    assert!(matches!(
        app.state.ui.dialog.as_ref().map(|d| d.kind),
        Some(DialogKind::DeleteRole {
            guild: G,
            role: HIDEOUT_MODS
        })
    ));
    assert!(
        !fake
            .sent()
            .iter()
            .any(|c| matches!(c, Command::DeleteRole { .. })),
        "nothing happens until it is confirmed"
    );
    app.act(|s| s.submit_dialog());
    assert_eq!(app.state.ui.dialog.as_ref().unwrap().phase, Phase::Busy);
    app.pump();
    assert!(app.state.ui.dialog.is_none());
    assert_eq!(names(&app, G), list(&["Admin", "Designer", "Regular"]));
    assert!(
        app.state.member_role_ids(G, BRUNO).is_empty(),
        "Bruno was the Moderator"
    );
    assert!(
        app.state
            .ui
            .server_settings
            .as_ref()
            .unwrap()
            .form
            .is_none(),
        "its editor closed"
    );
    assert_eq!(
        fake.roles(G).iter().map(|r| r.position).collect::<Vec<_>>(),
        [2, 1, 0],
        "positions stay compact"
    );
    assert_eq!(
        app.state
            .roles_of(G)
            .iter()
            .map(|r| r.position)
            .collect::<Vec<_>>(),
        [2, 1, 0]
    );
    assert!(fake.member_roles(G, BRUNO).is_empty());
}

// ---- reordering ------------------------------------------------------------------------------

#[test]
fn moving_a_role_up_is_optimistic_and_the_reply_confirms_it() {
    let (mut app, fake) = owner(ServerPage::Roles);
    assert_eq!(app.state.role_version(G), 4);
    app.act(|s| {
        assert!(s.can_move_role(G, HIDEOUT_DESIGNERS, -1));
        assert!(s.step_role(G, HIDEOUT_DESIGNERS, -1));
    });
    // Already reordered locally, before any answer.
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Designer", "Moderator", "Regular"])
    );
    assert!(app.state.data.role_undo.contains_key(&G));
    assert!(fake.sent().iter().any(|c| matches!(c, Command::ReorderRoles { guild: 1, expected_version: 4, role_ids } if role_ids == &[HIDEOUT_ADMINS, HIDEOUT_DESIGNERS, HIDEOUT_MODS, HIDEOUT_MEMBERS])));
    // A second move waits for the first one's answer.
    app.act(|s| assert!(!s.step_role(G, HIDEOUT_MEMBERS, -1)));
    app.pump();
    assert!(app.state.data.role_undo.is_empty());
    assert_eq!(app.state.role_version(G), 5);
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Designer", "Moderator", "Regular"])
    );
    assert_eq!(
        fake.roles(G)
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>(),
        names(&app, G)
    );
    assert_eq!(
        app.state
            .roles_of(G)
            .iter()
            .map(|r| r.position)
            .collect::<Vec<_>>(),
        [3, 2, 1, 0]
    );
}

#[test]
fn dragging_a_row_lands_it_where_the_pointer_let_go() {
    let (mut app, fake) = owner(ServerPage::Roles);
    // Pull "Regular" (last) up two places: two rows of pitch, a little short of the third.
    app.act(|s| {
        s.drag_role(
            G,
            HIDEOUT_MEMBERS,
            -(crate::server_settings::ROLE_PITCH * 2.0) + 6.0,
        );
        assert_eq!(s.role_drop_slot(), Some(1));
        s.end_role_drag(false);
    });
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Regular", "Moderator", "Designer"])
    );
    app.pump();
    assert_eq!(
        fake.roles(G)
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>(),
        names(&app, G)
    );
    // A cancelled drag changes nothing.
    app.act(|s| {
        s.drag_role(G, HIDEOUT_MEMBERS, -200.0);
        s.end_role_drag(true);
    });
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Regular", "Moderator", "Designer"])
    );
}

#[test]
fn a_stale_reorder_refetches_and_says_so() {
    let (mut app, fake) = owner(ServerPage::Roles);
    fake.conflict_next_roles();
    app.act(|s| {
        s.step_role(G, HIDEOUT_DESIGNERS, -1);
    });
    assert_eq!(names(&app, G)[1], "Designer", "shown at once");
    app.pump();
    // Back to what the server has, which moved on; the toast says why.
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Moderator", "Designer", "Regular"])
    );
    assert_eq!(
        app.state.role_version(G),
        5,
        "the refetch brought the server's version"
    );
    assert!(app.state.data.role_undo.is_empty());
    let toast = app.state.ui.toasts.back().unwrap();
    assert_eq!(
        (toast.kind, toast.message.as_str()),
        (ToastKind::Danger, ROLES_CONFLICT_TOAST)
    );
    // And the next move works from the new version.
    app.act(|s| {
        assert!(s.step_role(G, HIDEOUT_DESIGNERS, -1));
    });
    app.pump();
    assert_eq!(
        names(&app, G),
        list(&["Admin", "Designer", "Moderator", "Regular"])
    );
}

#[test]
fn another_clients_roles_event_replaces_the_list_and_closes_a_deleted_roles_editor() {
    let (mut app, fake) = owner(ServerPage::Roles);
    app.act(|s| s.select_role(HIDEOUT_DESIGNERS));
    let mut roles = fake.roles(G);
    roles.retain(|r| r.id != HIDEOUT_DESIGNERS);
    roles
        .iter_mut()
        .for_each(|r| r.position -= i32::from(r.position > 1));
    fake.gateway(GatewayUpdate::Event(Event::RolesUpdated {
        guild_id: G,
        roles,
        version: 9,
    }));
    app.pump();
    assert_eq!(names(&app, G), list(&["Admin", "Moderator", "Regular"]));
    assert_eq!(app.state.role_version(G), 9);
    assert!(app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .form
        .is_none());
}

// ---- hierarchy -------------------------------------------------------------------------------

#[test]
fn a_roadie_may_only_touch_roles_below_the_roadie_role() {
    let (app, _) = roadie();
    let s = &app.state;
    assert!(s.can_manage_roles(BAND) && !s.is_owner(BAND));
    assert_eq!(s.top_position(BAND), Some(2));
    let can = |id: u64| s.can_edit_role(BAND, s.role(BAND, id).unwrap());
    assert!(!can(BAND_DRUMMER), "above");
    assert!(
        !can(BAND_ROADIE),
        "their own highest role is out of reach too"
    );
    assert!(can(BAND_GUEST) && can(BAND_FAN));
    // Permissions: only the ones they hold.
    assert!(
        s.can_grant(BAND, Permissions::MANAGE_ROLES)
            && s.can_grant(BAND, Permissions::MANAGE_EMOJIS)
    );
    assert!(
        !s.can_grant(BAND, Permissions::MANAGE_SERVER)
            && !s.can_grant(BAND, Permissions::ADMINISTRATOR)
    );
    // The owner has no such limits.
    let (app, _) = owner(ServerPage::Roles);
    assert!(app
        .state
        .roles_of(G)
        .iter()
        .all(|r| app.state.can_edit_role(G, r)));
    assert!(app.state.can_grant(G, Permissions::ADMINISTRATOR));
}

#[test]
fn moves_that_cross_the_roadies_own_role_are_refused_before_they_are_sent() {
    let (mut app, fake) = roadie();
    let s = &app.state;
    // Order: Drummer, Roadie, Guest, Fan. Swapping the two below Roadie is fine.
    assert!(s.can_move_role(BAND, BAND_FAN, -1));
    assert!(
        !s.can_move_role(BAND, BAND_GUEST, -1),
        "would put Guest in Roadie's place"
    );
    assert!(!s.can_move_role(BAND, BAND_DRUMMER, 1), "Drummer is above");
    assert!(
        !s.can_move_role(BAND, BAND_ROADIE, 1),
        "so is Roadie itself"
    );
    let before = fake.sent().len();
    app.act(|s| {
        assert!(!s.step_role(BAND, BAND_GUEST, -1));
        assert!(!s.step_role(BAND, BAND_DRUMMER, 1));
    });
    assert_eq!(fake.sent().len(), before);
    app.act(|s| {
        assert!(s.step_role(BAND, BAND_FAN, -1));
    });
    app.pump();
    assert_eq!(
        names(&app, BAND),
        list(&["Drummer", "Roadie", "Fan", "Guest"])
    );
}

#[test]
fn the_editor_of_a_locked_role_cannot_change_anything() {
    let (mut app, fake) = roadie();
    app.act(|s| s.select_role(BAND_DRUMMER));
    assert!(!app.state.form_editable());
    let before = fake.sent().len();
    app.act(|s| {
        s.set_role_color("red");
        s.toggle_permission(Permissions::MANAGE_ROLES);
        s.role_form().unwrap().name.replace_text("Boss");
        s.save_role();
        s.ask_delete_role(BAND_DRUMMER);
    });
    let f = app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .form
        .as_ref()
        .unwrap();
    assert_eq!(
        (f.color, f.perms),
        (PaletteColor::Orange, Permissions::NONE),
        "switches and swatches are inert"
    );
    assert!(
        app.state.ui.dialog.is_none(),
        "no delete confirmation either"
    );
    assert_eq!(fake.sent().len(), before);
    // A role below is editable, but not with a permission the Roadie lacks.
    app.act(|s| s.select_role(BAND_GUEST));
    assert!(app.state.form_editable());
    app.act(|s| {
        s.toggle_permission(Permissions::MANAGE_SERVER);
        s.toggle_permission(Permissions::MANAGE_ROLES);
    });
    let f = app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .form
        .as_ref()
        .unwrap();
    assert_eq!(
        f.perms,
        Permissions::MANAGE_ROLES,
        "the one they hold goes on, the other does not"
    );
}

#[test]
fn a_roadie_hands_out_roles_below_their_own_and_not_the_others() {
    let (mut app, fake) = roadie();
    app.act(|s| s.set_member_role(BAND, ANA, BAND_DRUMMER, true));
    assert!(
        app.state
            .ui
            .server_settings
            .as_ref()
            .unwrap()
            .member_busy
            .is_none(),
        "refused before sending"
    );
    assert!(!fake
        .sent()
        .iter()
        .any(|c| matches!(c, Command::SetMemberRoles { .. })));
    let given = app
        .state
        .assignable_roles(BAND, ANA)
        .iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    assert_eq!(
        given,
        [BAND_GUEST, BAND_FAN],
        "Drummer and Roadie are not on offer"
    );
    app.act(|s| s.set_member_role(BAND, ANA, BAND_GUEST, true));
    app.pump();
    assert_eq!(app.state.member_role_ids(BAND, ANA), [BAND_GUEST]);
    assert_eq!(fake.member_roles(BAND, ANA), [BAND_GUEST]);
    // The server's refusal is shown too, if a client lets one through (the roles changed meanwhile).
    let mut tx = rune::Tx::default();
    app.state.ui.server_settings.as_mut().unwrap().member_busy = Some(ANA);
    crate::apply::apply(
        &mut app.state,
        Reply::MemberRolesSet {
            guild: BAND,
            user: ANA,
            result: Err(ApiError::PermissionDenied),
        },
        &mut tx,
    );
    let ss = app.state.ui.server_settings.as_ref().unwrap();
    assert_eq!(
        ss.member_error.as_deref(),
        Some("You don't have permission to do that.")
    );
    assert!(ss.member_busy.is_none());
}

// ---- members ---------------------------------------------------------------------------------

#[test]
fn assigning_and_removing_a_role_sends_the_members_whole_set() {
    let (mut app, fake) = owner(ServerPage::Members);
    assert_eq!(
        app.state.assignable_roles(G, CVETKA).len(),
        3,
        "everything but Admin, which they hold"
    );
    app.act(|s| s.set_member_role(G, CVETKA, HIDEOUT_DESIGNERS, true));
    assert!(fake.sent().iter().any(|c| matches!(c, Command::SetMemberRoles { guild: 1, user, role_ids } if *user == CVETKA && role_ids == &[HIDEOUT_ADMINS, HIDEOUT_DESIGNERS])));
    assert_eq!(
        app.state.ui.server_settings.as_ref().unwrap().member_busy,
        Some(CVETKA)
    );
    app.pump();
    assert_eq!(
        app.state.member_role_ids(G, CVETKA),
        [HIDEOUT_ADMINS, HIDEOUT_DESIGNERS]
    );
    assert!(app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .member_busy
        .is_none());
    app.act(|s| s.set_member_role(G, CVETKA, HIDEOUT_ADMINS, false));
    app.pump();
    assert_eq!(app.state.member_role_ids(G, CVETKA), [HIDEOUT_DESIGNERS]);
    assert_eq!(fake.member_roles(G, CVETKA), [HIDEOUT_DESIGNERS]);
}

#[test]
fn the_member_search_narrows_the_list() {
    let (mut app, _) = owner(ServerPage::Members);
    assert_eq!(app.state.searched_members(G).len(), 4);
    app.act(|s| {
        s.ui.server_settings
            .as_mut()
            .unwrap()
            .search
            .replace_text("  AN ")
    });
    let found: Vec<String> = app
        .state
        .searched_members(G)
        .iter()
        .map(|u| app.state.user_name(*u))
        .collect();
    assert_eq!(found, ["ana"]);
    app.act(|s| {
        s.ui.server_settings
            .as_mut()
            .unwrap()
            .search
            .replace_text("zzz")
    });
    assert!(app.state.searched_members(G).is_empty());
}

#[test]
fn a_member_updated_event_changes_chips_and_name_colour_at_once() {
    let (mut app, fake) = owner(ServerPage::Members);
    fake.set_moderator(G, ANA, true);
    app.pump();
    assert!(app.state.member_role_ids(G, ANA).contains(&HIDEOUT_MODS));
    assert_eq!(
        app.state.name_color(G, ANA),
        PaletteColor::Teal,
        "Moderator outranks Designer"
    );
}

// ---- colours and groups ----------------------------------------------------------------------

#[test]
fn a_name_takes_the_colour_of_the_highest_role_that_has_one() {
    let (mut app, fake) = booted(true);
    let s = &app.state;
    assert_eq!(s.name_color(G, CVETKA), PaletteColor::Red, "Admin");
    assert_eq!(s.name_color(G, BRUNO), PaletteColor::Teal, "Moderator");
    assert_eq!(s.name_color(G, ANA), PaletteColor::Pink, "Designer");
    assert_eq!(
        s.name_color(G, TIM),
        PaletteColor::None,
        "no role, no colour"
    );
    // Two coloured roles: the higher one wins.
    fake.set_moderator(G, ANA, true);
    app.pump();
    assert_eq!(app.state.name_color(G, ANA), PaletteColor::Teal);
    // A higher role without a colour is skipped for the next one down that has one.
    let mut roles = fake.roles(G);
    roles
        .iter_mut()
        .find(|r| r.id == HIDEOUT_MODS)
        .unwrap()
        .color = PaletteColor::None;
    fake.gateway(GatewayUpdate::Event(Event::RolesUpdated {
        guild_id: G,
        roles,
        version: 9,
    }));
    app.pump();
    assert_eq!(
        app.state.name_color(G, ANA),
        PaletteColor::Pink,
        "Moderator has none now, Designer is next"
    );
    assert_eq!(
        app.state.name_color(G, BRUNO),
        PaletteColor::None,
        "Bruno has only the colourless one"
    );
    // Roles not loaded for a guild: no colour.
    assert_eq!(app.state.name_color(99, ANA), PaletteColor::None);
}

#[test]
fn the_member_panel_groups_by_each_members_highest_role() {
    let (mut app, fake) = booted(true);
    let groups = app.state.member_groups(G);
    let shape: Vec<(Option<&str>, Vec<u64>)> = groups
        .iter()
        .map(|g| (g.role.as_ref().map(|r| r.name.as_str()), g.users.clone()))
        .collect();
    assert_eq!(
        shape,
        [
            (Some("Admin"), vec![CVETKA]),
            (Some("Moderator"), vec![BRUNO]),
            (Some("Designer"), vec![ANA]),
            (None, vec![TIM])
        ],
        "most senior first, then everyone else"
    );
    assert!(
        shape.iter().all(|(r, _)| *r != Some("Regular")),
        "a role nobody has gets no header"
    );
    // Ana also becomes a Moderator: she moves up, and Designer disappears.
    fake.set_moderator(G, ANA, true);
    app.pump();
    let groups = app.state.member_groups(G);
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[1].role.as_ref().unwrap().name, "Moderator");
    assert_eq!(groups[1].users, [ANA, BRUNO], "alphabetical inside a group");
    // With no roles at all everyone is in the one group without a header.
    app.state.data.roles.insert(G, Vec::new());
    let g = app.state.member_groups(G);
    assert_eq!(g.len(), 1);
    assert!(g[0].role.is_none() && g[0].users.len() == 4);
}

// ---- avatars and icons -----------------------------------------------------------------------

#[test]
fn picking_a_file_reads_it_off_the_ui_thread_then_saving_uploads_its_bytes() {
    let (mut app, fake) = booted(true);
    let (path, bytes) = tmp_png("me.png", 5);
    app.act(|s| {
        s.open_profile();
        s.pick_image(ImagePurpose::Avatar, &path);
    });
    // The state only asked: reading belongs to the backend.
    assert!(fake.sent().iter().any(
        |c| matches!(c, Command::ReadImage { path: p, purpose: ImagePurpose::Avatar } if *p == path)
    ));
    assert!(app.state.ui.dialog.as_ref().unwrap().pick.reading);
    app.pump();
    let d = app.state.ui.dialog.as_ref().unwrap();
    assert!(!d.pick.reading);
    let draft = d
        .pick
        .draft
        .as_ref()
        .expect("the picked picture is waiting");
    assert_eq!(
        (&draft.bytes, draft.content_type.as_str()),
        (&bytes, "image/png")
    );
    assert_eq!(draft.preview.wait().ok(), Some([32, 32]), "and previews");
    // Save: the command carries exactly the file's bytes.
    app.act(|s| s.submit_dialog());
    assert!(fake.sent().iter().any(|c| matches!(c, Command::UploadAvatar { bytes: b, content_type } if b == &bytes && content_type == "image/png")));
    assert_eq!(
        app.state.ui.dialog.as_ref().unwrap().phase,
        Phase::Busy,
        "busy while it uploads"
    );
    app.act(|s| s.submit_dialog());
    assert_eq!(
        fake.sent()
            .iter()
            .filter(|c| matches!(c, Command::UploadAvatar { .. }))
            .count(),
        1,
        "not twice"
    );
    app.pump();
    assert!(app.state.ui.dialog.is_none(), "closed on success");
    let me = app.state.me().unwrap().clone();
    assert!(me.avatar.is_some());
    assert_eq!(fake.user(TIM).unwrap().avatar, me.avatar);
    assert!(
        app.state.data.media.contains(me.avatar.as_deref().unwrap()),
        "fetched and cached"
    );
    app.state.data.media.settle();
    assert!(app.state.avatar_pixels(TIM).is_some(), "so it is drawn");
    assert!(app
        .state
        .ui
        .toasts
        .iter()
        .any(|t| t.message == "Profile picture updated"));
    std::fs::remove_file(path).ok();
}

#[test]
fn a_file_that_is_not_an_image_or_is_too_big_is_refused_in_the_dialog() {
    let (mut app, fake) = booted(true);
    let dir =
        std::env::temp_dir().join(format!("thehideout-roles-tests-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("notes.txt");
    std::fs::write(&text, "hi").unwrap();
    app.act(|s| {
        s.open_profile();
        s.pick_image(ImagePurpose::Avatar, &text);
    });
    assert!(app
        .state
        .ui
        .dialog
        .as_ref()
        .unwrap()
        .error
        .as_deref()
        .is_some_and(|e| e.contains("PNG")));
    assert!(
        !fake
            .sent()
            .iter()
            .any(|c| matches!(c, Command::ReadImage { .. })),
        "refused by its extension, never read"
    );
    let huge = dir.join("huge.png");
    std::fs::write(&huge, vec![0u8; thehideout_api::MAX_MEDIA_BYTES + 1]).unwrap();
    app.act(|s| s.pick_image(ImagePurpose::Avatar, &huge));
    app.pump();
    let d = app.state.ui.dialog.as_ref().unwrap();
    assert!(d.pick.draft.is_none() && !d.pick.reading);
    assert!(
        d.error.as_deref().is_some_and(|e| e.contains("8 MiB")),
        "{:?}",
        d.error
    );
    // An exactly 8 MiB file is within the limit.
    let edge = dir.join("edge.gif");
    std::fs::write(&edge, vec![b'G'; thehideout_api::MAX_MEDIA_BYTES]).unwrap();
    app.act(|s| s.pick_image(ImagePurpose::Avatar, &edge));
    app.pump();
    assert!(app.state.ui.dialog.as_ref().unwrap().pick.draft.is_some());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn removing_the_avatar_clears_it_everywhere() {
    let (mut app, fake) = booted(true);
    fake.give_avatars();
    app.pump();
    assert!(app.state.me().unwrap().avatar.is_some());
    app.act(|s| {
        s.open_profile();
        s.remove_avatar();
    });
    assert!(fake
        .sent()
        .iter()
        .any(|c| matches!(c, Command::RemoveAvatar)));
    app.pump();
    assert!(app.state.me().unwrap().avatar.is_none());
    assert!(app.state.data.users[&TIM].avatar.is_none());
    assert!(app.state.avatar_pixels(TIM).is_none(), "back to initials");
    assert!(app.state.ui.dialog.is_none());
    assert!(app
        .state
        .ui
        .toasts
        .iter()
        .any(|t| t.message == "Profile picture removed"));
}

#[test]
fn a_user_updated_event_refreshes_the_avatar_everywhere_it_is_shown() {
    let (mut app, fake) = booted(true);
    assert!(app.state.avatar_pixels(ANA).is_none());
    let mut ana = fake.user(ANA).unwrap();
    ana.avatar = Some("gen-777".into());
    fake.gateway(GatewayUpdate::Event(Event::UserUpdated(ana)));
    app.pump();
    app.state.data.media.settle();
    assert_eq!(
        app.state.data.users[&ANA].avatar.as_deref(),
        Some("gen-777")
    );
    let dm = app
        .state
        .data
        .dms
        .iter()
        .find(|d| d.participants.iter().any(|p| p.id == ANA))
        .unwrap();
    assert_eq!(
        dm.participants
            .iter()
            .find(|p| p.id == ANA)
            .unwrap()
            .avatar
            .as_deref(),
        Some("gen-777"),
        "the DM's copy too"
    );
    assert!(
        app.state.avatar_pixels(ANA).is_some(),
        "fetched, decoded and ready to draw"
    );
    // Told again with the same key: no second fetch. A new key: a new fetch.
    let fetches = |f: &FakeBackend| {
        f.sent()
            .iter()
            .filter(|c| matches!(c, Command::FetchMedia { .. }))
            .count()
    };
    let before = fetches(&fake);
    let mut ana = fake.user(ANA).unwrap();
    ana.avatar = Some("gen-777".into());
    fake.gateway(GatewayUpdate::Event(Event::UserUpdated(ana.clone())));
    app.pump();
    assert_eq!(fetches(&fake), before);
    ana.avatar = Some("gen-778".into());
    fake.gateway(GatewayUpdate::Event(Event::UserUpdated(ana)));
    app.pump();
    assert_eq!(fetches(&fake), before + 1);
    assert!(app.state.data.media.contains("gen-778"));
}

#[test]
fn an_avatar_that_cannot_be_fetched_stays_initials_and_is_not_retried_in_a_loop() {
    let (mut app, fake) = booted(true);
    let mut ana = fake.user(ANA).unwrap();
    ana.avatar = Some("missing-key".into());
    fake.gateway(GatewayUpdate::Event(Event::UserUpdated(ana)));
    app.pump();
    assert!(app.state.avatar_pixels(ANA).is_none());
    app.act(|s| s.go_home());
    app.pump();
    let asks = fake
        .sent()
        .iter()
        .filter(|c| matches!(c, Command::FetchMedia { key } if key == "missing-key"))
        .count();
    assert_eq!(asks, 1, "one failed fetch, no retry storm");
}

#[test]
fn the_server_icon_flow_picks_previews_saves_and_removes() {
    let (mut app, fake) = owner(ServerPage::Overview);
    let (path, bytes) = tmp_png("icon.png", 9);
    app.act(|s| s.pick_image(ImagePurpose::GuildIcon { guild: G }, &path));
    app.pump();
    let draft = app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .icon
        .draft
        .as_ref()
        .expect("picked");
    assert_eq!(draft.bytes, bytes);
    app.act(|s| s.save_icon());
    assert!(fake.sent().iter().any(|c| matches!(c, Command::UploadGuildIcon { guild: 1, bytes: b, content_type } if b == &bytes && content_type == "image/png")));
    assert!(app.state.ui.server_settings.as_ref().unwrap().icon_busy);
    app.pump();
    let ss = app.state.ui.server_settings.as_ref().unwrap();
    assert!(!ss.icon_busy && ss.icon.draft.is_none());
    let key = app
        .state
        .guild(G)
        .unwrap()
        .icon
        .clone()
        .expect("the guild has an icon");
    assert!(
        app.state.data.media.contains(&key),
        "GuildUpdated brought the key and it was fetched"
    );
    app.state.data.media.settle();
    assert!(app.state.icon_pixels(G).is_some());
    app.act(|s| s.remove_icon());
    app.pump();
    assert!(app.state.guild(G).unwrap().icon.is_none() && fake.guild(G).unwrap().icon.is_none());
    assert!(app.state.icon_pixels(G).is_none());
    std::fs::remove_file(path).ok();
}

#[test]
fn renaming_the_server_validates_then_saves() {
    let (mut app, fake) = owner(ServerPage::Overview);
    let before = fake.sent().len();
    app.act(|s| {
        s.ui.server_settings
            .as_mut()
            .unwrap()
            .name
            .replace_text("x");
        s.rename_guild();
    });
    assert!(app
        .state
        .ui
        .server_settings
        .as_ref()
        .unwrap()
        .name_error
        .is_some());
    app.act(|s| {
        s.ui.server_settings
            .as_mut()
            .unwrap()
            .name
            .replace_text("The Hideout");
        s.rename_guild();
    });
    assert_eq!(
        fake.sent().len(),
        before,
        "nothing for an invalid or unchanged name"
    );
    app.act(|s| {
        s.ui.server_settings
            .as_mut()
            .unwrap()
            .name
            .replace_text("  The Den  ");
        s.rename_guild();
    });
    assert!(fake
        .sent()
        .iter()
        .any(|c| matches!(c, Command::RenameGuild { guild: 1, name } if name == "The Den")));
    app.pump();
    assert_eq!(app.state.guild(G).unwrap().name, "The Den");
    assert!(!app.state.ui.server_settings.as_ref().unwrap().name_busy);
}

#[test]
fn a_guild_updated_event_from_someone_else_changes_the_icon_here() {
    let (mut app, fake) = booted(true);
    assert!(app.state.icon_pixels(G).is_none());
    fake.give_icon(G);
    app.pump();
    app.state.data.media.settle();
    assert!(app.state.guild(G).unwrap().icon.is_some());
    assert!(app.state.icon_pixels(G).is_some());
}

// ---- through the real view -------------------------------------------------------------------

#[test]
fn the_guild_menu_opens_server_settings_and_escape_leaves_them() {
    let (app, _) = booted(true);
    let mut rt: Runtime<App> = runtime(app);
    click(&mut rt, "guild-header");
    settle(&mut rt, 8);
    click(&mut rt, "guild-menu-item3");
    settle(&mut rt, 8);
    assert!(rt.state.state.ui.server_settings.is_some());
    assert!(
        has(&rt, "server-nav-Roles")
            && has(&rt, "server-nav-Members")
            && has(&rt, "server-nav-Overview")
    );
    click(&mut rt, "server-nav-Roles");
    settle(&mut rt, 8);
    assert!(has(&rt, "role-row-1001") && has(&rt, "role-new"));
    click(&mut rt, "role-row-1001");
    settle(&mut rt, 8);
    assert!(has(&rt, "role-editor") && has(&rt, "perm-administrator"));
    assert!(!has(&rt, "admin-warning"));
    click(&mut rt, "perm-administrator");
    settle(&mut rt, 4);
    assert!(
        has(&rt, "admin-warning"),
        "Administrator warns that it grants everything"
    );
    crate::tests::key(&mut rt, rune::Key::Escape);
    settle(&mut rt, 4);
    assert!(rt.state.state.ui.server_settings.is_none());
}

#[test]
fn the_roadie_sees_locked_rows_and_disabled_controls_in_the_view() {
    let (mut app, _) = booted(true);
    app.act(|s| s.go_guild(BAND));
    app.pump();
    app.act(|s| s.open_server_settings(BAND));
    let mut rt: Runtime<App> = runtime(app);
    assert!(
        has(&rt, "role-lock-2003") && has(&rt, "role-lock-2001"),
        "Drummer and Roadie show a lock"
    );
    assert!(
        !has(&rt, "role-up-2003") && has(&rt, "role-up-2002"),
        "only roles they may move have buttons"
    );
    assert!(
        !has(&rt, "server-nav-Overview"),
        "Overview needs MANAGE_SERVER"
    );
    click(&mut rt, "role-row-2003");
    settle(&mut rt, 6);
    assert!(has(&rt, "role-readonly"));
}

#[test]
fn the_profile_dialog_takes_a_picture_from_the_file_dialog_and_from_a_drop() {
    let (app, fake) = booted(true);
    let mut rt: Runtime<App> = runtime(app);
    click(&mut rt, "user-menu-button");
    settle(&mut rt, 8);
    click(&mut rt, "user-menu-item0");
    settle(&mut rt, 8);
    assert!(matches!(
        rt.state.state.ui.dialog.as_ref().map(|d| d.kind),
        Some(DialogKind::Profile)
    ));
    // The choose button asks Rune for an Open dialog, filtered to images; a fake picker answers.
    click(&mut rt, "profile-choose");
    settle(&mut rt, 2);
    let (kind, options) = rt.file_dialog().expect("the dialog was requested");
    assert_eq!(kind, rune::FileDialogKind::Open);
    assert!(options
        .filters
        .iter()
        .any(|(_, ext)| ext.iter().any(|e| e == "webp")));
    let (path, bytes) = tmp_png("picked.png", 21);
    rt.resolve_file_dialog(Some(path.clone())).unwrap();
    settle(&mut rt, 6);
    let d = rt.state.state.ui.dialog.as_ref().unwrap();
    assert_eq!(
        d.pick.draft.as_ref().map(|x| &x.bytes),
        Some(&bytes),
        "read through the backend and kept"
    );
    // Cancelling a dialog changes nothing; a drop on the zone replaces the draft.
    click(&mut rt, "profile-choose");
    settle(&mut rt, 2);
    rt.resolve_file_dialog(None).unwrap();
    settle(&mut rt, 4);
    assert!(rt
        .state
        .state
        .ui
        .dialog
        .as_ref()
        .unwrap()
        .pick
        .draft
        .is_some());
    let (second, bytes2) = tmp_png("dropped.png", 22);
    let at = rt.find("profile-zone").expect("the drop zone");
    rt.drop_files(vec![second.clone()], Some(at));
    settle(&mut rt, 6);
    assert_eq!(
        rt.state
            .state
            .ui
            .dialog
            .as_ref()
            .unwrap()
            .pick
            .draft
            .as_ref()
            .map(|x| &x.bytes),
        Some(&bytes2)
    );
    // A file of another kind dropped on it is not accepted at all.
    let txt = second.with_file_name("readme.txt");
    std::fs::write(&txt, "x").unwrap();
    rt.drop_files(vec![txt.clone()], Some(at));
    settle(&mut rt, 4);
    assert_eq!(
        rt.state
            .state
            .ui
            .dialog
            .as_ref()
            .unwrap()
            .pick
            .draft
            .as_ref()
            .map(|x| &x.bytes),
        Some(&bytes2)
    );
    click(&mut rt, "dialog-submit");
    settle(&mut rt, 10);
    assert!(rt.state.state.ui.dialog.is_none());
    assert!(fake.user(TIM).unwrap().avatar.is_some());
    for p in [path, second, txt] {
        std::fs::remove_file(p).ok();
    }
}
