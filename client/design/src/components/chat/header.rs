//! ChannelHeader: `#` + channel name + muted topic, with trailing IconButtons (members toggle,
//! search). The breadcrumb variant reads "Guild / # channel". `Metrics::header_h` tall, no rule under it (a scroll-edge hairline appears there while content is scrolled under).
//! Static: nothing animates here.

use rune::{col, icon, row, text, El, Icon};
use rune_kit::Kit;

use crate::icons::{CHEVRON_RIGHT, HASH};
use crate::theme_of;
use crate::tokens::color::Tone;
use crate::tokens::{icon as size, type_};

pub struct ChannelHeader {
    channel: String,
    guild: Option<String>,
    topic: Option<String>,
    actions: Vec<El>,
    glyph: &'static Icon,
    tint: Option<Tone>,
    art: Option<El>,
}

impl ChannelHeader {
    pub fn new(channel: impl Into<String>) -> ChannelHeader {
        ChannelHeader {
            channel: channel.into(),
            guild: None,
            topic: None,
            actions: Vec::new(),
            glyph: &HASH,
            tint: None,
            art: None,
        }
    }
    /// The breadcrumb variant: "Guild / # channel" (no topic).
    pub fn breadcrumb(guild: impl Into<String>, channel: impl Into<String>) -> ChannelHeader {
        ChannelHeader {
            guild: Some(guild.into()),
            ..ChannelHeader::new(channel)
        }
    }
    /// The channel's own icon and palette colour in place of the plain `#`.
    pub fn glyph(mut self, glyph: &'static Icon, tint: Option<Tone>) -> Self {
        self.glyph = glyph;
        self.tint = tint;
        self
    }
    /// An emoji (a built element about 18pt square) in place of the icon.
    pub fn emoji(mut self, art: El) -> Self {
        self.art = Some(art);
        self
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
        let hash = match self.art {
            Some(art) => row().size(size::MD, size::MD).center().child(art),
            None => icon(self.glyph, crate::metrics_of(k).icon(size::MD))
                .color(self.tint.unwrap_or(t.text_faint).color())
                .a11y_hidden(),
        };
        let mut left = row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
        if let Some(g) = &self.guild {
            left = left
                .child(
                    text(g.clone())
                        .font(type_::at(k, type_::CONTROL_STRONG))
                        .color(t.text_muted.color()),
                )
                .child(
                    icon(&CHEVRON_RIGHT, crate::metrics_of(k).icon(size::XS))
                        .color(t.text_faint.color())
                        .a11y_hidden(),
                );
        }
        left = left.child(hash).child(
            text(self.channel.clone())
                .font(type_::at(k, type_::TITLE))
                .color(t.text.color()),
        );
        if let (Some(topic), None) = (&self.topic, &self.guild) {
            left = left
                .child(col().w(1.0).h(16.0).fill(t.border_subtle.color()))
                .child(
                    text(topic.clone())
                        .font(type_::at(k, type_::CONTROL))
                        .color(t.text_muted.color())
                        .max_lines(1)
                        .shrink(),
                );
        }
        let mut r = row()
            .role("banner")
            .label(self.channel)
            .w_full()
            .h(crate::metrics_of(k).header_h())
            .px(16.0)
            .gap(12.0)
            .center_y()
            .child(left);
        if !self.actions.is_empty() {
            r = r.child(row().gap(4.0).center_y().children(self.actions));
        }
        r
    }
}
