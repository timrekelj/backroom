//! Site-specific providers: X/Twitter, TikTok, Instagram and Reddit.
//!
//! These sites do not serve usable previews to bots, so each provider reads a
//! public embed-friendly *mirror* (or the mirror's JSON API) and never the site
//! itself, apart from following short links. The message keeps the original
//! link; the mirror is only used to fetch. Every request goes through the
//! SSRF-safe fetcher (mirrors are ordinary public hosts) and mirrors are tried
//! in order until one yields something ([`Mirrors`]).
//!
//! A provider only *describes* the post ([`Unfurled`]): text, author and the
//! URLs of its media. Downloading into S3 is the pipeline's job
//! (`super::media`).

mod instagram;
mod reddit;
mod tiktok;
mod x;

use std::collections::HashMap;

use serde_json::Value;
use url::Url;

use super::fetch::FetchError;
use super::html::{self, Metadata};
use super::provider::{Ctx, Provider, Unfurled};

/// Longest post text kept (tweets, captions, self posts).
pub const TEXT_MAX: usize = 1000;

/// Mirror and API base URLs per role, tried in order. The defaults are the
/// ones that worked when this was written (see the provider modules for which
/// mirror serves what); `THEHIDEOUT_EMBED_MIRRORS` (a JSON object of role →
/// list of base URLs) replaces the lists it names.
///
/// Roles:
/// - `x_api`: FxTwitter-style JSON, `{base}/status/{id}`
/// - `x_vx_api`: VxTwitter-style JSON, `{base}/{user}/status/{id}`
/// - `x_html`: OpenGraph pages, `{base}/{user}/status/{id}`
/// - `tiktok`: OpenGraph pages, `{base}/@{user}/video/{id}`
/// - `instagram`: OpenGraph pages, `{base}/{p|reel}/{code}/`
/// - `reddit`: OpenGraph pages, `{base}/r/{sub}/comments/{id}/`
/// - `reddit_json`: Reddit's public JSON, `{base}/comments/{id}.json`
#[derive(Debug, Clone)]
pub struct Mirrors {
    lists: HashMap<String, Vec<String>>,
}

pub const MIRROR_ROLES: [&str; 7] = ["x_api", "x_vx_api", "x_html", "tiktok", "instagram", "reddit", "reddit_json"];

impl Default for Mirrors {
    fn default() -> Self {
        let table: [(&str, &[&str]); 7] = [
            ("x_api", &["https://api.fxtwitter.com"]),
            ("x_vx_api", &["https://api.vxtwitter.com"]),
            ("x_html", &["https://fxtwitter.com", "https://fixupx.com"]),
            ("tiktok", &["https://tnktok.com", "https://tiktxk.com"]),
            ("instagram", &["https://uuinstagram.com", "https://eeinstagram.com", "https://vxinstagram.com", "https://kkinstagram.com"]),
            ("reddit", &["https://vxreddit.com", "https://rxddit.com"]),
            ("reddit_json", &["https://www.reddit.com", "https://old.reddit.com"]),
        ];
        Self { lists: table.into_iter().map(|(k, v)| (k.to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect() }
    }
}

impl Mirrors {
    /// Applies a `THEHIDEOUT_EMBED_MIRRORS` override on top of the defaults.
    /// Unknown roles and non-http(s) base URLs are errors (so a typo does not
    /// silently disable a provider).
    pub fn with_override(mut self, json: &str) -> Result<Self, String> {
        let parsed: HashMap<String, Vec<String>> = serde_json::from_str(json).map_err(|e| format!("not a JSON object of lists: {e}"))?;
        for (role, bases) in parsed {
            if !MIRROR_ROLES.contains(&role.as_str()) {
                return Err(format!("unknown role {role:?}, expected one of {MIRROR_ROLES:?}"));
            }
            let mut cleaned = Vec::new();
            for base in bases {
                let trimmed = base.trim().trim_end_matches('/').to_owned();
                let url = Url::parse(&trimmed).map_err(|e| format!("{role}: {base:?} is not a URL: {e}"))?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    return Err(format!("{role}: {base:?} must be an http(s) URL"));
                }
                cleaned.push(trimmed);
            }
            self.lists.insert(role, cleaned);
        }
        Ok(self)
    }

    /// Replaces one role's list (tests point mirrors at their local site).
    pub fn with(mut self, role: &str, bases: &[&str]) -> Self {
        assert!(MIRROR_ROLES.contains(&role), "unknown role {role}");
        self.lists.insert(role.to_owned(), bases.iter().map(|b| b.trim_end_matches('/').to_owned()).collect());
        self
    }

    /// An empty set: no provider has a mirror.
    pub fn none() -> Self {
        Self { lists: MIRROR_ROLES.iter().map(|r| ((*r).to_owned(), Vec::new())).collect() }
    }

    pub fn get(&self, role: &str) -> &[String] {
        self.lists.get(role).map_or(&[], Vec::as_slice)
    }
}

/// What a URL points at, as far as detection can tell without any request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    X { id: String, user: Option<String> },
    /// `id` is `None` for a short link (`vm.`, `vt.`, `/t/…`) that has to be resolved first.
    TikTok { user: Option<String>, id: Option<String>, original: Url },
    /// `kind` is `p` or `reel`.
    Instagram { kind: &'static str, code: String },
    /// `path` is `/r/{sub}/comments/{id}/` or `/comments/{id}/`; `id` is `None` for a share link to resolve.
    Reddit { path: String, id: Option<String>, original: Url },
}

impl Target {
    pub fn provider(&self) -> Provider {
        match self {
            Self::X { .. } => Provider::X,
            Self::TikTok { .. } => Provider::TikTok,
            Self::Instagram { .. } => Provider::Instagram,
            Self::Reddit { .. } => Provider::Reddit,
        }
    }
}

fn is_digits(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_slug(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Recognises the URL shapes of the supported sites; `None` means "generic".
pub fn detect(url: &Url) -> Option<Target> {
    detect_on(url, url.host_str()?)
}

/// Like [`detect`], but reads the path as if the URL were on `host`. Used
/// after a short link redirect: the short link's host was already recognised
/// and the redirect chain vetted, so the landing page only has to have the
/// right shape.
fn detect_on(url: &Url, host: &str) -> Option<Target> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let strip = |prefixes: &[&str]| -> String {
        let mut h = host;
        for p in prefixes {
            if let Some(rest) = h.strip_prefix(p) {
                h = rest;
                break;
            }
        }
        h.to_owned()
    };
    match strip(&["www.", "mobile.", "m."]).as_str() {
        "x.com" | "twitter.com" => {
            let (user, id) = match segments.as_slice() {
                [user, "status" | "statuses", id, ..] if *user != "i" => (Some((*user).to_owned()), *id),
                ["i", "web", "status", id, ..] | ["i", "status", id, ..] => (None, *id),
                _ => return None,
            };
            if !is_digits(id, 20) {
                return None;
            }
            let user = user.filter(|u| is_slug(u, 1, 20));
            Some(Target::X { id: id.to_owned(), user })
        }
        "tiktok.com" => match segments.as_slice() {
            [user, "video" | "photo", id, ..] if user.starts_with('@') && is_digits(id, 25) => {
                Some(Target::TikTok { user: Some(user[1..].to_owned()), id: Some((*id).to_owned()), original: url.clone() })
            }
            ["t", code, ..] if is_slug(code, 3, 32) => Some(Target::TikTok { user: None, id: None, original: url.clone() }),
            _ => None,
        },
        "vm.tiktok.com" | "vt.tiktok.com" => match segments.as_slice() {
            [code, ..] if is_slug(code, 3, 32) => Some(Target::TikTok { user: None, id: None, original: url.clone() }),
            _ => None,
        },
        "instagram.com" => {
            let rest = match segments.as_slice() {
                [first, rest @ ..] if !matches!(*first, "p" | "reel" | "reels" | "tv") && !rest.is_empty() => rest,
                all => all,
            };
            match rest {
                [kind @ ("p" | "reel" | "reels" | "tv"), code, ..] if is_slug(code, 5, 20) => {
                    let kind = if matches!(*kind, "reel" | "reels") { "reel" } else { "p" };
                    Some(Target::Instagram { kind, code: (*code).to_owned() })
                }
                _ => None,
            }
        }
        "reddit.com" | "old.reddit.com" | "new.reddit.com" | "np.reddit.com" | "mobile.reddit.com" => {
            match segments.as_slice() {
                ["r", sub, "comments", id, ..] if is_slug(sub, 2, 21) && is_slug(id, 3, 10) => {
                    Some(Target::Reddit { path: format!("/r/{sub}/comments/{id}/"), id: Some((*id).to_owned()), original: url.clone() })
                }
                ["comments", id, ..] if is_slug(id, 3, 10) => {
                    Some(Target::Reddit { path: format!("/comments/{id}/"), id: Some((*id).to_owned()), original: url.clone() })
                }
                ["r", sub, "s", code, ..] if is_slug(sub, 2, 21) && is_slug(code, 3, 20) => {
                    Some(Target::Reddit { path: String::new(), id: None, original: url.clone() })
                }
                _ => None,
            }
        }
        "redd.it" => match segments.as_slice() {
            [id] if is_slug(id, 3, 10) => {
                Some(Target::Reddit { path: format!("/comments/{id}/"), id: Some((*id).to_owned()), original: url.clone() })
            }
            _ => None,
        },
        _ => None,
    }
}

/// What a short link landed on, judged by path only (see [`detect_on`]).
fn detect_landing(landed: &Url, expected: Provider) -> Option<Target> {
    let host = match expected {
        Provider::TikTok => "tiktok.com",
        Provider::Reddit => "reddit.com",
        _ => return None,
    };
    detect_on(landed, host).filter(|t| t.provider() == expected)
}

/// Runs the provider for `target`. Each provider tries its mirrors in order
/// and returns the first usable description; `Ok(None)` when none had any.
pub async fn unfurl(ctx: &Ctx<'_>, target: Target) -> Result<Option<Unfurled>, FetchError> {
    match target {
        Target::X { id, user } => x::unfurl(ctx, &id, user.as_deref()).await,
        Target::TikTok { user, id, original } => tiktok::unfurl(ctx, user, id, original).await,
        Target::Instagram { kind, code } => instagram::unfurl(ctx, kind, &code).await,
        Target::Reddit { path, id, original } => reddit::unfurl(ctx, path, id, original).await,
    }
}

// ------------------------------------------------------------------ helpers

/// `{base}{path}` for a mirror base (no trailing slash) and an absolute path.
fn join(base: &str, path: &str) -> Option<Url> {
    Url::parse(&format!("{base}{path}")).ok()
}

/// Resolves a possibly relative reference against the page it was found on;
/// only http(s) results count.
fn absolute(base: &Url, raw: &str) -> Option<Url> {
    base.join(raw.trim()).ok().filter(|u| matches!(u.scheme(), "http" | "https"))
}

/// A JSON document from `url` (at most [`JSON_CAP`] bytes).
const JSON_CAP: usize = 2 * 1024 * 1024;

async fn fetch_json(ctx: &Ctx<'_>, url: &Url) -> Result<Value, FetchError> {
    let fetcher = ctx.mirror_fetcher.with_total_timeout(ctx.attempt_timeout);
    fetcher
        .within(async {
            let reply = fetcher.get_accepting(url, "application/json,*/*;q=0.1").await?;
            let body = reply.read_capped(JSON_CAP).await?;
            serde_json::from_slice(&body).map_err(|e| FetchError::Http(format!("invalid JSON: {e}")))
        })
        .await
}

/// What a mirror answered with.
enum Page {
    /// An HTML page and the URL it ended up at.
    Html(Metadata, Url),
    /// The mirror redirected straight to a video file (some Instagram mirrors do).
    Video(Url),
}

async fn fetch_page(ctx: &Ctx<'_>, url: &Url) -> Result<Page, FetchError> {
    let fetcher = ctx.mirror_fetcher.with_total_timeout(ctx.attempt_timeout);
    fetcher
        .within(async {
            let reply = fetcher.get(url).await?;
            let final_url = reply.url.clone();
            match reply.content_type.as_str() {
                "text/html" | "application/xhtml+xml" => {
                    let body = reply.read_html_prefix(ctx.html_cap).await?;
                    Ok(Page::Html(html::parse(&body), final_url))
                }
                t if t.starts_with("video/") => Ok(Page::Video(final_url)),
                other => Err(FetchError::NotAllowedType(other.to_owned())),
            }
        })
        .await
}

/// Post text: newlines kept (at most one blank line), control characters
/// dropped, limited to `max` characters with an ellipsis.
pub fn clean_text(raw: &str, max: usize) -> String {
    let decoded = html_escape::decode_html_entities(raw);
    let mut out = String::new();
    let mut newlines = 0;
    for line in decoded.replace("\r\n", "\n").split('\n') {
        let line: String = line.chars().filter(|c| !c.is_control()).collect::<String>();
        let line = line.trim();
        if line.is_empty() {
            newlines += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if newlines > 0 { "\n\n" } else { "\n" });
        }
        newlines = 0;
        out.push_str(line);
    }
    if out.chars().count() <= max {
        return out;
    }
    let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

fn json_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn json_u32(v: &Value, key: &str) -> Option<u32> {
    let n = v.get(key)?;
    n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

/// Seconds (possibly fractional) as milliseconds.
fn seconds_to_ms(s: f64) -> Option<u32> {
    (s.is_finite() && s > 0.0).then(|| (s * 1000.0).min(f64::from(u32::MAX)) as u32)
}

/// The URLs of one video ordered for download: the variants whose estimated
/// size (`bitrate × duration`) fits `cap` best-quality first, then the rest
/// smallest first as a last resort. Variants without a bitrate sort last
/// among the fitting ones.
fn order_variants(mut variants: Vec<(String, u64)>, duration_s: Option<f64>, cap: usize) -> Vec<String> {
    variants.sort_by_key(|a| std::cmp::Reverse(a.1));
    let fits = |bitrate: u64| match duration_s {
        Some(d) if bitrate > 0 => (bitrate as f64 / 8.0 * d) <= cap as f64,
        _ => true,
    };
    let (fitting, too_big): (Vec<_>, Vec<_>) = variants.into_iter().partition(|(_, b)| fits(*b));
    let mut ordered: Vec<String> = fitting.into_iter().map(|(u, _)| u).collect();
    ordered.extend(too_big.into_iter().rev().map(|(u, _)| u));
    ordered.dedup();
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> Option<Target> {
        detect(&Url::parse(url).unwrap())
    }

    fn provider(url: &str) -> Provider {
        Provider::for_url(&Url::parse(url).unwrap())
    }

    #[test]
    fn x_and_twitter_urls() {
        for url in [
            "https://x.com/NASA/status/2041557036274475228",
            "https://twitter.com/NASA/status/2041557036274475228?s=20",
            "https://www.twitter.com/NASA/status/2041557036274475228/photo/2",
            "https://mobile.twitter.com/NASA/status/2041557036274475228",
            "http://x.com/NASA/statuses/2041557036274475228",
        ] {
            assert_eq!(target(url), Some(Target::X { id: "2041557036274475228".into(), user: Some("NASA".into()) }), "{url}");
        }
        assert_eq!(target("https://x.com/i/web/status/2041557036274475228"), Some(Target::X { id: "2041557036274475228".into(), user: None }));
        assert_eq!(target("https://x.com/i/status/1"), Some(Target::X { id: "1".into(), user: None }));
        for url in ["https://x.com/NASA", "https://x.com/NASA/status/abc", "https://x.com/NASA/likes", "https://x.com.evil.com/a/status/1", "https://notx.com/a/status/1"] {
            assert_eq!(provider(url), Provider::Generic, "{url}");
        }
    }

    #[test]
    fn tiktok_urls() {
        assert_eq!(
            target("https://www.tiktok.com/@scout2015/video/6718335390845095173?lang=en"),
            Some(Target::TikTok {
                user: Some("scout2015".into()),
                id: Some("6718335390845095173".into()),
                original: Url::parse("https://www.tiktok.com/@scout2015/video/6718335390845095173?lang=en").unwrap()
            })
        );
        assert!(matches!(target("https://www.tiktok.com/@u/photo/123"), Some(Target::TikTok { id: Some(_), .. })));
        assert!(matches!(target("https://m.tiktok.com/@u/video/123"), Some(Target::TikTok { id: Some(_), .. })));
        for short in ["https://vm.tiktok.com/ZMabc123/", "https://vt.tiktok.com/ZSabc123/", "https://www.tiktok.com/t/ZTabc123/"] {
            assert!(matches!(target(short), Some(Target::TikTok { id: None, user: None, .. })), "{short}");
        }
        for url in ["https://www.tiktok.com/@scout2015", "https://www.tiktok.com/tag/cats", "https://tiktok.com.evil.com/@a/video/1"] {
            assert_eq!(provider(url), Provider::Generic, "{url}");
        }
    }

    #[test]
    fn instagram_urls() {
        let post = Some(Target::Instagram { kind: "p", code: "C-6mYVRS5hD".into() });
        assert_eq!(target("https://www.instagram.com/p/C-6mYVRS5hD/"), post);
        assert_eq!(target("https://instagram.com/p/C-6mYVRS5hD"), post);
        assert_eq!(target("https://www.instagram.com/nasa/p/C-6mYVRS5hD/?img_index=2"), post);
        assert_eq!(target("https://www.instagram.com/tv/C-6mYVRS5hD/"), post);
        let reel = Some(Target::Instagram { kind: "reel", code: "DKS4Zklsbzb".into() });
        assert_eq!(target("https://www.instagram.com/reel/DKS4Zklsbzb/?hl=en"), reel);
        assert_eq!(target("https://www.instagram.com/reels/DKS4Zklsbzb/"), reel);
        assert_eq!(target("https://www.instagram.com/nasa/reel/DKS4Zklsbzb/"), reel);
        for url in ["https://www.instagram.com/nasa/", "https://www.instagram.com/p/", "https://www.instagram.com/explore/tags/space/"] {
            assert_eq!(provider(url), Provider::Generic, "{url}");
        }
    }

    #[test]
    fn reddit_urls() {
        let full = |path: &str, id: &str| Some((path.to_owned(), Some(id.to_owned())));
        let reddit = |url: &str| match target(url) {
            Some(Target::Reddit { path, id, .. }) => Some((path, id)),
            other => panic!("{url}: {other:?}"),
        };
        for url in [
            "https://www.reddit.com/r/Unexpected/comments/1xhqbt0/either_way_counts/",
            "https://old.reddit.com/r/Unexpected/comments/1xhqbt0/either_way_counts/?context=3",
            "https://np.reddit.com/r/Unexpected/comments/1xhqbt0/",
            "https://reddit.com/r/Unexpected/comments/1xhqbt0",
            "https://new.reddit.com/r/Unexpected/comments/1xhqbt0/x/",
        ] {
            assert_eq!(reddit(url), full("/r/Unexpected/comments/1xhqbt0/", "1xhqbt0"), "{url}");
        }
        assert_eq!(reddit("https://www.reddit.com/comments/1xhqbt0/"), full("/comments/1xhqbt0/", "1xhqbt0"));
        assert_eq!(reddit("https://redd.it/1xhqbt0"), full("/comments/1xhqbt0/", "1xhqbt0"));
        assert_eq!(reddit("https://www.reddit.com/r/Unexpected/s/AbCdEf123"), Some((String::new(), None)));
        for url in ["https://www.reddit.com/r/Unexpected/", "https://www.reddit.com/user/x/", "https://v.redd.it/8y6vj20728yh1", "https://example.com/r/a/comments/abc"] {
            assert_eq!(provider(url), Provider::Generic, "{url}");
        }
    }

    #[test]
    fn provider_names() {
        assert_eq!(provider("https://x.com/a/status/1").name(), "x");
        assert_eq!(provider("https://vm.tiktok.com/ZMabc/").name(), "tiktok");
        assert_eq!(provider("https://instagram.com/p/abcde/").name(), "instagram");
        assert_eq!(provider("https://redd.it/abc").name(), "reddit");
        assert_eq!(provider("https://example.com/").name(), "");
    }

    #[test]
    fn mirror_overrides_are_validated() {
        let m = Mirrors::default().with_override(r#"{"tiktok":["https://a.example/","http://b.example"]}"#).unwrap();
        assert_eq!(m.get("tiktok"), ["https://a.example", "http://b.example"]);
        assert!(!m.get("instagram").is_empty(), "other roles keep their defaults");
        assert!(Mirrors::default().with_override(r#"{"tiktok":[]}"#).unwrap().get("tiktok").is_empty());
        for bad in ["[]", "not json", r#"{"nope":["https://a.example"]}"#, r#"{"tiktok":["ftp://a.example"]}"#, r#"{"tiktok":["a.example"]}"#, r#"{"tiktok":"https://a"}"#] {
            assert!(Mirrors::default().with_override(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn post_text_keeps_paragraphs_and_is_limited() {
        assert_eq!(clean_text("a &amp; b\r\n\r\n\r\n\r\nc\n d\u{7} ", 100), "a & b\n\nc\nd");
        assert_eq!(clean_text("abcdefghij", 5), "abcd…");
        assert_eq!(clean_text("  \n ", 5), "");
    }

    #[test]
    fn variants_prefer_the_best_that_fits() {
        let v = |bitrates: &[u64]| bitrates.iter().map(|b| (format!("u{b}"), *b)).collect::<Vec<_>>();
        // 10 s: 8 Mbit/s ≈ 10 MB fits a 12 MB cap, 16 Mbit/s ≈ 20 MB does not.
        assert_eq!(order_variants(v(&[1_000_000, 16_000_000, 8_000_000]), Some(10.0), 12_000_000), ["u8000000", "u1000000", "u16000000"]);
        assert_eq!(order_variants(v(&[1_000_000, 2_000_000]), None, 1), ["u2000000", "u1000000"]);
    }
}
