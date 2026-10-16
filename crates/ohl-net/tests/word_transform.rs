//! Independently authored synthetic data; no captured packets or game assets.
//!
//! The four-byte worked vector is the mathematical specification's synthetic
//! example. Exhaustive key and word-position checks exercise the project model,
//! not an original endpoint or a claim of compatibility beyond observations.

use ohl_net::{
    channel::{
        ChannelError, DecodedDatagram, EXPERIMENTAL_COUNTER_LIMIT, MAX_CHANNEL_DATAGRAM_BYTES,
        OBSERVED_PREFIX_BYTES, PacketPrefix, PrefixProfile, decode_datagram,
    },
    word_transform::{
        MAX_FULL_WORD_BYTES, WordTransformError, WordTransformProfile, decode_full_words,
        encode_full_words,
    },
};

const PROFILE: WordTransformProfile = WordTransformProfile::ExperimentalProtocol48HltvToHlds;
const BIT_30: u32 = 1 << 30;
const BIT_31: u32 = 1 << 31;
const PLAIN_VECTOR: [u8; 4] = [0x10, 0x21, 0x32, 0x43];
const CIPHER_VECTOR: [u8; 4] = [0x43, 0x2A, 0x21, 0x4A];

fn packet(first: u32, second: u32, remainder: &[u8]) -> Vec<u8> {
    let mut bytes = first.to_le_bytes().to_vec();
    bytes.extend_from_slice(&second.to_le_bytes());
    bytes.extend_from_slice(remainder);
    bytes
}

fn prefix(bytes: &[u8]) -> PacketPrefix<'_> {
    let DecodedDatagram::Packet(prefix) =
        decode_datagram(bytes, PrefixProfile::ExperimentalProtocol48).unwrap()
    else {
        panic!("expected a synthetic guarded prefix");
    };
    prefix
}

#[test]
fn manually_worked_specification_vector_and_caller_regions() {
    // Absolute word start 8, q=0x5A. The mathematical specification works
    // the XOR/permutation by hand; this is not a captured byte fixture.
    let bytes = packet(0x5A, 17, &CIPHER_VECTOR);
    let original = bytes.clone();
    let mut decoded_storage = [0xA5; 12];
    let decoded = decode_full_words(PROFILE, prefix(&bytes), &mut decoded_storage[2..10]).unwrap();
    assert_eq!(decoded.complete_words().as_bytes(), PLAIN_VECTOR);
    assert_eq!(decoded.complete_words().len(), 4);
    assert!(!decoded.complete_words().is_empty());
    assert!(decoded.opaque_tail().is_empty());
    assert_eq!(&decoded_storage[..2], &[0xA5; 2]);
    assert_eq!(&decoded_storage[6..], &[0xA5; 6]);
    assert_eq!(bytes, original);

    let mut encoded_storage = [0xA6; 12];
    let encoded = encode_full_words(
        PROFILE,
        prefix(&bytes),
        &PLAIN_VECTOR,
        &mut encoded_storage[3..11],
    )
    .unwrap();
    assert_eq!(encoded.as_bytes(), CIPHER_VECTOR);
    assert_eq!(encoded.len(), 4);
    assert!(!encoded.is_empty());
    assert_eq!(&encoded_storage[..3], &[0xA6; 3]);
    assert_eq!(&encoded_storage[7..], &[0xA6; 5]);
    assert_eq!(bytes, original);
}

#[test]
fn absolute_mask_phase_repeats_after_sixty_four_bytes() {
    // Context remainder is deliberately unrelated to this new payload length.
    let context = packet(0x5A, 0, &[0xAA; 7]);
    let plain = PLAIN_VECTOR.repeat(17);
    let mut output = [0xCC; 72];
    let encoded = encode_full_words(PROFILE, prefix(&context), &plain, &mut output).unwrap();
    assert_eq!(&encoded.as_bytes()[..4], &CIPHER_VECTOR);
    assert_eq!(&encoded.as_bytes()[64..68], &CIPHER_VECTOR);
    assert_eq!(encoded.len(), 68);
    assert_eq!(&output[68..], &[0xCC; 4]);
}

#[test]
fn independently_worked_vector_at_absolute_sixty_four_distinguishes_mask_phase() {
    // Absolute s=64 is word index 14 after the selected prefix. This second
    // non-palindromic example was calculated independently from the spec.
    let plain_word = [0x19, 0xA2, 0x3B, 0xC4];
    let cipher_word = [0x03, 0x63, 0xF2, 0xDE];
    let context = packet(0xC7, 0, &[]);
    let mut plain = [0x11; 60];
    plain[56..].copy_from_slice(&plain_word);
    let mut encoded = [0; 60];
    let words = encode_full_words(PROFILE, prefix(&context), &plain, &mut encoded).unwrap();
    assert_eq!(&words.as_bytes()[56..], &cipher_word);

    let mut cipher = [0x22; 60];
    cipher[56..].copy_from_slice(&cipher_word);
    let bytes = packet(0xC7, 0, &cipher);
    let mut decoded = [0; 60];
    let words = decode_full_words(PROFILE, prefix(&bytes), &mut decoded).unwrap();
    assert_eq!(&words.complete_words().as_bytes()[56..], &plain_word);
}

#[test]
fn inverse_covers_every_key_byte_and_all_period_positions() {
    for q in 0_u8..=u8::MAX {
        let counter = EXPERIMENTAL_COUNTER_LIMIT - 256 + u32::from(q);
        let first = counter | if q % 2 == 0 { BIT_31 } else { 0 };
        let second = 7 | BIT_30 | if q % 3 == 0 { BIT_31 } else { 0 };
        let context = packet(first, second, &[0x91; 3]);
        let original_context = context.clone();
        let plain: Vec<u8> = (0..128)
            .map(|i| u8::try_from(i).unwrap().wrapping_mul(37) ^ q)
            .collect();
        let mut encoded_storage = [0xB1; 132];
        let encoded = encode_full_words(
            PROFILE,
            prefix(&context),
            &plain,
            &mut encoded_storage[1..131],
        )
        .unwrap();
        let mut bytes = packet(first, second, encoded.as_bytes());
        let tail = &[0xF1, 0xE2, 0xD3][..usize::from(q % 4)];
        bytes.extend_from_slice(tail);
        assert_eq!(encoded_storage[0], 0xB1);
        assert_eq!(&encoded_storage[129..], &[0xB1; 3]);

        let mut decoded_storage = [0xB2; 134];
        let decoded =
            decode_full_words(PROFILE, prefix(&bytes), &mut decoded_storage[2..132]).unwrap();
        assert_eq!(decoded.complete_words().as_bytes(), plain);
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(decoded.opaque_tail().len(), tail.len());
        assert_eq!(
            decoded.opaque_tail().as_bytes().as_ptr(),
            bytes[OBSERVED_PREFIX_BYTES + plain.len()..].as_ptr()
        );
        assert_eq!(&decoded_storage[..2], &[0xB2; 2]);
        assert_eq!(&decoded_storage[130..], &[0xB2; 4]);
        assert_eq!(context, original_context);
    }
}

#[test]
fn partial_tails_remain_original_opaque_bytes_outside_output() {
    for length in 0..=3 {
        let tail = &[0xDE, 0xAD, 0xBE][..length];
        let mut bytes = packet(0x5A, 0, &CIPHER_VECTOR);
        bytes.extend_from_slice(tail);
        let original = bytes.clone();
        let mut output = [0xC1; 9];
        let decoded = decode_full_words(PROFILE, prefix(&bytes), &mut output[1..8]).unwrap();
        assert_eq!(decoded.complete_words().as_bytes(), PLAIN_VECTOR);
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(decoded.opaque_tail().is_empty(), length == 0);
        assert_eq!(
            decoded.opaque_tail().as_bytes().as_ptr(),
            bytes[12..].as_ptr()
        );
        assert_eq!(output[0], 0xC1);
        assert_eq!(&output[5..], &[0xC1; 4]);
        assert_eq!(bytes, original);

        let only_tail = packet(3, 0, tail);
        let mut untouched = [0xC2; 4];
        let decoded = decode_full_words(PROFILE, prefix(&only_tail), &mut untouched).unwrap();
        assert!(decoded.complete_words().is_empty());
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(untouched, [0xC2; 4]);
    }
}

#[test]
fn empty_encoding_adds_no_prefix_padding_or_old_tail() {
    let context = packet(8, 0, &[0x12; 3]);
    let mut output = [0xC3; 8];
    let encoded = encode_full_words(PROFILE, prefix(&context), &[], &mut output).unwrap();
    assert!(encoded.is_empty());
    assert_eq!(encoded.len(), 0);
    assert_eq!(output, [0xC3; 8]);
    let empty = packet(9, 0, &[]);
    let decoded = decode_full_words(PROFILE, prefix(&empty), &mut output).unwrap();
    assert!(decoded.complete_words().is_empty());
    assert!(decoded.opaque_tail().is_empty());
    assert_eq!(output, [0xC3; 8]);
}

#[test]
fn partial_encoding_is_rejected_before_any_output_write() {
    let context = packet(12, 0, &[]);
    for extra in 1..=3 {
        let input = vec![0xDD; 4 + extra];
        let mut output = [0xC4; 16];
        assert_eq!(
            encode_full_words(PROFILE, prefix(&context), &input, &mut output).unwrap_err(),
            WordTransformError::PartialWordInput
        );
        assert_eq!(output, [0xC4; 16]);
    }
}

#[test]
fn first_bit_thirty_is_rejected_atomically_in_both_directions() {
    let bytes = packet(BIT_30 | 0x5A, 0, &[0x11; 11]);
    let mut output = [0xC5; 16];
    assert_eq!(
        decode_full_words(PROFILE, prefix(&bytes), &mut output).unwrap_err(),
        WordTransformError::FirstBit30Set
    );
    assert_eq!(output, [0xC5; 16]);
    assert_eq!(
        encode_full_words(PROFILE, prefix(&bytes), &[0; 8], &mut output).unwrap_err(),
        WordTransformError::FirstBit30Set
    );
    assert_eq!(output, [0xC5; 16]);
}

#[test]
fn insufficient_buffers_are_rejected_before_first_word_write() {
    let bytes = packet(20, 0, &[0x22; 16]);
    let mut output = [0xC6; 19];
    assert_eq!(
        decode_full_words(PROFILE, prefix(&bytes), &mut output[2..17]).unwrap_err(),
        WordTransformError::OutputTooSmall
    );
    assert_eq!(output, [0xC6; 19]);
    assert_eq!(
        encode_full_words(PROFILE, prefix(&bytes), &[0x33; 16], &mut output[2..17]).unwrap_err(),
        WordTransformError::OutputTooSmall
    );
    assert_eq!(output, [0xC6; 19]);
}

#[test]
fn project_datagram_limit_accepts_boundary_and_rejects_excess_atomically() {
    assert_eq!(
        MAX_FULL_WORD_BYTES + OBSERVED_PREFIX_BYTES,
        MAX_CHANNEL_DATAGRAM_BYTES
    );
    let context = packet(EXPERIMENTAL_COUNTER_LIMIT - 1, 0, &[]);
    let plain = vec![0x44; MAX_FULL_WORD_BYTES];
    let mut output = vec![0xC7; MAX_FULL_WORD_BYTES + 4];
    let encoded = encode_full_words(PROFILE, prefix(&context), &plain, &mut output).unwrap();
    assert_eq!(encoded.len(), MAX_FULL_WORD_BYTES);
    let bytes = packet(EXPERIMENTAL_COUNTER_LIMIT - 1, 0, encoded.as_bytes());
    assert_eq!(bytes.len(), MAX_CHANNEL_DATAGRAM_BYTES);
    assert_eq!(&output[MAX_FULL_WORD_BYTES..], &[0xC7; 4]);
    let mut decoded = vec![0xC8; MAX_FULL_WORD_BYTES + 4];
    let words = decode_full_words(PROFILE, prefix(&bytes), &mut decoded).unwrap();
    assert_eq!(words.complete_words().as_bytes(), plain);
    assert!(words.opaque_tail().is_empty());
    assert_eq!(&decoded[MAX_FULL_WORD_BYTES..], &[0xC8; 4]);

    let too_long = vec![0x55; MAX_FULL_WORD_BYTES + 4];
    let before = output.clone();
    assert_eq!(
        encode_full_words(PROFILE, prefix(&context), &too_long, &mut output).unwrap_err(),
        WordTransformError::DatagramLimit
    );
    assert_eq!(output, before);
}

#[test]
fn guarded_prefix_precondition_retains_counter_and_datagram_guards() {
    let too_large = packet(1, 0, &vec![0; MAX_FULL_WORD_BYTES + 1]);
    assert_eq!(
        decode_datagram(&too_large, PrefixProfile::ExperimentalProtocol48).unwrap_err(),
        ChannelError::DatagramLimit
    );
    for (first, second, error) in [
        (
            EXPERIMENTAL_COUNTER_LIMIT,
            0,
            ChannelError::FirstCounterGuard,
        ),
        (
            0,
            EXPERIMENTAL_COUNTER_LIMIT,
            ChannelError::EchoCounterGuard,
        ),
    ] {
        let bytes = packet(first, second, &[0; 4]);
        assert_eq!(
            decode_datagram(&bytes, PrefixProfile::ExperimentalProtocol48).unwrap_err(),
            error
        );
    }
}

#[test]
fn key_uses_only_first_counter_low_byte_and_no_other_flags() {
    for (first, second) in [(0x5A, 0), (0x15A, BIT_30), (0x205A | BIT_31, BIT_31)] {
        let bytes = packet(first, second, &[]);
        let before = bytes.clone();
        let mut output = [0; 4];
        let encoded =
            encode_full_words(PROFILE, prefix(&bytes), &PLAIN_VECTOR, &mut output).unwrap();
        assert_eq!(encoded.as_bytes(), CIPHER_VECTOR);
        assert_eq!(bytes, before);
    }
    let other_key = packet(0xFF, 0, &[]);
    let mut output = [0; 4];
    let encoded =
        encode_full_words(PROFILE, prefix(&other_key), &PLAIN_VECTOR, &mut output).unwrap();
    // Changing the key changes only the first/fourth wire bytes in each word.
    assert_eq!(encoded.as_bytes(), [0xE6, 0x2A, 0x21, 0xEF]);
}

#[test]
fn automatic_debug_and_errors_never_expose_bytes_or_counter() {
    let context = packet(0x0123_4567, 0, &[]);
    let mut encoded_storage = [0; 16];
    let encoded =
        encode_full_words(PROFILE, prefix(&context), &[0x41; 16], &mut encoded_storage).unwrap();
    let encoded_debug = format!("{encoded:?}");
    assert!(encoded_debug.contains("length"));
    assert!(!encoded_debug.contains('['));
    let mut bytes = packet(0x0123_4567, 0, encoded.as_bytes());
    bytes.extend_from_slice(&[251, 252, 253]);
    let mut output = [0; 16];
    let decoded = decode_full_words(PROFILE, prefix(&bytes), &mut output).unwrap();
    for debug in [
        format!("{decoded:?}"),
        format!("{:?}", decoded.complete_words()),
        format!("{:?}", decoded.opaque_tail()),
    ] {
        assert!(debug.contains("length"));
        assert!(!debug.contains('['));
        assert!(!debug.contains("AAAA"));
        assert!(!debug.contains("251"));
        assert!(!debug.contains("252"));
        assert!(!debug.contains("253"));
        assert!(!debug.contains(&0x0123_4567_u32.to_string()));
    }
    for error in [
        WordTransformError::FirstBit30Set,
        WordTransformError::OutputTooSmall,
        WordTransformError::PartialWordInput,
        WordTransformError::DatagramLimit,
    ] {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").contains('['));
        assert!(!error.to_string().contains(&0x0123_4567_u32.to_string()));
    }
}
