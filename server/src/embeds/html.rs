//! Page metadata from the first bytes of an HTML document: OpenGraph, Twitter
//! cards, `<title>` and `<meta name="description">`.
//!
//! This is a small tolerant scanner, not an HTML parser: it walks the tags of
//! the head (skipping comments, `<script>` and `<style>` bodies), stops at
//! `</head>` or `<body`, and treats the bytes as UTF-8 (invalid sequences
//! become U+FFFD; declared other charsets are not honoured).

use std::collections::HashMap;

pub const TITLE_MAX: usize = 256;
pub const DESCRIPTION_MAX: usize = 512;
pub const SITE_NAME_MAX: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: String,
    pub description: String,
    pub site_name: String,
    /// Raw (possibly relative) preview image URL.
    pub image: Option<String>,
    /// Raw (possibly relative) `og:video` / `twitter:player:stream` URL.
    pub video: Option<String>,
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
}

pub fn parse(bytes: &[u8]) -> Metadata {
    let text = String::from_utf8_lossy(bytes);
    let head = scan(&text);
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| head.meta.get(*k).filter(|v| !v.is_empty()))
            .cloned()
    };
    Metadata {
        title: clean(
            &pick(&["og:title", "twitter:title"])
                .or(head.title)
                .unwrap_or_default(),
            TITLE_MAX,
        ),
        description: clean(
            &pick(&["og:description", "twitter:description", "description"]).unwrap_or_default(),
            DESCRIPTION_MAX,
        ),
        site_name: clean(&pick(&["og:site_name"]).unwrap_or_default(), SITE_NAME_MAX),
        image: pick(&[
            "og:image",
            "og:image:url",
            "og:image:secure_url",
            "twitter:image",
            "twitter:image:src",
        ])
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty()),
        video: pick(&["og:video", "og:video:url", "og:video:secure_url", "twitter:player:stream"])
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty()),
        video_width: pick(&["og:video:width", "twitter:player:width"]).and_then(|v| v.trim().parse().ok()).filter(|v| *v > 0),
        video_height: pick(&["og:video:height", "twitter:player:height"]).and_then(|v| v.trim().parse().ok()).filter(|v| *v > 0),
    }
}

/// Decodes entities, collapses whitespace and limits to `max` characters
/// (an ellipsis replaces the cut text).
pub fn clean(raw: &str, max: usize) -> String {
    let decoded = html_escape::decode_html_entities(raw);
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed: String = collapsed.chars().filter(|c| !c.is_control()).collect();
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(max.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

#[derive(Default)]
struct Head {
    title: Option<String>,
    /// First value per lower-cased `property` / `name`.
    meta: HashMap<String, String>,
}

fn scan(text: &str) -> Head {
    let mut head = Head::default();
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        if let Some(after) = rest.strip_prefix("!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '!' || c == '?'))
            .unwrap_or(rest.len());
        let name = rest[..name_len].to_ascii_lowercase();
        let Some(end) = tag_end(&rest[name_len..]) else {
            break;
        };
        let attrs = &rest[name_len..name_len + end];
        let after = &rest[name_len + end + 1..];
        match name.as_str() {
            "/head" | "body" => break,
            "meta" => {
                let attrs = attributes(attrs);
                let key = attrs.get("property").or_else(|| attrs.get("name"));
                if let (Some(key), Some(content)) = (key, attrs.get("content")) {
                    head.meta
                        .entry(key.trim().to_ascii_lowercase())
                        .or_insert_with(|| content.clone());
                }
                rest = after;
            }
            "title" if head.title.is_none() => {
                let close = find_ci(after, "</title");
                head.title = Some(after[..close.unwrap_or(after.len())].to_owned());
                rest = close.map_or("", |c| &after[c..]);
            }
            "script" | "style" => {
                rest = find_ci(after, &format!("</{name}")).map_or("", |c| &after[c..]);
            }
            _ => rest = after,
        }
    }
    head
}

/// Offset of the `>` closing a tag whose attributes start at the beginning of
/// `s`, ignoring `>` inside quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '>') => return Some(i),
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => {}
        }
    }
    None
}

/// Attributes of a tag (`name="v"`, `name='v'`, `name=v`, `name`), names lower-cased, first wins.
fn attributes(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut chars = s.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() || c == '/' {
            chars.next();
            continue;
        }
        let mut end = s.len();
        while let Some(&(i, c)) = chars.peek() {
            if c.is_whitespace() || c == '=' || c == '/' {
                end = i;
                break;
            }
            chars.next();
        }
        let name = s[start..end].to_ascii_lowercase();
        while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek().is_some_and(|(_, c)| *c == '=') {
            chars.next();
            while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                chars.next();
            }
            match chars.peek().copied() {
                Some((_, q @ ('"' | '\''))) => {
                    chars.next();
                    for (_, c) in chars.by_ref() {
                        if c == q {
                            break;
                        }
                        value.push(c);
                    }
                }
                Some(_) => {
                    while let Some(&(_, c)) = chars.peek() {
                        if c.is_whitespace() {
                            break;
                        }
                        value.push(c);
                        chars.next();
                    }
                }
                None => {}
            }
        }
        if !name.is_empty() {
            out.entry(name).or_insert(value);
        }
    }
    out
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    (0..h.len().saturating_sub(n.len() - 1)).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(html: &str) -> Metadata {
        parse(html.as_bytes())
    }

    #[test]
    fn opengraph_wins_and_entities_decode() {
        let m = meta(
            r#"<!doctype html><html><head><title>Plain title</title>
            <meta property="og:title" content="Tom &amp; Jerry &#8211; &quot;fun&quot;">
            <meta property="og:description" content="Line one
               line   two">
            <meta property="og:site_name" content="Example">
            <meta property="og:image" content="/img/a.png">
            <meta name="description" content="ignored"></head><body><meta property="og:title" content="late"></body>"#,
        );
        assert_eq!(m.title, "Tom & Jerry – \"fun\"");
        assert_eq!(m.description, "Line one line two");
        assert_eq!(m.site_name, "Example");
        assert_eq!(m.image.as_deref(), Some("/img/a.png"));
    }

    #[test]
    fn twitter_cards_then_title_and_description_fallbacks() {
        let m = meta(
            r#"<meta name="twitter:title" content="TW"><meta name="twitter:description" content="TD"><meta name="twitter:image" content="https://x/i.jpg">"#,
        );
        assert_eq!(
            (m.title.as_str(), m.description.as_str(), m.image.as_deref()),
            ("TW", "TD", Some("https://x/i.jpg"))
        );
        let m = meta(
            r#"<TITLE>
              Just   a title </TITLE><META NAME="Description" CONTENT='Single &lt;quoted&gt;'>"#,
        );
        assert_eq!(m.title, "Just a title");
        assert_eq!(m.description, "Single <quoted>");
        assert_eq!(m.image, None);
        // OpenGraph beats Twitter beats plain.
        let m = meta(
            r#"<meta name="twitter:title" content="TW"><meta property="og:title" content="OG">"#,
        );
        assert_eq!(m.title, "OG");
    }

    #[test]
    fn tolerant_of_odd_markup() {
        // unquoted values, attribute order, `>` inside quotes, comments, scripts
        let m = meta(
            r#"<!-- <meta property="og:title" content="commented"> -->
            <script>var s = "<meta property='og:title' content='script'>";</script>
            <style>a > b { }</style>
            <meta content=Unquoted property=og:title>
            <meta property="og:description" content="a > b">
            <meta property="og:title" content="second">"#,
        );
        assert_eq!(m.title, "Unquoted");
        assert_eq!(m.description, "a > b");
        // truncated mid-tag and empty input do not panic
        assert_eq!(
            meta("<meta property=\"og:title\" content=\"cut"),
            Metadata::default()
        );
        assert_eq!(meta(""), Metadata::default());
        assert_eq!(meta("<title>no close").title, "no close");
        assert_eq!(parse(&[0xff, 0xfe, b'<', b'm']), Metadata::default());
    }

    #[test]
    fn lengths_are_limited() {
        let long = "x".repeat(1000);
        let m = meta(&format!(
            r#"<meta property="og:title" content="{long}"><meta property="og:description" content="{long}"><meta property="og:site_name" content="{long}">"#
        ));
        assert_eq!(m.title.chars().count(), TITLE_MAX);
        assert_eq!(m.description.chars().count(), DESCRIPTION_MAX);
        assert_eq!(m.site_name.chars().count(), SITE_NAME_MAX);
        assert!(m.title.ends_with('…'));
        // multi-byte characters are cut on character boundaries
        assert_eq!(clean(&"é".repeat(300), 10).chars().count(), 10);
        assert_eq!(clean("  a\u{0}b \n c ", 10), "ab c");
    }

    #[test]
    fn stops_at_head_end() {
        let m = meta(
            r#"<head><meta property="og:title" content="in"></head><meta property="og:description" content="out">"#,
        );
        assert_eq!(m.title, "in");
        assert_eq!(m.description, "");
    }
}
