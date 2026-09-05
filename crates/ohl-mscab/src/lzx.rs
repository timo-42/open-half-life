//! LZX decoding for cabinet folders.
//!
//! Implemented from Microsoft's public "Microsoft LZX Data Compression
//! Format" (published with the Microsoft Cabinet Format documentation) and
//! from [MS-PATCH] "LZX DELTA Compression and Decompression", which restates
//! the same block, tree, and token structures with more precise pseudo-code.
//! See `docs/FORMAT_SOURCES.md` for the exact sections used.
//!
//! Cabinet specifics that the format documents:
//!
//! * The window size is 2^15..=2^21 bytes and is *not* in the stream; it comes
//!   from `CFFOLDER.typeCompress`.
//! * `NUM_POSITION_SLOTS` is 30 for a 32 KiB window and grows by two per
//!   window bit up to 42 for 2 MiB.
//! * The uncompressed stream is divided into 32,768-byte frames; the bitstream
//!   is padded to a 16-bit boundary after each frame, and one cabinet `CFDATA`
//!   block carries exactly one frame. Decoder state (window, trees, R0/R1/R2,
//!   the current block) therefore persists across the folder's blocks while
//!   the bit reader restarts, byte-aligned, at each block.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::error::{CabError, Result};
use crate::limits::Limits;

/// Smallest cabinet LZX window, `2^15`.
pub const MIN_WINDOW_BITS: u8 = 15;
/// Largest cabinet LZX window, `2^21`.
pub const MAX_WINDOW_BITS: u8 = 21;

/// Frame size: the uncompressed span one `CFDATA` block represents.
const FRAME_SIZE: usize = 32_768;
/// `NUM_CHARS`: main-tree elements reserved for literals.
const NUM_CHARS: usize = 256;
/// Elements in the length tree.
const LENGTH_TREE_ELEMENTS: usize = 249;
/// Elements in the aligned offset tree.
const ALIGNED_TREE_ELEMENTS: usize = 8;
/// Elements in a pretree.
const PRETREE_ELEMENTS: usize = 20;
/// Longest permitted Huffman path length.
const MAX_CODE_BITS: u32 = 16;
/// E8 translation stops after this many frames (1 GiB of output).
const E8_FRAME_LIMIT: u64 = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Verbatim,
    Aligned,
    Uncompressed,
}

/// A canonical Huffman decoding table built from path lengths only, as the
/// format requires ("an LZX decoder uses only the path lengths of the Huffman
/// tree to reconstruct the identical tree").
struct HuffmanTable {
    counts: [u16; MAX_CODE_BITS as usize + 1],
    symbols: Vec<u16>,
}

impl HuffmanTable {
    fn new(lengths: &[u8]) -> Result<Self> {
        let mut counts = [0u16; MAX_CODE_BITS as usize + 1];
        for &length in lengths {
            if u32::from(length) > MAX_CODE_BITS {
                return Err(CabError::DecompressionFailed);
            }
            counts[usize::from(length)] += 1;
        }
        counts[0] = 0;

        // Reject over-subscribed length sets; under-subscribed sets are
        // tolerated, and decoding an absent code fails cleanly.
        let mut left = 1i32;
        for &count in counts.iter().skip(1) {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return Err(CabError::DecompressionFailed);
            }
        }

        let mut offsets = [0usize; MAX_CODE_BITS as usize + 2];
        for (length, &count) in counts.iter().enumerate().skip(1) {
            offsets[length + 1] = offsets[length] + usize::from(count);
        }
        let total = offsets[MAX_CODE_BITS as usize + 1];
        let mut symbols = vec![0u16; total];
        let mut cursor = offsets;
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = cursor[usize::from(length)];
                symbols[slot] = u16::try_from(symbol).map_err(|_| CabError::Internal)?;
                cursor[usize::from(length)] = slot + 1;
            }
        }
        Ok(Self { counts, symbols })
    }
}

/// A bit reader over the LZX bitstream: "a sequence of aligned 16-bit
/// integers stored in least-significant-byte to most-significant-byte order",
/// with bits consumed most-significant first within each word.
struct BitReader<'a> {
    data: &'a [u8],
    byte_position: usize,
    buffer: u32,
    bits: u32,
    overrun_words: u32,
}

impl<'a> BitReader<'a> {
    const MAX_OVERRUN_WORDS: u32 = 4;

    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_position: 0,
            buffer: 0,
            bits: 0,
            overrun_words: 0,
        }
    }

    fn fill(&mut self, count: u32) -> Result<()> {
        debug_assert!(count <= 17);
        while self.bits < count {
            let low = self.next_byte()?;
            let high = self.next_byte()?;
            let word = u32::from(u16::from_le_bytes([low, high]));
            self.buffer |= word << (16 - self.bits);
            self.bits += 16;
        }
        Ok(())
    }

    fn next_byte(&mut self) -> Result<u8> {
        if let Some(&byte) = self.data.get(self.byte_position) {
            self.byte_position += 1;
            return Ok(byte);
        }
        // A frame's final symbol can need a few bits that the encoder did not
        // have to emit; tolerate a bounded zero pad past the end.
        self.byte_position += 1;
        if self.byte_position.is_multiple_of(2) {
            self.overrun_words += 1;
            if self.overrun_words > Self::MAX_OVERRUN_WORDS {
                return Err(CabError::DecompressionFailed);
            }
        }
        Ok(0)
    }

    fn read_bits(&mut self, count: u32) -> Result<u32> {
        if count == 0 {
            return Ok(0);
        }
        self.fill(count)?;
        let value = self.buffer >> (32 - count);
        self.buffer <<= count;
        self.bits -= count;
        Ok(value)
    }

    /// Reads a 24-bit field as the documented three 8-bit groups, most
    /// significant first.
    fn read_u24(&mut self) -> Result<u32> {
        let high = self.read_bits(8)?;
        let middle = self.read_bits(8)?;
        let low = self.read_bits(8)?;
        Ok((high << 16) | (middle << 8) | low)
    }

    /// Consumes "1 to 16 bits of zero padding to align the bit buffer on a
    /// 16-bit boundary".
    fn align_to_word(&mut self) -> Result<()> {
        let remainder = self.bits % 16;
        if remainder == 0 {
            self.read_bits(16)?;
        } else {
            self.read_bits(remainder)?;
        }
        Ok(())
    }

    /// The byte offset the byte stream resumes at once the bit buffer is
    /// word-aligned.
    fn aligned_byte_position(&self) -> Result<usize> {
        if !self.bits.is_multiple_of(16) {
            return Err(CabError::Internal);
        }
        let buffered = usize::try_from(self.bits / 8).map_err(|_| CabError::Internal)?;
        self.byte_position
            .checked_sub(buffered)
            .ok_or(CabError::Internal)
    }

    /// Restarts the bit buffer at `position` in the byte stream.
    fn resume_at(&mut self, position: usize) {
        self.byte_position = position;
        self.buffer = 0;
        self.bits = 0;
    }

    fn decode(&mut self, table: &HuffmanTable) -> Result<u16> {
        let mut code = 0u32;
        let mut first = 0u32;
        let mut index = 0usize;
        for &count in table.counts.iter().skip(1) {
            code |= self.read_bits(1)?;
            let count = u32::from(count);
            let delta = code
                .checked_sub(first)
                .ok_or(CabError::DecompressionFailed)?;
            if delta < count {
                let slot = index + usize::try_from(delta).map_err(|_| CabError::Internal)?;
                return table
                    .symbols
                    .get(slot)
                    .copied()
                    .ok_or(CabError::DecompressionFailed);
            }
            index += usize::try_from(count).map_err(|_| CabError::Internal)?;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(CabError::DecompressionFailed)
    }
}

/// The LZX decoder for one cabinet folder.
pub(crate) struct LzxDecoder {
    window: Vec<u8>,
    window_mask: usize,
    window_position: usize,
    output_total: u64,
    position_slots: usize,
    base_position: Vec<u32>,
    footer_bits: Vec<u8>,
    main_lengths: Vec<u8>,
    length_lengths: Vec<u8>,
    main_table: Option<HuffmanTable>,
    length_table: Option<HuffmanTable>,
    aligned_table: Option<HuffmanTable>,
    repeated: [u32; 3],
    block_type: BlockType,
    block_remaining: u32,
    header_read: bool,
    e8_translation: bool,
    e8_file_size: u32,
    frames_done: u64,
}

impl LzxDecoder {
    pub(crate) fn new(window_bits: u8, limits: &Limits) -> Result<Self> {
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits)
            || window_bits > limits.max_lzx_window_bits
        {
            return Err(CabError::Unsupported);
        }
        let window_size = 1usize << window_bits;
        // "The window size determines the number of window subdivisions, or
        // position slots": 30 slots for a 32 KiB window, two more per bit.
        let position_slots = 2 * usize::from(window_bits);
        let (base_position, footer_bits) = slot_tables(position_slots);
        let main_elements = NUM_CHARS + 8 * position_slots;
        Ok(Self {
            window: vec![0u8; window_size],
            window_mask: window_size - 1,
            window_position: 0,
            output_total: 0,
            position_slots,
            base_position,
            footer_bits,
            main_lengths: vec![0u8; main_elements],
            length_lengths: vec![0u8; LENGTH_TREE_ELEMENTS],
            main_table: None,
            length_table: None,
            aligned_table: None,
            repeated: [1, 1, 1],
            block_type: BlockType::Verbatim,
            block_remaining: 0,
            header_read: false,
            e8_translation: false,
            e8_file_size: 0,
            frames_done: 0,
        })
    }

    /// Decodes one `CFDATA` payload, which carries exactly one frame of
    /// `expected` uncompressed bytes.
    pub(crate) fn decode_block(
        &mut self,
        input: &[u8],
        expected: usize,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        out.clear();
        if expected > FRAME_SIZE {
            return Err(CabError::LimitExceeded);
        }
        if expected == 0 {
            return Ok(());
        }
        let mut bits = BitReader::new(input);
        if !self.header_read {
            self.header_read = true;
            if bits.read_bits(1)? == 1 {
                let high = bits.read_bits(16)?;
                let low = bits.read_bits(16)?;
                self.e8_file_size = (high << 16) | low;
                self.e8_translation = true;
            }
        }

        let frame_start = self.window_position;
        let mut produced = 0usize;
        while produced < expected {
            if self.block_remaining == 0 {
                self.read_block_header(&mut bits)?;
                if self.block_remaining == 0 {
                    return Err(CabError::DecompressionFailed);
                }
            }
            let todo = (expected - produced).min(self.block_remaining as usize);
            match self.block_type {
                BlockType::Verbatim | BlockType::Aligned => self.decode_tokens(&mut bits, todo)?,
                BlockType::Uncompressed => self.copy_stored(&mut bits, todo)?,
            }
            produced += todo;
            self.block_remaining -= u32::try_from(todo).map_err(|_| CabError::Internal)?;
            if self.block_remaining == 0 && self.block_type == BlockType::Uncompressed {
                finish_stored_block(&mut bits);
            }
        }

        out.try_reserve(expected)
            .map_err(|_| CabError::LimitExceeded)?;
        let first = expected.min(self.window.len() - frame_start);
        out.extend_from_slice(&self.window[frame_start..frame_start + first]);
        if first < expected {
            out.extend_from_slice(&self.window[..expected - first]);
        }

        if self.e8_translation && self.frames_done < E8_FRAME_LIMIT {
            let chunk_offset = self.output_total - expected as u64;
            e8_reverse(out, chunk_offset, self.e8_file_size);
        }
        self.frames_done += 1;
        Ok(())
    }

    fn read_block_header(&mut self, bits: &mut BitReader<'_>) -> Result<()> {
        self.block_type = match bits.read_bits(3)? {
            1 => BlockType::Verbatim,
            2 => BlockType::Aligned,
            3 => BlockType::Uncompressed,
            _ => return Err(CabError::DecompressionFailed),
        };
        self.block_remaining = bits.read_u24()?;

        match self.block_type {
            BlockType::Aligned => {
                let mut aligned = [0u8; ALIGNED_TREE_ELEMENTS];
                for slot in &mut aligned {
                    *slot = u8::try_from(bits.read_bits(3)?).map_err(|_| CabError::Internal)?;
                }
                self.aligned_table = Some(HuffmanTable::new(&aligned)?);
                self.read_main_and_length_trees(bits)?;
            }
            BlockType::Verbatim => {
                self.read_main_and_length_trees(bits)?;
            }
            BlockType::Uncompressed => {
                bits.align_to_word()?;
                let position = bits.aligned_byte_position()?;
                let mut repeated = [0u32; 3];
                let mut cursor = position;
                for slot in &mut repeated {
                    let bytes = bits
                        .data
                        .get(cursor..cursor + 4)
                        .ok_or(CabError::DecompressionFailed)?;
                    *slot = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    if *slot == 0 {
                        return Err(CabError::DecompressionFailed);
                    }
                    cursor += 4;
                }
                self.repeated = repeated;
                bits.resume_at(cursor);
            }
        }
        Ok(())
    }

    fn read_main_and_length_trees(&mut self, bits: &mut BitReader<'_>) -> Result<()> {
        let main_elements = NUM_CHARS + 8 * self.position_slots;
        read_lengths(bits, &mut self.main_lengths, 0, NUM_CHARS)?;
        read_lengths(bits, &mut self.main_lengths, NUM_CHARS, main_elements)?;
        self.main_table = Some(HuffmanTable::new(&self.main_lengths)?);
        read_lengths(bits, &mut self.length_lengths, 0, LENGTH_TREE_ELEMENTS)?;
        self.length_table = Some(HuffmanTable::new(&self.length_lengths)?);
        Ok(())
    }

    /// Decodes tokens with the current trees moved out of `self`, so the
    /// window can be written while the tables are borrowed.
    fn decode_tokens(&mut self, bits: &mut BitReader<'_>, todo: usize) -> Result<()> {
        let main = self
            .main_table
            .take()
            .ok_or(CabError::DecompressionFailed)?;
        let length = self.length_table.take();
        let aligned = self.aligned_table.take();
        let result = self.decode_tokens_with(bits, todo, &main, length.as_ref(), aligned.as_ref());
        self.main_table = Some(main);
        self.length_table = length;
        self.aligned_table = aligned;
        result
    }

    fn decode_tokens_with(
        &mut self,
        bits: &mut BitReader<'_>,
        todo: usize,
        main_table: &HuffmanTable,
        length_table: Option<&HuffmanTable>,
        aligned_table: Option<&HuffmanTable>,
    ) -> Result<()> {
        let mut remaining = todo;
        while remaining > 0 {
            let element = usize::from(bits.decode(main_table)?);
            if element < NUM_CHARS {
                self.push_byte(u8::try_from(element).map_err(|_| CabError::Internal)?);
                remaining -= 1;
                continue;
            }
            let symbol = element - NUM_CHARS;
            let length_header = symbol & 7;
            let mut match_length = if length_header == 7 {
                let table = length_table.ok_or(CabError::DecompressionFailed)?;
                usize::from(bits.decode(table)?) + 7 + 2
            } else {
                length_header + 2
            };
            let position_slot = symbol >> 3;
            if position_slot >= self.position_slots {
                return Err(CabError::DecompressionFailed);
            }

            let match_offset = match position_slot {
                0 => self.repeated[0],
                1 => {
                    self.repeated.swap(0, 1);
                    self.repeated[0]
                }
                2 => {
                    self.repeated.swap(0, 2);
                    self.repeated[0]
                }
                slot => {
                    let footer = u32::from(self.footer_bits[slot]);
                    let formatted = if self.block_type == BlockType::Aligned && footer > 3 {
                        let verbatim = bits.read_bits(footer - 3)? << 3;
                        let table = aligned_table.ok_or(CabError::DecompressionFailed)?;
                        let aligned = u32::from(bits.decode(table)?);
                        self.base_position[slot]
                            .checked_add(verbatim)
                            .and_then(|value| value.checked_add(aligned))
                            .ok_or(CabError::DecompressionFailed)?
                    } else {
                        let verbatim = bits.read_bits(footer)?;
                        self.base_position[slot]
                            .checked_add(verbatim)
                            .ok_or(CabError::DecompressionFailed)?
                    };
                    let offset = formatted
                        .checked_sub(2)
                        .ok_or(CabError::DecompressionFailed)?;
                    self.repeated[2] = self.repeated[1];
                    self.repeated[1] = self.repeated[0];
                    self.repeated[0] = offset;
                    offset
                }
            };

            if match_length == 257 {
                let extra = if bits.read_bits(1)? != 0 {
                    if bits.read_bits(1)? != 0 {
                        if bits.read_bits(1)? != 0 {
                            bits.read_bits(15)?
                        } else {
                            bits.read_bits(12)? + 1024 + 256
                        }
                    } else {
                        bits.read_bits(10)? + 256
                    }
                } else {
                    bits.read_bits(8)?
                };
                match_length = 257usize
                    .checked_add(extra as usize)
                    .ok_or(CabError::DecompressionFailed)?;
            }

            if match_length > remaining {
                // No match may span a frame or block boundary.
                return Err(CabError::DecompressionFailed);
            }
            let offset = match_offset as usize;
            if offset == 0 || offset > self.window.len() || (offset as u64) > self.output_total {
                return Err(CabError::DecompressionFailed);
            }
            for _ in 0..match_length {
                let source = (self.window_position + self.window.len() - offset) & self.window_mask;
                let byte = self.window[source];
                self.push_byte(byte);
            }
            remaining -= match_length;
        }
        Ok(())
    }

    fn copy_stored(&mut self, bits: &mut BitReader<'_>, todo: usize) -> Result<()> {
        if bits.bits != 0 {
            return Err(CabError::Internal);
        }
        let start = bits.byte_position;
        let end = start.checked_add(todo).ok_or(CabError::OutOfBounds)?;
        let bytes = bits
            .data
            .get(start..end)
            .ok_or(CabError::DecompressionFailed)?;
        for &byte in bytes {
            self.push_byte(byte);
        }
        bits.resume_at(end);
        Ok(())
    }

    fn push_byte(&mut self, byte: u8) {
        self.window[self.window_position] = byte;
        self.window_position = (self.window_position + 1) & self.window_mask;
        self.output_total += 1;
    }
}

/// "If the uncompressed data length is odd, one extra byte of zero padding is
/// encoded to realign the following bitstream."
fn finish_stored_block(bits: &mut BitReader<'_>) {
    if !bits.byte_position.is_multiple_of(2) {
        bits.resume_at(bits.byte_position + 1);
    }
}

/// Builds the position-slot base/footer tables.
///
/// The documented table starts `0,1,2,3` with zero footer bits and then adds
/// one footer bit every two slots, each slot's base being the previous base
/// plus `2^footer`. Generating it keeps the 42-slot cabinet table and the
/// small-window prefixes consistent by construction.
fn slot_tables(slots: usize) -> (Vec<u32>, Vec<u8>) {
    let mut base = Vec::with_capacity(slots);
    let mut footer = Vec::with_capacity(slots);
    let mut position = 0u32;
    for slot in 0..slots {
        let bits = if slot < 4 {
            0u8
        } else {
            u8::try_from(slot / 2 - 1).unwrap_or(u8::MAX)
        };
        base.push(position);
        footer.push(bits);
        position = position.saturating_add(1u32 << bits);
    }
    (base, footer)
}

/// Reads delta-encoded path lengths for `lengths[start..end]` through a
/// freshly read pretree.
fn read_lengths(
    bits: &mut BitReader<'_>,
    lengths: &mut [u8],
    start: usize,
    end: usize,
) -> Result<()> {
    let mut pretree = [0u8; PRETREE_ELEMENTS];
    for slot in &mut pretree {
        *slot = u8::try_from(bits.read_bits(4)?).map_err(|_| CabError::Internal)?;
    }
    let table = HuffmanTable::new(&pretree)?;

    let mut index = start;
    while index < end {
        let code = bits.decode(&table)?;
        match code {
            0..=16 => {
                let previous = i32::from(lengths[index]);
                let value = (previous - i32::from(code) + 17) % 17;
                lengths[index] = u8::try_from(value).map_err(|_| CabError::Internal)?;
                index += 1;
            }
            17 => {
                let run = 4 + bits.read_bits(4)? as usize;
                fill(lengths, &mut index, end, run, 0)?;
            }
            18 => {
                let run = 20 + bits.read_bits(5)? as usize;
                fill(lengths, &mut index, end, run, 0)?;
            }
            19 => {
                let run = 4 + bits.read_bits(1)? as usize;
                let code = bits.decode(&table)?;
                if code > 16 {
                    return Err(CabError::DecompressionFailed);
                }
                let previous = i32::from(lengths[index]);
                let value = (previous - i32::from(code) + 17) % 17;
                let value = u8::try_from(value).map_err(|_| CabError::Internal)?;
                fill(lengths, &mut index, end, run, value)?;
            }
            _ => return Err(CabError::DecompressionFailed),
        }
    }
    Ok(())
}

fn fill(lengths: &mut [u8], index: &mut usize, end: usize, run: usize, value: u8) -> Result<()> {
    let stop = index
        .checked_add(run)
        .ok_or(CabError::DecompressionFailed)?;
    if stop > end {
        return Err(CabError::DecompressionFailed);
    }
    lengths[*index..stop].fill(value);
    *index = stop;
    Ok(())
}

/// Reverses the optional x86 `E8` call translation over one frame.
fn e8_reverse(frame: &mut [u8], chunk_offset: u64, file_size: u32) {
    if frame.len() <= 10 {
        return;
    }
    let file_size = i64::from(file_size);
    let limit = frame.len() - 10;
    let Ok(chunk_offset) = i64::try_from(chunk_offset) else {
        return;
    };
    let mut index = 0usize;
    while index < limit {
        if frame[index] != 0xE8 {
            index += 1;
            continue;
        }
        let Ok(position) = i64::try_from(index) else {
            return;
        };
        let current = chunk_offset + position;
        let bytes = [
            frame[index + 1],
            frame[index + 2],
            frame[index + 3],
            frame[index + 4],
        ];
        let value = i64::from(i32::from_le_bytes(bytes));
        if value >= -current && value < file_size {
            let displacement = if value >= 0 {
                value - current
            } else {
                value + file_size
            };
            if let Ok(displacement) = i32::try_from(displacement) {
                frame[index + 1..index + 5].copy_from_slice(&displacement.to_le_bytes());
            }
        }
        index += 5;
    }
}

#[cfg(test)]
mod tests {
    use super::{HuffmanTable, MAX_WINDOW_BITS, MIN_WINDOW_BITS, e8_reverse, slot_tables};
    use crate::error::CabError;
    use crate::limits::Limits;

    #[test]
    fn slot_tables_match_the_documented_values() {
        let (base, footer) = slot_tables(42);
        assert_eq!(&base[..12], &[0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48]);
        assert_eq!(&footer[..12], &[0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4]);
        // A 32 KiB window uses 30 slots and must cover exactly 32,768.
        let (base32, footer32) = slot_tables(30);
        assert_eq!(base32[29] + (1 << footer32[29]), 32_768);
        // A 2 MiB window uses 42 slots and must cover exactly 2,097,152.
        assert_eq!(base[41] + (1 << footer[41]), 2_097_152);
    }

    #[test]
    fn rejects_window_sizes_outside_the_cabinet_range() {
        let limits = Limits::default();
        assert_eq!(
            super::LzxDecoder::new(MIN_WINDOW_BITS - 1, &limits).err(),
            Some(CabError::Unsupported)
        );
        assert_eq!(
            super::LzxDecoder::new(MAX_WINDOW_BITS + 1, &limits).err(),
            Some(CabError::Unsupported)
        );
        assert!(super::LzxDecoder::new(MIN_WINDOW_BITS, &limits).is_ok());
        assert!(super::LzxDecoder::new(MAX_WINDOW_BITS, &limits).is_ok());
    }

    #[test]
    fn rejects_over_subscribed_huffman_lengths() {
        assert_eq!(
            HuffmanTable::new(&[1, 1, 1]).err(),
            Some(CabError::DecompressionFailed)
        );
        assert!(HuffmanTable::new(&[1, 1]).is_ok());
    }

    #[test]
    fn e8_reversal_leaves_short_frames_and_out_of_range_values_alone() {
        let mut short = [0xE8u8; 8];
        e8_reverse(&mut short, 0, 4_096);
        assert_eq!(short, [0xE8u8; 8]);

        let mut frame = [0u8; 32];
        frame[0] = 0xE8;
        frame[1..5].copy_from_slice(&0x7FFF_FFFFi32.to_le_bytes());
        let expected = frame;
        e8_reverse(&mut frame, 0, 4_096);
        assert_eq!(frame, expected);
    }
}
