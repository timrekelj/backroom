//! Unread markers. An unread dot is quiet (the text colour, 6pt); a mention pill is loud (accent
//! fill, count in tabular figures, `99+` cap). Red is reserved for errors.

use rune::{El, SpringPair, TextStyle, Transition, el, row, text};
use rune_kit::Kit;

use crate::{metrics_of, theme_of};
use crate::tokens::{motion, radius};

pub struct UnreadDot;

impl UnreadDot {
    pub fn build(k: &Kit) -> El {
        let t = theme_of(k);
        el().role("status")
            .label("unread")
            .size(6.0, 6.0)
            .radius(radius::FULL)
            .fill(t.text.color())
    }
}

/// The entrance of a mention count: opacity and a small scale up on `motion::POP`. The one
/// bouncing motion in the chat components, because a mention is the one thing that asks for you.
/// Only a badge that mounts in an input-caused or `animated` commit plays it.
pub fn pop_in() -> Transition {
    Transition::scale(0.6)
        .combined(Transition::opacity())
        .springs(SpringPair::new(motion::POP, motion::CONTROL))
}

pub struct MentionPill {
    count: u32,
}

impl MentionPill {
    pub fn new(count: u32) -> MentionPill {
        MentionPill { count }
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let shown = if self.count > 99 {
            "99+".to_owned()
        } else {
            self.count.to_string()
        };
        let m = metrics_of(k);
        let font = m.font(TextStyle {
            size: 11.0,
            weight: 650.0,
            line_height: 14.0,
            tracking: 0.0,
            features: "'tnum' 1",
            ..crate::tokens::type_::CAPTION
        });
        row()
            .role("status")
            .label(if self.count == 1 { "1 mention".to_owned() } else { format!("{} mentions", self.count) })
            .h(m.em(16.0))
            .min_w(m.em(16.0))
            .px(5.0)
            .center()
            .radius(radius::FULL)
            .fill(t.accent.color())
            .transition(pop_in())
            .child(
                text(shown)
                    .font(font)
                    .color(t.accent_fg.color())
                    .a11y_hidden(),
            )
    }
}
