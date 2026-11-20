//! The transcript: grouped messages with date and "New" dividers, opening at the bottom and
//! sticking to it, a "New messages" pill when the reader is scrolled away, older pages loading
//! as the top comes into view, and the empty, loading and failed states.
//!
//! Motion: switching channel crossfades the whole stage (`motion-control`, nothing slides); the
//! viewport of a new channel opens at its end on its first frame. A page that loads lands at once.
//! The pill rises and fades in on `motion-control`.
//!
//! Every message row is a plain element (no virtual list): 150 messages is a few thousand
//! elements and a page adds at most fifty more. Growing histories would want `virtualize`.

use std::collections::HashMap;
use std::rc::Rc;

use rune::{Color, El, Pixels, ScrollSpec, SpringPair, Transition, col, el, row, text};
use rune_kit::Kit;
use thehideout_design::components::{Button, ChatMessage, DateDivider, DeliveryState, EmptyState, MessageGroup, NewMessagesDivider, Size, Variant};
use thehideout_design::icons::{self, CHEVRON_DOWN, CHAT, PLUS};
use thehideout_design::{motion, palette_text, palette_wash, radius, theme_of, type_};

use crate::app::App;
use crate::format::{clock_label, day_label, unix};
use crate::state::{AppState, Delivery, DialogKind, Fetch, Marker, Place, chat_id_of};

/// A row of the transcript: the view model plus the server id (outgoing messages have none).
struct Item {
    msg: ChatMessage,
    id: Option<u64>,
}

fn items(s: &AppState, channel: u64) -> Vec<Item> {
    let me = s.my_id();
    let mut out = Vec::new();
    if let Some(log) = s.log(channel) {
        for m in &log.messages {
            let at = unix(m.created_at);
            out.push(Item {
                id: Some(m.id),
                msg: ChatMessage {
                    id: chat_id_of(m, me),
                    author_id: m.author_id.to_string(),
                    author_name: s.user_name(m.author_id),
                    at,
                    time_label: clock_label(s.tz, at),
                    body: m.content.clone(),
                    delivery: DeliveryState::Sent,
                    mentions_me: m.author_id != me && s.mentions_me(&m.content),
                },
            });
        }
    }
    for o in s.data.outgoing.iter().filter(|o| o.channel == channel) {
        let at = unix(o.at);
        out.push(Item {
            id: None,
            msg: ChatMessage {
                id: format!("n:{}", o.nonce),
                author_id: me.to_string(),
                author_name: s.user_name(me),
                at,
                time_label: clock_label(s.tz, at),
                body: o.content.clone(),
                delivery: if o.state == Delivery::Failed { DeliveryState::Failed } else { DeliveryState::Pending },
                mentions_me: false,
            },
        });
    }
    out
}

/// What the transcript draws from people rather than messages: their pictures, the colour of
/// their names, and the chip colours of an `@mention` of someone with a coloured role.
struct Looks {
    avatars: Rc<HashMap<String, Pixels>>,
    names: Rc<HashMap<String, Color>>,
    mentions: Rc<HashMap<String, (Color, Color)>>,
}

impl Looks {
    fn of(a: &App, k: &Kit, list: &[Item]) -> Looks {
        let t = theme_of(k);
        let s = &a.state;
        let guild = match s.ui.place {
            Place::Guild(g) => Some(g),
            Place::Home => None,
        };
        let mut avatars = HashMap::new();
        let mut names = HashMap::new();
        for it in list {
            let id = &it.msg.author_id;
            if avatars.contains_key(id) || names.contains_key(id) {
                continue;
            }
            let Ok(user) = id.parse::<u64>() else { continue };
            if let Some(p) = s.avatar_pixels(user) {
                avatars.insert(id.clone(), p);
            }
            if let Some(tone) = guild.and_then(|g| palette_text(s.name_color(g, user).as_key(), t)) {
                names.insert(id.clone(), tone.color());
            }
        }
        let mut mentions = HashMap::new();
        if let Some(g) = guild {
            for id in s.data.members.get(&g).into_iter().flatten() {
                let key = s.name_color(g, *id);
                if let (Some(ink), Some(wash)) = (palette_text(key.as_key(), t), palette_wash(key.as_key(), t)) {
                    mentions.insert(s.user_name(*id).to_lowercase(), (ink.color(), wash.color()));
                }
            }
        }
        Looks { avatars: Rc::new(avatars), names: Rc::new(names), mentions: Rc::new(mentions) }
    }

    fn apply(&self, g: MessageGroup) -> MessageGroup {
        let (avatars, names, mentions) = (self.avatars.clone(), self.names.clone(), self.mentions.clone());
        g.avatars(move |id| avatars.get(id).cloned())
            .name_colors(move |id| names.get(id).copied())
            .mention_ink(move |name| mentions.get(&name.to_lowercase()).copied())
    }
}

/// Index of the first message the "New" divider sits above.
fn marker_index(items: &[Item], marker: Marker, channel: u64) -> Option<usize> {
    let Marker::After { channel: c, id } = marker else { return None };
    if c != channel {
        return None;
    }
    items.iter().position(|i| matches!(i.id, Some(x) if id.is_none_or(|m| x > m)))
}

pub fn build(a: &App, k: &Kit) -> El {
    let t = theme_of(k);
    let s = &a.state;
    let stage = match s.current_channel() {
        Some(c) => channel_stage(a, k, c),
        None => placeholder_stage(a, k),
    };
    let mut area = col().key("transcript-area").grow(1.0).min_h(0.0).w_full().fill(t.surface.color()).child(stage);
    let pill_shown = !s.ui.at_bottom && s.ui.unseen_below > 0;
    if pill_shown
        && let Some(c) = s.current_channel()
    {
        area = area.child(pill(k, c));
    }
    if let Some(toast) = super::toasts::over_transcript(a, k, pill_shown) {
        area = area.child(toast);
    }
    area
}

fn fade() -> Transition {
    Transition::opacity().springs(SpringPair::new(motion::CONTROL, motion::CONTROL))
}

fn centered(e: El) -> El {
    col().w_full().h_full().center().child(e)
}

/// Nothing is open: Home with no conversation chosen, or a server with no channels.
fn placeholder_stage(a: &App, k: &Kit) -> El {
    let s = &a.state;
    let body = match s.ui.place {
        Place::Home => EmptyState::new(&CHAT, "Your conversations", "Pick one from the list, or start a new message.")
            .action(Button::new("New message").icon(&PLUS).on_press(|a: &mut App| a.act(|s| s.open_dialog(DialogKind::NewDm))).build(k))
            .build(k),
        Place::Guild(g) => {
            let mut e = EmptyState::new(&icons::HASH, "No channels yet", "Channels keep conversations in one place.");
            if s.guild(g).is_some_and(|x| x.owner_id == s.my_id()) {
                e = e.action(Button::new("Create channel").icon(&PLUS).on_press(move |a: &mut App| a.act(|s| s.open_dialog(DialogKind::CreateChannel { guild: g }))).build(k));
            }
            e.build(k)
        }
    };
    el().key("stage-none").absolute(0.0, 0.0).w_full().h_full().transition(fade()).child(centered(body))
}

fn channel_stage(a: &App, k: &Kit, channel: u64) -> El {
    let s = &a.state;
    let stage = el().key(format!("stage-{channel}")).absolute(0.0, 0.0).w_full().h_full().transition(fade());
    let Some(log) = s.log(channel) else { return stage };
    if !log.loaded {
        return stage.child(match log.fetch {
            Fetch::Initial { shown: true, .. } => skeleton(k),
            Fetch::Idle => centered(
                EmptyState::new(&icons::WARNING, "Couldn't load messages", "Check your connection and try again.")
                    .action(Button::new("Try again").variant(Variant::Secondary).on_press(|a: &mut App| a.act(|s| s.reload_current())).build(k))
                    .build(k),
            ),
            _ => el(),
        });
    }
    let list = items(s, channel);
    if list.is_empty() {
        let name = s.current_channel_name().unwrap_or_default();
        let (title, body) = match s.ui.place {
            Place::Home => (format!("This is the start of your chat with {name}"), "Say hello.".to_owned()),
            Place::Guild(_) => ("No messages yet".to_owned(), format!("Be the first to say something in #{name}.")),
        };
        return stage.child(centered(EmptyState::new(&CHAT, title, body).build(k)));
    }
    stage.child(viewport(a, k, channel, &list))
}

fn viewport(a: &App, k: &Kit, channel: u64, list: &[Item]) -> El {
    let t = theme_of(k);
    let s = &a.state;
    let log = s.log(channel);
    let mut c = col().key("messages").w_full().pb(8.0);

    if let Some(log) = log {
        if log.has_more {
            // Keyed by the oldest id: after a page lands this is a new element, so it reports
            // again if it is still on screen and the next page is wanted.
            let loading = log.fetch == Fetch::Older;
            let mut top = row().key(format!("older-{}", log.oldest_id.unwrap_or(0))).w_full().h(48.0).center().on_visible(move |a: &mut App, on, _tx| {
                if on {
                    a.act(|s| s.load_older(channel));
                }
            });
            if loading {
                top = top.child(text("Loading earlier messages\u{2026}").font(type_::CAPTION).color(t.text_muted.color()));
            }
            c = c.child(top);
        } else {
            let name = s.current_channel_name().unwrap_or_default();
            let title = match s.ui.place {
                Place::Home => format!("This is the start of your chat with {name}"),
                Place::Guild(_) => format!("Welcome to #{name}"),
            };
            c = c.child(col().key("start").w_full().px(16.0).pt(24.0).pb(4.0).gap(2.0).child(text(title).font(type_::TITLE).color(t.text.color())).child(text("This is where it begins.").font(type_::CONTROL).color(t.text_muted.color())));
        }
    }

    let looks = Looks::of(a, k, list);
    let marker_at = marker_index(list, s.ui.marker, channel);
    let mut start = 0;
    let mut prev_day: Option<i64> = None;
    for i in 1..=list.len() {
        let day_here = list.get(i).map(|it| s.day_of(it.msg.at));
        let page_edge = i < list.len() && list[i].id.is_some_and(|id| log.is_some_and(|l| l.breaks.contains(&id)));
        let boundary = i == list.len() || day_here != Some(s.day_of(list[i - 1].msg.at)) || Some(i) == marker_at || page_edge;
        if !boundary {
            continue;
        }
        let first = &list[start];
        let day = s.day_of(first.msg.at);
        if prev_day != Some(day) {
            c = c.child(DateDivider::new(day_label(day, s.today)).build(k));
            prev_day = Some(day);
        }
        if Some(start) == marker_at {
            c = c.child(NewMessagesDivider::build(k));
        }
        let msgs: Vec<ChatMessage> = list[start..i].iter().map(|it| it.msg.clone()).collect();
        for run in MessageGroup::runs(&msgs) {
            c = c.child(
                looks
                    .apply(MessageGroup::new(run))
                    .links::<App>()
                    .on_retry(|a: &mut App, id: &str| {
                        if let Some(nonce) = id.strip_prefix("n:") {
                            let nonce = nonce.to_owned();
                            a.act(|s| s.retry(&nonce));
                        }
                    })
                    .on_copy(|a: &mut App, id: &str| a.act(|s| s.copy_message(id)))
                    .on_reply(|a: &mut App, id: &str| a.act(|s| s.reply_to(id)))
                    .build(k),
            );
        }
        start = i;
    }

    col()
        .key(format!("tx-{channel}"))
        .w_full()
        .h_full()
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .on_at_bottom_if(|a: &mut App, at| {
            let ui = &mut a.state.ui;
            if ui.at_bottom == at {
                return false;
            }
            ui.at_bottom = at;
            if at {
                ui.unseen_below = 0;
            }
            true
        })
        .child(c)
}

/// Grey bars where messages will be. Shown only for loads slower than the placeholder delay.
fn skeleton(k: &Kit) -> El {
    let t = theme_of(k);
    let bar = |w: f32| el().w(w).h(10.0).radius(radius::SM).fill(t.surface_raised.color());
    let mut c = col().key("loading").w_full().h_full().justify_end().px(16.0).pb(16.0).gap(20.0);
    for (i, (a, b)) in [(120.0, 340.0), (90.0, 260.0), (140.0, 400.0), (100.0, 220.0), (110.0, 300.0)].into_iter().enumerate() {
        c = c.child(
            row()
                .key(format!("skeleton-{i}"))
                .gap(12.0)
                .items_start()
                .child(el().size(32.0, 32.0).radius(radius::MD).fill(t.surface_raised.color()))
                .child(col().gap(8.0).pt(4.0).child(bar(a)).child(bar(b))),
        );
    }
    c.a11y_hidden()
}

/// "New messages" at the bottom centre; pressing it glides to the end.
fn pill(k: &Kit, channel: u64) -> El {
    let enter = Transition::offset(0.0, 6.0).combined(Transition::opacity()).springs(SpringPair::new(motion::CONTROL, motion::CONTROL));
    let button = Button::new("New messages").icon(&CHEVRON_DOWN).size(Size::Sm).build(k).on_press_tx(move |_: &mut App, tx| tx.scroll_to_bottom(format!("tx-{channel}")));
    row().key("new-pill").pin(Some(0.0), None, Some(0.0), Some(12.0)).justify_center().transition(enter).child(k.elevate(button, &k.elevation.floating))
}
