//! The `themes` page: every accent on every background, in the current appearance, tint and
//! contrast. Press a tile to make it the look of the whole gallery.

use rune::{El, col};
use rune_kit::Kit;
use thehideout_design::components::ThemePreview;
use thehideout_design::{AccentPreset, BackgroundPreset, ThemeSpec, theme_of};

use crate::G;
use crate::ui::*;

pub fn page(s: &G, k: &Kit) -> El {
    let dark = theme_of(k).dark;
    let accents: Vec<&str> = AccentPreset::ALL.iter().map(|a| a.name()).collect();
    let backgrounds: Vec<&str> = BackgroundPreset::ALL.iter().map(|b| b.name()).collect();
    let spec = s.spec;
    let matrix = grid(k, 96.0, 136.0, &accents, &backgrounds, move |r, c| {
        let (accent, background) = (AccentPreset::ALL[r], BackgroundPreset::ALL[c]);
        let look = ThemeSpec { accent, background, ..spec };
        ThemePreview::new(look, dark)
            .size(120.0, 72.0)
            .selected(spec.accent == accent && spec.background == background)
            .build()
            .role("button")
            .label(format!("{} on {}", accent.name(), background.name()))
            .focusable(true)
            .on_press(move |g: &mut G| {
                g.spec.accent = accent;
                g.spec.background = background;
            })
    });
    col()
        .gap(24.0)
        .child(page_title(
            k,
            "Themes",
            "8 accents x 5 backgrounds. Users pick presets only: every combination is contrast-tested.",
        ))
        .child(section(
            k,
            "Accent x background",
            "Shown in the current appearance, tint and contrast (header buttons). Press a tile to apply it to every page.",
            matrix,
        ))
}
