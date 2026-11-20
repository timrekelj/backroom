//! Input validation shared by the guild and role services.

use connectrpc::ConnectError;

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
        assert!(display_name("  ", "x").is_err());
    }
}
