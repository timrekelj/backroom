//! The semantic colour tokens for dark and light. Components read these names, never the ramp.
//!
//! Dark is a lamp-lit room: warm near-black, never pure black. Light is paper: warm off-white,
//! never pure white (the default Warm background). A theme is built from a `ThemeSpec` and the
//! resolved appearance: the accent preset's colours and status hues, the background preset's
//! neutral ramp, an optional accent tint on surfaces, and a contrast level that pushes text and
//! edges further from the surfaces.

use std::sync::Mutex;

use super::accent::Palette;
use super::color::{fit, Tone};
use super::spec::{Contrast, ThemeSpec};

macro_rules! theme {
    ($($(#[$m:meta])* $f:ident,)*) => {
        /// All colour tokens for one appearance.
        #[derive(Clone, Copy, Debug)]
        pub struct Theme {
            pub dark: bool,
            /// What this theme was built from (`appearance` is the resolved one).
            pub spec: ThemeSpec,
            /// Inset fills one, two and three steps from the page (the kit's layers).
            pub layers: [Tone; 3],
            /// Switch knob, off and on.
            pub knob: Tone,
            pub knob_on: Tone,
            $($(#[$m])* pub $f: Tone,)*
        }

        impl Theme {
            /// Every token as `(name, tone)` in declaration order, for docs and the gallery.
            pub fn tokens(&self) -> Vec<(&'static str, Tone)> {
                vec![$((stringify!($f), self.$f),)*]
            }
        }
    };
}

theme! {
    /// The window behind everything.
    bg,
    /// Panels and cards sitting on `bg`.
    surface,
    /// Inputs, raised controls, hovered rows.
    surface_raised,
    /// Menus, popovers, tooltips, dialogs.
    surface_overlay,
    /// Dividers inside a surface.
    border_subtle,
    /// Edges of panels, cards, secondary buttons.
    border,
    /// Edges of inputs and anything that must read as a control.
    border_strong,
    text,
    text_muted,
    text_faint,
    accent,
    /// Ink on `accent`.
    accent_fg,
    /// The accent as text or an icon on `bg` (contrast-safe).
    accent_text,
    /// A quiet accent wash behind accent text.
    accent_subtle,
    /// The accent as a faint translucent wash over any surface: a message that mentions you.
    accent_wash,
    success,
    success_bg,
    success_border,
    warning,
    warning_bg,
    warning_border,
    danger,
    danger_bg,
    danger_border,
    /// Solid danger fill (destructive buttons) and its ink.
    danger_solid,
    on_danger,
    info,
    info_bg,
    info_border,
    focus,
    /// Overlay laid over any surface on hover / press.
    hover,
    pressed,
    /// Overlays laid over the accent fill on hover / press.
    accent_hover,
    accent_pressed,
    scrim,
}

/// The theme for `spec` drawn `dark`, built once per distinct look (at most
/// `ThemeSpec::LOOKS`, so the cache is bounded).
pub fn theme_for(spec: &ThemeSpec, dark: bool) -> &'static Theme {
    static CACHE: Mutex<Vec<Option<&'static Theme>>> = Mutex::new(Vec::new());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.is_empty() {
        cache.resize(ThemeSpec::LOOKS, None);
    }
    cache[spec.look_index(dark)].get_or_insert_with(|| Box::leak(Box::new(build(spec, dark))))
}

/// The default look (`ThemeSpec::default()`) drawn `dark`: a convenience for code with no spec.
pub fn theme(dark: bool) -> &'static Theme {
    theme_for(&ThemeSpec::default(), dark)
}

/// How far a tinted surface moves toward the accent hue, as OKLab chroma added at full strength.
const TINT_C: f32 = 0.006;

impl Theme {
    /// Builds the theme without caching (tests, and the cache itself).
    pub fn build(spec: &ThemeSpec, dark: bool) -> Theme {
        build(spec, dark)
    }
}

fn build(spec: &ThemeSpec, dark: bool) -> Theme {
    let p: &Palette = spec.accent.palette();
    let a = p.accent(dark);
    let (sh, wh, ih, dh) = (p.success_h, p.warning_h, p.info_h, p.danger_h);
    let sc = p.success_c;
    let high = spec.contrast == Contrast::High;
    let ramp = spec.background.ramp(dark);
    // A tint scales with how coloured the accent is, so Graphite stays grey.
    let tint_c = if spec.tint {
        TINT_C * (a.fill.c / 0.10).min(1.2)
    } else {
        0.0
    };
    let tint = |t: Tone, k: f32| {
        if tint_c > 0.0 {
            t.nudged(a.fill.h, tint_c * k)
        } else {
            t
        }
    };
    // `pick(dark_step, light_step)` selects a ramp step per appearance.
    let pick = |d: usize, l: usize| ramp.step(if dark { d } else { l });
    let ink = pick(2, 13);
    let tone = |dk: (f32, f32, f32), lt: (f32, f32, f32)| {
        let (l, c, h) = if dark { dk } else { lt };
        Tone::new(l, c, h)
    };

    let bg = tint(pick(13, 1), 1.0);
    let surface = tint(pick(12, 0), 1.0);
    let surface_raised = tint(pick(11, 0), 1.0);
    let surface_overlay = tint(pick(10, 0), 1.0);
    let on3 = [bg, surface, surface_raised];
    let on4 = [bg, surface, surface_raised, surface_overlay];

    // Text and edges are authored for the default look; `fit` only moves one whose contrast falls
    // short on this background, tint or level, so the presets read as designed wherever they pass.
    let text = fit(ink, &on4, if high { 10.0 } else { 7.0 }, dark);
    let text_muted = fit(pick(5, 8), &on4, if high { 8.0 } else { 4.5 }, dark);
    let text_faint = fit(pick(6, 7), &on3, if high { 7.0 } else { 4.5 }, dark);
    let (min_sub, min_border, min_strong): (f32, f32, f32) = if high {
        (1.8, 3.0, 4.5)
    } else {
        (1.1, 1.3, 3.0)
    };
    let ratio = |t: Tone, s: &[Tone]| {
        s.iter()
            .map(|s| super::color::contrast(t.color(), s.color()))
            .fold(0.0, f32::max)
    };
    let border_subtle = fit(tint(pick(10, 3), 0.8), &on3, min_sub, dark);
    let border = fit(
        tint(pick(9, 4), 0.8),
        &on3,
        min_border.max(ratio(border_subtle, &on3) + 0.02),
        dark,
    );
    // Dark: L .57 keeps 3:1 (non-text) on bg, surface and surface_raised; the ramp's N8 gave ~2.5.
    let strong = if dark {
        ramp.lit(0.57, 0.8)
    } else {
        ramp.step(6)
    };
    let border_strong = fit(
        tint(strong, 0.5),
        &on3,
        min_strong.max(ratio(border, &on3) + 0.02),
        dark,
    );

    let accent_subtle = a.subtle;
    let accent_text = fit(
        a.text,
        &[bg, surface, surface_raised, accent_subtle],
        if high { 7.0 } else { 4.5 },
        dark,
    );
    let status =
        |fg: Tone, wash: Tone| fit(fg, &[bg, surface, wash], if high { 7.0 } else { 4.5 }, dark);
    let (success_bg, warning_bg, danger_bg, info_bg) = (
        tone((0.27, 0.040 * sc, sh + 5.0), (0.95, 0.035 * sc, sh + 5.0)),
        tone((0.28, 0.050, wh - 5.0), (0.95, 0.045, wh)),
        tone((0.27, 0.060, dh), (0.95, 0.030, dh)),
        tone((0.27, 0.030, ih), (0.95, 0.020, ih)),
    );
    // Overlays on the accent fill follow its ink: darken under dark ink, lighten under light ink.
    let veil = if a.fg.l > 0.5 {
        Tone::new(1.0, 0.0, 0.0)
    } else {
        Tone::new(0.0, 0.0, 0.0)
    };
    let layer = |i: usize| {
        tint(
            if dark {
                ramp.step(12 - i)
            } else {
                [ramp.step(0), ramp.step(2), ramp.step(3)][i]
            },
            0.8,
        )
    };
    Theme {
        dark,
        spec: ThemeSpec {
            appearance: if dark {
                super::spec::Appearance::Dark
            } else {
                super::spec::Appearance::Light
            },
            ..*spec
        },
        layers: [layer(0), layer(1), layer(2)],
        knob: ramp.step(if dark { 2 } else { 0 }),
        knob_on: ramp.step(0),
        bg,
        surface,
        surface_raised,
        surface_overlay,
        border_subtle,
        border,
        border_strong,
        text,
        text_muted,
        text_faint,
        accent: a.fill,
        accent_fg: a.fg,
        accent_text,
        accent_subtle,
        accent_wash: a.fill.alpha(if dark { 0.07 } else { 0.08 }),
        success: status(
            tone((0.78, 0.130 * sc, sh), (0.50, 0.120 * sc, sh + 5.0)),
            success_bg,
        ),
        success_bg,
        success_border: tone((0.40, 0.070 * sc, sh + 5.0), (0.82, 0.080 * sc, sh + 5.0)),
        warning: status(
            tone((0.82, 0.130, wh), (0.50, 0.100, wh - 15.0)),
            warning_bg,
        ),
        warning_bg,
        warning_border: tone((0.42, 0.080, wh - 5.0), (0.83, 0.090, wh - 2.0)),
        danger: status(tone((0.74, 0.150, dh), (0.52, 0.170, dh + 2.0)), danger_bg),
        danger_bg,
        danger_border: tone((0.42, 0.110, dh), (0.83, 0.080, dh)),
        danger_solid: tone((0.56, 0.180, dh + 2.0), (0.54, 0.190, dh + 2.0)),
        on_danger: Tone::new(0.985, 0.006, 75.0),
        info: status(tone((0.76, 0.090, ih), (0.50, 0.090, ih + 5.0)), info_bg),
        info_bg,
        info_border: tone((0.41, 0.060, ih), (0.82, 0.050, ih)),
        focus: accent_text,
        hover: text.alpha(if dark { 0.06 } else { 0.05 }),
        pressed: text.alpha(if dark { 0.10 } else { 0.09 }),
        accent_hover: veil.alpha(0.10),
        accent_pressed: veil.alpha(0.18),
        scrim: Tone::new(0.12, 0.01, 55.0).alpha(if dark { 0.60 } else { 0.38 }),
    }
}
