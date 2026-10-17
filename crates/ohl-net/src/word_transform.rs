//! Experimental complete-word transform from reviewed synthetic observations.
//!
//! The caller explicitly selects the dated reference cohort. The protocol-48
//! profile was observed on 2026-10-16 between official HLTV and HLDS, app 90,
//! build 15961496, only in the HLTV-to-HLDS direction. The protocol-46 profiles
//! use the Linux 3.1.1.1 base plus d-update spectator cohort observed 2026-10-17;
//! native output reported
//! protocol 46/version 3.1.1.1 without an exact build. Separate explicit selectors
//! cover HLTV-to-HLDS and HLDS-to-HLTV with first-word bit 30 clear. No reverse
//! protocol-48 profile is established. Header extent, message boundaries and
//! authenticated channels remain unresolved, as does ordinary-player
//! interoperability.
//!
//! The independently authored mathematical specification is recorded in the
//! repository's `docs/GOLDSRC_TRANSFORM.md`. Its functional coefficients come
//! only from project-authored synthetic command experiments; no engine, SDK,
//! reimplementation source, disassembly, game assets, or packet fixtures were
//! used as implementation material. The frozen model matched 60 fully held-out
//! synthetic texts of 96 through 103 bytes at 57 distinct low-counter bytes.
//! The tests below the module boundary use independently authored data only.
//!
//! Without fitting, the same frozen model matched 60/60 new protocol-46
//! synthetic texts (5,962 known bytes, 96..103 bytes/text, 60 distinct key bytes
//! and all 16 fully known mask positions). Two earlier command-sized candidates
//! were excluded. Complete-capture SHA-256 is
//! `711e5f5fce9d2a9cc3432332f2aa7408716dd83fa543e7214813fcb01e29da2c`.
//! Forward known text occupied absolute UDP offsets 14..117 exclusive; no known
//! text entered a partial-word tail. This is a synthesized-envelope application
//! relay observation, not NIC acquisition or evidence for every protocol-46 peer.
//!
//! The separately reviewed protocol-46 HLDS-to-HLTV holdout predeclared 60 new
//! texts of 24..31 bytes and their exact windows before one native pair started.
//! All 60 full texts appeared in both endpoint consoles and matched the unchanged
//! model at absolute offset 18, ending at offsets 42..49 exclusive: 1,642 known
//! bytes, 358 wholly known words, seven whole mask positions, 31 byte positions
//! and 60 distinct low-counter bytes. Complete-capture SHA-256 is
//! `c7fd6f782eca67abe22a54f8d2a00244320bdcd5203f71a073aa9cb3f40eaae9`.
//! No known text entered a partial-word tail. A preceding pilot's 63-byte prefix
//! endpoints were measured after that run; they are separate from this holdout.
//!
//! Initial offsets 8..23 for protocol-48 forward, 8..13 for protocol-46 forward
//! and 8..17 for protocol-46 reverse, other positions outside each measured
//! cohort, higher guarded counters, other key bytes and longer periods
//! extrapolate the model. The prefix selector cannot establish direction;
//! callers must select it explicitly without automatic detection or fallback.
//! The 1400-byte datagram cap and counter guard are project bounds. They do not
//! establish compatibility throughout those ranges. Incomplete final words
//! were not validated with held-out known text and remain explicitly opaque.
//!
//! Only complete four-byte words after the selected eight-byte prefix view are
//! transformed. Decoding returns those words separately from the original
//! trailing zero through three bytes. Encoding accepts complete words only;
//! it adds no padding, prefix, tail, framing, transport, or reliability state.
//! Neither operation interprets or passes its result to a message reader.
//! All validation happens before output is written, and unused output remains
//! untouched. Automatic Debug output redacts bytes and functional coefficients.

use std::fmt;

use crate::channel::{
    MAX_CHANNEL_DATAGRAM_BYTES, OBSERVED_PREFIX_BYTES, PacketPrefix, PrefixProfile,
};

/// Maximum complete-word bytes after the selected prefix, a project bound.
pub const MAX_FULL_WORD_BYTES: usize = MAX_CHANNEL_DATAGRAM_BYTES - OBSERVED_PREFIX_BYTES;
const WORD_BYTES: usize = 4;
const POSITION_PERIOD: usize = 64;
const PERMUTATION: [usize; WORD_BYTES] = [3, 2, 1, 0];

const POSITION_MASKS: [[u8; WORD_BYTES]; POSITION_PERIOD / WORD_BYTES] = [
    [0x00, 0x58, 0x50, 0x00],
    [0x5A, 0x58, 0x10, 0x00],
    [0x5A, 0x18, 0x00, 0x00],
    [0x1A, 0x00, 0x10, 0x40],
    [0x00, 0x10, 0x40, 0x00],
    [0x12, 0x40, 0x10, 0x40],
    [0x40, 0x10, 0x50, 0x40],
    [0x10, 0x50, 0x40, 0x00],
    [0x52, 0x40, 0x10, 0x00],
    [0x40, 0x10, 0x00, 0x00],
    [0x10, 0x08, 0x10, 0x00],
    [0x0A, 0x18, 0x10, 0x40],
    [0x1A, 0x18, 0x40, 0x40],
    [0x18, 0x48, 0x50, 0x00],
    [0x4A, 0x50, 0x00, 0x40],
    [0x50, 0x00, 0x50, 0x40],
];

/// Explicit observation cohort; no version, direction, or framing detection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordTransformProfile {
    /// Official HLTV-to-HLDS, protocol 48, observed 2026-10-16; app 90/build 15961496.
    ExperimentalProtocol48HltvToHlds,
    /// Linux base plus d-update HLTV-to-HLDS cohort, observed 2026-10-17.
    /// Protocol 46/native version 3.1.1.1; not a normal-player or server direction.
    ExperimentalProtocol46Linux3111dHltvToHlds,
    /// Linux base plus d-update HLDS-to-HLTV cohort, observed 2026-10-17.
    /// Explicit reverse direction; the shared protocol-46 prefix cannot prove it.
    /// Known text at 18..49 exclusive; other positions and unmeasured keys infer
    /// the model. No ordinary-player, tail, framing or protocol-48 reverse claim.
    ExperimentalProtocol46Linux3111dHldsToHltv,
}

/// Complete transformed words, with explicit byte access and redacted Debug.
///
/// Depending on the operation, these are candidate decoded words or encoded
/// words. Neither interpretation establishes field meanings or authentication.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct FullWords<'a>(&'a [u8]);

impl<'a> FullWords<'a> {
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

impl fmt::Debug for FullWords<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FullWords")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Original trailing zero through three bytes; their plaintext is unvalidated.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OpaqueTail<'a>(&'a [u8]);

impl<'a> OpaqueTail<'a> {
    /// Explicit access to original opaque bytes, not decoded plaintext.
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

impl fmt::Debug for OpaqueTail<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueTail")
            .field("length", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Candidate decoded complete words and a separately borrowed opaque tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedFullWords<'output, 'packet> {
    words: FullWords<'output>,
    opaque_tail: OpaqueTail<'packet>,
}

impl<'output, 'packet> DecodedFullWords<'output, 'packet> {
    pub const fn complete_words(self) -> FullWords<'output> {
        self.words
    }

    pub const fn opaque_tail(self) -> OpaqueTail<'packet> {
        self.opaque_tail
    }
}

/// Fixed diagnostic categories without input, output, counter, or table data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordTransformError {
    FirstBit30Set,
    OutputTooSmall,
    PartialWordInput,
    DatagramLimit,
    ProfileMismatch,
}

impl fmt::Display for WordTransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FirstBit30Set => "first-word bit 30 is outside the experimental word profile",
            Self::OutputTooSmall => "experimental word output buffer is too small",
            Self::PartialWordInput => "experimental word encoding requires complete words",
            Self::DatagramLimit => "experimental word datagram limit exceeded",
            Self::ProfileMismatch => "experimental word and prefix profiles do not match",
        })
    }
}

impl std::error::Error for WordTransformError {}

fn validate_context(
    profile: WordTransformProfile,
    prefix: PacketPrefix<'_>,
) -> Result<u8, WordTransformError> {
    let expected_prefix = match profile {
        WordTransformProfile::ExperimentalProtocol48HltvToHlds => {
            PrefixProfile::ExperimentalProtocol48
        }
        WordTransformProfile::ExperimentalProtocol46Linux3111dHltvToHlds
        | WordTransformProfile::ExperimentalProtocol46Linux3111dHldsToHltv => {
            PrefixProfile::ExperimentalProtocol46Linux3111d
        }
    };
    if prefix.profile() != expected_prefix {
        return Err(WordTransformError::ProfileMismatch);
    }
    if prefix.first_bit_30() {
        return Err(WordTransformError::FirstBit30Set);
    }
    // PacketPrefix construction already enforces the shared datagram and both
    // candidate-counter guards. Bit 31 and second-word bit 30 do not key words.
    Ok(prefix.first_counter().to_le_bytes()[0])
}

/// Decode complete words after the selected prefix into caller-owned output.
///
/// The guarded prefix enforces the shared 1400-byte datagram/counter limits.
/// First-word bit 30 must be clear; no meaning is assigned to second-word bit
/// 30 or either bit 31. The original prefix and tail are never written to output.
/// Trailing bytes remain borrowed, opaque and separate. Errors preserve all
/// output bytes, and success writes only the returned complete-word region.
/// The prefix protocol selector must match. It does not identify direction;
/// the caller explicitly selects the word profile's observation cohort.
pub fn decode_full_words<'output, 'packet>(
    profile: WordTransformProfile,
    prefix: PacketPrefix<'packet>,
    output: &'output mut [u8],
) -> Result<DecodedFullWords<'output, 'packet>, WordTransformError> {
    let q = validate_context(profile, prefix)?;
    let source = prefix.unvalidated_remainder().as_bytes();
    let full_length = source.len() / WORD_BYTES * WORD_BYTES;
    if output.len() < full_length {
        return Err(WordTransformError::OutputTooSmall);
    }
    let (source_words, tail) = source.split_at(full_length);
    let written = &mut output[..full_length];
    let key = [q, 0, 0, q];
    for (index, (cipher, plain)) in source_words
        .as_chunks::<WORD_BYTES>()
        .0
        .iter()
        .zip(written.as_chunks_mut::<WORD_BYTES>().0.iter_mut())
        .enumerate()
    {
        let absolute = OBSERVED_PREFIX_BYTES + index * WORD_BYTES;
        let mask = POSITION_MASKS[(absolute % POSITION_PERIOD) / WORD_BYTES];
        for j in 0..WORD_BYTES {
            plain[PERMUTATION[j]] = cipher[j] ^ mask[j] ^ key[j];
        }
    }
    Ok(DecodedFullWords {
        words: FullWords(written),
        opaque_tail: OpaqueTail(tail),
    })
}

/// Encode complete words using a guarded prefix only as key/flag context.
///
/// The prefix's old remainder length is unrelated to the new input length.
/// Nothing copies or generates that prefix or its old remainder. Inputs must
/// be a multiple of four bytes and fit after the selected prefix within the
/// shared datagram cap. No partial tail or padding is accepted. Errors preserve
/// all output bytes; success writes exactly the returned encoded-word region.
/// The prefix protocol selector must match; direction remains caller-selected.
/// This mathematical inverse does not establish that an endpoint consumes it.
pub fn encode_full_words<'output>(
    profile: WordTransformProfile,
    prefix_context: PacketPrefix<'_>,
    complete_words: &[u8],
    output: &'output mut [u8],
) -> Result<FullWords<'output>, WordTransformError> {
    let q = validate_context(profile, prefix_context)?;
    if complete_words.len() > MAX_FULL_WORD_BYTES {
        return Err(WordTransformError::DatagramLimit);
    }
    if !complete_words.len().is_multiple_of(WORD_BYTES) {
        return Err(WordTransformError::PartialWordInput);
    }
    if output.len() < complete_words.len() {
        return Err(WordTransformError::OutputTooSmall);
    }
    let written = &mut output[..complete_words.len()];
    let key = [q, 0, 0, q];
    for (index, (plain, cipher)) in complete_words
        .as_chunks::<WORD_BYTES>()
        .0
        .iter()
        .zip(written.as_chunks_mut::<WORD_BYTES>().0.iter_mut())
        .enumerate()
    {
        let absolute = OBSERVED_PREFIX_BYTES + index * WORD_BYTES;
        let mask = POSITION_MASKS[(absolute % POSITION_PERIOD) / WORD_BYTES];
        for j in 0..WORD_BYTES {
            cipher[j] = plain[PERMUTATION[j]] ^ mask[j] ^ key[j];
        }
    }
    Ok(FullWords(written))
}
