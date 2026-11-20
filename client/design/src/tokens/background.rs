//! The background presets: a neutral ramp for dark and one for light. Surfaces, borders and text
//! are picked from the ramp by step (`theme.rs`), so a preset only has to say what grey it is.
//! Warm is the default (the original palette). Deep only changes dark; Paper only changes light.

use serde::{Deserialize, Serialize};

use super::color::{NEUTRAL_L, Ramp, WARM_RAMP};

/// True grey: no hue at all.
const NEUTRAL_RAMP: Ramp = Ramp {
    l: NEUTRAL_L,
    c: (0.0, 0.0),
    h: (0.0, 0.0),
};

/// Slate: a quiet blue-grey, hue about 250, still low chroma.
const COOL_RAMP: Ramp = Ramp {
    l: NEUTRAL_L,
    c: (0.008, 0.013),
    h: (250.0, 255.0),
};

/// OLED dark: the dark end sinks to near `#0b0b0b` with a warm hint. Steps 0..8 are the warm
/// ramp's, so a light theme built from it would match.
const DEEP_DARK_RAMP: Ramp = Ramp {
    l: [
        0.985, 0.965, 0.94, 0.90, 0.84, 0.74, 0.64, 0.54, 0.49, 0.37, 0.29, 0.235, 0.195, 0.150,
        0.120,
    ],
    c: (0.003, 0.0055),
    h: (75.0, 58.0),
};

/// Paper: cream instead of off-white, a little warmer and yellower.
const PAPER_LIGHT_RAMP: Ramp = Ramp {
    l: [
        0.982, 0.955, 0.93, 0.89, 0.83, 0.74, 0.64, 0.54, 0.49, 0.38, 0.31, 0.26, 0.22, 0.185,
        0.155,
    ],
    c: (0.014, 0.016),
    h: (88.0, 70.0),
};

/// The five neutral backgrounds a user can pick from, default first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundPreset {
    #[default]
    Warm,
    Neutral,
    Cool,
    Deep,
    Paper,
}

impl BackgroundPreset {
    pub const ALL: [BackgroundPreset; 5] = [
        BackgroundPreset::Warm,
        BackgroundPreset::Neutral,
        BackgroundPreset::Cool,
        BackgroundPreset::Deep,
        BackgroundPreset::Paper,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BackgroundPreset::Warm => "Warm",
            BackgroundPreset::Neutral => "Neutral",
            BackgroundPreset::Cool => "Cool",
            BackgroundPreset::Deep => "Deep",
            BackgroundPreset::Paper => "Paper",
        }
    }

    /// The ramp for one appearance.
    pub fn ramp(self, dark: bool) -> Ramp {
        match (self, dark) {
            (BackgroundPreset::Neutral, _) => NEUTRAL_RAMP,
            (BackgroundPreset::Cool, _) => COOL_RAMP,
            (BackgroundPreset::Deep, true) => DEEP_DARK_RAMP,
            (BackgroundPreset::Paper, false) => PAPER_LIGHT_RAMP,
            _ => WARM_RAMP,
        }
    }

    /// Position in `ALL`, for caches.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}
