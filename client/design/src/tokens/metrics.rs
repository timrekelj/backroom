//! Metrics: the two sizing choices a user makes besides colour. `Density` tightens or relaxes the
//! layout (row heights, header and footer, message spacing, paddings); `TextSize` scales the type
//! scale, its line heights, and the icons and inline emoji that sit with it. They are part of the
//! `ThemeSpec` and travel with the kit (`metrics_of(k)`), so a component asks the kit it was
//! given and nothing is global. Comfortable at 100% is the shipped look.
//!
//! Every derived size lives here, so screens never branch on the density themselves, and the
//! header and footer are one function each: sidebar, channel header, member panel and composer
//! zone cannot drift apart.

use rune::TextStyle;
use serde::{Deserialize, Serialize};

/// How tight the layout is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Density {
    /// The shipped layout.
    #[default]
    Comfortable,
    /// Shorter rows, headers and gaps: more on screen.
    Compact,
}

impl Density {
    pub const ALL: [Density; 2] = [Density::Comfortable, Density::Compact];

    pub fn name(self) -> &'static str {
        match self {
            Density::Comfortable => "Comfortable",
            Density::Compact => "Compact",
        }
    }

    fn pick(self, comfortable: f32, compact: f32) -> f32 {
        match self {
            Density::Comfortable => comfortable,
            Density::Compact => compact,
        }
    }
}

/// How large text is, as a share of the type scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TextSize {
    #[serde(rename = "90")]
    P90,
    #[default]
    #[serde(rename = "100")]
    P100,
    #[serde(rename = "110")]
    P110,
    #[serde(rename = "125")]
    P125,
}

impl TextSize {
    pub const ALL: [TextSize; 4] = [TextSize::P90, TextSize::P100, TextSize::P110, TextSize::P125];

    pub fn name(self) -> &'static str {
        match self {
            TextSize::P90 => "90%",
            TextSize::P100 => "100%",
            TextSize::P110 => "110%",
            TextSize::P125 => "125%",
        }
    }

    /// The factor on every font size and line height.
    pub fn factor(self) -> f32 {
        match self {
            TextSize::P90 => 0.90,
            TextSize::P100 => 1.0,
            TextSize::P110 => 1.10,
            TextSize::P125 => 1.25,
        }
    }

    fn index(self) -> usize {
        match self {
            TextSize::P90 => 0,
            TextSize::P100 => 1,
            TextSize::P110 => 2,
            TextSize::P125 => 3,
        }
    }
}

/// Density and text size together. `Copy`, 2 bytes: pass it around freely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Metrics {
    pub density: Density,
    pub text: TextSize,
}

fn half(x: f32) -> f32 {
    (x * 2.0).round() / 2.0
}

impl Metrics {
    /// Number of distinct metrics (2 densities x 4 text sizes).
    pub const COUNT: usize = 8;

    pub fn new(density: Density, text: TextSize) -> Metrics {
        Metrics { density, text }
    }

    /// Every combination.
    pub fn all() -> impl Iterator<Item = Metrics> {
        Density::ALL.into_iter().flat_map(|d| TextSize::ALL.into_iter().map(move |t| Metrics::new(d, t)))
    }

    /// Dense index in `0..COUNT`.
    pub fn index(self) -> usize {
        self.density as usize * 4 + self.text.index()
    }

    // ---- type ----

    /// `style` at this text size: font size and line height scaled (the line height to the half
    /// point, so rows stay on a grid the renderer snaps well).
    pub fn font(self, style: TextStyle) -> TextStyle {
        let f = self.text.factor();
        if f == 1.0 {
            return style;
        }
        TextStyle { size: style.size * f, line_height: half(style.line_height * f), ..style }
    }

    /// A length that sits with text (an inline emoji, a text-sized glyph), scaled with it.
    pub fn em(self, base: f32) -> f32 {
        (base * self.text.factor()).round()
    }

    /// An icon size: follows the text at half rate, so 16 becomes 18 at 125% and 15 at 90%.
    pub fn icon(self, base: f32) -> f32 {
        (base * (1.0 + (self.text.factor() - 1.0) * 0.5)).round()
    }

    // ---- heights ----

    fn text_floor(self, style: TextStyle, extra: f32) -> f32 {
        (self.font(style).line_height + extra).ceil()
    }

    /// A sidebar row (nav, channel, DM) and a member row: 32 / 28, never shorter than its text.
    pub fn row_h(self) -> f32 {
        self.density.pick(32.0, 28.0).max(self.text_floor(crate::tokens::type_::CONTROL, 4.0))
    }

    /// Gap between sidebar rows.
    pub fn row_gap(self) -> f32 {
        2.0
    }

    /// Distance between the tops of two rows in a list.
    pub fn row_pitch(self) -> f32 {
        self.row_h() + self.row_gap()
    }

    /// A control (Md button, segmented switch): 32 / 28, never shorter than its text.
    pub fn control_h(self) -> f32 {
        self.density.pick(32.0, 28.0).max(self.text_floor(crate::tokens::type_::CONTROL, 4.0))
    }

    /// A collapsible section's header: 28 / 24.
    pub fn section_h(self) -> f32 {
        self.density.pick(28.0, 24.0).max(self.text_floor(crate::tokens::type_::LABEL, 4.0))
    }

    /// An input line inside a card (the composer's row): 32 / 28.
    pub fn field_h(self) -> f32 {
        self.density.pick(32.0, 28.0).max(self.text_floor(crate::tokens::type_::BODY, 6.0))
    }

    /// Margin around the composer card inside its zone: 12 / 8.
    pub fn zone_pad(self) -> f32 {
        self.density.pick(12.0, 8.0)
    }

    /// Every panel header (sidebar title, channel header, member title): 48 / 40.
    pub fn header_h(self) -> f32 {
        self.density.pick(48.0, 40.0).max(self.text_floor(crate::tokens::type_::TITLE, 8.0))
    }

    /// The sidebar's user strip and the composer zone at rest: the card (one field line, 4pt of
    /// padding each side) with the zone's margin around it. 64 / 52 at 100%.
    pub fn footer_h(self) -> f32 {
        self.field_h() + 8.0 + 2.0 * self.zone_pad()
    }

    // ---- the transcript ----

    /// Space above a message group (before the row's own padding).
    pub fn group_gap(self) -> f32 {
        self.density.pick(12.0, 7.0)
    }

    /// Vertical padding of a message row: `(top, bottom)` for the first row of a group and for the
    /// rest. A follow-up's top plus the previous bottom is the gap inside a group: 2 / 1.
    pub fn msg_pad(self, head: bool) -> (f32, f32) {
        match (self.density, head) {
            (Density::Comfortable, true) => (3.0, 1.0),
            (Density::Comfortable, false) => (1.0, 1.0),
            (Density::Compact, true) => (2.0, 0.5),
            (Density::Compact, false) => (0.5, 0.5),
        }
    }

    /// The author's picture in the transcript (and the gutter beside follow-ups): 32 / 24.
    pub fn msg_avatar(self) -> u32 {
        if self.density == Density::Compact && self.text != TextSize::P125 { 24 } else { 32 }
    }

    /// The server rail's tile: 40 / 32.
    pub fn rail_tile(self) -> f32 {
        self.density.pick(40.0, 32.0)
    }

    /// Admin and settings table rows: `base` comfortable, 8 shorter compact.
    pub fn table_row(self, base: f32) -> f32 {
        self.density.pick(base, base - 8.0).max(self.text_floor(crate::tokens::type_::CONTROL, 8.0))
    }

    /// A padding or gap: `base` comfortable, a quarter less (to the 2pt) compact.
    pub fn pad(self, base: f32) -> f32 {
        match self.density {
            Density::Comfortable => base,
            Density::Compact => ((base * 0.75) / 2.0).round() * 2.0,
        }
    }
}
