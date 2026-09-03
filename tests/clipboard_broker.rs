//! Host-side tests for the inner Plasma <-> Android clipboard broker.

#[path = "../src/core/clipboard_broker.rs"]
mod broker;

use std::io::Cursor;

use broker::{
    base64_encoded_len, decode_base64, encode_base64, encode_clear, encode_clear_event,
    encode_hello, encode_push, encode_subscribe, encode_value_event, read_event, read_request,
    AuthToken, BrokerEvent, BrokerState, ClientRequest, CodecError, EchoSuppressor, RequestKind,
    MAX_TEXT_BYTES, TOKEN_BYTES,
};

#[test]
fn token_roundtrip_and_constant_time_comparison_cover_lengths() {
    let token = AuthToken::from_bytes([0x5a; TOKEN_BYTES]);
    let encoded = token.to_hex();
    assert_eq!(encoded.len(), 64);
    assert_eq!(AuthToken::from_hex(&encoded), Ok(token.clone()));
    assert_eq!(
        AuthToken::from_hex(&encoded.to_ascii_uppercase()),
        Ok(token.clone())
    );
    assert!(token.constant_time_eq(&[0x5a; TOKEN_BYTES]));
    assert!(!token.constant_time_eq(&[0x5a; TOKEN_BYTES - 1]));
    assert!(!token.constant_time_eq(&[0x5a; TOKEN_BYTES + 1]));
    assert!(!token.constant_time_eq(&[0x5b; TOKEN_BYTES]));
    assert_eq!(AuthToken::from_hex("00"), Err(CodecError::InvalidToken));
    assert_eq!(
        AuthToken::from_hex(&"g".repeat(64)),
        Err(CodecError::InvalidToken)
    );
}

#[test]
fn base64_codec_is_canonical_and_roundtrips_utf8() {
    for (raw, encoded) in [
        (b"f".as_slice(), "Zg=="),
        (b"fo".as_slice(), "Zm8="),
        (b"foo".as_slice(), "Zm9v"),
        ("clipboard 🙂".as_bytes(), "Y2xpcGJvYXJkIPCfmYI="),
    ] {
        assert_eq!(base64_encoded_len(raw.len()), Some(encoded.len()));
        assert_eq!(encode_base64(raw), Ok(encoded.to_owned()));
        assert_eq!(
            decode_base64(encoded.as_bytes(), raw.len()),
            Ok(raw.to_vec())
        );
    }
    assert_eq!(decode_base64(b"Zh==", 1), Err(CodecError::InvalidBase64));
    assert_eq!(decode_base64(b"Zm9=", 3), Err(CodecError::InvalidBase64));
    assert_eq!(decode_base64(b"Zg==\n", 1), Err(CodecError::InvalidBase64));
    assert_eq!(decode_base64(b"", 0), Err(CodecError::LengthOutOfRange));
    assert_eq!(decode_base64(b"Zm9v", 2), Err(CodecError::InvalidBase64));
}

#[test]
fn request_codec_preserves_clear_as_distinct_from_text() {
    let token = AuthToken::from_bytes([7; TOKEN_BYTES]);
    let hello = encode_hello(&token);
    assert_eq!(
        read_request(&mut Cursor::new(hello)),
        Ok(ClientRequest::Hello(token))
    );
    assert_eq!(
        read_request(&mut Cursor::new(encode_subscribe())),
        Ok(ClientRequest::Subscribe)
    );
    let push = encode_push("line one\nline two").expect("valid push");
    assert_eq!(
        read_request(&mut Cursor::new(push)),
        Ok(ClientRequest::Push("line one\nline two".to_owned()))
    );
    assert_eq!(
        read_request(&mut Cursor::new(encode_clear())),
        Ok(ClientRequest::Clear)
    );
    assert_eq!(encode_push(""), Err(CodecError::EmptyText));
    assert_eq!(
        read_request(&mut Cursor::new(b"LDCL/1 CLEAR extra\n".as_slice())),
        Err(CodecError::UnexpectedPayload)
    );
    assert_eq!(
        read_request(&mut Cursor::new(b"LDCL/2 SUBSCRIBE\n".as_slice())),
        Err(CodecError::UnsupportedVersion)
    );
    assert_eq!(
        read_request(&mut Cursor::new(b"LDCL/1 PUSH 4\nZm9v".as_slice())),
        Err(CodecError::MissingTerminator)
    );
}

#[test]
fn request_codec_rejects_oversized_or_non_utf8_payloads_before_accepting_them() {
    let oversized = format!("LDCL/1 PUSH {}\n", MAX_TEXT_BYTES + 1);
    assert_eq!(
        read_request(&mut Cursor::new(oversized.into_bytes())),
        Err(CodecError::LengthOutOfRange)
    );

    // One decoded byte with a non-UTF-8 value is rejected after the bounded
    // base64 decode, rather than being handed to the Android clipboard.
    assert_eq!(
        read_request(&mut Cursor::new(b"LDCL/1 PUSH 1\n/w==\n".as_slice())),
        Err(CodecError::InvalidUtf8)
    );
    assert_eq!(
        read_request(&mut Cursor::new(b"LDCL/1 PUSH 1\nZg!!\n".as_slice())),
        Err(CodecError::InvalidBase64)
    );

    let mut unterminated = Cursor::new(vec![b'x'; broker::MAX_HEADER_BYTES + 4096]);
    assert_eq!(
        read_request(&mut unterminated),
        Err(CodecError::LengthOutOfRange)
    );
    assert_eq!(unterminated.position(), broker::MAX_HEADER_BYTES as u64);
}

#[test]
fn event_codec_handles_values_acknowledgements_and_explicit_clear() {
    let value = encode_value_event("Android → Plasma").expect("valid event");
    assert_eq!(
        read_event(&mut Cursor::new(value)),
        Ok(BrokerEvent::Value("Android → Plasma".to_owned()))
    );
    assert_eq!(
        read_event(&mut Cursor::new(encode_clear_event())),
        Ok(BrokerEvent::Clear)
    );
    assert_eq!(
        read_event(&mut Cursor::new(b"ACK PUSH\n".as_slice())),
        Ok(BrokerEvent::Ack(RequestKind::Push))
    );
    assert_eq!(
        read_event(&mut Cursor::new(b"VALUE 1\n/w==\n".as_slice())),
        Err(CodecError::InvalidUtf8)
    );
}

#[test]
fn echo_suppression_consumes_only_one_matching_observation() {
    let mut suppressor = EchoSuppressor::default();
    suppressor.mark_host_update(Some("same text"));
    assert!(!suppressor.should_forward_guest_observation(Some("same text")));
    assert!(suppressor.should_forward_guest_observation(Some("same text")));

    suppressor.mark_host_update(None);
    assert!(!suppressor.should_forward_guest_observation(None));
    assert!(suppressor.should_forward_guest_observation(Some("new text")));

    suppressor.mark_host_update(Some("discarded"));
    suppressor.clear();
    assert!(suppressor.should_forward_guest_observation(Some("discarded")));
}

#[test]
fn broker_state_uses_generation_for_real_changes_and_rejects_empty_values() {
    let mut state = BrokerState::default();
    assert_eq!(state.value(), None);
    assert_eq!(state.generation(), 0);
    assert_eq!(state.apply(Some("hello")), Ok(true));
    assert_eq!(state.value(), Some("hello"));
    assert_eq!(state.generation(), 1);
    assert_eq!(state.apply(Some("hello")), Ok(false));
    assert_eq!(state.generation(), 1);
    assert_eq!(state.apply(None), Ok(true));
    assert_eq!(state.value(), None);
    assert_eq!(state.generation(), 2);
    assert_eq!(state.apply(Some("")), Err(CodecError::EmptyText));
    assert_eq!(state.generation(), 2);
}

#[test]
fn guest_helpers_require_inner_wayland_and_ext_capable_clipboard_tools() {
    let push = include_str!("../assets/localdesktop-clipboard-push.sh");
    let sync = include_str!("../assets/localdesktop-clipboard-sync.sh");
    assert!(push.contains("/dev/tcp/$host/$port_number"));
    assert!(push.contains("LDCL/1 HELLO"));
    assert!(push.contains("MAX_TEXT_BYTES + 1"));
    assert!(sync.contains("WAYLAND_DISPLAY"));
    assert!(sync.contains("XDG_RUNTIME_DIR"));
    assert!(sync.contains("wl-paste --version"));
    assert!(sync.contains("version_minor < 3"));
    assert!(sync.contains("--watch"));
    assert!(sync.contains("--foreground"));
    assert!(!sync.contains("WAYLAND_DISPLAY=wayland-1"));
}
