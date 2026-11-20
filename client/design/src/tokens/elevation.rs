//! Elevation: flat first. Borders separate surfaces; shadows exist only for things that float
//! above the page (popovers, modals). Two levels, both subtle and warm-tinted.

use rune::Shadow;

use super::color::Tone;

/// Where a surface sits in z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// On the page: no shadow, a border does the separating.
    Flat,
    /// Menus, popovers, tooltips, select lists.
    Popover,
    /// Dialogs and sheets.
    Modal,
}

pub const LEVELS: [(&str, Level); 3] = [
    ("flat", Level::Flat),
    ("popover", Level::Popover),
    ("modal", Level::Modal),
];

fn shade(dark: bool, strong: f32) -> Tone {
    // Warm umber rather than black; dark mode needs more alpha to read at all.
    Tone::new(0.08, 0.02, 50.0).alpha(if dark { strong } else { strong * 0.42 })
}

/// The shadow layers for a level in one appearance.
pub fn shadows(level: Level, dark: bool) -> Vec<Shadow> {
    let layer = |y: f32, blur: f32, a: f32| Shadow {
        x: 0.0,
        y,
        blur,
        spread: 0.0,
        color: shade(dark, a).color(),
        inset: false,
    };
    match level {
        Level::Flat => Vec::new(),
        Level::Popover => vec![layer(1.0, 2.0, 0.30), layer(6.0, 16.0, 0.38)],
        Level::Modal => vec![layer(2.0, 6.0, 0.30), layer(16.0, 40.0, 0.50)],
    }
}
