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
const PROFILE46: WordTransformProfile =
    WordTransformProfile::ExperimentalProtocol46Linux3111dHltvToHlds;
const PROFILE46_REVERSE: WordTransformProfile =
    WordTransformProfile::ExperimentalProtocol46Linux3111dHldsToHltv;
const PROFILE46_FLAGGED: WordTransformProfile =
    WordTransformProfile::ExperimentalProtocol46Linux3111dHldsToHltvFirstBit30Set;
const PREFIX46: PrefixProfile = PrefixProfile::ExperimentalProtocol46Linux3111d;
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
    prefix_with_profile(PrefixProfile::ExperimentalProtocol48, bytes)
}

fn prefix_with_profile(profile: PrefixProfile, bytes: &[u8]) -> PacketPrefix<'_> {
    let DecodedDatagram::Packet(prefix) = decode_datagram(bytes, profile).unwrap() else {
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
        WordTransformError::FirstBit30Clear,
        WordTransformError::OutputTooSmall,
        WordTransformError::PartialWordInput,
        WordTransformError::DatagramLimit,
        WordTransformError::ProfileMismatch,
    ] {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}").contains('['));
        assert!(!error.to_string().contains(&0x0123_4567_u32.to_string()));
    }
}

#[test]
fn protocol46_matches_both_manual_vectors_and_retains_opaque_tails() {
    for length in 0..=3 {
        let tail = &[0x97, 0xA8, 0xB9][..length];
        let mut bytes = packet(0x5A, BIT_30 | BIT_31, &CIPHER_VECTOR);
        bytes.extend_from_slice(tail);
        let selected = prefix_with_profile(PREFIX46, &bytes);
        assert_eq!(selected.profile(), PREFIX46);
        let mut output = [0xCC; 12];
        let decoded = decode_full_words(PROFILE46, selected, &mut output[2..10]).unwrap();
        assert_eq!(decoded.complete_words().as_bytes(), PLAIN_VECTOR);
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(
            decoded.opaque_tail().as_bytes().as_ptr(),
            bytes[12..].as_ptr()
        );
        assert_eq!(&output[..2], &[0xCC; 2]);
        assert_eq!(&output[6..], &[0xCC; 6]);
        let mut encoded = [0xDD; 12];
        let words =
            encode_full_words(PROFILE46, selected, &PLAIN_VECTOR, &mut encoded[1..9]).unwrap();
        assert_eq!(words.as_bytes(), CIPHER_VECTOR);
        assert_eq!(encoded[0], 0xDD);
        assert_eq!(&encoded[5..], &[0xDD; 7]);
    }

    // Reuse the independently worked s=64/q=C7 example, not a captured word.
    let plain_word = [0x19, 0xA2, 0x3B, 0xC4];
    let cipher_word = [0x03, 0x63, 0xF2, 0xDE];
    let context = packet(0xC7, 0, &[]);
    let mut plain = [0x11; 60];
    plain[56..].copy_from_slice(&plain_word);
    let mut output = [0xCC; 64];
    let encoded = encode_full_words(
        PROFILE46,
        prefix_with_profile(PREFIX46, &context),
        &plain,
        &mut output,
    )
    .unwrap();
    assert_eq!(&encoded.as_bytes()[56..], &cipher_word);
    assert_eq!(&output[60..], &[0xCC; 4]);
    let mut cipher = [0x22; 60];
    cipher[56..].copy_from_slice(&cipher_word);
    let bytes = packet(0xC7, 0, &cipher);
    let decoded = decode_full_words(
        PROFILE46,
        prefix_with_profile(PREFIX46, &bytes),
        &mut output,
    )
    .unwrap();
    assert_eq!(&decoded.complete_words().as_bytes()[56..], &plain_word);
    assert_eq!(&output[60..], &[0xCC; 4]);
}

#[test]
fn protocol46_reverse_uses_independently_worked_words_at_twenty_and_forty_four() {
    // Independently authored non-palindromic words, not packet observations.
    // s=20: reverse P=08 A6 71 2D; XOR M=12 40 10 40, K=37 00 00 37.
    // s=44: reverse P=27 D4 3B 96; XOR M=0A 18 10 40, K=A2 00 00 A2.
    for (absolute, q, plain_word, cipher_word) in [
        (
            20_usize,
            0x37_u8,
            [0x2D, 0x71, 0xA6, 0x08],
            [0x2D, 0xE6, 0x61, 0x5A],
        ),
        (44, 0xA2, [0x96, 0x3B, 0xD4, 0x27], [0x8F, 0xCC, 0x2B, 0x74]),
    ] {
        let relative = absolute - OBSERVED_PREFIX_BYTES;
        let mut cipher = vec![0xE1; relative + 4];
        cipher[relative..].copy_from_slice(&cipher_word);
        let bytes = packet(u32::from(q) | BIT_31, 9 | BIT_30 | BIT_31, &cipher);
        let original = bytes.clone();
        let selected = prefix_with_profile(PREFIX46, &bytes);
        let mut output = vec![0xC9; cipher.len() + 8];
        let decoded = decode_full_words(PROFILE46_REVERSE, selected, &mut output[2..]).unwrap();
        assert_eq!(
            &decoded.complete_words().as_bytes()[relative..],
            &plain_word
        );
        assert!(decoded.opaque_tail().is_empty());
        assert_eq!(&output[..2], &[0xC9; 2]);
        assert_eq!(&output[cipher.len() + 2..], &[0xC9; 6]);

        let mut plain = vec![0xD2; relative + 4];
        plain[relative..].copy_from_slice(&plain_word);
        let mut output = vec![0xCA; plain.len() + 8];
        let encoded =
            encode_full_words(PROFILE46_REVERSE, selected, &plain, &mut output[3..]).unwrap();
        assert_eq!(&encoded.as_bytes()[relative..], &cipher_word);
        assert_eq!(&output[..3], &[0xCA; 3]);
        assert_eq!(&output[plain.len() + 3..], &[0xCA; 5]);
        assert_eq!(bytes, original);
    }
}

#[test]
fn protocol46_reverse_keeps_all_partial_tails_opaque_and_outside_output() {
    for length in 0..=3 {
        let tail = &[0xD7, 0xE8, 0xF9][..length];
        let mut cipher = [0xE1; 16];
        cipher[12..].copy_from_slice(&[0x2D, 0xE6, 0x61, 0x5A]);
        let mut bytes = packet(0x37, 0, &cipher);
        bytes.extend_from_slice(tail);
        let original = bytes.clone();
        let selected = prefix_with_profile(PREFIX46, &bytes);
        let mut output = [0xCB; 24];
        let decoded = decode_full_words(PROFILE46_REVERSE, selected, &mut output[1..]).unwrap();
        assert_eq!(decoded.complete_words().len(), 16);
        assert_eq!(
            &decoded.complete_words().as_bytes()[12..],
            &[0x2D, 0x71, 0xA6, 0x08]
        );
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(
            decoded.opaque_tail().as_bytes().as_ptr(),
            bytes[24..].as_ptr()
        );
        assert_eq!(output[0], 0xCB);
        assert_eq!(&output[17..], &[0xCB; 7]);
        assert_eq!(bytes, original);

        // A new complete-word encoding never appends the context's old tail.
        let mut encoded = [0xCC; 8];
        let words =
            encode_full_words(PROFILE46_REVERSE, selected, &PLAIN_VECTOR, &mut encoded).unwrap();
        assert_eq!(words.len(), 4);
        assert_eq!(&encoded[4..], &[0xCC; 4]);
        let only_tail = packet(0x37, 0, tail);
        let mut untouched = [0xCD; 8];
        let decoded = decode_full_words(
            PROFILE46_REVERSE,
            prefix_with_profile(PREFIX46, &only_tail),
            &mut untouched,
        )
        .unwrap();
        assert!(decoded.complete_words().is_empty());
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(untouched, [0xCD; 8]);
    }
}

#[test]
fn cross_profile_word_operations_reject_before_any_output_write() {
    for (word_profile, foreign_prefix) in [
        (PROFILE46, PrefixProfile::ExperimentalProtocol48),
        (PROFILE46_REVERSE, PrefixProfile::ExperimentalProtocol48),
        (PROFILE46_FLAGGED, PrefixProfile::ExperimentalProtocol48),
        (PROFILE, PREFIX46),
    ] {
        for first_flags in [0, BIT_30] {
            let bytes = packet(0x5A | first_flags, 0, &CIPHER_VECTOR);
            let foreign = prefix_with_profile(foreign_prefix, &bytes);
            let mut output = [0xCE; 12];
            assert_eq!(
                decode_full_words(word_profile, foreign, &mut output).unwrap_err(),
                WordTransformError::ProfileMismatch
            );
            assert_eq!(output, [0xCE; 12]);
            assert_eq!(
                encode_full_words(word_profile, foreign, &PLAIN_VECTOR, &mut output).unwrap_err(),
                WordTransformError::ProfileMismatch
            );
            assert_eq!(output, [0xCE; 12]);
            // Mismatch is reported even for an empty output or partial input.
            assert_eq!(
                decode_full_words(word_profile, foreign, &mut output[..0]).unwrap_err(),
                WordTransformError::ProfileMismatch
            );
            assert_eq!(
                encode_full_words(word_profile, foreign, &[0; 3], &mut output[..0]).unwrap_err(),
                WordTransformError::ProfileMismatch
            );
            assert_eq!(output, [0xCE; 12]);
        }
    }
}

#[test]
fn protocol46_word_profile_preserves_flag_length_and_capacity_guards() {
    for profile in [PROFILE46, PROFILE46_REVERSE] {
        let bytes = packet(0x5A | BIT_30, 0, &CIPHER_VECTOR);
        let mut output = [0xCF; 12];
        let blocked = prefix_with_profile(PREFIX46, &bytes);
        assert_eq!(
            decode_full_words(profile, blocked, &mut output).unwrap_err(),
            WordTransformError::FirstBit30Set
        );
        assert_eq!(
            encode_full_words(profile, blocked, &PLAIN_VECTOR, &mut output).unwrap_err(),
            WordTransformError::FirstBit30Set
        );
        assert_eq!(output, [0xCF; 12]);
        let bytes = packet(0x5A, 0, &CIPHER_VECTOR);
        let context = prefix_with_profile(PREFIX46, &bytes);
        for length in 1..=3 {
            assert_eq!(
                encode_full_words(profile, context, &PLAIN_VECTOR[..length], &mut output)
                    .unwrap_err(),
                WordTransformError::PartialWordInput
            );
            assert_eq!(output, [0xCF; 12]);
        }
        assert_eq!(
            decode_full_words(profile, context, &mut output[..3]).unwrap_err(),
            WordTransformError::OutputTooSmall
        );
        assert_eq!(
            encode_full_words(profile, context, &PLAIN_VECTOR, &mut output[..3]).unwrap_err(),
            WordTransformError::OutputTooSmall
        );
        assert_eq!(
            encode_full_words(
                profile,
                context,
                &vec![0; MAX_FULL_WORD_BYTES + 4],
                &mut output
            )
            .unwrap_err(),
            WordTransformError::DatagramLimit
        );
        assert_eq!(output, [0xCF; 12]);
    }
}

#[test]
fn flagged_protocol46_uses_manual_word_vector_with_all_opaque_flag_combinations() {
    // Same independently worked s=8/q=5A model vector, with the new explicit
    // first-bit-30-set selector. No flag-dependent coefficient is introduced.
    for first_31 in [0, BIT_31] {
        for second_flags in [0, BIT_30, BIT_31, BIT_30 | BIT_31] {
            let bytes = packet(
                BIT_30 | first_31 | 0x5A,
                second_flags | 0x11,
                &CIPHER_VECTOR,
            );
            let original = bytes.clone();
            let context = prefix_with_profile(PREFIX46, &bytes);
            let mut output = [0xD1; 10];
            let decoded = decode_full_words(PROFILE46_FLAGGED, context, &mut output[2..8]).unwrap();
            assert_eq!(decoded.complete_words().as_bytes(), PLAIN_VECTOR);
            assert!(decoded.opaque_tail().is_empty());
            assert_eq!(&output[..2], &[0xD1; 2]);
            assert_eq!(&output[6..], &[0xD1; 4]);

            let mut output = [0xD2; 10];
            let encoded =
                encode_full_words(PROFILE46_FLAGGED, context, &PLAIN_VECTOR, &mut output[1..7])
                    .unwrap();
            assert_eq!(encoded.as_bytes(), CIPHER_VECTOR);
            assert_eq!(output[0], 0xD2);
            assert_eq!(&output[5..], &[0xD2; 5]);
            assert_eq!(bytes, original);
        }
    }
}

#[test]
fn flagged_protocol46_keeps_partial_tails_opaque_and_excludes_them_from_inverse() {
    for tail_length in 0..=3 {
        let tail = &[0xD7, 0xE8, 0xF9][..tail_length];
        let mut bytes = packet(BIT_30 | 0x5A, 0, &CIPHER_VECTOR);
        bytes.extend_from_slice(tail);
        let context = prefix_with_profile(PREFIX46, &bytes);
        let mut output = [0xD3; 12];
        let decoded = decode_full_words(PROFILE46_FLAGGED, context, &mut output[3..]).unwrap();
        assert_eq!(decoded.complete_words().as_bytes(), PLAIN_VECTOR);
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(
            decoded.opaque_tail().as_bytes().as_ptr(),
            bytes[12..].as_ptr()
        );
        assert_eq!(&output[..3], &[0xD3; 3]);
        assert_eq!(&output[7..], &[0xD3; 5]);
        let mut encoded = [0xD4; 12];
        let words =
            encode_full_words(PROFILE46_FLAGGED, context, &PLAIN_VECTOR, &mut encoded).unwrap();
        assert_eq!(words.as_bytes(), CIPHER_VECTOR);
        assert_eq!(&encoded[4..], &[0xD4; 8]);

        let bytes = packet(BIT_30 | 0x5A, 0, tail);
        let mut output = [0xD5; 8];
        let decoded = decode_full_words(
            PROFILE46_FLAGGED,
            prefix_with_profile(PREFIX46, &bytes),
            &mut output,
        )
        .unwrap();
        assert!(decoded.complete_words().is_empty());
        assert_eq!(decoded.opaque_tail().as_bytes(), tail);
        assert_eq!(output, [0xD5; 8]);
    }
}

#[test]
fn flagged_protocol46_requires_set_bit_and_retains_all_atomic_word_guards() {
    let bytes = packet(0x5A, 0, &CIPHER_VECTOR);
    let context = prefix_with_profile(PREFIX46, &bytes);
    let mut output = [0xD6; 12];
    assert_eq!(
        decode_full_words(PROFILE46_FLAGGED, context, &mut output).unwrap_err(),
        WordTransformError::FirstBit30Clear
    );
    assert_eq!(
        encode_full_words(PROFILE46_FLAGGED, context, &PLAIN_VECTOR, &mut output).unwrap_err(),
        WordTransformError::FirstBit30Clear
    );
    assert_eq!(output, [0xD6; 12]);

    let bytes = packet(BIT_30 | 0x5A, 0, &CIPHER_VECTOR);
    let context = prefix_with_profile(PREFIX46, &bytes);
    for length in 1..=3 {
        assert_eq!(
            encode_full_words(
                PROFILE46_FLAGGED,
                context,
                &PLAIN_VECTOR[..length],
                &mut output
            )
            .unwrap_err(),
            WordTransformError::PartialWordInput
        );
        assert_eq!(output, [0xD6; 12]);
    }
    assert_eq!(
        decode_full_words(PROFILE46_FLAGGED, context, &mut output[..3]).unwrap_err(),
        WordTransformError::OutputTooSmall
    );
    assert_eq!(
        encode_full_words(PROFILE46_FLAGGED, context, &PLAIN_VECTOR, &mut output[..3]).unwrap_err(),
        WordTransformError::OutputTooSmall
    );
    assert_eq!(
        encode_full_words(
            PROFILE46_FLAGGED,
            context,
            &vec![0; MAX_FULL_WORD_BYTES + 4],
            &mut output
        )
        .unwrap_err(),
        WordTransformError::DatagramLimit
    );
    assert_eq!(output, [0xD6; 12]);
}
