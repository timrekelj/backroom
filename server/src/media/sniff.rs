//! Pure helpers for message attachments: filename sanitising, content-type
//! sniffing and the serving policy (`Content-Type` / `Content-Disposition`).

/// Bytes of a file's head used for sniffing.
pub const SNIFF_LEN: usize = 512;
/// Longest stored file name, in characters.
pub const MAX_FILENAME_CHARS: usize = 200;
const FALLBACK_FILENAME: &str = "file";
const OCTET_STREAM: &str = "application/octet-stream";

/// Strips path components, control and bidi-override characters, and caps the
/// length at [`MAX_FILENAME_CHARS`] while keeping a short extension.
pub fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}'))
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        return FALLBACK_FILENAME.to_owned();
    }
    if cleaned.chars().count() <= MAX_FILENAME_CHARS {
        return cleaned.to_owned();
    }
    let (stem, ext) = match cleaned.rfind('.') {
        Some(i) if cleaned[i..].chars().count() <= 20 => cleaned.split_at(i),
        _ => (cleaned, ""),
    };
    let keep = MAX_FILENAME_CHARS - ext.chars().count();
    let stem: String = stem.chars().take(keep).collect();
    format!("{stem}{ext}")
}

/// A `type/subtype` MIME type without parameters, lowercased, or `None`.
pub fn sane_mime(declared: &str) -> Option<String> {
    let essence = declared.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, sub) = essence.split_once('/')?;
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    (token(kind) && token(sub)).then_some(essence)
}

/// Content type of a file from its first bytes, for the types we recognise.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let starts = |p: &[u8]| head.starts_with(p);
    let at = |i: usize, p: &[u8]| head.get(i..i + p.len()) == Some(p);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if starts(b"RIFF") && at(8, b"WEBP") {
        Some("image/webp")
    } else if starts(b"RIFF") && at(8, b"WAVE") {
        Some("audio/wav")
    } else if starts(b"%PDF-") {
        Some("application/pdf")
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        Some("application/zip")
    } else if at(4, b"ftyp") {
        Some(match head.get(8..12) {
            Some(b"qt  ") => "video/quicktime",
            Some(b"M4A ") | Some(b"M4B ") => "audio/mp4",
            _ => "video/mp4",
        })
    } else if starts(&[0x1a, 0x45, 0xdf, 0xa3]) {
        let doctype_webm = head.windows(4).any(|w| w == b"webm");
        Some(if doctype_webm {
            "video/webm"
        } else {
            "video/x-matroska"
        })
    } else if starts(b"OggS") {
        Some("audio/ogg")
    } else if starts(b"fLaC") {
        Some("audio/flac")
    } else if starts(b"ID3")
        || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe0 == 0xe0 && head[1] & 0x06 != 0)
    {
        Some("audio/mpeg")
    } else if starts(&[0x1f, 0x8b]) {
        Some("application/gzip")
    } else if starts(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        Some("application/x-7z-compressed")
    } else if starts(b"Rar!\x1a\x07") {
        Some("application/vnd.rar")
    } else {
        None
    }
}

/// Whether the browser may render the file inside a page: only types that were
/// positively identified from the bytes and cannot execute script.
pub fn renders_inline(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "video/mp4"
            | "video/webm"
            | "video/quicktime"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/wav"
            | "audio/flac"
            | "audio/mp4"
            | "application/pdf"
    )
}

/// Types a browser could execute or interpret as markup when served inline.
fn is_active_content(content_type: &str) -> bool {
    content_type.contains("html")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("ecmascript")
        || content_type == "image/svg+xml"
        || content_type.starts_with("text/xsl")
}

/// How `/media` serves an attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serving {
    pub content_type: String,
    pub inline: bool,
}

/// Decides the served `Content-Type` and disposition. `verified` says the type
/// was recognised from the file's bytes (not just declared by the client);
/// unverified files are never served inline, and active content (svg, html,
/// xml, scripts) is served as an opaque download.
pub fn serving(content_type: &str, verified: bool) -> Serving {
    if is_active_content(content_type) {
        return Serving {
            content_type: OCTET_STREAM.to_owned(),
            inline: false,
        };
    }
    Serving {
        content_type: content_type.to_owned(),
        inline: verified && renders_inline(content_type),
    }
}

/// `inline; filename*=UTF-8''…` or `attachment; filename*=UTF-8''…` (RFC 6266 / 8187).
pub fn content_disposition(inline: bool, filename: &str) -> String {
    let mut out = String::from(if inline { "inline" } else { "attachment" });
    out.push_str("; filename*=UTF-8''");
    for b in filename.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_sanitised() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Users\\me\\a b.txt"), "a b.txt");
        assert_eq!(sanitize_filename("a\u{0}b\nc.txt"), "abc.txt");
        assert_eq!(sanitize_filename("exe\u{202e}gpj.exe"), "exegpj.exe");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename("///"), "file");
        let long = format!("{}.tar.gz", "a".repeat(500));
        let out = sanitize_filename(&long);
        assert_eq!(out.chars().count(), MAX_FILENAME_CHARS);
        assert!(out.ends_with(".gz"));
        assert_eq!(
            sanitize_filename(&"é".repeat(300)).chars().count(),
            MAX_FILENAME_CHARS
        );
    }

    #[test]
    fn mime_types_are_checked() {
        assert_eq!(
            sane_mime("Text/Plain; charset=utf-8").as_deref(),
            Some("text/plain")
        );
        assert_eq!(sane_mime("image/svg+xml").as_deref(), Some("image/svg+xml"));
        for bad in [
            "",
            "text",
            "/x",
            "x/",
            "a b/c",
            "text/pl\nain",
            "te\"xt/x",
            "*/*",
        ] {
            assert_eq!(sane_mime(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn known_types_are_sniffed() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("audio/wav"));
        assert_eq!(sniff(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(sniff(b"PK\x03\x04rest"), Some("application/zip"));
        assert_eq!(sniff(b"\0\0\0\x18ftypisom"), Some("video/mp4"));
        assert_eq!(sniff(b"\0\0\0\x14ftypqt  "), Some("video/quicktime"));
        assert_eq!(
            sniff(b"\x1a\x45\xdf\xa3\x01\0\0\0 webm"),
            Some("video/webm")
        );
        assert_eq!(sniff(b"ID3\x04"), Some("audio/mpeg"));
        assert_eq!(sniff(b"hello world"), None);
        assert_eq!(sniff(b"<svg xmlns=...>"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn serving_policy() {
        let png = serving("image/png", true);
        assert!(png.inline && png.content_type == "image/png");
        // Declared-only types never render inline.
        assert!(!serving("image/png", false).inline);
        assert!(!serving("application/zip", true).inline);
        for active in [
            "image/svg+xml",
            "text/html",
            "application/xhtml+xml",
            "text/xml",
            "application/javascript",
        ] {
            let s = serving(active, true);
            assert!(
                !s.inline && s.content_type == "application/octet-stream",
                "{active}"
            );
        }
        assert_eq!(serving("text/plain", false).content_type, "text/plain");
    }

    #[test]
    fn disposition_is_rfc8187_encoded() {
        assert_eq!(
            content_disposition(false, "a b.txt"),
            "attachment; filename*=UTF-8''a%20b.txt"
        );
        assert_eq!(
            content_disposition(true, "ž.png"),
            "inline; filename*=UTF-8''%C5%BE.png"
        );
        assert_eq!(
            content_disposition(false, "a\"b;c'd"),
            "attachment; filename*=UTF-8''a%22b%3Bc%27d"
        );
    }
}
