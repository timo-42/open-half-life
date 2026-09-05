//! A bounded decoder for PKWARE Data Compression Library "imploded" streams.
//!
//! The format is public: a two-byte header (literal-coding flag, then the
//! base-2 logarithm of the dictionary size minus six), followed by a bit
//! stream of literals and length/distance pairs terminated by an end code.
//! Three fixed Huffman codebooks are built into the format; they are restated
//! here as `(bit length, run length)` tables and checked for completeness by
//! a unit test. See `docs/FORMAT_SOURCES.md` for the sources.
//!
//! Hardening: the decoder is `no_std`, allocation-free apart from the
//! caller's output buffer, never indexes outside its fixed 4 KiB window,
//! refuses a distance that reaches before the start of the stream, refuses a
//! dictionary code outside 4..=6, and is resumable so a caller can feed it
//! bounded input chunks and drain bounded output chunks between cancellation
//! checks.

use alloc::vec::Vec;

use crate::error::{Error, Result};

/// Maximum code length in bits used by the built-in codebooks.
const MAX_BITS: usize = 13;
/// The decoder's sliding window, which is also the largest dictionary.
pub const MAX_WINDOW: usize = 4096;
/// Longest match a single length/distance pair can produce.
pub const MAX_MATCH: usize = 518;
/// The length value reserved as the end-of-stream code.
const END_CODE: u32 = 519;

/// Smallest dictionary code, selecting a 1 KiB window.
pub const MIN_DICTIONARY_CODE: u8 = 4;
/// Largest dictionary code, selecting a 4 KiB window.
pub const MAX_DICTIONARY_CODE: u8 = 6;

/// Bit lengths of the 256 literal codes, run-length encoded.
const LITERAL_CODE_LENGTHS: &[(u8, u16)] = &[
    (11, 1),
    (12, 8),
    (8, 1),
    (7, 1),
    (12, 2),
    (7, 1),
    (12, 12),
    (13, 1),
    (12, 5),
    (4, 1),
    (10, 1),
    (8, 1),
    (12, 1),
    (10, 1),
    (12, 1),
    (10, 1),
    (8, 1),
    (7, 2),
    (8, 1),
    (9, 1),
    (7, 1),
    (6, 1),
    (7, 1),
    (8, 1),
    (7, 1),
    (6, 1),
    (7, 4),
    (8, 1),
    (7, 2),
    (8, 2),
    (12, 1),
    (11, 1),
    (7, 1),
    (9, 1),
    (11, 1),
    (12, 1),
    (6, 1),
    (7, 1),
    (6, 2),
    (5, 1),
    (7, 1),
    (8, 2),
    (6, 1),
    (11, 1),
    (9, 1),
    (6, 1),
    (7, 1),
    (6, 2),
    (7, 1),
    (11, 1),
    (6, 3),
    (7, 1),
    (9, 1),
    (8, 1),
    (9, 2),
    (11, 1),
    (8, 1),
    (11, 1),
    (9, 1),
    (12, 1),
    (8, 1),
    (12, 1),
    (5, 1),
    (6, 3),
    (5, 1),
    (6, 3),
    (5, 1),
    (11, 1),
    (7, 1),
    (5, 1),
    (6, 1),
    (5, 2),
    (6, 1),
    (10, 1),
    (5, 4),
    (8, 1),
    (7, 1),
    (8, 2),
    (10, 1),
    (11, 2),
    (12, 3),
    (13, 48),
    (12, 48),
    (13, 1),
    (12, 1),
    (13, 3),
    (12, 1),
    (13, 3),
    (12, 1),
    (13, 4),
    (12, 1),
    (13, 3),
    (12, 3),
    (13, 11),
];

/// Bit lengths of the 16 length codes, run-length encoded.
const LENGTH_CODE_LENGTHS: &[(u8, u16)] = &[(2, 1), (3, 3), (4, 3), (5, 4), (6, 3), (7, 2)];

/// Bit lengths of the 64 distance codes, run-length encoded.
const DISTANCE_CODE_LENGTHS: &[(u8, u16)] = &[(2, 1), (4, 2), (5, 4), (6, 15), (7, 26), (8, 16)];

/// Base match length for each length code.
const LENGTH_BASE: [u16; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
/// Number of extra bits carried by each length code.
const LENGTH_EXTRA: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

/// A canonical codebook: how many codes exist of each bit length, and the
/// symbols in canonical order.
struct Huffman<const N: usize> {
    count: [u16; MAX_BITS + 1],
    symbol: [u16; N],
}

// `N` is at most 256 and every index below is bounded by it, so the two casts
// in this function cannot truncate. `const fn` cannot use `try_from`.
#[allow(clippy::cast_possible_truncation)]
const fn construct<const N: usize>(runs: &[(u8, u16)]) -> Huffman<N> {
    let mut lengths = [0u8; N];
    let mut filled = 0usize;
    let mut run_index = 0usize;
    while run_index < runs.len() {
        let (bits, repeats) = runs[run_index];
        let mut repeat = 0u16;
        while repeat < repeats {
            lengths[filled] = bits;
            filled += 1;
            repeat += 1;
        }
        run_index += 1;
    }
    assert!(filled == N, "code length table does not cover every symbol");

    let mut count = [0u16; MAX_BITS + 1];
    let mut symbol_index = 0usize;
    while symbol_index < N {
        count[lengths[symbol_index] as usize] += 1;
        symbol_index += 1;
    }

    let mut offsets = [0u16; MAX_BITS + 2];
    let mut bits = 1usize;
    while bits <= MAX_BITS {
        offsets[bits + 1] = offsets[bits] + count[bits];
        bits += 1;
    }

    let mut symbol = [0u16; N];
    let mut symbol_index = 0usize;
    while symbol_index < N {
        let bits = lengths[symbol_index] as usize;
        if bits != 0 {
            symbol[offsets[bits] as usize] = symbol_index as u16;
            offsets[bits] += 1;
        }
        symbol_index += 1;
    }

    Huffman { count, symbol }
}

static LITERAL_CODES: Huffman<256> = construct(LITERAL_CODE_LENGTHS);
static LENGTH_CODES: Huffman<16> = construct(LENGTH_CODE_LENGTHS);
static DISTANCE_CODES: Huffman<64> = construct(DISTANCE_CODE_LENGTHS);

/// Why a token could not be completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenStop {
    /// The token needs bits the caller has not supplied yet.
    NeedMoreInput,
    /// The token is malformed and the stream cannot continue.
    Failed,
}

type Partial<T> = core::result::Result<T, TokenStop>;

/// A bit reader over one caller-supplied input chunk.
///
/// Bits are taken from each byte least-significant bit first.
struct BitReader<'a> {
    input: &'a [u8],
    position: usize,
    buffer: u32,
    count: u32,
}

impl BitReader<'_> {
    fn take(&mut self, need: u32) -> Partial<u32> {
        debug_assert!(need <= 16);
        while self.count < need {
            let Some(byte) = self.input.get(self.position) else {
                return Err(TokenStop::NeedMoreInput);
            };
            self.position += 1;
            self.buffer |= u32::from(*byte) << self.count;
            self.count += 8;
        }
        let value = self.buffer & ((1u32 << need) - 1);
        self.buffer >>= need;
        self.count -= need;
        Ok(value)
    }

    fn decode<const N: usize>(&mut self, codes: &Huffman<N>) -> Partial<u16> {
        // Codes are stored bit-reversed relative to a simple integer
        // ordering, and the first code of the shortest length is all ones,
        // so each bit is inverted to recover the natural ordering.
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for bits in 1..=MAX_BITS {
            // Each stream bit is inverted, so a zero bit contributes one.
            code |= i32::from(self.take(1)? == 0);
            let count = i32::from(codes.count[bits]);
            if code < first + count {
                let at = usize::try_from(index + (code - first)).map_err(|_| TokenStop::Failed)?;
                return codes.symbol.get(at).copied().ok_or(TokenStop::Failed);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        // Unreachable for a complete codebook, but never a panic.
        Err(TokenStop::Failed)
    }
}

/// What one [`Explode::decode`] call achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Input bytes consumed from the chunk that was supplied.
    pub consumed: usize,
    /// Whether the end-of-stream code has now been decoded.
    pub finished: bool,
}

/// A resumable PKWARE DCL explode decoder.
#[derive(Debug)]
pub struct Explode {
    coded_literals: bool,
    distance_bits: u32,
    window: [u8; MAX_WINDOW],
    window_position: usize,
    total_output: u64,
    saved_buffer: u32,
    saved_count: u32,
    header_read: bool,
    finished: bool,
}

impl Default for Explode {
    fn default() -> Self {
        Self::new()
    }
}

impl Explode {
    /// A decoder awaiting the two-byte stream header.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            coded_literals: false,
            distance_bits: 0,
            window: [0u8; MAX_WINDOW],
            window_position: 0,
            total_output: 0,
            saved_buffer: 0,
            saved_count: 0,
            header_read: false,
            finished: false,
        }
    }

    /// Whether the end-of-stream code has been decoded.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Total bytes produced so far.
    #[must_use]
    pub const fn total_output(&self) -> u64 {
        self.total_output
    }

    /// The dictionary size in bytes, once the header has been read.
    #[must_use]
    pub const fn dictionary_size(&self) -> u32 {
        if self.header_read {
            1u32 << (self.distance_bits + 6)
        } else {
            0
        }
    }

    fn push(&mut self, byte: u8, out: &mut Vec<u8>) {
        self.window[self.window_position] = byte;
        self.window_position = (self.window_position + 1) % MAX_WINDOW;
        self.total_output = self.total_output.saturating_add(1);
        out.push(byte);
    }

    /// Decodes as much of `input` as possible, appending to `out`.
    ///
    /// `input` is a prefix of the remaining stream; set `last` once no more
    /// input will ever follow. Decoding stops early when appending another
    /// token could push `out` past `out_budget`, so the caller can drain the
    /// buffer and resume. `out_budget` must be at least [`MAX_MATCH`].
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidInput`] when `out_budget` is too small.
    /// - [`Error::DecompressionFailed`] for a bad header, a distance that
    ///   reaches before the start of the stream, or a stream that ends
    ///   mid-token when `last` is set.
    pub fn decode(
        &mut self,
        input: &[u8],
        last: bool,
        out: &mut Vec<u8>,
        out_budget: usize,
    ) -> Result<Progress> {
        if out_budget < MAX_MATCH {
            return Err(Error::InvalidInput);
        }
        if self.finished {
            return Ok(Progress {
                consumed: 0,
                finished: true,
            });
        }

        let mut reader = BitReader {
            input,
            position: 0,
            buffer: self.saved_buffer,
            count: self.saved_count,
        };

        if !self.header_read {
            // The two header bytes are re-read from scratch until both are
            // available, so a chunk boundary inside the header loses nothing.
            let (Ok(literal_flag), Ok(dictionary)) = (reader.take(8), reader.take(8)) else {
                return Self::suspend(last);
            };
            if literal_flag > 1 {
                return Err(Error::DecompressionFailed);
            }
            let dictionary = u8::try_from(dictionary).map_err(|_| Error::DecompressionFailed)?;
            if !(MIN_DICTIONARY_CODE..=MAX_DICTIONARY_CODE).contains(&dictionary) {
                return Err(Error::DecompressionFailed);
            }
            self.coded_literals = literal_flag == 1;
            self.distance_bits = u32::from(dictionary);
            self.header_read = true;
            self.save(&reader);
        }

        loop {
            if out.len() + MAX_MATCH > out_budget {
                break;
            }
            let restart = (reader.position, reader.buffer, reader.count);
            match self.token(&mut reader, out) {
                Ok(true) => {
                    self.finished = true;
                    self.save(&reader);
                    return Ok(Progress {
                        consumed: reader.position,
                        finished: true,
                    });
                }
                Ok(false) => self.save(&reader),
                Err(TokenStop::Failed) => return Err(Error::DecompressionFailed),
                Err(TokenStop::NeedMoreInput) => {
                    reader.position = restart.0;
                    reader.buffer = restart.1;
                    reader.count = restart.2;
                    // A token cannot be completed with the input in hand. If
                    // nothing more is coming the stream ended mid-token.
                    if last {
                        return Err(Error::DecompressionFailed);
                    }
                    break;
                }
            }
        }

        self.save(&reader);
        Ok(Progress {
            consumed: reader.position,
            finished: false,
        })
    }

    fn suspend(last: bool) -> Result<Progress> {
        if last {
            return Err(Error::DecompressionFailed);
        }
        Ok(Progress {
            consumed: 0,
            finished: false,
        })
    }

    fn save(&mut self, reader: &BitReader<'_>) {
        self.saved_buffer = reader.buffer;
        self.saved_count = reader.count;
    }

    /// Decodes one literal or one length/distance pair. Returns `Ok(true)`
    /// once the end code is reached.
    fn token(&mut self, reader: &mut BitReader<'_>, out: &mut Vec<u8>) -> Partial<bool> {
        if reader.take(1)? == 0 {
            let literal = if self.coded_literals {
                reader.decode(&LITERAL_CODES)?
            } else {
                u16::try_from(reader.take(8)?).map_err(|_| TokenStop::Failed)?
            };
            let literal = u8::try_from(literal).map_err(|_| TokenStop::Failed)?;
            self.push(literal, out);
            return Ok(false);
        }

        let length_symbol = reader.decode(&LENGTH_CODES)? as usize;
        // `LENGTH_CODES` has exactly 16 symbols, so the index is in range.
        let extra = u32::from(LENGTH_EXTRA[length_symbol & 0x0f]);
        let length = u32::from(LENGTH_BASE[length_symbol & 0x0f]) + reader.take(extra)?;
        if length == END_CODE {
            return Ok(true);
        }

        let shift = if length == 2 { 2 } else { self.distance_bits };
        let distance_symbol = u32::from(reader.decode(&DISTANCE_CODES)?);
        let distance = ((distance_symbol << shift) | reader.take(shift)?) + 1;
        self.copy(distance, length, out)
    }

    fn copy(&mut self, distance: u32, length: u32, out: &mut Vec<u8>) -> Partial<bool> {
        // A distance reaching before the first output byte, or past the
        // window, is rejected rather than wrapping into stale window bytes.
        if u64::from(distance) > self.total_output || distance as usize > MAX_WINDOW {
            return Err(TokenStop::Failed);
        }
        for _ in 0..length {
            let from = (self.window_position + MAX_WINDOW - distance as usize) % MAX_WINDOW;
            let byte = self.window[from];
            self.push(byte, out);
        }
        Ok(false)
    }
}

/// Canonical-code lookup used only by the test-only implode encoder.
///
/// Returns the symbol's natural (pre-inversion) code value and its bit
/// length, mirroring exactly the enumeration [`BitReader::decode`] performs.
#[cfg(any(test, feature = "test-support"))]
fn encode_symbol<const N: usize>(codes: &Huffman<N>, symbol: u16) -> Option<(u32, u32)> {
    let mut index = 0i32;
    let mut first = 0i32;
    for bits in 1..=MAX_BITS {
        let count = i32::from(codes.count[bits]);
        for offset in 0..count {
            let at = usize::try_from(index + offset).ok()?;
            if *codes.symbol.get(at)? == symbol {
                return Some((
                    u32::try_from(first + offset).ok()?,
                    u32::try_from(bits).ok()?,
                ));
            }
        }
        index += count;
        first = (first + count) << 1;
    }
    None
}

/// Canonical code for one literal byte.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn literal_code(symbol: u8) -> Option<(u32, u32)> {
    encode_symbol(&LITERAL_CODES, u16::from(symbol))
}

/// Canonical code for one length symbol.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn length_code(symbol: u8) -> Option<(u32, u32)> {
    encode_symbol(&LENGTH_CODES, u16::from(symbol))
}

/// Canonical code for one distance symbol.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn distance_code(symbol: u8) -> Option<(u32, u32)> {
    encode_symbol(&DISTANCE_CODES, u16::from(symbol))
}

/// Base length for each length symbol.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub const fn length_base(symbol: u8) -> u16 {
    LENGTH_BASE[(symbol & 0x0f) as usize]
}

/// Extra bit count for each length symbol.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub const fn length_extra(symbol: u8) -> u8 {
    LENGTH_EXTRA[(symbol & 0x0f) as usize]
}

/// Decodes a complete imploded stream held in memory.
///
/// # Errors
///
/// Returns [`Error::DecompressionFailed`] for a malformed stream and
/// [`Error::InvalidInput`] when `max_output` is below [`MAX_MATCH`].
pub fn explode_to_vec(input: &[u8], max_output: usize) -> Result<Vec<u8>> {
    if max_output < MAX_MATCH {
        return Err(Error::InvalidInput);
    }
    let mut decoder = Explode::new();
    let mut out = Vec::new();
    let mut consumed = 0usize;
    loop {
        let progress = decoder.decode(&input[consumed..], true, &mut out, max_output)?;
        consumed += progress.consumed;
        if progress.finished {
            return Ok(out);
        }
        if out.len() + MAX_MATCH > max_output {
            return Err(Error::DecompressionFailed);
        }
        if progress.consumed == 0 {
            return Err(Error::DecompressionFailed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DISTANCE_CODES, Explode, LENGTH_CODES, LITERAL_CODES, MAX_BITS, MAX_MATCH, explode_to_vec,
    };
    use crate::error::Error;
    use alloc::vec;
    use alloc::vec::Vec;

    fn kraft(counts: &[u16; MAX_BITS + 1]) -> i64 {
        let mut left = 1i64;
        for count in counts.iter().skip(1) {
            left = left * 2 - i64::from(*count);
        }
        left
    }

    #[test]
    fn code_tables_are_complete() {
        assert_eq!(kraft(&LITERAL_CODES.count), 0);
        assert_eq!(kraft(&LENGTH_CODES.count), 0);
        assert_eq!(kraft(&DISTANCE_CODES.count), 0);
        assert_eq!(LITERAL_CODES.symbol.len(), 256);
        assert_eq!(LENGTH_CODES.symbol.len(), 16);
        assert_eq!(DISTANCE_CODES.symbol.len(), 64);
    }

    /// The known-answer vector published with the public format description
    /// (Mark Adler's `blast`, which credits Ben Rudiak-Gould's 2001
    /// comp.compression description): this stream decodes to "AIAIAIAIAIAIA".
    const KNOWN_ANSWER: [u8; 8] = [0x00, 0x04, 0x82, 0x24, 0x25, 0x8f, 0x80, 0x7f];

    #[test]
    fn decodes_the_published_known_answer_vector() {
        let out = explode_to_vec(&KNOWN_ANSWER, 4096).unwrap();
        assert_eq!(out, b"AIAIAIAIAIAIA");
    }

    #[test]
    fn decodes_the_known_answer_vector_one_byte_at_a_time() {
        let mut decoder = Explode::new();
        let mut out = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        for (index, byte) in KNOWN_ANSWER.iter().enumerate() {
            pending.push(*byte);
            let last = index + 1 == KNOWN_ANSWER.len();
            let progress = decoder.decode(&pending, last, &mut out, 4096).unwrap();
            pending.drain(..progress.consumed);
            if progress.finished {
                break;
            }
        }
        assert!(decoder.is_finished());
        assert_eq!(out, b"AIAIAIAIAIAIA");
        assert_eq!(decoder.total_output(), 13);
        assert_eq!(decoder.dictionary_size(), 1024);
    }

    #[test]
    fn rejects_a_bad_literal_flag() {
        assert_eq!(
            explode_to_vec(&[0x02, 0x06, 0x00], 4096),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_a_dictionary_code_below_the_minimum() {
        assert_eq!(
            explode_to_vec(&[0x00, 0x03, 0x00], 4096),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_a_dictionary_code_above_the_maximum() {
        assert_eq!(
            explode_to_vec(&[0x00, 0x07, 0x00], 4096),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_an_empty_stream() {
        assert_eq!(explode_to_vec(&[], 4096), Err(Error::DecompressionFailed));
    }

    #[test]
    fn rejects_a_truncated_stream() {
        assert_eq!(
            explode_to_vec(&KNOWN_ANSWER[..4], 4096),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_a_distance_before_the_start_of_the_stream() {
        // Header, then an immediate length/distance pair: nothing has been
        // emitted, so any distance reaches before the start.
        let stream = vec![0x00, 0x06, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            explode_to_vec(&stream, 4096),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_an_output_budget_below_one_match() {
        assert_eq!(explode_to_vec(&KNOWN_ANSWER, 8), Err(Error::InvalidInput));
        let mut decoder = Explode::new();
        let mut out = Vec::new();
        assert_eq!(
            decoder.decode(&KNOWN_ANSWER, true, &mut out, MAX_MATCH - 1),
            Err(Error::InvalidInput)
        );
    }
}
