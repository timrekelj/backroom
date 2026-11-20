//! Motion: Rune's spring palette under thehideout names. Values are Rune's own
//! (`rune_motion::tokens`), so timing matches the rest of the framework.

use rune::Spring;

/// Pointer-tied feedback (press): barely a spring.
pub const PRESS: Spring = Spring::new(0.10, 0.0);
/// Controls changing state: switch knob, checkbox, colour shifts.
pub const CONTROL: Spring = Spring::new(0.20, 0.0);
/// Popovers, menus, tooltips opening and closing.
pub const OVERLAY: Spring = Spring::new(0.24, 0.0);
/// Disclosure, panes and anything that changes height.
pub const DISCLOSURE: Spring = Spring::new(0.30, 0.0);
/// Layout reflow: lists re-ordering, sidebars resizing.
pub const REFLOW: Spring = Spring::new(0.40, 0.0);
/// A change of look (accent, background, appearance): every colour crosses on the calm spring.
pub const CALM: Spring = REFLOW;
/// A small celebratory overshoot (reactions, new-message pop). Use sparingly.
pub const POP: Spring = Spring::new(0.25, 0.15);

pub const ALL: [(&str, Spring); 6] = [
    ("motion-press", PRESS),
    ("motion-control", CONTROL),
    ("motion-overlay", OVERLAY),
    ("motion-disclosure", DISCLOSURE),
    ("motion-reflow", REFLOW),
    ("motion-pop", POP),
];

pub const fn kit() -> rune_kit::Motion {
    rune_kit::Motion {
        control: CONTROL,
        overlay: OVERLAY,
        disclosure: DISCLOSURE,
    }
}
