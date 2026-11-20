//! Finds the URLs of a message that should get an embed.
//!
//! Rules:
//! - bare `http://` / `https://` URLs, URLs in angle brackets (`<url>`; unlike
//!   Discord, brackets do not opt out) and the destination of masked links
//!   `[text](url)` / `[text](<url>)` all count;
//! - the only URLs that do not are those inside inline code (`` `…` ``) or a
//!   code fence (```` ``` ````), unclosed fences included;
//! - duplicates (by normalised URL) count once; at most [`MAX_EMBED_URLS`].
//!
//! The returned strings are the URLs as written (so clients can match them in
//! the text), already checked to parse as http(s) URLs without credentials.

use std::collections::HashSet;

use url::Url;

/// URLs one message may unfurl.
pub const MAX_EMBED_URLS: usize = 3;
const MAX_URL_LEN: usize = 2048;

pub fn extract_urls(content: &str) -> Vec<String> {
    let text: Vec<char> = blank_code(content).chars().collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < text.len() && out.len() < MAX_EMBED_URLS {
        let (url, next) = match text[i] {
            '[' => markdown_link(&text, i).unwrap_or((None, i + 1)),
            'h' | 'H' if starts_with_scheme(&text, i) && (i == 0 || !text[i - 1].is_alphanumeric()) => bare(&text, i),
            _ => (None, i + 1),
        };
        i = next.max(i + 1);
        if let Some(url) = url
            && let Some(normalised) = parse(&url)
            && seen.insert(normalised)
        {
            out.push(url);
        }
    }
    out
}

/// Replaces code spans and fences by spaces (same length).
fn blank_code(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut out = chars.clone();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let run = chars[i..].iter().take_while(|c| **c == '`').count();
        // A run of three or more opens a fence; an unclosed one (or span) runs to the end,
        // an unmatched short run is plain text.
        let close = find_run(&chars, i + run, run);
        let end = match close {
            Some(at) => at + run,
            None if run >= 3 => chars.len(),
            None => {
                i += run;
                continue;
            }
        };
        out[i..end].iter_mut().for_each(|c| *c = ' ');
        i = end;
    }
    out.into_iter().collect()
}

/// Start of the next run of exactly `len` backticks at or after `from`.
fn find_run(chars: &[char], from: usize, len: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '`' {
            let run = chars[i..].iter().take_while(|c| **c == '`').count();
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

fn starts_with_scheme(text: &[char], at: usize) -> bool {
    let rest = &text[at.min(text.len())..];
    let matches = |s: &str| rest.len() >= s.len() && rest.iter().zip(s.chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b));
    matches("http://") || matches("https://")
}

/// A bare (or `<…>`-wrapped) URL: up to whitespace or `<`/`>`, minus trailing punctuation.
fn bare(text: &[char], at: usize) -> (Option<String>, usize) {
    let len = text[at..].iter().position(|c| c.is_whitespace() || matches!(c, '<' | '>')).unwrap_or(text.len() - at);
    let mut end = at + len;
    // Trailing prose punctuation, and closing brackets nobody opened inside the URL.
    while end > at {
        let c = text[end - 1];
        let unbalanced = |open: char| text[at..end].iter().filter(|x| **x == c).count() > text[at..end].iter().filter(|x| **x == open).count();
        let trim = matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '_' | '~' | '|')
            || (c == ')' && unbalanced('('))
            || (c == ']' && unbalanced('['));
        if !trim {
            break;
        }
        end -= 1;
    }
    (Some(text[at..end].iter().collect()), at + len)
}

/// `[text](url "title")` starting at `at` (`[`). `Some((url, next))`; `url` is
/// `None` when the destination is not http(s). `None` if this is not a
/// Markdown link.
fn markdown_link(text: &[char], at: usize) -> Option<(Option<String>, usize)> {
    // Link text: up to the matching `]` on the same line.
    let mut depth = 0;
    let mut close = None;
    for (n, c) in text[at..].iter().enumerate() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(at + n);
                    break;
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    let close = close?;
    if text.get(close + 1) != Some(&'(') {
        return None;
    }
    let start = close + 2;
    let angled = text.get(start) == Some(&'<');
    // Destination: `<…>` or non-space with balanced parentheses.
    let mut depth = 0;
    let mut end = start;
    if angled {
        end = start + text[start..].iter().position(|c| *c == '>' || *c == '\n')?;
        if text[end] != '>' {
            return None;
        }
        end += 1;
    } else {
        while end < text.len() {
            match text[end] {
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                c if c.is_whitespace() => break,
                _ => {}
            }
            end += 1;
        }
    }
    let dest: String = if angled { text[start + 1..end - 1].iter().collect() } else { text[start..end].iter().collect() };
    // Optional title, then the closing parenthesis.
    let mut rest = end;
    while text.get(rest).is_some_and(|c| c.is_whitespace() && *c != '\n') {
        rest += 1;
    }
    if matches!(text.get(rest), Some('"' | '\'')) {
        let quote = text[rest];
        let n = text[rest + 1..].iter().position(|c| *c == quote)?;
        rest += n + 2;
        while text.get(rest).is_some_and(|c| c.is_whitespace() && *c != '\n') {
            rest += 1;
        }
    }
    if text.get(rest) != Some(&')') {
        return None;
    }
    let url = starts_with_scheme(&dest.chars().collect::<Vec<_>>(), 0).then_some(dest);
    Some((url, rest + 1))
}

/// Normalised form of an acceptable URL, `None` if it must not be fetched.
fn parse(raw: &str) -> Option<String> {
    if raw.len() > MAX_URL_LEN {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let ok = matches!(url.scheme(), "http" | "https") && url.host().is_some() && url.username().is_empty() && url.password().is_none();
    ok.then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<String> {
        extract_urls(s)
    }

    #[test]
    fn bare_urls_with_punctuation() {
        assert_eq!(urls("see https://example.com/a?b=1, ok"), ["https://example.com/a?b=1"]);
        assert_eq!(urls("(http://example.com/x) and \"https://b.org/\"."), ["http://example.com/x", "https://b.org/"]);
        assert_eq!(urls("https://en.wikipedia.org/wiki/Rust_(language)."), ["https://en.wikipedia.org/wiki/Rust_(language)"]);
        assert_eq!(urls("HTTPS://EXAMPLE.COM/X"), ["HTTPS://EXAMPLE.COM/X"]);
        assert_eq!(urls("||https://spoiler.example/||"), ["https://spoiler.example/"]);
        assert_eq!(urls("**https://bold.example**"), ["https://bold.example"]);
    }

    #[test]
    fn schemes_other_than_http_and_non_urls_are_ignored() {
        assert!(urls("ftp://example.com file:///etc/passwd javascript:alert(1) mailto:a@b.c").is_empty());
        assert!(urls("nothttps://example.com").is_empty());
        assert!(urls("http://").is_empty());
        assert!(urls("https://user:pw@example.com/").is_empty());
        assert!(urls(&format!("https://example.com/{}", "a".repeat(3000))).is_empty());
    }

    #[test]
    fn markdown_links_use_the_destination() {
        assert_eq!(urls("[docs](https://example.com/docs)"), ["https://example.com/docs"]);
        assert_eq!(urls("[docs](https://example.com/docs \"Title\")"), ["https://example.com/docs"]);
        assert_eq!(urls("[x](https://en.wikipedia.org/wiki/Rust_(language))"), ["https://en.wikipedia.org/wiki/Rust_(language)"]);
        // The link text is not a second URL.
        assert_eq!(urls("[https://a.example](https://b.example)"), ["https://b.example"]);
        assert_eq!(urls("[x](<https://example.com/a b>)").len(), 1);
        assert!(urls("[x](/relative) [y](ftp://z)").is_empty());
        assert!(urls("[broken](https://example.com").len() <= 1);
    }

    #[test]
    fn angle_brackets_do_not_suppress_the_embed() {
        assert_eq!(urls("<https://example.com>"), ["https://example.com"]);
        assert_eq!(urls("see <https://example.com/a?b=1>."), ["https://example.com/a?b=1"]);
        assert_eq!(urls("[x](<https://example.com>)"), ["https://example.com"]);
        assert_eq!(urls("<https://a.example> https://b.example"), ["https://a.example", "https://b.example"]);
        // Wrapped and bare forms of one URL count once.
        assert_eq!(urls("<https://a.example> https://a.example"), ["https://a.example"]);
        // Not closed: still a URL.
        assert_eq!(urls("<https://a.example b"), ["https://a.example"]);
        assert_eq!(urls("<<https://a.example>>"), ["https://a.example"]);
    }

    #[test]
    fn code_is_ignored() {
        assert!(urls("`https://example.com`").is_empty());
        assert!(urls("``https://example.com ` x``").is_empty());
        assert!(urls("```\nhttps://example.com\n```").is_empty());
        assert!(urls("```rust\nlet u = \"https://example.com\";\n```").is_empty());
        assert!(urls("```\nhttps://example.com").is_empty(), "unclosed fence runs to the end");
        assert_eq!(urls("`a` https://example.com `b`"), ["https://example.com"]);
        assert_eq!(urls("```\nx\n``` https://after.example"), ["https://after.example"]);
        // A lone backtick is just text.
        assert_eq!(urls("it`s https://example.com"), ["https://example.com"]);
        assert_eq!(urls("`https://example.com"), ["https://example.com"]);
        // URLs touching code spans: inside is ignored, outside is not.
        assert_eq!(urls("`a`https://example.com"), ["https://example.com"]);
        assert_eq!(urls("https://example.com`b`"), ["https://example.com"]);
        assert_eq!(urls("`https://in.example`https://out.example"), ["https://out.example"]);
        assert_eq!(urls("https://out.example`https://in.example`"), ["https://out.example"]);
        assert!(urls("`<https://example.com>`").is_empty());
        assert!(urls("`[x](https://example.com)`").is_empty());
        assert!(urls("``` https://example.com ```").is_empty());
        assert!(urls("```https://example.com```").is_empty());
        // Longer runs close only with an equal run.
        assert!(urls("```` ``` https://example.com ```` ").is_empty());
        assert_eq!(urls("```` x ```` https://after.example"), ["https://after.example"]);
        // Code ends where it ends: a second paragraph's URL is found after a closed fence.
        assert_eq!(urls("```\ncode\n```\n\nhttps://example.com"), ["https://example.com"]);
    }

    #[test]
    fn deduplicated_and_capped() {
        assert_eq!(urls("https://a.example https://a.example [x](https://a.example)"), ["https://a.example"]);
        assert_eq!(
            urls("https://a.example https://b.example https://c.example https://d.example https://e.example"),
            ["https://a.example", "https://b.example", "https://c.example"]
        );
        // Duplicates do not use up the cap; wrapped URLs count like bare ones.
        assert_eq!(
            urls("<https://x.example> https://x.example https://a.example https://a.example https://b.example https://c.example"),
            ["https://x.example", "https://a.example", "https://b.example"]
        );
    }

    #[test]
    fn unicode_does_not_confuse_positions() {
        assert_eq!(urls("héllo 🎉 https://example.com/é ✓"), ["https://example.com/é"]);
        assert!(urls("`é` `https://example.com`").is_empty());
    }
}
