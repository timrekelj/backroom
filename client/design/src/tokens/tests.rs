use std::sync::OnceLock;

use super::accent::AccentPreset;
use super::background::BackgroundPreset;
use super::color::{Tone, contrast};
use super::spec::{Appearance, Contrast, ThemeSpec};
use super::theme::Theme;

/// Every look: 8 accents x 5 backgrounds x dark/light x tint on/off x contrast (320 themes),
/// built once and shared by all tests.
fn all_themes() -> &'static [Theme] {
    static ALL: OnceLock<Vec<Theme>> = OnceLock::new();
    ALL.get_or_init(|| {
        ThemeSpec::all_looks()
            .map(|(spec, dark)| Theme::build(&spec, dark))
            .collect()
    })
}

fn c(t: Tone) -> rune::Color {
    t.color()
}

fn surfaces(t: &Theme) -> [(&'static str, Tone); 3] {
    [
        ("bg", t.bg),
        ("surface", t.surface),
        ("surface_raised", t.surface_raised),
    ]
}

fn name(t: &Theme) -> String {
    format!(
        "{} on {} {} tint={} contrast={}",
        t.spec.accent.name(),
        t.spec.background.name(),
        if t.dark { "dark" } else { "light" },
        t.spec.tint,
        t.spec.contrast.name()
    )
}

fn high(t: &Theme) -> bool {
    t.spec.contrast == Contrast::High
}

#[test]
fn every_combination_is_covered() {
    assert_eq!(all_themes().len(), 8 * 5 * 2 * 2 * 2);
    assert_eq!(all_themes().len(), ThemeSpec::LOOKS);
    let mut seen: Vec<usize> = ThemeSpec::all_looks().map(|(s, d)| s.look_index(d)).collect();
    seen.sort_unstable();
    assert!(seen.iter().copied().eq(0..ThemeSpec::LOOKS), "look_index is a bijection");
}

#[test]
fn text_meets_aa_on_every_surface() {
    for t in all_themes() {
        for (sn, s) in surfaces(t) {
            for (tn, fg, min) in [
                ("text", t.text, 7.0),
                ("text_muted", t.text_muted, if high(t) { 8.0 } else { 4.5 }),
                ("text_faint", t.text_faint, if high(t) { 7.0 } else { 4.5 }),
                ("accent_text", t.accent_text, if high(t) { 7.0 } else { 4.5 }),
            ] {
                let r = contrast(c(fg), c(s));
                assert!(r >= min, "{} {tn} on {sn}: {r:.2}", name(t));
            }
        }
        let r = contrast(c(t.text), c(t.surface_overlay));
        assert!(r >= 4.5, "{} text on overlay: {r:.2}", name(t));
        let r = contrast(c(t.text_muted), c(t.surface_overlay));
        assert!(r >= 4.5, "{} text_muted on overlay: {r:.2}", name(t));
    }
}

#[test]
fn ink_on_fills_meets_aa() {
    for t in all_themes() {
        let r = contrast(c(t.accent_fg), c(t.accent));
        assert!(r >= 4.5, "{} accent_fg on accent: {r:.2}", name(t));
        let r = contrast(c(t.on_danger), c(t.danger_solid));
        assert!(r >= 4.5, "{} on_danger on danger_solid: {r:.2}", name(t));
        let r = contrast(c(t.accent_text), c(t.accent_subtle));
        assert!(r >= 4.5, "{} accent_text on accent_subtle: {r:.2}", name(t));
    }
}

#[test]
fn status_text_meets_aa_on_surface_and_its_wash() {
    for t in all_themes() {
        for (n, fg, bg) in [
            ("success", t.success, t.success_bg),
            ("warning", t.warning, t.warning_bg),
            ("danger", t.danger, t.danger_bg),
            ("info", t.info, t.info_bg),
        ] {
            for (sn, s) in [("bg", t.bg), ("surface", t.surface), ("wash", bg)] {
                let r = contrast(c(fg), c(s));
                let min = if high(t) { 7.0 } else { 4.5 };
                assert!(r >= min, "{} {n} on {sn}: {r:.2}", name(t));
            }
        }
    }
}

#[test]
fn borders_are_visible() {
    for t in all_themes() {
        for (sn, s) in surfaces(t) {
            let sub = contrast(c(t.border_subtle), c(s));
            let b = contrast(c(t.border), c(s));
            let strong = contrast(c(t.border_strong), c(s));
            let (min_sub, min_b, min_strong) = if high(t) { (1.8, 3.0, 4.5) } else { (1.1, 1.3, 3.0) };
            assert!(b >= min_b, "{} border on {sn}: {b:.2}", name(t));
            assert!(sub >= min_sub, "{} border_subtle on {sn}: {sub:.2}", name(t));
            assert!(
                strong >= min_strong,
                "{} border_strong on {sn}: {strong:.2}",
                name(t)
            );
            assert!(sub <= b && b <= strong, "{} border order on {sn}", name(t));
        }
        let f = contrast(c(t.focus), c(t.bg));
        assert!(f >= 3.0, "{} focus ring on bg: {f:.2}", name(t));
    }
}

#[test]
fn border_strong_meets_non_text_contrast() {
    for t in all_themes() {
        for (sn, s) in surfaces(t) {
            let r = contrast(c(t.border_strong), c(s));
            assert!(
                r >= 3.0,
                "{} border_strong on {sn} is {r:.2}, needs 3:1",
                name(t)
            );
        }
    }
}

#[test]
fn colours_stay_in_gamut() {
    // Out-of-gamut OKLCH clamps silently and shifts hue; catch tokens that would.
    for t in all_themes() {
        for (n, tone) in t.tokens() {
            if tone.a < 0.999 {
                continue;
            }
            let back = tone.color().to_oklab();
            let chroma = back[1].hypot(back[2]);
            assert!(
                (chroma - tone.c).abs() < 0.012,
                "{} {n} is out of gamut: wants {:.3}, gets {chroma:.3}",
                name(t),
                tone.c
            );
        }
    }
}

#[test]
fn backgrounds_have_their_character_and_are_never_pure() {
    for t in all_themes().iter().filter(|t| !t.spec.tint) {
        for (n, tone) in [("bg", t.bg), ("surface", t.surface), ("text", t.text)] {
            let ok = match t.spec.background {
                BackgroundPreset::Neutral => tone.c < 0.002,
                BackgroundPreset::Cool => (245.0..=260.0).contains(&tone.h) && tone.c <= 0.022,
                BackgroundPreset::Paper if !t.dark => {
                    (65.0..=92.0).contains(&tone.h) && (0.011..=0.020).contains(&tone.c)
                }
                _ => (50.0..=92.0).contains(&tone.h) && (0.003..=0.015).contains(&tone.c),
            };
            assert!(ok, "{} {n}: {}", name(t), tone.css());
        }
        assert!(t.bg.l > 0.10 && t.bg.l < 0.99, "{} bg is neither black nor white", name(t));
        if t.dark {
            let deep = t.spec.background == BackgroundPreset::Deep;
            assert!(
                (t.bg.l <= 0.16) == deep,
                "{} dark bg L {} (Deep is the only one at or under .16)",
                name(t),
                t.bg.l
            );
        }
    }
}

#[test]
fn tint_is_faint_and_leaves_lightness_alone() {
    let all = all_themes();
    for t in all.iter().filter(|t| t.spec.tint) {
        let plain = Theme::build(&ThemeSpec { tint: false, ..t.spec }, t.dark);
        for (n, a, b) in [
            ("bg", t.bg, plain.bg),
            ("surface", t.surface, plain.surface),
            ("border", t.border, plain.border),
        ] {
            let (da, db) = (
                (a.c * a.h.to_radians().cos(), a.c * a.h.to_radians().sin()),
                (b.c * b.h.to_radians().cos(), b.c * b.h.to_radians().sin()),
            );
            let shift = (da.0 - db.0).hypot(da.1 - db.1);
            assert!(shift <= 0.008, "{} {n} tint shifts chroma by {shift:.4}", name(t));
            assert!((a.l - b.l).abs() < 0.001 || n == "border", "{} {n} tint moved L", name(t));
        }
        if t.spec.accent == AccentPreset::Graphite {
            assert!((t.bg.c - plain.bg.c).abs() < 0.003, "{} tint stays grey", name(t));
        }
    }
}

#[test]
fn accent_presets_are_distinct_and_never_purple() {
    for p in AccentPreset::ALL {
        let f = p.palette().dark.fill;
        assert!(!(275.0..=345.0).contains(&f.h) || f.c < 0.03, "{} is purple", p.name());
    }
    let (ocean, coral) = (AccentPreset::Ocean.palette().dark.fill, AccentPreset::Coral.palette().dark.fill);
    assert!((235.0..=262.0).contains(&ocean.h), "Ocean is a clear blue");
    assert!((0.0..=22.0).contains(&coral.h), "Coral is a pink-red");
    assert!(AccentPreset::Graphite.palette().dark.fill.c < 0.02, "Graphite is grey");
    let mut names: Vec<_> = AccentPreset::ALL.iter().map(|p| p.name()).collect();
    names.dedup();
    assert_eq!(names.len(), 8);
}

#[test]
fn a_spec_round_trips_through_json_and_missing_fields_default() {
    let spec = ThemeSpec {
        accent: AccentPreset::Coral,
        background: BackgroundPreset::Paper,
        appearance: Appearance::Light,
        tint: true,
        contrast: Contrast::High,
    };
    let json = serde_json::to_string(&spec).unwrap();
    assert_eq!(
        json,
        r#"{"accent":"coral","background":"paper","appearance":"light","tint":true,"contrast":"high"}"#
    );
    assert_eq!(serde_json::from_str::<ThemeSpec>(&json).unwrap(), spec);
    assert_eq!(serde_json::from_str::<ThemeSpec>("{}").unwrap(), ThemeSpec::default());
}

#[test]
fn kits_and_themes_are_cached_and_a_kit_knows_its_theme() {
    let spec = ThemeSpec { accent: AccentPreset::Ocean, tint: true, ..ThemeSpec::default() };
    let (k, t) = (crate::kit_for(&spec, true), super::theme::theme_for(&spec, true));
    assert!(std::ptr::eq(k, crate::kit_for(&spec, true)));
    assert!(std::ptr::eq(t, super::theme::theme_for(&spec, true)));
    assert!(std::ptr::eq(crate::theme_of(k), t));
    assert!(std::ptr::eq(crate::theme_of(crate::kit(false)), crate::theme(false)));
    assert_eq!(t.spec.accent, AccentPreset::Ocean);
}

#[test]
fn avatar_ink_meets_aa_on_every_avatar_colour() {
    let ink = crate::components::avatar::AVATAR_INK;
    for p in AccentPreset::ALL.map(AccentPreset::palette) {
        for (i, bg) in p.avatars.iter().enumerate() {
            let r = contrast(c(ink), c(*bg));
            assert!(r >= 4.5, "{} avatar tone {i}: {r:.2}", p.name);
        }
    }
}

#[test]
fn avatars_are_not_brown_heavy_or_purple_heavy() {
    // Ember is the warm preset: its avatars are allowed to be.
    for p in AccentPreset::ALL.map(AccentPreset::palette).iter().filter(|p| p.name != "Ember") {
        let hue_in = |lo: f32, hi: f32| {
            p.avatars
                .iter()
                .filter(|t| (lo..=hi).contains(&t.h))
                .count()
        };
        assert!(
            hue_in(35.0, 85.0) == 0,
            "{} has brown-orange avatar fills",
            p.name
        );
        assert!(
            hue_in(270.0, 340.0) == 0,
            "{} has purple avatar fills",
            p.name
        );
        assert!(
            hue_in(340.0, 360.0) <= 1,
            "{} has more than one plum",
            p.name
        );
    }
}

#[test]
fn status_hues_stay_clear_of_the_accent() {
    let apart = |a: f32, b: f32| {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    };
    for preset in AccentPreset::ALL {
        let p = preset.palette();
        // A grey accent has no hue to collide with; Ember is the warm preset.
        if p.dark.fill.c < 0.03 {
            continue;
        }
        let h = p.dark.fill.h;
        for (what, status_h, exempt) in [
            ("success", p.success_h, p.name == "Ember"),
            ("info", p.info_h, false),
            ("warning", p.warning_h, p.name == "Ember"),
            ("danger", p.danger_h, false),
        ] {
            assert!(
                apart(status_h, h) >= 25.0 || exempt,
                "{} {what} hue is too close to the accent",
                p.name
            );
        }
    }
}

/// `cargo test -p thehideout-design tables -- --ignored --nocapture`
#[test]
#[ignore]
fn tables() {
    let spec = |accent, background| ThemeSpec { accent, background, ..ThemeSpec::default() };
    for a in AccentPreset::ALL {
        let (d, l) = (Theme::build(&spec(a, BackgroundPreset::Warm), true), Theme::build(&spec(a, BackgroundPreset::Warm), false));
        let badges: Vec<String> = a.palette().avatars.iter().map(|t| t.hex()).collect();
        println!(
            "| {} | `{}` | `{}` | `{}` / `{}` | success `{}` / `{}` danger `{}` info `{}` | {} |",
            a.name(), d.accent.hex(), l.accent.hex(), d.accent_text.hex(), l.accent_text.hex(),
            d.success.hex(), l.success.hex(), d.danger.hex(), d.info.hex(), badges.join(" ")
        );
    }
    for b in BackgroundPreset::ALL {
        let (d, l) = (Theme::build(&spec(AccentPreset::SageTeal, b), true), Theme::build(&spec(AccentPreset::SageTeal, b), false));
        println!(
            "| {} | bg `{}` surface `{}` raised `{}` border `{}` | bg `{}` surface `{}` border `{}` |",
            b.name(), d.bg.hex(), d.surface.hex(), d.surface_raised.hex(), d.border.hex(),
            l.bg.hex(), l.surface.hex(), l.border.hex()
        );
    }
}

#[test]
fn accent_wash_is_faint_and_keeps_text_readable() {
    for t in all_themes() {
        assert!(
            (0.05..=0.15).contains(&t.accent_wash.a),
            "{} accent_wash alpha {}",
            name(t),
            t.accent_wash.a
        );
        // Messages sit on `bg` or `surface`, never on a raised fill.
        for (sn, s) in surfaces(t).into_iter().take(2) {
            let under = crate::components::util::over(c(t.accent_wash), c(s));
            for (tn, fg) in [
                ("text", t.text),
                ("text_muted", t.text_muted),
                ("accent_text", t.accent_text),
            ] {
                let r = contrast(c(fg), under);
                assert!(
                    r >= 4.5,
                    "{} {tn} on accent_wash over {sn}: {r:.2}",
                    name(t)
                );
            }
        }
    }
}
