//! Explicit experimental prefix cohorts, not a complete network channel.
//!
//! For the historical protocol-48 cohort, the selected eight-byte view contains
//! two little-endian words at offsets 0 and 4. An independently reviewed model
//! accepted strictly increasing
//! first candidate counters. First-word bit 31 toggled an initially false state
//! only on accepted advances. At the exact accepted counter referenced by an
//! opposite accepted packet's second word, that state matched second-word bit
//! 31 in 5,342 of 5,342 eligible observations across six controlled runs and one
//! held-out run. Seven unseen references were excluded. Mirroring a referenced
//! packet's first bit 31 instead had 777 contradictions, even allowing fixed
//! inversion per run/direction. See the repository's `docs/GOLDSRC_PROTOCOL.md`
//! experimental packet-prefix evidence subsection for the reviewed evidence.
//!
//! A separate 2026-10-17 Linux HLDS 3.1.1.1 base plus d-update spectator
//! cohort reported protocol 46 and native version 3.1.1.1. The unchanged
//! candidate matched 733/733 and 708/708 eligible echoes in its two runs;
//! one unseen initial reference per run was excluded. Both spectator directions
//! were measured. Complete-capture SHA-256 values are
//! `ed6097fd53b796c2d4fff8502a1dcb4af6b62817216d0efbfbc7a970b2e84471`
//! and `711e5f5fce9d2a9cc3432332f2aa7408716dd83fa543e7214813fcb01e29da2c`.
//! These are application-relay observations with synthesized envelopes and
//! timestamps, not NIC acquisition or proof for all protocol-46 implementations.
//! Profile identities remain distinct even where their candidate rules agree.
//!
//! Both candidate counters use a low-30-bit projection, with bit 30 retained
//! separately as opaque. The observations do not distinguish 29 from 30 counter
//! bits, so both profiles reject either projected counter at or above 2^29.
//! This is a conservative project guard, not a conclusion about actual counter
//! width. Wrap, resets, stream association, packet authentication, fragment
//! layout, reliable delivery, transforms, and any header extent beyond this
//! selected prefix remain unimplemented. No send path is provided.
//!
//! The returned remainder is **unvalidated**. It may contain more header bytes,
//! framing, or transformed data. It is never passed to the server-message reader
//! automatically. A successfully decoded prefix is not an authenticated packet
//! or a gameplay connection. Callers must select the experimental profile and
//! associate both directions externally; the tracker cannot identify peers or
//! distinguish a new stream from a reused endpoint.
//!
//! Connectionless/split marker recognition uses only the public Valve [query
//! framing]. Those datagrams are classified without interpreting their bodies.
//! All tests are independently authored synthetic bytes, not packet captures.
//! Debug output redacts raw prefix words, packet-specific state values, and all
//! opaque remainders.
//!
//! [query framing]: https://web.archive.org/web/20230831123627id_/https://developer.valvesoftware.com/wiki/Server_queries#Protocol

use std::fmt;

use crate::{SINGLE_HEADER, SPLIT_HEADER};

/// Project datagram cap, shared with the bounded query path.
pub const MAX_CHANNEL_DATAGRAM_BYTES: usize = crate::MAX_DATAGRAM_BYTES;
/// Bytes in the selected prefix view; this is not a complete header length.
pub const OBSERVED_PREFIX_BYTES: usize = 8;
/// Exclusive project guard while the true counter width remains unresolved.
pub const EXPERIMENTAL_COUNTER_LIMIT: u32 = 1 << 29;
/// Fixed number of exact accepted-counter states retained for echo comparisons.
pub const MAX_ACCEPTED_STATES: usize = 512;

const COUNTER_PROJECTION: u32 = 0x3FFF_FFFF;
const OPAQUE_BIT_30: u32 = 1 << 30;
const MARKER_BIT_31: u32 = 1 << 31;

/// Caller-selected observation profile. No engine/version detection occurs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrefixProfile {
    /// Reviewed protocol-48 prefix model with a conservative counter guard.
    ExperimentalProtocol48,
    /// Linux 3.1.1.1 base plus d-update spectator cohort, observed 2026-10-17.
    /// Native output reported protocol 46/version 3.1.1.1, not an exact build.
    ExperimentalProtocol46Linux3111d,
}

/// Borrowed opaque bytes whose automatic Debug representation is redacted.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OpaqueRemainder<'a>(&'a [u8]);

impl<'a> OpaqueRemainder<'a> {
    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub const fn len(self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for OpaqueRemainder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueRemainder")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// A guarded selected prefix; its remainder is not a validated svc boundary.
///
/// Accessors expose observations, not reliable-delivery or authentication state.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PacketPrefix<'a> {
    profile: PrefixProfile,
    first_word: u32,
    second_word: u32,
    remainder: OpaqueRemainder<'a>,
}

impl<'a> PacketPrefix<'a> {
    pub const fn profile(self) -> PrefixProfile {
        self.profile
    }

    pub const fn first_counter(self) -> u32 {
        self.first_word & COUNTER_PROJECTION
    }

    pub const fn echoed_counter(self) -> u32 {
        self.second_word & COUNTER_PROJECTION
    }

    pub const fn first_bit_31(self) -> bool {
        self.first_word & MARKER_BIT_31 != 0
    }

    pub const fn echoed_bit_31(self) -> bool {
        self.second_word & MARKER_BIT_31 != 0
    }

    /// Preserved opaque flag; no fragment or reliability meaning is assigned.
    pub const fn first_bit_30(self) -> bool {
        self.first_word & OPAQUE_BIT_30 != 0
    }

    /// Preserved opaque flag; no fragment or reliability meaning is assigned.
    pub const fn second_bit_30(self) -> bool {
        self.second_word & OPAQUE_BIT_30 != 0
    }

    /// Explicit raw access for a caller operating within its own data boundary.
    pub const fn raw_words(self) -> [u32; 2] {
        [self.first_word, self.second_word]
    }

    /// Bytes after the selected view, without a claim about their interpretation.
    pub const fn unvalidated_remainder(self) -> OpaqueRemainder<'a> {
        self.remainder
    }
}

impl fmt::Debug for PacketPrefix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PacketPrefix")
            .field("profile", &self.profile)
            .field("unvalidated_remainder", &self.remainder)
            .finish_non_exhaustive()
    }
}

/// Prefix classification only; connectionless and split bodies are not parsed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodedDatagram<'a> {
    /// Bytes after the documented single-packet connectionless marker.
    Connectionless(OpaqueRemainder<'a>),
    /// Bytes after the documented split marker, without fragment interpretation.
    Split(OpaqueRemainder<'a>),
    /// The selected experimental view, with an unvalidated remainder.
    Packet(PacketPrefix<'a>),
}

/// Fixed diagnostic categories that retain no datagram data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    DatagramLimit,
    TruncatedPrefix,
    FirstCounterGuard,
    EchoCounterGuard,
    ConnectionlessDatagram,
    SplitDatagram,
    ProfileMismatch,
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DatagramLimit => "experimental prefix datagram limit exceeded",
            Self::TruncatedPrefix => "truncated experimental packet prefix",
            Self::FirstCounterGuard => "experimental first-counter guard exceeded",
            Self::EchoCounterGuard => "experimental echo-counter guard exceeded",
            Self::ConnectionlessDatagram => "connectionless datagram is outside receive tracking",
            Self::SplitDatagram => "split datagram is outside receive tracking",
            Self::ProfileMismatch => "experimental packet prefix profile mismatch",
        })
    }
}

impl std::error::Error for ChannelError {}

/// Decode only the selected prefix, never a channel or server-message payload.
///
/// Standard markers are recognized before the experimental packet path. A marker
/// alone can be classified; this says nothing about the validity of its body.
pub fn decode_datagram(
    datagram: &[u8],
    profile: PrefixProfile,
) -> Result<DecodedDatagram<'_>, ChannelError> {
    if datagram.len() > MAX_CHANNEL_DATAGRAM_BYTES {
        return Err(ChannelError::DatagramLimit);
    }
    let marker = datagram.get(..4).ok_or(ChannelError::TruncatedPrefix)?;
    if marker == SINGLE_HEADER {
        return Ok(DecodedDatagram::Connectionless(OpaqueRemainder(
            &datagram[4..],
        )));
    }
    if marker == SPLIT_HEADER {
        return Ok(DecodedDatagram::Split(OpaqueRemainder(&datagram[4..])));
    }
    match profile {
        PrefixProfile::ExperimentalProtocol48 | PrefixProfile::ExperimentalProtocol46Linux3111d => {
            let prefix = datagram
                .get(..OBSERVED_PREFIX_BYTES)
                .ok_or(ChannelError::TruncatedPrefix)?;
            let first_word = u32::from_le_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
            let second_word = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]);
            if first_word & COUNTER_PROJECTION >= EXPERIMENTAL_COUNTER_LIMIT {
                return Err(ChannelError::FirstCounterGuard);
            }
            if second_word & COUNTER_PROJECTION >= EXPERIMENTAL_COUNTER_LIMIT {
                return Err(ChannelError::EchoCounterGuard);
            }
            Ok(DecodedDatagram::Packet(PacketPrefix {
                profile,
                first_word,
                second_word,
                remainder: OpaqueRemainder(&datagram[OBSERVED_PREFIX_BYTES..]),
            }))
        }
    }
}

/// The model's state at one exact accepted first counter.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ReceiveState {
    counter: u32,
    parity: bool,
}

impl fmt::Debug for ReceiveState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReceiveState { .. }")
    }
}

impl ReceiveState {
    pub const fn counter(self) -> u32 {
        self.counter
    }

    pub const fn parity(self) -> bool {
        self.parity
    }
}

/// Project receive policy, derived from the reviewed prefix observations only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveDisposition {
    Advanced(ReceiveState),
    Duplicate,
    Stale,
}

/// A prefix and its local receive-policy disposition; no remainder is executed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveObservation<'a> {
    prefix: PacketPrefix<'a>,
    disposition: ReceiveDisposition,
}

impl<'a> ReceiveObservation<'a> {
    pub const fn prefix(self) -> PacketPrefix<'a> {
        self.prefix
    }

    pub const fn disposition(self) -> ReceiveDisposition {
        self.disposition
    }
}

/// Read-only comparison against one exact historical counter in this tracker.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum EchoComparison {
    Matches(ReceiveState),
    Mismatch {
        expected: ReceiveState,
        observed_bit_31: bool,
    },
    /// The counter was never accepted here, or its state has been evicted.
    ///
    /// A bounded tracker cannot distinguish those cases after eviction. This is
    /// an exclusion, not a protocol error or a reason to reset receiver state.
    Unobserved {
        echoed_counter: u32,
    },
    /// Duplicate/stale opposite observations do not contribute echo evidence.
    NotAdvanced,
    /// The opposite observation belongs to a different caller-selected cohort.
    ProfileMismatch,
}

impl fmt::Debug for EchoComparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Matches(_) => "Matches(..)",
            Self::Mismatch { .. } => "Mismatch { .. }",
            Self::Unobserved { .. } => "Unobserved { .. }",
            Self::NotAdvanced => "NotAdvanced",
            Self::ProfileMismatch => "ProfileMismatch",
        })
    }
}

/// One externally associated direction's high-water and toggle model.
///
/// The initial parity is false and the first observed guarded counter advances
/// the state, including counter zero. Gaps are permitted without synthesizing
/// state for missing counters. Equal/lower counters never toggle state or enter
/// history. No wrap comparison or automatic reset occurs.
///
/// Exact accepted states use a fixed ring of [`MAX_ACCEPTED_STATES`] entries.
/// Old entries are evicted, so echo validation can become `Unobserved`. Creating
/// a new tracker requires the caller's independent stream-association decision;
/// the prefix view itself provides no evidence of a new authenticated session.
#[derive(Clone, Eq, PartialEq)]
pub struct ReceiveTracker {
    profile: PrefixProfile,
    state: Option<ReceiveState>,
    history: [Option<ReceiveState>; MAX_ACCEPTED_STATES],
    next_slot: usize,
    history_len: usize,
}

impl ReceiveTracker {
    pub const fn new(profile: PrefixProfile) -> Self {
        Self {
            profile,
            state: None,
            history: [None; MAX_ACCEPTED_STATES],
            next_slot: 0,
            history_len: 0,
        }
    }

    pub const fn state(&self) -> Option<ReceiveState> {
        self.state
    }

    pub const fn retained_states(&self) -> usize {
        self.history_len
    }

    pub const fn profile(&self) -> PrefixProfile {
        self.profile
    }

    /// Decode and observe atomically; errors leave the entire tracker unchanged.
    ///
    /// Connectionless/split packets are outside this policy and do not mutate
    /// the tracker. Use [`decode_datagram`] to classify and route those packets.
    pub fn observe_datagram<'a>(
        &mut self,
        datagram: &'a [u8],
    ) -> Result<ReceiveObservation<'a>, ChannelError> {
        match decode_datagram(datagram, self.profile)? {
            DecodedDatagram::Packet(prefix) => self.observe_prefix(prefix),
            DecodedDatagram::Connectionless(_) => Err(ChannelError::ConnectionlessDatagram),
            DecodedDatagram::Split(_) => Err(ChannelError::SplitDatagram),
        }
    }

    /// Apply only the high-water/toggle policy to an already guarded prefix.
    /// A different profile is rejected before any state or history mutation.
    pub fn observe_prefix<'a>(
        &mut self,
        prefix: PacketPrefix<'a>,
    ) -> Result<ReceiveObservation<'a>, ChannelError> {
        if prefix.profile() != self.profile {
            return Err(ChannelError::ProfileMismatch);
        }
        let counter = prefix.first_counter();
        let disposition = match self.state {
            Some(state) if counter == state.counter => ReceiveDisposition::Duplicate,
            Some(state) if counter < state.counter => ReceiveDisposition::Stale,
            _ => {
                let previous_parity = self.state.is_some_and(|state| state.parity);
                let state = ReceiveState {
                    counter,
                    parity: previous_parity ^ prefix.first_bit_31(),
                };
                self.history[self.next_slot] = Some(state);
                self.next_slot = (self.next_slot + 1) % MAX_ACCEPTED_STATES;
                self.history_len = (self.history_len + 1).min(MAX_ACCEPTED_STATES);
                self.state = Some(state);
                ReceiveDisposition::Advanced(state)
            }
        };
        Ok(ReceiveObservation {
            prefix,
            disposition,
        })
    }

    /// Compare an accepted observation from the opposite associated direction.
    ///
    /// The caller supplies the direction/stream association. Only an exact
    /// retained first-counter state is used; never the latest arrival's state.
    /// This function is diagnostic and does not accept/reject or mutate packets.
    /// Different-profile observations are rejected even when not advanced.
    pub fn compare_echo(&self, opposite: &ReceiveObservation<'_>) -> EchoComparison {
        if opposite.prefix.profile() != self.profile {
            return EchoComparison::ProfileMismatch;
        }
        if !matches!(opposite.disposition, ReceiveDisposition::Advanced(_)) {
            return EchoComparison::NotAdvanced;
        }
        let counter = opposite.prefix.echoed_counter();
        let Some(expected) = self
            .history
            .iter()
            .flatten()
            .find(|state| state.counter == counter)
            .copied()
        else {
            return EchoComparison::Unobserved {
                echoed_counter: counter,
            };
        };
        let observed = opposite.prefix.echoed_bit_31();
        if expected.parity == observed {
            EchoComparison::Matches(expected)
        } else {
            EchoComparison::Mismatch {
                expected,
                observed_bit_31: observed,
            }
        }
    }
}

impl fmt::Debug for ReceiveTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveTracker")
            .field("profile", &self.profile)
            .field("state", &self.state)
            .field("retained_states", &self.history_len)
            .finish_non_exhaustive()
    }
}
