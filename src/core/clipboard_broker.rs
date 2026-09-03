//! Bounded, authenticated protocol shared by the Android clipboard broker and
//! the guest Plasma helper.
//!
//! The guest helper talks to the Android process over loopback TCP.  A Unix
//! pathname is intentionally not part of this protocol: the pathname visible
//! to the Android process is not guaranteed to be visible after PRoot changes
//! the guest root.  The broker therefore authenticates every connection with
//! a per-session 256-bit token and keeps the wire format deliberately small.
//!
//! This module contains no Android or Wayland code.  That makes the framing,
//! base64 codec, and loop-suppression policy testable on the host before the
//! lifecycle owner wires the broker into the NativeActivity.

use std::fmt;
use std::io::{self, BufRead, Write};

pub const PROTOCOL_PREFIX: &str = "LDCL/1";
pub const TOKEN_BYTES: usize = 32;
pub const TOKEN_HEX_BYTES: usize = TOKEN_BYTES * 2;
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_HEADER_BYTES: usize = 128;

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthToken([u8; TOKEN_BYTES]);

impl AuthToken {
    pub fn from_bytes(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(value: &str) -> Result<Self, CodecError> {
        if value.len() != TOKEN_HEX_BYTES {
            return Err(CodecError::InvalidToken);
        }
        let mut bytes = [0u8; TOKEN_BYTES];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_nibble(pair[0]).ok_or(CodecError::InvalidToken)?;
            let low = hex_nibble(pair[1]).ok_or(CodecError::InvalidToken)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        let mut result = String::with_capacity(TOKEN_HEX_BYTES);
        for byte in self.0 {
            result.push(hex_digit(byte >> 4));
            result.push(hex_digit(byte & 0x0f));
        }
        result
    }

    pub fn constant_time_eq(&self, candidate: &[u8]) -> bool {
        // Do not return early on a length mismatch.  The loop always scans the
        // fixed-size token and folds the length into the result.
        let mut difference = candidate.len() ^ TOKEN_BYTES;
        for index in 0..TOKEN_BYTES {
            let value = candidate.get(index).copied().unwrap_or_default();
            difference |= (self.0[index] ^ value) as usize;
        }
        difference == 0
    }
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + value - 10) as char,
        _ => unreachable!("hex digit is only called with a nibble"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Hello,
    Subscribe,
    Push,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequest {
    Hello(AuthToken),
    Subscribe,
    Push(String),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerEvent {
    Value(String),
    Clear,
    Ack(RequestKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    InvalidUtf8,
    InvalidToken,
    InvalidHeader,
    UnsupportedVersion,
    UnknownCommand,
    LengthOutOfRange,
    InvalidBase64,
    MissingTerminator,
    EmptyText,
    OversizedText,
    UnexpectedPayload,
    AuthenticationRequired,
    AuthenticationFailed,
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidUtf8 => "clipboard payload is not UTF-8",
            Self::InvalidToken => "invalid broker token",
            Self::InvalidHeader => "invalid broker frame header",
            Self::UnsupportedVersion => "unsupported broker protocol version",
            Self::UnknownCommand => "unknown broker command",
            Self::LengthOutOfRange => "broker payload length is out of range",
            Self::InvalidBase64 => "invalid broker base64 payload",
            Self::MissingTerminator => "broker frame is missing its terminator",
            Self::EmptyText => "empty clipboard text must use CLEAR",
            Self::OversizedText => "clipboard text exceeds the broker limit",
            Self::UnexpectedPayload => "broker command has an unexpected payload",
            Self::AuthenticationRequired => "broker authentication is required",
            Self::AuthenticationFailed => "broker authentication failed",
        })
    }
}

impl std::error::Error for CodecError {}

fn ensure_text(bytes: &[u8]) -> Result<&str, CodecError> {
    if bytes.is_empty() {
        return Err(CodecError::EmptyText);
    }
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(CodecError::OversizedText);
    }
    std::str::from_utf8(bytes).map_err(|_| CodecError::InvalidUtf8)
}

pub fn base64_encoded_len(decoded_len: usize) -> Option<usize> {
    decoded_len.checked_add(2)?.checked_div(3)?.checked_mul(4)
}

pub fn encode_base64(bytes: &[u8]) -> Result<String, CodecError> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(CodecError::OversizedText);
    }
    let encoded_len = base64_encoded_len(bytes.len()).ok_or(CodecError::LengthOutOfRange)?;
    let mut result = String::with_capacity(encoded_len);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied();
        let third = chunk.get(2).copied();
        result.push(BASE64_TABLE[(first >> 2) as usize] as char);
        result.push(
            BASE64_TABLE[((first & 0x03) << 4 | second.unwrap_or_default() >> 4) as usize] as char,
        );
        result.push(match second {
            Some(value) => {
                BASE64_TABLE[((value & 0x0f) << 2 | third.unwrap_or_default() >> 6) as usize]
                    as char
            }
            None => '=',
        });
        result.push(match third {
            Some(value) => BASE64_TABLE[(value & 0x3f) as usize] as char,
            None => '=',
        });
    }
    Ok(result)
}

fn base64_value(value: u8) -> Option<u8> {
    match value {
        b'A'..=b'Z' => Some(value - b'A'),
        b'a'..=b'z' => Some(value - b'a' + 26),
        b'0'..=b'9' => Some(value - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

pub fn decode_base64(encoded: &[u8], expected_len: usize) -> Result<Vec<u8>, CodecError> {
    if expected_len == 0 || expected_len > MAX_TEXT_BYTES {
        return Err(CodecError::LengthOutOfRange);
    }
    let expected_encoded_len =
        base64_encoded_len(expected_len).ok_or(CodecError::LengthOutOfRange)?;
    if encoded.len() != expected_encoded_len
        || encoded
            .iter()
            .any(|byte| *byte == b'\n' || *byte == b'\r' || *byte == b' ' || *byte == b'\t')
    {
        return Err(CodecError::InvalidBase64);
    }

    let mut decoded = Vec::with_capacity(expected_len);
    for (chunk_index, chunk) in encoded.chunks_exact(4).enumerate() {
        let last = chunk_index + 1 == encoded.len() / 4;
        let first = base64_value(chunk[0]).ok_or(CodecError::InvalidBase64)?;
        let second = base64_value(chunk[1]).ok_or(CodecError::InvalidBase64)?;
        let third = if chunk[2] == b'=' {
            if !last || chunk[3] != b'=' || second & 0x0f != 0 {
                return Err(CodecError::InvalidBase64);
            }
            None
        } else {
            Some(base64_value(chunk[2]).ok_or(CodecError::InvalidBase64)?)
        };
        let fourth = if chunk[3] == b'=' {
            if !last || (third.is_some() && third.unwrap_or_default() & 0x03 != 0) {
                return Err(CodecError::InvalidBase64);
            }
            None
        } else {
            Some(base64_value(chunk[3]).ok_or(CodecError::InvalidBase64)?)
        };

        decoded.push((first << 2) | (second >> 4));
        if let Some(third) = third {
            decoded.push((second << 4) | (third >> 2));
            if let Some(fourth) = fourth {
                decoded.push((third << 6) | fourth);
            }
        }
    }
    if decoded.len() != expected_len {
        return Err(CodecError::InvalidBase64);
    }
    Ok(decoded)
}

fn parse_decimal(value: &str) -> Result<usize, CodecError> {
    if value.is_empty() || value.len() > 8 {
        return Err(CodecError::LengthOutOfRange);
    }
    let parsed = value
        .bytes()
        .try_fold(0usize, |current, byte| {
            if !byte.is_ascii_digit() {
                return None;
            }
            current.checked_mul(10)?.checked_add((byte - b'0') as usize)
        })
        .ok_or(CodecError::LengthOutOfRange)?;
    if parsed == 0 || parsed > MAX_TEXT_BYTES {
        return Err(CodecError::LengthOutOfRange);
    }
    Ok(parsed)
}

pub fn encode_hello(token: &AuthToken) -> Vec<u8> {
    format!("{PROTOCOL_PREFIX} HELLO {}\n", token.to_hex()).into_bytes()
}

pub fn encode_subscribe() -> &'static [u8] {
    b"LDCL/1 SUBSCRIBE\n"
}

pub fn encode_push(text: &str) -> Result<Vec<u8>, CodecError> {
    let text = ensure_text(text.as_bytes())?;
    let encoded = encode_base64(text.as_bytes())?;
    let mut frame = format!("{PROTOCOL_PREFIX} PUSH {}\n", text.len()).into_bytes();
    frame.extend_from_slice(encoded.as_bytes());
    frame.push(b'\n');
    Ok(frame)
}

pub fn encode_clear() -> &'static [u8] {
    b"LDCL/1 CLEAR\n"
}

pub fn encode_value_event(text: &str) -> Result<Vec<u8>, CodecError> {
    let text = ensure_text(text.as_bytes())?;
    let encoded = encode_base64(text.as_bytes())?;
    let mut frame = format!("VALUE {}\n", text.len()).into_bytes();
    frame.extend_from_slice(encoded.as_bytes());
    frame.push(b'\n');
    Ok(frame)
}

pub fn encode_clear_event() -> &'static [u8] {
    b"CLEAR\n"
}

pub fn encode_ack(kind: RequestKind) -> &'static [u8] {
    match kind {
        RequestKind::Hello => b"ACK HELLO\n",
        RequestKind::Subscribe => b"ACK SUBSCRIBE\n",
        RequestKind::Push => b"ACK PUSH\n",
        RequestKind::Clear => b"ACK CLEAR\n",
    }
}

fn read_header_line<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, CodecError> {
    let mut line = Vec::with_capacity(MAX_HEADER_BYTES);
    // Read a byte at a time so a peer cannot make `read_until` allocate an
    // unbounded unterminated header before the size check runs.  Headers are
    // intentionally tiny; payloads use the length-checked path below.
    loop {
        if line.len() >= MAX_HEADER_BYTES {
            return Err(CodecError::LengthOutOfRange);
        }
        let mut byte = [0u8; 1];
        let read = reader
            .read(&mut byte)
            .map_err(|_| CodecError::InvalidHeader)?;
        if read == 0 {
            return Err(CodecError::MissingTerminator);
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if line.is_empty() {
        return Err(CodecError::InvalidHeader);
    }
    Ok(line)
}

pub fn read_request<R: BufRead>(reader: &mut R) -> Result<ClientRequest, CodecError> {
    let line = read_header_line(reader)?;
    let line = std::str::from_utf8(&line).map_err(|_| CodecError::InvalidUtf8)?;
    let fields: Vec<&str> = line.split_ascii_whitespace().collect();
    if fields.first().copied() != Some(PROTOCOL_PREFIX) {
        return if fields.first().map(|value| value.starts_with("LDCL/")) == Some(true) {
            Err(CodecError::UnsupportedVersion)
        } else {
            Err(CodecError::InvalidHeader)
        };
    }
    let command = fields.get(1).copied().ok_or(CodecError::InvalidHeader)?;
    match command {
        "HELLO" => {
            if fields.len() != 3 {
                return Err(CodecError::InvalidHeader);
            }
            Ok(ClientRequest::Hello(AuthToken::from_hex(fields[2])?))
        }
        "SUBSCRIBE" => {
            if fields.len() != 2 {
                return Err(CodecError::UnexpectedPayload);
            }
            Ok(ClientRequest::Subscribe)
        }
        "CLEAR" => {
            if fields.len() != 2 {
                return Err(CodecError::UnexpectedPayload);
            }
            Ok(ClientRequest::Clear)
        }
        "PUSH" => {
            if fields.len() != 3 {
                return Err(CodecError::InvalidHeader);
            }
            let byte_len = parse_decimal(fields[2])?;
            let encoded_len = base64_encoded_len(byte_len).ok_or(CodecError::LengthOutOfRange)?;
            let mut encoded = vec![0u8; encoded_len + 1];
            reader
                .read_exact(&mut encoded)
                .map_err(|_| CodecError::MissingTerminator)?;
            if encoded.pop() != Some(b'\n') {
                return Err(CodecError::MissingTerminator);
            }
            let bytes = decode_base64(&encoded, byte_len)?;
            let text = ensure_text(&bytes)?.to_owned();
            Ok(ClientRequest::Push(text))
        }
        _ => Err(CodecError::UnknownCommand),
    }
}

/// Read one server event.  This is used by host tests and by integrations
/// which choose to implement the guest side in Rust instead of the bundled
/// shell helper.
pub fn read_event<R: BufRead>(reader: &mut R) -> Result<BrokerEvent, CodecError> {
    let line = read_header_line(reader)?;
    let line = std::str::from_utf8(&line).map_err(|_| CodecError::InvalidUtf8)?;
    let fields: Vec<&str> = line.split_ascii_whitespace().collect();
    match fields.as_slice() {
        ["CLEAR"] => Ok(BrokerEvent::Clear),
        ["VALUE", value] => {
            let byte_len = parse_decimal(value)?;
            let encoded_len = base64_encoded_len(byte_len).ok_or(CodecError::LengthOutOfRange)?;
            let mut encoded = vec![0u8; encoded_len + 1];
            reader
                .read_exact(&mut encoded)
                .map_err(|_| CodecError::MissingTerminator)?;
            if encoded.pop() != Some(b'\n') {
                return Err(CodecError::MissingTerminator);
            }
            let bytes = decode_base64(&encoded, byte_len)?;
            Ok(BrokerEvent::Value(ensure_text(&bytes)?.to_owned()))
        }
        ["ACK", kind] => {
            let kind = match *kind {
                "HELLO" => RequestKind::Hello,
                "SUBSCRIBE" => RequestKind::Subscribe,
                "PUSH" => RequestKind::Push,
                "CLEAR" => RequestKind::Clear,
                _ => return Err(CodecError::UnknownCommand),
            };
            Ok(BrokerEvent::Ack(kind))
        }
        _ => Err(CodecError::InvalidHeader),
    }
}

pub fn write_all<W: Write>(writer: &mut W, frame: &[u8]) -> io::Result<()> {
    writer.write_all(frame)
}

/// One-shot echo suppression for host writes.  A matching value is consumed
/// exactly once; a later legitimate copy of the same text is not discarded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EchoSuppressor {
    pending: Option<Option<String>>,
}

impl EchoSuppressor {
    pub fn mark_host_update(&mut self, value: Option<&str>) {
        self.pending = Some(value.map(str::to_owned));
    }

    pub fn should_forward_guest_observation(&mut self, value: Option<&str>) -> bool {
        let observed = value.map(str::to_owned);
        if self.pending.as_ref() == Some(&observed) {
            self.pending = None;
            false
        } else {
            true
        }
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }
}

/// Last-value state for the broker.  Empty text is never represented as a
/// value: `None` means an explicit clipboard clear and `Some` is valid UTF-8
/// text within the byte bound.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BrokerState {
    value: Option<String>,
    generation: u64,
}

impl BrokerState {
    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn apply(&mut self, value: Option<&str>) -> Result<bool, CodecError> {
        let next = match value {
            Some(value) => Some(ensure_text(value.as_bytes())?.to_owned()),
            None => None,
        };
        if self.value == next {
            return Ok(false);
        }
        self.value = next;
        self.generation = self.generation.wrapping_add(1).max(1);
        Ok(true)
    }
}
