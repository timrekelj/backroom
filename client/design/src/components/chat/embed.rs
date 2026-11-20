//! EmbedCard: a link preview under a message, collapsible like an accordion. A left accent rail
//! (the provider's palette colour, or `border_strong` for a plain link), a header row (site icon,
//! site, author, a chevron) that folds the card, and under it the body: a title and description
//! with a thumbnail for a plain link, or the author, the post text and the media (built by the app,
//! which owns the players) for a post.
//!
//! Motion: the chevron turns a quarter and the body fades while what is below moves to make room,
//! all on `motion::DISCLOSURE`, the same as a sidebar section. Nothing animates on the first frame.

use std::f32::consts::FRAC_PI_2;

use rune::{col, el, icon, row, text, El, Icon, Pixels, SpringPair, SurfaceFit, Transition};
use rune_kit::Kit;

use super::attachments::{fit_size, inert, Wire, MEDIA_BACK};
use crate::icons::{self, ARROW_SQUARE_OUT, CHEVRON_RIGHT, GLOBE};
use crate::palette_color;
use crate::tokens::{icon as isize, motion, radius, type_};
use crate::theme_of;

/// Widest an embed card grows.
pub const EMBED_W: f32 = 440.0;
/// Lines of description a link card shows.
pub const DESCRIPTION_LINES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Link,
    X,
    TikTok,
    Instagram,
    Reddit,
}

impl Provider {
    /// From the wire's provider key ("x", "tiktok", "instagram", "reddit"; anything else is a link).
    pub fn from_key(key: &str) -> Provider {
        match key {
            "x" => Provider::X,
            "tiktok" => Provider::TikTok,
            "instagram" => Provider::Instagram,
            "reddit" => Provider::Reddit,
            _ => Provider::Link,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Link => "Link",
            Provider::X => "X",
            Provider::TikTok => "TikTok",
            Provider::Instagram => "Instagram",
            Provider::Reddit => "Reddit",
        }
    }

    pub fn icon(self) -> &'static Icon {
        match self {
            Provider::Link => &GLOBE,
            Provider::X => &icons::X_LOGO,
            Provider::TikTok => &icons::TIKTOK_LOGO,
            Provider::Instagram => &icons::INSTAGRAM_LOGO,
            Provider::Reddit => &icons::REDDIT_LOGO,
        }
    }

    /// The palette key of the rail, `None` for a plain link.
    pub fn palette(self) -> Option<&'static str> {
        match self {
            Provider::Link => None,
            Provider::X => Some("slate"),
            Provider::TikTok => Some("cyan"),
            Provider::Instagram => Some("pink"),
            Provider::Reddit => Some("orange"),
        }
    }
}

pub struct EmbedCard {
    key: String,
    provider: Provider,
    site: String,
    author: String,
    author_icon: Option<Pixels>,
    collapsed: bool,
    toggle: Wire,
    open_original: Wire,
    open_link: Wire,
    title: String,
    description: String,
    thumb: Option<(Pixels, Option<(u32, u32)>)>,
    text: Option<El>,
    media: Vec<El>,
    published: String,
    post: bool,
}

impl EmbedCard {
    pub fn new(key: impl Into<String>, provider: Provider) -> EmbedCard {
        EmbedCard {
            key: key.into(),
            provider,
            site: String::new(),
            author: String::new(),
            author_icon: None,
            collapsed: false,
            toggle: inert(),
            open_original: inert(),
            open_link: inert(),
            title: String::new(),
            description: String::new(),
            thumb: None,
            text: None,
            media: Vec::new(),
            published: String::new(),
            post: provider != Provider::Link,
        }
    }
    /// The site's own name ("GitHub"); a provider card shows the provider's.
    pub fn site(mut self, s: impl Into<String>) -> Self {
        self.site = s.into();
        self
    }
    pub fn author(mut self, name: impl Into<String>, icon: Option<Pixels>) -> Self {
        self.author = name.into();
        self.author_icon = icon;
        self
    }
    pub fn collapsed(mut self, on: bool) -> Self {
        self.collapsed = on;
        self
    }
    /// Attaches the press that folds or unfolds the card to its header.
    pub fn on_toggle(mut self, w: Wire) -> Self {
        self.toggle = w;
        self
    }
    /// "Open original" (the post's page in the browser).
    pub fn on_open_original(mut self, w: Wire) -> Self {
        self.open_original = w;
        self
    }
    /// The title as a link.
    pub fn on_open_link(mut self, w: Wire) -> Self {
        self.open_link = w;
        self
    }
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = t.into();
        self
    }
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }
    pub fn thumbnail(mut self, pixels: Pixels, size: Option<(u32, u32)>) -> Self {
        self.thumb = Some((pixels, size));
        self
    }
    /// The post's text, built by the app (markdown-light, with working links).
    pub fn text(mut self, body: El) -> Self {
        self.text = Some(body);
        self
    }
    /// Media of a post: video cards and image grids, built by the app.
    pub fn media(mut self, items: Vec<El>) -> Self {
        self.media = items;
        self
    }
    /// "Nov 19, 14:03".
    pub fn published(mut self, label: impl Into<String>) -> Self {
        self.published = label.into();
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let rail = self
            .provider
            .palette()
            .and_then(|p| palette_color(p, t))
            .unwrap_or(t.border_strong);
        let site = if self.provider == Provider::Link {
            if self.site.is_empty() {
                "Link".to_owned()
            } else {
                self.site.clone()
            }
        } else {
            self.provider.name().to_owned()
        };
        let chevron = icon(&CHEVRON_RIGHT, isize::XS)
            .key("chevron")
            .color(t.text_faint.color())
            .rotate(if self.collapsed { 0.0 } else { FRAC_PI_2 })
            .rotate_motion(motion::DISCLOSURE)
            .a11y_hidden();
        let mut head = row()
            .key("toggle")
            .role("button")
            .label(format!("{site} preview"))
            .expanded(!self.collapsed)
            .w_full()
            .h(28.0)
            .px(6.0)
            .gap(6.0)
            .center_y()
            .radius(radius::SM)
            .focusable(true)
            .focus_ring(2.0, 0.0, k.c.border_focus)
            .hover(t.hover.color())
            .child(
                icon(self.provider.icon(), isize::XS)
                    .color(t.text_muted.color())
                    .a11y_hidden(),
            )
            .child(
                text(site)
                    .font(type_::LABEL)
                    .color(t.text_muted.color())
                    .a11y_hidden(),
            );
        if !self.author.is_empty() && self.collapsed {
            head = head.child(
                text(format!("\u{b7} {}", self.author))
                    .font(type_::LABEL)
                    .color(t.text_faint.color())
                    .truncate()
                    .shrink()
                    .min_w(0.0)
                    .a11y_hidden(),
            );
        }
        if self.collapsed && !self.title.is_empty() {
            head = head.child(
                text(self.title.clone())
                    .font(type_::LABEL)
                    .color(t.text_faint.color())
                    .truncate()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .a11y_hidden(),
            );
        } else {
            head = head.child(el().grow(1.0));
        }
        head = (self.toggle)(head.child(el().size(14.0, 14.0).center().child(chevron)));

        let mut card = col()
            .key(self.key.clone())
            .role("group")
            .label("Link preview")
            .w_full()
            .max_w(EMBED_W)
            .pl(10.0)
            .pr(8.0)
            .py(4.0)
            .radius(radius::LG)
            .fill(t.hover.color())
            .token_motion(motion::DISCLOSURE)
            .child(
                el().key("rail")
                    .absolute(0.0, 0.0)
                    .w(3.0)
                    .h_full()
                    .radius(radius::XS)
                    .fill(rail.color())
                    .a11y_hidden(),
            )
            .child(head);
        if !self.collapsed {
            let enter =
                Transition::opacity().springs(SpringPair::new(motion::DISCLOSURE, motion::CONTROL));
            card = card.child(
                col()
                    .key("body")
                    .w_full()
                    .gap(8.0)
                    .px(6.0)
                    .pt(2.0)
                    .pb(8.0)
                    .transition(enter)
                    .children(self.body(k)),
            );
        }
        card
    }

    fn body(self, k: &Kit) -> Vec<El> {
        let t = theme_of(k);
        let mut out = Vec::new();
        if self.post {
            if !self.author.is_empty() {
                let mut who = row().key("author").gap(8.0).center_y();
                if let Some(p) = &self.author_icon {
                    who = who.child(
                        el().size(20.0, 20.0)
                            .radius(radius::FULL)
                            .fill(MEDIA_BACK.color())
                            .pixels_fit(p, SurfaceFit::Fill)
                            .a11y_hidden(),
                    );
                }
                out.push(
                    who.child(
                        text(self.author.clone())
                            .font(type_::BODY_STRONG)
                            .color(t.text.color()),
                    ),
                );
            }
            if let Some(b) = self.text {
                out.push(b);
            }
            out.extend(self.media);
            let mut foot = row().key("foot").w_full().gap(12.0).center_y();
            if !self.published.is_empty() {
                foot = foot.child(
                    text(self.published.clone())
                        .font(type_::CAPTION)
                        .color(t.text_muted.color()),
                );
            }
            let original = (self.open_original)(
                row()
                    .key("original")
                    .role("button")
                    .label("Open original")
                    .gap(4.0)
                    .h(20.0)
                    .center_y()
                    .focusable(true)
                    .focus_ring(2.0, 1.0, k.c.border_focus)
                    .hover(t.hover.color())
                    .child(
                        icon(&ARROW_SQUARE_OUT, isize::XS)
                            .color(t.accent_text.color())
                            .a11y_hidden(),
                    )
                    .child(
                        text("Open original")
                            .font(type_::CONTROL_STRONG)
                            .color(t.accent_text.color()),
                    ),
            );
            out.push(foot.child(original));
            return out;
        }
        let side = self.thumb.as_ref().is_some_and(|(_, size)| {
            size.is_some_and(|(w, h)| h > 0 && (w as f32 / h as f32) < 1.4)
        });
        let mut texts = col().key("texts").grow(1.0).shrink().min_w(0.0).gap(4.0);
        if !self.title.is_empty() {
            texts = texts.child((self.open_link)(
                text(self.title.clone())
                    .key("title")
                    .font(type_::BODY_STRONG)
                    .color(t.accent_text.color())
                    .wrap()
                    .w_full()
                    .role("link")
                    .label(self.title.clone())
                    .focusable(true)
                    .focus_ring(2.0, 1.0, k.c.border_focus)
                    .hover(t.hover.color()),
            ));
        }
        if !self.description.is_empty() {
            texts = texts.child(
                text(self.description.clone())
                    .key("description")
                    .font(type_::CONTROL)
                    .color(t.text_muted.color())
                    .wrap()
                    .max_lines(DESCRIPTION_LINES)
                    .w_full(),
            );
        }
        match (self.thumb, side) {
            (Some((p, _)), true) => out.push(
                row().gap(12.0).items_start().child(texts).child(
                    el().key("thumb")
                        .size(96.0, 96.0)
                        .radius(radius::MD)
                        .fill(MEDIA_BACK.color())
                        .pixels_fit(&p, SurfaceFit::Fill)
                        .a11y_hidden(),
                ),
            ),
            (Some((p, size)), false) => {
                let (w, h) = fit_size(size, 400.0, 225.0);
                out.push(texts);
                out.push(
                    el().key("thumb")
                        .size(w, h)
                        .radius(radius::MD)
                        .fill(MEDIA_BACK.color())
                        .pixels_fit(&p, SurfaceFit::Fill)
                        .a11y_hidden(),
                );
            }
            (None, _) => out.push(texts),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_come_from_the_wire_key() {
        assert_eq!(Provider::from_key("x"), Provider::X);
        assert_eq!(Provider::from_key("tiktok"), Provider::TikTok);
        assert_eq!(Provider::from_key("instagram"), Provider::Instagram);
        assert_eq!(Provider::from_key("reddit"), Provider::Reddit);
        assert_eq!(Provider::from_key(""), Provider::Link);
        assert_eq!(Provider::from_key("mastodon"), Provider::Link);
        assert!(Provider::Link.palette().is_none() && Provider::Reddit.palette().is_some());
    }
}
