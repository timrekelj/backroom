//! Message bodies: Discord-flavoured markup (see [`super::markdown`] and the README's "Message
//! markup") drawn as Rune text. [`parse`] is the flat view the app uses to find mentions (pure,
//! unit-tested); [`render`] / [`render_with`] turn the real parse into elements, with inline marks
//! as `Span`s on one wrapping text so lines break naturally.
//!
//! Links and spoilers are pressable ranges of that text. They need the app's state type to
//! attach a handler, so `MessageGroup::links::<S>()` supplies a [`Wire`]; without one they are
//! styled but inert. Which spoilers of a message are revealed is local to the UI (a per-thread
//! set keyed by message id), not app state.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use rune::{El, Link, LinkPaint, Span, TextStyle, Tx, col, el, row, text};
use rune_kit::Kit;
use rune_kit::syntax::{SyntaxTheme, highlight};

use super::markdown::{self as md, Inlines, Kind};
use crate::theme_of;
use crate::tokens::{radius, type_};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inline {
    Code,
    Mention,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Display text (marks stripped) and the byte ranges of its inline marks.
    Text {
        text: String,
        marks: Vec<(usize, usize, Inline)>,
    },
    Code {
        lang: Option<String>,
        code: String,
    },
}

fn flatten(blocks: Vec<md::Block>, out: &mut Vec<Block>) {
    let text = |i: Inlines| {
        let mut marks: Vec<(usize, usize, Inline)> = i
            .marks
            .iter()
            .filter_map(|m| match m.kind {
                Kind::Code => Some((m.start, m.end, Inline::Code)),
                Kind::Mention => Some((m.start, m.end, Inline::Mention)),
                _ => None,
            })
            .collect();
        marks.sort_by_key(|m| m.0);
        Block::Text { text: i.text, marks }
    };
    for b in blocks {
        match b {
            md::Block::Paragraph(i) | md::Block::Heading(_, i) => out.push(text(i)),
            md::Block::Code { lang, code } => out.push(Block::Code { lang, code }),
            md::Block::Quote(inner) => flatten(inner, out),
            md::Block::List { items, .. } => items.into_iter().for_each(|i| flatten(i, out)),
        }
    }
}

/// The body flattened to text runs and code blocks, keeping only code and mention marks. Mentions
/// are found wherever the markup allows them (inside bold, quotes, lists) and never in code.
pub fn parse(body: &str) -> Vec<Block> {
    let mut out = Vec::new();
    flatten(md::parse(body), &mut out);
    out
}

// ---- spoilers and link presses ----

thread_local! {
    static REVEALED: RefCell<HashSet<(String, u32)>> = RefCell::new(HashSet::new());
}

/// Whether spoiler number `index` (in reading order) of message `scope` has been clicked open.
pub fn spoiler_revealed(scope: &str, index: u32) -> bool {
    REVEALED.with_borrow(|s| s.contains(&(scope.to_owned(), index)))
}

pub fn reveal_spoiler(scope: &str, index: u32) {
    REVEALED.with_borrow_mut(|s| {
        s.insert((scope.to_owned(), index));
    });
}

/// What pressing a link of message `scope` does: a spoiler opens, a URL goes to the system's
/// default handler (`Tx::open_url` only opens http, https and mailto).
pub fn activate(scope: &str, id: &str, tx: &mut Tx) {
    match id.strip_prefix(SPOILER_ID) {
        Some(n) => {
            if let Ok(n) = n.parse() {
                reveal_spoiler(scope, n);
            }
        }
        None => {
            let _ = tx.open_url(id);
        }
    }
}

const SPOILER_ID: &str = "spoiler:";

/// Attaches the press handler for state type `S` to a text with links, given the message id.
pub type Wire = Rc<dyn Fn(El, &str) -> El>;

pub fn wire<S: 'static>() -> Wire {
    Rc::new(|e: El, scope: &str| {
        let scope = scope.to_owned();
        e.on_link(move |_: &mut S, id: &str, tx: &mut Tx| activate(&scope, id, tx))
    })
}

// ---- render ----

struct Cx<'a> {
    k: &'a Kit,
    scope: &'a str,
    wire: Option<&'a Wire>,
    mention: Option<&'a MentionInk>,
    spoilers: u32,
}

/// What colours an `@name` chip: given the name (no `@`), the text and wash colours of that
/// member's role, or `None` for the accent chip every other mention gets.
pub type MentionInk = Rc<dyn Fn(&str) -> Option<(rune::Color, rune::Color)>>;

/// The body as a column of blocks, 6pt apart, links inert and spoilers hidden.
pub fn render(k: &Kit, body: &str) -> El {
    render_with(k, body, "", None)
}

/// [`render`] for message `scope`, with presses wired when `wire` is given.
pub fn render_with(k: &Kit, body: &str, scope: &str, wire: Option<&Wire>) -> El {
    render_styled(k, body, scope, wire, None)
}

/// [`render_with`], and mention chips of members with a coloured role take that role's colours.
pub fn render_styled(k: &Kit, body: &str, scope: &str, wire: Option<&Wire>, mention: Option<&MentionInk>) -> El {
    let mut cx = Cx { k, scope, wire, mention, spoilers: 0 };
    blocks_el(&mut cx, &md::parse(body), 0, 6.0)
}

fn blocks_el(cx: &mut Cx, blocks: &[md::Block], depth: usize, gap: f32) -> El {
    let mut c = col().w_full().gap(gap);
    for b in blocks {
        c = c.child(block_el(cx, b, depth));
    }
    c
}

fn block_el(cx: &mut Cx, b: &md::Block, depth: usize) -> El {
    let k = cx.k;
    let t = theme_of(k);
    match b {
        md::Block::Paragraph(i) => inline_el(cx, i, type_::BODY, 600.0),
        md::Block::Heading(level, i) => {
            let font = match level {
                1 => type_::TITLE,
                2 => TextStyle::new(15.0, 600.0).lh(22.0),
                _ => type_::BODY_STRONG,
            };
            inline_el(cx, i, font, 700.0).mt(4.0)
        }
        md::Block::Code { lang, code } => {
            let found = highlight(lang.as_deref(), code);
            let spans = SyntaxTheme::from_kit(k).spans(&found, 0);
            col()
                .w_full()
                .px(12.0)
                .py(8.0)
                .radius(radius::MD)
                .fill(t.surface_raised.color())
                .border(1.0, t.border_subtle.color())
                .child(
                    text(code.clone())
                        .font(type_::CODE)
                        .color(t.text.color())
                        .wrap()
                        .wrap_anywhere()
                        .w_full()
                        .spans(spans)
                        .selectable(true)
                        .selection_color(k.selection()),
                )
        }
        md::Block::Quote(inner) => row()
            .w_full()
            .gap(10.0)
            .items_stretch()
            .child(
                el().w(3.0)
                    .radius(radius::XS)
                    .fill(t.border_strong.color())
                    .a11y_hidden(),
            )
            .child(
                blocks_el(cx, inner, depth + 1, 6.0)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0),
            ),
        md::Block::List { start, items } => {
            let mut c = col().w_full().gap(2.0);
            for (n, item) in items.iter().enumerate() {
                let (mark, w) = match start {
                    Some(s) => {
                        let label = format!("{}.", s + n as u64);
                        let w = 8.5 * (label.len() - 1) as f32 + 6.0;
                        (label, w)
                    }
                    None => ((if depth == 0 { "\u{2022}" } else { "\u{25e6}" }).to_owned(), 12.0),
                };
                c = c.child(
                    row()
                        .w_full()
                        .gap(6.0)
                        .items_start()
                        .child(
                            row().w(w).justify_end().child(
                                text(mark).font(type_::BODY).color(t.text_muted.color()),
                            ),
                        )
                        .child(
                            blocks_el(cx, item, depth + 1, 2.0)
                                .grow(1.0)
                                .shrink()
                                .min_w(0.0),
                        ),
                );
            }
            c
        }
    }
}

fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// One wrapping text: marks as spans, links and spoilers as pressable ranges, strikes as lines.
fn inline_el(cx: &mut Cx, x: &Inlines, font: TextStyle, bold: f32) -> El {
    let k = cx.k;
    let t = theme_of(k);
    let mut marks: Vec<&md::Mark> = x.marks.iter().collect();
    marks.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));

    // Spoilers are numbered in reading order across the whole message.
    let mut hidden: Vec<(usize, usize)> = Vec::new();
    let mut spoilers: Vec<(u32, usize, usize, bool)> = Vec::new();
    for m in marks.iter().filter(|m| m.kind == Kind::Spoiler) {
        let n = cx.spoilers;
        cx.spoilers += 1;
        let open = spoiler_revealed(cx.scope, n);
        if !open {
            hidden.push((m.start, m.end));
        }
        spoilers.push((n, m.start, m.end, open));
    }
    let covered = |r: (usize, usize)| hidden.iter().any(|&h| overlaps(r, h));

    let (mut spans, mut code, mut strikes, mut links) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for m in &marks {
        let (a, b) = (m.start, m.end);
        match &m.kind {
            Kind::Bold => spans.push(Span::new(a, b).weight(bold)),
            Kind::Italic => spans.push(Span::new(a, b).italic()),
            Kind::Underline => spans.push(Span::new(a, b).underline(t.text.color())),
            Kind::Strike => {
                if !covered((a, b)) {
                    strikes.push((a, b, None));
                }
            }
            Kind::Code => code.push(
                Span::new(a, b)
                    .mono()
                    .size(12.5)
                    .bg(t.surface_raised.color(), 3.0),
            ),
            Kind::Mention => {
                let name = x.text[a..b].trim_start_matches('@');
                let (ink, wash) = cx
                    .mention
                    .and_then(|f| f(name))
                    .unwrap_or((t.accent_text.color(), t.accent_subtle.color()));
                spans.push(Span::new(a, b).weight(560.0).color(ink).bg(wash, 2.0));
            }
            Kind::Link(url) => {
                spans.push(Span::new(a, b).color(t.accent_text.color()));
                if !covered((a, b)) {
                    let shown = &x.text[a..b];
                    // A masked link names where it goes; a bare URL already says so.
                    links.push(
                        Link::new(a..b, url.clone())
                            .hover(LinkPaint::new().underline(t.accent_text.color()))
                            .help(if shown == url { "" } else { url.as_str() }),
                    );
                }
            }
            Kind::Emoji { .. } | Kind::Spoiler => {}
        }
    }
    spans.append(&mut code);
    // Under a hidden spoiler only the metrics stay, so revealing never reflows the line.
    for s in &mut spans {
        if covered((s.start as usize, s.end as usize)) {
            s.bg = None;
            s.color = None;
            s.underline = None;
        }
    }
    let hide = t.border.color();
    let mut uncopied = Vec::new();
    for &(n, a, b, open) in &spoilers {
        if open {
            spans.push(Span::new(a, b).bg(t.surface_raised.color(), 2.0));
        } else {
            spans.push(Span::new(a, b).color(hide).bg(hide, 2.0));
            uncopied.push(a..b);
            links.push(
                Link::new(a..b, format!("{SPOILER_ID}{n}"))
                    .role("button")
                    .label("Spoiler, press to reveal")
                    .hover(LinkPaint::new().bg(t.border_strong.color()).color(t.border_strong.color())),
            );
        }
    }

    let pressable = !links.is_empty();
    let mut e = text(x.text.clone())
        .font(font)
        .color(t.text.color())
        .wrap()
        .w_full()
        .spans(spans)
        .strikes(strikes);
    for l in links {
        e = e.link(l);
    }
    if pressable && let Some(w) = cx.wire {
        e = w(e, cx.scope);
    }
    for r in uncopied {
        e = e.uncopied(r);
    }
    e.selectable(true).selection_color(k.selection())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_block() {
        assert_eq!(
            parse("hello there"),
            vec![Block::Text {
                text: "hello there".into(),
                marks: vec![]
            }]
        );
    }

    #[test]
    fn inline_code_and_mentions_become_ranges_over_stripped_text() {
        let Block::Text { text, marks } = &parse("ping @ana.k, run `cargo test` now")[0] else {
            panic!()
        };
        assert_eq!(text, "ping @ana.k, run cargo test now");
        assert_eq!(marks[0], (5, 11, Inline::Mention));
        assert_eq!(&text[marks[1].0..marks[1].1], "cargo test");
        assert_eq!(marks[1].2, Inline::Code);
    }

    #[test]
    fn a_mention_does_not_swallow_trailing_punctuation_or_emails() {
        let Block::Text { text, marks } = &parse("thanks @bruno.")[0] else {
            panic!()
        };
        assert_eq!(&text[marks[0].0..marks[0].1], "@bruno");
        let Block::Text { marks, .. } = &parse("mail a@b.co")[0] else {
            panic!()
        };
        assert!(marks.is_empty());
    }

    #[test]
    fn fenced_blocks_split_the_body_and_keep_their_lines() {
        let b = parse("look:\n```rust\nfn main() {}\n\nlet x = `1`;\n```\nthat's it");
        assert_eq!(b.len(), 3);
        assert_eq!(
            b[1],
            Block::Code {
                lang: Some("rust".into()),
                code: "fn main() {}\n\nlet x = `1`;".into()
            }
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end_and_a_lone_tick_is_text() {
        assert_eq!(
            parse("```\nabc"),
            vec![Block::Code {
                lang: None,
                code: "abc".into()
            }]
        );
        let Block::Text { text, marks } = &parse("it`s fine")[0] else {
            panic!()
        };
        assert_eq!((text.as_str(), marks.len()), ("it`s fine", 0));
    }

    #[test]
    fn flat_parse_finds_mentions_in_markup_but_not_in_code() {
        let has = |body: &str| {
            parse(body).iter().any(|b| {
                matches!(b, Block::Text { marks, .. } if marks.iter().any(|m| m.2 == Inline::Mention))
            })
        };
        assert!(has("**hey @ana**"));
        assert!(has("> quoted @ana"));
        assert!(has("- item @ana"));
        assert!(has("||secret @ana||"));
        assert!(!has("`@ana`"));
        assert!(!has("```\n@ana\n```"));
        assert!(!has("\\@ana"));
    }

    // Presses on rendered links and spoilers, through the real runtime.
    use rune::{Cx, Env, Runtime};

    struct S {
        body: &'static str,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let k = crate::kit(true);
        let wire = wire::<S>();
        col()
            .w(400.0)
            .child(
                render_with(k, s.body, "msg", Some(&wire))
                    .key("body"),
            )
    }

    fn runtime(body: &'static str) -> Runtime<S> {
        let mut rt = Runtime::new(
            S { body },
            view,
            (400.0, 200.0),
            Env { system_dark: true, scale: 1.0 },
        );
        for _ in 0..5 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    fn click(rt: &mut Runtime<S>, key: &str) {
        // A little in from the left edge: the text, not the empty rest of the full-width block.
        let r = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("no element {key}"))
            .rect;
        let (x, y) = (r[0] + 25.0, r[1] + 8.0);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
        }
    }

    #[test]
    fn clicking_a_spoiler_reveals_it() {
        let mut rt = runtime("||secret||");
        assert!(!spoiler_revealed("msg", 0));
        click(&mut rt, "body");
        assert!(spoiler_revealed("msg", 0), "the press revealed spoiler 0");
        assert!(!spoiler_revealed("other", 0), "reveal state is per message");
        assert!(rt.take_url_requests().is_empty(), "a spoiler opens no link");
        // Revealed spoilers carry no press target any more.
        click(&mut rt, "body");
        assert!(rt.take_url_requests().is_empty());
    }

    #[test]
    fn clicking_a_link_asks_to_open_its_url() {
        let mut rt = runtime("https://example.com/a");
        click(&mut rt, "body");
        assert_eq!(rt.take_url_requests(), ["https://example.com/a"]);
        let mut rt = runtime("[docs](https://example.com/docs)");
        click(&mut rt, "body");
        assert_eq!(rt.take_url_requests(), ["https://example.com/docs"]);
    }

    #[test]
    fn a_link_inside_a_hidden_spoiler_does_not_open() {
        let mut rt = runtime("||https://example.com/s||");
        click(&mut rt, "body");
        assert!(rt.take_url_requests().is_empty(), "the press revealed instead");
        assert!(spoiler_revealed("msg", 0));
    }
}
