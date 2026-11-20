//! thehideout's design language on Rune: tokens, a `Kit` mapping, and the basic controls.
//!
//! Platform-neutral on purpose (no fs, no windowing): the desktop, web and mobile apps all
//! depend on this crate. Warm greys, one accent (sage-teal by default; the user picks a `ThemeSpec`), crisp OpenCode-scale radii, borders
//! before shadows. See `README.md` for the spec.
//!
//! ```ignore
//! let k = thehideout_design::kit(dark);
//! Button::new("Send").variant(Variant::Primary).build(k)
//! ```

#![forbid(unsafe_code)]

pub mod components;
pub mod icons;
pub mod kit;
pub mod tokens;

pub use kit::{kit, kit_for, theme_of};
pub use tokens::accent::AccentPreset;
pub use tokens::background::BackgroundPreset;
pub use tokens::spec::{Appearance, Contrast, ThemeSpec};
pub use tokens::palette::{PALETTE_KEYS, palette_color};
pub use tokens::theme::{Theme, theme, theme_for};
pub use tokens::{accent, background, color, spec, elevation, icon, motion, radius, space, type_};

/// Instrument Sans (variable, wdth and wght axes) and JetBrains Mono (Regular, Bold), for
/// `AppSpec::fonts`. Both SIL OFL 1.1; licence texts sit next to the files in `fonts/`.
pub const FONTS: &[&[u8]] = &[
    include_bytes!("../fonts/instrument-sans/InstrumentSans-VF.ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Bold.ttf"),
];

/// `AppSpec::families`: (sans, mono), the family names inside the font files.
pub const FAMILIES: (&str, &str) = ("Instrument Sans", "JetBrains Mono");
