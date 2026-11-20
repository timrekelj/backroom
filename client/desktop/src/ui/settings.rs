//! The settings page: a full-window view that replaces the shell, a nav on the left (Appearance
//! is the only page for now) and the page on an inset panel, laid out like the main screen.
//!
//! Every control applies at once. Each press asks Rune for its calm colour spring
//! (`tx.color_motion`), so a change of look is colours crossing and nothing else moving. Presets
//! only, no free colour: every combination is contrast-tested in the design crate.

use rune::{col, el, row, text, Color, El, ScrollSpec, Tx};
use rune_kit::Kit;
use thehideout_design::components::{
    switch, IconButton, Kbd, NavRow, Segmented, ThemePreview, HEADER_H,
};
use thehideout_design::icons::{self, X};
use thehideout_design::{
    motion, radius, theme_for, theme_of, type_, AccentPreset, Appearance, BackgroundPreset, Contrast,
    ThemeSpec,
};

use crate::app::App;
use crate::look::{Look, ReduceMotion};
use crate::state::Section;

/// Space between the window edge and the panel, as on the main screen.
const INSET: f32 = 8.0;
const NAV_W: f32 = 240.0;
/// The page's text column.
pub(super) const PAGE_W: f32 = 760.0;

/// Applies `f` to the look inside a press: queues the save and asks for the calm colour spring.
fn change(a: &mut App, tx: &mut Tx, f: impl FnOnce(&mut Look)) {
    tx.color_motion(motion::CALM);
    a.act(|s| s.edit_look(f));
}

pub fn build(a: &App, k: &Kit, section: Section, bar: f32) -> El {
    let page = match section {
        Section::Appearance => appearance(a, k),
    };
    frame(a, k, bar, nav(k, section), page)
}

/// The page shell both settings screens share: `nav` on the left, `page` on the inset panel.
pub(super) fn frame(a: &App, k: &Kit, bar: f32, nav: El, page: El) -> El {
    let t = theme_of(k);
    let panel = col()
        .key("settings-panel")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .fill(t.surface.color())
        .border(1.0, t.border.color())
        .radius(radius::LG)
        .clip_rounded(radius::LG)
        .child(a.state.ui.edges.page.wrap(
            k,
            col()
                .key("settings-scroll")
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(page),
        ));
    let body = row()
        .key("settings-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .pt(INSET)
        .pb(INSET)
        .child(nav)
        .child(col().grow(1.0).min_w(0.0).h_full().pr(INSET).child(panel));
    col()
        .key("settings")
        .w_full()
        .h_full()
        .fill(t.bg.color())
        .pt(bar)
        .child(body)
}

/// The left column: a title as tall as the sidebar's header, then one row per page.
fn nav(k: &Kit, section: Section) -> El {
    nav_shell(
        k,
        "Settings",
        vec![
            NavRow::new("Appearance", &icons::SUN)
                .selected(section == Section::Appearance)
                .highlight_group("settings-nav")
                .on_press(|a: &mut App| a.act(|s| s.ui.settings = Some(Section::Appearance)))
                .build(k),
        ],
    )
}

/// A nav column: `title` as tall as the sidebar's header, then `rows`.
pub(super) fn nav_shell(k: &Kit, title: &str, rows: Vec<El>) -> El {
    let t = theme_of(k);
    col()
        .key("settings-nav")
        .w(NAV_W)
        .h_full()
        .fill(t.bg.color())
        .child(
            row()
                .w_full()
                .h(HEADER_H)
                .px(16.0)
                .center_y()
                .child(text(title.to_owned()).font(type_::TITLE).color(t.text.color()).max_lines(1)),
        )
        .child(col().w_full().px(8.0).gap(2.0).children(rows))
}

/// A page's title with the Esc hint and the close button.
pub(super) fn header(k: &Kit, title: &str, close: impl Fn(&mut App) + 'static) -> El {
    let t = theme_of(k);
    row()
        .w_full()
        .h(HEADER_H)
        .gap(8.0)
        .center_y()
        .child(
            text(title.to_owned())
                .font(type_::HEADING)
                .color(t.text.color())
                .grow(1.0),
        )
        .child(Kbd::new(&["Esc"]).build(k))
        .child(
            IconButton::new(&X, "Close settings")
                .on_press(close)
                .build(k)
                .key("settings-close"),
        )
}

fn appearance(a: &App, k: &Kit) -> El {
    let t = theme_of(k);
    let look = a.state.ui.look;
    let spec = look.spec;
    let dark = t.dark;

    let appearance = Segmented::new(
        &Appearance::ALL.map(Appearance::name),
        Appearance::ALL.iter().position(|p| *p == spec.appearance).unwrap_or(0),
    )
    .label("Appearance")
    .on_pick_tx(|a: &mut App, i, tx| change(a, tx, |l| l.spec.appearance = Appearance::ALL[i]))
    .build(k);

    let mut accents = row().key("set-accent").flex_wrap().gap(8.0);
    for preset in AccentPreset::ALL {
        accents = accents.child(accent_swatch(k, spec, dark, preset));
    }
    let mut backgrounds = row().key("set-background").flex_wrap().gap(12.0);
    for preset in BackgroundPreset::ALL {
        backgrounds = backgrounds.child(background_tile(k, spec, dark, preset));
    }

    let tint = switch(spec.tint)
        .label("Tint surfaces with accent")
        .hide_label()
        .on_toggle_tx(|a: &mut App, tx| change(a, tx, |l| l.spec.tint = !l.spec.tint))
        .build(k);
    let contrast = Segmented::new(
        &Contrast::ALL.map(Contrast::name),
        Contrast::ALL.iter().position(|p| *p == spec.contrast).unwrap_or(0),
    )
    .label("Contrast")
    .on_pick_tx(|a: &mut App, i, tx| change(a, tx, |l| l.spec.contrast = Contrast::ALL[i]))
    .build(k);
    let motion_seg = Segmented::new(
        &ReduceMotion::ALL.map(ReduceMotion::name),
        ReduceMotion::ALL.iter().position(|p| *p == look.reduce_motion).unwrap_or(0),
    )
    .label("Reduce motion")
    .on_pick_tx(|a: &mut App, i, tx| change(a, tx, |l| l.reduce_motion = ReduceMotion::ALL[i]))
    .build(k);

    let page = col()
        .key("page-appearance")
        .w_full()
        .max_w(PAGE_W)
        .px(32.0)
        .pb(32.0)
        .child(header(k, "Appearance", |a: &mut App| a.act(|s| s.close_settings())))
        .child(setting(k, "Appearance", "Follow the system, or always light or dark.", col().key("set-appearance").w(320.0).child(appearance)))
        .child(setting(k, "Accent", "Buttons, links, selection and focus.", accents))
        .child(setting(k, "Background", "The greys behind everything, shown in the current appearance.", backgrounds))
        .child(toggle(k, "Tint surfaces with accent", "A faint accent hue in backgrounds and borders.", col().key("set-tint").child(tint)))
        .child(setting(k, "Contrast", "High pushes muted text and borders further from the background.", col().key("set-contrast").w(240.0).child(contrast)))
        .child(setting(k, "Reduce motion", "System follows your operating system. On keeps changes to quick fades.", col().key("set-motion").w(240.0).child(motion_seg)));
    // The column is centred in the panel, whatever its width.
    col().w_full().items_center().child(page)
}

/// A title, its description and the control; settings are told apart by spacing, not rules.
pub(super) fn setting(k: &Kit, title: &str, description: &str, control: El) -> El {
    col()
        .w_full()
        .gap(12.0)
        .py(14.0)
        .child(labels(k, title, description))
        .child(control)
}

/// Like `setting`, with the control to the right of the text.
pub(super) fn toggle(k: &Kit, title: &str, description: &str, control: El) -> El {
    row()
        .w_full()
        .gap(16.0)
        .py(14.0)
        .center_y()
        .child(labels(k, title, description).grow(1.0))
        .child(control)
}

pub(super) fn labels(k: &Kit, title: &str, description: &str) -> El {
    let t = theme_of(k);
    col()
        .gap(2.0)
        .child(text(title.to_owned()).font(type_::CONTROL_STRONG).color(t.text.color()))
        .child(
            text(description.to_owned())
                .font(type_::CAPTION)
                .color(t.text_muted.color())
                .wrap(),
        )
}

/// A round swatch in the accent's fill (for the current appearance), its name under it.
fn accent_swatch(k: &Kit, spec: ThemeSpec, dark: bool, preset: AccentPreset) -> El {
    let t = theme_of(k);
    let fill = theme_for(&ThemeSpec { accent: preset, ..spec }, dark).accent;
    let selected = spec.accent == preset;
    let ring = if selected { t.text.color() } else { Color::TRANSPARENT };
    col()
        .key(format!("accent-{}", preset.name().to_lowercase().replace('-', "")))
        .role("radio")
        .label(preset.name())
        .focus_selected(selected)
        .w(76.0)
        .py(6.0)
        .gap(6.0)
        .items_center()
        .radius(radius::MD)
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .hover(t.hover.color())
        .press(t.pressed.color())
        .on_press_tx(move |a: &mut App, tx| change(a, tx, |l| l.spec.accent = preset))
        .child(
            el().size(36.0, 36.0)
                .radius(radius::FULL)
                .border(2.0, ring)
                .center()
                .child(el().size(26.0, 26.0).radius(radius::FULL).fill(fill.color())),
        )
        .child(
            text(preset.name().to_owned())
                .font(type_::CAPTION)
                .color(if selected { t.text } else { t.text_muted }.color())
                .a11y_hidden(),
        )
}

/// A miniature of the app on this background, its name under it.
fn background_tile(k: &Kit, spec: ThemeSpec, dark: bool, preset: BackgroundPreset) -> El {
    let t = theme_of(k);
    let selected = spec.background == preset;
    col()
        .key(format!("background-{}", preset.name().to_lowercase()))
        .role("radio")
        .label(preset.name())
        .focus_selected(selected)
        .w(104.0)
        .padding(4.0)
        .gap(6.0)
        .radius(radius::LG)
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .on_press_tx(move |a: &mut App, tx| change(a, tx, |l| l.spec.background = preset))
        .child(
            ThemePreview::new(ThemeSpec { background: preset, ..spec }, dark)
                .size(96.0, 60.0)
                .selected(selected)
                .build(),
        )
        .child(
            text(preset.name().to_owned())
                .font(type_::CAPTION)
                .color(if selected { t.text } else { t.text_muted }.color())
                .a11y_hidden(),
        )
}
