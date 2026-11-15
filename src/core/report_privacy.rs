//! Diagnostics archives end up attached to public issues, and the guest log
//! carries whatever desktop apps print. Strip what identifies the person or
//! what they were doing, and keep what identifies the bug: package names,
//! system paths, versions, errors and Portal's own download hosts.

use regex::{Captures, Regex};
use reqwest::Url;
use std::net::Ipv6Addr;
use std::sync::OnceLock;

struct Patterns {
    url: Regex,
    text_input_context: Regex,
    email: Regex,
    mac: Regex,
    ipv4: Regex,
    ipv6: Regex,
    quoted_paths: [Regex; 2],
    shared_storage: Regex,
    home: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        url: Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s"<>\\]+"#).unwrap(),
        text_input_context: Regex::new(r"(?m)(Portal text-input-v2: other request: SetSurroundingText\b)[^\r\n]*")
            .unwrap(),
        email: Regex::new(r"[A-Za-z0-9._%+-]+@(?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,}").unwrap(),
        mac: Regex::new(r"(?i)\b[0-9a-f]{2}(?::[0-9a-f]{2}){5}\b").unwrap(),
        ipv4: Regex::new(r"\b\d{1,3}(?:\.\d{1,3}){3}\b").unwrap(),
        // Global unicast only (2000::/3); link-local and ULA say nothing.
        ipv6: Regex::new(r"(?i)\b[23][0-9a-f]{3}:[0-9a-f:.]+").unwrap(),
        quoted_paths: ['"', '\''].map(|quote| {
            let base = r#"(?:file://)?(?:/storage/(?:emulated/\d+|[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4})|/sdcard|/home/[^/\s"']+|/root)"#;
            Regex::new(&format!(r#"{quote}({base})/([^{quote}./\r\n][^{quote}\r\n]*){quote}"#))
                .unwrap()
        }),
        shared_storage: Regex::new(
            r#"(/storage/(?:emulated/\d+|[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4})|/sdcard)/[^\s"':]+"#,
        )
        .unwrap(),
        // Dot directories stay: `.config/kwinrc` is a setting, not a document.
        home: Regex::new(r#"(/home/[^/\s"']+|/root)/[^.\s"':/][^\s"':]*"#).unwrap(),
    })
}

fn kept_path(host: &str, path: &str) -> bool {
    let portal_repo = |prefix: &str| {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    match host {
        "github.com" | "raw.githubusercontent.com" => portal_repo("/Retrorerr/Portal"),
        "api.github.com" => portal_repo("/repos/Retrorerr/Portal"),
        host => ["debian.org", "flathub.org"].into_iter().any(|domain| {
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.ends_with('.'))
        }),
    }
}

fn scrub_url(raw: &str) -> String {
    let url = raw.trim_end_matches(['\'', ')', ',', ';']);
    let punctuation = &raw[url.len()..];
    let (scheme, rest) = url.split_once("://").expect("matched a URL");
    if scheme.eq_ignore_ascii_case("file") && rest.starts_with('/') {
        // The path rules below handle local file URLs.
        return raw.to_owned();
    }
    if let Ok(mut parsed) = Url::parse(url) {
        if matches!(parsed.scheme(), "http" | "https")
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed
                .host_str()
                .is_some_and(|host| kept_path(host, parsed.path()))
        {
            parsed.set_query(None);
            parsed.set_fragment(None);
            return format!("{parsed}{punctuation}");
        }
    }
    format!("{scheme}://<url>{punctuation}")
}

/// A dotted quad that is an address, not a version: `libfoo.so.1.2.3.4` and
/// `mesa-26.3.0.1` stay, as do loopback, the unspecified address and masks.
fn is_address(text: &str, start: usize, end: usize) -> bool {
    let quad = &text[start..end];
    let octets: Vec<u32> = quad
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    if octets.len() != 4 || octets.iter().any(|octet| *octet > 255) {
        return false;
    }
    if octets[0] == 127 || octets[0] == 255 || octets.iter().all(|octet| *octet == 0) {
        return false;
    }
    let before = text[..start].chars().next_back();
    if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')) {
        return false;
    }
    let mut after = text[end..].chars();
    match after.next() {
        Some(c) if c.is_alphanumeric() || matches!(c, '_' | '-') => false,
        Some('.') => !after.next().is_some_and(|c| c.is_alphanumeric()),
        _ => true,
    }
}

/// Text safe enough to post, as far as a pattern can tell. It cannot catch
/// everything an app might print, so the report screen still asks people to
/// look the logs over.
pub fn scrub(text: &str) -> String {
    let patterns = patterns();
    // Older Portal builds logged full text-input requests at info level.
    let text = patterns.text_input_context.replace_all(text, "$1 <text>");
    let text = patterns
        .url
        .replace_all(&text, |caps: &Captures| scrub_url(&caps[0]));
    let text = patterns.email.replace_all(&text, "<email>");
    let text = patterns.ipv6.replace_all(&text, |caps: &Captures| {
        let candidate = &caps[0];
        let address = candidate.trim_end_matches('.');
        if address.parse::<Ipv6Addr>().is_ok() {
            format!("<ip>{}", &candidate[address.len()..])
        } else if let Some(address) = address
            .strip_suffix(':')
            .filter(|s| s.parse::<Ipv6Addr>().is_ok())
        {
            format!("<ip>{}", &candidate[address.len()..])
        } else {
            candidate.to_owned()
        }
    });
    let text = patterns.mac.replace_all(&text, "<mac>");
    let quads = text.clone();
    let text = patterns.ipv4.replace_all(&text, |caps: &Captures| {
        let found = caps.get(0).expect("group 0 always exists");
        if is_address(&quads, found.start(), found.end()) {
            "<ip>".to_owned()
        } else {
            found.as_str().to_owned()
        }
    });
    let text = patterns.quoted_paths[0].replace_all(&text, r#""$1/<path>""#);
    let text = patterns.quoted_paths[1].replace_all(&text, "'$1/<path>'");
    let text = patterns.shared_storage.replace_all(&text, "$1/<path>");
    patterns.home.replace_all(&text, "$1/<path>").into_owned()
}

/// Binary files can't be scrubbed or read in an issue, so they stay home.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}
