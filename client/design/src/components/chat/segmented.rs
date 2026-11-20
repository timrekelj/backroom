//! Segmented: a one-of-N switch ("Log in" / "Sign up"). One container with one 1pt border on
//! `bg`, 32pt tall (the md control height); the selected segment is a filled `surface_raised`
//! tile with no border of its own. Fill and ink move on `motion::CONTROL`.

use std::rc::Rc;

use rune::{row, text, El, Tx};
use rune_kit::{Kit, State};

use crate::components::util::{paint, Surface};
use crate::theme_of;
use crate::tokens::{motion, radius, type_};

type Pick = Rc<dyn Fn(El, usize) -> El>;

pub struct Segmented {
    labels: Vec<String>,
    selected: usize,
    label: Option<String>,
    pick: Option<Pick>,
}

impl Segmented {
    pub fn new<S: AsRef<str>>(labels: &[S], selected: usize) -> Segmented {
        Segmented {
            labels: labels.iter().map(|l| l.as_ref().to_owned()).collect(),
            selected,
            label: None,
            pick: None,
        }
    }
    /// The accessible name of the choice ("Log in or sign up").
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    pub fn on_pick<S: 'static>(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        let f = Rc::new(f);
        self.pick = Some(Rc::new(move |e, i| {
            let f = f.clone();
            e.on_press(move |s: &mut S| f(s, i))
        }));
        self
    }

    /// Like `on_pick`, with the transaction: a pick that changes the look sets `tx.color_motion`.
    pub fn on_pick_tx<S: 'static>(mut self, f: impl Fn(&mut S, usize, &mut Tx) + 'static) -> Self {
        let f = Rc::new(f);
        self.pick = Some(Rc::new(move |e, i| {
            let f = f.clone();
            e.on_press_tx(move |s: &mut S, tx| f(s, i, tx))
        }));
        self
    }

    /// Fills its parent's width, in equal segments.
    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let mut root = row()
            .role("radiogroup")
            .focus_group()
            .w_full()
            .h(crate::metrics_of(k).control_h())
            .padding(2.0)
            .gap(2.0)
            .center_y()
            .radius(radius::MD)
            .fill(t.bg.color());
        if let Some(l) = self.label {
            root = root.label(l);
        }
        for (i, l) in self.labels.into_iter().enumerate() {
            let on = i == self.selected;
            let (fill, ink) = if on {
                (t.surface_raised.color(), t.text)
            } else {
                (rune::Color::TRANSPARENT, t.text_muted)
            };
            let seg = row()
                .key(format!("segment{i}"))
                .role("radio")
                .focus_selected(on)
                .label(l.clone())
                .grow(1.0)
                .shrink()
                .h_full()
                .center()
                .radius(radius::SM)
                .token_motion(motion::CONTROL)
                .child(
                    text(l)
                        .key("label")
                        .font(if on {
                            type_::at(k, type_::CONTROL_STRONG)
                        } else {
                            type_::at(k, type_::CONTROL)
                        })
                        .color(ink.color())
                        .truncate()
                        .a11y_hidden(),
                );
            let surface = Surface {
                fill,
                hover: if on {
                    rune::Color::TRANSPARENT
                } else {
                    t.hover.color()
                },
                pressed: t.pressed.color(),
            };
            let mut seg = paint(k, seg, surface, State::Rest, (2.0, 0.0));
            if let Some(p) = &self.pick {
                seg = p(seg, i);
            }
            root = root.child(seg);
        }
        root
    }
}
