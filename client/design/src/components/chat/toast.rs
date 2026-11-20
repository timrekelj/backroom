//! Toast: a small notice at the bottom centre, info / success / danger. The caller owns when it
//! shows and when it goes (auto-dismiss is a `wake_after` in the app). Mounting slides it up
//! 8pt while fading in on `motion::OVERLAY`; dismissing fades it out. [`Toast::host`] puts it
//! on its own inert layer so it never blocks the window beneath.

use rune::{El, SpringPair, Transition, col, icon, row, text};
use rune_kit::Kit;

use crate::icons::{CHECK, INFO, WARNING};
use crate::theme_of;
use crate::tokens::{icon as size, motion, radius, type_};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Danger,
}

pub struct Toast {
    kind: ToastKind,
    message: String,
}

impl Toast {
    pub fn new(kind: ToastKind, message: impl Into<String>) -> Toast {
        Toast {
            kind,
            message: message.into(),
        }
    }

    /// The notice itself, with its enter transition.
    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let (glyph, ink) = match self.kind {
            ToastKind::Info => (&INFO, t.info),
            ToastKind::Success => (&CHECK, t.success),
            ToastKind::Danger => (&WARNING, t.danger),
        };
        let enter = Transition::offset(0.0, 8.0)
            .combined(Transition::opacity())
            .springs(SpringPair::new(motion::OVERLAY, motion::CONTROL));
        let e = row()
            .role("status")
            .label(self.message.clone())
            .min_h(crate::metrics_of(k).em(36.0))
            .px(12.0)
            .gap(8.0)
            .center_y()
            .radius(radius::MD)
            .fill(t.surface_overlay.color())
            .border(1.0, t.border_subtle.color())
            .transition(enter)
            .child(icon(glyph, crate::metrics_of(k).icon(size::SM)).color(ink.color()).a11y_hidden())
            .child(
                text(self.message)
                    .font(type_::at(k, type_::CONTROL))
                    .color(t.text.color())
                    .a11y_hidden(),
            );
        k.elevate(e, &k.elevation.floating)
    }

    /// A full-window inert layer holding `toast` 24pt above the bottom edge, for the view's
    /// overlay list. Pass `None` to unmount (the toast fades out).
    pub fn host(k: &Kit, toast: Option<Toast>) -> El {
        let layer = col()
            .key("toast-host")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .justify_end()
            .items_center()
            .pb(24.0)
            .inert()
            .a11y_hidden();
        match toast {
            Some(t) => layer.child(t.build(k)),
            None => layer,
        }
    }
}
