//! Transcript dividers. [`DateDivider`]: a hairline with a centred date ("Today", "Yesterday",
//! "Mon, Nov 17"); the caller formats it. [`NewMessagesDivider`]: an accent line with a "New"
//! tag at its right end. Neither animates.

use rune::{El, el, row, text};
use rune_kit::Kit;

use crate::theme_of;
use crate::tokens::{radius, type_};

pub struct DateDivider {
    label: String,
}

impl DateDivider {
    pub fn new(label: impl Into<String>) -> DateDivider {
        DateDivider {
            label: label.into(),
        }
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let line = || el().grow(1.0).h(1.0).fill(t.border_subtle.color());
        row()
            .role("separator")
            .label(self.label.clone())
            .w_full()
            .px(16.0)
            .mt(16.0)
            .mb(8.0)
            .gap(12.0)
            .center_y()
            .child(line())
            .child(
                text(self.label)
                    .font(type_::LABEL)
                    .color(t.text_muted.color())
                    .a11y_hidden(),
            )
            .child(line())
    }
}

pub struct NewMessagesDivider;

impl NewMessagesDivider {
    pub fn build(k: &Kit) -> El {
        let t = theme_of(k);
        row()
            .role("separator")
            .label("New messages")
            .w_full()
            .px(16.0)
            .mt(12.0)
            .mb(4.0)
            .center_y()
            .child(el().grow(1.0).h(1.0).fill(t.accent.color()))
            .child(
                row()
                    .h(16.0)
                    .px(6.0)
                    .center()
                    .radius(radius::XS)
                    .fill(t.accent.color())
                    .child(
                        text("New")
                            .font(type_::LABEL)
                            .color(t.accent_fg.color())
                            .a11y_hidden(),
                    ),
            )
    }
}
