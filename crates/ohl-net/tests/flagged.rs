//! Independently authored synthetic fields and manually calculated wire words.
//! No native packet slices, text, resource identifiers or capture fixtures.

use ohl_net::{
    channel::{
        ChannelError, DecodedDatagram, EXPERIMENTAL_COUNTER_LIMIT, MAX_CHANNEL_DATAGRAM_BYTES,
        OBSERVED_PREFIX_BYTES, PacketPrefix, PrefixProfile, decode_datagram,
    },
    flagged::{
        CANDIDATE_BODY_OFFSET, Error, MAX_CANDIDATE_BODY_BYTES, MAX_CANDIDATE_COUNT,
        MIN_COMPLETE_HEADER_END, Profile, decode_body,
    },
    word_transform::{WordTransformError, WordTransformProfile, encode_full_words},
};

const PROFILE: Profile = Profile::ExperimentalProtocol46Linux3111dHldsToHltvFirstBit30Set;
const WORD_PROFILE: WordTransformProfile =
    WordTransformProfile::ExperimentalProtocol46Linux3111dHldsToHltvFirstBit30Set;
const PREFIX: PrefixProfile = PrefixProfile::ExperimentalProtocol46Linux3111d;
const BIT_30: u32 = 1 << 30;
const BIT_31: u32 = 1 << 31;

// P at absolute 8..24: count3/index2/reserved0/bodylen6 and six arbitrary bytes.
// Four hand-worked words, q=6D, K=[6D,00,00,6D], reverse P then XOR M and K:
// s=8:  [02,00,03,01] ^ [5A,18,00,00] ^ K = [35,18,03,6C]
// s=12: [06,00,00,00] ^ [1A,00,10,40] ^ K = [71,00,10,2D]
// s=16: [A6,19,00,00] ^ [00,10,40,00] ^ K = [CB,09,40,6D]
// s=20: [E8,57,C4,3B] ^ [12,40,10,40] ^ K = [97,17,D4,16]
const MANUAL_CIPHER: [u8; 16] = [
    0x35, 0x18, 0x03, 0x6C, 0x71, 0x00, 0x10, 0x2D, 0xCB, 0x09, 0x40, 0x6D, 0x97, 0x17, 0xD4, 0x16,
];
const MANUAL_BODY: [u8; 6] = [0x19, 0xA6, 0x3B, 0xC4, 0x57, 0xE8];

fn packet(first: u32, second: u32, remainder: &[u8]) -> Vec<u8> {
    let mut bytes = first.to_le_bytes().to_vec();
    bytes.extend_from_slice(&second.to_le_bytes());
    bytes.extend_from_slice(remainder);
    bytes
}

fn prefix_with_profile(profile: PrefixProfile, bytes: &[u8]) -> PacketPrefix<'_> {
    let DecodedDatagram::Packet(prefix) = decode_datagram(bytes, profile).unwrap() else {
        panic!("expected an authored synthetic packet prefix");
    };
    prefix
}

fn prefix(bytes: &[u8]) -> PacketPrefix<'_> {
    prefix_with_profile(PREFIX, bytes)
}

fn authored_plain(body_length: usize, tail_length: usize) -> Vec<u8> {
    assert_eq!((10 + body_length) % 4, 0);
    let mut plain = vec![1, 3, 0, 2, 0, 0, 0];
    plain.extend_from_slice(
        &u16::try_from(body_length + tail_length)
            .unwrap()
            .to_le_bytes(),
    );
    plain.push(0);
    plain.extend((0..body_length).map(|index| u8::try_from(index % 251).unwrap()));
    plain
}

// Mutation fixtures use the already independently tested mathematical inverse;
// MANUAL_CIPHER above independently checks both transform orientation and grammar.
fn encoded_fixture(plain: &[u8], tail: &[u8]) -> Vec<u8> {
    let context = packet(BIT_30 | 0x6D, BIT_30 | BIT_31 | 0x11, &[]);
    let mut encoded = vec![0; plain.len()];
    encode_full_words(WORD_PROFILE, prefix(&context), plain, &mut encoded).unwrap();
    let mut bytes = packet(BIT_30 | 0x6D, BIT_30 | BIT_31 | 0x11, &encoded);
    bytes.extend_from_slice(tail);
    bytes
}

#[test]
fn manual_wire_vector_validates_header_and_copies_only_body() {
    let bytes = packet(BIT_30 | 0x6D, 17, &MANUAL_CIPHER);
    let original = bytes.clone();
    let mut output = [0xAB; 13];
    let decoded = decode_body(PROFILE, prefix(&bytes), &mut output[2..11]).unwrap();
    assert_eq!(decoded.header().count(), 3);
    assert_eq!(decoded.header().index(), 2);
    assert_eq!(decoded.header().reserved_word(), 0);
    assert_eq!(decoded.header().declared_body_length(), 6);
    assert_eq!(decoded.body().as_bytes(), MANUAL_BODY);
    assert_eq!(decoded.body().len(), 6);
    assert!(!decoded.body().is_empty());
    assert!(decoded.opaque_tail().is_empty());
    assert_eq!(&output[..2], &[0xAB; 2]);
    assert_eq!(&output[8..], &[0xAB; 5]);
    assert_eq!(bytes, original);
}

#[test]
fn all_tail_residues_stay_original_opaque_and_count_in_declared_length() {
    for tail_length in 0..=3 {
        let tail = &[0xD7, 0xE8, 0xF9][..tail_length];
        let plain = authored_plain(6, tail_length);
        let bytes = encoded_fixture(&plain, tail);
        let original = bytes.clone();
        let mut output = [0xB1; 12];
        let decoded = decode_body(PROFILE, prefix(&bytes), &mut output[1..10]).unwrap();
        assert_eq!(decoded.body().as_bytes(), &plain[10..]);
        assert_eq!(
            usize::from(decoded.header().declared_body_length()),
            6 + tail_length
        );
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(
            decoded.opaque_tail().as_bytes().as_ptr(),
            bytes[24..].as_ptr()
        );
        assert_eq!(decoded.opaque_tail().len(), tail_length);
        assert_eq!(output[0], 0xB1);
        assert_eq!(&output[7..], &[0xB1; 5]);
        assert_eq!(bytes, original);
    }
}

#[test]
fn minimum_whole_header_requires_twenty_bytes_even_with_apparent_tail_fields() {
    assert_eq!(MIN_COMPLETE_HEADER_END, 20);
    assert_eq!(CANDIDATE_BODY_OFFSET, 18);
    let plain = authored_plain(2, 0);
    let bytes = encoded_fixture(&plain, &[]);
    let mut output = [0xB2; 6];
    let decoded = decode_body(PROFILE, prefix(&bytes), &mut output).unwrap();
    assert_eq!(decoded.body().as_bytes(), &[0, 1]);
    assert_eq!(&output[2..], &[0xB2; 4]);

    // The two original tail bytes cannot complete the missing decoded header.
    let bytes = encoded_fixture(&plain[..8], &[0, 0]);
    let mut output = [0xB3; 6];
    assert_eq!(
        decode_body(PROFILE, prefix(&bytes), &mut output).unwrap_err(),
        Error::IncompleteHeader
    );
    assert_eq!(output, [0xB3; 6]);
}

#[test]
fn every_prefix_preserving_truncation_fails_without_publishing_partial_output() {
    let bytes = packet(BIT_30 | 0x6D, 17, &MANUAL_CIPHER);
    for length in OBSERVED_PREFIX_BYTES..bytes.len() {
        let mut output = [0xB4; 18];
        let expected = if length < MIN_COMPLETE_HEADER_END {
            Error::IncompleteHeader
        } else {
            Error::BodyLength
        };
        assert_eq!(
            decode_body(PROFILE, prefix(&bytes[..length]), &mut output[2..16]).unwrap_err(),
            expected
        );
        assert_eq!(output, [0xB4; 18]);
    }
}

#[test]
fn all_fixed_header_bytes_are_checked_before_output_publication() {
    for offset in [0, 5, 6, 9] {
        for replacement in if offset == 0 { [0, 0x9D] } else { [1, 0x9D] } {
            let mut plain = authored_plain(6, 0);
            plain[offset] = replacement;
            let bytes = encoded_fixture(&plain, &[]);
            let mut output = [0xB5; 12];
            assert_eq!(
                decode_body(PROFILE, prefix(&bytes), &mut output).unwrap_err(),
                Error::FixedFields
            );
            assert_eq!(output, [0xB5; 12]);
            // Header failures stay failures even when the output is also short.
            assert_eq!(
                decode_body(PROFILE, prefix(&bytes), &mut output[..0]).unwrap_err(),
                Error::FixedFields
            );
            assert_eq!(output, [0xB5; 12]);
        }
    }
}

#[test]
fn count_and_one_based_index_guards_reject_zero_excess_and_inconsistent_values() {
    for (count, index, error) in [
        (0_u16, 1_u16, Error::CountLimit),
        (MAX_CANDIDATE_COUNT + 1, 1, Error::CountLimit),
        (u16::MAX, 1, Error::CountLimit),
        (3, 0, Error::IndexRange),
        (3, 4, Error::IndexRange),
        (3, u16::MAX, Error::IndexRange),
    ] {
        let mut plain = authored_plain(6, 0);
        plain[1..3].copy_from_slice(&count.to_le_bytes());
        plain[3..5].copy_from_slice(&index.to_le_bytes());
        let bytes = encoded_fixture(&plain, &[]);
        let mut output = [0xB6; 12];
        assert_eq!(
            decode_body(PROFILE, prefix(&bytes), &mut output).unwrap_err(),
            error
        );
        assert_eq!(output, [0xB6; 12]);
    }
}

#[test]
fn declared_length_must_equal_original_length_including_each_opaque_tail() {
    for tail_length in 0..=3 {
        for wrong in [0_u16, 5, 10, u16::MAX] {
            let mut plain = authored_plain(6, tail_length);
            plain[7..9].copy_from_slice(&wrong.to_le_bytes());
            let bytes = encoded_fixture(&plain, &[0xD1, 0xE2, 0xF3][..tail_length]);
            let mut output = [0xB7; 12];
            assert_eq!(
                decode_body(PROFILE, prefix(&bytes), &mut output).unwrap_err(),
                Error::BodyLength
            );
            assert_eq!(output, [0xB7; 12]);
        }
    }
}

#[test]
fn caller_profile_and_first_bit_thirty_are_mandatory_without_fallback() {
    for (first, prefix_profile, error) in [
        (0x6D, PREFIX, WordTransformError::FirstBit30Clear),
        (
            BIT_30 | 0x6D,
            PrefixProfile::ExperimentalProtocol48,
            WordTransformError::ProfileMismatch,
        ),
        (
            0x6D,
            PrefixProfile::ExperimentalProtocol48,
            WordTransformError::ProfileMismatch,
        ),
    ] {
        let bytes = packet(first, 17, &MANUAL_CIPHER);
        let mut output = [0xB8; 12];
        assert_eq!(
            decode_body(
                PROFILE,
                prefix_with_profile(prefix_profile, &bytes),
                &mut output
            )
            .unwrap_err(),
            Error::WordTransform(error)
        );
        assert_eq!(output, [0xB8; 12]);
    }
}

#[test]
fn insufficient_body_buffer_is_atomic_and_does_not_require_header_capacity() {
    let bytes = packet(BIT_30 | 0x6D, 17, &MANUAL_CIPHER);
    for length in 0..MANUAL_BODY.len() {
        let mut output = [0xB9; 12];
        assert_eq!(
            decode_body(PROFILE, prefix(&bytes), &mut output[2..2 + length]).unwrap_err(),
            Error::OutputTooSmall
        );
        assert_eq!(output, [0xB9; 12]);
    }
    let mut exact_body_only = [0; MANUAL_BODY.len()];
    let decoded = decode_body(PROFILE, prefix(&bytes), &mut exact_body_only).unwrap();
    assert_eq!(decoded.body().as_bytes(), MANUAL_BODY);
}

#[test]
fn maximum_datagram_count_and_index_boundaries_are_supported() {
    assert_eq!(
        MAX_CANDIDATE_BODY_BYTES,
        MAX_CHANNEL_DATAGRAM_BYTES - CANDIDATE_BODY_OFFSET
    );
    let mut plain = authored_plain(MAX_CANDIDATE_BODY_BYTES, 0);
    plain[1..3].copy_from_slice(&MAX_CANDIDATE_COUNT.to_le_bytes());
    plain[3..5].copy_from_slice(&MAX_CANDIDATE_COUNT.to_le_bytes());
    let bytes = encoded_fixture(&plain, &[]);
    assert_eq!(bytes.len(), MAX_CHANNEL_DATAGRAM_BYTES);
    let mut output = vec![0xBA; MAX_CANDIDATE_BODY_BYTES + 8];
    let decoded = decode_body(PROFILE, prefix(&bytes), &mut output[3..]).unwrap();
    assert_eq!(decoded.header().count(), MAX_CANDIDATE_COUNT);
    assert_eq!(decoded.header().index(), MAX_CANDIDATE_COUNT);
    assert_eq!(
        usize::from(decoded.header().declared_body_length()),
        MAX_CANDIDATE_BODY_BYTES
    );
    assert_eq!(decoded.body().as_bytes(), &plain[10..]);
    assert!(decoded.opaque_tail().is_empty());
    assert_eq!(&output[..3], &[0xBA; 3]);
    assert_eq!(&output[MAX_CANDIDATE_BODY_BYTES + 3..], &[0xBA; 5]);
}

#[test]
fn guarded_prefix_precondition_retains_counter_and_datagram_limits() {
    for (first, second, error) in [
        (
            BIT_30 | EXPERIMENTAL_COUNTER_LIMIT,
            0,
            ChannelError::FirstCounterGuard,
        ),
        (
            BIT_30 | 0x6D,
            EXPERIMENTAL_COUNTER_LIMIT,
            ChannelError::EchoCounterGuard,
        ),
    ] {
        let bytes = packet(first, second, &MANUAL_CIPHER);
        assert_eq!(decode_datagram(&bytes, PREFIX).unwrap_err(), error);
    }
    let oversized = packet(
        BIT_30 | 1,
        0,
        &vec![0; MAX_CHANNEL_DATAGRAM_BYTES - OBSERVED_PREFIX_BYTES + 1],
    );
    assert_eq!(
        decode_datagram(&oversized, PREFIX).unwrap_err(),
        ChannelError::DatagramLimit
    );
}

#[test]
fn debug_and_errors_redact_body_tail_counters_and_candidate_metadata() {
    let mut debug_outputs = Vec::new();
    for (count, index, byte) in [(939_u16, 795_u16, 0xD1), (4091, 3017, 0xE2)] {
        let mut plain = authored_plain(18, 3);
        plain[1..3].copy_from_slice(&count.to_le_bytes());
        plain[3..5].copy_from_slice(&index.to_le_bytes());
        plain[10..].fill(byte);
        let bytes = encoded_fixture(&plain, &[0xF7, 0xF8, 0xF9]);
        let mut output = [0; 18];
        let decoded = decode_body(PROFILE, prefix(&bytes), &mut output).unwrap();
        let debug = format!("{decoded:?}");
        assert!(!debug.contains('['));
        assert!(!debug.contains(&count.to_string()));
        assert!(!debug.contains(&index.to_string()));
        assert!(!debug.contains("247"));
        assert!(!debug.contains("248"));
        assert!(!debug.contains("249"));
        assert!(!format!("{:?}", decoded.header()).contains(&count.to_string()));
        assert!(!format!("{:?}", decoded.body()).contains('['));
        debug_outputs.push(debug);
    }
    assert_eq!(debug_outputs[0], debug_outputs[1]);
    for error in [
        Error::WordTransform(WordTransformError::FirstBit30Clear),
        Error::WordTransform(WordTransformError::ProfileMismatch),
        Error::IncompleteHeader,
        Error::FixedFields,
        Error::CountLimit,
        Error::IndexRange,
        Error::BodyLength,
        Error::OutputTooSmall,
    ] {
        assert!(!error.to_string().is_empty());
        assert!(!error.to_string().contains('['));
        assert!(!format!("{error:?}").contains('['));
    }
}
