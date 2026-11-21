//! The message markup parser: a Discord-flavoured CommonMark subset, pure (no kit) so it is
//! unit-tested. [`parse`] gives blocks; inline text is stripped of its markers and carries
//! [`Mark`]s as byte ranges of the stripped text, which `body::render` turns into Rune spans.
//!
//! Why not `rune_kit::Markdown`: it is plain CommonMark (no `||spoiler||`, no @mentions or
//! emoji tokens, `__x__` is bold where Discord underlines) and it renders whole documents, so it
//! cannot compose inline with our mention chips or hide a spoiler. A small parser with the
//! Discord semantics is simpler than bending it, adds no dependency, and is linear:
//!
//! - blocks are line based, recursion is capped at [`MAX_BLOCK_DEPTH`], each level rescans its
//!   lines once;
//! - inline delimiters pair with per-kind stacks (every token pushed and popped at most once),
//!   at most [`MAX_OPEN`] open at a time, at most [`MAX_MARKS`] marks per text;
//! - code spans and URLs scan forward only a bounded distance, or remember that a closer
//!   does not exist.
//!
//! Syntax: see the README ("Message markup").

/// Block nesting (quotes inside lists inside quotes...). Deeper lines are plain text.
pub const MAX_BLOCK_DEPTH: usize = 4;
/// Inline delimiters open at once, so formatting nests at most this deep.
pub const MAX_OPEN: usize = 16;
/// Marks one text can carry; further markers stay literal.
pub const MAX_MARKS: usize = 4096;
/// Source bytes parsed as markup; the rest of a longer message is one plain paragraph.
pub const MAX_PARSE_BYTES: usize = 64 * 1024;
/// Longest link destination or bare URL.
pub const MAX_URL: usize = 2048;
/// Longest run of backticks that opens a code span.
const MAX_TICKS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Bold,
    Italic,
    /// `__x__` (Discord's meaning, not bold).
    Underline,
    Strike,
    Code,
    Mention,
    Spoiler,
    /// A link: the range is the visible text, the string the destination (http, https, mailto).
    Link(String),
    /// `<:name:id>` / `<a:name:id>`; the text holds `:name:` until emoji render.
    Emoji {
        name: String,
        id: String,
        animated: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Display text (markers stripped) and the marks over it. Marks are ordered by `end`
/// (inner ones first when nested); sort if you need another order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inlines {
    pub text: String,
    pub marks: Vec<Mark>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Inlines),
    /// Level 1 to 3.
    Heading(u8, Inlines),
    Code {
        lang: Option<String>,
        code: String,
    },
    Quote(Vec<Block>),
    /// `start` is `Some(first number)` for an ordered list.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
}

/// Whether `url` may become a link: http, https or mailto (any case), with something after the
/// scheme, no whitespace or control characters, and at most [`MAX_URL`] bytes.
pub fn safe_url(url: &str) -> bool {
    if url.len() > MAX_URL || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let lower = |n: usize| url.get(..n).map(str::to_ascii_lowercase);
    let rest = if lower(8).as_deref() == Some("https://") {
        8
    } else if matches!(lower(7).as_deref(), Some("http://" | "mailto:")) {
        7
    } else {
        return false;
    };
    url.len() > rest
}

pub fn parse(src: &str) -> Vec<Block> {
    let (head, tail) = if src.len() > MAX_PARSE_BYTES {
        let mut cut = MAX_PARSE_BYTES;
        while !src.is_char_boundary(cut) {
            cut -= 1;
        }
        // Prefer ending on a line so a block is not cut mid-line.
        if let Some(nl) = src[..cut].rfind('\n') {
            cut = nl + 1;
        }
        src.split_at(cut)
    } else {
        (src, "")
    };
    let lines: Vec<&str> = head
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut blocks = blocks(&lines, 0);
    if !tail.trim().is_empty() {
        blocks.push(Block::Paragraph(Inlines {
            text: tail.trim_matches('\n').to_owned(),
            marks: Vec::new(),
        }));
    }
    blocks
}

// ---- blocks ----

fn fence_of(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("```")
}

fn is_quote(line: &str) -> bool {
    line == ">" || line.starts_with("> ")
}

fn heading_of(line: &str) -> Option<(u8, &str)> {
    for (n, p) in [(1u8, "# "), (2, "## "), (3, "### ")] {
        if let Some(rest) = line.strip_prefix(p)
            && !rest.trim().is_empty()
        {
            return Some((n, rest.trim()));
        }
    }
    None
}

/// `(ordered start, bytes of marker + space)` when `line` opens a list item at column 0.
fn list_marker(line: &str) -> Option<(Option<u64>, usize)> {
    if line.starts_with("- ") || line.starts_with("* ") {
        return (!line[2..].trim().is_empty()).then_some((None, 2));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        if (rest.starts_with(". ") || rest.starts_with(") ")) && !rest[2..].trim().is_empty() {
            return Some((line[..digits].parse().ok(), digits + 2));
        }
    }
    None
}

fn starts_block(line: &str) -> bool {
    fence_of(line).is_some()
        || is_quote(line)
        || line.starts_with(">>> ")
        || heading_of(line).is_some()
        || list_marker(line).is_some()
}

fn blocks(lines: &[&str], depth: usize) -> Vec<Block> {
    let mut out = Vec::new();
    let nest = depth < MAX_BLOCK_DEPTH;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(after) = fence_of(line) {
            if let Some(end) = after.find("```") {
                // One line: ```code```
                out.push(Block::Code {
                    lang: None,
                    code: after[..end].to_owned(),
                });
                i += 1;
                continue;
            }
            let lang = after.trim();
            let mut j = i + 1;
            while j < lines.len() && fence_of(lines[j]).is_none() {
                j += 1;
            }
            out.push(Block::Code {
                lang: (!lang.is_empty()).then(|| lang.to_owned()),
                code: lines[i + 1..j.min(lines.len())].join("\n"),
            });
            i = j + 1;
            continue;
        }
        if nest && let Some(rest) = line.strip_prefix(">>> ") {
            let mut inner = vec![rest];
            inner.extend_from_slice(&lines[i + 1..]);
            out.push(Block::Quote(blocks(&inner, depth + 1)));
            break;
        }
        if nest && is_quote(line) {
            let mut inner = Vec::new();
            while i < lines.len() && is_quote(lines[i]) {
                inner.push(lines[i].strip_prefix("> ").unwrap_or(""));
                i += 1;
            }
            out.push(Block::Quote(blocks(&inner, depth + 1)));
            continue;
        }
        if let Some((level, rest)) = heading_of(line) {
            out.push(Block::Heading(level, inline(rest)));
            i += 1;
            continue;
        }
        if nest && let Some((start, _)) = list_marker(line) {
            let ordered = start.is_some();
            let mut items = Vec::new();
            while i < lines.len() {
                let Some((_, width)) = list_marker(lines[i]).filter(|(s, _)| s.is_some() == ordered)
                else {
                    break;
                };
                let mut inner = vec![&lines[i][width..]];
                i += 1;
                while i < lines.len() && lines[i].starts_with("  ") && !lines[i].trim().is_empty() {
                    let l = lines[i];
                    let indent = l.bytes().take_while(|&b| b == b' ').count().min(width);
                    inner.push(&l[indent..]);
                    i += 1;
                }
                items.push(blocks(&inner, depth + 1));
            }
            out.push(Block::List { start, items });
            continue;
        }
        // A paragraph: lines up to the next block start; interior blank lines stay.
        let mut j = i;
        while j < lines.len() && (j == i || !starts_block(lines[j])) {
            j += 1;
        }
        let text = lines[i..j].join("\n");
        let text = text.trim_matches('\n');
        if !text.trim().is_empty() {
            out.push(Block::Paragraph(inline(text)));
        }
        i = j;
    }
    out
}

// ---- inline ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Literal,
    Open,
    Close,
}

/// The six delimiter stacks: `*`, `**`, `_`, `__`, `~~`, `||`.
const STACKS: usize = 6;

fn stack_kind(stack: usize) -> Kind {
    match stack {
        0 | 2 => Kind::Italic,
        1 => Kind::Bold,
        3 => Kind::Underline,
        4 => Kind::Strike,
        _ => Kind::Spoiler,
    }
}

#[derive(Clone)]
enum Ev {
    /// One literal char of the output.
    Lit(char),
    Delim {
        stack: usize,
        /// The marker as written, kept when it does not pair.
        text: &'static str,
        role: Role,
    },
    /// Output text that carries a mark by itself.
    Atom(String, Kind),
    LinkOpen(String),
    LinkClose,
}

fn is_mention_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn starts_with_ci(chars: &[char], at: usize, pat: &str) -> bool {
    pat.chars().enumerate().all(|(k, p)| {
        chars
            .get(at + k)
            .is_some_and(|c| c.to_ascii_lowercase() == p)
    })
}

/// End (exclusive) of a bare URL starting at `at`, trailing punctuation and markers trimmed.
fn url_end(chars: &[char], at: usize, scheme: usize) -> Option<usize> {
    let mut end = at;
    while end < chars.len() && end - at <= MAX_URL && !chars[end].is_whitespace() && chars[end] != '<' {
        end += 1;
    }
    if end - at > MAX_URL {
        return None;
    }
    loop {
        let last = chars[end - 1];
        let open = chars[at..end].iter().filter(|&&c| c == '(').count();
        let close = chars[at..end].iter().filter(|&&c| c == ')').count();
        let trim = matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | ']' | '}' | '>' | '*' | '_' | '~' | '|'
        ) || (last == ')' && close > open);
        if !trim || end - at <= scheme {
            break;
        }
        end -= 1;
    }
    (end - at > scheme).then_some(end)
}

/// A `[label](destination)` whose `]` is `close`: its destination and the index after `)`.
fn link_dest(chars: &[char], close: usize) -> Option<(String, usize)> {
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut p = close + 2;
    let angle = chars.get(p) == Some(&'<');
    if angle {
        p += 1;
    }
    let start = p;
    let mut depth = 0usize;
    while p < chars.len() && p - start <= MAX_URL {
        let c = chars[p];
        if angle {
            if c == '>' {
                break;
            }
        } else if c.is_whitespace() {
            break;
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            if depth == 0 {
                break;
            }
            depth -= 1;
        }
        p += 1;
    }
    let dest: String = chars[start..p.min(chars.len())].iter().collect();
    if angle {
        p += 1;
    }
    while chars.get(p).is_some_and(|c| c.is_whitespace()) {
        p += 1;
    }
    if chars.get(p) == Some(&'"') {
        // An optional title, ignored.
        let limit = (p + 257).min(chars.len());
        p += 1;
        while p < limit && chars[p] != '"' {
            p += 1;
        }
        if chars.get(p) != Some(&'"') {
            return None;
        }
        p += 1;
        while chars.get(p).is_some_and(|c| c.is_whitespace()) {
            p += 1;
        }
    }
    (chars.get(p) == Some(&')') && safe_url(&dest)).then_some((dest, p + 1))
}

/// `<:name:id>` or `<a:name:id>` at `at`: its end (exclusive) and parts.
fn emoji_at(chars: &[char], at: usize) -> Option<(usize, String, String, bool)> {
    let mut p = at + 1;
    let animated = chars.get(p) == Some(&'a');
    if animated {
        p += 1;
    }
    if chars.get(p) != Some(&':') {
        return None;
    }
    p += 1;
    let n0 = p;
    while p < chars.len() && p - n0 <= 32 && (chars[p].is_ascii_alphanumeric() || chars[p] == '_') {
        p += 1;
    }
    if !(2..=32).contains(&(p - n0)) || chars.get(p) != Some(&':') {
        return None;
    }
    let name: String = chars[n0..p].iter().collect();
    p += 1;
    let i0 = p;
    while p < chars.len() && p - i0 <= 24 && chars[p].is_ascii_digit() {
        p += 1;
    }
    if !(1..=24).contains(&(p - i0)) || chars.get(p) != Some(&'>') {
        return None;
    }
    let id: String = chars[i0..p].iter().collect();
    Some((p + 1, name, id, animated))
}

/// Matching `]` for each `[` (not escaped), by a single stack pass.
fn bracket_matches(chars: &[char]) -> Vec<Option<usize>> {
    let mut out = vec![None; chars.len()];
    let mut open = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => open.push(i),
            ']' => {
                if let Some(o) = open.pop() {
                    out[o] = Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn delim_text(stack: usize) -> &'static str {
    ["*", "**", "_", "__", "~~", "||"][stack]
}

fn inline(src: &str) -> Inlines {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let close_of = bracket_matches(&chars);
    let mut evs: Vec<Ev> = Vec::new();
    // (open event, sequence) per stack.
    let mut stacks: [Vec<(usize, usize)>; STACKS] = Default::default();
    let (mut seq, mut open_total, mut pairs) = (0usize, 0usize, 0usize);
    let mut no_close = [false; MAX_TICKS + 1];
    let mut link_end: Option<(usize, usize)> = None; // (the label's `]`, index after `)`)
    let mut i = 0;

    while i < n {
        if let Some((close, after)) = link_end
            && i >= close
        {
            // (A code span or URL inside the label can jump past the `]`; the link ends there.)
            evs.push(Ev::LinkClose);
            link_end = None;
            if i == close {
                i = after;
                continue;
            }
        }
        let c = chars[i];
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let word_start = prev.is_none_or(|p| !p.is_alphanumeric());
        match c {
            '\\' if chars.get(i + 1).is_some_and(char::is_ascii_punctuation) => {
                evs.push(Ev::Lit(chars[i + 1]));
                i += 2;
            }
            '`' => {
                let run = chars[i..].iter().take_while(|&&x| x == '`').count();
                let mut done = false;
                if run <= MAX_TICKS && !no_close[run] {
                    let mut p = i + run;
                    while p < n {
                        if chars[p] == '`' {
                            let r = chars[p..].iter().take_while(|&&x| x == '`').count();
                            if r == run {
                                let code: String = chars[i + run..p].iter().collect();
                                evs.push(Ev::Atom(code, Kind::Code));
                                i = p + r;
                                done = true;
                                break;
                            }
                            p += r;
                        } else {
                            p += 1;
                        }
                    }
                    if !done {
                        no_close[run] = true;
                    }
                }
                if !done {
                    evs.extend(std::iter::repeat_n(Ev::Lit('`'), run));
                    i += run;
                }
            }
            '[' if link_end.is_none() && pairs < MAX_MARKS => {
                let link = close_of[i].and_then(|j| {
                    let label_blank = chars[i + 1..j].iter().all(|c| c.is_whitespace());
                    (!label_blank)
                        .then(|| link_dest(&chars, j))
                        .flatten()
                        .map(|(url, after)| (j, after, url))
                });
                if let Some((j, after, url)) = link {
                    evs.push(Ev::LinkOpen(url));
                    link_end = Some((j, after));
                    pairs += 1;
                } else {
                    evs.push(Ev::Lit('['));
                }
                i += 1;
            }
            '<' => {
                if let Some((end, name, id, animated)) = emoji_at(&chars, i) {
                    evs.push(Ev::Atom(
                        format!(":{name}:"),
                        Kind::Emoji { name, id, animated },
                    ));
                    i = end;
                } else if link_end.is_none()
                    && (starts_with_ci(&chars, i + 1, "https://")
                        || starts_with_ci(&chars, i + 1, "http://"))
                    && let Some(close) = chars[i + 1..]
                        .iter()
                        .take(MAX_URL + 1)
                        .position(|&x| x == '>' || x.is_whitespace())
                        .filter(|&p| chars[i + 1 + p] == '>')
                    && safe_url(&chars[i + 1..i + 1 + close].iter().collect::<String>())
                {
                    let url: String = chars[i + 1..i + 1 + close].iter().collect();
                    evs.push(Ev::Atom(url.clone(), Kind::Link(url)));
                    i += close + 2;
                } else {
                    evs.push(Ev::Lit('<'));
                    i += 1;
                }
            }
            'h' | 'H' if link_end.is_none() && word_start => {
                let scheme = if starts_with_ci(&chars, i, "https://") {
                    8
                } else if starts_with_ci(&chars, i, "http://") {
                    7
                } else {
                    0
                };
                match (scheme > 0).then(|| url_end(&chars, i, scheme)).flatten() {
                    Some(end) => {
                        let url: String = chars[i..end].iter().collect();
                        evs.push(Ev::Atom(url.clone(), Kind::Link(url)));
                        i = end;
                    }
                    None => {
                        evs.push(Ev::Lit(c));
                        i += 1;
                    }
                }
            }
            '@' if word_start => {
                let run = chars[i + 1..].iter().take_while(|&&x| is_mention_char(x)).count();
                // Trailing '.', '-' (and '_', so `__@ana__` closes) are not part of the name.
                let name = chars[i + 1..i + 1 + run]
                    .iter()
                    .rposition(|&x| !matches!(x, '.' | '-' | '_'))
                    .map_or(0, |p| p + 1);
                if name > 0 {
                    let s: String = chars[i..i + 1 + name].iter().collect();
                    evs.push(Ev::Atom(s, Kind::Mention));
                    i += name + 1;
                } else {
                    evs.push(Ev::Lit('@'));
                    i += 1;
                }
            }
            '*' | '_' | '~' | '|' => {
                let run = chars[i..].iter().take_while(|&&x| x == c).count();
                let sizes: &[usize] = match (c, run) {
                    ('*' | '_', 1) => &[1],
                    ('*' | '_', 2) => &[2],
                    ('*' | '_', 3) => &[1, 2],
                    ('~' | '|', 2) => &[2],
                    _ => &[],
                };
                if sizes.is_empty() {
                    evs.extend(std::iter::repeat_n(Ev::Lit(c), run));
                    i += run;
                    continue;
                }
                let ws_prev = prev.is_none_or(char::is_whitespace);
                let next = chars.get(i + run).copied();
                let ws_next = next.is_none_or(char::is_whitespace);
                let under = c == '_';
                let can_open = !ws_next && (!under || prev.is_none_or(|p| !p.is_alphanumeric()));
                let can_close = !ws_prev && (!under || next.is_none_or(|x| !x.is_alphanumeric()));
                let base = match c {
                    '*' => 0,
                    '_' => 2,
                    '~' => 4,
                    _ => 5,
                };
                let stack_of = |size: usize| match c {
                    '*' | '_' => base + size - 1,
                    _ => base,
                };
                // `***x***`: as a closer the longer marker closes first, as an opener last.
                let closing = can_close && sizes.iter().any(|&s| !stacks[stack_of(s)].is_empty());
                let mut order: Vec<usize> = sizes.to_vec();
                if closing {
                    order.reverse();
                }
                for size in order {
                    let stack = stack_of(size);
                    let at = evs.len();
                    let mut role = Role::Literal;
                    let mut closed = false;
                    if can_close
                        && pairs < MAX_MARKS
                        && let Some(&(open_at, s)) = stacks[stack].last()
                        && at > open_at + 1
                    {
                        stacks[stack].pop();
                        open_total -= 1;
                        pairs += 1;
                        closed = true;
                        role = Role::Close;
                        if let Ev::Delim { role: r, .. } = &mut evs[open_at] {
                            *r = Role::Open;
                        }
                        // Openers of other kinds still inside this pair can no longer pair.
                        for other in stacks.iter_mut() {
                            while other.last().is_some_and(|&(_, t)| t > s) {
                                other.pop();
                                open_total -= 1;
                            }
                        }
                    }
                    if !closed && can_open && open_total < MAX_OPEN {
                        stacks[stack].push((at, seq));
                        seq += 1;
                        open_total += 1;
                    }
                    evs.push(Ev::Delim {
                        stack,
                        text: delim_text(stack),
                        role,
                    });
                }
                i += run;
            }
            _ => {
                evs.push(Ev::Lit(c));
                i += 1;
            }
        }
    }

    // Emit: unpaired markers return as text; paired ones become marks.
    let mut text = String::with_capacity(src.len());
    let mut marks = Vec::new();
    let mut open_stack: [Vec<usize>; STACKS] = Default::default();
    let mut link_open: Option<(usize, String)> = None;
    for ev in evs {
        match ev {
            Ev::Lit(c) => text.push(c),
            Ev::Atom(s, kind) => {
                let start = text.len();
                text.push_str(&s);
                marks.push(Mark {
                    start,
                    end: text.len(),
                    kind,
                });
            }
            Ev::Delim { stack, text: t, role } => match role {
                Role::Literal => text.push_str(t),
                Role::Open => open_stack[stack].push(text.len()),
                Role::Close => {
                    if let Some(start) = open_stack[stack].pop() {
                        marks.push(Mark {
                            start,
                            end: text.len(),
                            kind: stack_kind(stack),
                        });
                    }
                }
            },
            Ev::LinkOpen(url) => link_open = Some((text.len(), url)),
            Ev::LinkClose => {
                if let Some((start, url)) = link_open.take() {
                    marks.push(Mark {
                        start,
                        end: text.len(),
                        kind: Kind::Link(url),
                    });
                }
            }
        }
    }
    marks.retain(|m| m.start < m.end);
    Inlines { text, marks }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(src: &str) -> Inlines {
        match parse(src).into_iter().next() {
            Some(Block::Paragraph(i)) => i,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    /// `(marked text, kind)` for each mark, by start.
    fn marked(src: &str) -> Vec<(String, Kind)> {
        let i = one(src);
        let mut m = i.marks.clone();
        m.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
        m.into_iter()
            .map(|m| (i.text[m.start..m.end].to_owned(), m.kind))
            .collect()
    }

    #[test]
    fn plain_text_has_no_marks() {
        let i = one("hello there");
        assert_eq!((i.text.as_str(), i.marks.len()), ("hello there", 0));
    }

    #[test]
    fn bold_italic_strike_underline_spoiler() {
        assert_eq!(marked("a **b** c"), [("b".into(), Kind::Bold)]);
        assert_eq!(marked("a *b* c"), [("b".into(), Kind::Italic)]);
        assert_eq!(marked("a _b_ c"), [("b".into(), Kind::Italic)]);
        assert_eq!(marked("a ~~b~~ c"), [("b".into(), Kind::Strike)]);
        assert_eq!(marked("a __b__ c"), [("b".into(), Kind::Underline)]);
        assert_eq!(marked("a ||b|| c"), [("b".into(), Kind::Spoiler)]);
        assert_eq!(one("a **b** c").text, "a b c");
    }

    #[test]
    fn triple_markers_nest_both() {
        let m = marked("***both***");
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|(t, _)| t == "both"));
        assert!(m.iter().any(|(_, k)| *k == Kind::Bold));
        assert!(m.iter().any(|(_, k)| *k == Kind::Italic));
        let m = marked("___both___");
        assert!(m.iter().any(|(_, k)| *k == Kind::Underline));
        assert!(m.iter().any(|(_, k)| *k == Kind::Italic));
    }

    #[test]
    fn nesting_marks_inner_ranges() {
        let m = marked("**bold *and italic* too**");
        assert_eq!(m[0], ("bold and italic too".into(), Kind::Bold));
        assert_eq!(m[1], ("and italic".into(), Kind::Italic));
        let m = marked("~~gone **loud**~~");
        assert_eq!(m[0].1, Kind::Strike);
        assert_eq!(m[1], ("loud".into(), Kind::Bold));
    }

    #[test]
    fn unpaired_and_spaced_markers_stay_literal() {
        assert_eq!(one("2 * 3 * 4").text, "2 * 3 * 4");
        assert_eq!(one("a **b").text, "a **b");
        assert_eq!(one("snake__case__word").text, "snake__case__word");
        assert_eq!(one("snake_case_word").marks.len(), 0);
        assert_eq!(one("a ~ b ~~ c").marks.len(), 0);
        assert_eq!(one("****").text, "****");
        assert_eq!(one("|| ||").marks.len(), 0);
    }

    #[test]
    fn inline_code_protects_markup_and_mentions() {
        let m = marked("run `**x** @ana https://a.co` now");
        assert_eq!(m, [("**x** @ana https://a.co".into(), Kind::Code)]);
        let i = one("``a ` b`` and `c`");
        assert_eq!(i.text, "a ` b and c");
        assert_eq!(one("it`s fine").text, "it`s fine");
        assert_eq!(one("it`s fine").marks.len(), 0);
        assert_eq!(one("`**not bold`").marks.len(), 1);
    }

    #[test]
    fn mentions_keep_their_rules_and_work_inside_formatting() {
        let m = marked("ping @ana.k, thanks @bruno.");
        assert_eq!(m[0], ("@ana.k".into(), Kind::Mention));
        assert_eq!(m[1], ("@bruno".into(), Kind::Mention));
        assert!(marked("mail a@b.co").is_empty());
        let m = marked("**hi @ana**");
        assert_eq!(m[0], ("hi @ana".into(), Kind::Bold));
        assert_eq!(m[1], ("@ana".into(), Kind::Mention));
        assert!(marked("__@ana__").contains(&("@ana".into(), Kind::Mention)));
        assert!(marked("||@ana||").contains(&("@ana".into(), Kind::Mention)));
        assert!(marked("||@ana||").contains(&("@ana".into(), Kind::Spoiler)));
    }

    #[test]
    fn a_mention_that_is_all_the_emphasis_keeps_both() {
        for (src, kind) in [("**@tim**", Kind::Bold), ("*@tim*", Kind::Italic), ("_@tim_", Kind::Italic), ("__@tim__", Kind::Underline), ("~~@tim~~", Kind::Strike)] {
            let i = one(src);
            assert_eq!(i.text, "@tim", "{src}");
            let m = marked(src);
            assert!(m.contains(&("@tim".into(), Kind::Mention)), "{src}: {m:?}");
            assert!(m.contains(&("@tim".into(), kind)), "{src}: {m:?}");
        }
        let m = marked("***@tim***");
        assert!([Kind::Bold, Kind::Italic, Kind::Mention].iter().all(|k| m.contains(&("@tim".into(), k.clone()))), "{m:?}");
        let m = marked("hey **@tim, look**");
        assert!(m.contains(&("@tim, look".into(), Kind::Bold)) && m.contains(&("@tim".into(), Kind::Mention)), "{m:?}");
    }

    #[test]
    fn emoji_tokens_become_name_text() {
        let i = one("hi <:blob:123456> and <a:wave:9>!");
        assert_eq!(i.text, "hi :blob: and :wave:!");
        assert_eq!(
            i.marks[0].kind,
            Kind::Emoji {
                name: "blob".into(),
                id: "123456".into(),
                animated: false
            }
        );
        assert!(matches!(
            i.marks[1].kind,
            Kind::Emoji { animated: true, .. }
        ));
        // Not tokens: too-short name, no id, in code, escaped.
        assert_eq!(one("<:a:1>").marks.len(), 0);
        assert_eq!(one("<:blob:x>").marks.len(), 0);
        assert_eq!(one("`<:blob:1>`").text, "<:blob:1>");
        assert_eq!(one("`<:blob:1>`").marks[0].kind, Kind::Code);
        assert_eq!(one("\\<:blob:1>").marks.len(), 0);
    }

    #[test]
    fn links_masked_and_bare() {
        let i = one("see [the docs](https://example.com/a?b=1) and https://rust-lang.org.");
        assert_eq!(i.text, "see the docs and https://rust-lang.org.");
        let links: Vec<_> = i
            .marks
            .iter()
            .filter_map(|m| match &m.kind {
                Kind::Link(u) => Some((&i.text[m.start..m.end], u.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            links,
            [
                ("the docs", "https://example.com/a?b=1"),
                ("https://rust-lang.org", "https://rust-lang.org")
            ]
        );
        // Trailing punctuation, markers and unbalanced parens are not the link.
        let u = |s: &str| match &marked(s)[0].1 {
            Kind::Link(u) => u.clone(),
            k => panic!("{k:?}"),
        };
        assert_eq!(u("(see https://a.co/x)"), "https://a.co/x");
        assert_eq!(u("https://en.wikipedia.org/wiki/Rust_(language)"), "https://en.wikipedia.org/wiki/Rust_(language)");
        assert_eq!(u("**https://a.co**"), "https://a.co");
        assert_eq!(u("<https://a.co/x_y>"), "https://a.co/x_y");
        assert_eq!(u("mail [me](mailto:a@b.co)"), "mailto:a@b.co");
    }

    #[test]
    fn formatting_inside_a_masked_link_label() {
        let m = marked("[**big** news](https://a.co)");
        assert!(m.iter().any(|(t, k)| t == "big news" && matches!(k, Kind::Link(_))));
        assert!(m.iter().any(|(t, k)| t == "big" && *k == Kind::Bold));
        // A bare URL inside a masked label is label text, not a second link.
        let i = one("[https://a.co](https://b.co)");
        assert_eq!(i.marks.len(), 1);
    }

    #[test]
    fn unsafe_links_stay_plain_text() {
        for src in [
            "[x](javascript:alert(1))",
            "[x](JavaScript:alert(1))",
            "[x](file:///etc/passwd)",
            "[x](data:text/html,hi)",
            "[x](//evil.com)",
            "[x](ftp://a.co)",
            "[x]( javascript:1)",
            "[x](https://)",
            "javascript:alert(1)",
            "file:///etc/passwd",
        ] {
            let i = one(src);
            assert!(
                i.marks.iter().all(|m| !matches!(m.kind, Kind::Link(_))),
                "{src}"
            );
            assert_eq!(i.text, src, "shown verbatim: {src}");
        }
        assert!(safe_url("HTTPS://a.co"));
        assert!(!safe_url("https://a b"));
        assert!(!safe_url("https://a\u{0}b"));
        assert!(!safe_url(&format!("https://{}", "a".repeat(MAX_URL))));
    }

    #[test]
    fn escapes_drop_the_backslash_and_block_markup() {
        assert_eq!(one(r"\*not italic\*").text, "*not italic*");
        assert_eq!(one(r"\*not italic\*").marks.len(), 0);
        assert_eq!(one(r"\@ana").marks.len(), 0);
        assert_eq!(one(r"\[x\](https://a.co)").marks.len(), 1); // the bare URL only
        assert_eq!(one(r"a \d b").text, r"a \d b");
        assert_eq!(one(r"\\*x*").text, r"\x");
        assert_eq!(one(r"\|\|x||").marks.len(), 0);
    }

    #[test]
    fn fenced_blocks_keep_their_lines_and_language() {
        let b = parse("look:\n```rust\nfn main() {}\n\nlet x = `1`; **y**\n```\nthat's it");
        assert_eq!(b.len(), 3);
        assert_eq!(
            b[1],
            Block::Code {
                lang: Some("rust".into()),
                code: "fn main() {}\n\nlet x = `1`; **y**".into()
            }
        );
        assert_eq!(
            parse("```\nabc"),
            [Block::Code {
                lang: None,
                code: "abc".into()
            }]
        );
        assert_eq!(
            parse("```a *b*```"),
            [Block::Code {
                lang: None,
                code: "a *b*".into()
            }]
        );
    }

    #[test]
    fn headings_only_at_line_start() {
        let b = parse("# One\n## Two\n### Three\n#### Four\nnot # a heading\n#nospace");
        assert!(matches!(&b[0], Block::Heading(1, i) if i.text == "One"));
        assert!(matches!(&b[1], Block::Heading(2, _)));
        assert!(matches!(&b[2], Block::Heading(3, _)));
        let Block::Paragraph(p) = &b[3] else { panic!() };
        assert_eq!(p.text, "#### Four\nnot # a heading\n#nospace");
        let Block::Heading(_, h) = &parse("# **b** @ana")[0] else {
            panic!()
        };
        assert_eq!(h.marks.len(), 2);
    }

    #[test]
    fn quotes_nest_and_hold_blocks() {
        let b = parse("> a **b**\n> second\nafter");
        let Block::Quote(inner) = &b[0] else { panic!() };
        assert!(matches!(&inner[0], Block::Paragraph(p) if p.text == "a b\nsecond"));
        assert!(matches!(&b[1], Block::Paragraph(p) if p.text == "after"));
        let b = parse(">>> all\nof this\n- and this");
        assert_eq!(b.len(), 1);
        let Block::Quote(inner) = &b[0] else { panic!() };
        assert!(matches!(inner.last(), Some(Block::List { .. })));
        assert!(matches!(&parse(">not a quote")[0], Block::Paragraph(_)));
        let b = parse("> ```\n> code\n> ```");
        let Block::Quote(inner) = &b[0] else { panic!() };
        assert!(matches!(&inner[0], Block::Code { code, .. } if code == "code"));
    }

    #[test]
    fn lists_bulleted_ordered_and_nested() {
        let b = parse("- a\n- b **x**\n* c");
        let Block::List { start, items } = &b[0] else { panic!() };
        assert_eq!((start, items.len()), (&None, 3));
        let b = parse("3. a\n4. b\n\ntext");
        let Block::List { start, items } = &b[0] else { panic!() };
        assert_eq!((*start, items.len()), (Some(3), 2));
        assert!(matches!(&b[1], Block::Paragraph(_)));
        let b = parse("- a\n  - nested\n  - nested 2\n- b");
        let Block::List { items, .. } = &b[0] else { panic!() };
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0][1], Block::List { items, .. } if items.len() == 2));
        // `*italic*` and `-5` are not list items.
        assert!(matches!(&parse("*italic* text")[0], Block::Paragraph(_)));
        assert!(matches!(&parse("-5 degrees")[0], Block::Paragraph(_)));
        assert!(matches!(&parse("2024. A year")[0], Block::List { .. }));
    }

    #[test]
    fn newlines_stay_in_the_paragraph() {
        assert_eq!(one("a\nb\n\nc").text, "a\nb\n\nc");
        assert_eq!(parse("\n\n").len(), 0);
    }

    #[test]
    fn block_nesting_is_capped() {
        let src = format!("{}deep", "> ".repeat(50));
        let mut b = parse(&src);
        let mut depth = 0;
        while let Some(Block::Quote(inner)) = b.into_iter().next() {
            depth += 1;
            b = inner;
        }
        assert_eq!(depth, MAX_BLOCK_DEPTH);
    }

    #[test]
    fn inline_nesting_is_capped_and_still_terminates() {
        let src = format!("{}x{}", "*a _b ~~c ||d ".repeat(200), "|| ~~ _ *".repeat(200));
        let i = one(&src);
        assert!(i.marks.len() <= MAX_MARKS);
    }

    #[test]
    fn pathological_input_is_linear() {
        use std::time::{Duration, Instant};
        let n = 60_000;
        let cases: Vec<String> = vec![
            "*".repeat(n),
            "*a ".repeat(n / 3),
            "_a *b ".repeat(n / 6),
            "`a ``b ```c ".repeat(n / 12),
            "[a](".repeat(n / 4),
            "[a](https://x.co/".repeat(n / 17),
            "http://http://".repeat(n / 14),
            "@@@@a".repeat(n / 5),
            "<:a:1".repeat(n / 5),
            "||a ~~b ".repeat(n / 8),
            "> ".repeat(n / 2),
            "- ".repeat(n / 2) + "x",
            "\\".repeat(n),
            "```".repeat(n / 3),
            "**a** _b_ ~~c~~ ||d|| `e` @f https://g.co\n".repeat(n / 43),
        ];
        for c in &cases {
            let t = Instant::now();
            let _ = parse(c);
            assert!(
                t.elapsed() < Duration::from_secs(2),
                "slow on {:?}...: {:?}",
                &c[..c.len().min(12)],
                t.elapsed()
            );
        }
    }

    #[test]
    fn a_very_long_message_is_cut_to_plain_text() {
        let src = "**a**\n".repeat(MAX_PARSE_BYTES);
        let b = parse(&src);
        let Some(Block::Paragraph(last)) = b.last() else { panic!() };
        assert!(last.marks.is_empty() && last.text.contains("**a**"));
    }
}
