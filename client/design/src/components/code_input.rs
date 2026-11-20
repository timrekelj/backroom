//! CodeInput: a one-time code (6 digits by default) shown one character per cell. A single field
//! lies over the cells and takes the keys, so typing moves on to the next cell, Backspace steps
//! back and a pasted code fills every cell at once; the cells only draw what it holds. The caller
//! keeps the text to what a code may hold (`on_change`) and may submit once it is full.
//!
//! Cells are 40x48 (scaled with the text size), `radius-md`, on `surface_raised` with the control
//! edge; the cell the next character goes into takes the focus ring while the field is focused.

use rune::{el, row, text, text_input, Color, El, TextEdit, TextStyle, Tx};
use rune_kit::Kit;

use crate::tokens::{radius, type_};
use crate::{metrics_of, theme_of};

type Wire = Box<dyn FnOnce(El) -> El>;

pub struct CodeInput {
    edit: TextEdit,
    len: usize,
    label: String,
    invalid: bool,
    disabled: bool,
    wires: Vec<Wire>,
}

impl CodeInput {
    pub fn new(edit: &TextEdit) -> CodeInput {
        CodeInput {
            edit: edit.clone(),
            len: 6,
            label: "Code".into(),
            invalid: false,
            disabled: false,
            wires: Vec::new(),
        }
    }

    /// How many cells (6 by default).
    pub fn len(mut self, len: usize) -> Self {
        self.len = len.max(1);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// The cells take the danger edge (a wrong code).
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_edit<S: 'static>(
        mut self,
        f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    ) -> Self {
        self.wires.push(Box::new(move |e: El| e.on_edit(f)));
        self
    }

    /// After every change (typing, Backspace, paste): filter the text here.
    pub fn on_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.wires.push(Box::new(move |e: El| e.on_input_change(f)));
        self
    }

    /// Enter.
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.wires.push(Box::new(move |e: El| e.on_submit(f)));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let m = metrics_of(k);
        let (w, h, gap) = (m.em(40.0), m.em(48.0), 8.0);
        let font = m.font(TextStyle {
            size: 20.0,
            line_height: 28.0,
            weight: 500.0,
            ..type_::CODE
        });
        let chars: Vec<char> = self.edit.text.chars().collect();
        let focused = self.edit.focused && !self.disabled;
        let next = chars.len().min(self.len - 1);
        let mut cells = row().key("cells").gap(gap).a11y_hidden();
        for i in 0..self.len {
            let edge = if self.invalid {
                t.danger.color()
            } else if focused && i == next {
                k.c.border_focus
            } else {
                t.control_border.color()
            };
            let mut cell = row()
                .key(format!("cell{i}"))
                .size(w, h)
                .center()
                .radius(radius::MD)
                .fill(t.surface_raised.color())
                .border(if focused && i == next { 2.0 } else { 1.0 }, edge);
            if let Some(c) = chars.get(i) {
                cell = cell.child(text(c.to_string()).font(font).color(t.text.color()));
            }
            cells = cells.child(cell);
        }
        let total = w * self.len as f32 + gap * (self.len - 1) as f32;
        let mut field = text_input(&self.edit, font)
            .single_line()
            .key("input")
            .label(self.label.clone())
            .disabled(self.disabled)
            .color(Color::TRANSPARENT)
            .caret(Color::TRANSPARENT, Color::TRANSPARENT)
            .placeholder("", Color::TRANSPARENT)
            .absolute(0.0, 0.0)
            .w(total)
            .h(h);
        if !self.disabled {
            for wire in self.wires {
                field = wire(field);
            }
        }
        let e = el().size(total, h).child(cells).child(field);
        if self.disabled {
            e.opacity(0.5)
        } else {
            e
        }
    }
}

/// `raw` kept to digits, at most `len` of them: what a code field's change handler stores back.
pub fn digits_only(raw: &str, len: usize) -> String {
    raw.chars().filter(char::is_ascii_digit).take(len).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_only_strips_spaces_and_dashes_and_caps_the_length() {
        assert_eq!(digits_only(" 123-456 ", 6), "123456");
        assert_eq!(digits_only("1234567", 6), "123456");
        assert_eq!(digits_only("ab", 6), "");
    }
}
