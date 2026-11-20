//! The dialogs: Create server, Join server, Create channel, Channel settings, Create / Rename /
//! Delete section, Invite and New message. The design `Dialog` supplies the modal, Esc and the
//! motion; these supply the content. Enter submits, the primary button says what it is doing
//! while it waits, and an error from the server (or from validation) shows under the field.

use rune::{col, icon, row, text, El, Key, TextEdit};
use rune_kit::{Kit, Select};
use thehideout_design::components::chat::pickers::row_key;
use thehideout_design::components::{
    dialog_field, Avatar, Button, ColorSwatches, Dialog, IconButton, IconGrid, PickerTabs, Size,
    TextInput, ToastKind, Variant, ROW_H,
};
use thehideout_design::icons::{catalog, CHEVRON_DOWN, COPY, HASH, SEARCH, X};
use thehideout_design::{palette_color, radius, theme_of, type_};

use crate::app::App;
use crate::state::{ChannelForm, DialogKind, DialogState, Phase};

fn dialog_edit(a: &mut App) -> &mut TextEdit {
    match &mut a.state.ui.dialog {
        Some(d) => &mut d.input,
        None => &mut a.state.ui.idle_composer,
    }
}

fn topic_edit(a: &mut App) -> &mut TextEdit {
    match a.state.ui.dialog.as_mut().and_then(|d| d.channel.as_mut()) {
        Some(f) => &mut f.topic,
        None => &mut a.state.ui.idle_composer,
    }
}

fn search_edit(a: &mut App) -> &mut TextEdit {
    match a.state.ui.dialog.as_mut().and_then(|d| d.channel.as_mut()) {
        Some(f) => &mut f.search,
        None => &mut a.state.ui.idle_composer,
    }
}

pub fn build(a: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(d) = &a.state.ui.dialog else { return };
    match d.kind {
        DialogKind::CreateGuild => form(
            k,
            layer,
            d,
            "Create server",
            "Give your server a name. You can change it later.",
            "Server name",
            "The Hideout",
            ("Create", "Creating\u{2026}"),
            false,
        ),
        DialogKind::JoinGuild => form(
            k,
            layer,
            d,
            "Join server",
            "Paste an invite code from a friend.",
            "Invite code",
            "HIDEOUT1",
            ("Join", "Joining\u{2026}"),
            false,
        ),
        DialogKind::CreateSection { .. } => form(
            k,
            layer,
            d,
            "Create section",
            "Sections group channels in the sidebar.",
            "Section name",
            "Projects",
            ("Create", "Creating\u{2026}"),
            false,
        ),
        DialogKind::RenameSection { .. } => form(
            k,
            layer,
            d,
            "Rename section",
            "Channels in it stay where they are.",
            "Section name",
            "Projects",
            ("Save", "Saving\u{2026}"),
            false,
        ),
        DialogKind::DeleteSection { section, .. } => delete_section(a, k, layer, d, section),
        DialogKind::CreateChannel { guild } => channel_dialog(
            a,
            k,
            layer,
            d,
            guild,
            "Create channel",
            "Channels keep a conversation in one place.",
            ("Create", "Creating\u{2026}"),
        ),
        DialogKind::ChannelSettings { guild, .. } => channel_dialog(
            a,
            k,
            layer,
            d,
            guild,
            "Edit channel",
            "Change how this channel looks and what it is about.",
            ("Save", "Saving\u{2026}"),
        ),
        DialogKind::NewDm => new_dm(a, k, layer, d),
        DialogKind::Invite { guild } => invite(a, k, layer, guild),
    }
}

fn close(a: &mut App) {
    a.act(|s| s.close_dialog());
}

fn footer(k: &Kit, d: &DialogState, labels: (&str, &str)) -> (El, El) {
    let busy = d.phase == Phase::Busy;
    let cancel = Button::new("Cancel")
        .variant(Variant::Secondary)
        .on_press(close)
        .build(k)
        .key("dialog-cancel");
    let submit = Button::new(if busy { labels.1 } else { labels.0 })
        .disabled(busy)
        .on_press(|a: &mut App| a.act(|s| s.submit_dialog()))
        .build(k)
        .key("dialog-submit");
    (cancel, submit)
}

#[allow(clippy::too_many_arguments)]
fn form(
    k: &Kit,
    layer: &mut Vec<El>,
    d: &DialogState,
    title: &str,
    description: &str,
    label: &str,
    placeholder: &str,
    labels: (&str, &str),
    hash: bool,
) {
    let mut input = TextInput::new(&d.input)
        .placeholder(placeholder)
        .on_edit(dialog_edit)
        .on_submit(|a: &mut App, _| a.act(|s| s.submit_dialog()));
    if hash {
        input = input.leading_icon(&HASH);
    }
    let (cancel, submit) = footer(k, d, labels);
    Dialog::new("dlg", title)
        .description(description)
        .open(true)
        .child(col().key("dialog-field").w_full().child(dialog_field(
            k,
            label,
            input,
            d.error.as_deref(),
        )))
        .footer(cancel)
        .footer(submit)
        .on_close(close)
        .build(k, layer);
}

fn delete_section(a: &App, k: &Kit, layer: &mut Vec<El>, d: &DialogState, section: u64) {
    let guild = match d.kind {
        DialogKind::DeleteSection { guild, .. } => guild,
        _ => return,
    };
    let name = a
        .state
        .layout(guild)
        .and_then(|l| l.sections.iter().find(|s| s.id == section))
        .map_or_else(String::new, |s| s.name.clone());
    let busy = d.phase == Phase::Busy;
    let t = theme_of(k);
    let mut body = col().key("dialog-confirm").w_full().gap(6.0);
    if let Some(e) = &d.error {
        body = body.child(
            text(e.clone())
                .font(type_::CONTROL)
                .color(t.danger.color())
                .wrap()
                .w_full(),
        );
    }
    Dialog::new("dlg", "Delete section")
        .description(format!("\u{201c}{name}\u{201d} will be removed. Its channels move to the top of the list; nothing is deleted from them."))
        .open(true)
        .child(body)
        .footer(Button::new("Cancel").variant(Variant::Secondary).on_press(close).build(k).key("dialog-cancel"))
        .footer(
            Button::new(if busy { "Deleting\u{2026}" } else { "Delete section" })
                .variant(Variant::Danger)
                .disabled(busy)
                .on_press(|a: &mut App| a.act(|s| s.submit_dialog()))
                .build(k)
                .key("dialog-submit"),
        )
        .on_close(close)
        .build(k, layer);
}

// ---- channel create / settings ---------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn channel_dialog(
    a: &App,
    k: &Kit,
    layer: &mut Vec<El>,
    d: &DialogState,
    guild: u64,
    title: &str,
    description: &str,
    labels: (&str, &str),
) {
    let Some(f) = &d.channel else { return };
    let t = theme_of(k);
    let creating = matches!(d.kind, DialogKind::CreateChannel { .. });
    let name = TextInput::new(&d.input)
        .placeholder("plans")
        .leading_icon(&HASH)
        .on_edit(dialog_edit)
        .on_submit(|a: &mut App, _| a.act(|s| s.submit_dialog()));
    let mut body = col()
        .key("dialog-field")
        .w_full()
        .gap(16.0)
        .child(dialog_field(k, "Channel name", name, d.error.as_deref()));
    if f.picker_open {
        body = body.child(icon_row(k, f)).child(picker(k, f));
    } else {
        if !creating {
            let topic = TextInput::new(&f.topic)
                .placeholder("What is this channel about?")
                .on_edit(topic_edit)
                .on_submit(|a: &mut App, _| a.act(|s| s.submit_dialog()));
            body = body.child(dialog_field(k, "Topic", topic, None));
        }
        if creating {
            body = body.child(section_field(a, k, layer, f, guild));
        }
        body = body.child(icon_row(k, f)).child(
            col()
                .key("color-field")
                .w_full()
                .gap(6.0)
                .child(
                    text("Colour")
                        .font(type_::LABEL)
                        .color(t.text_muted.color()),
                )
                .child(
                    ColorSwatches::new(f.color)
                        .on_pick(|a: &mut App, key: &'static str| a.act(|s| s.pick_color(key)))
                        .build(k),
                ),
        );
    }
    let (cancel, submit) = footer(k, d, labels);
    Dialog::new("dlg", title)
        .description(description)
        .open(true)
        .child(body)
        .footer(cancel)
        .footer(submit)
        .on_close(close)
        .build(k, layer);
}

/// "Icon": a field-looking button showing the chosen icon; pressing it opens the picker.
fn icon_row(k: &Kit, f: &ChannelForm) -> El {
    let t = theme_of(k);
    let glyph = catalog::by_name(&f.icon).unwrap_or(&thehideout_design::icons::HASH);
    let tone = palette_color(f.color, t).unwrap_or(t.text_muted);
    let label = if f.icon.is_empty() {
        "Default".to_owned()
    } else {
        f.icon.replace('-', " ")
    };
    let field = row()
        .key("icon-field")
        .role("button")
        .label(format!("Icon: {label}"))
        .expanded(f.picker_open)
        .grow(1.0)
        .h(32.0)
        .px(12.0)
        .gap(10.0)
        .center_y()
        .radius(radius::MD)
        .fill(t.surface_raised.color())
        .border(
            1.0,
            if f.picker_open {
                k.c.border_focus
            } else {
                t.border_strong.color()
            },
        )
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .hover(t.hover.color())
        .press(t.pressed.color())
        .on_press(|a: &mut App| a.act(|s| s.toggle_picker()))
        .child(
            icon(glyph, thehideout_design::icon::SM)
                .color(tone.color())
                .a11y_hidden(),
        )
        .child(
            text(label)
                .font(type_::CONTROL)
                .color(t.text.color())
                .grow(1.0)
                .max_lines(1)
                .a11y_hidden(),
        )
        .child(
            icon(&CHEVRON_DOWN, thehideout_design::icon::SM)
                .color(t.text_muted.color())
                .a11y_hidden(),
        );
    let mut line = row()
        .key("icon-line")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(field);
    if !f.icon.is_empty() {
        line = line.child(
            IconButton::new(&X, "Use the default icon")
                .size(Size::Md)
                .on_press(|a: &mut App| a.act(|s| s.pick_icon("")))
                .build(k),
        );
    }
    col()
        .key("icon-field-wrap")
        .w_full()
        .gap(6.0)
        .child(text("Icon").font(type_::LABEL).color(t.text_muted.color()))
        .child(line)
}

/// Tabs, search and the grid. Only Icons is populated; Emoji and Server emoji are coming.
fn picker(k: &Kit, f: &ChannelForm) -> El {
    let search = TextInput::new(&f.search)
        .placeholder("Search icons")
        .label("Search icons")
        .leading_icon(&SEARCH)
        .large()
        .width(None)
        .on_edit(search_edit)
        .on_change(|a: &mut App, _tx| {
            a.state
                .channel_form()
                .map(ChannelForm::research)
                .unwrap_or_default()
        })
        .on_submit(|a: &mut App, _| a.act(|s| s.pick_highlighted()))
        .on_key(|a: &mut App, ev, tx| {
            let (dx, dy) = match ev.key {
                Key::Left => (-1, 0),
                Key::Right => (1, 0),
                Key::Up => (0, -1),
                Key::Down => (0, 1),
                _ => return false,
            };
            if let Some(f) = a.state.channel_form() {
                f.step_highlight(dx, dy);
                if let Some(h) = f.highlight {
                    tx.scroll_into_view(row_key(
                        thehideout_design::components::chat::pickers::row_of(h),
                    ));
                }
            }
            true
        });
    col()
        .key("icon-picker")
        .w_full()
        .gap(8.0)
        .child(
            PickerTabs::new(
                &[("Icons", true), ("Emoji", false), ("Server emoji", false)],
                f.tab,
            )
            .on_pick(|a: &mut App, i: usize| {
                a.act(|s| {
                    if let Some(f) = s.channel_form() {
                        f.tab = i;
                    }
                })
            })
            .build(k),
        )
        .child(search.build(k))
        .child(
            IconGrid::new(f.results.clone(), f.icon.clone())
                .highlight(f.highlight)
                .on_pick(|a: &mut App, name: &'static str| a.act(|s| s.pick_icon(name)))
                .build::<App>(k),
        )
}

fn section_field(a: &App, k: &Kit, layer: &mut Vec<El>, f: &ChannelForm, guild: u64) -> El {
    let t = theme_of(k);
    let sections: Vec<(u64, String)> = a
        .state
        .layout(guild)
        .map(|l| l.sections.iter().map(|s| (s.id, s.name.clone())).collect())
        .unwrap_or_default();
    let mut labels = vec!["Top level".to_owned()];
    labels.extend(sections.iter().map(|(_, n)| n.clone()));
    let current = f
        .section
        .and_then(|id| sections.iter().find(|(s, _)| *s == id))
        .map_or("Top level", |(_, n)| n.as_str());
    let by_label = sections.clone();
    let select = Select::new("section-select", &labels)
        .width(392.0)
        .label("Section")
        .current(Some(current))
        .open(f.section_open)
        .on_toggle(|a: &mut App| {
            if let Some(f) = a.state.channel_form() {
                f.section_open = !f.section_open;
            }
        })
        .on_close(|a: &mut App| {
            if let Some(f) = a.state.channel_form() {
                f.section_open = false;
            }
        })
        .on_choose(move |a: &mut App, label: &str, _tx| {
            let chosen = by_label.iter().find(|(_, n)| n == label).map(|(id, _)| *id);
            if let Some(f) = a.state.channel_form() {
                f.section = chosen;
                f.section_open = false;
            }
        });
    col()
        .key("section-field")
        .w_full()
        .gap(6.0)
        .child(
            text("Section")
                .font(type_::LABEL)
                .color(t.text_muted.color()),
        )
        .child(select.build(k, layer))
}

// ---- the rest --------------------------------------------------------------------------------

fn new_dm(a: &App, k: &Kit, layer: &mut Vec<El>, d: &DialogState) {
    let t = theme_of(k);
    let typed = d.input.text.trim().trim_start_matches('@').to_lowercase();
    let input = TextInput::new(&d.input)
        .placeholder("ana")
        .leading_icon(&thehideout_design::icons::AT)
        .on_edit(dialog_edit)
        .on_submit(|a: &mut App, _| a.act(|s| s.submit_dialog()));
    let mut people = col().key("dialog-people").w_full().gap(2.0);
    for u in a
        .state
        .known_users()
        .into_iter()
        .filter(|u| u.username.to_lowercase().contains(&typed))
        .take(6)
    {
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
                .child(
                    Avatar::new(u.id.to_string(), u.username.clone())
                        .size(24)
                        .backdrop(t.surface_overlay.color())
                        .build(k),
                )
                .child(
                    text(u.username.clone())
                        .font(type_::CONTROL)
                        .color(t.text.color())
                        .a11y_hidden(),
                ),
        );
    }
    let (cancel, submit) = footer(k, d, ("Open", "Opening\u{2026}"));
    Dialog::new("dlg", "New message")
        .description("Start a conversation with someone from your servers.")
        .open(true)
        .child(col().key("dialog-field").w_full().child(dialog_field(
            k,
            "Username",
            input,
            d.error.as_deref(),
        )))
        .child(people)
        .footer(cancel)
        .footer(submit)
        .on_close(close)
        .build(k, layer);
}

fn invite(a: &App, k: &Kit, layer: &mut Vec<El>, guild: u64) {
    let t = theme_of(k);
    let Some(g) = a.state.guild(guild) else {
        return;
    };
    let code = g.invite_code.clone();
    let copy = Button::new("Copy")
        .icon(&COPY)
        .variant(Variant::Secondary)
        .build(k)
        .key("invite-copy")
        .on_press_tx(move |a: &mut App, tx| {
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
        .child(
            text(g.invite_code.clone())
                .font(type_::CODE)
                .color(t.text.color())
                .a11y_hidden(),
        );
    Dialog::new("dlg", "Invite people")
        .description(format!("Anyone with this code can join {}.", g.name))
        .open(true)
        .child(
            row()
                .key("invite-row")
                .w_full()
                .gap(8.0)
                .center_y()
                .child(shown)
                .child(copy),
        )
        .footer(
            Button::new("Done")
                .on_press(close)
                .build(k)
                .key("dialog-submit"),
        )
        .on_close(close)
        .build(k, layer);
}
