//! The `foundations` page: colour, type, spacing, radius, elevation and motion.

use rune::{El, col, el, row, text};
use rune_kit::Kit;
use thehideout_design::color::NEUTRAL_STEPS;
use thehideout_design::elevation::{LEVELS, Level, shadows};
use thehideout_design::{motion, radius, space, theme_of, type_};

use crate::ui::*;

pub fn page(k: &Kit) -> El {
    let left = col()
        .gap(20.0)
        .w(660.0)
        .child(section(k, "Neutral ramp", &format!("The {} background ramp in this appearance (Warm: hue 75 to 55, chroma 0.006 to 0.0105). Surfaces, borders and text pick from it.", theme_of(k).spec.background.name()), ramp(k)))
        .child(section(k, "Surfaces, borders, text", "Semantic tokens. Components read these names, never the ramp.", tokens(k, &SURFACES)))
        .child(section(k, &format!("Accent: {}", theme_of(k).spec.accent.name()), "One of eight presets (tokens/accent.rs); the Themes page shows them all.", tokens(k, &ACCENT)))
        .child(section(k, "Overlays", "Translucent: laid over any surface.", tokens(k, &OVERLAYS)))
        .child(section(k, "Status", "Text on `bg`, `surface` and its own wash clears 4.5:1.", status(k)));
    let right = col()
        .gap(20.0)
        .w(456.0)
        .child(section(
            k,
            "Type",
            "Instrument Sans for UI, JetBrains Mono for code. Size / line height, weight.",
            types(k),
        ))
        .child(section(k, "Spacing", "4pt scale.", spacing(k)))
        .child(section(
            k,
            "Radius",
            "OpenCode's scale, copied exactly. Dots and count pills only go round.",
            radii(k),
        ))
        .child(section(
            k,
            "Elevation",
            "Flat first: borders separate. Two shadow levels, for things that float.",
            elevation(k),
        ))
        .child(section(
            k,
            "Motion",
            "Rune's springs under our names (duration, bounce).",
            motions(k),
        ));
    col()
        .gap(24.0)
        .child(page_title(
            k,
            "Foundations",
            "thehideout design language: warm, minimal, crisp.",
        ))
        .child(row().gap(20.0).child(left).child(right))
}

const SURFACES: [&str; 10] = [
    "bg",
    "surface",
    "surface_raised",
    "surface_overlay",
    "border_subtle",
    "border",
    "border_strong",
    "text",
    "text_muted",
    "text_faint",
];
const ACCENT: [&str; 5] = [
    "accent",
    "accent_fg",
    "accent_text",
    "accent_subtle",
    "focus",
];
const OVERLAYS: [&str; 5] = [
    "hover",
    "pressed",
    "accent_hover",
    "accent_pressed",
    "scrim",
];

fn ramp(k: &Kit) -> El {
    let mut r = row().gap(4.0);
    let mut r2 = row().gap(4.0);
    for i in 0..NEUTRAL_STEPS {
        let tone = theme_of(k).spec.background.ramp(theme_of(k).dark).step(i);
        let cell = col()
            .gap(4.0)
            .child(chip(k, tone.color(), 60.0, 44.0))
            .child(code(k, format!("N{i}")))
            .child(mono_muted(k, tone.hex()));
        if i < 8 {
            r = r.child(cell)
        } else {
            r2 = r2.child(cell)
        }
    }
    col().gap(12.0).child(r).child(r2)
}

fn tokens(k: &Kit, names: &[&str]) -> El {
    let t = theme_of(k);
    let all = t.tokens();
    let mut list = row().flex_wrap().gap(12.0);
    for n in names {
        let tone = all
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, v)| *v)
            .expect("token exists");
        list = list.child(token_row_w(k, n, tone));
    }
    list
}

fn token_row_w(k: &Kit, n: &str, tone: thehideout_design::color::Tone) -> El {
    token_row(k, n, tone).w(300.0)
}

fn status(k: &Kit) -> El {
    let t = theme_of(k);
    let mut g = row().flex_wrap().gap(12.0);
    for (name, fg, bg, border, solid) in [
        ("success", t.success, t.success_bg, t.success_border, None),
        ("warning", t.warning, t.warning_bg, t.warning_border, None),
        (
            "danger",
            t.danger,
            t.danger_bg,
            t.danger_border,
            Some((t.danger_solid, t.on_danger)),
        ),
        ("info", t.info, t.info_bg, t.info_border, None),
    ] {
        let mut c = col()
            .gap(8.0)
            .w(300.0)
            .padding(12.0)
            .radius(radius::MD)
            .fill(bg.color())
            .border(1.0, border.color())
            .child(
                text(format!("{name}: the build finished"))
                    .font(type_::CONTROL_STRONG)
                    .color(fg.color()),
            )
            .child(mono_muted_on(k, format!("{name}  {}", fg.hex()), fg))
            .child(mono_muted_on(k, format!("{name}_bg  {}", bg.hex()), fg))
            .child(mono_muted_on(
                k,
                format!("{name}_border  {}", border.hex()),
                fg,
            ));
        if let Some((s, on)) = solid {
            c = c.child(
                row()
                    .gap(8.0)
                    .center_y()
                    .child(
                        row()
                            .px(8.0)
                            .h(22.0)
                            .center()
                            .radius(radius::SM)
                            .fill(s.color())
                            .child(
                                text("on_danger".to_owned())
                                    .font(type_::LABEL)
                                    .color(on.color()),
                            ),
                    )
                    .child(mono_muted_on(k, format!("danger_solid  {}", s.hex()), fg)),
            );
        }
        g = g.child(c);
    }
    g
}

fn mono_muted_on(_k: &Kit, s: String, ink: thehideout_design::color::Tone) -> El {
    text(s)
        .font(rune::TextStyle {
            size: 11.0,
            ..type_::CODE
        })
        .color(ink.color())
}

fn types(k: &Kit) -> El {
    let mut c = col().gap(10.0);
    for (name, style) in type_::SCALE {
        let sample = if name == "code" {
            "let id = Uuid::now_v7();"
        } else {
            "Warm light, crisp edges"
        };
        let spec = format!(
            "{name}\n{}/{} w{}",
            style.size, style.line_height, style.weight
        );
        c = c.child(
            row()
                .gap(12.0)
                .center_y()
                .child(
                    col()
                        .w(104.0)
                        .child(code(k, name.to_owned()))
                        .child(mono_muted(
                            k,
                            spec.split('\n').nth(1).unwrap_or("").to_owned(),
                        )),
                )
                .child(
                    text(sample.to_owned())
                        .font(style)
                        .color(theme_of(k).text.color()),
                ),
        );
    }
    c
}

fn spacing(k: &Kit) -> El {
    let t = theme_of(k);
    let mut c = col().gap(4.0);
    for (name, v) in space::SCALE {
        c = c.child(
            row()
                .gap(12.0)
                .center_y()
                .child(row().w(84.0).child(code(k, name.to_owned())))
                .child(el().w(v).h(12.0).radius(radius::XS).fill(t.accent.color()))
                .child(mono_muted(k, format!("{v}"))),
        );
    }
    c
}

fn radii(k: &Kit) -> El {
    let t = theme_of(k);
    let mut r = row().gap(10.0);
    for (name, v) in radius::SCALE {
        r = r.child(
            col()
                .gap(6.0)
                .child(
                    el().size(56.0, 56.0)
                        .radius(v)
                        .fill(t.surface_raised.color())
                        .border(1.0, t.border_strong.color()),
                )
                .child(code(k, name.trim_start_matches("radius-").to_owned()))
                .child(mono_muted(k, format!("{v}"))),
        );
    }
    r.child(
        col()
            .gap(6.0)
            .child(
                el().size(56.0, 56.0).center().child(
                    el().size(24.0, 16.0)
                        .radius(radius::FULL)
                        .fill(t.accent.color()),
                ),
            )
            .child(code(k, "full".to_owned()))
            .child(mono_muted(k, "pills".to_owned())),
    )
}

fn elevation(k: &Kit) -> El {
    let t = theme_of(k);
    let mut r = row().gap(20.0).padding(12.0);
    for (name, level) in LEVELS {
        let mut card = el()
            .size(120.0, 76.0)
            .center()
            .radius(radius::LG)
            .fill(if level == Level::Flat {
                t.surface.color()
            } else {
                t.surface_overlay.color()
            })
            .border(1.0, t.border.color());
        for s in shadows(level, k.dark) {
            card = card.box_shadow(s);
        }
        r = r.child(col().gap(8.0).child(card.child(code(k, name.to_owned()))));
    }
    r
}

fn motions(k: &Kit) -> El {
    let mut c = col().gap(6.0);
    for (name, s) in motion::ALL {
        c = c.child(
            row()
                .gap(12.0)
                .child(row().w(150.0).child(code(k, name.to_owned())))
                .child(mono_muted(
                    k,
                    format!("{:.2}s  bounce {:.2}", s.duration, s.bounce),
                )),
        );
    }
    c
}
