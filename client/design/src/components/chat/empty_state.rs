//! EmptyState: an icon tile, a title, a short body and an optional action. Centred in whatever
//! it is placed in. Static.

use rune::{Align, El, Icon, col, icon, row, text};
use rune_kit::Kit;

use crate::theme_of;
use crate::tokens::{icon as size, radius, type_};

pub struct EmptyState {
    glyph: &'static Icon,
    title: String,
    body: String,
    action: Option<El>,
}

impl EmptyState {
    pub fn new(
        glyph: &'static Icon,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> EmptyState {
        EmptyState {
            glyph,
            title: title.into(),
            body: body.into(),
            action: None,
        }
    }
    /// The one action, usually a built primary or secondary `Button`.
    pub fn action(mut self, button: El) -> Self {
        self.action = Some(button);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let tile = row()
            .size(40.0, 40.0)
            .center()
            .radius(radius::LG)
            .fill(t.surface_raised.color())
            .child(
                icon(self.glyph, crate::metrics_of(k).icon(size::MD))
                    .color(t.text_muted.color())
                    .a11y_hidden(),
            );
        let mut c = col()
            .w_full()
            .max_w(320.0)
            .gap(6.0)
            .items_center()
            .child(tile)
            .child(col().h(6.0))
            .child(
                text(self.title)
                    .font(type_::at(k, type_::TITLE))
                    .color(t.text.color())
                    .align(Align::Center),
            )
            .child(
                text(self.body)
                    .font(type_::at(k, type_::CONTROL))
                    .color(t.text_muted.color())
                    .wrap()
                    .align(Align::Center),
            );
        if let Some(a) = self.action {
            c = c.child(col().h(10.0)).child(a);
        }
        c.role("group")
    }
}
