//! A minimal, test-only LZX **encoder**, used to exercise the decoder in
//! `crate::lzx`.
//!
//! It is not a compressor: it emits whatever blocks and tokens a test asks
//! for, using deliberately simple uniform-length Huffman trees. That keeps it
//! short while still producing streams that are structurally what the format
//! documents: 16-bit little-endian bitstream words, pretree-encoded delta path
//! lengths, verbatim/aligned/uncompressed blocks, and 32,768-byte frames that
//! end on a 16-bit boundary.

// Test-only fixture code with small, invented values, so the `as` conversions
// below cannot lose information.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Path length used for every main-tree element. Ten bits covers the largest
/// cabinet main tree (256 + 8 * 42 = 592 elements) without over-subscribing.
const MAIN_CODE_BITS: u32 = 10;
/// Path length used for every length-tree element.
const LENGTH_CODE_BITS: u32 = 8;
/// Path length used for every pretree element.
const PRETREE_CODE_BITS: u32 = 5;
/// Main-tree elements reserved for literals.
const NUM_CHARS: usize = 256;
/// Elements in the length tree.
const LENGTH_TREE_ELEMENTS: usize = 249;

/// The block kinds a test can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// A verbatim block.
    Verbatim,
    /// An aligned offset block.
    Aligned,
}

/// Builds one LZX stream and slices it into per-frame `CFDATA` payloads.
pub struct LzxEncoder {
    out: Vec<u8>,
    accumulator: u32,
    bits: u32,
    frames: Vec<Vec<u8>>,
    frame_start: usize,
    base_position: Vec<u32>,
    footer_bits: Vec<u8>,
    main_previous: Vec<u8>,
    length_previous: Vec<u8>,
    block: Block,
}

impl LzxEncoder {
    /// Starts a stream for a `2^window_bits`-byte window.
    #[must_use]
    pub fn new(window_bits: u8) -> Self {
        let slots = 2 * usize::from(window_bits);
        let mut base_position = Vec::with_capacity(slots);
        let mut footer_bits = Vec::with_capacity(slots);
        let mut position = 0u32;
        for slot in 0..slots {
            let bits = if slot < 4 { 0u8 } else { (slot / 2 - 1) as u8 };
            base_position.push(position);
            footer_bits.push(bits);
            position += 1u32 << bits;
        }
        Self {
            out: Vec::new(),
            accumulator: 0,
            bits: 0,
            frames: Vec::new(),
            frame_start: 0,
            base_position,
            footer_bits,
            main_previous: vec![0u8; NUM_CHARS + 8 * slots],
            length_previous: vec![0u8; LENGTH_TREE_ELEMENTS],
            block: Block::Verbatim,
        }
    }

    fn put(&mut self, value: u32, count: u32) {
        assert!(count <= 17 && self.bits + count <= 32);
        if count > 0 {
            self.accumulator |= value << (32 - self.bits - count);
            self.bits += count;
        }
        while self.bits >= 16 {
            let word = (self.accumulator >> 16) as u16;
            self.out.extend_from_slice(&word.to_le_bytes());
            self.accumulator <<= 16;
            self.bits -= 16;
        }
    }

    /// Writes the stream header: the E8 translation bit and, when enabled, the
    /// 32-bit translation size as two 16-bit fields.
    pub fn header(&mut self, e8_file_size: Option<u32>) {
        match e8_file_size {
            Some(size) => {
                self.put(1, 1);
                self.put(size >> 16, 16);
                self.put(size & 0xFFFF, 16);
            }
            None => self.put(0, 1),
        }
    }

    /// Emits a verbatim or aligned block header covering `size` bytes.
    pub fn begin_block(&mut self, block: Block, size: u32) {
        self.block = block;
        let kind = match block {
            Block::Verbatim => 1,
            Block::Aligned => 2,
        };
        self.put(kind, 3);
        self.put((size >> 16) & 0xFF, 8);
        self.put((size >> 8) & 0xFF, 8);
        self.put(size & 0xFF, 8);
        if block == Block::Aligned {
            for _ in 0..8 {
                self.put(3, 3);
            }
        }
        let main = self.main_previous.clone();
        let split = NUM_CHARS;
        self.write_lengths(&main[..split], MAIN_CODE_BITS as u8);
        self.write_lengths(&main[split..], MAIN_CODE_BITS as u8);
        let lengths = self.length_previous.clone();
        self.write_lengths(&lengths, LENGTH_CODE_BITS as u8);
        self.main_previous.fill(MAIN_CODE_BITS as u8);
        self.length_previous.fill(LENGTH_CODE_BITS as u8);
    }

    /// Emits an uncompressed block holding `data`.
    pub fn stored_block(&mut self, data: &[u8], repeated: [u32; 3]) {
        self.put(3, 3);
        let size = data.len() as u32;
        self.put((size >> 16) & 0xFF, 8);
        self.put((size >> 8) & 0xFF, 8);
        self.put(size & 0xFF, 8);
        // "1 to 16 bits of zero padding to align the bit buffer on a 16-bit
        // boundary": a full word when the buffer is already aligned.
        if self.bits == 0 {
            self.put(0, 16);
        } else {
            self.put(0, 16 - self.bits);
        }
        assert_eq!(self.bits, 0);
        for value in repeated {
            self.out.extend_from_slice(&value.to_le_bytes());
        }
        self.out.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            self.out.push(0);
        }
    }

    /// Emits one literal byte.
    pub fn literal(&mut self, byte: u8) {
        self.put(u32::from(byte), MAIN_CODE_BITS);
    }

    /// Emits a match of `length` bytes at `offset` bytes back.
    pub fn emit_match(&mut self, length: usize, offset: u32) {
        let formatted = offset + 2;
        let slot = self
            .base_position
            .iter()
            .rposition(|base| *base <= formatted)
            .expect("a position slot covers the offset");
        self.emit_token(slot, length);
        let footer = u32::from(self.footer_bits[slot]);
        let value = formatted - self.base_position[slot];
        if self.block == Block::Aligned && footer > 3 {
            self.put(value >> 3, footer - 3);
            self.put(value & 7, 3);
        } else {
            self.put(value, footer);
        }
        self.emit_extra_length(length);
    }

    /// Emits a match that reuses repeated offset `index` (0, 1 or 2).
    pub fn emit_repeat_match(&mut self, index: usize, length: usize) {
        assert!(index < 3);
        self.emit_token(index, length);
        self.emit_extra_length(length);
    }

    fn emit_token(&mut self, slot: usize, length: usize) {
        assert!(length >= 2);
        let header = if length - 2 < 7 { length - 2 } else { 7 };
        let element = NUM_CHARS + slot * 8 + header;
        self.put(element as u32, MAIN_CODE_BITS);
        if header == 7 {
            let symbol = (length.min(257) - 9) as u32;
            self.put(symbol, LENGTH_CODE_BITS);
        }
    }

    fn emit_extra_length(&mut self, length: usize) {
        if length >= 257 {
            let extra = length - 257;
            assert!(extra < 256, "the test encoder only emits the short form");
            self.put(0, 1);
            self.put(extra as u32, 8);
        }
    }

    fn write_lengths(&mut self, previous: &[u8], new_length: u8) {
        for _ in 0..20 {
            self.put(PRETREE_CODE_BITS, 4);
        }
        for &previous in previous {
            let code = (i32::from(previous) - i32::from(new_length) + 17) % 17;
            self.put(code as u32, PRETREE_CODE_BITS);
        }
    }

    /// Ends the current 32 KiB frame, padding the bitstream to a 16-bit
    /// boundary, and records the frame's `CFDATA` payload.
    pub fn end_frame(&mut self) {
        if self.bits > 0 {
            self.put(0, 16 - self.bits);
        }
        let frame = self.out[self.frame_start..].to_vec();
        self.frame_start = self.out.len();
        self.frames.push(frame);
    }

    /// Returns the per-frame payloads.
    #[must_use]
    pub fn finish(mut self) -> Vec<Vec<u8>> {
        if self.out.len() > self.frame_start || self.bits > 0 {
            self.end_frame();
        }
        self.frames
    }
}

/// The forward x86 `E8` call translation, as the format documents it for the
/// encoder side. The decoder must undo exactly this.
pub fn e8_forward(frame: &mut [u8], chunk_offset: u32, file_size: u32) {
    if frame.len() <= 10 {
        return;
    }
    let limit = frame.len() - 10;
    let mut index = 0usize;
    while index < limit {
        if frame[index] != 0xE8 {
            index += 1;
            continue;
        }
        let current = i64::from(chunk_offset) + index as i64;
        let displacement = i64::from(i32::from_le_bytes([
            frame[index + 1],
            frame[index + 2],
            frame[index + 3],
            frame[index + 4],
        ]));
        let target = current + displacement;
        if target >= 0 && target < i64::from(file_size) + current {
            let target = if target >= i64::from(file_size) {
                displacement - i64::from(file_size)
            } else {
                target
            };
            frame[index + 1..index + 5].copy_from_slice(&(target as i32).to_le_bytes());
        }
        index += 5;
    }
}
