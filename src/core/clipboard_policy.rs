//! Host-testable policy for the Android <-> Wayland text clipboard bridge.

pub const TEXT_MIME: &str = "text/plain";
pub const UTF8_TEXT_MIME: &str = "text/plain;charset=utf-8";

pub fn supports_mime_type(mime_type: &str) -> bool {
    mime_type.eq_ignore_ascii_case(TEXT_MIME) || mime_type.eq_ignore_ascii_case(UTF8_TEXT_MIME)
}

pub fn choose_text_mime<'a, I>(mime_types: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut plain = None;
    for mime_type in mime_types {
        if mime_type.eq_ignore_ascii_case(UTF8_TEXT_MIME) {
            return Some(mime_type);
        }
        if plain.is_none() && mime_type.eq_ignore_ascii_case(TEXT_MIME) {
            plain = Some(mime_type);
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_plain_text_mime_types() {
        assert!(supports_mime_type("text/plain"));
        assert!(supports_mime_type("TEXT/PLAIN;CHARSET=UTF-8"));
        assert!(!supports_mime_type("text/html"));
    }

    #[test]
    fn prefers_utf8_text_when_offered() {
        assert_eq!(
            choose_text_mime(["text/html", UTF8_TEXT_MIME]),
            Some(UTF8_TEXT_MIME)
        );
        assert_eq!(choose_text_mime(["text/html", TEXT_MIME]), Some(TEXT_MIME));
        assert_eq!(choose_text_mime(["text/html"]), None);
    }
}
