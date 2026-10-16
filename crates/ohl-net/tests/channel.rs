//! Project-authored synthetic prefix fixtures; none are packet captures.
//!
//! Scalars, flags, counter timelines, and opaque text below were independently
//! selected for these tests. They contain no original game identifiers or data.

use ohl_net::channel::{
    ChannelError, DecodedDatagram, EXPERIMENTAL_COUNTER_LIMIT, EchoComparison, MAX_ACCEPTED_STATES,
    MAX_CHANNEL_DATAGRAM_BYTES, OBSERVED_PREFIX_BYTES, PacketPrefix, PrefixProfile,
    ReceiveDisposition, ReceiveObservation, ReceiveState, ReceiveTracker, decode_datagram,
};

const PROFILE: PrefixProfile = PrefixProfile::ExperimentalProtocol48;
const BIT_30: u32 = 1 << 30;
const BIT_31: u32 = 1 << 31;

fn packet(first_word: u32, second_word: u32, remainder: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(OBSERVED_PREFIX_BYTES + remainder.len());
    bytes.extend_from_slice(&first_word.to_le_bytes());
    bytes.extend_from_slice(&second_word.to_le_bytes());
    bytes.extend_from_slice(remainder);
    bytes
}

fn prefix(bytes: &[u8]) -> PacketPrefix<'_> {
    let DecodedDatagram::Packet(prefix) = decode_datagram(bytes, PROFILE).unwrap() else {
        panic!("expected synthetic guarded prefix");
    };
    prefix
}

fn advanced(observation: ReceiveObservation<'_>) -> ReceiveState {
    let ReceiveDisposition::Advanced(state) = observation.disposition() else {
        panic!("expected synthetic first-counter advance");
    };
    state
}

#[test]
fn selected_view_decodes_two_words_and_preserves_opaque_bits_and_remainder() {
    let bytes = [
        0x67, 0x45, 0x23, 0xC1, // two opaque/marker bits plus low30 projection
        0x78, 0x56, 0x34, 0xC2, // independently selected echoed word
        b'x', 0, 0xFF, b'y', // uninterpreted bytes, not assumed svc opcodes
    ];
    let prefix = prefix(&bytes);
    assert_eq!(prefix.profile(), PROFILE);
    assert_eq!(prefix.raw_words(), [0xC123_4567, 0xC234_5678]);
    assert_eq!(prefix.first_counter(), 0x0123_4567);
    assert_eq!(prefix.echoed_counter(), 0x0234_5678);
    assert!(prefix.first_bit_31());
    assert!(prefix.echoed_bit_31());
    assert!(prefix.first_bit_30());
    assert!(prefix.second_bit_30());
    let remainder = prefix.unvalidated_remainder();
    assert_eq!(remainder.as_bytes(), &bytes[OBSERVED_PREFIX_BYTES..]);
    assert_eq!(remainder.as_bytes().as_ptr(), bytes[8..].as_ptr());
    assert_eq!(remainder.len(), 4);
    assert!(!remainder.is_empty());

    let bytes = packet(1, 2, &[]);
    let prefix = self::prefix(&bytes);
    assert!(!prefix.first_bit_31());
    assert!(!prefix.echoed_bit_31());
    assert!(!prefix.first_bit_30());
    assert!(!prefix.second_bit_30());
    assert!(prefix.unvalidated_remainder().is_empty());
}

#[test]
fn documented_markers_are_classified_before_experimental_prefix_checks() {
    for (marker, split) in [([0xFF; 4], false), ([0xFE, 0xFF, 0xFF, 0xFF], true)] {
        for body in [b"".as_slice(), b"Synthetic opaque bytes".as_slice()] {
            let mut bytes = marker.to_vec();
            bytes.extend_from_slice(body);
            let decoded = decode_datagram(&bytes, PROFILE).unwrap();
            let opaque = match decoded {
                DecodedDatagram::Connectionless(opaque) if !split => opaque,
                DecodedDatagram::Split(opaque) if split => opaque,
                _ => panic!("unexpected synthetic marker classification"),
            };
            assert_eq!(opaque.as_bytes(), body);
            assert_eq!(opaque.as_bytes().as_ptr(), bytes[4..].as_ptr());
        }
    }
}

#[test]
fn every_incomplete_selected_prefix_is_rejected() {
    let bytes = [1, 2, 3, 4, 5, 6, 7, 8];
    for length in 0..OBSERVED_PREFIX_BYTES {
        assert_eq!(
            decode_datagram(&bytes[..length], PROFILE).unwrap_err(),
            ChannelError::TruncatedPrefix
        );
    }
    assert!(decode_datagram(&bytes, PROFILE).is_ok());
}

#[test]
fn datagram_limit_applies_to_all_three_paths() {
    let mut bytes = packet(
        1,
        0,
        &vec![0; MAX_CHANNEL_DATAGRAM_BYTES - OBSERVED_PREFIX_BYTES],
    );
    assert!(decode_datagram(&bytes, PROFILE).is_ok());
    bytes.push(0);
    assert_eq!(
        decode_datagram(&bytes, PROFILE).unwrap_err(),
        ChannelError::DatagramLimit
    );
    for marker in [[0xFF; 4], [0xFE, 0xFF, 0xFF, 0xFF]] {
        bytes[..4].copy_from_slice(&marker);
        assert_eq!(
            decode_datagram(&bytes, PROFILE).unwrap_err(),
            ChannelError::DatagramLimit
        );
        assert!(decode_datagram(&bytes[..MAX_CHANNEL_DATAGRAM_BYTES], PROFILE).is_ok());
    }
}

#[test]
fn conservative_counter_guard_does_not_fold_in_bit_thirty_or_bit_thirty_one() {
    let maximum = EXPERIMENTAL_COUNTER_LIMIT - 1;
    for first_flags in [0, BIT_30, BIT_31, BIT_30 | BIT_31] {
        for second_flags in [0, BIT_30, BIT_31, BIT_30 | BIT_31] {
            let bytes = packet(maximum | first_flags, maximum | second_flags, &[]);
            let prefix = prefix(&bytes);
            assert_eq!(prefix.first_counter(), maximum);
            assert_eq!(prefix.echoed_counter(), maximum);
            assert_eq!(prefix.first_bit_30(), first_flags & BIT_30 != 0);
            assert_eq!(prefix.second_bit_30(), second_flags & BIT_30 != 0);
        }
        let bytes = packet(EXPERIMENTAL_COUNTER_LIMIT | first_flags, 0, &[]);
        assert_eq!(
            decode_datagram(&bytes, PROFILE).unwrap_err(),
            ChannelError::FirstCounterGuard
        );
        let bytes = packet(1, EXPERIMENTAL_COUNTER_LIMIT | first_flags, &[]);
        assert_eq!(
            decode_datagram(&bytes, PROFILE).unwrap_err(),
            ChannelError::EchoCounterGuard
        );
    }
    // A full low30 value is rejected rather than silently truncated to low29.
    let bytes = packet(0x3FFF_FFFF, 0, &[]);
    assert_eq!(
        decode_datagram(&bytes, PROFILE).unwrap_err(),
        ChannelError::FirstCounterGuard
    );
    let bytes = packet(1, u32::MAX, &[]);
    assert_eq!(
        decode_datagram(&bytes, PROFILE).unwrap_err(),
        ChannelError::EchoCounterGuard
    );
}

#[test]
fn first_observation_advances_from_false_at_any_guarded_counter() {
    for (counter, marker) in [(0, false), (0, true), (900, false), (900, true)] {
        let mut tracker = ReceiveTracker::new(PROFILE);
        assert_eq!(tracker.state(), None);
        assert_eq!(tracker.retained_states(), 0);
        assert_eq!(tracker.profile(), PROFILE);
        let bytes = packet(counter | if marker { BIT_31 } else { 0 }, 0, &[]);
        let state = advanced(tracker.observe_datagram(&bytes).unwrap());
        assert_eq!(state.counter(), counter);
        assert_eq!(state.parity(), marker);
        assert_eq!(tracker.state(), Some(state));
        assert_eq!(tracker.retained_states(), 1);
    }
}

#[test]
fn advancing_markers_toggle_while_loss_and_counter_gaps_add_no_invented_state() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    for (first_word, expected_parity) in [
        (1 | BIT_31, true),
        (6 | BIT_30, true),
        (7 | BIT_31, false),
        (9 | BIT_31 | BIT_30, true),
        (12, true),
    ] {
        let bytes = packet(first_word, 0, &[]);
        let state = advanced(tracker.observe_datagram(&bytes).unwrap());
        assert_eq!(state.counter(), first_word & 0x3FFF_FFFF);
        assert_eq!(state.parity(), expected_parity);
    }
    assert_eq!(tracker.retained_states(), 5);
    let mut opposite = ReceiveTracker::new(PROFILE);
    let bytes = packet(30, 4, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Unobserved { echoed_counter: 4 }
    );
}

#[test]
fn duplicates_and_reordered_stale_prefixes_never_toggle_or_enter_history() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    let bytes = packet(5 | BIT_31, 0, b"Synthetic accepted remainder");
    advanced(tracker.observe_datagram(&bytes).unwrap());
    let before = tracker.clone();
    for (first_word, expected) in [
        (5 | BIT_31, ReceiveDisposition::Duplicate),
        (5 | BIT_30, ReceiveDisposition::Duplicate),
        (4 | BIT_31, ReceiveDisposition::Stale),
        (1 | BIT_31 | BIT_30, ReceiveDisposition::Stale),
    ] {
        let bytes = packet(first_word, 0, b"Synthetic rejected remainder");
        let observation = tracker.observe_datagram(&bytes).unwrap();
        assert_eq!(observation.disposition(), expected);
        assert_eq!(tracker, before);
    }
    let bytes = packet(8 | BIT_31, 0, &[]);
    let state = advanced(tracker.observe_datagram(&bytes).unwrap());
    assert_eq!(state.counter(), 8);
    assert!(!state.parity());
    assert_eq!(tracker.retained_states(), 2);
}

#[test]
fn malformed_guarded_and_nonpacket_paths_fail_atomically() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    let bytes = packet(7 | BIT_31, 2, &[]);
    advanced(tracker.observe_datagram(&bytes).unwrap());
    let before = tracker.clone();
    let cases = [
        (vec![], ChannelError::TruncatedPrefix),
        (vec![1, 0, 0, 0, 0, 0, 0], ChannelError::TruncatedPrefix),
        (
            packet(EXPERIMENTAL_COUNTER_LIMIT | BIT_31, 0, &[]),
            ChannelError::FirstCounterGuard,
        ),
        (
            packet(8 | BIT_31, EXPERIMENTAL_COUNTER_LIMIT, &[]),
            ChannelError::EchoCounterGuard,
        ),
        (vec![0xFF; 4], ChannelError::ConnectionlessDatagram),
        (vec![0xFE, 0xFF, 0xFF, 0xFF], ChannelError::SplitDatagram),
        (
            vec![0; MAX_CHANNEL_DATAGRAM_BYTES + 1],
            ChannelError::DatagramLimit,
        ),
    ];
    for (bytes, expected) in cases {
        assert_eq!(tracker.observe_datagram(&bytes).unwrap_err(), expected);
        assert_eq!(tracker, before);
    }
}

#[test]
fn echo_comparison_uses_exact_historical_state_instead_of_latest_or_mirrored_bit() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    let bytes = packet(1 | BIT_31, 0, &[]);
    let old = advanced(tracker.observe_datagram(&bytes).unwrap());
    let bytes = packet(2 | BIT_31, 0, &[]);
    let latest = advanced(tracker.observe_datagram(&bytes).unwrap());
    assert!(old.parity());
    assert!(!latest.parity());

    let mut opposite = ReceiveTracker::new(PROFILE);
    let bytes = packet(10, 1 | BIT_31, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Matches(old)
    );
    let bytes = packet(11, 1, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Mismatch {
            expected: old,
            observed_bit_31: false,
        }
    );

    // The second accepted marked prefix's parity is false despite first bit31=1.
    let bytes = packet(12, 2, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Matches(latest)
    );
    let before = tracker.clone();
    let bytes = packet(13, 0x0384 | BIT_31, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Unobserved {
            echoed_counter: 900,
        }
    );
    assert_eq!(tracker, before);
}

#[test]
fn duplicate_and_stale_opposite_observations_are_excluded_from_echo_evidence() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    let bytes = packet(1 | BIT_31, 0, &[]);
    let state = advanced(tracker.observe_datagram(&bytes).unwrap());
    let mut opposite = ReceiveTracker::new(PROFILE);
    let bytes = packet(10, 1 | BIT_31, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Matches(state)
    );

    for first_word in [0x000A | BIT_31, 9 | BIT_31] {
        let bytes = packet(first_word, 1, &[]); // marker would contradict if admitted
        let observation = opposite.observe_datagram(&bytes).unwrap();
        assert_eq!(
            tracker.compare_echo(&observation),
            EchoComparison::NotAdvanced
        );
    }
    let bytes = packet(11, 1, &[]);
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert!(matches!(
        tracker.compare_echo(&observation),
        EchoComparison::Mismatch { .. }
    ));
}

#[test]
fn accepted_history_is_bounded_and_evicted_or_never_seen_echoes_stay_unobserved() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    for counter in 0..MAX_ACCEPTED_STATES + 3 {
        let counter = u32::try_from(counter).unwrap();
        let bytes = packet(counter | BIT_31, 0, &[]);
        let state = advanced(tracker.observe_datagram(&bytes).unwrap());
        assert_eq!(state.parity(), counter % 2 == 0);
        assert!(tracker.retained_states() <= MAX_ACCEPTED_STATES);
    }
    assert_eq!(tracker.retained_states(), MAX_ACCEPTED_STATES);
    let mut opposite = ReceiveTracker::new(PROFILE);
    let bytes = packet(1, 2 | BIT_31, &[]); // was accepted, now evicted
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Unobserved { echoed_counter: 2 }
    );
    let bytes = packet(2, 3, &[]); // oldest retained counter has false parity
    let observation = opposite.observe_datagram(&bytes).unwrap();
    let EchoComparison::Matches(state) = tracker.compare_echo(&observation) else {
        panic!("expected retained synthetic counter match");
    };
    assert_eq!(state.counter(), 3);
    assert!(!state.parity());
    let bytes = packet(3, 900, &[]); // never accepted
    let observation = opposite.observe_datagram(&bytes).unwrap();
    assert_eq!(
        tracker.compare_echo(&observation),
        EchoComparison::Unobserved {
            echoed_counter: 900,
        }
    );
}

#[test]
fn reaching_the_guard_does_not_enable_wrap_or_reset_fallback() {
    let mut tracker = ReceiveTracker::new(PROFILE);
    let bytes = packet((EXPERIMENTAL_COUNTER_LIMIT - 1) | BIT_31, 0, &[]);
    advanced(tracker.observe_datagram(&bytes).unwrap());
    let before = tracker.clone();
    let bytes = packet(BIT_31, 0, &[]);
    let observation = tracker.observe_datagram(&bytes).unwrap();
    assert_eq!(observation.disposition(), ReceiveDisposition::Stale);
    assert_eq!(tracker, before);
    let bytes = packet(EXPERIMENTAL_COUNTER_LIMIT | BIT_31, 0, &[]);
    assert_eq!(
        tracker.observe_datagram(&bytes).unwrap_err(),
        ChannelError::FirstCounterGuard
    );
    assert_eq!(tracker, before);
}

#[test]
fn debug_and_errors_redact_raw_words_and_all_opaque_remainders() {
    const REMAINDER: &[u8] = b"SECRET_SYNTHETIC_REMAINDER";
    let first_word = u32::from_le_bytes(*b"ABCD");
    let second_word = u32::from_le_bytes(*b"EFGH");
    let bytes = packet(first_word, second_word, REMAINDER);
    let decoded = decode_datagram(&bytes, PROFILE).unwrap();
    let prefix = prefix(&bytes);
    assert_eq!(prefix.raw_words(), [first_word, second_word]);
    assert_eq!(prefix.unvalidated_remainder().as_bytes(), REMAINDER);
    let mut tracker = ReceiveTracker::new(PROFILE);
    let observation = tracker.observe_datagram(&bytes).unwrap();
    let state = tracker.state().unwrap();
    let debug_values = [
        format!("{decoded:?}"),
        format!("{prefix:?}"),
        format!("{:?}", prefix.unvalidated_remainder()),
        format!("{observation:?}"),
        format!("{tracker:?}"),
        format!("{state:?}"),
        format!("{:?}", EchoComparison::Matches(state)),
        format!(
            "{:?}",
            EchoComparison::Mismatch {
                expected: state,
                observed_bit_31: true,
            }
        ),
        format!(
            "{:?}",
            EchoComparison::Unobserved {
                echoed_counter: state.counter(),
            }
        ),
    ];
    for debug in debug_values {
        assert!(!debug.contains("SECRET"));
        assert!(!debug.contains("ABCD"));
        assert!(!debug.contains("EFGH"));
        assert!(!debug.contains(&first_word.to_string()));
        assert!(!debug.contains(&second_word.to_string()));
        assert!(!debug.contains(&state.counter().to_string()));
        assert!(!debug.contains("83, 69, 67"));
    }
    for marker in [[0xFF; 4], [0xFE, 0xFF, 0xFF, 0xFF]] {
        let mut bytes = marker.to_vec();
        bytes.extend_from_slice(REMAINDER);
        let decoded = decode_datagram(&bytes, PROFILE).unwrap();
        assert!(!format!("{decoded:?}").contains("SECRET"));
    }
    for error in [
        ChannelError::DatagramLimit,
        ChannelError::TruncatedPrefix,
        ChannelError::FirstCounterGuard,
        ChannelError::EchoCounterGuard,
        ChannelError::ConnectionlessDatagram,
        ChannelError::SplitDatagram,
    ] {
        assert!(!format!("{error:?}").contains("SECRET"));
        assert!(!format!("{error}").contains("SECRET"));
    }
}
