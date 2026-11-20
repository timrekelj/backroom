//! ChannelHeader: `#` + channel name + muted topic, with trailing IconButtons (members toggle,
//! search). The breadcrumb variant reads "Guild / # channel". `HEADER_H` tall with a bottom hairline.
//! Static: nothing animates here.

use rune::{El, col, icon, row, text};
use rune_kit::Kit;

use super::HEADER_H;
use crate::icons::{CHEVRON_RIGHT, HASH};
use crate::theme_of;
use crate::tokens::{icon as size, type_};

pub struct ChannelHeader {
    channel: String,
    guild: Option<String>,
    topic: Option<String>,
    actions: Vec<El>,
}

impl ChannelHeader {
    pub fn new(channel: impl Into<String>) -> ChannelHeader {
        ChannelHeader {
            channel: channel.into(),
            guild: None,
            topic: None,
            actions: Vec::new(),
        }
    }
    /// The breadcrumb variant: "Guild / # channel" (no topic).
    pub fn breadcrumb(guild: impl Into<String>, channel: impl Into<String>) -> ChannelHeader {
        ChannelHeader {
            guild: Some(guild.into()),
            ..ChannelHeader::new(channel)
        }
    }
    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }
    /// A trailing control, usually a built `IconButton` (Md, ghost).
    pub fn action(mut self, button: El) -> Self {
        self.actions.push(button);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let hash = icon(&HASH, size::MD)
            .color(t.text_faint.color())
            .a11y_hidden();
        let mut left = row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
        if let Some(g) = &self.guild {
            left = left
                .child(
                    text(g.clone())
                        .font(type_::CONTROL_STRONG)
                        .color(t.text_muted.color()),
                )
                .child(
                    icon(&CHEVRON_RIGHT, size::XS)
                        .color(t.text_faint.color())
                        .a11y_hidden(),
                );
        }
        left = left.child(hash).child(
            text(self.channel.clone())
                .font(type_::TITLE)
                .color(t.text.color()),
        );
        if let (Some(topic), None) = (&self.topic, &self.guild) {
            left = left
                .child(col().w(1.0).h(16.0).fill(t.border.color()))
                .child(
                    text(topic.clone())
                        .font(type_::CONTROL)
                        .color(t.text_muted.color())
                        .max_lines(1)
                        .shrink(),
                );
        }
        let mut r = row()
            .role("banner")
            .label(self.channel)
            .w_full()
            .h(HEADER_H)
            .px(16.0)
            .gap(12.0)
            .center_y()
            .border_sides([0.0, 0.0, 1.0, 0.0], t.border.color())
            .child(left);
        if !self.actions.is_empty() {
            r = r.child(row().gap(4.0).center_y().children(self.actions));
        }
        r
    }
}
