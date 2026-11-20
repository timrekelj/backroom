//! The toast layer: the front of the queue at the bottom centre, on its own inert layer so it
//! never blocks the window. Each toast has its own key, so the next one enters rather than
//! swapping text in place.
//!
//! Where a channel is open the toast sits at the bottom of the transcript, above the composer
//! (`over_transcript`), so it never covers the input. Screens without a composer (auth, Home
//! with no conversation) keep it at the window's bottom centre (`build`).

use rune::{col, el, El};
use rune_kit::Kit;
use thehideout_design::components::Toast;

use crate::app::App;
use crate::state::Screen;

/// Space between a toast and what is below it.
const GAP: f32 = 16.0;
/// Extra room while the "New messages" pill sits at the bottom of the transcript.
const PILL: f32 = 40.0;

/// Whether the shell shows a composer, so toasts go over the transcript instead.
fn above_composer(a: &App) -> bool {
    a.state.screen == Screen::Main && a.state.current_channel().is_some()
}

fn host(a: &App, k: &Kit, bottom: f32) -> El {
    let host = col()
        .key("toast-host")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .justify_end()
        .items_center()
        .pb(bottom)
        .inert()
        .a11y_hidden();
    match a.state.ui.toasts.front() {
        Some(t) => host.child(
            Toast::new(t.kind, t.message.clone())
                .build(k)
                .key(format!("toast-{}", t.id)),
        ),
        None => host,
    }
}

/// The window-level layer: bottom centre, for screens without a composer.
pub fn build(a: &App, k: &Kit) -> El {
    if above_composer(a) {
        return el()
            .key("toast-none")
            .absolute(0.0, 0.0)
            .inert()
            .a11y_hidden();
    }
    host(a, k, 24.0)
}

/// The layer inside the transcript area: its bottom edge is the composer's top edge.
pub fn over_transcript(a: &App, k: &Kit, pill: bool) -> Option<El> {
    above_composer(a).then(|| host(a, k, GAP + if pill { PILL } else { 0.0 }))
}
