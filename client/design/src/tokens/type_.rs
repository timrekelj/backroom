//! Type: Instrument Sans for UI, JetBrains Mono for code (ligatures off). Seven reading styles plus the control face.
//! Sizes, weights and line heights are fixed here; screens never invent their own.

use rune::TextStyle;

use super::metrics::Metrics;

/// Small print: timestamps, counts, helper text. 11 / 16.
pub const CAPTION: TextStyle = TextStyle::new(11.0, 450.0).lh(16.0).tracking(0.05);
/// Section labels and chips. 12 / 16.
pub const LABEL: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0).tracking(0.02);
/// Message text and general reading. 14 / 21.
pub const BODY: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
/// Names, emphasis within body copy. 14 / 21, semibold.
pub const BODY_STRONG: TextStyle = TextStyle::new(14.0, 600.0).lh(21.0);
/// Panel and dialog titles. 16 / 24.
pub const TITLE: TextStyle = TextStyle::new(16.0, 600.0).lh(24.0).tracking(-0.1);
/// Page headings. 20 / 28.
pub const HEADING: TextStyle = TextStyle::new(20.0, 650.0).lh(28.0).tracking(-0.2);
/// Rare, large moments (welcome, empty states). 28 / 36.
pub const DISPLAY: TextStyle = TextStyle::new(28.0, 700.0).lh(36.0).tracking(-0.4);

/// Text inside controls (buttons, inputs, menu items). 13 / 20.
pub const CONTROL: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
pub const CONTROL_STRONG: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// Code and identifiers. 12.5 / 19, JetBrains Mono.
pub const CODE: TextStyle = TextStyle::new(12.5, 400.0).lh(19.0).mono();

/// The scale in reading order, named, for docs and the gallery.
pub const SCALE: [(&str, TextStyle); 9] = [
    ("caption", CAPTION),
    ("label", LABEL),
    ("control", CONTROL),
    ("body", BODY),
    ("body-strong", BODY_STRONG),
    ("title", TITLE),
    ("heading", HEADING),
    ("display", DISPLAY),
    ("code", CODE),
];

pub fn kit(m: Metrics) -> rune_kit::Type {
    let f = |s: TextStyle| m.font(s);
    let base = base_kit();
    rune_kit::Type {
        ui: f(base.ui),
        ui_medium: f(base.ui_medium),
        ui_compact: f(base.ui_compact),
        small: f(base.small),
        small_medium: f(base.small_medium),
        body: f(base.body),
        body_medium: f(base.body_medium),
        text_small: f(base.text_small),
        text_small_medium: f(base.text_small_medium),
        text_xsmall: f(base.text_xsmall),
        text_xsmall_medium: f(base.text_xsmall_medium),
        mono: f(base.mono),
    }
}

/// `style` at the size the kit `k` was built for.
pub fn at(k: &rune_kit::Kit, style: TextStyle) -> TextStyle {
    crate::kit::metrics_of(k).font(style)
}

const fn base_kit() -> rune_kit::Type {
    rune_kit::Type {
        ui: CONTROL,
        ui_medium: CONTROL_STRONG,
        ui_compact: CONTROL.lh(16.0),
        small: CAPTION,
        small_medium: TextStyle::new(11.0, 530.0).lh(16.0).tracking(0.05),
        body: BODY,
        body_medium: TextStyle::new(14.0, 500.0).lh(21.0),
        text_small: TextStyle::new(13.0, 400.0).lh(19.5),
        text_small_medium: TextStyle::new(13.0, 500.0).lh(19.5),
        text_xsmall: TextStyle::new(12.0, 400.0).lh(18.0),
        text_xsmall_medium: TextStyle::new(12.0, 500.0).lh(18.0),
        mono: CODE,
    }
}
