//! Bounded experimental view of one observed protocol-46 flagged packet grammar.
//!
//! This explicitly selected Linux 3.1.1.1 base plus d-update HLDS-to-HLTV
//! spectator cohort was observed on 2026-10-17. Native output reported protocol
//! 46/version 3.1.1.1 without an exact build. A separately frozen header candidate
//! matched 25 server-to-proxy packets in one fresh run, forming three candidate
//! index sequences. An unchanged complete-word transform predicted one full
//! project-authored 48-byte control: 11 wholly known words, 11 whole mask slots,
//! 48 byte positions and one low-counter byte. The complete capture SHA-256 is
//! `c74c00416d5a65acb7e744e8f8f3d46d704ff2d4da05a85b84fb5b43d13779d2`.
//! All 51 partial-word bytes remained opaque. These application-relay
//! observations do not establish every byte, key, position, or guarded counter.
//! The only known text was at absolute UDP 97..145 exclusive, with complete-word
//! end 1040 and two unknown trailing original bytes. Twenty-four other selected
//! candidates had no whole-token hit. Header consistency establishes no other
//! body plaintext; other positions and keys extrapolate the unchanged model.
//!
//! [`decode_body`] couples this candidate grammar to that exact word profile.
//! It requires a caller-guarded protocol-46 prefix with first-word bit 30 set.
//! The same prefix cannot identify direction; the caller chooses the cohort.
//! The selected eight-byte prefix remains a view, not a complete header. After
//! word decoding, absolute UDP offsets 8 and 17 must contain 1 and 0; the
//! little-endian words at 9, 11, 13 and 15 are candidate count, one-based index,
//! observed reserved zero, and declared body length. The latter includes any
//! opaque tail and must equal the original datagram length minus 18.
//!
//! The header must lie entirely inside complete decoded words, whose absolute
//! end F is at least 20. Only UDP bytes [18,F) are returned as candidate body.
//! The final zero through three original bytes are borrowed separately and
//! never interpreted as plaintext or padding. The 1400-byte datagram bound,
//! conservative counter guards and 4096-count bound are project choices, not
//! original engine maxima. Other positions, keys, higher guarded counters and
//! longer periods extrapolate the frozen mathematical model described in
//! `docs/GOLDSRC_TRANSFORM.md`.
//!
//! No concatenation, reassembly, group identity, service-message parsing,
//! encoding, socket, reliable state or command interpretation is provided.
//! Metadata meanings beyond this candidate grammar remain unresolved. Output
//! is published only after all header and buffer validation. All errors leave
//! the complete caller buffer untouched; success leaves its unused suffix
//! untouched. Automatic diagnostics redact body, tail and packet metadata.
//! Implementation and test data are independently authored; no native packet
//! slices, resources, engine source or other implementation were used.

use std::fmt;

use crate::{
    channel::{MAX_CHANNEL_DATAGRAM_BYTES, OBSERVED_PREFIX_BYTES, PacketPrefix},
    word_transform::{
        MAX_FULL_WORD_BYTES, OpaqueTail, WordTransformError, WordTransformProfile,
        decode_full_words,
    },
};

/// Absolute start of the candidate body, not a complete transport-header claim.
pub const CANDIDATE_BODY_OFFSET: usize = 18;
/// Smallest absolute complete-word end that contains the entire candidate header.
pub const MIN_COMPLETE_HEADER_END: usize = 20;
/// Project guard, not an observed original maximum.
pub const MAX_CANDIDATE_COUNT: u16 = 4096;
/// Largest candidate body that fits in the shared project datagram bound.
pub const MAX_CANDIDATE_BODY_BYTES: usize = MAX_CHANNEL_DATAGRAM_BYTES - CANDIDATE_BODY_OFFSET;

/// Explicit dated observation cohort, without direction or version detection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    /// Linux base plus d-update HLDS-to-HLTV, first bit 30 set, observed 2026-10-17.
    ExperimentalProtocol46Linux3111dHldsToHltvFirstBit30Set,
}

/// Validated candidate metadata, with explicit typed access and redacted Debug.
///
/// Count and index are not a group identity and do not authorize reassembly.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CandidateHeader {
    count: u16,
    index: u16,
    reserved_word: u16,
    declared_body_length: u16,
}

impl CandidateHeader {
    pub const fn count(self) -> u16 {
        self.count
    }

    pub const fn index(self) -> u16 {
        self.index
    }

    /// Observed reserved zero; no offset or other semantics are assigned.
    pub const fn reserved_word(self) -> u16 {
        self.reserved_word
    }

    /// Includes the original opaque tail, which is not returned as body plaintext.
    pub const fn declared_body_length(self) -> u16 {
        self.declared_body_length
    }
}

impl fmt::Debug for CandidateHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CandidateHeader { .. }")
    }
}

/// Candidate complete-word body bytes; this type assigns no payload semantics.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CandidateBody<'a>(&'a [u8]);

impl<'a> CandidateBody<'a> {
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

impl fmt::Debug for CandidateBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CandidateBody")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Validated candidate metadata, copied body and separately borrowed opaque tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedBody<'output, 'packet> {
    header: CandidateHeader,
    body: CandidateBody<'output>,
    opaque_tail: OpaqueTail<'packet>,
}

impl<'output, 'packet> DecodedBody<'output, 'packet> {
    pub const fn header(self) -> CandidateHeader {
        self.header
    }

    pub const fn body(self) -> CandidateBody<'output> {
        self.body
    }

    pub const fn opaque_tail(self) -> OpaqueTail<'packet> {
        self.opaque_tail
    }
}

/// Fixed diagnostics without packet bytes, metadata values or counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    WordTransform(WordTransformError),
    IncompleteHeader,
    FixedFields,
    CountLimit,
    IndexRange,
    BodyLength,
    OutputTooSmall,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WordTransform(_) => "experimental flagged word profile rejected the packet",
            Self::IncompleteHeader => "candidate header is not inside complete decoded words",
            Self::FixedFields => "experimental flagged candidate fixed fields disagree",
            Self::CountLimit => "experimental flagged candidate count guard exceeded",
            Self::IndexRange => "experimental flagged candidate index is outside its count",
            Self::BodyLength => "experimental flagged candidate body length disagrees",
            Self::OutputTooSmall => "experimental flagged body output buffer is too small",
        })
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WordTransform(error) => Some(error),
            _ => None,
        }
    }
}

/// Decode and validate one candidate packet before publishing body bytes.
///
/// `prefix` must come from the caller-selected guarded protocol-46 prefix
/// decoder. The profile additionally requires first-word bit 30 set. Body and
/// metadata cannot be supplied separately or mixed with arbitrary decoded words.
/// A fixed stack scratch area of 1392 bytes keeps all output atomic on error.
/// The caller buffer needs room only for [18,F), never header or opaque tail.
/// No byte outside the returned body region is written on success.
pub fn decode_body<'output, 'packet>(
    profile: Profile,
    prefix: PacketPrefix<'packet>,
    output: &'output mut [u8],
) -> Result<DecodedBody<'output, 'packet>, Error> {
    let word_profile = match profile {
        Profile::ExperimentalProtocol46Linux3111dHldsToHltvFirstBit30Set => {
            WordTransformProfile::ExperimentalProtocol46Linux3111dHldsToHltvFirstBit30Set
        }
    };
    let mut scratch = [0; MAX_FULL_WORD_BYTES];
    let decoded =
        decode_full_words(word_profile, prefix, &mut scratch).map_err(Error::WordTransform)?;
    let words = decoded.complete_words().as_bytes();
    if words.len() < MIN_COMPLETE_HEADER_END - OBSERVED_PREFIX_BYTES {
        return Err(Error::IncompleteHeader);
    }
    let read_word = |offset: usize| u16::from_le_bytes([words[offset], words[offset + 1]]);
    let header = CandidateHeader {
        count: read_word(1),
        index: read_word(3),
        reserved_word: read_word(5),
        declared_body_length: read_word(7),
    };
    if words[0] != 1 || header.reserved_word != 0 || words[9] != 0 {
        return Err(Error::FixedFields);
    }
    if header.count == 0 || header.count > MAX_CANDIDATE_COUNT {
        return Err(Error::CountLimit);
    }
    if header.index == 0 || header.index > header.count {
        return Err(Error::IndexRange);
    }
    let body_start = CANDIDATE_BODY_OFFSET - OBSERVED_PREFIX_BYTES;
    let original_body_length = prefix.unvalidated_remainder().len() - body_start;
    if usize::from(header.declared_body_length) != original_body_length {
        return Err(Error::BodyLength);
    }
    let body = &words[body_start..];
    if output.len() < body.len() {
        return Err(Error::OutputTooSmall);
    }
    let written = &mut output[..body.len()];
    written.copy_from_slice(body);
    Ok(DecodedBody {
        header,
        body: CandidateBody(written),
        opaque_tail: decoded.opaque_tail(),
    })
}
