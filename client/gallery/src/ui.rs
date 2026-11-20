//! Layout helpers for the gallery pages: text in the design language's own styles, cards,
//! sections and labelled grids. Everything reads `thehideout_design` tokens.

use rune::{Color, El, col, row, text};
use rune_kit::Kit;
use thehideout_design::color::Tone;
use thehideout_design::type_::{CAPTION, CODE, HEADING, LABEL, TITLE};
use thehideout_design::{radius, theme_of};

pub fn muted(k: &Kit, s: impl Into<String>) -> El {
    text(s.into())
        .font(CAPTION)
        .color(theme_of(k).text_muted.color())
}

/// A wrapping caption for section notes.
pub fn note(k: &Kit, s: impl Into<String>) -> El {
    muted(k, s).wrap()
}

pub fn label(k: &Kit, s: impl Into<String>) -> El {
    text(s.into()).font(LABEL).color(theme_of(k).text.color())
}

pub fn code(k: &Kit, s: impl Into<String>) -> El {
    text(s.into())
        .font(rune::TextStyle { size: 11.5, ..CODE })
        .color(theme_of(k).text.color())
}

pub fn mono_muted(k: &Kit, s: impl Into<String>) -> El {
    text(s.into())
        .font(rune::TextStyle { size: 11.0, ..CODE })
        .color(theme_of(k).text_muted.color())
}

/// A bordered panel on `surface`.
pub fn card(k: &Kit) -> El {
    let t = theme_of(k);
    col()
        .gap(16.0)
        .padding(20.0)
        .radius(radius::LG)
        .fill(t.surface.color())
        .border(1.0, t.border_subtle.color())
}

/// A card with a title and an optional one-line note.
pub fn section(k: &Kit, title: &str, note_text: &str, body: El) -> El {
    let head = col().gap(2.0).child(
        text(title.to_owned())
            .font(TITLE)
            .color(theme_of(k).text.color()),
    );
    let head = if note_text.is_empty() {
        head
    } else {
        head.child(note(k, note_text))
    };
    card(k).child(head).child(body)
}

pub fn page_title(k: &Kit, title: &str, note_text: &str) -> El {
    col()
        .gap(4.0)
        .child(
            text(title.to_owned())
                .font(HEADING)
                .color(theme_of(k).text.color()),
        )
        .child(muted(k, note_text))
}

/// A swatch square with a hairline edge so near-bg tones stay visible.
pub fn chip(k: &Kit, c: Color, w: f32, h: f32) -> El {
    rune::el()
        .size(w, h)
        .radius(radius::SM)
        .fill(c)
        .border(1.0, theme_of(k).border.color())
}

/// `name`, hex and oklch beside a swatch.
pub fn token_row(k: &Kit, name: &str, tone: Tone) -> El {
    row()
        .gap(10.0)
        .center_y()
        .w(340.0)
        .child(chip(k, tone.color(), 32.0, 32.0))
        .child(
            col()
                .child(code(k, name.to_owned()))
                .child(mono_muted(k, format!("{}  {}", tone.hex(), tone.css()))),
        )
}

/// A labelled grid: `cols` headings across, `rows` headings down, `cell(r, c)` between.
pub fn grid(
    k: &Kit,
    head_w: f32,
    col_w: f32,
    rows: &[&str],
    cols: &[&str],
    cell: impl Fn(usize, usize) -> El,
) -> El {
    let mut header = row().gap(8.0).child(rune::el().w(head_w));
    for c in cols {
        header = header.child(row().w(col_w).child(muted(k, (*c).to_owned())));
    }
    let mut g = col().gap(12.0).child(header);
    for (r, name) in rows.iter().enumerate() {
        let mut line = row()
            .gap(8.0)
            .center_y()
            .child(row().w(head_w).child(label(k, (*name).to_owned())));
        for c in 0..cols.len() {
            line = line.child(row().w(col_w).center_y().child(cell(r, c)));
        }
        g = g.child(line);
    }
    g
}
