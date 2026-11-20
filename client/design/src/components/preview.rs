//! ThemePreview: a miniature of the app in one look (window, panel, two lines of text, an accent
//! pill), for the settings page's background tiles and the gallery's theme matrix. Built from
//! the spec directly, so a tile shows its own look whatever the surrounding one is.

use rune::{El, col, el, row};

use crate::radius;
use crate::tokens::spec::ThemeSpec;
use crate::tokens::theme::theme_for;

pub struct ThemePreview {
    spec: ThemeSpec,
    dark: bool,
    size: (f32, f32),
    selected: bool,
}

impl ThemePreview {
    /// `spec` drawn `dark`; 88x56 unless `.size` says otherwise.
    pub fn new(spec: ThemeSpec, dark: bool) -> ThemePreview {
        ThemePreview { spec, dark, size: (88.0, 56.0), selected: false }
    }
    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.size = (w, h);
        self
    }
    /// A 2pt accent ring instead of the hairline.
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    pub fn build(self) -> El {
        let t = theme_for(&self.spec, self.dark);
        let bar = |w: f32, c: rune::Color| el().w(w).h(4.0).radius(radius::XS).fill(c);
        let (w, h) = self.size;
        let edge = if self.selected { (2.0, t.accent_text) } else { (1.0, t.border_strong) };
        let panel = col()
            .grow(1.0)
            .gap(5.0)
            .padding(6.0)
            .radius(radius::SM)
            .fill(t.surface.color())
            .border(1.0, t.border.color())
            .child(bar(w * 0.34, t.text.color()))
            .child(bar(w * 0.52, t.text_muted.color()))
            .child(
                row().gap(4.0).child(
                    el().w(w * 0.2).h(8.0).radius(radius::XS).fill(t.accent.color()),
                ),
            );
        row()
            .size(w, h)
            .padding(6.0)
            .radius(radius::MD)
            .fill(t.bg.color())
            .border(edge.0, edge.1.color())
            .child(panel)
    }
}
