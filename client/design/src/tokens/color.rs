//! Colour primitives: `Tone` (an OKLCH colour authored by number), the neutral `Ramp` every
//! surface, border and text colour is picked from, and the contrast `fit` that keeps tokens legible.

use rune::{Color, oklcha};

/// An OKLCH colour with alpha. Authored by number so the docs, the gallery and the tests all
/// read the same source of truth; `color()` converts for painting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub a: f32,
}

impl Tone {
    pub const fn new(l: f32, c: f32, h: f32) -> Tone {
        Tone { l, c, h, a: 1.0 }
    }

    pub const fn alpha(self, a: f32) -> Tone {
        Tone { a, ..self }
    }

    /// The colour as Rune paints it (linear sRGB; out-of-gamut values clamp).
    pub fn color(self) -> Color {
        oklcha(self.l, self.c, self.h, self.a)
    }

    /// `#rrggbb`, or `#rrggbbaa` when translucent.
    pub fn hex(self) -> String {
        let h = self.color().hex();
        if self.a >= 0.999 {
            h[..7].to_owned()
        } else {
            h
        }
    }

    /// `oklch(L C H)` as CSS writes it, with `/ A` when translucent.
    pub fn css(self) -> String {
        let base = format!("oklch({:.3} {:.3} {:.0}", self.l, self.c, self.h);
        if self.a >= 0.999 {
            format!("{base})")
        } else {
            format!("{base} / {:.2})", self.a)
        }
    }
}

/// Number of steps in a neutral ramp (`N0` lightest .. `N14` darkest).
pub const NEUTRAL_STEPS: usize = 15;

/// OKLab lightness of each step of the default (warm) ramp.
pub const NEUTRAL_L: [f32; NEUTRAL_STEPS] = [
    0.985, 0.965, 0.94, 0.90, 0.84, 0.74, 0.64, 0.54, 0.49, 0.38, 0.31, 0.26, 0.22, 0.185, 0.155,
];

/// A neutral ramp: 15 lightness steps whose chroma and hue drift linearly from the light end
/// (`c.0`, `h.0`) to the dark end (`c.1`, `h.1`). Every background preset is one ramp per
/// appearance (`background.rs`); surfaces, borders and text are picked from it by step.
#[derive(Clone, Copy, Debug)]
pub struct Ramp {
    pub l: [f32; NEUTRAL_STEPS],
    pub c: (f32, f32),
    pub h: (f32, f32),
}

impl Ramp {
    /// Step `i` (0 lightest .. 14 darkest).
    pub fn step(&self, i: usize) -> Tone {
        let i = i.min(NEUTRAL_STEPS - 1);
        self.lit(self.l[i], i as f32 / (NEUTRAL_STEPS - 1) as f32)
    }

    /// Lightness `l` with the chroma and hue found `t` (0..1) of the way down the ramp.
    pub fn lit(&self, l: f32, t: f32) -> Tone {
        Tone::new(
            l,
            self.c.0 + (self.c.1 - self.c.0) * t,
            self.h.0 + (self.h.1 - self.h.0) * t,
        )
    }
}

/// The default warm ramp: hue drifts from paper-yellow (75) at the light end to ember-brown (55)
/// at the dark end; chroma stays at 0.006 to 0.0105, so greys read warm but never brown.
pub const WARM_RAMP: Ramp = Ramp {
    l: NEUTRAL_L,
    c: (0.006, 0.0105),
    h: (75.0, 55.0),
};

/// One step of the warm neutral ramp.
pub fn neutral(step: usize) -> Tone {
    WARM_RAMP.step(step)
}

impl Tone {
    /// Pushes the colour toward hue `h` by `dc` of chroma (added in OKLab, so a grey takes the
    /// hue and a tinted tone shifts toward it).
    pub fn nudged(self, h: f32, dc: f32) -> Tone {
        let (a, b) = (self.c * self.h.to_radians().cos(), self.c * self.h.to_radians().sin());
        let (a, b) = (a + dc * h.to_radians().cos(), b + dc * h.to_radians().sin());
        Tone {
            c: a.hypot(b),
            h: b.atan2(a).to_degrees().rem_euclid(360.0),
            ..self
        }
    }

    /// The tone with its chroma reduced until sRGB can show it.
    pub fn in_gamut(mut self) -> Tone {
        for _ in 0..40 {
            let lab = self.color().to_oklab();
            if (lab[1].hypot(lab[2]) - self.c).abs() < 0.004 {
                break;
            }
            self.c *= 0.95;
        }
        self
    }
}

/// Moves `tone`'s lightness (toward lighter or darker) just far enough that its contrast against
/// every colour in `against` is at least `min`. Untouched when it already is.
pub fn fit(tone: Tone, against: &[Tone], min: f32, lighten: bool) -> Tone {
    let ok = |t: Tone| against.iter().all(|a| contrast(t.color(), a.color()) >= min);
    let mut t = tone;
    if ok(t) {
        return t;
    }
    let step = if lighten { 0.004 } else { -0.004 };
    // Test the clipped colour: easing chroma into gamut shifts luminance a little.
    while (0.0..=1.0).contains(&(t.l + step)) {
        t.l += step;
        let shown = t.in_gamut();
        if ok(shown) {
            return shown;
        }
    }
    t.in_gamut()
}

/// WCAG relative-luminance contrast ratio between two opaque colours.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (a.luminance(), b.luminance());
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
