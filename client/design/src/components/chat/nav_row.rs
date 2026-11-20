//! Sidebar rows: [`NavRow`] (icon or 20pt avatar + label, for Home, Friends, DMs) and [`ChannelRow`]
//! (`#` + name). Both show selected, unread and muted, with an optional trailing unread dot or
//! mention pill.
//!
//! Selected: the highlight is its own element (`matched`), so when the selection moves the
//! highlight travels to the new row on `motion::REFLOW` instead of fading in place. Rows that
//! share a list share a `.highlight_group(..)`. Nav rows switch to the Phosphor fill icon when
//! selected; channels keep the regular `#` (README: never fill `hash`).

use rune::{El, Icon, Transition, el, icon, row, text};
use rune_kit::{Kit, State};

use crate::components::util::{Acts, over};
use super::ROW_H;
use crate::components::{Avatar, MentionPill, UnreadDot};
use crate::icons::HASH;
use crate::theme_of;
use crate::tokens::color::Tone;
use crate::tokens::{icon as size, motion, radius, type_};

struct Row {
    label: String,
    glyph: &'static Icon,
    glyph_selected: &'static Icon,
    /// A user (id, name) drawn as a 20pt avatar in place of the icon.
    avatar: Option<String>,
    selected: bool,
    unread: bool,
    muted: bool,
    mentions: u32,
    state: State,
    group: String,
    acts: Acts,
    /// A palette colour for the icon (channels); the selected highlight takes a faint wash of it.
    tint: Option<Tone>,
    /// A control shown on hover at the row's right edge (a "more" button).
    trailing: Option<El>,
    /// Replaces the label as the element key, so a rename keeps the row (and its slide) alive.
    id: Option<String>,
}

impl Row {
    fn new(label: String, glyph: &'static Icon, glyph_selected: &'static Icon) -> Row {
        Row {
            label,
            glyph,
            glyph_selected,
            avatar: None,
            selected: false,
            unread: false,
            muted: false,
            mentions: 0,
            state: State::Rest,
            group: "rows".into(),
            acts: Acts::default(),
            tint: None,
            trailing: None,
            id: None,
        }
    }

    fn build(self, k: &Kit) -> El {
        let t = theme_of(k);
        let strong = self.selected || (self.unread && !self.muted);
        let ink = if self.selected {
            t.text
        } else if self.muted {
            t.text_faint
        } else if self.unread {
            t.text
        } else {
            t.text_muted
        };
        let icon_ink = if let Some(tone) = self.tint {
            tone.alpha(if self.muted { 0.55 } else { 1.0 })
        } else if self.selected {
            t.text
        } else if self.muted {
            t.text_faint
        } else if self.unread {
            t.text
        } else {
            t.text_muted
        };
        let glyph = if self.selected {
            self.glyph_selected
        } else {
            self.glyph
        };
        let font = if strong {
            type_::CONTROL_STRONG
        } else {
            type_::CONTROL
        };

        let mut e = row()
            .key(format!("{}-row-{}", self.group, self.id.as_deref().unwrap_or(&self.label)))
            .role("button")
            .label(self.label.clone())
            .w_full()
            .h(ROW_H)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .radius(radius::MD)
            .focusable(true)
            .focus_ring(2.0, 0.0, k.c.border_focus)
            .token_motion(motion::CONTROL);
        if self.selected {
            // The shared element: unkeyed rows of other lists never match it.
            let hl = el()
                .key(format!("{}-highlight", self.group))
                .matched(format!("{}-highlight", self.group))
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .radius(radius::MD)
                .fill(match self.tint {
                    Some(tone) => over(tone.alpha(0.12).color(), t.pressed.color()),
                    None => t.pressed.color(),
                })
                .token_motion(motion::REFLOW)
                .transition(Transition::opacity())
                .a11y_hidden();
            e = e.child(hl);
        } else if self.state == State::Hover {
            e = e.fill(over(t.hover.color(), rune::Color::TRANSPARENT));
        } else if self.state == State::Rest {
            e = e.hover(t.hover.color()).press(t.pressed.color());
        }
        e = e.child(match &self.avatar {
            Some(id) => Avatar::new(id.clone(), self.label.clone())
                .size(20)
                .backdrop(t.bg.color())
                .build(k)
                .key("avatar"),
            None => icon(glyph, size::SM)
                .key("icon")
                .color(icon_ink.color())
                .a11y_hidden(),
        });
        e = e.child(
            text(self.label)
                .key("label")
                .font(font)
                .color(ink.color())
                .grow(1.0)
                .shrink()
                .max_lines(1)
                .a11y_hidden(),
        );
        if self.mentions > 0 {
            e = e.child(MentionPill::new(self.mentions).build(k));
        } else if self.unread && !self.muted && !self.selected {
            e = e.child(UnreadDot::build(k));
        }
        if let Some(more) = self.trailing {
            e = e.hover_group().child(
                row()
                    .key("more")
                    .pin(Some(0.0), Some(4.0), Some(0.0), Some(0.0))
                    .justify_end()
                    .center_y()
                    .group_opacity(0.0, 1.0)
                    .child(more),
            );
        }
        self.acts.apply(e, true)
    }
}

macro_rules! row_api {
    ($t:ident) => {
        impl $t {
            pub fn selected(mut self, on: bool) -> Self {
                self.0.selected = on;
                self
            }
            pub fn unread(mut self, on: bool) -> Self {
                self.0.unread = on;
                self
            }
            pub fn muted(mut self, on: bool) -> Self {
                self.0.muted = on;
                self
            }
            pub fn mentions(mut self, count: u32) -> Self {
                self.0.mentions = count;
                self
            }
            /// Forces hover (gallery and tests). At rest the runtime drives it.
            pub fn state(mut self, s: State) -> Self {
                self.0.state = s;
                self
            }
            /// Rows of one list share a group, so only they exchange the selection highlight.
            /// Defaults to `"rows"`; give each list in a window its own.
            pub fn highlight_group(mut self, group: impl Into<String>) -> Self {
                self.0.group = group.into();
                self
            }
            pub fn on_press<S: 'static>(mut self, f: impl Fn(&mut S) + 'static) -> Self {
                self.0.acts.press(f);
                self
            }
            pub fn build(self, k: &Kit) -> El {
                self.0.build(k)
            }
        }
    };
}

/// A nav row: an icon, its fill twin for the selected state, and a label.
pub struct NavRow(Row);

impl NavRow {
    pub fn new(
        label: impl Into<String>,
        glyph: &'static Icon,
        glyph_selected: &'static Icon,
    ) -> NavRow {
        NavRow(Row::new(label.into(), glyph, glyph_selected))
    }
}
impl NavRow {
    /// A row for a person: their 20pt avatar (colour from `id`, initials from `label`) instead of
    /// an icon, so a conversation list reads as people.
    pub fn person(label: impl Into<String>, id: impl Into<String>) -> NavRow {
        let mut r = Row::new(label.into(), &HASH, &HASH);
        r.avatar = Some(id.into());
        NavRow(r)
    }
}
row_api!(NavRow);

/// A channel row: `#` and a name.
pub struct ChannelRow(Row);

impl ChannelRow {
    pub fn new(name: impl Into<String>) -> ChannelRow {
        ChannelRow(Row::new(name.into(), &HASH, &HASH))
    }
}
row_api!(ChannelRow);

impl ChannelRow {
    /// A Phosphor icon in place of `#` (the channel's own), regular weight even when selected.
    pub fn icon(mut self, glyph: &'static Icon) -> Self {
        self.0.glyph = glyph;
        self.0.glyph_selected = glyph;
        self
    }
    /// Tints the icon with a palette colour and washes the selected highlight with it.
    pub fn tint(mut self, tone: Option<Tone>) -> Self {
        self.0.tint = tone;
        self
    }
    /// A built control that fades in at the row's right edge on hover (an `IconButton`).
    pub fn trailing(mut self, control: El) -> Self {
        self.0.trailing = Some(control);
        self
    }
    /// A stable id for the element key (the channel id), so renames and moves keep the row.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.0.id = Some(id.into());
        self
    }
}
