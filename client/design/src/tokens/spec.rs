//! `ThemeSpec`: everything a user can choose about the look, and nothing else. Presets only, no
//! free colour, because every combination is contrast-tested (`tests.rs`). A spec plus the
//! resolved appearance builds a `Theme` (`theme.rs`) and a `Kit` (`kit.rs`), both cached.

use serde::{Deserialize, Serialize};

use super::accent::AccentPreset;
use super::background::BackgroundPreset;

/// Which appearance to draw: follow the system, or force one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn name(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    /// Dark or not, given what the system prefers.
    pub fn resolve(self, system_dark: bool) -> bool {
        match self {
            Appearance::System => system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        }
    }
}

/// How hard text and edges are pushed against the surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Contrast {
    #[default]
    Standard,
    /// Muted text at least 8:1 (faint 7:1), borders at least 3:1 and strong borders 4.5:1.
    High,
}

impl Contrast {
    pub const ALL: [Contrast; 2] = [Contrast::Standard, Contrast::High];

    pub fn name(self) -> &'static str {
        match self {
            Contrast::Standard => "Standard",
            Contrast::High => "High",
        }
    }
}

/// The user's look. `Default` is the shipped one: Sage-teal on Warm, following the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSpec {
    pub accent: AccentPreset,
    pub background: BackgroundPreset,
    pub appearance: Appearance,
    /// Surfaces and borders pick up a faint accent hue.
    pub tint: bool,
    pub contrast: Contrast,
}

impl ThemeSpec {
    /// Whether this spec draws dark, given what the system prefers.
    pub fn is_dark(&self, system_dark: bool) -> bool {
        self.appearance.resolve(system_dark)
    }

    /// Number of distinct looks (accent x background x dark/light x tint x contrast): the bound
    /// of the theme and kit caches.
    pub const LOOKS: usize = 8 * 5 * 2 * 2 * 2;

    /// Index of this spec drawn `dark` among the `LOOKS` distinct looks. `appearance` is not
    /// part of it: it only decides `dark`.
    pub fn look_index(&self, dark: bool) -> usize {
        let mut i = self.accent.index();
        i = i * 5 + self.background.index();
        i = i * 2 + usize::from(dark);
        i = i * 2 + usize::from(self.tint);
        i * 2 + usize::from(self.contrast == Contrast::High)
    }

    /// Every look as a spec with the appearance set to the resolved one.
    pub fn all_looks() -> impl Iterator<Item = (ThemeSpec, bool)> {
        AccentPreset::ALL.into_iter().flat_map(|accent| {
            BackgroundPreset::ALL.into_iter().flat_map(move |background| {
                [true, false].into_iter().flat_map(move |dark| {
                    [false, true].into_iter().flat_map(move |tint| {
                        Contrast::ALL.into_iter().map(move |contrast| {
                            let appearance = if dark { Appearance::Dark } else { Appearance::Light };
                            (ThemeSpec { accent, background, appearance, tint, contrast }, dark)
                        })
                    })
                })
            })
        })
    }
}
