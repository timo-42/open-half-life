//! Experimental complete-word transform from reviewed synthetic observations.
//!
//! The caller explicitly selects the profile observed on 2026-10-16 between the
//! official protocol-48 HLTV proxy and HLDS, app 90, build 15961496. Only the
//! proxy-to-server direction with first-word bit 30 clear was tested. This is
//! not a server-to-client transform, a complete header, a message boundary, an
//! authenticated channel, or evidence of ordinary-player interoperability.
//!
//! The independently authored mathematical specification is recorded in the
//! repository's `docs/GOLDSRC_TRANSFORM.md`. Its functional coefficients come
//! only from project-authored synthetic command experiments; no engine, SDK,
//! reimplementation source, disassembly, game assets, or packet fixtures were
//! used as implementation material. The frozen model matched 60 fully held-out
//! synthetic texts of 96 through 103 bytes at 57 distinct low-counter bytes.
//! The tests below the module boundary use independently authored data only.
//!
//! Operations on initial word positions 8 through 23, higher guarded counters,
//! all other key bytes, and longer period repetitions extrapolate that model.
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

use crate::channel::{MAX_CHANNEL_DATAGRAM_BYTES, OBSERVED_PREFIX_BYTES, PacketPrefix};

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
}

impl fmt::Display for WordTransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FirstBit30Set => "first-word bit 30 is outside the experimental word profile",
            Self::OutputTooSmall => "experimental word output buffer is too small",
            Self::PartialWordInput => "experimental word encoding requires complete words",
            Self::DatagramLimit => "experimental word datagram limit exceeded",
        })
    }
}

impl std::error::Error for WordTransformError {}

fn validate_context(
    profile: WordTransformProfile,
    prefix: PacketPrefix<'_>,
) -> Result<u8, WordTransformError> {
    match profile {
        WordTransformProfile::ExperimentalProtocol48HltvToHlds => {
            if prefix.first_bit_30() {
                return Err(WordTransformError::FirstBit30Set);
            }
            // PacketPrefix construction already enforces the shared datagram
            // and both candidate-counter guards. Bit 31 and second-word bit 30
            // are preserved by its owner; neither participates in this key.
            Ok(prefix.first_counter().to_le_bytes()[0])
        }
    }
}

/// Decode complete words after the selected prefix into caller-owned output.
///
/// The guarded prefix enforces the shared 1400-byte datagram/counter limits.
/// First-word bit 30 must be clear; no meaning is assigned to second-word bit
/// 30 or either bit 31. The original prefix and tail are never written to output.
/// Trailing bytes remain borrowed, opaque and separate. Errors preserve all
/// output bytes, and success writes only the returned complete-word region.
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
