//! Independently authored synthetic query fixtures, using the public field
//! tables cited by `ohl_net::server`. No game media, captures, SDK/engine source,
//! or other query implementation supplied these bytes or field values.

use ohl_net::{
    ExtraInfo, GoldSrcReassembler, InfoFormat, LegacyInfo, MAX_DATAGRAM_BYTES, MAX_FRAGMENTS,
    MAX_RESPONSE_BYTES, MAX_RULES, MAX_STRING_BYTES, ModInfo, PlayerInfo, Query, Response,
    ServerInfo, ServerRule, SpectatorInfo, decode_response,
    server::{
        QueryChallenge, QueryRequest, ServerCodecError, decode_request, encode_response,
        goldsrc_response_datagrams,
    },
};

const INFO_REQUEST: &[u8] = b"\xff\xff\xff\xffTSource Engine Query\0";

fn info(format: InfoFormat) -> ServerInfo {
    ServerInfo {
        format,
        protocol: 48,
        name: "A".into(),
        map: "B".into(),
        folder: "C".into(),
        game: "D".into(),
        app_id: (format == InfoFormat::Modern).then_some(0x1234),
        players: 2,
        max_players: 8,
        bots: 1,
        server_type: b'd',
        environment: b'l',
        password_protected: true,
        vac: false,
        version: (format == InfoFormat::Modern).then(|| "E".into()),
        legacy: (format == InfoFormat::GoldSrc).then(|| LegacyInfo {
            address: "X".into(),
            modification: None,
        }),
        extra: ExtraInfo::default(),
    }
}

fn encoded_bytes(response: &Response) -> Vec<u8> {
    let mut output = vec![0xa5; MAX_RESPONSE_BYTES];
    let length = encode_response(response, &mut output).unwrap().len();
    output.truncate(length);
    output
}

fn assert_exact(response: &Response, expected: &[u8]) {
    let mut output = vec![0xa5; expected.len() + 8];
    let encoded = encode_response(response, &mut output).unwrap();
    assert_eq!(encoded.as_bytes(), expected);
    assert_eq!(encoded.len(), expected.len());
    assert!(!encoded.is_empty());
    assert_eq!(&output[expected.len()..], &[0xa5; 8]);
    assert_eq!(decode_response(expected).unwrap(), *response);
}

fn assert_rejected(response: &Response, expected: ServerCodecError) {
    let mut output = vec![0xa5; MAX_RESPONSE_BYTES + 8];
    assert_eq!(encode_response(response, &mut output), Err(expected));
    assert!(output.iter().all(|byte| *byte == 0xa5));
}

fn rules_sized(total: usize) -> Response {
    // Published RULES fields: four-byte header, type, two-byte count, then each
    // empty name's terminator and each value's terminator plus its UTF-8 bytes.
    let mut remaining = total - 7;
    let mut rules = Vec::new();
    while remaining > MAX_STRING_BYTES + 2 {
        rules.push(ServerRule {
            name: String::new(),
            value: "x".repeat(MAX_STRING_BYTES),
        });
        remaining -= MAX_STRING_BYTES + 2;
    }
    assert!(remaining >= 2);
    rules.push(ServerRule {
        name: String::new(),
        value: "y".repeat(remaining - 2),
    });
    Response::Rules(rules)
}

#[test]
fn manual_requests_decode_exact_documented_query_variants() {
    assert_eq!(
        decode_request(INFO_REQUEST),
        Ok(QueryRequest::Info { challenge: None })
    );
    let mut challenged_info = INFO_REQUEST.to_vec();
    challenged_info.extend_from_slice(&[0x78, 0x56, 0x34, 0x12]);
    assert_eq!(
        decode_request(&challenged_info),
        Ok(QueryRequest::Info {
            challenge: Some(QueryChallenge::from_i32(0x1234_5678))
        })
    );
    let players = decode_request(b"\xff\xff\xff\xffU\xff\xff\xff\xff").unwrap();
    assert_eq!(
        players,
        QueryRequest::Players {
            challenge: QueryChallenge::initial()
        }
    );
    let rules = decode_request(b"\xff\xff\xff\xffV\xfe\xff\xff\xff").unwrap();
    assert_eq!(
        rules,
        QueryRequest::Rules {
            challenge: QueryChallenge::from_i32(-2)
        }
    );
    assert_eq!(
        decode_request(b"\xff\xff\xff\xffW"),
        Ok(QueryRequest::GetChallenge)
    );
    assert_eq!(players.query(), Some(Query::Players));
    assert_eq!(rules.query(), Some(Query::Rules));
    assert_eq!(
        QueryRequest::Info { challenge: None }.query(),
        Some(Query::Info)
    );
    assert_eq!(QueryRequest::GetChallenge.query(), None);
    assert_eq!(QueryChallenge::initial().as_i32(), -1);
    assert!(QueryChallenge::initial().is_initial());
    assert!(!QueryChallenge::from_i32(0).is_initial());
}

#[test]
fn every_request_truncation_and_unsupported_suffix_is_rejected() {
    for complete in [
        INFO_REQUEST,
        b"\xff\xff\xff\xffU\x78\x56\x34\x12",
        b"\xff\xff\xff\xffV\x78\x56\x34\x12",
        b"\xff\xff\xff\xffW",
    ] {
        for end in 0..complete.len() {
            assert_eq!(
                decode_request(&complete[..end]),
                Err(ServerCodecError::TruncatedRequest)
            );
        }
        let mut suffixed = complete.to_vec();
        // INFO supports one complete challenge; every other request supports
        // no suffix. Five extra bytes cannot form either supported layout.
        suffixed.extend_from_slice(&[1; 5]);
        assert_eq!(
            decode_request(&suffixed),
            Err(ServerCodecError::TrailingRequestData)
        );
    }
    for tail in 1..4 {
        let mut partial_challenge = INFO_REQUEST.to_vec();
        partial_challenge.extend_from_slice(&[1; 3][..tail]);
        assert_eq!(
            decode_request(&partial_challenge),
            Err(ServerCodecError::TruncatedRequest)
        );
    }
}

#[test]
fn request_header_text_type_and_datagram_limits_are_fail_closed() {
    assert_eq!(
        decode_request(b"\xfe\xff\xff\xffT"),
        Err(ServerCodecError::InvalidRequestHeader)
    );
    assert_eq!(
        decode_request(b"\0\0\0\0W"),
        Err(ServerCodecError::InvalidRequestHeader)
    );
    for kind in [0, b'i', b'A', 0x91, 0xe3] {
        assert_eq!(
            decode_request(&[255, 255, 255, 255, kind]),
            Err(ServerCodecError::UnsupportedRequest)
        );
    }
    for index in [5, INFO_REQUEST.len() - 1] {
        let mut wrong_text = INFO_REQUEST.to_vec();
        wrong_text[index] ^= 1;
        assert_eq!(
            decode_request(&wrong_text),
            Err(ServerCodecError::InvalidInfoQuery)
        );
    }
    let mut limit = vec![0; MAX_DATAGRAM_BYTES];
    limit[..5].copy_from_slice(b"\xff\xff\xff\xffW");
    assert_eq!(
        decode_request(&limit),
        Err(ServerCodecError::TrailingRequestData)
    );
    limit.push(0);
    assert_eq!(decode_request(&limit), Err(ServerCodecError::DatagramLimit));
    // Query challenge W is distinct from an unobserved gameplay getchallenge
    // request: the codec never accepts arbitrary command strings.
    assert_eq!(
        decode_request(b"\xff\xff\xff\xffgetchallenge\n"),
        Err(ServerCodecError::UnsupportedRequest)
    );
}

#[test]
fn challenge_player_and_rules_responses_match_manual_wire_vectors() {
    assert_exact(
        &Response::Challenge(0x1234_5678),
        b"\xff\xff\xff\xffA\x78\x56\x34\x12",
    );
    assert_exact(
        &Response::Players(vec![PlayerInfo {
            index: 7,
            name: "A".into(),
            score: -2,
            duration_seconds: 1.5,
        }]),
        b"\xff\xff\xff\xffD\x01\x07A\0\xfe\xff\xff\xff\0\0\xc0\x3f",
    );
    assert_exact(
        &Response::Rules(vec![
            ServerRule {
                name: "k".into(),
                value: "v".into(),
            },
            ServerRule {
                name: "k".into(),
                value: String::new(),
            },
        ]),
        b"\xff\xff\xff\xffE\x02\0k\0v\0k\0\0",
    );
    assert_exact(&Response::Players(Vec::new()), b"\xff\xff\xff\xffD\0");
    assert_exact(&Response::Rules(Vec::new()), b"\xff\xff\xff\xffE\0\0");
}

#[test]
fn modern_info_and_all_edf_fields_match_manual_order() {
    assert_exact(
        &Response::Info(Box::new(info(InfoFormat::Modern))),
        b"\xff\xff\xff\xffI\x30A\0B\0C\0D\0\x34\x12\x02\x08\x01dl\x01\0E\0\0",
    );
    let mut extended = info(InfoFormat::Modern);
    extended.extra = ExtraInfo {
        game_port: Some(0x1234),
        steam_id: Some(0x0102_0304_0506_0708),
        spectator: Some(SpectatorInfo {
            port: 0xabcd,
            name: "S".into(),
        }),
        keywords: Some("K".into()),
        game_id: Some(0x1112_1314_1516_1718),
    };
    assert_exact(
        &Response::Info(Box::new(extended)),
        b"\xff\xff\xff\xffI\x30A\0B\0C\0D\0\x34\x12\x02\x08\x01dl\x01\0E\0\xf1\x34\x12\x08\x07\x06\x05\x04\x03\x02\x01\xcd\xabS\0K\0\x18\x17\x16\x15\x14\x13\x12\x11",
    );
}

#[test]
fn each_optional_edf_field_has_its_own_manual_flag_and_payload() {
    let cases = [
        (
            ExtraInfo {
                game_port: Some(0x1234),
                ..ExtraInfo::default()
            },
            b"\x80\x34\x12".as_slice(),
        ),
        (
            ExtraInfo {
                steam_id: Some(0x0102_0304_0506_0708),
                ..ExtraInfo::default()
            },
            b"\x10\x08\x07\x06\x05\x04\x03\x02\x01".as_slice(),
        ),
        (
            ExtraInfo {
                spectator: Some(SpectatorInfo {
                    port: 0xabcd,
                    name: "S".into(),
                }),
                ..ExtraInfo::default()
            },
            b"\x40\xcd\xabS\0".as_slice(),
        ),
        (
            ExtraInfo {
                keywords: Some("K".into()),
                ..ExtraInfo::default()
            },
            b"\x20K\0".as_slice(),
        ),
        (
            ExtraInfo {
                game_id: Some(0x1112_1314_1516_1718),
                ..ExtraInfo::default()
            },
            b"\x01\x18\x17\x16\x15\x14\x13\x12\x11".as_slice(),
        ),
    ];
    for (extra, suffix) in cases {
        let mut metadata = info(InfoFormat::Modern);
        metadata.extra = extra;
        let mut expected =
            b"\xff\xff\xff\xffI\x30A\0B\0C\0D\0\x34\x12\x02\x08\x01dl\x01\0E\0".to_vec();
        expected.extend_from_slice(suffix);
        assert_exact(&Response::Info(Box::new(metadata)), &expected);
    }
}

#[test]
fn legacy_info_and_mod_block_match_manual_wire_vectors() {
    assert_exact(
        &Response::Info(Box::new(info(InfoFormat::GoldSrc))),
        b"\xff\xff\xff\xffmX\0A\0B\0C\0D\0\x02\x08\x30dl\x01\0\0\x01",
    );
    let mut modified = info(InfoFormat::GoldSrc);
    modified.legacy.as_mut().unwrap().modification = Some(ModInfo {
        website: "W".into(),
        download: "L".into(),
        version: 0x0102_0304,
        size: 0x0506_0708,
        multiplayer_only: true,
        custom_dll: false,
    });
    assert_exact(
        &Response::Info(Box::new(modified)),
        b"\xff\xff\xff\xffmX\0A\0B\0C\0D\0\x02\x08\x30dl\x01\x01W\0L\0\0\x04\x03\x02\x01\x08\x07\x06\x05\x01\0\0\x01",
    );
}

#[test]
fn incompatible_info_fields_cannot_be_silently_dropped() {
    let mut missing_app = info(InfoFormat::Modern);
    missing_app.app_id = None;
    let mut missing_version = info(InfoFormat::Modern);
    missing_version.version = None;
    let mut wrong_legacy = info(InfoFormat::Modern);
    wrong_legacy.legacy = info(InfoFormat::GoldSrc).legacy;
    let mut missing_legacy = info(InfoFormat::GoldSrc);
    missing_legacy.legacy = None;
    let mut wrong_app = info(InfoFormat::GoldSrc);
    wrong_app.app_id = Some(1);
    let mut wrong_version = info(InfoFormat::GoldSrc);
    wrong_version.version = Some(String::new());
    let mut wrong_extra = info(InfoFormat::GoldSrc);
    wrong_extra.extra.game_port = Some(0);
    for incompatible in [
        missing_app,
        missing_version,
        wrong_legacy,
        missing_legacy,
        wrong_app,
        wrong_version,
        wrong_extra,
    ] {
        assert_rejected(
            &Response::Info(Box::new(incompatible)),
            ServerCodecError::IncompatibleInfoFields,
        );
    }
    let mut extension = info(InfoFormat::Modern);
    extension.app_id = Some(2400);
    assert_rejected(
        &Response::Info(Box::new(extension)),
        ServerCodecError::UnsupportedInfoExtension,
    );
}

fn responses_with_string(value: &str) -> Vec<Response> {
    let mut responses = Vec::new();
    for format in [InfoFormat::Modern, InfoFormat::GoldSrc] {
        for position in 0..4 {
            let mut metadata = info(format);
            match position {
                0 => metadata.name = value.into(),
                1 => metadata.map = value.into(),
                2 => metadata.folder = value.into(),
                _ => metadata.game = value.into(),
            }
            responses.push(Response::Info(Box::new(metadata)));
        }
    }
    let mut version = info(InfoFormat::Modern);
    version.version = Some(value.into());
    responses.push(Response::Info(Box::new(version)));
    let mut spectator = info(InfoFormat::Modern);
    spectator.extra.spectator = Some(SpectatorInfo {
        port: 1,
        name: value.into(),
    });
    responses.push(Response::Info(Box::new(spectator)));
    let mut keywords = info(InfoFormat::Modern);
    keywords.extra.keywords = Some(value.into());
    responses.push(Response::Info(Box::new(keywords)));
    let mut address = info(InfoFormat::GoldSrc);
    address.legacy.as_mut().unwrap().address = value.into();
    responses.push(Response::Info(Box::new(address)));
    for website in [false, true] {
        let mut modification = info(InfoFormat::GoldSrc);
        modification.legacy.as_mut().unwrap().modification = Some(ModInfo {
            website: if website { value.into() } else { String::new() },
            download: if website { String::new() } else { value.into() },
            version: 0,
            size: 0,
            multiplayer_only: false,
            custom_dll: true,
        });
        responses.push(Response::Info(Box::new(modification)));
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
fn every_string_position_enforces_nul_and_byte_limits_atomically() {
    for response in responses_with_string("SYNTHETIC\0PRIVATE") {
        assert_rejected(&response, ServerCodecError::EmbeddedNul);
    }
    for response in responses_with_string(&"x".repeat(MAX_STRING_BYTES + 1)) {
        assert_rejected(&response, ServerCodecError::StringLimit);
    }
    for response in responses_with_string(&"x".repeat(MAX_STRING_BYTES)) {
        let encoded = encoded_bytes(&response);
        assert_eq!(decode_response(&encoded).unwrap(), response);
    }
    let unicode = "é".repeat(MAX_STRING_BYTES / 2 + 1);
    assert_rejected(
        &Response::Rules(vec![ServerRule {
            name: unicode,
            value: String::new(),
        }]),
        ServerCodecError::StringLimit,
    );
}

#[test]
fn counts_and_finite_numeric_fields_match_existing_query_model() {
    let player = PlayerInfo {
        index: 255,
        name: String::new(),
        score: i32::MIN,
        duration_seconds: -1.0,
    };
    let maximum = Response::Players(vec![player.clone(); 255]);
    let bytes = encoded_bytes(&maximum);
    assert_eq!(&bytes[..6], b"\xff\xff\xff\xffD\xff");
    assert_eq!(decode_response(&bytes).unwrap(), maximum);
    assert_rejected(
        &Response::Players(vec![player.clone(); 256]),
        ServerCodecError::PlayerLimit,
    );
    for duration in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let invalid = PlayerInfo {
            duration_seconds: duration,
            ..player.clone()
        };
        assert_rejected(
            &Response::Players(vec![player.clone(), invalid]),
            ServerCodecError::NonFiniteDuration,
        );
    }
    let rule = ServerRule {
        name: String::new(),
        value: String::new(),
    };
    let maximum = Response::Rules(vec![rule.clone(); MAX_RULES]);
    assert_eq!(decode_response(&encoded_bytes(&maximum)).unwrap(), maximum);
    assert_rejected(
        &Response::Rules(vec![rule; MAX_RULES + 1]),
        ServerCodecError::RuleLimit,
    );
    let mut unusual = info(InfoFormat::Modern);
    unusual.players = 255;
    unusual.max_players = 0;
    unusual.bots = 254;
    unusual.server_type = 0xf3;
    unusual.environment = 0xe2;
    let response = Response::Info(Box::new(unusual));
    assert_eq!(
        decode_response(&encoded_bytes(&response)).unwrap(),
        response
    );
}

#[test]
fn cumulative_logical_bound_is_exact_and_preserves_output_on_overflow() {
    let maximum = rules_sized(MAX_RESPONSE_BYTES);
    let bytes = encoded_bytes(&maximum);
    assert_eq!(bytes.len(), MAX_RESPONSE_BYTES);
    assert_eq!(decode_response(&bytes).unwrap(), maximum);
    assert_rejected(
        &rules_sized(MAX_RESPONSE_BYTES + 1),
        ServerCodecError::ResponseLimit,
    );
}

#[test]
fn output_capacity_errors_are_atomic_for_every_response_variant() {
    for response in [
        Response::Challenge(-2),
        Response::Info(Box::new(info(InfoFormat::Modern))),
        Response::Info(Box::new(info(InfoFormat::GoldSrc))),
        Response::Players(Vec::new()),
        Response::Rules(Vec::new()),
    ] {
        let expected = encoded_bytes(&response);
        for capacity in [0, expected.len() - 1] {
            let mut output = vec![0xa5; capacity];
            assert_eq!(
                encode_response(&response, &mut output),
                Err(ServerCodecError::OutputTooSmall)
            );
            assert!(output.iter().all(|byte| *byte == 0xa5));
        }
        assert_exact(&response, &expected);
    }
}

#[test]
fn split_boundary_and_manual_header_fields_are_exact() {
    let mut storage = vec![0xa5; MAX_RESPONSE_BYTES];
    let boundary_response = rules_sized(MAX_DATAGRAM_BYTES);
    let boundary = encode_response(&boundary_response, &mut storage).unwrap();
    let datagrams = goldsrc_response_datagrams(boundary, 0x1234_5678).unwrap();
    assert_eq!(datagrams.len(), 1);
    assert!(!datagrams.is_empty());
    assert_eq!(datagrams.datagrams()[0].as_bytes(), boundary.as_bytes());

    let split_response = rules_sized(MAX_DATAGRAM_BYTES + 1);
    let encoded = encode_response(&split_response, &mut storage).unwrap();
    let split = goldsrc_response_datagrams(encoded, 0x1234_5678).unwrap();
    assert_eq!(split.len(), 2);
    let first = split.datagrams()[0].as_bytes();
    let second = split.datagrams()[1].as_bytes();
    assert_eq!(&first[..9], b"\xfe\xff\xff\xff\x78\x56\x34\x12\x02");
    assert_eq!(&second[..9], b"\xfe\xff\xff\xff\x78\x56\x34\x12\x12");
    assert_eq!(first.len(), 1400);
    assert_eq!(second.len(), 19);
    assert_eq!(&first[9..], &encoded.as_bytes()[..1391]);
    assert_eq!(&second[9..], &encoded.as_bytes()[1391..]);
    assert_eq!(&first[9..14], b"\xff\xff\xff\xffE");
}

#[test]
fn split_packet_budget_rejects_overflow_instead_of_truncating() {
    // Fifteen packets each have a nine-byte split header inside the datagram
    // budget. This is intentionally smaller than the logical codec's 64 KiB.
    let maximum = MAX_FRAGMENTS * (MAX_DATAGRAM_BYTES - 9);
    let mut storage = vec![0xa5; MAX_RESPONSE_BYTES];
    let response = rules_sized(maximum);
    let encoded = encode_response(&response, &mut storage).unwrap();
    let split = goldsrc_response_datagrams(encoded, 0x7fff_ffff).unwrap();
    assert_eq!(split.len(), 15);
    for (index, datagram) in split.datagrams().iter().enumerate() {
        assert_eq!(datagram.as_bytes().len(), MAX_DATAGRAM_BYTES);
        assert_eq!(usize::from(datagram.as_bytes()[8]), (index << 4) | 15);
    }
    for length in [maximum + 1, MAX_RESPONSE_BYTES] {
        let response = rules_sized(length);
        let encoded = encode_response(&response, &mut storage).unwrap();
        assert_eq!(
            goldsrc_response_datagrams(encoded, 0),
            Err(ServerCodecError::FragmentLimit)
        );
        assert_eq!(encoded.len(), length);
    }
    let encoded = encode_response(&Response::Challenge(0), &mut storage).unwrap();
    for id in [0x8000_0000, u32::MAX] {
        assert_eq!(
            goldsrc_response_datagrams(encoded, id),
            Err(ServerCodecError::UnsupportedSplitIdentifier)
        );
    }
}

#[test]
fn emitted_split_responses_survive_reordering_and_duplicates_in_client() {
    let response = rules_sized(8000);
    let mut output = vec![0xa5; MAX_RESPONSE_BYTES];
    let encoded = encode_response(&response, &mut output).unwrap();
    let split = goldsrc_response_datagrams(encoded, 17).unwrap();
    let mut client = GoldSrcReassembler::new();
    assert_eq!(
        client
            .push(split.datagrams().last().unwrap().as_bytes())
            .unwrap(),
        None
    );
    assert_eq!(
        client
            .push(split.datagrams().last().unwrap().as_bytes())
            .unwrap(),
        None
    );
    for datagram in split.datagrams()[1..split.len() - 1].iter().rev() {
        assert_eq!(client.push(datagram.as_bytes()).unwrap(), None);
    }
    let complete = client
        .push(split.datagrams()[0].as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!(complete, encoded.as_bytes());
    assert_eq!(decode_response(&complete).unwrap(), response);
}

#[test]
fn automatic_diagnostics_redact_tokens_packet_data_and_identifiers() {
    let token_a = QueryChallenge::from_i32(0x1234_5678);
    let token_b = QueryChallenge::from_i32(0x1122_3344);
    assert_eq!(format!("{token_a:?}"), format!("{token_b:?}"));
    for request in [
        QueryRequest::Info {
            challenge: Some(token_a),
        },
        QueryRequest::Players { challenge: token_a },
        QueryRequest::Rules { challenge: token_a },
    ] {
        assert!(!format!("{request:?}").contains("305419896"));
    }
    let response = Response::Rules(vec![ServerRule {
        name: "SYNTHETIC_PRIVATE_MARKER".into(),
        value: "PRIVATE_VALUE".repeat(150),
    }]);
    let mut output = vec![0xa5; MAX_RESPONSE_BYTES];
    let encoded = encode_response(&response, &mut output).unwrap();
    let datagrams = goldsrc_response_datagrams(encoded, 0x1234_5678).unwrap();
    assert!(datagrams.len() > 1);
    for diagnostic in [
        format!("{encoded:?}"),
        format!("{datagrams:?}"),
        format!("{:?}", datagrams.datagrams()[0]),
    ] {
        assert!(!diagnostic.contains("SYNTHETIC_PRIVATE_MARKER"));
        assert!(!diagnostic.contains("PRIVATE_VALUE"));
        assert!(!diagnostic.contains("305419896"));
        assert!(!diagnostic.contains("[255"));
        assert!(!diagnostic.contains("[254"));
    }
    let first = decode_request(b"\xff\xff\xff\xff\x91").unwrap_err();
    let second = decode_request(b"\xff\xff\xff\xff\xe3").unwrap_err();
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert_eq!(first.to_string(), second.to_string());
}
