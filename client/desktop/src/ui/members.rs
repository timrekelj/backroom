//! The right-hand member panel. Its width springs open and closed (`motion-reflow`); the content
//! keeps its own fixed width inside a clip, so rows never squash while it moves.
//!
//! In a server the members sit under a header per role (the ones that are somebody's highest, by
//! position, most senior first), then "Members" for those with no role. A name takes the colour
//! of the member's highest role that has one, in the text variant of the palette.

use rune::{col, row, text, El, ScrollSpec};
use rune_kit::Kit;
use thehideout_api::User;
use thehideout_design::components::{MemberRow, HEADER_H};
use thehideout_design::{motion, palette_text, theme_of, type_};

use crate::app::App;
use crate::state::Place;

pub const WIDTH: f32 = 232.0;

fn people(a: &App) -> Vec<User> {
    let s = &a.state;
    match s.ui.place {
        Place::Guild(g) => s
            .data
            .members
            .get(&g)
            .map(|ids| {
                ids.iter()
                    .filter_map(|i| s.data.users.get(i).cloned())
                    .collect()
            })
            .unwrap_or_default(),
        Place::Home => s
            .current_channel()
            .and_then(|c| s.dm(c))
            .map(|d| d.participants.clone())
            .unwrap_or_default(),
    }
}

fn section_label(k: &Kit, label: String, key: String) -> El {
    let t = theme_of(k);
    row()
        .key(key)
        .w_full()
        .h(28.0)
        .px(8.0)
        .pt(8.0)
        .center_y()
        .child(
            text(label)
                .font(type_::LABEL)
                .color(t.text_muted.color())
                .max_lines(1),
        )
}

/// `allowed`: the window is wide enough to offer the panel at all.
pub fn build(a: &App, k: &Kit, allowed: bool) -> El {
    let t = theme_of(k);
    let s = &a.state;
    let open = s.ui.members_open && allowed;
    let owner = s.current_guild().map(|g| g.owner_id);
    let list = people(a);
    let guild = match s.ui.place {
        Place::Guild(g) => Some(g),
        Place::Home => None,
    };
    let member = |u: &User| {
        let ink = guild
            .and_then(|g| palette_text(s.name_color(g, u.id).as_key(), t))
            .map(|tone| tone.color());
        MemberRow::new(u.id.to_string(), u.username.clone())
            .backdrop(t.surface.color())
            .owner(owner == Some(u.id))
            .image(s.avatar_pixels(u.id))
            .name_color(ink)
            .build(k)
    };
    let mut rows = col().key("member-rows").w_full().gap(2.0).padding(8.0);
    match guild {
        Some(g) => {
            let groups = s.member_groups(g);
            let grouped = groups.iter().any(|x| x.role.is_some());
            for group in &groups {
                if grouped {
                    let (label, key) = match &group.role {
                        Some(r) => (
                            format!("{} \u{2014} {}", r.name, group.users.len()),
                            format!("members-role-{}", r.id),
                        ),
                        None => (
                            format!("Members \u{2014} {}", group.users.len()),
                            "members-rest".to_owned(),
                        ),
                    };
                    rows = rows.child(section_label(k, label, key));
                }
                for id in &group.users {
                    if let Some(u) = s.data.users.get(id) {
                        rows = rows.child(member(u));
                    }
                }
            }
        }
        None => {
            for u in &list {
                rows = rows.child(member(u));
            }
        }
    }
    let inner = col()
        .key("members-inner")
        .w(WIDTH)
        .h_full()
        .border_sides([0.0, 0.0, 0.0, 1.0], t.border.color())
        .child(
            row()
                .h(HEADER_H)
                .px(16.0)
                .center_y()
                .border_sides([0.0, 0.0, 1.0, 0.0], t.border.color())
                .child(
                    text(format!("Members \u{00b7} {}", list.len()))
                        .font(type_::CONTROL_STRONG)
                        .color(t.text_muted.color()),
                ),
        )
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(rows),
        );
    let inner = if open {
        inner
    } else {
        inner.inert().a11y_hidden()
    };
    col()
        .key("members")
        .h_full()
        .w(if open { WIDTH } else { 0.0 })
        .clip_rounded(0.0)
        .token_motion(motion::REFLOW)
        .child(inner)
}
