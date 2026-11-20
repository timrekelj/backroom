//! Badge: a small status chip, 18pt tall, `radius-xs` corners, label type 11/600.
//! Neutral and Accent are quiet washes; Success/Warning/Danger/Info use the status tokens.

use rune::{El, row, text};
use rune_kit::Kit;

use crate::theme_of;
use crate::tokens::color::Tone;
use crate::tokens::{radius, type_};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeVariant {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Info,
}

pub struct Badge {
    label: String,
    variant: BadgeVariant,
}

impl Badge {
    pub fn new(label: impl Into<String>) -> Badge {
        Badge {
            label: label.into(),
            variant: BadgeVariant::Neutral,
        }
    }
    pub fn variant(mut self, v: BadgeVariant) -> Self {
        self.variant = v;
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let (fill, ink, border): (Tone, Tone, Tone) = match self.variant {
            BadgeVariant::Neutral => (t.surface_raised, t.text_muted, t.border),
            BadgeVariant::Accent => (t.accent_subtle, t.accent_text, t.accent_subtle),
            BadgeVariant::Success => (t.success_bg, t.success, t.success_border),
            BadgeVariant::Warning => (t.warning_bg, t.warning, t.warning_border),
            BadgeVariant::Danger => (t.danger_bg, t.danger, t.danger_border),
            BadgeVariant::Info => (t.info_bg, t.info, t.info_border),
        };
        let font = rune::TextStyle {
            size: 11.0,
            weight: 600.0,
            line_height: 14.0,
            tracking: 0.04,
            ..type_::LABEL
        };
        row()
            .role("status")
            .label(self.label.clone())
            .h(18.0)
            .px(6.0)
            .center()
            .radius(radius::XS)
            .fill(fill.color())
            .border(1.0, border.color())
            .child(text(self.label).font(font).color(ink.color()).a11y_hidden())
    }
}
