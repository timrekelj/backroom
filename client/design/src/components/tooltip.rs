//! Tooltip: `rune_kit::Tooltip` in its standard appearance, which our Kit gives an overlay
//! surface, a 1px border, `radius-sm` corners and the popover shadow.

use rune::El;
use rune_kit::{Kit, Placement, Tooltip, TooltipAppearance};

use super::kbd::Kbd;

/// A tooltip centred above `trigger`, 4pt clear of it. The app owns `open` (hover delay is its
/// `wake_after`). The content mounts in `layer`, which the view hands to `rune_kit::root`.
/// `keys` adds a shortcut chip. Leave room above the trigger: the tooltip is centred on it, so
/// a trigger nearer an edge than half the tooltip's width pushes it out of its container.
pub fn tooltip(
    k: &Kit,
    key: &str,
    trigger: El,
    label: &str,
    keys: &[&str],
    open: bool,
    layer: &mut Vec<El>,
) -> El {
    tooltip_at(k, key, trigger, label, keys, open, Placement::Top, layer)
}

/// [`tooltip`] with an explicit side, e.g. `Placement::Bottom` for triggers at the top of a window.
#[allow(clippy::too_many_arguments)]
pub fn tooltip_at(
    k: &Kit,
    key: &str,
    trigger: El,
    label: &str,
    keys: &[&str],
    open: bool,
    placement: Placement,
    layer: &mut Vec<El>,
) -> El {
    let mut t = Tooltip::new(key)
        .appearance(TooltipAppearance::Standard)
        .trigger(trigger)
        .text(k, label)
        .open(open)
        .placement(placement);
    if !keys.is_empty() {
        t = t.child(Kbd::new(keys).build(k));
    }
    t.build(k, layer)
}
