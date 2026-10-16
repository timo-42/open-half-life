//! Independently authored synthetic field values and manual wire vectors from
//! the published protocol's field order. No source packet dumps, observed
//! names, resource identifiers, original media or implementation supplied these
//! fixtures. Tests select byte primitives explicitly and identify no game build.

use ohl_net::{
    MAX_DATAGRAM_BYTES, MAX_RESPONSE_BYTES, MAX_RULES, MAX_STRING_BYTES, PlayerInfo, ServerRule,
    legacy::{
        Details, Error, Modification, Profile, Request, Response, decode_request, decode_response,
        encode_request, encode_response,
    },
};

const PROFILE: Profile = Profile::PublishedAscii;
const DETAILS: &[u8] = b"\xff\xff\xff\xffmX\0A\0B\0C\0D\0\x02\x08\x2adl\x01\0\x01";
const MOD_DETAILS: &[u8] = b"\xff\xff\xff\xffmX\0A\0B\0C\0D\0\x02\x08\x2adl\x01\x01W\0L\0U\0\xfe\xff\xff\xff\x04\x03\x02\x01\x01\0\x01";
const PLAYERS: &[u8] =
    b"\xff\xff\xff\xffD\x02\x07A\0\xfe\xff\xff\xff\0\0\xc0\x3f\x07\0\0\0\0\0\0\0\x80\xbf";
const RULES: &[u8] = b"\xff\xff\xff\xffE\x02\0k\0v\0k\0\0";

fn details() -> Details {
    Details {
        address: "X".into(),
        name: "A".into(),
        map: "B".into(),
        game_directory: "C".into(),
        game_description: "D".into(),
        players: 2,
        max_players: 8,
        protocol: 0x2a,
        server_type: b'd',
        environment: b'l',
        password_protected: true,
        modification: None,
        secure_server: true,
    }
}

fn modification() -> Modification {
    Modification {
        website: "W".into(),
        download_server: "L".into(),
        unused: "U".into(),
        version: -2,
        size: 0x0102_0304,
        server_side_only: true,
        custom_dll: false,
    }
}

fn modified_details() -> Details {
    Details {
        modification: Some(modification()),
        ..details()
    }
}

fn players() -> Response {
    Response::Players(vec![
        PlayerInfo {
            index: 7,
            name: "A".into(),
            score: -2,
            duration_seconds: 1.5,
        },
        PlayerInfo {
            index: 7,
            name: String::new(),
            score: 0,
            duration_seconds: -1.0,
        },
    ])
}

fn rules() -> Response {
    Response::Rules(vec![
        ServerRule {
            name: "k".into(),
            value: "v".into(),
        },
        ServerRule {
            name: "k".into(),
            value: String::new(),
        },
    ])
}

fn encoded_bytes(response: &Response) -> Vec<u8> {
    let mut output = vec![0xa5; MAX_RESPONSE_BYTES];
    let length = encode_response(PROFILE, response, &mut output)
        .unwrap()
        .len();
    output.truncate(length);
    output
}

fn assert_exact(response: &Response, expected: &[u8]) {
    let mut output = vec![0xa5; expected.len() + 8];
    let encoded = encode_response(PROFILE, response, &mut output).unwrap();
    assert_eq!(encoded.as_bytes(), expected);
    assert_eq!(encoded.len(), expected.len());
    assert!(!encoded.is_empty());
    assert_eq!(&output[expected.len()..], &[0xa5; 8]);
    assert_eq!(decode_response(PROFILE, expected).unwrap(), *response);
}

fn assert_rejected(response: &Response, expected: Error) {
    let mut output = vec![0xa5; MAX_RESPONSE_BYTES + 8];
    assert_eq!(
        encode_response(PROFILE, response, &mut output),
        Err(expected)
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));
}

fn rules_sized(total: usize) -> Response {
    // Four header bytes, response type and two-byte count, then two string
    // terminators per rule plus each independently authored value's bytes.
    let mut remaining = total - 7;
    let mut entries = Vec::new();
    while remaining > MAX_STRING_BYTES + 2 {
        entries.push(ServerRule {
            name: String::new(),
            value: "x".repeat(MAX_STRING_BYTES),
        });
        remaining -= MAX_STRING_BYTES + 2;
    }
    assert!(remaining >= 2);
    entries.push(ServerRule {
        name: String::new(),
        value: "y".repeat(remaining - 2),
    });
    Response::Rules(entries)
}

#[test]
fn manual_ascii_requests_have_exactly_no_terminator() {
    for (request, expected) in [
        (Request::Details, b"\xff\xff\xff\xffdetails".as_slice()),
        (Request::Players, b"\xff\xff\xff\xffplayers".as_slice()),
        (Request::Rules, b"\xff\xff\xff\xffrules".as_slice()),
    ] {
        assert_eq!(decode_request(PROFILE, expected), Ok(request));
        let mut output = vec![0xa5; expected.len() + 8];
        let encoded = encode_request(PROFILE, request, &mut output).unwrap();
        assert_eq!(encoded.as_bytes(), expected);
        assert_eq!(encoded.len(), expected.len());
        assert!(!encoded.is_empty());
        assert_eq!(&output[expected.len()..], &[0xa5; 8]);
        for suffix in [
            b"\0".as_slice(),
            b"\n".as_slice(),
            b"\r\n".as_slice(),
            &[1, 2, 3, 4],
        ] {
            let mut extended = expected.to_vec();
            extended.extend_from_slice(suffix);
            assert_eq!(decode_request(PROFILE, &extended), Err(Error::TrailingData));
        }
    }
}

#[test]
fn request_truncation_unknown_commands_and_datagram_bounds_are_exact() {
    for packet in [
        b"\xff\xff\xff\xffdetails".as_slice(),
        b"\xff\xff\xff\xffplayers".as_slice(),
        b"\xff\xff\xff\xffrules".as_slice(),
    ] {
        for end in 0..packet.len() {
            assert_eq!(
                decode_request(PROFILE, &packet[..end]),
                Err(Error::Truncated)
            );
        }
    }
    for packet in [
        b"\xff\xff\xff\xffDetails".as_slice(),
        b"\xff\xff\xff\xffunknown".as_slice(),
        b"\xff\xff\xff\xffTSource Engine Query\0".as_slice(),
        b"\xff\xff\xff\xffU\xff\xff\xff\xff".as_slice(),
    ] {
        assert_eq!(
            decode_request(PROFILE, packet),
            Err(Error::UnsupportedRequest)
        );
    }
    assert_eq!(
        decode_request(PROFILE, b"\0\0\0\0rules"),
        Err(Error::InvalidHeader)
    );
    assert_eq!(
        decode_request(PROFILE, b"\xfe\xff\xff\xffrules"),
        Err(Error::InvalidHeader)
    );
    let mut maximum = vec![0; MAX_DATAGRAM_BYTES];
    maximum[..9].copy_from_slice(b"\xff\xff\xff\xffrules");
    assert_eq!(decode_request(PROFILE, &maximum), Err(Error::TrailingData));
    maximum.push(0);
    assert_eq!(decode_request(PROFILE, &maximum), Err(Error::DatagramLimit));
}

#[test]
fn request_capacity_failure_preserves_entire_caller_buffer() {
    for request in [Request::Details, Request::Players, Request::Rules] {
        let mut success = [0xa5; 32];
        let length = encode_request(PROFILE, request, &mut success)
            .unwrap()
            .len();
        for capacity in 0..length {
            let mut output = vec![0xa5; capacity];
            assert_eq!(
                encode_request(PROFILE, request, &mut output),
                Err(Error::OutputTooSmall)
            );
            assert!(output.iter().all(|byte| *byte == 0xa5));
        }
    }
}

#[test]
fn details_matches_manual_vector_and_ends_at_secure_byte() {
    assert_exact(&Response::Details(Box::new(details())), DETAILS);
    assert_eq!(DETAILS.last(), Some(&1));
    for extra_bot_byte in [0, 1, 255] {
        let mut later_layout = DETAILS.to_vec();
        later_layout.push(extra_bot_byte);
        assert_eq!(
            decode_response(PROFILE, &later_layout),
            Err(Error::TrailingData)
        );
    }
    assert!(ohl_net::decode_response(DETAILS).is_err());
}

#[test]
fn mod_details_preserve_nonempty_unused_string_and_signed_words() {
    let response = Response::Details(Box::new(modified_details()));
    assert_exact(&response, MOD_DETAILS);
    let Response::Details(parsed) = decode_response(PROFILE, MOD_DETAILS).unwrap() else {
        panic!("wrong synthetic response type")
    };
    assert_eq!(parsed.modification.as_ref().unwrap().unused, "U");
    assert_eq!(parsed.modification.as_ref().unwrap().version, -2);
    // The later A2S DTO expects a single reserved zero here; this independent
    // historical DTO deliberately preserves a different published field.
    assert!(ohl_net::decode_response(MOD_DETAILS).is_err());
    let mut empty = modified_details();
    empty.modification.as_mut().unwrap().unused.clear();
    let bytes = encoded_bytes(&Response::Details(Box::new(empty.clone())));
    assert_eq!(
        decode_response(PROFILE, &bytes),
        Ok(Response::Details(Box::new(empty)))
    );
}

#[test]
fn player_and_rule_lists_match_manual_vectors_and_preserve_duplicates() {
    assert_exact(&players(), PLAYERS);
    assert_exact(&rules(), RULES);
    assert_exact(&Response::Players(Vec::new()), b"\xff\xff\xff\xffD\0");
    assert_exact(&Response::Rules(Vec::new()), b"\xff\xff\xff\xffE\0\0");
}

#[test]
fn every_synthetic_response_prefix_is_truncated_and_suffix_rejected() {
    for response in [DETAILS, MOD_DETAILS, PLAYERS, RULES] {
        for end in 0..response.len() {
            assert_eq!(
                decode_response(PROFILE, &response[..end]),
                Err(Error::Truncated)
            );
        }
        let mut extended = response.to_vec();
        extended.push(0x91);
        assert_eq!(
            decode_response(PROFILE, &extended),
            Err(Error::TrailingData)
        );
    }
    for packet in [
        b"\xff\xff\xff\xffI".as_slice(),
        b"\xff\xff\xff\xffA\0\0\0\0".as_slice(),
        b"\xff\xff\xff\xff\x91".as_slice(),
    ] {
        assert_eq!(
            decode_response(PROFILE, packet),
            Err(Error::UnsupportedResponse)
        );
    }
    assert_eq!(
        decode_response(PROFILE, b"\xfe\xff\xff\xffD\0"),
        Err(Error::InvalidHeader)
    );
    assert_eq!(
        decode_response(PROFILE, b"\0\0\0\0D\0"),
        Err(Error::InvalidHeader)
    );
}

#[test]
fn canonical_boolean_selection_is_enforced_at_every_detail_boolean() {
    // Five two-byte strings follow the five-byte response prefix. The last
    // eight base bytes then contain counts, protocol, type, OS and three bools.
    for index in [20, 21, 22] {
        let mut invalid = DETAILS.to_vec();
        invalid[index] = 2;
        assert_eq!(
            decode_response(PROFILE, &invalid),
            Err(Error::InvalidBoolean)
        );
    }
    for index in [
        20,
        21,
        MOD_DETAILS.len() - 3,
        MOD_DETAILS.len() - 2,
        MOD_DETAILS.len() - 1,
    ] {
        let mut invalid = MOD_DETAILS.to_vec();
        invalid[index] = 0x91;
        assert_eq!(
            decode_response(PROFILE, &invalid),
            Err(Error::InvalidBoolean)
        );
    }
}

fn responses_with_text(value: &str) -> Vec<Response> {
    let mut responses = Vec::new();
    for position in 0..5 {
        let mut metadata = details();
        match position {
            0 => metadata.address = value.into(),
            1 => metadata.name = value.into(),
            2 => metadata.map = value.into(),
            3 => metadata.game_directory = value.into(),
            _ => metadata.game_description = value.into(),
        }
        responses.push(Response::Details(Box::new(metadata)));
    }
    for position in 0..3 {
        let mut metadata = modified_details();
        let modification = metadata.modification.as_mut().unwrap();
        match position {
            0 => modification.website = value.into(),
            1 => modification.download_server = value.into(),
            _ => modification.unused = value.into(),
        }
        responses.push(Response::Details(Box::new(metadata)));
    }
    responses.push(Response::Players(vec![PlayerInfo {
        index: 0,
        name: value.into(),
        score: 0,
        duration_seconds: 0.0,
    }]));
    for name in [false, true] {
        responses.push(Response::Rules(vec![
            ServerRule {
                name: "valid".into(),
                value: "earlier".into(),
            },
            ServerRule {
                name: if name { value.into() } else { String::new() },
                value: if name { String::new() } else { value.into() },
            },
        ]));
    }
    responses
}

#[test]
fn all_response_text_positions_have_bounded_atomic_encoding() {
    for response in responses_with_text("SYNTHETIC\0PRIVATE") {
        assert_rejected(&response, Error::EmbeddedNul);
    }
    for response in responses_with_text(&"x".repeat(MAX_STRING_BYTES + 1)) {
        assert_rejected(&response, Error::StringLimit);
    }
    for response in responses_with_text(&"é".repeat(MAX_STRING_BYTES / 2)) {
        assert_eq!(
            decode_response(PROFILE, &encoded_bytes(&response)),
            Ok(response)
        );
    }
    let too_many_bytes = Response::Rules(vec![ServerRule {
        name: "é".repeat(MAX_STRING_BYTES / 2 + 1),
        value: String::new(),
    }]);
    assert_rejected(&too_many_bytes, Error::StringLimit);
}

#[test]
fn every_text_position_checks_utf8_terminator_and_bound_during_decoding() {
    const MARKER: &[u8] = b"SYNTHETIC_FIELD";
    for response in responses_with_text("SYNTHETIC_FIELD") {
        let bytes = encoded_bytes(&response);
        let start = bytes
            .windows(MARKER.len())
            .position(|part| part == MARKER)
            .unwrap();
        let mut invalid = bytes.clone();
        invalid[start] = 255;
        assert_eq!(decode_response(PROFILE, &invalid), Err(Error::InvalidUtf8));
        assert_eq!(
            decode_response(PROFILE, &bytes[..start + MARKER.len()]),
            Err(Error::Truncated)
        );
        let mut oversized = bytes;
        oversized.splice(
            start..start + MARKER.len(),
            vec![b'x'; MAX_STRING_BYTES + 1],
        );
        assert_eq!(
            decode_response(PROFILE, &oversized),
            Err(Error::StringLimit)
        );
    }
}

#[test]
fn count_budgets_and_minimum_entry_sizes_are_checked_before_allocation() {
    let player = PlayerInfo {
        index: 255,
        name: String::new(),
        score: i32::MIN,
        duration_seconds: -1.0,
    };
    let maximum = Response::Players(vec![player.clone(); 255]);
    assert_eq!(
        decode_response(PROFILE, &encoded_bytes(&maximum)),
        Ok(maximum)
    );
    assert_rejected(&Response::Players(vec![player; 256]), Error::PlayerLimit);
    assert_eq!(
        decode_response(PROFILE, b"\xff\xff\xff\xffD\xff"),
        Err(Error::Truncated)
    );
    let rule = ServerRule {
        name: String::new(),
        value: String::new(),
    };
    let maximum = Response::Rules(vec![rule.clone(); MAX_RULES]);
    assert_eq!(
        decode_response(PROFILE, &encoded_bytes(&maximum)),
        Ok(maximum)
    );
    assert_rejected(
        &Response::Rules(vec![rule; MAX_RULES + 1]),
        Error::RuleLimit,
    );
    assert_eq!(
        decode_response(PROFILE, b"\xff\xff\xff\xffE\x01\x04"),
        Err(Error::RuleLimit)
    );
    assert_eq!(
        decode_response(PROFILE, b"\xff\xff\xff\xffE\xff\xff"),
        Err(Error::RuleLimit)
    );
    assert_eq!(
        decode_response(PROFILE, b"\xff\xff\xff\xffE\x01\0\0"),
        Err(Error::Truncated)
    );
}

#[test]
fn nonfinite_durations_are_rejected_on_both_paths_without_partial_output() {
    for (duration, little_endian) in [
        (f32::INFINITY, [0, 0, 0x80, 0x7f]),
        (f32::NEG_INFINITY, [0, 0, 0x80, 0xff]),
        (f32::from_bits(0x7fc0_0000), [0, 0, 0xc0, 0x7f]),
    ] {
        let response = Response::Players(vec![PlayerInfo {
            index: 0,
            name: String::new(),
            score: 0,
            duration_seconds: duration,
        }]);
        assert_rejected(&response, Error::NonFiniteDuration);
        let mut invalid = b"\xff\xff\xff\xffD\x01\0\0\0\0\0\0".to_vec();
        invalid.extend_from_slice(&little_endian);
        assert_eq!(
            decode_response(PROFILE, &invalid),
            Err(Error::NonFiniteDuration)
        );
    }
}

#[test]
fn opaque_population_indicators_and_signed_mod_words_are_preserved() {
    let mut unusual = modified_details();
    unusual.players = 255;
    unusual.max_players = 0;
    unusual.protocol = 0;
    unusual.server_type = 0x91;
    unusual.environment = 0xe3;
    unusual.password_protected = false;
    unusual.secure_server = false;
    let modification = unusual.modification.as_mut().unwrap();
    modification.version = i32::MIN;
    modification.size = -1;
    modification.server_side_only = false;
    modification.custom_dll = true;
    let response = Response::Details(Box::new(unusual));
    assert_eq!(
        decode_response(PROFILE, &encoded_bytes(&response)),
        Ok(response)
    );
}

#[test]
fn logical_response_limit_is_exact_without_claiming_datagram_transport() {
    let maximum = rules_sized(MAX_RESPONSE_BYTES);
    let bytes = encoded_bytes(&maximum);
    assert_eq!(bytes.len(), MAX_RESPONSE_BYTES);
    assert_eq!(decode_response(PROFILE, &bytes), Ok(maximum));
    assert_rejected(&rules_sized(MAX_RESPONSE_BYTES + 1), Error::ResponseLimit);
    let mut over_limit = bytes;
    over_limit.push(0);
    assert_eq!(
        decode_response(PROFILE, &over_limit),
        Err(Error::ResponseLimit)
    );
}

#[test]
fn insufficient_response_capacity_is_atomic_for_all_variants() {
    for response in [
        Response::Details(Box::new(details())),
        Response::Details(Box::new(modified_details())),
        players(),
        rules(),
    ] {
        let expected = encoded_bytes(&response);
        for capacity in [0, expected.len() - 1] {
            let mut output = vec![0xa5; capacity];
            assert_eq!(
                encode_response(PROFILE, &response, &mut output),
                Err(Error::OutputTooSmall)
            );
            assert!(output.iter().all(|byte| *byte == 0xa5));
        }
        assert_exact(&response, &expected);
    }
}

#[test]
fn automatic_debug_and_errors_redact_all_untrusted_values() {
    const MARKER: &str = "SYNTHETIC_PRIVATE_MARKER";
    for response in responses_with_text(MARKER) {
        assert!(!format!("{response:?}").contains(MARKER));
        let mut output = vec![0xa5; MAX_RESPONSE_BYTES];
        let encoded = encode_response(PROFILE, &response, &mut output).unwrap();
        let diagnostic = format!("{encoded:?}");
        assert!(!diagnostic.contains(MARKER));
        assert!(!diagnostic.contains("[255"));
    }
    let mut secret = modified_details();
    secret.name = MARKER.into();
    secret.protocol = 0x91;
    secret.modification.as_mut().unwrap().website = MARKER.into();
    secret.modification.as_mut().unwrap().version = 305_419_896;
    assert_eq!(format!("{secret:?}"), format!("{:?}", details()));
    assert_eq!(
        format!("{:?}", secret.modification.as_ref().unwrap()),
        format!("{:?}", modification())
    );
    let first = decode_response(PROFILE, b"\xff\xff\xff\xff\x91").unwrap_err();
    let second = decode_response(PROFILE, b"\xff\xff\xff\xff\xe3").unwrap_err();
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert_eq!(first.to_string(), second.to_string());
}

#[test]
fn bounded_arbitrary_short_messages_never_panic_or_produce_invalid_results() {
    for length in 0..64 {
        for byte in 0..=255 {
            let mut packet = vec![byte; length];
            if length >= 4 {
                packet[..4].fill(255);
            }
            let _ = decode_request(PROFILE, &packet);
            if let Ok(response) = decode_response(PROFILE, &packet) {
                let bytes = encoded_bytes(&response);
                assert!(bytes.len() <= MAX_RESPONSE_BYTES);
                assert_eq!(decode_response(PROFILE, &bytes), Ok(response));
            }
        }
    }
}
