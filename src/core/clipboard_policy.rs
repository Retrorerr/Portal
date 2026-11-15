//! Size limits for text crossing the Android <-> guest clipboard bridge.

/// Maximum clipboard payload accepted (4MB).
pub const MAX_CLIPBOARD_BYTES: usize = 4 * 1024 * 1024;

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
