//! The right-hand member panel. Its width springs open and closed (`motion-reflow`); the content
//! keeps its own fixed width inside a clip, so rows never squash while it moves.

use rune::{El, ScrollSpec, col, row, text};
use rune_kit::Kit;
use thehideout_design::components::{HEADER_H, MemberRow};
use thehideout_design::{motion, theme_of, type_};
use thehideout_api::User;

use crate::app::App;
use crate::state::Place;

pub const WIDTH: f32 = 232.0;

fn people(a: &App) -> Vec<User> {
    let s = &a.state;
    match s.ui.place {
        Place::Guild(g) => s.data.members.get(&g).map(|ids| ids.iter().filter_map(|i| s.data.users.get(i).cloned()).collect()).unwrap_or_default(),
        Place::Home => s.current_channel().and_then(|c| s.dm(c)).map(|d| d.participants.clone()).unwrap_or_default(),
    }
}

/// `allowed`: the window is wide enough to offer the panel at all.
pub fn build(a: &App, k: &Kit, allowed: bool) -> El {
    let t = theme_of(k);
    let open = a.state.ui.members_open && allowed;
    let owner = a.state.current_guild().map(|g| g.owner_id);
    let list = people(a);
    let mut rows = col().key("member-rows").w_full().gap(2.0).padding(8.0);
    for u in &list {
        rows = rows.child(MemberRow::new(u.id.to_string(), u.username.clone()).backdrop(t.surface.color()).owner(owner == Some(u.id)).build(k));
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
                .child(text(format!("Members \u{00b7} {}", list.len())).font(type_::CONTROL_STRONG).color(t.text_muted.color())),
        )
        .child(col().grow(1.0).min_h(0.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(rows));
    let inner = if open { inner } else { inner.inert().a11y_hidden() };
    col().key("members").h_full().w(if open { WIDTH } else { 0.0 }).clip_rounded(0.0).token_motion(motion::REFLOW).child(inner)
}
