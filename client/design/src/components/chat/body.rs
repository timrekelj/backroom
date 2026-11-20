//! Message bodies: plain text with wrapping, `inline code`, ```fenced``` code blocks and
//! @mentions. [`parse`] is pure (no kit) so it is unit-tested; [`render`] turns blocks into
//! Rune elements, with inline marks as `Span`s on one wrapping text so lines break naturally.

use rune::{El, Span, col, text};
use rune_kit::Kit;

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

fn is_mention_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn inline(src: &str) -> Block {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut marks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`'
            && let Some(len) = chars[i + 1..]
                .iter()
                .position(|&x| x == '`')
                .filter(|&n| n > 0)
        {
            let start = out.len();
            out.extend(&chars[i + 1..i + 1 + len]);
            marks.push((start, out.len(), Inline::Code));
            i += len + 2;
            continue;
        }
        let at_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        if c == '@' && at_word_start {
            let n = chars[i + 1..]
                .iter()
                .take_while(|&&x| is_mention_char(x))
                .count();
            // A trailing '.' or '-' is sentence punctuation, not part of the name.
            let n = chars[i + 1..i + 1 + n]
                .iter()
                .rposition(|&x| x != '.' && x != '-')
                .map_or(0, |p| p + 1);
            if n > 0 {
                let start = out.len();
                out.extend(&chars[i..i + 1 + n]);
                marks.push((start, out.len(), Inline::Mention));
                i += n + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    Block::Text { text: out, marks }
}

/// Splits a body into text and fenced code blocks. An unclosed fence runs to the end.
pub fn parse(body: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut text_lines: Vec<&str> = Vec::new();
    let mut code: Option<(Option<String>, Vec<&str>)> = None;
    let flush = |lines: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        let joined = lines.join("\n");
        lines.clear();
        if !joined.trim().is_empty() {
            blocks.push(inline(joined.trim_matches('\n')));
        }
    };
    for line in body.split('\n') {
        let fence = line.trim_start().strip_prefix("```");
        match (&mut code, fence) {
            (None, Some(lang)) => {
                flush(&mut text_lines, &mut blocks);
                let lang = lang.trim();
                code = Some(((!lang.is_empty()).then(|| lang.to_owned()), Vec::new()));
            }
            (Some(_), Some(_)) => {
                let (lang, lines) = code.take().unwrap();
                blocks.push(Block::Code {
                    lang,
                    code: lines.join("\n"),
                });
            }
            (Some((_, lines)), None) => lines.push(line),
            (None, None) => text_lines.push(line),
        }
    }
    if let Some((lang, lines)) = code {
        blocks.push(Block::Code {
            lang,
            code: lines.join("\n"),
        });
    }
    flush(&mut text_lines, &mut blocks);
    blocks
}

/// The body as a column of blocks, 6pt apart.
pub fn render(k: &Kit, body: &str) -> El {
    let t = theme_of(k);
    let mut c = col().w_full().gap(6.0);
    for block in parse(body) {
        c = c.child(match block {
            Block::Text { text: s, marks } => {
                let spans: Vec<Span> = marks
                    .into_iter()
                    .map(|(a, b, kind)| match kind {
                        Inline::Code => Span::new(a, b)
                            .mono()
                            .size(12.5)
                            .bg(t.surface_raised.color(), 3.0),
                        Inline::Mention => Span::new(a, b)
                            .weight(560.0)
                            .color(t.accent_text.color())
                            .bg(t.accent_subtle.color(), 2.0),
                    })
                    .collect();
                text(s)
                    .font(type_::BODY)
                    .color(t.text.color())
                    .wrap()
                    .w_full()
                    .spans(spans)
            }
            Block::Code { code, .. } => col()
                .w_full()
                .px(12.0)
                .py(8.0)
                .radius(radius::MD)
                .fill(t.surface_raised.color())
                .border(1.0, t.border_subtle.color())
                .child(
                    text(code)
                        .font(type_::CODE)
                        .color(t.text.color())
                        .wrap()
                        .wrap_anywhere()
                        .w_full(),
                ),
        });
    }
    c
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
}
