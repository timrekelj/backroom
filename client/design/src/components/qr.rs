//! QrCode: a QR code drawn with elements, crisp at any scale. Scanners want dark modules on a
//! light ground, so it is always drawn in the light look's `text` on its `surface` (in the dark
//! look too: an inverted code fails in many authenticator apps), with a quiet zone of 3 modules
//! and `radius-md` corners. Each row is a strip of runs, so a 41-module code is a few hundred
//! elements, not 1,700.

use rune::{col, el, row, El};
use rune_kit::Kit;

use crate::theme;
use crate::tokens::radius;

/// Modules of light ground around the code.
const QUIET: usize = 3;

pub struct QrCode {
    data: String,
    module: f32,
    label: String,
}

impl QrCode {
    pub fn new(data: impl Into<String>) -> QrCode {
        QrCode {
            data: data.into(),
            module: 4.0,
            label: "QR code".into(),
        }
    }

    /// Points per module (4 by default).
    pub fn module(mut self, points: f32) -> Self {
        self.module = points;
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// The modules, row by row (`true` = dark); `None` when the data doesn't fit a QR code.
    pub fn matrix(data: &str) -> Option<(usize, Vec<bool>)> {
        let code = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::M)
            .ok()?;
        let width = code.width();
        let dark = code
            .to_colors()
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect();
        Some((width, dark))
    }

    pub fn build(self, _k: &Kit) -> El {
        let paper = theme(false);
        let Some((width, dark)) = Self::matrix(&self.data) else {
            return el().key("qr").size(0.0, 0.0);
        };
        let px = self.module;
        let side = (width + 2 * QUIET) as f32 * px;
        let mut grid = col().key("qr-modules").a11y_hidden();
        for y in 0..width {
            let line = &dark[y * width..(y + 1) * width];
            let mut strip = row().h(px);
            let mut x = 0;
            while x < width {
                let on = line[x];
                let run = line[x..].iter().take_while(|&&d| d == on).count();
                let cell = el().w(run as f32 * px).h(px);
                strip = strip.child(if on {
                    cell.fill(paper.text.color())
                } else {
                    cell
                });
                x += run;
            }
            grid = grid.child(strip);
        }
        col()
            .key("qr")
            .role("img")
            .label(self.label)
            .size(side, side)
            .center()
            .radius(radius::MD)
            .fill(paper.surface.color())
            .child(grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_otpauth_uri_makes_a_square_matrix_with_finder_corners() {
        let uri =
            "otpauth://totp/thehideout:tim%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=thehideout";
        let (w, m) = QrCode::matrix(uri).expect("fits");
        assert_eq!(m.len(), w * w);
        assert!(w >= 21 && (w - 17) % 4 == 0, "a QR version's width: {w}");
        // The finder pattern: a dark 7-module top row in the top-left corner.
        assert!(m[..7].iter().all(|&d| d));
        assert!(!m[7]);
    }
}
