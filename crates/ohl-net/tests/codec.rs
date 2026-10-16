//! Fixtures are independently authored synthetic metadata, never game captures.

use ohl_net::{
    Error, GoldSrcReassembler, InfoFormat, MAX_DATAGRAM_BYTES, MAX_RESPONSE_BYTES, MAX_RULES,
    MAX_STRING_BYTES, Query, Response, decode_response, encode_request,
};

fn modern() -> Vec<u8> {
    let mut packet =
        b"\xff\xff\xff\xffI\x30Synthetic server\0arena\0testgame\0Test Game\0".to_vec();
    packet.extend_from_slice(&70_u16.to_le_bytes());
    packet.extend_from_slice(&[2, 8, 1, b'd', b'l', 1, 0]);
    packet.extend_from_slice(b"test-version\0");
    packet
}

fn fragment(id: u32, numbering: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = b"\xfe\xff\xff\xff".to_vec();
    packet.extend_from_slice(&id.to_le_bytes());
    packet.push(numbering);
    packet.extend_from_slice(payload);
    packet
}

#[test]
fn original_request_bytes_and_challenge_signedness_are_exact() {
    assert_eq!(
        encode_request(Query::Info, None),
        b"\xff\xff\xff\xffTSource Engine Query\0"
    );
    assert_eq!(
        encode_request(Query::Players, None),
        b"\xff\xff\xff\xffU\xff\xff\xff\xff"
    );
    assert_eq!(
        encode_request(Query::Rules, Some(0x1234_5678)),
        b"\xff\xff\xff\xffV\x78\x56\x34\x12"
    );
    let challenge = b"\xff\xff\xff\xffA\xff\xff\xff\x80";
    assert!(matches!(
        decode_response(challenge),
        Ok(Response::Challenge(-2_130_706_433))
    ));
    let request = encode_request(Query::Info, Some(-2_130_706_433));
    assert_eq!(&request[request.len() - 4..], &challenge[5..]);
}

#[test]
fn modern_info_reads_all_extra_fields_in_protocol_order() {
    let mut packet = modern();
    packet.push(0xf1);
    packet.extend_from_slice(&28000_u16.to_le_bytes());
    packet.extend_from_slice(&0x8877_6655_4433_2211_u64.to_le_bytes());
    packet.extend_from_slice(&28001_u16.to_le_bytes());
    packet.extend_from_slice(b"Synthetic spectator\0test,synthetic\0");
    packet.extend_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
    let Response::Info(info) = decode_response(&packet).unwrap() else {
        panic!()
    };
    assert_eq!(info.format, InfoFormat::Modern);
    assert_eq!(info.protocol, 48);
    assert_eq!(info.app_id, Some(70));
    assert_eq!(info.name, "Synthetic server");
    assert_eq!(info.map, "arena");
    assert_eq!(info.folder, "testgame");
    assert_eq!(info.game, "Test Game");
    assert_eq!((info.players, info.max_players, info.bots), (2, 8, 1));
    assert!(info.password_protected);
    assert!(!info.vac);
    assert_eq!(info.version.as_deref(), Some("test-version"));
    assert!(info.legacy.is_none());
    assert_eq!(info.extra.game_port, Some(28000));
    assert_eq!(info.extra.steam_id, Some(0x8877_6655_4433_2211));
    let spectator = info.extra.spectator.unwrap();
    assert_eq!(spectator.port, 28001);
    assert_eq!(spectator.name, "Synthetic spectator");
    assert_eq!(info.extra.keywords.as_deref(), Some("test,synthetic"));
    assert_eq!(info.extra.game_id, Some(0x0123_4567_89ab_cdef));
}

#[test]
fn optional_edf_can_be_absent_or_zero() {
    let Response::Info(before) = decode_response(&modern()).unwrap() else {
        panic!()
    };
    let mut with_zero = modern();
    with_zero.push(0);
    let Response::Info(after) = decode_response(&with_zero).unwrap() else {
        panic!()
    };
    assert_eq!(before, after);
}

#[test]
fn legacy_info_reads_mod_block_and_preserves_claimed_address_as_data() {
    let mut packet =
        b"\xff\xff\xff\xffm192.0.2.9:27015\0Synthetic server\0arena\0testgame\0Test Game\0"
            .to_vec();
    packet.extend_from_slice(&[3, 16, 47, b'D', b'W', 0, 1]);
    packet.extend_from_slice(b"https://example.invalid/\0\0\0");
    packet.extend_from_slice(&9_u32.to_le_bytes());
    packet.extend_from_slice(&12345_u32.to_le_bytes());
    packet.extend_from_slice(&[1, 1, 1, 2]);
    let Response::Info(info) = decode_response(&packet).unwrap() else {
        panic!()
    };
    assert_eq!(info.format, InfoFormat::GoldSrc);
    assert_eq!(info.protocol, 47);
    assert_eq!(info.app_id, None);
    assert_eq!(info.bots, 2);
    assert!(info.vac);
    assert_eq!(info.server_type, b'D');
    let legacy = info.legacy.unwrap();
    assert_eq!(legacy.address, "192.0.2.9:27015");
    let modification = legacy.modification.unwrap();
    assert_eq!(modification.website, "https://example.invalid/");
    assert!(modification.download.is_empty());
    assert_eq!((modification.version, modification.size), (9, 12345));
    assert!(modification.multiplayer_only && modification.custom_dll);
}

#[test]
fn legacy_info_without_mod_still_consumes_vac_and_bots() {
    let mut packet = b"\xff\xff\xff\xffm\0Synthetic\0arena\0testgame\0Test\0".to_vec();
    packet.extend_from_slice(&[0, 8, 48, b'd', b'l', 0, 0, 0, 0]);
    let Response::Info(info) = decode_response(&packet).unwrap() else {
        panic!()
    };
    assert!(info.legacy.unwrap().modification.is_none());
}

#[test]
fn player_scores_and_durations_are_little_endian_and_indices_may_repeat() {
    let mut packet = b"\xff\xff\xff\xffD\x02\0Synthetic A\0".to_vec();
    packet.extend_from_slice(&(-7_i32).to_le_bytes());
    packet.extend_from_slice(&12.5_f32.to_le_bytes());
    packet.extend_from_slice(b"\0Synthetic B\0");
    packet.extend_from_slice(&99_i32.to_le_bytes());
    packet.extend_from_slice(&(-1.0_f32).to_le_bytes());
    let Response::Players(players) = decode_response(&packet).unwrap() else {
        panic!()
    };
    assert_eq!(players.len(), 2);
    assert_eq!(players[0].index, players[1].index);
    assert_eq!(players[0].score, -7);
    assert_eq!(players[0].duration_seconds.to_bits(), 12.5_f32.to_bits());
    assert_eq!(players[1].duration_seconds.to_bits(), (-1.0_f32).to_bits());
}

#[test]
fn ordered_rules_preserve_duplicates_and_empty_values() {
    let packet = b"\xff\xff\xff\xffE\x03\0setting\0first\0setting\0second\0empty\0\0";
    let Response::Rules(rules) = decode_response(packet).unwrap() else {
        panic!()
    };
    assert_eq!(rules.len(), 3);
    assert_eq!(rules[0].name, rules[1].name);
    assert_eq!(rules[0].value, "first");
    assert_eq!(rules[1].value, "second");
    assert!(rules[2].value.is_empty());
}

#[test]
fn every_truncated_prefix_of_a_fixed_info_and_challenge_is_rejected() {
    for packet in [modern(), b"\xff\xff\xff\xffA\x12\x34\x56\x78".to_vec()] {
        for length in 0..packet.len() {
            assert!(
                decode_response(&packet[..length]).is_err(),
                "accepted prefix {length}"
            );
        }
        assert!(decode_response(&packet).is_ok());
    }
}

#[test]
fn strings_counts_flags_and_trailing_fields_are_bounded() {
    let mut oversized_name = b"\xff\xff\xff\xffI\x30".to_vec();
    oversized_name.extend(std::iter::repeat_n(b'x', MAX_STRING_BYTES + 1));
    oversized_name.push(0);
    assert!(matches!(
        decode_response(&oversized_name),
        Err(Error::Limit("string bytes"))
    ));

    let mut invalid_utf8 = modern();
    invalid_utf8[6] = 0xff;
    assert!(matches!(
        decode_response(&invalid_utf8),
        Err(Error::Invalid("UTF-8 string"))
    ));

    let mut flags = modern();
    flags.push(2);
    assert!(matches!(
        decode_response(&flags),
        Err(Error::Unsupported("extra-data flags"))
    ));
    flags.pop();
    flags.extend_from_slice(&[0, 1]);
    assert!(matches!(
        decode_response(&flags),
        Err(Error::Unsupported("trailing response fields"))
    ));

    let mut rules = b"\xff\xff\xff\xffE".to_vec();
    rules.extend_from_slice(&u16::try_from(MAX_RULES + 1).unwrap().to_le_bytes());
    assert!(matches!(
        decode_response(&rules),
        Err(Error::Limit("rule count"))
    ));
    assert!(matches!(
        decode_response(b"\xff\xff\xff\xffD\xff"),
        Err(Error::Truncated)
    ));
    assert!(matches!(
        decode_response(&vec![0; MAX_RESPONSE_BYTES + 1]),
        Err(Error::Limit("response bytes"))
    ));
}

#[test]
fn noncanonical_booleans_and_nonfinite_player_durations_are_rejected() {
    let mut packet = modern();
    let password = packet.len() - b"test-version\0".len() - 2;
    packet[password] = 3;
    assert!(matches!(
        decode_response(&packet),
        Err(Error::Invalid("boolean byte"))
    ));
    let mut player = b"\xff\xff\xff\xffD\x01\0\0".to_vec();
    player.extend_from_slice(&0_i32.to_le_bytes());
    player.extend_from_slice(&f32::NAN.to_le_bytes());
    assert!(matches!(
        decode_response(&player),
        Err(Error::Invalid("non-finite player duration"))
    ));
}

#[test]
fn split_reassembly_handles_reordering_and_identical_duplicates() {
    let full = modern();
    let cuts = [0, 16, 32, full.len()];
    let parts: Vec<_> = (0_u8..3)
        .map(|index| {
            fragment(
                123,
                (index << 4) | 3,
                &full[cuts[usize::from(index)]..cuts[usize::from(index) + 1]],
            )
        })
        .collect();
    let mut assembler = GoldSrcReassembler::new();
    assert!(assembler.push(&parts[2]).unwrap().is_none());
    assert!(assembler.push(&parts[2]).unwrap().is_none());
    assert!(assembler.push(&parts[0]).unwrap().is_none());
    assert_eq!(assembler.push(&parts[1]).unwrap().unwrap(), full);
}

#[test]
fn split_rejects_invalid_headers_conflicts_id_changes_and_compression() {
    let full = modern();
    for packed in [0, 1, 0x22, 0xf2] {
        assert!(
            GoldSrcReassembler::new()
                .push(&fragment(1, packed, &full[..16]))
                .is_err()
        );
    }
    let mut assembler = GoldSrcReassembler::new();
    assert!(
        assembler
            .push(&fragment(1, 2, &full[..16]))
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        assembler.push(&fragment(2, 0x12, &full[16..])),
        Err(Error::Invalid("split response identity changed"))
    ));
    assert!(
        assembler
            .push(&fragment(1, 2, &full[..16]))
            .unwrap()
            .is_none()
    );
    let mut corrupt = full[..16].to_vec();
    corrupt[15] ^= 1;
    assert!(matches!(
        assembler.push(&fragment(1, 2, &corrupt)),
        Err(Error::Invalid("conflicting duplicate fragment"))
    ));
    assert!(matches!(
        assembler.push(&fragment(0x8000_0001, 2, &full[..16])),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        assembler.push(&fragment(1, 2, b"\0\0\xe0\x04\xff\xff\xff\xffI")),
        Err(Error::Unsupported("ambiguous split response framing"))
    ));
    assert!(matches!(
        assembler.push(&vec![255; MAX_DATAGRAM_BYTES + 1]),
        Err(Error::Limit("datagram bytes"))
    ));
}

#[test]
fn split_errors_clear_pending_state_and_single_packet_replaces_it() {
    let full = modern();
    let mut assembler = GoldSrcReassembler::new();
    assert!(
        assembler
            .push(&fragment(1, 2, &full[..16]))
            .unwrap()
            .is_none()
    );
    assert!(assembler.push(&fragment(1, 3, &full[..16])).is_err());
    assert!(
        assembler
            .push(&fragment(2, 0x12, &full[16..]))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        assembler
            .push(&fragment(2, 2, &full[..16]))
            .unwrap()
            .unwrap(),
        full
    );
    assert!(
        assembler
            .push(&fragment(3, 2, &full[..16]))
            .unwrap()
            .is_none()
    );
    assert_eq!(assembler.push(&full).unwrap().unwrap(), full);
    assembler.reset();
    assert!(
        assembler
            .push(&fragment(4, 0x12, &full[16..]))
            .unwrap()
            .is_none()
    );
}

#[test]
fn arbitrary_short_inputs_never_panic_or_grow_without_bounds() {
    for length in 0..64 {
        for byte in [0, 1, 65, 254, 255] {
            let packet = vec![byte; length];
            let _ = decode_response(&packet);
            let _ = GoldSrcReassembler::new().push(&packet);
        }
    }
}
