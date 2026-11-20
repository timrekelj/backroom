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

/// The alpha of a role chip's wash (a mention of a coloured member).
pub const WASH_ALPHA: f32 = 0.16;

/// The palette tone for `key` as **text** (a role-coloured name, a mention): the icon tone with its
/// lightness moved just far enough that it reads at the theme's text contrast (4.5:1, 7:1 on High)
/// on `bg`, `surface`, `surface_raised`, `surface_overlay`, under a hover or pressed row, under the
/// mention wash and inside its own chip wash ([`palette_wash`]). Hue and chroma stay the key's.
pub fn palette_text(key: &str, theme: &Theme) -> Option<Tone> {
    use crate::components::util::over;
    use crate::tokens::color::contrast;
    use crate::tokens::spec::Contrast;
    let ink = palette_color(key, theme)?;
    let min = if theme.spec.contrast == Contrast::High {
        7.0
    } else {
        4.5
    };
    let wash = palette_wash(key, theme)?.color();
    let mut grounds = Vec::new();
    for base in [
        theme.bg,
        theme.surface,
        theme.surface_raised,
        theme.surface_overlay,
    ] {
        let base = base.color();
        grounds.push(base);
        grounds.push(over(theme.hover.color(), base));
        grounds.push(over(theme.pressed.color(), base));
        grounds.push(over(theme.accent_wash.color(), base));
        grounds.push(over(wash, base));
    }
    let ok = |t: Tone| grounds.iter().all(|g| contrast(t.color(), *g) >= min);
    let step = if theme.dark { 0.004 } else { -0.004 };
    let mut t = ink;
    while !ok(t) && (0.0..=1.0).contains(&(t.l + step)) {
        t.l += step;
        t = Tone { c: ink.c, ..t }.in_gamut();
    }
    Some(t)
}

/// The faint fill of a chip in palette colour `key` (a mention): the icon tone at [`WASH_ALPHA`].
pub fn palette_wash(key: &str, theme: &Theme) -> Option<Tone> {
    palette_color(key, theme).map(|t| t.alpha(WASH_ALPHA))
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    use crate::tokens::color::contrast;
    use crate::tokens::spec::ThemeSpec;

    #[test]
    fn text_tones_clear_the_text_bar_on_every_surface_in_every_look() {
        use crate::components::util::over;
        use crate::tokens::spec::Contrast;
        let mut checked = 0;
        for t in themes() {
            let min = if t.spec.contrast == Contrast::High {
                7.0
            } else {
                4.5
            };
            for k in PALETTE_KEYS {
                let ink = palette_text(k, &t).unwrap().color();
                let wash = palette_wash(k, &t).unwrap().color();
                for (sn, base) in [
                    ("bg", t.bg),
                    ("surface", t.surface),
                    ("raised", t.surface_raised),
                    ("overlay", t.surface_overlay),
                ] {
                    let base = base.color();
                    for (gn, ground) in [
                        ("plain", base),
                        ("hover", over(t.hover.color(), base)),
                        ("pressed", over(t.pressed.color(), base)),
                        ("mention wash", over(t.accent_wash.color(), base)),
                        ("chip wash", over(wash, base)),
                    ] {
                        let ratio = contrast(ink, ground);
                        assert!(
                            ratio >= min,
                            "{k} on {sn}/{gn} is {ratio:.2} < {min} (dark={}, {:?}/{:?})",
                            t.dark,
                            t.spec.accent,
                            t.spec.background
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(
            checked,
            320 * 12 * 4 * 5,
            "all 320 looks, 12 keys, 4 surfaces, 5 grounds"
        );
    }

    #[test]
    fn text_tones_keep_their_hue_and_stay_in_gamut() {
        for t in themes().iter().step_by(7) {
            for (i, k) in PALETTE_KEYS.iter().enumerate() {
                let (ink, text) = (palette_color(k, t).unwrap(), palette_text(k, t).unwrap());
                assert_eq!(text.h, ink.h, "{k}");
                assert!(text.c <= ink.c + 1e-4, "{k}: chroma only ever eases");
                let lab = text.color().to_oklab();
                assert!(
                    (lab[1].hypot(lab[2]) - text.c).abs() < 0.01,
                    "{k} is out of gamut"
                );
                let _ = i;
            }
        }
        let t = Theme::build(&ThemeSpec::default(), true);
        assert!(palette_text("", &t).is_none() && palette_wash("nope", &t).is_none());
    }

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
