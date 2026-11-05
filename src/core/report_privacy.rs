//! Diagnostics archives end up attached to public issues, and the guest log
//! carries whatever desktop apps print. Strip what identifies the person or
//! what they were doing, and keep what identifies the bug: package names,
//! system paths, versions, errors and Portal's own download hosts.

use regex::{Captures, Regex};
use std::sync::OnceLock;

/// Hosts Portal or the guest's package tools fetch from. Which of them failed
/// is often the bug, and nothing in those URLs is about the user.
const KEPT_HOSTS: &[&str] = &[
    "github.com",
    "githubusercontent.com",
    "debian.org",
    "flathub.org",
    "localhost",
    "127.0.0.1",
];

struct Patterns {
    url: Regex,
    email: Regex,
    mac: Regex,
    ipv4: Regex,
    ipv6: Regex,
    shared_storage: Regex,
    home: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        url: Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s"'<>\\)\]]*"#).unwrap(),
        email: Regex::new(r"[A-Za-z0-9._%+-]+@(?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,}").unwrap(),
        mac: Regex::new(r"(?i)\b[0-9a-f]{2}(?::[0-9a-f]{2}){5}\b").unwrap(),
        ipv4: Regex::new(r"\b\d{1,3}(?:\.\d{1,3}){3}\b").unwrap(),
        // Global unicast only (2000::/3); link-local and ULA say nothing.
        ipv6: Regex::new(r"(?i)\b[23][0-9a-f]{3}(?::[0-9a-f]{0,4}){2,7}\b").unwrap(),
        shared_storage: Regex::new(
            r#"(/storage/(?:emulated/\d+|[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4})|/sdcard)/[^\s"':]+"#,
        )
        .unwrap(),
        // Dot directories stay: `.config/kwinrc` is a setting, not a document.
        home: Regex::new(r#"(/home/[^/\s"']+|/root)/[^.\s"':/][^\s"':]*"#).unwrap(),
    })
}

fn kept_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    KEPT_HOSTS
        .iter()
        .any(|kept| host == *kept || host.ends_with(&format!(".{kept}")))
}

fn scrub_url(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // file:///path has no host; the path rules below judge it.
    if host.is_empty() {
        return None;
    }
    // Userinfo can hide a token, and fools a naive host check.
    if !host.contains('@') && kept_host(host.split(':').next().unwrap_or_default()) {
        return None;
    }
    Some(format!("{scheme}://<url>"))
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
    let text = patterns.url.replace_all(text, |caps: &Captures| {
        let url = &caps[0];
        scrub_url(url).unwrap_or_else(|| url.to_owned())
    });
    let text = patterns.email.replace_all(&text, "<email>");
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
    let text = patterns.ipv6.replace_all(&text, "<ip>");
    let text = patterns.shared_storage.replace_all(&text, "$1/<path>");
    patterns.home.replace_all(&text, "$1/<path>").into_owned()
}

/// Binary files can't be scrubbed or read in an issue, so they stay home.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8 * 1024).any(|byte| *byte == 0)
}
