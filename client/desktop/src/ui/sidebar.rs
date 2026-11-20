//! The sidebar: the server's name (a menu), its channels, or Home's conversations, and the user
//! strip. Switching server crossfades the body (`motion-control`); the selection highlight
//! travels between rows of one list (`motion-reflow`, from the components).

use rune::{col, icon, row, text, El, ScrollSpec, SpringPair, Transition};
use rune_kit::{Kit, Menu, MenuItem};
use thehideout_design::components::{
    Avatar, ChannelRow, ConnectionPill, ConnectionStatus, IconButton, NavRow, Presence, SidebarSection,
    FOOTER_H, HEADER_H,
};
use thehideout_design::icons::{CHEVRON_DOWN, GEAR, PLUS, SIGN_OUT, USER_PLUS};
use thehideout_design::{icon as size, motion, theme_of, type_};

use crate::app::App;
use crate::state::{DialogKind, Menu as MenuState, Place};

/// `width` comes from the window (`shell::Layout`): 240 when there is room, 208 when not.
pub fn build(a: &App, k: &Kit, layer: &mut Vec<El>, width: f32) -> El {
    let t = theme_of(k);
    let place = match a.state.ui.place {
        Place::Home => "home".to_owned(),
        Place::Guild(g) => format!("guild-{g}"),
    };
    let fade = Transition::opacity().springs(SpringPair::new(motion::CONTROL, motion::CONTROL));
    let content = match a.state.ui.place {
        Place::Home => home_rows(a, k),
        Place::Guild(g) => channel_rows(a, k, g),
    };
    let body = col()
        .key("sidebar-body")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .child(
            col()
                .key(format!("sidebar-{place}"))
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .transition(fade)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(content),
        );
    col()
        .key("sidebar")
        .w(width)
        .h_full()
        .fill(t.bg.color())
        .child(header(a, k, layer, width))
        .child(body)
        .child(user_strip(a, k, layer))
}

fn header(a: &App, k: &Kit, layer: &mut Vec<El>, width: f32) -> El {
    let t = theme_of(k);
    let line = |e: El| e.border_sides([0.0, 0.0, 1.0, 0.0], t.border.color());
    let Some(g) = a.state.current_guild() else {
        return line(
            row()
                .key("sidebar-header")
                .w_full()
                .h(HEADER_H)
                .px(16.0)
                .center_y()
                .child(
                    text("Direct messages")
                        .font(type_::TITLE)
                        .color(t.text.color()),
                ),
        );
    };
    let (id, owner) = (g.id, g.owner_id == a.state.my_id());
    let trigger = row()
        .key("guild-header")
        .role("button")
        .label(format!("{}, server menu", g.name))
        .w(width)
        .h(HEADER_H)
        .px(16.0)
        .gap(8.0)
        .center_y()
        .focusable(true)
        .focus_ring(2.0, -2.0, k.c.border_focus)
        .hover(t.hover.color())
        .on_press(|a: &mut App| {
            a.state.ui.menu = if a.state.ui.menu == MenuState::Guild {
                MenuState::None
            } else {
                MenuState::Guild
            }
        })
        .child(
            text(g.name.clone())
                .font(type_::TITLE)
                .color(t.text.color())
                .max_lines(1)
                .grow(1.0)
                .shrink()
                .a11y_hidden(),
        )
        .child(
            icon(&CHEVRON_DOWN, size::SM)
                .color(t.text_muted.color())
                .a11y_hidden(),
        );
    let menu = Menu::new("guild-menu")
        .trigger(trigger)
        .item(
            MenuItem::new("Invite people")
                .icon(&USER_PLUS)
                .on_select(move |a: &mut App| {
                    a.act(|s| s.open_dialog(DialogKind::Invite { guild: id }))
                }),
        )
        .item(
            MenuItem::new("Create channel")
                .icon(&PLUS)
                .disabled(!owner)
                .on_select(move |a: &mut App| {
                    a.act(|s| s.open_dialog(DialogKind::CreateChannel { guild: id }))
                }),
        )
        .open(a.state.ui.menu == MenuState::Guild)
        .min_width(width - 16.0)
        .on_close(|a: &mut App| a.state.ui.menu = MenuState::None)
        .build(k, layer);
    line(row().w_full().child(menu))
}

fn channel_rows(a: &App, k: &Kit, guild: u64) -> El {
    let s = &a.state;
    let owner = s.guild(guild).is_some_and(|g| g.owner_id == s.my_id());
    let group = format!("channels-{guild}");
    let mut list = SidebarSection::new("Text channels", s.ui.channels_open)
        .on_toggle(|a: &mut App| a.state.ui.channels_open = !a.state.ui.channels_open);
    if owner {
        list = list.add_label("Create channel").on_add(move |a: &mut App| {
            a.act(|s| s.open_dialog(DialogKind::CreateChannel { guild }))
        });
    }
    for c in s.channels_of(guild) {
        let u = s.unread_of(c.id);
        let id = c.id;
        list = list.child(
            ChannelRow::new(c.name.clone())
                .highlight_group(group.clone())
                .selected(s.current_channel() == Some(id))
                .unread(u.count > 0)
                .mentions(u.mentions)
                .on_press(move |a: &mut App| a.act(|s| s.select_channel(id)))
                .build(k),
        );
    }
    col()
        .key("channel-list")
        .w_full()
        .padding(8.0)
        .gap(8.0)
        .child(list.build(k))
}

fn home_rows(a: &App, k: &Kit) -> El {
    let t = theme_of(k);
    let s = &a.state;
    let mut list = SidebarSection::new("Direct messages", s.ui.dms_open)
        .on_toggle(|a: &mut App| a.state.ui.dms_open = !a.state.ui.dms_open);
    for dm in &s.data.dms {
        let name = s
            .dm_peer(dm)
            .map_or_else(|| "Unknown".to_owned(), |u| u.username.clone());
        let u = s.unread_of(dm.id);
        let id = dm.id;
        list = list.child(
            NavRow::person(name, dm_peer_id(s, dm))
                .highlight_group("dms")
                .selected(s.current_channel() == Some(id))
                .unread(u.count > 0)
                .mentions(u.mentions)
                .on_press(move |a: &mut App| a.act(|s| s.select_channel(id)))
                .build(k),
        );
    }
    let new_dm = NavRow::new("New message", &PLUS, &PLUS)
        .highlight_group("home-actions")
        .on_press(|a: &mut App| a.act(|s| s.open_dialog(DialogKind::NewDm)))
        .build(k)
        .key("new-dm");
    let mut c = col()
        .key("dm-list")
        .w_full()
        .padding(8.0)
        .gap(8.0)
        .child(new_dm)
        .child(list.build(k));
    if s.data.dms.is_empty() {
        c = c.child(
            text("No conversations yet.")
                .font(type_::CONTROL)
                .color(t.text_faint.color())
                .px(8.0),
        );
    }
    c
}

/// The id that colours the other person's avatar (the same one the transcript uses).
fn dm_peer_id(s: &crate::state::AppState, dm: &thehideout_api::DmChannel) -> String {
    s.dm_peer(dm).map_or_else(String::new, |u| u.id.to_string())
}

/// The strip under the sidebar: your avatar (its dot is the connection), your name, and a gear
/// menu. `FOOTER_H` tall, like the composer zone, so the hairlines above them line up. The
/// connection pill appears only while something is wrong.
fn user_strip(a: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let t = theme_of(k);
    let me = a.state.me();
    let name = me.map_or_else(String::new, |u| u.username.clone());
    let id = me.map_or_else(String::new, |u| u.id.to_string());
    let status = a.state.ui.connection;
    let presence = match status {
        ConnectionStatus::Connected => Presence::Online,
        ConnectionStatus::Connecting | ConnectionStatus::Reconnecting(_) => Presence::Idle,
        ConnectionStatus::Offline => Presence::Offline,
    };
    let gear = IconButton::new(&GEAR, "Account menu")
        .selected(a.state.ui.menu == MenuState::User)
        .on_press(|a: &mut App| {
            a.state.ui.menu = if a.state.ui.menu == MenuState::User {
                MenuState::None
            } else {
                MenuState::User
            }
        })
        .build(k)
        .key("user-menu-button");
    let menu = Menu::new("user-menu")
        .trigger(gear)
        .item(
            MenuItem::new("Settings\u{2026}")
                .icon(&GEAR)
                .on_select(|a: &mut App| a.act(|s| s.open_settings())),
        )
        .item(
            MenuItem::new("Sign out")
                .icon(&SIGN_OUT)
                .on_select(|a: &mut App| a.act(|s| s.sign_out())),
        )
        .open(a.state.ui.menu == MenuState::User)
        .placement(rune_kit::Placement::TopEnd)
        .min_width(160.0)
        .on_close(|a: &mut App| a.state.ui.menu = MenuState::None)
        .build(k, layer);
    let mut who = col().key("who").grow(1.0).shrink().min_w(0.0).gap(2.0).child(
        text(name.clone())
            .font(type_::CONTROL_STRONG)
            .color(t.text.color())
            .max_lines(1),
    );
    if status != ConnectionStatus::Connected {
        who = who.child(row().key("connection").child(ConnectionPill::new(status).build(k)));
    }
    row()
        .key("user-strip")
        .w_full()
        .h(FOOTER_H)
        .px(12.0)
        .gap(10.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border.color())
        .child(
            Avatar::new(id, name)
                .size(32)
                .presence(presence)
                .backdrop(t.bg.color())
                .build(k),
        )
        .child(who)
        .child(menu)
}
