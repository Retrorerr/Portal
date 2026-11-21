const MAX_CAPTURED_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIAGNOSTIC_LINE_BYTES: usize = 64 * 1024;
// Look past the diagnostic limit far enough to decode a UTF-8 character that
// crosses it before deciding where to truncate the rendered text.
const MAX_LINE_PREVIEW_BYTES: usize = MAX_DIAGNOSTIC_LINE_BYTES + 4;

/// Keep the raw prefix returned to the caller and a separate bounded preview
/// for line-oriented diagnostics. An unfinished line can span any number of
/// reads without growing either buffer beyond its limit.
pub(crate) struct GuestOutput {
    captured: Vec<u8>,
    preview: Vec<u8>,
    oversized: bool,
    tail_has_content: bool,
    filter_transfer_progress: bool,
}

impl GuestOutput {
    pub(crate) fn new(filter_transfer_progress: bool) -> Self {
        Self {
            captured: Vec::new(),
            preview: Vec::new(),
            oversized: false,
            tail_has_content: false,
            filter_transfer_progress,
        }
    }

    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) {
        let preview_bytes = chunk.len().min(MAX_LINE_PREVIEW_BYTES - self.preview.len());
        self.preview.extend_from_slice(&chunk[..preview_bytes]);
        let remainder = &chunk[preview_bytes..];
        if remainder.is_empty() {
            return;
        }
        if !self.tail_has_content {
            self.tail_has_content = remainder.iter().any(|byte| !matches!(byte, b'\r' | b'\n'));
        }
        if !self.oversized {
            // The helper's transfer progress records contain two integers.
            // Once a line outgrows the preview, it is ordinary output and
            // can be captured as it arrives.
            self.oversized = true;
            append_capture(&mut self.captured, &self.preview);
        }
        append_capture(&mut self.captured, remainder);
    }

    pub(crate) fn finish_line(&mut self) -> Option<String> {
        if self.preview.is_empty() {
            return None;
        }
        if !self.oversized
            && !(self.filter_transfer_progress
                && serde_json::from_slice::<serde_json::Value>(&self.preview)
                    .is_ok_and(|value| value.get("bytes").is_some()))
        {
            append_capture(&mut self.captured, &self.preview);
        }

        let decoded = String::from_utf8_lossy(&self.preview);
        let line = if self.tail_has_content {
            decoded.as_ref()
        } else {
            decoded.trim_end_matches(['\r', '\n'])
        };
        let text = if line.len() > MAX_DIAGNOSTIC_LINE_BYTES {
            let mut end = MAX_DIAGNOSTIC_LINE_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            let mut text = line[..end].to_owned();
            text.push_str("…<line truncated>");
            text
        } else {
            line.to_owned()
        };
        self.preview.clear();
        self.oversized = false;
        self.tail_has_content = false;
        Some(text)
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.captured
    }
}

fn append_capture(output: &mut Vec<u8>, bytes: &[u8]) {
    let remaining = MAX_CAPTURED_OUTPUT_BYTES - output.len();
    output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_larger_than_capture_keeps_the_raw_prefix_and_a_bounded_preview() {
        let mut output = GuestOutput::new(false);
        let chunk = [b'x'; 8192];
        for _ in 0..=MAX_CAPTURED_OUTPUT_BYTES / chunk.len() {
            output.push_chunk(&chunk);
        }
        assert_eq!(output.preview.len(), MAX_LINE_PREVIEW_BYTES);
        assert_eq!(
            output.finish_line().unwrap(),
            format!("{}…<line truncated>", "x".repeat(MAX_DIAGNOSTIC_LINE_BYTES))
        );
        output.push_chunk(b"next\r\n");
        assert_eq!(output.finish_line().as_deref(), Some("next"));
        assert_eq!(output.finish_line(), None);
        let captured = output.into_bytes();
        assert_eq!(captured.len(), MAX_CAPTURED_OUTPUT_BYTES);
        assert!(captured.iter().all(|&byte| byte == b'x'));
    }

    #[test]
    fn progress_is_reported_without_entering_the_raw_capture() {
        let mut output = GuestOutput::new(true);
        output.push_chunk(b"{\"bytes\":");
        output.push_chunk(b"1,\"total\":10}\n");
        assert_eq!(
            output.finish_line().as_deref(),
            Some("{\"bytes\":1,\"total\":10}")
        );
        output.push_chunk(b"error: \xff");
        assert_eq!(output.finish_line().as_deref(), Some("error: �"));
        assert_eq!(output.into_bytes(), b"error: \xff");
    }

    #[test]
    fn the_preview_preserves_utf8_boundaries_and_keeps_all_captured_bytes() {
        let mut output = GuestOutput::new(false);
        let prefix = "a".repeat(MAX_DIAGNOSTIC_LINE_BYTES - 1);
        output.push_chunk(prefix.as_bytes());
        let suffix = "🦊remainder\n";
        output.push_chunk(suffix.as_bytes());
        assert_eq!(
            output.finish_line().unwrap(),
            format!("{prefix}…<line truncated>")
        );
        assert_eq!(output.into_bytes(), format!("{prefix}{suffix}").as_bytes());
    }

    #[test]
    fn a_long_line_ending_still_reports_the_complete_short_message() {
        let mut output = GuestOutput::new(false);
        let carriage_returns = vec![b'\r'; MAX_LINE_PREVIEW_BYTES];
        output.push_chunk(b"done");
        output.push_chunk(&carriage_returns);
        output.push_chunk(b"\n");
        assert_eq!(output.finish_line().as_deref(), Some("done"));

        output.push_chunk(b"next");
        output.push_chunk(&carriage_returns);
        output.push_chunk(b"end\n");
        let preview = format!(
            "next{}…<line truncated>",
            "\r".repeat(MAX_DIAGNOSTIC_LINE_BYTES - "next".len())
        );
        assert_eq!(output.finish_line().as_deref(), Some(preview.as_str()));
    }
}
