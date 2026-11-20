//! SidebarSection: a collapsible group of rows under a small header. The chevron turns a
//! quarter on `motion::DISCLOSURE`; the rows fade in and out on the same spring while what is
//! below them moves to make room. A "+" IconButton shows on hover (or with `.show_add(true)`).

use std::f32::consts::FRAC_PI_2;

use rune::{col, el, icon, row, text, El, SpringPair, Transition};
use rune_kit::Kit;

use crate::components::util::Acts;
use crate::components::{IconButton, Size};
use crate::icons::{CHEVRON_RIGHT, PLUS};
use crate::theme_of;
use crate::tokens::{icon as isize, motion, radius, type_};

pub struct SidebarSection {
    title: String,
    open: bool,
    rows: Vec<El>,
    show_add: bool,
    add_label: Option<String>,
    toggle: Acts,
    add: Acts,
}

impl SidebarSection {
    pub fn new(title: impl Into<String>, open: bool) -> SidebarSection {
        SidebarSection {
            title: title.into(),
            open,
            rows: Vec::new(),
            show_add: false,
            add_label: None,
            toggle: Acts::default(),
            add: Acts::default(),
        }
    }
    pub fn child(mut self, row: El) -> Self {
        self.rows.push(row);
        self
    }
    /// Shows the "+" button without hover (gallery and tests).
    pub fn show_add(mut self, on: bool) -> Self {
        self.show_add = on;
        self
    }
    /// Adds the hover "+" button; `label` names it ("Create channel").
    pub fn add_label(mut self, label: impl Into<String>) -> Self {
        self.add_label = Some(label.into());
        self
    }
    pub fn on_toggle<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.toggle.press(f);
        self
    }
    pub fn on_add<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
        self.add.press(f);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let chevron = icon(&CHEVRON_RIGHT, isize::XS)
            .key("chevron")
            .color(t.text_faint.color())
            .rotate(if self.open { FRAC_PI_2 } else { 0.0 })
            .rotate_motion(motion::DISCLOSURE)
            .a11y_hidden();
        let title = row()
            .key("toggle")
            .role("button")
            .label(self.title.clone())
            .expanded(self.open)
            .grow(1.0)
            .h(28.0)
            .px(4.0)
            .gap(4.0)
            .center_y()
            .radius(radius::SM)
            .focusable(true)
            .focus_ring(2.0, 0.0, k.c.border_focus)
            .hover(t.hover.color())
            .child(el().size(14.0, 14.0).center().child(chevron))
            .child(
                text(self.title)
                    .font(type_::LABEL)
                    .color(t.text_muted.color())
                    .a11y_hidden(),
            );
        let title = self.toggle.apply(title, true);
        let mut header = row()
            .key("header")
            .w_full()
            .gap(2.0)
            .center_y()
            .hover_group()
            .child(title);
        if let Some(label) = self.add_label {
            let add = self
                .add
                .apply(IconButton::new(&PLUS, label).size(Size::Sm).build(k), true);
            let add = el().key("add").child(add);
            header = header.child(if self.show_add {
                add
            } else {
                add.group_opacity(0.0, 1.0)
            });
        }
        let mut root = col()
            .role("group")
            .w_full()
            .token_motion(motion::DISCLOSURE)
            .child(header);
        if self.open {
            let enter =
                Transition::opacity().springs(SpringPair::new(motion::DISCLOSURE, motion::CONTROL));
            root = root.child(
                col()
                    .key("rows")
                    .w_full()
                    .gap(2.0)
                    .transition(enter)
                    .children(self.rows),
            );
        }
        root
    }
}
