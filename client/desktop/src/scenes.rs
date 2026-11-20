//! Headless scenes for `--scene NAME --snap out.png`: each boots the fake backend, signs in
//! where the scene needs it and sets one situation up. `NAME-light` is the light twin.

use thehideout_api::ApiError;
use thehideout_design::components::ToastKind;
use thehideout_design::{AccentPreset, Appearance, BackgroundPreset, ThemeSpec};

use crate::app::{fake_now, App};
use crate::backend::Command;
use crate::emoji::PickerTarget;
use crate::fake::{FakeBackend, ANA, ARCHIVE, BRUNO, CVETKA, DESIGN, DEV, DM_ANA, GENERAL, HIDEOUT_DESIGNERS, HIDEOUT_MODS, MEMES, MUSIC, PLANS, RANDOM, SMUG_CAT};
use crate::format::Tz;
use crate::server_settings::ServerPage;
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

/// The busy main screen with pictures: every member has an avatar, The Hideout an icon, role
/// colours show in the transcript and the member panel, and a mention of coloured members.
fn avatars_screen(dark: bool) -> (App, FakeBackend) {
    let (mut a, fake) = main_screen(dark);
    fake.give_avatars();
    fake.give_icon(1);
    fake.incoming(GENERAL, BRUNO, "@ana @cvetka the mockups are in #design, could you both take a look?");
    fake.incoming(GENERAL, CVETKA, "on it. @bruno thanks for the heads up");
    fake.incoming(GENERAL, ANA, "@tim you can merge when ready");
    a.pump();
    a.state.data.media.settle();
    (a, fake)
}

fn main_avatars(a: &mut App, dark: bool) {
    let (mut app, _fake) = avatars_screen(dark);
    app.state.ui.members_open = true;
    *a = app;
}

/// The server settings page on `page`, for the owner of The Hideout.
fn server_settings(a: &mut App, dark: bool, page: ServerPage, role: Option<u64>) {
    let (mut app, _fake) = avatars_screen(dark);
    app.act(|s| {
        s.open_server_settings(1);
        s.server_page(page);
        if let Some(r) = role {
            s.select_role(r);
        }
    });
    app.pump();
    *a = app;
}

/// The same as Tim, a Roadie in Garage Band: roles above his own are locked.
fn server_settings_limited(a: &mut App, dark: bool) {
    let (mut app, fake) = booted(dark);
    fake.give_avatars();
    app.act(|s| s.go_guild(2));
    app.pump();
    app.act(|s| {
        s.open_server_settings(2);
        s.select_role(crate::fake::BAND_DRUMMER);
    });
    app.pump();
    app.state.data.media.settle();
    *a = app;
}

/// The profile dialog with a picture chosen and waiting to be saved.
fn profile_dialog(a: &mut App, dark: bool) {
    let (mut app, _fake) = avatars_screen(dark);
    let path = std::env::temp_dir().join(format!("thehideout-profile-scene-{}.png", std::process::id()));
    std::fs::write(&path, crate::media::generated_png(41, 96)).ok();
    app.act(|s| {
        s.open_profile();
        s.pick_image(crate::backend::ImagePurpose::Avatar, &path);
    });
    app.pump();
    app.state.data.media.settle();
    if let Some(d) = app.state.ui.dialog.as_ref().and_then(|d| d.pick.draft.as_ref()) {
        let _ = d.preview.wait();
    }
    std::fs::remove_file(&path).ok();
    *a = app;
}

// ---- emojis ----------------------------------------------------------------------------------

/// #memes open with the emojis fetched and decoded: custom ones in lines, jumbo messages, chips.
fn memes(dark: bool) -> (App, FakeBackend) {
    let (mut app, fake) = booted(dark);
    app.act(|s| s.select_channel(MEMES));
    app.pump();
    app.state.data.media.settle();
    (app, fake)
}

fn emoji_picker(a: &mut App, dark: bool) {
    let (mut app, _fake) = memes(dark);
    app.act(|s| s.open_picker(PickerTarget::Composer, "composer-emoji"));
    // A few recents, as if the user had been reacting.
    app.act(|s| {
        for key in ["u:\u{1f44d}", "c:7002", "u:\u{1f602}", "u:\u{2764}\u{fe0f}", "c:7001", "u:\u{1f389}"].iter().rev() {
            if let Some(p) = thehideout_design::components::Pick::from_key(key, &s.server_emojis()) {
                s.record_recent(&p);
            }
        }
    });
    app.state.data.media.settle();
    *a = app;
}

fn emoji_autocomplete(a: &mut App, dark: bool) {
    let (mut app, _fake) = memes(dark);
    let edit = app.state.composer_mut();
    edit.replace_text("that deserves a :sm");
    let end = edit.text.len();
    edit.set_caret(end);
    edit.focused = true;
    *a = app;
}

/// #general's bottom with reactions from several people: chips, one of them yours, and one picker
/// open from a message's "+" chip.
fn reactions(a: &mut App, dark: bool, picker: bool) {
    let (mut app, _fake) = memes(dark);
    app.act(|s| s.select_channel(GENERAL));
    app.pump();
    if picker {
        let target = app.state.current_channel().and_then(|c| app.state.log(c)).and_then(|l| l.messages.iter().rev().nth(2)).map(|m| (m.channel_id, m.id));
        if let Some((channel, message)) = target {
            let anchor = format!("react-add-m:{message}");
            app.act(|s| s.open_picker(PickerTarget::Reaction { channel, message }, anchor));
        }
    }
    *a = app;
}

/// #memes with a run of jumbo messages at the bottom.
fn jumbo_emoji(a: &mut App, dark: bool) {
    let (mut app, fake) = memes(dark);
    fake.incoming(MEMES, ANA, "<:blob_love:7004>");
    fake.incoming(MEMES, ANA, "<a:dance_cat:7005><a:dance_cat:7005><a:dance_cat:7005>");
    fake.incoming(MEMES, BRUNO, "\u{1f680} \u{2728} \u{1f525}");
    fake.incoming(MEMES, BRUNO, "<:smug_cat:7001> \u{1f60e}");
    app.pump();
    app.state.data.media.settle();
    *a = app;
}

fn server_settings_emojis(a: &mut App, dark: bool) {
    let (mut app, _fake) = memes(dark);
    // A GIF waiting to be named, as if just picked.
    let path = std::env::temp_dir().join(format!("Party Cat-{}.gif", std::process::id()));
    std::fs::write(&path, crate::media::generated_animated_emoji(77, 64, 8)).ok();
    app.act(|s| {
        s.open_server_settings(1);
        s.server_page(ServerPage::Emojis);
        s.pick_emoji_file(&path);
    });
    app.pump();
    app.state.data.media.settle();
    if let Some(d) = app.state.ui.server_settings.as_ref().and_then(|s| s.emojis.draft.as_ref()) {
        let _ = d.image.preview.wait();
    }
    std::fs::remove_file(&path).ok();
    *a = app;
}

/// #dev's settings dialog with the icon picker open on `tab` (1 Emoji, 2 Server emoji); the
/// sidebar already shows a Unicode emoji icon on #general and a custom one on #memes.
fn channel_icon_emoji(a: &mut App, dark: bool, tab: usize) {
    let (mut app, _fake) = memes(dark);
    let set = |channel: u64, icon: thehideout_api::ChannelIconRef| Command::UpdateChannel { guild: 1, channel, name: None, icon: Some(icon), color: None, topic: None };
    app.state.outbox.push(set(GENERAL, thehideout_api::ChannelIconRef::Unicode("\u{1f4ac}".into())));
    app.state.outbox.push(set(MEMES, thehideout_api::ChannelIconRef::Custom(SMUG_CAT)));
    app.act(|s| s.select_channel(DEV));
    app.pump();
    app.state.data.media.settle();
    app.act(|s| s.open_dialog(DialogKind::ChannelSettings { guild: 1, channel: DEV }));
    app.act(|s| s.toggle_picker());
    app.act(|s| s.set_picker_tab(tab));
    app.state.data.media.settle();
    *a = app;
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
    ("main-avatars", |a| main_avatars(a, true)),
    ("main-avatars-light", |a| main_avatars(a, false)),
    ("server-settings-overview", |a| server_settings(a, true, ServerPage::Overview, None)),
    ("server-settings-overview-light", |a| server_settings(a, false, ServerPage::Overview, None)),
    ("server-settings-roles", |a| server_settings(a, true, ServerPage::Roles, Some(HIDEOUT_MODS))),
    ("server-settings-roles-light", |a| server_settings(a, false, ServerPage::Roles, Some(HIDEOUT_DESIGNERS))),
    ("server-settings-roles-limited", |a| server_settings_limited(a, true)),
    ("server-settings-roles-limited-light", |a| server_settings_limited(a, false)),
    ("server-settings-members", |a| server_settings(a, true, ServerPage::Members, None)),
    ("server-settings-members-light", |a| server_settings(a, false, ServerPage::Members, None)),
    ("profile-dialog", |a| profile_dialog(a, true)),
    ("profile-dialog-light", |a| profile_dialog(a, false)),
    ("emoji-picker", |a| emoji_picker(a, true)),
    ("emoji-picker-light", |a| emoji_picker(a, false)),
    ("emoji-autocomplete", |a| emoji_autocomplete(a, true)),
    ("emoji-autocomplete-light", |a| emoji_autocomplete(a, false)),
    ("reactions", |a| reactions(a, true, false)),
    ("reactions-light", |a| reactions(a, false, false)),
    ("reactions-picker", |a| reactions(a, true, true)),
    ("jumbo-emoji", |a| jumbo_emoji(a, true)),
    ("jumbo-emoji-light", |a| jumbo_emoji(a, false)),
    ("server-settings-emojis", |a| server_settings_emojis(a, true)),
    ("server-settings-emojis-light", |a| server_settings_emojis(a, false)),
    ("channel-icon-emoji-tabs", |a| channel_icon_emoji(a, true, 2)),
    ("channel-icon-emoji-tabs-light", |a| channel_icon_emoji(a, false, 2)),
    ("channel-icon-unicode-tab", |a| channel_icon_emoji(a, true, 1)),
    ("main-small", |a| main_(a, true)),
    ("main-small-light", |a| main_(a, false)),
];
