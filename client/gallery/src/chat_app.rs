//! The assembled chat preview and its demo state: guild rail, sidebar, header, transcript and
//! composer, all live. Sending appends a pending message (the previous pending one is acked),
//! selecting a channel moves the sidebar highlight, the Create server dialog opens and toasts.

use rune::{El, ScrollSpec, TextEdit, col, el, row, text};
use rune_kit::Kit;
use thehideout_design::components::chat::{
    ChannelHeader, ChannelRow, ChatMessage, Composer, ConnectionPill, ConnectionStatus,
    DateDivider, DeliveryState, Dialog, EmptyState, GuildBadge, MessageGroup, NavRow,
    NewMessagesDivider, SidebarSection, Toast, ToastKind, dialog_field,
};
use thehideout_design::components::{Button, IconButton, TextInput, Variant};
use thehideout_design::icons::{self, BELL, PLUS, SEARCH, USERS};
use thehideout_design::{radius, theme_of, type_};

use crate::G;
use crate::chat::{chip, sample};

const DAY: i64 = 86_400;
const CHANNELS: [(&str, bool, u32, bool); 5] = [
    ("general", false, 0, false),
    ("random", true, 0, false),
    ("dev", true, 3, false),
    ("plans", false, 0, true),
    ("ops", true, 0, false),
];

pub struct ChatDemo {
    pub messages: Vec<ChatMessage>,
    pub draft: TextEdit,
    pub name: TextEdit,
    pub channel: usize,
    pub channels_open: bool,
    pub dialog: bool,
    pub toast: Option<(ToastKind, String)>,
    pub next: u32,
}

impl ChatDemo {
    pub fn new() -> ChatDemo {
        const ANA: (&str, &str) = ("user-ana", "Ana Kovac");
        const BRUNO: (&str, &str) = ("user-bruno", "Bruno");
        const CVETKA: (&str, &str) = ("user-cvetka", "Cvetka Z");
        const TIM: (&str, &str) = ("user-tim", "Tim Rekelj");
        let d = |n: i64, secs: i64| 100 * DAY + n * DAY + secs;
        let mut ask = sample(
            "m4",
            BRUNO,
            d(-1, 17 * 3600 + 3000),
            "17:50",
            "Yep, capped at 30s. @Tim can you check the gateway side?",
        );
        ask.mentions_me = true;
        let mut failed = sample(
            "m8",
            TIM,
            d(0, 9 * 3600 + 360),
            "09:06",
            "Also bumping the client crate to `0.0.2`.",
        );
        failed.delivery = DeliveryState::Failed;
        let mut gateway = sample(
            "m5",
            ANA,
            d(0, 9 * 3600 + 180),
            "09:03",
            "Gateway looks fine. It backs off to `5s`, then `30s`.",
        );
        gateway.reactions = vec![chip("\u{1f44d}", 2, false), chip("\u{1f389}", 1, true)];
        let messages = vec![
            sample(
                "m1",
                CVETKA,
                d(-2, 9 * 3600 + 720),
                "09:12",
                "Morning all. Staging is back up after the migration.",
            ),
            sample(
                "m2",
                CVETKA,
                d(-2, 9 * 3600 + 840),
                "09:14",
                "Ping me if anything looks off.",
            ),
            sample(
                "m3",
                BRUNO,
                d(-1, 17 * 3600 + 2400),
                "17:40",
                "Pushed the fix for the reconnect loop, review welcome.",
            ),
            sample(
                "m3b",
                ANA,
                d(-1, 17 * 3600 + 2640),
                "17:44",
                "On it. Is `backoff_ms` capped? Here is what I read:\n```rust\nlet delay = (base << attempt).min(30_000);\n```",
            ),
            ask,
            gateway,
            sample(
                "m6",
                ANA,
                d(0, 9 * 3600 + 200),
                "09:03",
                "I reset the counter on a clean connect, so no surprises.",
            ),
            sample(
                "m7",
                TIM,
                d(0, 9 * 3600 + 300),
                "09:05",
                "Thanks @ana. Merging after lunch.",
            ),
            failed,
        ];
        ChatDemo {
            messages,
            draft: TextEdit::new(""),
            name: TextEdit::new(""),
            channel: 0,
            channels_open: true,
            dialog: false,
            toast: None,
            next: 1,
        }
    }

    pub fn send(&mut self, text: String) {
        for m in &mut self.messages {
            if m.delivery == DeliveryState::Pending {
                m.delivery = DeliveryState::Sent;
            }
        }
        let last = self.messages.last().map_or(100 * DAY, |m| m.at);
        let at = last + 20;
        let label = format!("{:02}:{:02}", (at % DAY) / 3600, (at % 3600) / 60);
        let mut m = sample(
            &format!("n{}", self.next),
            ("user-tim", "Tim Rekelj"),
            at,
            &label,
            &text,
        );
        m.delivery = DeliveryState::Pending;
        self.next += 1;
        self.messages.push(m);
        self.toast = None;
    }

    /// The Create server dialog and the toast host, on the view's overlay list.
    pub fn dialog_layer(&self, k: &Kit, layer: &mut Vec<El>) {
        let name = dialog_field(
            k,
            "Server name",
            TextInput::new(&self.name)
                .placeholder("The Hideout")
                .on_edit(|g: &mut G| &mut g.chat.name)
                .on_submit(|g: &mut G, _tx| create_server(g)),
            None,
        );
        Dialog::new("demo", "Create server")
            .description("Give your server a name. You can change it later.")
            .open(self.dialog)
            .child(name)
            .footer(
                Button::new("Cancel")
                    .variant(Variant::Secondary)
                    .on_press(|g: &mut G| g.chat.dialog = false)
                    .build(k),
            )
            .footer(Button::new("Create").on_press(create_server).build(k))
            .on_close(|g: &mut G| g.chat.dialog = false)
            .build(k, layer);
        layer.push(Toast::host(
            k,
            self.toast.clone().map(|(kind, msg)| Toast::new(kind, msg)),
        ));
    }
}

fn create_server(g: &mut G) {
    g.chat.dialog = false;
    g.chat.toast = Some((ToastKind::Success, "Server created".to_owned()));
    g.chat.name.clear();
}

pub fn preview(s: &G, k: &Kit, _layer: &mut Vec<El>) -> El {
    let t = theme_of(k);
    let edge = |e: El| e.border_sides([0.0, 1.0, 0.0, 0.0], t.border_subtle.color());
    let rail = edge(
        col()
            .w(64.0)
            .h_full()
            .py(12.0)
            .gap(10.0)
            .items_center()
            .fill(t.bg.color())
            .child(
                GuildBadge::new("guild-hideout", "The Hideout")
                    .size(40)
                    .selected(true)
                    .build(k),
            )
            .child(
                GuildBadge::new("guild-garage", "Garage Band")
                    .size(40)
                    .unread(true)
                    .build(k),
            )
            .child(
                GuildBadge::new("guild-cook", "Cooking Club")
                    .size(40)
                    .mentions(2)
                    .build(k),
            )
            .child(
                IconButton::new(&PLUS, "Create or join a server")
                    .variant(Variant::Secondary)
                    .build(k),
            ),
    );

    let mut list = SidebarSection::new("Text channels", s.chat.channels_open)
        .add_label("Create channel")
        .on_toggle(|g: &mut G| g.chat.channels_open = !g.chat.channels_open)
        .on_add(|g: &mut G| g.chat.dialog = true);
    for (i, (name, unread, mentions, muted_)) in CHANNELS.iter().enumerate() {
        list = list.child(
            ChannelRow::new(*name)
                .highlight_group("channels")
                .selected(i == s.chat.channel)
                .unread(*unread)
                .mentions(*mentions)
                .muted(*muted_)
                .on_press(move |g: &mut G| {
                    g.chat.channel = i;
                    g.chat.toast = None;
                })
                .build(k),
        );
    }
    let sidebar = edge(
        col()
            .w(232.0)
            .h_full()
            .fill(t.surface.color())
            .child(
                row()
                    .h(48.0)
                    .px(16.0)
                    .center_y()
                    .child(
                        text("The Hideout".to_owned())
                            .font(type_::TITLE)
                            .color(t.text.color()),
                    ),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_h(0.0)
                    .padding(8.0)
                    .gap(8.0)
                    .child(
                        col()
                            .gap(2.0)
                            .child(
                                NavRow::new("Mentions", &BELL, &icons::fill::BELL)
                                    .highlight_group("nav")
                                    .mentions(3)
                                    .build(k),
                            )
                            .child(
                                NavRow::new("Members", &USERS, &icons::fill::USERS)
                                    .highlight_group("nav")
                                    .build(k),
                            ),
                    )
                    .child(list.build(k)),
            )
            .child(
                row()
                    .padding(12.0)
                    .child(ConnectionPill::new(ConnectionStatus::Connected).build(k)),
            ),
    );

    let channel = CHANNELS[s.chat.channel].0;
    let header = ChannelHeader::new(channel)
        .topic("Announcements, plans and the occasional gif")
        .action(IconButton::new(&USERS, "Show members").build(k))
        .action(IconButton::new(&SEARCH, "Search").build(k))
        .build(k);

    let main = col()
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .fill(t.bg.color())
        .child(header)
        .child(transcript(s, k))
        .child(
            col().w_full().px(16.0).pb(16.0).pt(8.0).child(
                Composer::new(&s.chat.draft)
                    .placeholder(format!("Message #{channel}"))
                    .bind(
                        |g: &mut G| &mut g.chat.draft,
                        |g: &mut G, text, _tx| g.chat.send(text),
                    )
                    .build(k),
            ),
        );

    row()
        .key("assembled")
        .w_full()
        .h(640.0)
        .radius(radius::LG)
        .clip_rounded(radius::LG)
        .border(1.0, t.border.color())
        .child(rail)
        .child(sidebar)
        .child(main)
}

fn transcript(s: &G, k: &Kit) -> El {
    let t = theme_of(k);
    let mut c = col().w_full().pb(8.0);
    let runs = MessageGroup::runs(&s.chat.messages);
    let mut prev_day: Option<i64> = None;
    for run in runs {
        let day = run[0].at.div_euclid(DAY);
        if prev_day != Some(day) {
            c = c.child(DateDivider::new(day_label(day)).build(k));
            if run[0].id == "m5" {
                c = c.child(NewMessagesDivider::build(k));
            }
            prev_day = Some(day);
        }
        c = c.child(
            MessageGroup::new(run)
                .links::<G>()
                .on_retry(|g: &mut G, id: &str| {
                    if let Some(m) = g.chat.messages.iter_mut().find(|m| m.id == id) {
                        m.delivery = DeliveryState::Pending;
                    }
                })
                .on_react(|g: &mut G, id: &str, key: &str| toggle_reaction(&mut g.chat.messages, id, key))
                .on_add_reaction(|g: &mut G, id: &str, _anchor: &str| {
                    // The app opens its picker here; the demo reacts with a rocket.
                    if let Some(m) = g.chat.messages.iter_mut().find(|m| m.id == id)
                        && !m.reactions.iter().any(|r| r.key == "u:\u{1f680}")
                        && m.reactions.len() < thehideout_design::components::MAX_REACTIONS
                    {
                        m.reactions.push(chip("\u{1f680}", 1, true));
                    }
                })
                .on_reply(|_: &mut G, _: &str| {})
                .on_copy(|_: &mut G, _: &str| {})
                .on_more(|_: &mut G, _: &str| {})
                .build(k),
        );
    }
    let body = if s.chat.messages.is_empty() {
        EmptyState::new(&icons::CHAT, "No messages yet", "Say hello.").build(k)
    } else {
        c
    };
    col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .fill(t.bg.color())
        .scroll(ScrollSpec { stick_bottom: true })
        .child(el().h(8.0))
        .child(body)
}

/// A chip was pressed: your reaction flips, and a chip nobody holds any more goes.
fn toggle_reaction(messages: &mut [ChatMessage], id: &str, key: &str) {
    let Some(m) = messages.iter_mut().find(|m| m.id == id) else { return };
    let Some(at) = m.reactions.iter().position(|r| r.key == key) else { return };
    let r = &mut m.reactions[at];
    if r.me {
        r.me = false;
        r.count = r.count.saturating_sub(1);
        if r.count == 0 {
            m.reactions.remove(at);
        }
    } else {
        r.me = true;
        r.count += 1;
    }
}

fn day_label(day: i64) -> &'static str {
    match day - 100 {
        0 => "Today",
        -1 => "Yesterday",
        _ => "Wed, Nov 18",
    }
}
