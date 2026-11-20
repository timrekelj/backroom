//! The accent presets, as values. Sage-teal is the default. A `Palette` is the accent plus
//! everything that must move with it: the status hues that would otherwise collide with the
//! accent (success is green, so a green accent shifts it), and the avatar / server-badge fills.
//! `AccentPreset` names the eight presets a user can choose from; nothing else is configurable,
//! because every preset is contrast-tested against every background (`tests.rs`).

use serde::{Deserialize, Serialize};

use super::color::Tone;

/// The accent in one appearance: a fill, the ink on that fill, the same hue made dark/light
/// enough to read as text or a focus ring on `bg`, and a quiet wash for accent backgrounds.
#[derive(Clone, Copy, Debug)]
pub struct Accent {
    pub fill: Tone,
    pub fg: Tone,
    pub text: Tone,
    pub subtle: Tone,
}

/// An accent in both appearances, the status hues that stay distinct from it, and the avatar fills.
#[derive(Debug)]
pub struct Palette {
    pub name: &'static str,
    pub dark: Accent,
    pub light: Accent,
    /// Hue of `success` (the other success tokens sit 5 degrees warmer).
    pub success_h: f32,
    /// Chroma scale of the success tokens (teal greens clip the sRGB gamut sooner).
    pub success_c: f32,
    /// Hue of `warning` in dark; light and the washes derive from it.
    pub warning_h: f32,
    /// Hue of `info` (light text sits 5 degrees away).
    pub info_h: f32,
    /// Hue of `danger` (the solid fill sits 2 degrees warmer); moves off a red-leaning accent.
    pub danger_h: f32,
    /// Eight avatar / guild-badge fills, all with ink-safe lightness.
    pub avatars: [Tone; 8],
}

impl Palette {
    pub fn accent(&self, dark: bool) -> Accent {
        if dark { self.dark } else { self.light }
    }
}

/// Sage-teal: a quiet grey-green-blue. Success turns to a livelier leaf green. The default.
pub static SAGE_TEAL: Palette = Palette {
    name: "Sage-teal",
    dark: Accent {
        fill: Tone::new(0.74, 0.090, 175.0),
        fg: Tone::new(0.20, 0.030, 175.0),
        text: Tone::new(0.80, 0.085, 175.0),
        subtle: Tone::new(0.29, 0.040, 175.0),
    },
    light: Accent {
        fill: Tone::new(0.72, 0.090, 175.0),
        fg: Tone::new(0.20, 0.030, 175.0),
        text: Tone::new(0.50, 0.085, 178.0),
        subtle: Tone::new(0.93, 0.035, 175.0),
    },
    success_h: 138.0,
    success_c: 1.0,
    warning_h: 90.0,
    info_h: 255.0,
    danger_h: 25.0,
    avatars: [
        Tone::new(0.48, 0.105, 20.0),  // brick
        Tone::new(0.48, 0.085, 175.0), // sage-teal
        Tone::new(0.48, 0.090, 245.0), // slate blue
        Tone::new(0.48, 0.095, 135.0), // leaf
        Tone::new(0.48, 0.100, 5.0),   // rose
        Tone::new(0.48, 0.075, 215.0), // steel
        Tone::new(0.48, 0.085, 100.0), // olive gold
        Tone::new(0.48, 0.070, 345.0), // muted plum
    ],
};

/// Ember: a lamp-lit amber-orange. The fill is the same bright ember in both appearances, so
/// the brand reads identically; only the text variant shifts for contrast.
pub static EMBER: Palette = Palette {
    name: "Ember",
    dark: Accent {
        fill: Tone::new(0.74, 0.155, 58.0),
        fg: Tone::new(0.20, 0.025, 55.0),
        text: Tone::new(0.78, 0.140, 60.0),
        subtle: Tone::new(0.29, 0.055, 52.0),
    },
    light: Accent {
        fill: Tone::new(0.72, 0.150, 58.0),
        fg: Tone::new(0.20, 0.025, 55.0),
        text: Tone::new(0.52, 0.140, 45.0),
        subtle: Tone::new(0.93, 0.045, 65.0),
    },
    success_h: 135.0,
    success_c: 1.0,
    warning_h: 90.0,
    info_h: 235.0,
    danger_h: 25.0,
    avatars: [
        Tone::new(0.48, 0.110, 30.0),  // rust
        Tone::new(0.48, 0.100, 55.0),  // copper
        Tone::new(0.48, 0.085, 80.0),  // ochre
        Tone::new(0.48, 0.085, 110.0), // olive
        Tone::new(0.48, 0.085, 145.0), // moss
        Tone::new(0.48, 0.060, 170.0), // sage
        Tone::new(0.48, 0.100, 355.0), // rose clay
        Tone::new(0.48, 0.090, 320.0), // plum
    ],
};

/// Moss: muted olive-leaning green. Success moves to a cool mint so the two never read alike.
pub static MOSS: Palette = Palette {
    name: "Moss",
    dark: Accent {
        fill: Tone::new(0.74, 0.125, 128.0),
        fg: Tone::new(0.20, 0.035, 128.0),
        text: Tone::new(0.80, 0.115, 128.0),
        subtle: Tone::new(0.29, 0.050, 128.0),
    },
    light: Accent {
        fill: Tone::new(0.72, 0.125, 128.0),
        fg: Tone::new(0.20, 0.035, 128.0),
        text: Tone::new(0.50, 0.110, 130.0),
        subtle: Tone::new(0.93, 0.045, 128.0),
    },
    success_h: 172.0,
    success_c: 0.75,
    warning_h: 80.0,
    info_h: 245.0,
    danger_h: 25.0,
    avatars: [
        Tone::new(0.48, 0.110, 22.0),  // brick
        Tone::new(0.48, 0.080, 195.0), // teal
        Tone::new(0.48, 0.095, 128.0), // moss
        Tone::new(0.48, 0.090, 250.0), // slate blue
        Tone::new(0.48, 0.100, 5.0),   // rose
        Tone::new(0.48, 0.085, 160.0), // sea green
        Tone::new(0.48, 0.075, 225.0), // steel
        Tone::new(0.48, 0.070, 345.0), // muted plum
    ],
};

/// Petrol: a deep desaturated blue-green. Info moves to a clearer blue so it stays distinct.
pub static PETROL: Palette = Palette {
    name: "Petrol",
    dark: Accent {
        fill: Tone::new(0.67, 0.085, 220.0),
        fg: Tone::new(0.17, 0.030, 220.0),
        text: Tone::new(0.77, 0.085, 220.0),
        subtle: Tone::new(0.29, 0.040, 220.0),
    },
    light: Accent {
        fill: Tone::new(0.65, 0.085, 220.0),
        fg: Tone::new(0.17, 0.030, 220.0),
        text: Tone::new(0.47, 0.085, 222.0),
        subtle: Tone::new(0.93, 0.030, 220.0),
    },
    success_h: 145.0,
    success_c: 1.0,
    warning_h: 90.0,
    info_h: 268.0,
    danger_h: 25.0,
    avatars: [
        Tone::new(0.48, 0.105, 22.0),  // brick
        Tone::new(0.48, 0.085, 215.0), // petrol
        Tone::new(0.48, 0.090, 150.0), // green
        Tone::new(0.48, 0.095, 255.0), // blue
        Tone::new(0.48, 0.095, 8.0),   // rose
        Tone::new(0.48, 0.080, 185.0), // teal
        Tone::new(0.48, 0.085, 105.0), // olive gold
        Tone::new(0.48, 0.070, 345.0), // muted plum
    ],
};

/// Brass: a muted yellow-gold, kept well clear of orange. Warning moves to amber-orange.
pub static BRASS: Palette = Palette {
    name: "Brass",
    dark: Accent {
        fill: Tone::new(0.78, 0.125, 92.0),
        fg: Tone::new(0.20, 0.030, 92.0),
        text: Tone::new(0.82, 0.120, 92.0),
        subtle: Tone::new(0.30, 0.055, 92.0),
    },
    light: Accent {
        fill: Tone::new(0.76, 0.125, 92.0),
        fg: Tone::new(0.20, 0.030, 92.0),
        text: Tone::new(0.50, 0.100, 90.0),
        subtle: Tone::new(0.94, 0.050, 95.0),
    },
    success_h: 150.0,
    success_c: 1.0,
    warning_h: 55.0,
    info_h: 235.0,
    danger_h: 25.0,
    avatars: [
        Tone::new(0.48, 0.105, 18.0),  // brick
        Tone::new(0.48, 0.080, 200.0), // teal
        Tone::new(0.48, 0.090, 150.0), // green
        Tone::new(0.48, 0.090, 250.0), // slate blue
        Tone::new(0.48, 0.100, 2.0),   // rose
        Tone::new(0.48, 0.075, 225.0), // steel
        Tone::new(0.48, 0.085, 125.0), // lichen
        Tone::new(0.48, 0.070, 345.0), // muted plum
    ],
};

/// Ocean: a clear mid blue, well short of violet. Info moves to cyan so the two never read alike.
pub static OCEAN: Palette = Palette {
    name: "Ocean",
    dark: Accent {
        fill: Tone::new(0.72, 0.125, 250.0),
        fg: Tone::new(0.18, 0.040, 250.0),
        text: Tone::new(0.80, 0.100, 250.0),
        subtle: Tone::new(0.30, 0.055, 250.0),
    },
    light: Accent {
        fill: Tone::new(0.70, 0.125, 250.0),
        fg: Tone::new(0.18, 0.040, 250.0),
        text: Tone::new(0.48, 0.130, 255.0),
        subtle: Tone::new(0.93, 0.035, 250.0),
    },
    success_h: 150.0,
    success_c: 1.0,
    warning_h: 90.0,
    info_h: 205.0,
    danger_h: 25.0,
    avatars: [
        Tone::new(0.48, 0.105, 20.0),  // brick
        Tone::new(0.48, 0.080, 195.0), // teal
        Tone::new(0.48, 0.090, 150.0), // green
        Tone::new(0.48, 0.095, 250.0), // blue
        Tone::new(0.48, 0.100, 5.0),   // rose
        Tone::new(0.48, 0.085, 100.0), // olive gold
        Tone::new(0.48, 0.075, 225.0), // steel
        Tone::new(0.48, 0.070, 345.0), // muted plum
    ],
};

/// Coral: a warm pink-red, nowhere near magenta. Danger moves to a deeper brick red.
pub static CORAL: Palette = Palette {
    name: "Coral",
    dark: Accent {
        fill: Tone::new(0.74, 0.130, 12.0),
        fg: Tone::new(0.20, 0.030, 12.0),
        text: Tone::new(0.80, 0.115, 12.0),
        subtle: Tone::new(0.29, 0.055, 12.0),
    },
    light: Accent {
        fill: Tone::new(0.72, 0.135, 12.0),
        fg: Tone::new(0.20, 0.030, 12.0),
        text: Tone::new(0.52, 0.160, 15.0),
        subtle: Tone::new(0.94, 0.035, 12.0),
    },
    success_h: 145.0,
    success_c: 1.0,
    warning_h: 90.0,
    info_h: 245.0,
    danger_h: 38.0,
    avatars: [
        Tone::new(0.48, 0.100, 30.0),  // brick
        Tone::new(0.48, 0.080, 190.0), // teal
        Tone::new(0.48, 0.090, 145.0), // green
        Tone::new(0.48, 0.090, 250.0), // slate blue
        Tone::new(0.48, 0.100, 10.0),  // coral rose
        Tone::new(0.48, 0.085, 105.0), // olive gold
        Tone::new(0.48, 0.075, 220.0), // steel
        Tone::new(0.48, 0.070, 345.0), // muted plum
    ],
};

/// Graphite: a neutral grey accent for a near-mono look. In dark the fill is light grey with
/// dark ink; in light it is charcoal with light ink. Status hues keep their usual places.
pub static GRAPHITE: Palette = Palette {
    name: "Graphite",
    dark: Accent {
        fill: Tone::new(0.84, 0.008, 260.0),
        fg: Tone::new(0.20, 0.010, 260.0),
        text: Tone::new(0.84, 0.008, 260.0),
        subtle: Tone::new(0.30, 0.012, 260.0),
    },
    light: Accent {
        fill: Tone::new(0.30, 0.010, 260.0),
        fg: Tone::new(0.985, 0.004, 260.0),
        text: Tone::new(0.34, 0.010, 260.0),
        subtle: Tone::new(0.92, 0.008, 260.0),
    },
    success_h: 145.0,
    success_c: 1.0,
    warning_h: 90.0,
    info_h: 250.0,
    danger_h: 25.0,
    avatars: SAGE_TEAL.avatars,
};

/// The eight accent presets a user can pick, default first. Serialises as its `key()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccentPreset {
    #[default]
    SageTeal,
    Moss,
    Petrol,
    Ocean,
    Brass,
    Ember,
    Coral,
    Graphite,
}

impl AccentPreset {
    pub const ALL: [AccentPreset; 8] = [
        AccentPreset::SageTeal,
        AccentPreset::Moss,
        AccentPreset::Petrol,
        AccentPreset::Ocean,
        AccentPreset::Brass,
        AccentPreset::Ember,
        AccentPreset::Coral,
        AccentPreset::Graphite,
    ];

    pub fn palette(self) -> &'static Palette {
        match self {
            AccentPreset::SageTeal => &SAGE_TEAL,
            AccentPreset::Moss => &MOSS,
            AccentPreset::Petrol => &PETROL,
            AccentPreset::Ocean => &OCEAN,
            AccentPreset::Brass => &BRASS,
            AccentPreset::Ember => &EMBER,
            AccentPreset::Coral => &CORAL,
            AccentPreset::Graphite => &GRAPHITE,
        }
    }

    pub fn name(self) -> &'static str {
        self.palette().name
    }

    /// Position in `ALL`, for caches.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}
