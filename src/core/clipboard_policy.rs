//! Host-testable policy for the Android <-> Wayland text clipboard bridge.

pub const TEXT_MIME: &str = "text/plain";
pub const UTF8_TEXT_MIME: &str = "text/plain;charset=utf-8";

/// Maximum clipboard payload accepted (4MB).
pub const MAX_CLIPBOARD_BYTES: usize = 4 * 1024 * 1024;

fn is_utf8_mime(mime: &str) -> bool {
    let m = mime.trim();
    m.eq_ignore_ascii_case(UTF8_TEXT_MIME)
        || m.eq_ignore_ascii_case("text/plain; charset=utf-8")
        || m.eq_ignore_ascii_case("UTF8_STRING")
}

fn is_plain_mime(mime: &str) -> bool {
    let m = mime.trim();
    m.eq_ignore_ascii_case(TEXT_MIME)
        || m.eq_ignore_ascii_case("STRING")
        || m.eq_ignore_ascii_case("TEXT")
}

/// Return whether a MIME type can be represented by the Android text bridge.
pub fn supports_mime_type(mime_type: &str) -> bool {
    is_utf8_mime(mime_type) || is_plain_mime(mime_type)
}

/// Pick the strongest supported text MIME type offered by a Wayland client.
/// UTF-8 variants are preferred over general plain text.
pub fn choose_text_mime<'a, I>(mime_types: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut plain = None;
    for mime_type in mime_types {
        if is_utf8_mime(mime_type) {
            return Some(mime_type);
        }
        if plain.is_none() && is_plain_mime(mime_type) {
            plain = Some(mime_type);
        }
    }
    plain
}

/// Check whether a clipboard text is non-empty and within bounds.
pub fn is_valid_clip_text(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_CLIPBOARD_BYTES
}

/// Validate and sanitize clipboard text, rejecting empty or oversized clips.
pub fn validate_clip_text(text: &str) -> Option<&str> {
    if is_valid_clip_text(text) {
        Some(text)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_plain_text_mime_types() {
        assert!(supports_mime_type("text/plain"));
        assert!(supports_mime_type("TEXT/PLAIN;CHARSET=UTF-8"));
        assert!(supports_mime_type("text/plain; charset=utf-8"));
        assert!(supports_mime_type("  text/plain  "));
        assert!(supports_mime_type("UTF8_STRING"));
        assert!(supports_mime_type("STRING"));
        assert!(supports_mime_type("TEXT"));
        assert!(!supports_mime_type("text/html"));
        assert!(!supports_mime_type("application/octet-stream"));
        assert!(!supports_mime_type(""));
    }

    #[test]
    fn prefers_utf8_text_when_offered() {
        assert_eq!(
            choose_text_mime(["text/html", UTF8_TEXT_MIME]),
            Some(UTF8_TEXT_MIME)
        );
        assert_eq!(
            choose_text_mime(["text/html", "text/plain; charset=utf-8"]),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(
            choose_text_mime(["text/plain", "UTF8_STRING"]),
            Some("UTF8_STRING")
        );
        assert_eq!(choose_text_mime(["text/html", TEXT_MIME]), Some(TEXT_MIME));
        assert_eq!(choose_text_mime(["STRING"]), Some("STRING"));
        assert_eq!(choose_text_mime(["text/html"]), None);
    }

    #[test]
    fn validates_clip_text_ignoring_empty_and_oversized() {
        assert!(is_valid_clip_text("hello"));
        assert!(!is_valid_clip_text(""));
        assert_eq!(validate_clip_text("hello world"), Some("hello world"));
        assert_eq!(validate_clip_text(""), None);

        let oversized = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert!(!is_valid_clip_text(&oversized));
        assert_eq!(validate_clip_text(&oversized), None);
    }

    #[test]
    fn validates_utf8_by_encoded_byte_length_at_the_boundary() {
        let exact_ascii = "a".repeat(MAX_CLIPBOARD_BYTES);
        assert_eq!(exact_ascii.len(), MAX_CLIPBOARD_BYTES);
        assert_eq!(validate_clip_text(&exact_ascii), Some(exact_ascii.as_str()));

        let exact_utf8 = "é".repeat(MAX_CLIPBOARD_BYTES / 2);
        assert_eq!(exact_utf8.len(), MAX_CLIPBOARD_BYTES);
        assert_eq!(validate_clip_text(&exact_utf8), Some(exact_utf8.as_str()));

        let oversized_utf8 = format!("{exact_utf8}é");
        assert!(oversized_utf8.len() > MAX_CLIPBOARD_BYTES);
        assert_eq!(validate_clip_text(&oversized_utf8), None);

        let exact_emoji = "🙂".repeat(MAX_CLIPBOARD_BYTES / 4);
        assert_eq!(exact_emoji.len(), MAX_CLIPBOARD_BYTES);
        assert!(is_valid_clip_text(&exact_emoji));
    }
}
