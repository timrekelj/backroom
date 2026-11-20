//! Kbd: keyboard shortcut chips. One 18pt cap per key, `radius-xs`, raised fill, 1px border.

use rune::{El, TextStyle, row, text};
use rune_kit::Kit;

use crate::{metrics_of, theme_of};
use crate::tokens::{radius, type_};

pub struct Kbd {
    keys: Vec<String>,
}

impl Kbd {
    pub fn new<S: AsRef<str>>(keys: &[S]) -> Kbd {
        Kbd {
            keys: keys.iter().map(|k| k.as_ref().to_owned()).collect(),
        }
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let m = metrics_of(k);
        let font = m.font(TextStyle {
            size: 11.0,
            weight: 530.0,
            line_height: 14.0,
            tracking: 0.0,
            ..type_::CAPTION
        });
        let label = self.keys.join(" ");
        row()
            .role("group")
            .label(label)
            .gap(2.0)
            .center_y()
            .children(self.keys.into_iter().enumerate().map(|(i, key)| {
                row()
                    .key(format!("key{i}"))
                    .h(m.em(18.0))
                    .min_w(m.em(18.0))
                    .px(5.0)
                    .center()
                    .radius(radius::XS)
                    .fill(t.surface_raised.color())
                    .border(1.0, t.border_subtle.color())
                    .child(
                        text(key)
                            .font(font)
                            .color(t.text_muted.color())
                            .a11y_hidden(),
                    )
            }))
    }
}
