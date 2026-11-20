//! The auth screen: a centred card with the wordmark, a Log in / Sign up switch, two fields,
//! the one primary button and a quiet server line. Errors from the server land under the field
//! they belong to. The button says what it is doing while it waits (no spinner).

use rune::{El, col, el, row, text};
use rune_kit::Kit;
use thehideout_design::components::{Button, Segmented, TextInput};
use thehideout_design::{radius, theme_of, type_};

use crate::app::App;
use crate::state::{AuthField, AuthMode, Phase};

fn username_edit(a: &mut App) -> &mut rune::TextEdit {
    &mut a.state.auth.username
}
fn password_edit(a: &mut App) -> &mut rune::TextEdit {
    &mut a.state.auth.password
}
fn server_edit(a: &mut App) -> &mut rune::TextEdit {
    &mut a.state.auth.server
}

fn field(k: &Kit, key: &str, label: &str, input: TextInput, error: Option<&str>) -> El {
    let t = theme_of(k);
    let mut c = col().key(key.to_owned()).w_full().gap(6.0).child(text(label.to_owned()).font(type_::at(k, type_::LABEL)).color(t.text_muted.color()));
    c = c.child(input.width(None).large().invalid(error.is_some()).label(label.to_owned()).build(k));
    match error {
        Some(e) => c.child(text(e.to_owned()).key("error").font(type_::at(k, type_::CONTROL)).color(t.danger.color()).wrap().w_full()),
        None => c,
    }
}

pub fn build(a: &App, k: &Kit) -> El {
    let t = theme_of(k);
    let auth = &a.state.auth;
    let busy = auth.phase == Phase::Busy;
    let error_for = |f: AuthField| auth.error.as_ref().filter(|e| e.field == f).map(|e| e.message.as_str());
    let sign_up = auth.mode == AuthMode::SignUp;

    let mode = Segmented::new(&["Log in", "Sign up"], usize::from(sign_up))
        .label("Log in or sign up")
        .on_pick(|a: &mut App, i| a.act(|s| s.set_auth_mode(if i == 0 { AuthMode::Login } else { AuthMode::SignUp })))
        .build(k);

    let submit = |a: &mut App, _tx: &mut rune::Tx| a.act(|s| s.submit_auth());
    let username = TextInput::new(&auth.username).placeholder("Username").on_edit(username_edit).on_submit(submit);
    let password = TextInput::new(&auth.password).placeholder("Password").secure().on_edit(password_edit).on_submit(submit);

    let label = match (busy, sign_up) {
        (true, true) => "Creating account\u{2026}",
        (true, false) => "Logging in\u{2026}",
        (false, true) => "Create account",
        (false, false) => "Log in",
    };
    let button = Button::new(label).disabled(busy).on_press(|a: &mut App| a.act(|s| s.submit_auth())).build(k).key("auth-submit").w_full().mt(8.0);

    let mut card = col()
        .key("auth-card")
        .w(380.0)
        .max_w(380.0)
        .padding(32.0)
        .gap(16.0)
        .radius(radius::LG)
        .fill(t.surface.color())
        .border(1.0, t.border.color())
        .child(
            col()
                .gap(4.0)
                .child(text("thehideout").font(type_::at(k, type_::HEADING)).color(t.accent_text.color()))
                .child(text(if sign_up { "Make an account to join your friends." } else { "Welcome back. Your friends are waiting." }).font(type_::at(k, type_::CONTROL)).color(t.text_muted.color()).wrap().w_full()),
        )
        .child(col().key("auth-mode").w_full().child(mode))
        .child(field(k, "auth-username", "Username", username, error_for(AuthField::Username)))
        .child(field(k, "auth-password", "Password", password, error_for(AuthField::Password)));
    if let Some(msg) = error_for(AuthField::Form) {
        card = card.child(row().key("auth-form-error").w_full().child(text(msg.to_owned()).font(type_::at(k, type_::CONTROL)).color(t.danger.color()).wrap().w_full()));
    }
    card = card.child(button).child(server_line(a, k));

    col().key("auth").w_full().h_full().center().fill(t.bg.color()).child(card)
}

/// "Server: http://127.0.0.1:8080 · Change", which opens a field for another URL.
fn server_line(a: &App, k: &Kit) -> El {
    let t = theme_of(k);
    let auth = &a.state.auth;
    let faint = |s: String| text(s).font(type_::at(k, type_::CAPTION)).color(t.text_muted.color());
    let toggle = text(if auth.server_open { "Done" } else { "Change" })
        .font(type_::at(k, type_::CAPTION))
        .color(t.accent_text.color())
        .key("auth-server-toggle")
        .role("button")
        .label("Change server")
        .focusable(true)
        .hover(t.hover.color())
        .on_press(|a: &mut App| a.state.auth.server_open = !a.state.auth.server_open);
    let mut c = col().w_full().gap(8.0).child(
        row().w_full().gap(4.0).center_y().child(faint(format!("Server: {}", a.state.server_url()))).child(faint("\u{00b7}".into())).child(toggle),
    );
    if auth.server_open {
        let input = TextInput::new(&auth.server).placeholder("http://127.0.0.1:8080").width(None).large().label("Server URL").on_edit(server_edit).on_submit(|a: &mut App, _| a.state.auth.server_open = false);
        c = c.child(col().key("auth-server").w_full().child(input.build(k)));
    }
    c.child(el().h(0.0))
}
