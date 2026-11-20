//! The 12-key palette channels and roles are coloured with (slate red orange amber yellow lime
//! green teal cyan blue indigo pink; `""` is none). The keys are the wire contract (`proto/README.md`);
//! what they look like is the design's: one tone per key and appearance, authored in OKLCH like
//! everything else, for use as **icon ink** (non-text, 3:1) on `bg`, `surface`, `surface_raised`
//! and under the selected-row wash, in every look. Tests enforce it (`palette_tests`).

use super::color::Tone;
use super::theme::Theme;

/// The 12 keys in palette order.
pub const PALETTE_KEYS: [&str; 12] = [
    "slate", "red", "orange", "amber", "yellow", "lime", "green", "teal", "cyan", "blue", "indigo",
    "pink",
];

/// (chroma, hue) per key, in `PALETTE_KEYS` order. Lightness is shared per appearance, so the
/// twelve read as one family and contrast is a property of L alone.
const HUES: [(f32, f32); 12] = [
    (0.030, 255.0),
    (0.170, 25.0),
    (0.150, 52.0),
    (0.140, 78.0),
    (0.140, 100.0),
    (0.150, 128.0),
    (0.150, 150.0),
    (0.110, 185.0),
    (0.110, 215.0),
    (0.150, 255.0),
    (0.150, 282.0),
    (0.150, 352.0),
];

const L_DARK: f32 = 0.76;
const L_LIGHT: f32 = 0.53;

/// The tone for palette `key` in `theme`'s appearance, or `None` for `""` and unknown keys.
pub fn palette_color(key: &str, theme: &Theme) -> Option<Tone> {
    let i = PALETTE_KEYS.iter().position(|k| *k == key)?;
    let (c, h) = HUES[i];
    Some(Tone::new(if theme.dark { L_DARK } else { L_LIGHT }, c, h).in_gamut())
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    use crate::tokens::color::contrast;
    use crate::tokens::spec::ThemeSpec;

    fn themes() -> Vec<Theme> {
        ThemeSpec::all_looks()
            .map(|(spec, dark)| Theme::build(&spec, dark))
            .collect()
    }

    #[test]
    fn every_key_has_a_tone_and_none_does_not() {
        let t = Theme::build(&ThemeSpec::default(), true);
        for k in PALETTE_KEYS {
            assert!(palette_color(k, &t).is_some(), "{k}");
        }
        assert!(palette_color("", &t).is_none());
        assert!(palette_color("magenta", &t).is_none());
    }

    #[test]
    fn the_keys_match_the_api_palette() {
        // Same order as `thehideout_api::PaletteColor::ALL` (the wire keys).
        assert_eq!(
            PALETTE_KEYS.join(" "),
            "slate red orange amber yellow lime green teal cyan blue indigo pink"
        );
    }

    #[test]
    fn icon_ink_has_three_to_one_on_every_surface_in_every_look() {
        for t in themes() {
            let wash = crate::components::util::over(t.pressed.color(), t.bg.color());
            for k in PALETTE_KEYS {
                let ink = palette_color(k, &t).unwrap().color();
                for (sn, s) in [
                    ("bg", t.bg.color()),
                    ("surface", t.surface.color()),
                    ("raised", t.surface_raised.color()),
                    ("selected row", wash),
                ] {
                    let ratio = contrast(ink, s);
                    assert!(
                        ratio >= 3.0,
                        "{k} on {sn} is {ratio:.2} (dark={}, {:?}/{:?})",
                        t.dark,
                        t.spec.accent,
                        t.spec.background
                    );
                }
            }
        }
    }

    #[test]
    fn tones_are_in_gamut_and_distinct() {
        for dark in [false, true] {
            let t = Theme::build(&ThemeSpec::default(), dark);
            let tones: Vec<Tone> = PALETTE_KEYS
                .iter()
                .map(|k| palette_color(k, &t).unwrap())
                .collect();
            for tone in &tones {
                let lab = tone.color().to_oklab();
                assert!(
                    (lab[1].hypot(lab[2]) - tone.c).abs() < 0.01,
                    "{tone:?} is out of gamut"
                );
            }
            for (i, a) in tones.iter().enumerate() {
                for b in &tones[i + 1..] {
                    let d = |t: &Tone| (t.c * t.h.to_radians().cos(), t.c * t.h.to_radians().sin());
                    let ((a1, a2), (b1, b2)) = (d(a), d(b));
                    let dist = (a1 - b1).hypot(a2 - b2);
                    assert!(dist > 0.03, "{a:?} and {b:?} are too close ({dist:.3})");
                }
            }
        }
    }
}
