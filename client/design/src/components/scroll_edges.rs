//! ScrollEdges: the only divider a panel has. A 1pt `border_subtle` hairline under the header that
//! shows while content is scrolled underneath it, and one above the footer while more content lies
//! below. Wrap any scroll viewport; at rest (top, or bottom) the line is hidden, so a panel has no
//! permanent rules and the edge appears exactly when something is cut off.
//!
//! The viewport reports its offset with `El::on_scroll_offset` in the frame it draws, and the
//! handler only sets two `Live<f32>` opacities (0 or 1) that the lines are bound to through
//! `live_opacity`, smoothed on `motion::CONTROL`. A scroll therefore runs no view and commits
//! nothing; the fade is the spring, never a pop. Keep one `ScrollEdges` per viewport for its life
//! (in app state): a rebuilt view binds the same lines again.

use rune::{col, el, El, Live, ScrollPos};
use rune_kit::Kit;

use crate::motion;
use crate::theme_of;

/// A viewport counts as scrolled once it moved more than this far from an end.
const SLACK: f32 = 0.5;

/// The two edge opacities of one scroll viewport. Clones share the same values.
#[derive(Clone)]
pub struct ScrollEdges {
    top: Live<f32>,
    bottom: Live<f32>,
}

impl Default for ScrollEdges {
    fn default() -> Self {
        ScrollEdges::new()
    }
}

impl ScrollEdges {
    pub fn new() -> ScrollEdges {
        ScrollEdges {
            top: Live::new(0.0),
            bottom: Live::new(0.0),
        }
    }

    /// The top line's target: 1 while content is scrolled under the header, else 0.
    pub fn top(&self) -> f32 {
        self.top.get()
    }

    /// The bottom line's target: 1 while there is more content below the viewport, else 0.
    pub fn bottom(&self) -> f32 {
        self.bottom.get()
    }

    /// What the two lines should read for a viewport at `at`: `(top, bottom)`, each 0 or 1.
    pub fn of(at: ScrollPos) -> (f32, f32) {
        let [_, y] = at.offset;
        let [_, max] = at.max;
        (f32::from(y > SLACK), f32::from(max - y > SLACK))
    }

    /// Wraps `scroller` (a scroll viewport) in a box that fills the space the caller gives it, with
    /// the two hairlines pinned over its top and bottom edges. Pass the box where the viewport
    /// would have gone, between a header and a footer.
    pub fn wrap(&self, k: &Kit, scroller: El) -> El {
        let t = theme_of(k);
        let (top, bottom) = (self.top.clone(), self.bottom.clone());
        let scroller = scroller.w_full().h_full().on_scroll_offset(move |at| {
            let (a, b) = ScrollEdges::of(at);
            top.set(a);
            bottom.set(b);
        });
        let line = |key: &str, live: &Live<f32>| {
            el().key(key.to_owned())
                .h(1.0)
                .fill(t.border_subtle.color())
                .live_opacity(live.smooth(motion::CONTROL))
                .inert()
                .a11y_hidden()
        };
        col()
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .child(scroller)
            .child(line("edge-top", &self.top).pin(Some(0.0), Some(0.0), Some(0.0), None))
            .child(line("edge-bottom", &self.bottom).pin(Some(0.0), None, Some(0.0), Some(0.0)))
    }
}

/// `scroller` with scroll-edge hairlines (see [`ScrollEdges`]); the shorthand for
/// `edges.wrap(k, scroller)`.
pub fn scroll_edges(k: &Kit, edges: &ScrollEdges, scroller: El) -> El {
    edges.wrap(k, scroller)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(offset: f32, max: f32) -> ScrollPos {
        let mut p = ScrollPos::default();
        p.offset = [0.0, offset];
        p.max = [0.0, max];
        p
    }

    #[test]
    fn the_top_line_shows_once_scrolled_and_the_bottom_line_until_the_end() {
        assert_eq!(ScrollEdges::of(at(0.0, 400.0)), (0.0, 1.0), "at the top");
        assert_eq!(ScrollEdges::of(at(120.0, 400.0)), (1.0, 1.0), "in the middle");
        assert_eq!(ScrollEdges::of(at(400.0, 400.0)), (1.0, 0.0), "at the end");
        assert_eq!(ScrollEdges::of(at(0.0, 0.0)), (0.0, 0.0), "nothing to scroll");
        assert_eq!(ScrollEdges::of(at(0.2, 400.2)), (0.0, 1.0), "sub-point drift is not a scroll");
    }

    #[test]
    fn clones_share_one_value() {
        let a = ScrollEdges::new();
        let b = a.clone();
        a.top.set(1.0);
        assert_eq!(b.top(), 1.0);
        assert_eq!(b.bottom(), 0.0);
    }
}
