//! The five dialogs: Create server, Join server, Create channel, Invite and New message. The
//! design `Dialog` supplies the modal, Esc and the motion; these supply the content. Enter
//! submits, the primary button says what it is doing while it waits, and an error from the
//! server (or from validation) shows under the field.

use rune::{El, TextEdit, col, row, text};
use rune_kit::Kit;
use thehideout_design::components::{Avatar, Button, Dialog, ROW_H, TextInput, Variant, dialog_field};
use thehideout_design::icons::{COPY, HASH};
use thehideout_design::components::ToastKind;
use thehideout_design::{radius, theme_of, type_};

use crate::app::App;
use crate::state::{DialogKind, DialogState, Phase};

fn dialog_edit(a: &mut App) -> &mut TextEdit {
    match &mut a.state.ui.dialog {
        Some(d) => &mut d.input,
        None => &mut a.state.ui.idle_composer,
    }
}

pub fn build(a: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(d) = &a.state.ui.dialog else { return };
    match d.kind {
        DialogKind::CreateGuild => form(k, layer, d, "Create server", "Give your server a name. You can change it later.", "Server name", "The Hideout", ("Create", "Creating\u{2026}"), false),
        DialogKind::JoinGuild => form(k, layer, d, "Join server", "Paste an invite code from a friend.", "Invite code", "HIDEOUT1", ("Join", "Joining\u{2026}"), false),
        DialogKind::CreateChannel { .. } => form(k, layer, d, "Create channel", "Channels keep a conversation in one place.", "Channel name", "plans", ("Create", "Creating\u{2026}"), true),
        DialogKind::NewDm => new_dm(a, k, layer, d),
        DialogKind::Invite { guild } => invite(a, k, layer, guild),
    }
}

fn close(a: &mut App) {
    a.act(|s| s.close_dialog());
}

fn footer(k: &Kit, d: &DialogState, labels: (&str, &str)) -> (El, El) {
    let busy = d.phase == Phase::Busy;
    let cancel = Button::new("Cancel").variant(Variant::Secondary).on_press(close).build(k).key("dialog-cancel");
    let submit = Button::new(if busy { labels.1 } else { labels.0 }).disabled(busy).on_press(|a: &mut App| a.act(|s| s.submit_dialog())).build(k).key("dialog-submit");
    (cancel, submit)
}

#[allow(clippy::too_many_arguments)]
fn form(k: &Kit, layer: &mut Vec<El>, d: &DialogState, title: &str, description: &str, label: &str, placeholder: &str, labels: (&str, &str), hash: bool) {
    let mut input = TextInput::new(&d.input).placeholder(placeholder).on_edit(dialog_edit).on_submit(|a: &mut App, _| a.act(|s| s.submit_dialog()));
    if hash {
        input = input.leading_icon(&HASH);
    }
    let (cancel, submit) = footer(k, d, labels);
    Dialog::new("dlg", title)
        .description(description)
        .open(true)
        .child(col().key("dialog-field").w_full().child(dialog_field(k, label, input, d.error.as_deref())))
        .footer(cancel)
        .footer(submit)
        .on_close(close)
        .build(k, layer);
}

fn new_dm(a: &App, k: &Kit, layer: &mut Vec<El>, d: &DialogState) {
    let t = theme_of(k);
    let typed = d.input.text.trim().trim_start_matches('@').to_lowercase();
    let input = TextInput::new(&d.input).placeholder("ana").leading_icon(&thehideout_design::icons::AT).on_edit(dialog_edit).on_submit(|a: &mut App, _| a.act(|s| s.submit_dialog()));
    let mut people = col().key("dialog-people").w_full().gap(2.0);
    for u in a.state.known_users().into_iter().filter(|u| u.username.to_lowercase().contains(&typed)).take(6) {
        let name = u.username.clone();
        people = people.child(
            row()
                .key(format!("dm-user-{}", u.id))
                .role("button")
                .label(name.clone())
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(10.0)
                .center_y()
                .radius(radius::MD)
                .focusable(true)
                .focus_ring(2.0, 0.0, k.c.border_focus)
                .hover(t.hover.color())
                .press(t.pressed.color())
                .on_press(move |a: &mut App| {
                    let name = name.clone();
                    a.act(|s| {
                        if let Some(dlg) = &mut s.ui.dialog {
                            dlg.input.replace_text(name);
                        }
                        s.submit_dialog();
                    });
                })
                .child(Avatar::new(u.id.to_string(), u.username.clone()).size(24).backdrop(t.surface_overlay.color()).build(k))
                .child(text(u.username.clone()).font(type_::CONTROL).color(t.text.color()).a11y_hidden()),
        );
    }
    let (cancel, submit) = footer(k, d, ("Open", "Opening\u{2026}"));
    Dialog::new("dlg", "New message")
        .description("Start a conversation with someone from your servers.")
        .open(true)
        .child(col().key("dialog-field").w_full().child(dialog_field(k, "Username", input, d.error.as_deref())))
        .child(people)
        .footer(cancel)
        .footer(submit)
        .on_close(close)
        .build(k, layer);
}

fn invite(a: &App, k: &Kit, layer: &mut Vec<El>, guild: u64) {
    let t = theme_of(k);
    let Some(g) = a.state.guild(guild) else { return };
    let code = g.invite_code.clone();
    let copy = Button::new("Copy").icon(&COPY).variant(Variant::Secondary).build(k).key("invite-copy").on_press_tx(move |a: &mut App, tx| {
        tx.copy(code.clone());
        a.state.toast(ToastKind::Success, "Invite code copied");
    });
    let shown = row()
        .key("invite-code")
        .grow(1.0)
        .h(32.0)
        .px(12.0)
        .center_y()
        .radius(radius::MD)
        .fill(t.surface_raised.color())
        .border(1.0, t.border_strong.color())
        .label(format!("Invite code {}", g.invite_code))
        .child(text(g.invite_code.clone()).font(type_::CODE).color(t.text.color()).a11y_hidden());
    Dialog::new("dlg", "Invite people")
        .description(format!("Anyone with this code can join {}.", g.name))
        .open(true)
        .child(row().key("invite-row").w_full().gap(8.0).center_y().child(shown).child(copy))
        .footer(Button::new("Done").on_press(close).build(k).key("dialog-submit"))
        .on_close(close)
        .build(k, layer);
}
