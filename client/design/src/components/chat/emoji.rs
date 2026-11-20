//! Emoji data and the pure logic around it: the bundled Unicode set (the `emojis` crate, no
//! network), the guild's custom emojis as plain data, search by shortcode or name, the `:sm`
//! autocomplete ranking and the "only emoji" test behind jumbo messages.
//!
//! Unicode emoji draw as text, so the system's colour emoji font does the pictures (Apple Color
//! Emoji on macOS, Segoe UI Emoji on Windows, Noto Color Emoji where Linux has it). Only the
//! default skin tone is offered; a tone someone else typed still renders, because that is just
//! text. Custom emojis arrive as [`EmojiImage`]s that the app resolves by id.

use std::rc::Rc;

use emojis::{Emoji, Group};
use rune::Pixels;

/// Emojis in a message at most this many still count as "only emoji" and draw jumbo.
pub const JUMBO_MAX: usize = 27;
/// Most rows the `:name` autocomplete shows.
pub const SUGGEST_MAX: usize = 8;
/// The recent emojis kept (and the picker shows).
pub const RECENT_MAX: usize = 24;

/// A custom emoji of the open guild, as the picker and the autocomplete need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEmoji {
    pub id: u64,
    pub name: String,
    pub animated: bool,
}

impl ServerEmoji {
    /// The message token: `<:name:id>`, or `<a:name:id>` when animated.
    pub fn token(&self) -> String {
        format!(
            "<{}:{}:{}>",
            if self.animated { "a" } else { "" },
            self.name,
            self.id
        )
    }
}

/// What a picker or an autocomplete row chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Unicode(&'static str),
    Custom(ServerEmoji),
}

impl Pick {
    /// What goes into the composer: the character itself, or the custom emoji's token.
    pub fn text(&self) -> String {
        match self {
            Pick::Unicode(s) => (*s).to_owned(),
            Pick::Custom(e) => e.token(),
        }
    }

    /// The reaction key (`u:<emoji>` / `c:<id>`), also how recents are stored.
    pub fn key(&self) -> String {
        match self {
            Pick::Unicode(s) => format!("u:{s}"),
            Pick::Custom(e) => format!("c:{}", e.id),
        }
    }

    /// The pick a [`key`](Self::key) names; `None` when it is not a known Unicode emoji or the
    /// custom emoji isn't among `server`.
    pub fn from_key(key: &str, server: &[ServerEmoji]) -> Option<Pick> {
        if let Some(u) = key.strip_prefix("u:") {
            return emojis::get(u).map(|e| Pick::Unicode(e.as_str()));
        }
        let id: u64 = key.strip_prefix("c:")?.parse().ok()?;
        server
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .map(Pick::Custom)
    }

    /// A short name for tooltips and the picker's preview line: `:smile:`.
    pub fn label(&self) -> String {
        match self {
            Pick::Unicode(s) => emojis::get(s).map_or_else(|| (*s).to_owned(), unicode_label),
            Pick::Custom(e) => format!(":{}:", e.name),
        }
    }
}

/// `:shortcode:` for an emoji that has one, else its name.
pub fn unicode_label(e: &Emoji) -> String {
    match e.shortcode() {
        Some(s) => format!(":{s}:"),
        None => e.name().to_owned(),
    }
}

/// `:shortcode:` (or the name) of the Unicode emoji `glyph`, or the glyph itself when unknown.
pub fn glyph_label(glyph: &str) -> String {
    emojis::get(glyph).map_or_else(|| glyph.to_owned(), unicode_label)
}

/// The picture of a custom emoji, resolved by the app from its image cache.
#[derive(Clone)]
pub struct EmojiImage {
    pub name: String,
    pub animated: bool,
    /// Draws the emoji (all its frames when animated); `None` until it is downloaded.
    pub live: Option<Pixels>,
    /// The first frame of an animated emoji, for reduced motion and the picker at rest.
    pub still: Option<Pixels>,
}

type Lookup = Rc<dyn Fn(&str) -> Option<EmojiImage>>;

/// How message text and reactions find the pictures of custom emojis: by id, `None` for an id
/// that isn't one of the channel's guild's (it then shows as `:name:` text).
#[derive(Clone)]
pub struct EmojiSet {
    lookup: Lookup,
    reduced_motion: bool,
}

impl EmojiSet {
    pub fn new(lookup: impl Fn(&str) -> Option<EmojiImage> + 'static) -> EmojiSet {
        EmojiSet {
            lookup: Rc::new(lookup),
            reduced_motion: false,
        }
    }

    /// No custom emojis exist (a direct message): every token falls back to `:name:`.
    pub fn none() -> EmojiSet {
        EmojiSet::new(|_| None)
    }

    /// Reduced motion shows an animated emoji's first frame instead of playing it.
    pub fn reduced_motion(mut self, on: bool) -> Self {
        self.reduced_motion = on;
        self
    }

    /// Whether animated emojis are held on their first frame.
    pub fn is_reduced(&self) -> bool {
        self.reduced_motion
    }

    pub fn get(&self, id: &str) -> Option<EmojiImage> {
        (self.lookup)(id)
    }

    /// The pixels to draw `img` with at rest: the live image, or its first frame under reduced
    /// motion.
    pub fn pixels(&self, img: &EmojiImage) -> Option<Pixels> {
        if self.reduced_motion && img.animated && img.still.is_some() {
            return img.still.clone();
        }
        img.live.clone()
    }
}

// ---- Unicode ---------------------------------------------------------------------------------

/// The newest Unicode emoji version the picker offers. Newer emojis exist in the bundled data but
/// no shipping colour emoji font draws them yet (they would show as boxes), so they wait until
/// the fonts catch up; one someone types still renders as whatever their system makes of it.
pub const NEWEST: emojis::UnicodeVersion = emojis::UnicodeVersion::new(15, 1);

/// Every default-tone emoji the picker offers: the bundled set up to [`NEWEST`].
pub fn supported() -> impl Iterator<Item = &'static Emoji> + Clone {
    emojis::iter().filter(|e| e.unicode_version() <= NEWEST)
}

/// The picker's Unicode groups, in order, with their title.
pub fn groups() -> Vec<(Group, &'static str)> {
    Group::iter().map(|g| (g, group_title(g))).collect()
}

pub fn group_title(g: Group) -> &'static str {
    match g {
        Group::SmileysAndEmotion => "Smileys & emotion",
        Group::PeopleAndBody => "People & body",
        Group::AnimalsAndNature => "Animals & nature",
        Group::FoodAndDrink => "Food & drink",
        Group::TravelAndPlaces => "Travel & places",
        Group::Activities => "Activities",
        Group::Objects => "Objects",
        Group::Symbols => "Symbols",
        Group::Flags => "Flags",
    }
}

/// Every default-tone emoji of `g`, in Unicode order.
pub fn group_emojis(g: Group) -> impl Iterator<Item = &'static Emoji> {
    supported().filter(move |e| e.group() == g)
}

/// How well `e` matches the lowercase `q`: 0 an exact shortcode, 1 a shortcode prefix, 2 a
/// shortcode or name that contains it, `None` no match.
fn rank(e: &Emoji, q: &str) -> Option<u8> {
    let mut best: Option<u8> = None;
    for s in e.shortcodes() {
        let r = if s == q {
            0
        } else if s.starts_with(q) {
            1
        } else if s.contains(q) {
            2
        } else {
            continue;
        };
        best = Some(best.map_or(r, |b| b.min(r)));
    }
    if best.is_none() && e.name().to_lowercase().contains(q) {
        best = Some(2);
    }
    best
}

/// The query without the colons a user types around a shortcode, lowercased.
pub fn normalize_query(q: &str) -> String {
    q.trim().trim_matches(':').to_lowercase()
}

/// The Unicode emojis matching `query` by shortcode or name, best first (an exact shortcode,
/// then a prefix, then anything containing it; Unicode order within each).
pub fn search_unicode(query: &str) -> Vec<&'static Emoji> {
    let q = normalize_query(query);
    if q.is_empty() {
        return supported().collect();
    }
    let mut hits: Vec<(u8, &'static Emoji)> = supported()
        .filter_map(|e| rank(e, &q).map(|r| (r, e)))
        .collect();
    hits.sort_by_key(|(r, _)| *r);
    hits.into_iter().map(|(_, e)| e).collect()
}

/// The custom emojis whose name matches `query`, best first.
pub fn search_server<'a>(query: &str, server: &'a [ServerEmoji]) -> Vec<&'a ServerEmoji> {
    let q = normalize_query(query);
    if q.is_empty() {
        return server.iter().collect();
    }
    let mut hits: Vec<(u8, &ServerEmoji)> = server
        .iter()
        .filter_map(|e| {
            let n = e.name.to_lowercase();
            let r = if n == q {
                0
            } else if n.starts_with(&q) {
                1
            } else if n.contains(&q) {
                2
            } else {
                return None;
            };
            Some((r, e))
        })
        .collect();
    hits.sort_by_key(|(r, _)| *r);
    hits.into_iter().map(|(_, e)| e).collect()
}

/// One row of the `:name` autocomplete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub pick: Pick,
    /// The shortcode or name without colons.
    pub name: String,
}

/// The best [`SUGGEST_MAX`] emojis for the text typed after a `:` (at least two characters):
/// custom ones beat Unicode ones on equal rank, and only shortcodes and custom names count, so
/// `:sm` offers smile and smirk but not every emoji with "sm" somewhere in its name.
pub fn suggest(query: &str, server: &[ServerEmoji]) -> Vec<Suggestion> {
    let q = normalize_query(query);
    if q.chars().count() < 2 {
        return Vec::new();
    }
    let mut hits: Vec<(u8, Suggestion)> = Vec::new();
    for e in search_server(&q, server) {
        let n = e.name.to_lowercase();
        let r = if n == q {
            0
        } else if n.starts_with(&q) {
            1
        } else {
            2
        };
        hits.push((
            r,
            Suggestion {
                pick: Pick::Custom(e.clone()),
                name: e.name.clone(),
            },
        ));
    }
    for e in supported() {
        let best = e
            .shortcodes()
            .filter_map(|s| {
                if s == q {
                    Some((0, s))
                } else if s.starts_with(&q) {
                    Some((1, s))
                } else if s.contains(&q) {
                    Some((2, s))
                } else {
                    None
                }
            })
            .min_by_key(|(r, s)| (*r, s.len()));
        if let Some((r, s)) = best {
            hits.push((
                r,
                Suggestion {
                    pick: Pick::Unicode(e.as_str()),
                    name: s.to_owned(),
                },
            ));
        }
    }
    // Stable: equal ranks keep customs first, then Unicode order.
    hits.sort_by_key(|(r, _)| *r);
    hits.into_iter().map(|(_, s)| s).take(SUGGEST_MAX).collect()
}

/// The `:partial` being typed at `caret` (a byte offset): the byte range of the colon and what
/// follows it, and the text after the colon. A trigger needs the colon at the start of the
/// text or after whitespace, then at least two shortcode characters.
pub fn trigger_at(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, &str)> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let before = &text[..caret];
    let start =
        before.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-'))?;
    let colon = before[start..].starts_with(':');
    if !colon {
        return None;
    }
    let at_boundary = before[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    let word = &before[start + 1..];
    (at_boundary && word.chars().count() >= 2).then_some((start..caret, word))
}

/// Splits `s` into emoji clusters (a flag, a keycap, a ZWJ family each count as one), or `None`
/// when it holds anything else but whitespace or has more than `limit` of them. Whitespace is
/// skipped.
pub fn clusters(s: &str, limit: usize) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next()?;
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let end = longest_emoji(rest)?;
        out.push(&rest[..end]);
        if out.len() > limit {
            return None;
        }
        i += end;
    }
    Some(out)
}

/// The byte length of the longest emoji `rest` starts with.
fn longest_emoji(rest: &str) -> Option<usize> {
    // Longest known sequence (a family with tones, a flag with a subdivision tag) is far shorter.
    let ends: Vec<usize> = rest
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take(14)
        .collect();
    for &end in ends.iter().rev() {
        let head = &rest[..end];
        if emojis::get(head).is_some() {
            return Some(end);
        }
    }
    // A symbol typed without its emoji presentation selector (a heart, a sun): accept above the
    // Latin range only, so digits and `#` stay text.
    let first = rest.chars().next()?;
    if first as u32 >= 0x2000 {
        let with = format!("{first}\u{fe0f}");
        if emojis::get(&with).is_some() {
            return Some(first.len_utf8());
        }
    }
    None
}

/// Whether `s` is exactly one Unicode emoji (a reaction or a channel icon).
pub fn is_single(s: &str) -> bool {
    matches!(clusters(s, 1), Some(c) if c.len() == 1 && c[0].len() == s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Vec<ServerEmoji> {
        vec![
            ServerEmoji {
                id: 7,
                name: "smug_cat".into(),
                animated: false,
            },
            ServerEmoji {
                id: 9,
                name: "party_parrot".into(),
                animated: true,
            },
        ]
    }

    #[test]
    fn tokens_and_keys_round_trip() {
        let s = server();
        assert_eq!(s[0].token(), "<:smug_cat:7>");
        assert_eq!(s[1].token(), "<a:party_parrot:9>");
        let p = Pick::Custom(s[1].clone());
        assert_eq!(p.key(), "c:9");
        assert_eq!(Pick::from_key("c:9", &s), Some(p));
        assert_eq!(Pick::from_key("c:10", &s), None);
        let heart = Pick::Unicode(emojis::get("\u{2764}\u{fe0f}").unwrap().as_str());
        assert_eq!(Pick::from_key(&heart.key(), &s), Some(heart));
        assert_eq!(Pick::from_key("u:not an emoji", &s), None);
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_contains() {
        let hits = search_unicode(":smile:");
        assert_eq!(hits[0].shortcode(), Some("smile"));
        let hits = search_unicode("smi");
        assert!(hits.len() > 5);
        assert!(hits[0].shortcodes().any(|s| s.starts_with("smi")));
        assert!(
            search_unicode("grinning")
                .iter()
                .any(|e| e.shortcode() == Some("grinning")),
            "names match too"
        );
        assert!(search_unicode("zzzzqqq").is_empty());
        assert!(
            search_unicode("").len() > 1500,
            "an empty query is the whole set"
        );
    }

    #[test]
    fn search_finds_server_emojis_by_name() {
        let s = server();
        assert_eq!(search_server("parrot", &s).len(), 1);
        assert_eq!(search_server("SMUG", &s)[0].id, 7);
        assert!(search_server("nope", &s).is_empty());
    }

    #[test]
    fn suggestions_need_two_characters_and_are_capped() {
        let s = server();
        assert!(suggest("s", &s).is_empty());
        let top = suggest("sm", &s);
        assert!(!top.is_empty() && top.len() <= SUGGEST_MAX);
        assert!(
            matches!(&top[0].pick, Pick::Custom(e) if e.name == "smug_cat"),
            "a custom emoji leads on equal rank"
        );
        assert!(top.iter().any(|x| x.name == "smile"));
        let exact = suggest("par", &s);
        assert!(matches!(&exact[0].pick, Pick::Custom(e) if e.id == 9));
    }

    #[test]
    fn the_trigger_is_a_colon_after_whitespace_with_two_letters() {
        assert_eq!(trigger_at("hi :sm", 6), Some((3..6, "sm")));
        assert_eq!(trigger_at(":smi rest", 4), Some((0..4, "smi")));
        assert_eq!(trigger_at("hi :s", 5), None, "one letter is too early");
        assert_eq!(
            trigger_at("a:sm", 4),
            None,
            "needs a boundary before the colon"
        );
        assert_eq!(trigger_at("hi sm", 5), None);
        assert_eq!(trigger_at("12:30", 5), None);
        assert_eq!(
            trigger_at("hi :sm ok", 9),
            None,
            "the caret moved past the word"
        );
    }

    #[test]
    fn clusters_find_whole_sequences() {
        assert_eq!(clusters("\u{1f600}", 27).unwrap().len(), 1);
        assert_eq!(
            clusters("\u{1f600} \u{1f44d}\u{1f3fd}\u{2764}\u{fe0f}", 27)
                .unwrap()
                .len(),
            3,
            "a tone stays with its hand"
        );
        assert_eq!(
            clusters("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}", 27)
                .unwrap()
                .len(),
            1,
            "a ZWJ family is one"
        );
        assert_eq!(
            clusters("\u{1f1fa}\u{1f1f8}", 27).unwrap().len(),
            1,
            "a flag is one"
        );
        assert_eq!(
            clusters("1\u{fe0f}\u{20e3}", 27).unwrap().len(),
            1,
            "a keycap is one"
        );
        assert!(clusters("hi \u{1f600}", 27).is_none());
        assert!(clusters("12", 27).is_none(), "digits are text");
        assert!(clusters(&"\u{1f600}".repeat(28), 27).is_none());
        assert_eq!(clusters(&"\u{1f600}".repeat(27), 27).unwrap().len(), 27);
        assert!(is_single("\u{1f600}") && !is_single("\u{1f600}\u{1f600}") && !is_single("a"));
    }
}
