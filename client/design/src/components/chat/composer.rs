//! Composer: the message box. A "+" button, a multi-line input that grows to eight lines and
//! then scrolls, a smiley and Send. Enter sends, Shift+Enter breaks the line (Rune's multi-line
//! field does that); empty or whitespace-only text never sends.
//!
//! Sending takes the trimmed text, clears the field in the same commit (no animation, so the
//! field is empty the instant you press) and hands the text to `send`. Send is a Primary
//! IconButton while there is text and a disabled Ghost while there is none; the fill moves
//! between them on `motion::CONTROL`. Pressing it squeezes it 4% (`press_scale`, Rune's press
//! spring). With `.disabled_reason("Reconnecting\u{2026}")` the box shows the reason instead
//! of a field and every button is disabled.

use std::rc::Rc;

use rune::{El, TextEdit, TextStyle, Tx, auto_grow, col, row, text, text_input};
use rune_kit::Kit;

use crate::components::{IconButton, Size, Variant};
use crate::icons::{PLUS, SEND, SMILEY};
use crate::theme_of;
use crate::tokens::{motion, radius, type_};

/// Lines the input grows to before it scrolls.
pub const MAX_LINES: usize = 8;

type Applier = Box<dyn FnOnce(El) -> El>;

pub struct Composer {
    key: String,
    edit: TextEdit,
    placeholder: String,
    reason: Option<String>,
    bind: Option<(Applier, Applier, Applier)>,
    add: Option<Applier>,
    emoji: Option<Applier>,
}

impl Composer {
    pub fn new(edit: &TextEdit) -> Composer {
        Composer {
            key: "composer".into(),
            edit: edit.clone(),
            placeholder: "Message".into(),
            reason: None,
            bind: None,
            add: None,
            emoji: None,
        }
    }
    /// The root key; give each composer in a window its own (default "composer").
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }
    /// "Message #general".
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// Replaces the field with `reason` and disables every button.
    pub fn disabled_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
    /// Wires the field to the app's state: `edit` finds the `TextEdit`, `send` gets the trimmed
    /// text after the field has been cleared. Both Enter and the Send button run it.
    pub fn bind<S: 'static>(
        mut self,
        edit: fn(&mut S) -> &mut TextEdit,
        send: impl Fn(&mut S, String, &mut Tx) + 'static,
    ) -> Self {
        let send = Rc::new(send);
        let go = move |s: &mut S, tx: &mut Tx| {
            let text = edit(s).text.trim().to_owned();
            if text.is_empty() {
                return;
            }
            edit(s).clear();
            send(s, text, tx);
        };
        let go = Rc::new(go);
        let (g1, g2) = (go.clone(), go);
        let field: Applier = Box::new(move |e| {
            e.on_edit(move |s: &mut S| edit(s))
                .on_submit(move |s: &mut S, tx| g1(s, tx))
        });
        let button: Applier = Box::new(move |e| e.on_press_tx(move |s: &mut S, tx| g2(s, tx)));
        let noop: Applier = Box::new(|e| e);
        self.bind = Some((field, button, noop));
        self
    }
    pub fn on_add<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.add = Some(Box::new(move |e| e.on_press(f)));
        self
    }
    pub fn on_emoji<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.emoji = Some(Box::new(move |e| e.on_press(f)));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let blocked = self.reason.is_some();
        let has_text = !self.edit.text.trim().is_empty();
        let style: TextStyle = type_::BODY;
        let pad = (32.0 - style.line_px()) / 2.0;

        let mid = if let Some(reason) = self.reason {
            row()
                .key("reason")
                .w_full()
                .h(style.line_px())
                .center_y()
                .child(text(reason).font(style).color(t.text_faint.color()))
        } else {
            let mut field = text_input(&self.edit, style)
                .key("input")
                .w_full()
                .color(t.text.color())
                .placeholder(self.placeholder.clone(), t.text_faint.color())
                .caret(t.accent_text.color(), t.accent_subtle.color())
                .label(self.placeholder.clone());
            let (field_bind, send_bind) = match self.bind {
                Some((f, b, _)) => (Some(f), Some(b)),
                None => (None, None),
            };
            if let Some(f) = field_bind {
                field = f(field);
            }
            return finish(
                k,
                &self.key,
                self.edit.focused,
                auto_grow(field, 1, MAX_LINES),
                send_bind,
                self.add,
                self.emoji,
                has_text,
                blocked,
                pad,
            );
        };
        finish(
            k, &self.key, false, mid, None, self.add, self.emoji, false, blocked, pad,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    k: &Kit,
    key: &str,
    focused: bool,
    mid: El,
    send_bind: Option<Applier>,
    add: Option<Applier>,
    emoji: Option<Applier>,
    has_text: bool,
    blocked: bool,
    pad: f32,
) -> El {
    let t = theme_of(k);
    let mut plus = IconButton::new(&PLUS, "Attach or add")
        .disabled(blocked)
        .build(k);
    if let (Some(a), false) = (add, blocked) {
        plus = a(plus);
    }
    let mut smile = IconButton::new(&SMILEY, "Emoji").disabled(blocked).build(k);
    if let (Some(a), false) = (emoji, blocked) {
        smile = a(smile);
    }
    let live = has_text && !blocked;
    let mut send = IconButton::new(&SEND, "Send message")
        .variant(if live {
            Variant::Primary
        } else {
            Variant::Ghost
        })
        .size(Size::Md)
        .disabled(!live)
        .build(k);
    send = send.key("send").token_motion(motion::CONTROL);
    if let (Some(b), true) = (send_bind, live) {
        send = b(send).press_scale(0.96);
    }
    let mut e = row()
        .key(key.to_owned())
        .role("group")
        .label("Message composer")
        .w_full()
        .padding(4.0)
        .gap(4.0)
        .items_end()
        .radius(radius::LG)
        .fill(t.surface_raised.color())
        .border(1.0, t.border_strong.color())
        .token_motion(motion::CONTROL)
        .child(plus)
        .child(col().grow(1.0).shrink().min_w(0.0).py(pad).child(mid))
        .child(smile)
        .child(send);
    if focused {
        e = e.outline(1.0, -1.0, t.focus.color());
    }
    if blocked {
        e = e.fill(t.surface.color());
    }
    e
}
