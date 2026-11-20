//! Input validation shared by the guild, role, emoji and message services.

use connectrpc::ConnectError;
use unicode_segmentation::UnicodeSegmentation;

/// Valid colour palette keys (see `types.proto`); `""` means "no colour".
pub const PALETTE: [&str; 12] = [
    "slate", "red", "orange", "amber", "yellow", "lime", "green", "teal", "cyan", "blue", "indigo",
    "pink",
];
pub const ICON_MAX_LEN: usize = 48;
pub const TOPIC_MAX_CHARS: usize = 256;
pub const DISPLAY_NAME_LEN: std::ops::RangeInclusive<usize> = 1..=100;

pub fn validate_color(color: &str) -> Result<(), ConnectError> {
    if color.is_empty() || PALETTE.contains(&color) {
        Ok(())
    } else {
        Err(ConnectError::invalid_argument(format!(
            "color must be empty or one of: {}",
            PALETTE.join(", ")
        )))
    }
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, at most [`ICON_MAX_LEN`] characters; `""` = default.
pub fn validate_icon(icon: &str) -> Result<(), ConnectError> {
    let valid = icon.is_empty()
        || (icon.len() <= ICON_MAX_LEN
            && icon
                .split('-')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())));
    if valid {
        Ok(())
    } else {
        Err(ConnectError::invalid_argument(
            "icon must be a lowercase kebab-case name of at most 48 characters",
        ))
    }
}

/// Largest Unicode emoji (one grapheme cluster) in bytes.
pub const UNICODE_EMOJI_MAX_BYTES: usize = 32;

/// `^[a-z0-9_]{2,32}$` (custom emoji names).
pub fn validate_emoji_name(name: &str) -> Result<(), ConnectError> {
    if (2..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        Ok(())
    } else {
        Err(ConnectError::invalid_argument("emoji name must match ^[a-z0-9_]{2,32}$"))
    }
}

/// Exactly one extended grapheme cluster of at most [`UNICODE_EMOJI_MAX_BYTES`]
/// bytes that is an emoji (channel icons and reactions).
///
/// "Is an emoji" is a deliberately light heuristic, not the full UTS #51
/// sequence grammar: the cluster must contain a pictographic code point from
/// the main emoji blocks, a regional indicator (flags), a keycap
/// (`[0-9#*]` U+FE0F U+20E3), or a text-default symbol (©, ™, ↔, ▶ ...)
/// explicitly requested as emoji with U+FE0F. Clusters made only of ASCII,
/// letters, digits or punctuation are rejected. ZWJ sequences, skin tones and
/// tag flags are single clusters under UAX #29 and pass through that rule.
pub fn validate_unicode_emoji(value: &str) -> Result<(), ConnectError> {
    let invalid = || ConnectError::invalid_argument("expected a single Unicode emoji of at most 32 bytes");
    if value.is_empty() || value.len() > UNICODE_EMOJI_MAX_BYTES {
        return Err(invalid());
    }
    let mut graphemes = value.graphemes(true);
    let (Some(cluster), None) = (graphemes.next(), graphemes.next()) else {
        return Err(invalid());
    };
    if is_emoji_cluster(cluster) { Ok(()) } else { Err(invalid()) }
}

fn is_emoji_cluster(cluster: &str) -> bool {
    const VS16: char = '\u{FE0F}';
    const KEYCAP: char = '\u{20E3}';
    let emoji_presentation = |c: char| {
        matches!(c as u32,
            0x1F000..=0x1FAFF // mahjong, cards, enclosed, pictographs, emoticons, transport, extended-A
            | 0x2600..=0x27BF // misc symbols, dingbats
            | 0x2B05..=0x2B07 | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55
            | 0x231A | 0x231B | 0x23E9..=0x23F3 | 0x23F8..=0x23FA
            | 0x2934 | 0x2935 | 0x3030 | 0x303D | 0x3297 | 0x3299)
    };
    let text_default = |c: char| {
        matches!(c as u32,
            0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA
            | 0x2328 | 0x23CF | 0x24C2 | 0x25AA..=0x25FE)
    };
    let mut chars = cluster.chars();
    let first = chars.next();
    if cluster.ends_with(KEYCAP) {
        return cluster.contains(VS16) && first.is_some_and(|c| matches!(c, '0'..='9' | '#' | '*'));
    }
    cluster.chars().any(emoji_presentation) || (cluster.contains(VS16) && cluster.chars().any(text_default))
}

pub fn validate_topic(topic: &str) -> Result<(), ConnectError> {
    if topic.chars().count() <= TOPIC_MAX_CHARS {
        Ok(())
    } else {
        Err(ConnectError::invalid_argument("topic must be at most 256 characters"))
    }
}

/// Trims `raw` and checks it is 1-100 characters (guild, section and role names).
pub fn display_name(raw: &str, what: &str) -> Result<String, ConnectError> {
    let name = raw.trim();
    if DISPLAY_NAME_LEN.contains(&name.chars().count()) {
        Ok(name.to_owned())
    } else {
        Err(ConnectError::invalid_argument(format!("{what} name must be 1-100 characters")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_icons_and_topics() {
        assert!(validate_color("").is_ok() && validate_color("pink").is_ok());
        assert!(validate_color("Red").is_err() && validate_color("#fff").is_err());
        assert!(validate_icon("").is_ok() && validate_icon("game-controller").is_ok());
        assert!(validate_icon("a1-b2").is_ok());
        for bad in ["Game", "-a", "a-", "a--b", "a b", "a_b", &"a".repeat(49)] {
            assert!(validate_icon(bad).is_err(), "{bad}");
        }
        assert!(validate_icon(&"a".repeat(48)).is_ok());
        assert!(validate_topic(&"é".repeat(256)).is_ok() && validate_topic(&"é".repeat(257)).is_err());
        assert_eq!(display_name("  hi ", "x").unwrap(), "hi");
    }

    #[test]
    fn emoji_names_and_unicode_emoji() {
        for ok in ["ab", "party_parrot", "a1_", &"a".repeat(32)] {
            assert!(validate_emoji_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a", "Ab", "a-b", "a b", "é_é", &"a".repeat(33)] {
            assert!(validate_emoji_name(bad).is_err(), "{bad}");
        }
        for ok in [
            "😀", "👍🏽", "👨\u{200d}👩\u{200d}👧\u{200d}👦", "🇸🇮", "1\u{FE0F}\u{20E3}", "❤\u{FE0F}", "™\u{FE0F}", "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}", "⭐",
        ] {
            assert!(validate_unicode_emoji(ok).is_ok(), "{ok:?}");
        }
        for bad in ["", "a", "1", "ab", "😀😀", "😀a", "é", "™", "x\u{FE0F}", " ", "あ", &"😀".repeat(9)] {
            assert!(validate_unicode_emoji(bad).is_err(), "{bad:?}");
        }
        assert!(display_name("  ", "x").is_err());
    }
}
