//! Composer: the message box. A "+" button, a multi-line input that grows to eight lines and
//! then scrolls, a smiley and Send. Enter sends, Shift+Enter breaks the line (Rune's multi-line
//! field does that); empty or whitespace-only text never sends.
//!
//! Sending takes the trimmed text, clears the field in the same commit (no animation, so the
//! field is empty the instant you press) and hands the text to `send`. Send is a Primary
//! IconButton while there is text and a disabled Ghost while there is none; the fill moves
//! between them on `motion::CONTROL`. Pressing it squeezes it 4% (`press_scale`, Rune's press
//! spring). With `.disabled_reason("Reconnecting\u{2026}")` the box shows the reason instead
//! of a field and every button is disabled. The emoji button is keyed `<key>-emoji` so the app can
//! anchor its picker on it.

use std::rc::Rc;

use rune::{El, TextEdit, TextStyle, Tx, auto_grow, col, el, row, text, text_input};
use rune_kit::Kit;

use crate::components::{IconButton, Size, Variant};
use crate::icons::{PLUS, SEND, SMILEY};
use crate::theme_of;
use crate::tokens::{motion, radius, type_};

/// Lines the input grows to before it scrolls.
pub const MAX_LINES: usize = 8;

type Applier = Box<dyn FnOnce(El) -> El>;
/// Builds the field's and the Send button's handlers once the card knows whether empty text may send.
type Bind = Box<dyn FnOnce(bool) -> (Applier, Applier)>;

pub struct Composer {
    key: String,
    edit: TextEdit,
    placeholder: String,
    reason: Option<String>,
    bind: Option<Bind>,
    allow_empty: bool,
    tray: Option<El>,
    paste: Option<Applier>,
    add: Option<Applier>,
    emoji: Option<Applier>,
    field_key: Option<Applier>,
}

impl Composer {
    pub fn new(edit: &TextEdit) -> Composer {
        Composer {
            key: "composer".into(),
            edit: edit.clone(),
            placeholder: "Message".into(),
            reason: None,
            bind: None,
            allow_empty: false,
            tray: None,
            paste: None,
            add: None,
            emoji: None,
            field_key: None,
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
        self.bind = Some(Box::new(move |allow_empty| {
            let go = move |s: &mut S, tx: &mut Tx| {
                let text = edit(s).text.trim().to_owned();
                if text.is_empty() && !allow_empty {
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
            (field, button)
        }));
        self
    }
    /// Sends with an empty text too (the app has files waiting): the Send button is live and Enter
    /// sends. The app's `send` still decides whether there is anything to send.
    pub fn allow_empty(mut self, on: bool) -> Self {
        self.allow_empty = on;
        self
    }
    /// What sits above the input inside the card (the attachments tray).
    pub fn tray(mut self, tray: El) -> Self {
        self.tray = Some(tray);
        self
    }
    /// Sees a paste into the field with everything the clipboard held (an image, files): return
    /// true when it took the paste, so the field leaves it alone.
    pub fn on_paste<S: 'static>(mut self, f: impl Fn(&mut S, &rune::ClipboardContent, &mut Tx) -> bool + 'static) -> Self {
        self.paste = Some(Box::new(move |e| e.on_input_paste(f)));
        self
    }
    /// [`on_add`](Self::on_add) for a handler that needs the runtime (a file dialog).
    pub fn on_add_tx<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.add = Some(Box::new(move |e| e.on_press_tx(f)));
        self
    }
    pub fn on_add<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.add = Some(Box::new(move |e| e.on_press(f)));
        self
    }
    /// Sees keys before the field does (the app's autocomplete takes Tab, Enter, Escape and the
    /// arrows while its list is open): return true to consume one.
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &rune::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.field_key = Some(Box::new(move |e| e.on_input_key(f)));
        self
    }
    pub fn on_emoji<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.emoji = Some(Box::new(move |e| e.on_press(f)));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let blocked = self.reason.is_some();
        let has_text = !self.edit.text.trim().is_empty() || self.allow_empty;
        let m = crate::metrics_of(k);
        let style: TextStyle = m.font(type_::BODY);
        let pad = (m.field_h() - style.line_px()) / 2.0;

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
                Some(bind) => {
                    let (f, b) = bind(self.allow_empty);
                    (Some(f), Some(b))
                }
                None => (None, None),
            };
            if let Some(f) = field_bind {
                field = f(field);
            }
            if let Some(f) = self.field_key {
                field = f(field);
            }
            if let Some(f) = self.paste {
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
                self.tray,
            );
        };
        finish(
            k, &self.key, false, mid, None, self.add, self.emoji, false, blocked, pad, None,
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
    tray: Option<El>,
) -> El {
    let t = theme_of(k);
    let mut plus = IconButton::new(&PLUS, "Attach or add")
        .disabled(blocked)
        .build(k)
        .key(format!("{key}-add"));
    if let (Some(a), false) = (add, blocked) {
        plus = a(plus);
    }
    let mut smile = IconButton::new(&SMILEY, "Emoji").disabled(blocked).build(k).key(format!("{key}-emoji"));
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
    let line = row()
        .w_full()
        .padding(4.0)
        .gap(4.0)
        .items_end()
        .child(plus)
        .child(col().grow(1.0).shrink().min_w(0.0).py(pad).child(mid))
        .child(smile)
        .child(send);
    // The card is always a column with the tray's slot first (empty without a tray), so the
    // input keeps its place in the tree, and its focus, when the first chip appears.
    let slot = tray.unwrap_or_else(|| el().key("tray-slot"));
    let mut e = col()
        .child(slot)
        .child(line)
        .key(key.to_owned())
        .role("group")
        .label("Message composer")
        .w_full()
        .radius(radius::LG)
        .fill(t.surface_raised.color())
        .border(1.0, t.control_border.color())
        .token_motion(motion::CONTROL);
    if focused {
        e = e.outline(1.0, -1.0, t.focus.color());
    }
    if blocked {
        e = e.fill(t.surface.color());
    }
    e
}
