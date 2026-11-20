//! Corner radii: OpenCode's scale, copied exactly (`rune-theme-opencode`: 2, 4, 6, 8, 10).
//! Crisp corners are part of the language: never round more than `XL` except dots and pills.

use rune_kit::Radius;

pub const XS: f32 = 2.0;
pub const SM: f32 = 4.0;
pub const MD: f32 = 6.0;
pub const LG: f32 = 8.0;
pub const XL: f32 = 10.0;
/// Fully round, for dots and count pills only.
pub const FULL: f32 = 999.0;

pub const SCALE: [(&str, f32); 5] = [
    ("radius-xs", XS),
    ("radius-sm", SM),
    ("radius-md", MD),
    ("radius-lg", LG),
    ("radius-xl", XL),
];

pub const fn kit() -> Radius {
    Radius {
        xs: XS,
        sm: SM,
        md: MD,
        lg: LG,
        xl: XL,
    }
}
