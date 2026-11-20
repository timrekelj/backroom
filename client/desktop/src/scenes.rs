//! Headless scenes for `--scene NAME --snap out.png`: each boots the fake backend, signs in
//! where the scene needs it and sets one situation up. `NAME-light` is the light twin.

use thehideout_api::ApiError;
use thehideout_design::components::ToastKind;
use thehideout_design::{AccentPreset, Appearance, BackgroundPreset, ThemeSpec};

use crate::app::{fake_now, App};
use crate::fake::{FakeBackend, ANA, ARCHIVE, BRUNO, CVETKA, DESIGN, DEV, DM_ANA, GENERAL, MUSIC, PLANS, RANDOM};
use crate::format::Tz;
use crate::state::{auth_error, AppState, AuthMode, DialogKind, Menu};

/// A signed-in app on the fake world, in the given theme, with The Hideout's #general open.
pub fn booted(dark: bool) -> (App, FakeBackend) {
    booted_as(dark, "tim")
}

/// The same, signed in as `who`: tim owns The Hideout, ana is a plain member there.
pub fn booted_as(dark: bool, who: &str) -> (App, FakeBackend) {
    let fake = FakeBackend::new();
    let mut state = AppState::new(fake_now, Tz::Fixed(0));
    state.ui.look.spec.appearance = if dark { Appearance::Dark } else { Appearance::Light };
    let mut app = App::new(Box::new(fake.clone()), state);
    app.state.auth.username.replace_text(who);
    app.state.auth.password.replace_text("hunter22");
    app.act(|s| s.submit_auth());
    app.pump();
    (app, fake)
}

fn signed_out(dark: bool) -> App {
    let mut state = AppState::new(fake_now, Tz::Fixed(0));
    state.ui.look.spec.appearance = if dark { Appearance::Dark } else { Appearance::Light };
    App::new(Box::new(FakeBackend::new()), state)
}

fn auth_login(a: &mut App, dark: bool) {
    *a = signed_out(dark);
    a.state.auth.username.replace_text("tim");
    a.state.auth.username.focused = false;
    a.state.auth.password.replace_text("hunter22");
    a.state.auth.password.focused = true;
}

fn auth_signup_error(a: &mut App, dark: bool) {
    *a = signed_out(dark);
    a.state.set_auth_mode(AuthMode::SignUp);
    a.state.auth.username.replace_text("taken");
    a.state.auth.username.focused = false;
    a.state.auth.password.replace_text("secret123");
    a.state.auth.error = Some(auth_error(&ApiError::AlreadyExists));
}

/// The busy main screen: a read channel, a channel with unread messages (one mentions you), a
/// DM waiting, another server with news, and a "New" divider in #general.
fn main_screen(dark: bool) -> (App, FakeBackend) {
    let (mut a, fake) = booted(dark);
    a.act(|s| s.select_channel(DEV));
    a.pump();
    fake.incoming(
        GENERAL,
        BRUNO,
        "Anyone up for lunch? The ramen place opens at noon.",
    );
    fake.incoming(GENERAL, ANA, "@tim bring the speaker on Friday!");
    a.pump();
    a.act(|s| s.select_channel(GENERAL));
    a.pump();
    fake.incoming(
        DEV,
        CVETKA,
        "@tim could you review the backoff test when you have a minute?",
    );
    fake.incoming(RANDOM, CVETKA, "look what I found");
    fake.incoming(20, ANA, "rehearsal at seven");
    fake.incoming(DM_ANA, ANA, "did you see my message?");
    a.pump();
    (a, fake)
}

fn main_(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
}

/// The busy main screen in a chosen look (accent, background, tint, contrast), for review.
fn main_look(a: &mut App, dark: bool, accent: AccentPreset, background: BackgroundPreset, tint: bool) {
    *a = main_screen(dark).0;
    a.state.ui.look.spec = ThemeSpec {
        accent,
        background,
        tint,
        appearance: a.state.ui.look.spec.appearance,
        ..ThemeSpec::default()
    };
}

/// The Appearance page of the settings, over a look that is not the default, so the selected
/// swatch and tile are not the first ones.
fn settings_appearance(a: &mut App, dark: bool) {
    main_look(a, dark, AccentPreset::Ocean, BackgroundPreset::Cool, true);
    a.act(|s| s.open_settings());
    a.pump();
}

fn main_members(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
    a.state.ui.members_open = true;
}

fn home_dms(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
    a.act(|s| {
        s.go_home();
        s.select_channel(DM_ANA);
    });
    a.pump();
}

fn home_empty(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
    a.act(|s| s.go_home());
    a.pump();
}

/// A toast while a channel is open: it must sit above the composer.
fn toast(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
    a.state.toast(
        ToastKind::Danger,
        "Message not sent. Can't reach the server.",
    );
}

fn empty_channel(a: &mut App, dark: bool) {
    *a = booted(dark).0;
    a.act(|s| s.select_channel(PLANS));
    a.pump();
}

fn dialog_create_server(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
    a.state.open_dialog(DialogKind::CreateGuild);
    if let Some(d) = &mut a.state.ui.dialog {
        d.input.replace_text("Weekend Climbers");
    }
}

fn reconnecting(a: &mut App, dark: bool) {
    let (mut app, fake) = main_screen(dark);
    fake.disconnect(5);
    app.pump();
    *a = app;
}

/// The busy sidebar: top-level channels, three sections (one folded), coloured icons, unread
/// and a mention, #design selected.
fn sections(a: &mut App, dark: bool) {
    let (mut app, fake) = main_screen(dark);
    fake.incoming(MUSIC, CVETKA, "new playlist is up");
    app.pump();
    app.act(|s| {
        s.toggle_section(1, ARCHIVE);
        s.select_channel(DESIGN);
    });
    app.pump();
    *a = app;
}

/// The channel settings dialog for #dev with the icon picker open on its Icons tab.
fn channel_settings(a: &mut App, dark: bool, search: &str) {
    *a = main_screen(dark).0;
    a.act(|s| s.open_dialog(DialogKind::ChannelSettings { guild: 1, channel: DEV }));
    a.act(|s| s.toggle_picker());
    if let Some(f) = a.state.channel_form() {
        f.search.replace_text(search);
        f.research();
    }
}

/// A channel's context menu, open where a right-click landed.
fn reorder_menu(a: &mut App, dark: bool) {
    *a = main_screen(dark).0;
    a.state.ui.menu = Menu::Channel { id: DESIGN, at: Some((110, 330)) };
}

type Scene = (&'static str, fn(&mut App));

pub const SCENES: &[Scene] = &[
    ("auth-login", |a| auth_login(a, true)),
    ("auth-login-light", |a| auth_login(a, false)),
    ("auth-signup-error", |a| auth_signup_error(a, true)),
    ("auth-signup-error-light", |a| auth_signup_error(a, false)),
    ("main", |a| main_(a, true)),
    ("main-light", |a| main_(a, false)),
    ("main-members", |a| main_members(a, true)),
    ("main-members-light", |a| main_members(a, false)),
    ("home-dms", |a| home_dms(a, true)),
    ("home-dms-light", |a| home_dms(a, false)),
    ("home-empty", |a| home_empty(a, true)),
    ("home-empty-light", |a| home_empty(a, false)),
    ("toast", |a| toast(a, true)),
    ("toast-light", |a| toast(a, false)),
    ("empty-channel", |a| empty_channel(a, true)),
    ("empty-channel-light", |a| empty_channel(a, false)),
    ("dialog-create-server", |a| dialog_create_server(a, true)),
    ("dialog-create-server-light", |a| {
        dialog_create_server(a, false)
    }),
    ("reconnecting", |a| reconnecting(a, true)),
    ("reconnecting-light", |a| reconnecting(a, false)),
    ("settings-appearance", |a| settings_appearance(a, true)),
    ("settings-appearance-light", |a| settings_appearance(a, false)),
    ("main-ocean-cool", |a| {
        main_look(a, true, AccentPreset::Ocean, BackgroundPreset::Cool, true)
    }),
    ("main-coral-paper-light", |a| {
        main_look(a, false, AccentPreset::Coral, BackgroundPreset::Paper, true)
    }),
    ("main-graphite-deep", |a| {
        main_look(a, true, AccentPreset::Graphite, BackgroundPreset::Deep, false)
    }),
    ("sections", |a| sections(a, true)),
    ("sections-light", |a| sections(a, false)),
    ("channel-settings", |a| channel_settings(a, true, "")),
    ("channel-settings-light", |a| channel_settings(a, false, "")),
    ("icon-picker-search", |a| channel_settings(a, true, "game")),
    ("icon-picker-search-light", |a| channel_settings(a, false, "game")),
    ("reorder-menu", |a| reorder_menu(a, true)),
    ("reorder-menu-light", |a| reorder_menu(a, false)),
    ("main-small", |a| main_(a, true)),
    ("main-small-light", |a| main_(a, false)),
];
