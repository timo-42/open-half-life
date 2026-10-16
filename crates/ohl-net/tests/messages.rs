//! Independently authored synthetic fixtures for published svc field tables.
//!
//! These bytes are constructed here from scalar constants and invented text.
//! No fixture is a network capture, game file, extracted identifier, SDK code,
//! or another engine's implementation. All directory/map/command strings below
//! are project-authored synthetic text and are never opened or executed.

use ohl_net::message::{
    ByteModeProfile, MAX_MAPCYCLE_BYTES, MAX_MESSAGES, MAX_PAYLOAD_BYTES, MAX_TEXT_BYTES,
    MessageErrorKind, MessageReader, ServerMessage,
};

const PROFILE: ByteModeProfile = ByteModeProfile::InferredLittleEndianNulStrings;

fn reader(payload: &[u8]) -> MessageReader<'_> {
    MessageReader::new(payload, PROFILE).unwrap()
}

fn string_message(opcode: u8, text: &[u8]) -> Vec<u8> {
    let mut bytes = vec![opcode];
    bytes.extend_from_slice(text);
    bytes.push(0);
    bytes
}

fn server_info(mapcycle: &[u8]) -> Vec<u8> {
    // Table order: 3 longs, 16-byte hash, 3 bytes, 4 strings, reserved zero.
    // The values are deliberately independent of the current engine/server.
    let mut bytes = vec![11];
    bytes.extend_from_slice(&[0x30, 0x00, 0x00, 0x00]);
    bytes.extend_from_slice(&[0x02, 0x01, 0x00, 0x00]);
    bytes.extend_from_slice(&[0x78, 0x56, 0x34, 0x92]);
    bytes.extend_from_slice(&[0xA5; 16]);
    bytes.extend_from_slice(&[32, 7, 1]);
    for text in [
        b"synthetic-directory".as_slice(),
        b"Synthetic server".as_slice(),
        b"synthetic-map".as_slice(),
        mapcycle,
    ] {
        bytes.extend_from_slice(text);
        bytes.push(0);
    }
    bytes.push(0);
    bytes
}

fn examples() -> Vec<Vec<u8>> {
    let mut extra_info = string_message(54, b"synthetic-fallback");
    extra_info.push(1);
    vec![
        vec![1],
        string_message(2, b"Synthetic disconnect reason"),
        vec![4, 0x34, 0x12, 0, 0],
        vec![5, 0x34, 0x12],
        vec![7, 0x01, 0x23, 0x45, 0x67],
        string_message(8, b"Synthetic console text"),
        string_message(9, b"synthetic_command; synthetic_value\n"),
        vec![10, 0x34, 0x12, 0, 0x80, 0xFF, 0xFF],
        server_info(b"Synthetic rotation\n"),
        vec![24, 0],
        vec![24, 1],
        vec![25, 1],
        string_message(26, b"Synthetic centered text"),
        vec![30],
        vec![42],
        extra_info,
    ]
}

#[test]
fn documented_messages_preserve_stream_boundaries() {
    let examples = examples();
    let bytes: Vec<u8> = examples.iter().flatten().copied().collect();
    let mut messages = reader(&bytes);
    let mut expected_position = 0;
    for example in examples {
        let message = messages.next().unwrap().unwrap();
        expected_position += example.len();
        assert_eq!(message.opcode(), example[0]);
        assert_eq!(messages.position(), expected_position);
    }
    assert_eq!(messages.message_count(), 16);
    assert_eq!(messages.position(), bytes.len());
    assert_eq!(messages.profile(), PROFILE);
    assert!(messages.next().is_none());
    assert!(messages.next().is_none());
}

#[test]
fn selected_profile_preserves_raw_float_and_decodes_integer_words() {
    let bytes = [
        4, 0xFE, 0xFF, 0xFF, 0xFF, // long -2
        5, 0x00, 0x80, // short -32768
        10, 0x34, 0x12, 0, 0x80, 0xFF, 0xFF, // raw angle shorts
        7, 0xD1, 0xE2, 0xF3, 0x84, // raw float word, no interpretation
    ];
    let mut messages = reader(&bytes);
    assert_eq!(
        messages.next().unwrap().unwrap(),
        ServerMessage::Version { protocol: -2 }
    );
    assert_eq!(
        messages.next().unwrap().unwrap(),
        ServerMessage::SetView {
            entity_index: i16::MIN
        }
    );
    assert_eq!(
        messages.next().unwrap().unwrap(),
        ServerMessage::SetAngle {
            angles: [0x1234, i16::MIN, -1]
        }
    );
    let ServerMessage::Time { time } = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic time message");
    };
    assert_eq!(time.as_bytes(), &[0xD1, 0xE2, 0xF3, 0x84]);
    assert!(messages.next().is_none());
}

#[test]
fn strings_are_borrowed_opaque_bytes_and_commands_remain_data() {
    let bytes = [
        2, 0, // empty disconnect reason
        8, 0xFF, 0xFE, b'\n', 0, // non-UTF-8 console text
        9, b'a', b';', b'b', b'\n', 0, // command syntax is merely bytes
        26, 0, // empty center print
        54, 0, 0xF3, // empty fallback and unvalidated cheat byte
    ];
    let mut messages = reader(&bytes);
    let ServerMessage::Disconnect { reason } = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic disconnect message");
    };
    assert!(reason.is_empty());
    assert_eq!(reason.len(), 0);
    let ServerMessage::Print { text } = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic print message");
    };
    assert_eq!(text.as_bytes(), &[0xFF, 0xFE, b'\n']);
    assert_eq!(text.as_bytes().as_ptr(), bytes[3..].as_ptr());
    let ServerMessage::StuffText { command } = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic stufftext message");
    };
    assert_eq!(command.as_bytes(), b"a;b\n");
    let ServerMessage::CenterPrint { text } = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic centerprint message");
    };
    assert!(text.is_empty());
    let ServerMessage::SendExtraInfo {
        fallback_directory,
        can_cheat,
    } = messages.next().unwrap().unwrap()
    else {
        panic!("expected synthetic extra-info message");
    };
    assert!(fallback_directory.is_empty());
    assert_eq!(can_cheat, 0xF3);
    assert!(messages.next().is_none());
}

#[test]
fn server_info_uses_the_declared_field_order_and_preserves_hash() {
    let bytes = server_info(b"Synthetic rotation\n");
    let mut messages = reader(&bytes);
    let ServerMessage::ServerInfo(info) = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic server-info message");
    };
    assert_eq!(info.protocol, 48);
    assert_eq!(info.spawn_count, 258);
    assert_eq!(info.map_crc.to_le_bytes(), [0x78, 0x56, 0x34, 0x92]);
    assert_eq!(info.client_dll_hash.as_bytes(), &[0xA5; 16]);
    assert_eq!(info.max_players, 32);
    assert_eq!(info.player_index, 7);
    assert_eq!(info.is_deathmatch, 1);
    assert_eq!(info.game_dir.as_bytes(), b"synthetic-directory");
    assert_eq!(info.hostname.as_bytes(), b"Synthetic server");
    assert_eq!(info.map_file_name.as_bytes(), b"synthetic-map");
    assert_eq!(info.mapcycle.as_bytes(), b"Synthetic rotation\n");
    assert!(messages.next().is_none());
}

#[test]
fn every_incomplete_message_prefix_is_rejected_and_reader_is_fused() {
    for bytes in examples() {
        for length in 1..bytes.len() {
            let mut messages = reader(&bytes[..length]);
            let error = messages.next().unwrap().unwrap_err();
            assert_eq!(error.kind, MessageErrorKind::Truncated);
            assert_eq!(error.opcode, Some(bytes[0]));
            assert!(error.offset <= length);
            assert_eq!(messages.position(), 0);
            assert_eq!(messages.message_count(), 0);
            assert!(messages.next().is_none());
            assert!(messages.next().is_none());
        }
    }
    assert!(reader(&[]).next().is_none());
}

#[test]
fn unsupported_messages_stop_at_the_opcode_without_resynchronizing() {
    // Byte 1 after each unsupported opcode resembles NOP but is never consumed.
    for opcode in 0..=u8::MAX {
        if [1, 2, 4, 5, 7, 8, 9, 10, 11, 24, 25, 26, 30, 42, 54].contains(&opcode) {
            continue;
        }
        let bytes = [1, opcode, 1, 1, 1, 1, 1];
        let mut messages = reader(&bytes);
        assert_eq!(messages.next().unwrap().unwrap(), ServerMessage::Nop);
        let error = messages.next().unwrap().unwrap_err();
        assert_eq!(
            error.kind,
            if opcode == 39 {
                MessageErrorKind::UnsupportedLayout
            } else {
                MessageErrorKind::UnsupportedOpcode
            }
        );
        assert_eq!(error.offset, 1);
        assert_eq!(error.opcode, Some(opcode));
        assert_eq!(messages.position(), 1);
        assert_eq!(messages.message_count(), 1);
        assert!(messages.next().is_none());
    }
}

#[test]
fn malformed_message_after_a_valid_prefix_reports_absolute_offset() {
    let bytes = [1, 4, 0x30, 0, 0, 0, 5, 0x12];
    let mut messages = reader(&bytes);
    assert_eq!(messages.next().unwrap().unwrap(), ServerMessage::Nop);
    assert_eq!(
        messages.next().unwrap().unwrap(),
        ServerMessage::Version { protocol: 48 }
    );
    let error = messages.next().unwrap().unwrap_err();
    assert_eq!(error.kind, MessageErrorKind::Truncated);
    assert_eq!(error.opcode, Some(5));
    assert_eq!(error.offset, 7);
    assert_eq!(messages.position(), 6);
    assert_eq!(messages.message_count(), 2);
    assert!(messages.next().is_none());
}

#[test]
fn only_published_pause_and_active_signon_values_are_supported() {
    for value in 0..=u8::MAX {
        let bytes = [25, value, 1];
        let mut messages = reader(&bytes);
        if value == 1 {
            assert_eq!(
                messages.next().unwrap().unwrap(),
                ServerMessage::SignOnActive
            );
            assert_eq!(messages.next().unwrap().unwrap(), ServerMessage::Nop);
        } else {
            let error = messages.next().unwrap().unwrap_err();
            assert_eq!(error.kind, MessageErrorKind::UnsupportedLayout);
            assert_eq!(error.offset, 1);
            assert_eq!(messages.position(), 0);
            assert!(messages.next().is_none());
        }
        let bytes = [24, value, 1];
        let mut messages = reader(&bytes);
        if value <= 1 {
            assert_eq!(
                messages.next().unwrap().unwrap(),
                ServerMessage::SetPause { paused: value == 1 }
            );
        } else {
            let error = messages.next().unwrap().unwrap_err();
            assert_eq!(error.kind, MessageErrorKind::InvalidPause);
            assert_eq!(error.offset, 1);
            assert!(messages.next().is_none());
        }
    }
}

#[test]
fn server_info_requires_reserved_zero_without_guessing_optional_fields() {
    let mut bytes = server_info(b"");
    let reserved_offset = bytes.len() - 1;
    bytes[reserved_offset] = 1;
    bytes.extend_from_slice(&[1, 1, 1]);
    let mut messages = reader(&bytes);
    let error = messages.next().unwrap().unwrap_err();
    assert_eq!(error.kind, MessageErrorKind::NonZeroReservedByte);
    assert_eq!(error.offset, reserved_offset);
    assert_eq!(error.opcode, Some(11));
    assert_eq!(messages.position(), 0);
    assert!(messages.next().is_none());
}

#[test]
fn every_text_field_enforces_its_limit_and_requires_a_terminator() {
    for opcode in [2, 8, 9, 26, 54] {
        let text = vec![b'x'; MAX_TEXT_BYTES];
        let mut bytes = string_message(opcode, &text);
        if opcode == 54 {
            bytes.push(0);
        }
        assert!(reader(&bytes).next().unwrap().is_ok());

        let bytes = string_message(opcode, &vec![b'x'; MAX_TEXT_BYTES + 1]);
        let mut messages = reader(&bytes);
        let error = messages.next().unwrap().unwrap_err();
        assert_eq!(error.kind, MessageErrorKind::TextLimit);
        assert_eq!(error.offset, 1);
        assert!(messages.next().is_none());

        let mut bytes = vec![opcode];
        bytes.extend_from_slice(&text);
        let error = reader(&bytes).next().unwrap().unwrap_err();
        assert_eq!(error.kind, MessageErrorKind::Truncated);
    }

    // Exercise all three regular server-info text fields independently.
    for field in 0..3 {
        let mut bytes = server_info(b"");
        let mut offset = 32; // opcode, 3 longs, hash, 3 bytes
        for _ in 0..field {
            offset += bytes[offset..].iter().position(|byte| *byte == 0).unwrap() + 1;
        }
        let original_length = bytes[offset..].iter().position(|byte| *byte == 0).unwrap();
        bytes.splice(offset..offset + original_length, vec![b'x'; MAX_TEXT_BYTES]);
        assert!(reader(&bytes).next().unwrap().is_ok());
        bytes.insert(offset, b'x');
        let error = reader(&bytes).next().unwrap().unwrap_err();
        assert_eq!(error.kind, MessageErrorKind::TextLimit);
        assert_eq!(error.offset, offset);
    }
}

#[test]
fn mapcycle_has_a_separate_eight_kib_project_bound() {
    let bytes = server_info(&vec![b'x'; MAX_MAPCYCLE_BYTES]);
    let mut messages = reader(&bytes);
    let ServerMessage::ServerInfo(info) = messages.next().unwrap().unwrap() else {
        panic!("expected synthetic server-info message");
    };
    assert_eq!(info.mapcycle.len(), MAX_MAPCYCLE_BYTES);

    let bytes = server_info(&vec![b'x'; MAX_MAPCYCLE_BYTES + 1]);
    let error = reader(&bytes).next().unwrap().unwrap_err();
    assert_eq!(error.kind, MessageErrorKind::TextLimit);
    assert_eq!(error.opcode, Some(11));
}

#[test]
fn payload_and_message_budgets_are_separate_and_fail_closed() {
    let bytes = vec![1; MAX_PAYLOAD_BYTES];
    assert!(MessageReader::new(&bytes, PROFILE).is_ok());
    let bytes = vec![1; MAX_PAYLOAD_BYTES + 1];
    let error = MessageReader::new(&bytes, PROFILE).unwrap_err();
    assert_eq!(error.kind, MessageErrorKind::PayloadLimit);
    assert_eq!(error.offset, 0);
    assert_eq!(error.opcode, None);

    let bytes = vec![1; MAX_MESSAGES];
    let mut messages = reader(&bytes);
    assert_eq!(
        messages
            .by_ref()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len(),
        MAX_MESSAGES
    );
    assert_eq!(messages.position(), MAX_MESSAGES);
    assert!(messages.next().is_none());

    let bytes = vec![1; MAX_MESSAGES + 1];
    let mut messages = reader(&bytes);
    for _ in 0..MAX_MESSAGES {
        assert_eq!(messages.next().unwrap().unwrap(), ServerMessage::Nop);
    }
    let error = messages.next().unwrap().unwrap_err();
    assert_eq!(error.kind, MessageErrorKind::MessageLimit);
    assert_eq!(error.offset, MAX_MESSAGES);
    assert_eq!(error.opcode, Some(1));
    assert_eq!(messages.position(), MAX_MESSAGES);
    assert_eq!(messages.message_count(), MAX_MESSAGES);
    assert!(messages.next().is_none());
}

#[test]
fn server_info_debug_redacts_crc_while_explicit_access_preserves_it() {
    // Independently chosen synthetic scalar, unrelated to any original asset.
    const CRC_MARKER: i32 = 0x1357_9BDF;
    let mut bytes = server_info(b"");
    bytes[9..13].copy_from_slice(&CRC_MARKER.to_le_bytes());
    let message = reader(&bytes).next().unwrap().unwrap();
    let ServerMessage::ServerInfo(info) = message else {
        panic!("expected synthetic server-info message");
    };
    assert_eq!(info.map_crc, CRC_MARKER);
    let marker = CRC_MARKER.to_string();
    for debug in [format!("{info:?}"), format!("{message:?}")] {
        assert!(!debug.contains(&marker));
        assert!(debug.contains("map_crc"));
        assert!(debug.contains("redacted"));
    }
}

#[test]
fn error_diagnostics_omit_unknown_opcode_bytes_including_at_message_limit() {
    // Independently chosen synthetic bytes, unrelated to original packet data.
    let opcodes = [0xA3, 0xCC];
    for at_message_limit in [false, true] {
        let errors = opcodes.map(|opcode| {
            let mut bytes = if at_message_limit {
                vec![1; MAX_MESSAGES]
            } else {
                vec![]
            };
            bytes.push(opcode);
            reader(&bytes).find_map(Result::err).unwrap()
        });
        let expected_kind = if at_message_limit {
            MessageErrorKind::MessageLimit
        } else {
            MessageErrorKind::UnsupportedOpcode
        };
        assert_eq!(errors[0].kind, expected_kind);
        assert_eq!(errors[0].kind, errors[1].kind);
        assert_eq!(errors[0].offset, errors[1].offset);
        assert_eq!(errors[0].opcode, Some(opcodes[0]));
        assert_eq!(errors[1].opcode, Some(opcodes[1]));
        assert_ne!(errors[0].opcode, errors[1].opcode);
        assert_eq!(format!("{:?}", errors[0]), format!("{:?}", errors[1]));
        assert_eq!(format!("{}", errors[0]), format!("{}", errors[1]));
    }
}

#[test]
fn debug_and_errors_do_not_reveal_opaque_text_or_hash_bytes() {
    const MARKER: &[u8] = b"SECRET_SYNTHETIC_MARKER";
    for opcode in [2, 8, 9, 26, 54] {
        let mut bytes = string_message(opcode, MARKER);
        if opcode == 54 {
            bytes.push(0);
        }
        let mut messages = reader(&bytes);
        assert!(!format!("{messages:?}").contains("SECRET"));
        let message = messages.next().unwrap().unwrap();
        assert!(!format!("{message:?}").contains("SECRET"));
    }
    let mut bytes = server_info(MARKER);
    bytes[13..29].copy_from_slice(b"SYNTHETIC_HASH_X");
    let message = reader(&bytes).next().unwrap().unwrap();
    let debug = format!("{message:?}");
    assert!(!debug.contains("SECRET"));
    assert!(!debug.contains("Synthetic server"));
    assert!(!debug.contains("SYNTHETIC_HASH_X"));
    assert!(!debug.contains("83, 89, 78"));
    let ServerMessage::ServerInfo(info) = message else {
        panic!("expected synthetic server-info message");
    };
    assert_eq!(info.client_dll_hash.as_bytes(), b"SYNTHETIC_HASH_X");
    assert!(!format!("{:?}", info.client_dll_hash).contains("SYNTHETIC"));
    assert!(!format!("{:?}", info.mapcycle).contains("SECRET"));

    let mut bytes = vec![8];
    bytes.extend_from_slice(MARKER);
    let error = reader(&bytes).next().unwrap().unwrap_err();
    assert!(!format!("{error:?}").contains("SECRET"));
    assert!(!format!("{error}").contains("SECRET"));
}
