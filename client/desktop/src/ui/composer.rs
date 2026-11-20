//! The message box under the transcript: the design `Composer` wired to the open channel's
//! draft, plus the character counter that appears near the limit. Enter sends (Shift+Enter breaks
//! the line); while the gateway is down the box shows why it can't.

use rune::{El, TextEdit, Tx, col, el, row, text};
use rune_kit::Kit;
use thehideout_api::MAX_MESSAGE_CHARS;
use thehideout_design::components::{Composer, FOOTER_H};
use thehideout_design::{theme_of, type_};

use crate::app::App;
use crate::state::Place;

/// The counter shows once this few characters remain.
const COUNTER_FROM: usize = 500;

fn draft(a: &mut App) -> &mut TextEdit {
    a.state.composer_mut()
}

fn send(a: &mut App, text: String, tx: &mut Tx) {
    if !a.state.send(&text) {
        // Over the limit: the field was cleared to send, so give the text back.
        a.state.composer_mut().replace_text(&text);
        return;
    }
    a.flush();
    if let Some(c) = a.state.current_channel() {
        // Your own message always brings you to the end.
        tx.scroll_to_bottom(format!("tx-{c}"));
    }
}

pub fn build(a: &App, k: &Kit) -> El {
    let t = theme_of(k);
    let s = &a.state;
    let Some(name) = s.current_channel_name() else { return el().h(0.0) };
    let placeholder = match s.ui.place {
        Place::Home => format!("Message @{name}"),
        Place::Guild(_) => format!("Message #{name}"),
    };
    let mut composer = Composer::new(s.composer()).key("composer").placeholder(placeholder).bind(draft, send);
    if let Some(reason) = s.composer_block() {
        composer = composer.disabled_reason(reason);
    }
    let used = s.composer().text.chars().count();
    // At rest exactly `FOOTER_H`, the height of the sidebar's user strip, so the hairline above
    // them is one line across the window; a taller (multi-line) composer pushes it up.
    let mut c = col()
        .key("composer-area")
        .w_full()
        .min_h(FOOTER_H)
        .px(16.0)
        .py(12.0)
        .gap(4.0)
        .justify_center()
        .border_sides([1.0, 0.0, 0.0, 0.0], t.border.color());
    if used + COUNTER_FROM >= MAX_MESSAGE_CHARS {
        let over = used > MAX_MESSAGE_CHARS;
        let ink = if over { t.danger } else { t.text_muted };
        c = c.child(row().key("counter").w_full().justify_end().child(text(format!("{used} / {MAX_MESSAGE_CHARS}")).font(type_::CAPTION).color(ink.color())));
    }
    c.child(composer.build(k))
}
