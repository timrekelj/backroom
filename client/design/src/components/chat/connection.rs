//! ConnectionPill: the connection status as a small pill. Connected (success dot), Connecting,
//! Reconnecting in Ns (warning), Offline (danger). Colours move between states on
//! `motion::CONTROL`; the pill stays one element so they animate rather than swap.

use rune::{El, el, row, text};
use rune_kit::Kit;

use crate::theme_of;
use crate::tokens::{motion, radius, type_};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    /// Seconds until the next attempt.
    Reconnecting(u32),
    Offline,
}

impl ConnectionStatus {
    pub fn label(self) -> String {
        match self {
            ConnectionStatus::Connected => "Connected".into(),
            ConnectionStatus::Connecting => "Connecting\u{2026}".into(),
            ConnectionStatus::Reconnecting(s) => format!("Reconnecting in {s}s"),
            ConnectionStatus::Offline => "Offline".into(),
        }
    }
}

pub struct ConnectionPill {
    status: ConnectionStatus,
}

impl ConnectionPill {
    pub fn new(status: ConnectionStatus) -> ConnectionPill {
        ConnectionPill { status }
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let (ink, bg) = match self.status {
            ConnectionStatus::Connected => (t.success, t.success_bg),
            ConnectionStatus::Connecting => (t.text_muted, t.surface_raised),
            ConnectionStatus::Reconnecting(_) => (t.warning, t.warning_bg),
            ConnectionStatus::Offline => (t.danger, t.danger_bg),
        };
        let label = self.status.label();
        row()
            .role("status")
            .label(label.clone())
            .h(crate::metrics_of(k).em(22.0))
            .px(8.0)
            .gap(6.0)
            .center_y()
            .radius(radius::SM)
            .fill(bg.color())
            .token_motion(motion::CONTROL)
            .child(
                el().key("dot")
                    .size(6.0, 6.0)
                    .radius(radius::FULL)
                    .fill(ink.color()),
            )
            .child(
                text(label)
                    .key("label")
                    .font(type_::at(k, type_::LABEL))
                    .color(ink.color())
                    .a11y_hidden(),
            )
    }
}
