//! LZX decoder tests, driven by the crate's own minimal LZX encoder.
//!
//! The encoder is deliberately independent of the decoder: it emits the block,
//! tree, and token structures the public LZX documentation describes, and the
//! decoder must recover the original bytes. All payloads are invented here.

use ohl_mscab::lzx_writer::{Block, LzxEncoder, e8_forward};
use ohl_mscab::test_support::{BlockSpec, CabinetSpec, FileSpec, FolderSpec, Method, build};
use ohl_mscab::{
    CabError, Cabinet, Compression, FolderStream, Limits, NeverCancelled, SliceSource,
};

/// `typeCompress` for LZX with the given window size in bits.
fn lzx_type(window_bits: u16) -> u16 {
    3 | (window_bits << 8)
}

/// Tracks the bytes an encoded token sequence is expected to produce.
#[derive(Default)]
struct Model {
    data: Vec<u8>,
}

impl Model {
    fn literal(&mut self, byte: u8) {
        self.data.push(byte);
    }

    fn copy(&mut self, length: usize, offset: usize) {
        for _ in 0..length {
            let byte = self.data[self.data.len() - offset];
            self.data.push(byte);
        }
    }
}

/// Runs `frames` (compressed payload plus its uncompressed length) through a
/// synthetic LZX cabinet folder and returns the decoded folder stream.
fn decode(frames: &[(Vec<u8>, u16)], window_bits: u16) -> Result<Vec<u8>, CabError> {
    let total: usize = frames.iter().map(|(_, length)| usize::from(*length)).sum();
    let mut folder = FolderSpec::new(
        Method::Raw(lzx_type(window_bits)),
        vec![FileSpec::new("lzx.bin", vec![0u8; total])],
    );
    folder.blocks = Some(
        frames
            .iter()
            .map(|(compressed, length)| BlockSpec {
                compressed: compressed.clone(),
                uncompressed_len: *length,
            })
            .collect(),
    );
    let built = build(&CabinetSpec::new(vec![folder]));
    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default())?;
    assert_eq!(
        cabinet.folders()[0].compression,
        Compression::Lzx {
            window_bits: u8::try_from(window_bits).expect("a cabinet window fits in a byte")
        }
    );
    let mut stream = FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default())?;
    let mut out = Vec::new();
    let mut buffer = [0u8; 1_024];
    loop {
        let read = stream.read(&mut buffer, &NeverCancelled)?;
        if read == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

fn frames(encoder: LzxEncoder, lengths: &[u16]) -> Vec<(Vec<u8>, u16)> {
    let payloads = encoder.finish();
    assert_eq!(payloads.len(), lengths.len());
    payloads.into_iter().zip(lengths.iter().copied()).collect()
}

#[test]
fn decodes_a_verbatim_block_of_literals() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 300);
    for index in 0..300u32 {
        let byte = (index % 251) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[300]), 15).unwrap(), model.data);
}

#[test]
fn decodes_matches_and_repeated_offsets() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 122);
    for index in 0..100u32 {
        let byte = (index % 97) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    encoder.emit_match(6, 50);
    model.copy(6, 50);
    encoder.emit_match(4, 12);
    model.copy(4, 12);
    // R1 is now 50: reuse it, then R0 (which the reuse swapped back to 50).
    encoder.emit_repeat_match(1, 8);
    model.copy(8, 50);
    encoder.emit_repeat_match(0, 4);
    model.copy(4, 50);
    assert_eq!(model.data.len(), 122);
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[122]), 15).unwrap(), model.data);
}

#[test]
fn decodes_an_aligned_offset_block() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Aligned, 2_040);
    for index in 0..2_000u32 {
        let byte = (index % 211) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    // Offsets large enough that the position footer splits into verbatim and
    // aligned parts.
    encoder.emit_match(20, 1_000);
    model.copy(20, 1_000);
    encoder.emit_match(20, 1_777);
    model.copy(20, 1_777);
    assert_eq!(model.data.len(), 2_040);
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[2_040]), 15).unwrap(), model.data);
}

#[test]
fn decodes_an_uncompressed_block_with_odd_length_padding() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    let stored: Vec<u8> = (0..101u32).map(|value| (value % 199) as u8).collect();
    encoder.stored_block(&stored, [1, 1, 1]);
    model.data.extend_from_slice(&stored);
    // A verbatim block follows in the same frame, so the bitstream must have
    // been realigned correctly by the odd-length pad byte.
    encoder.begin_block(Block::Verbatim, 24);
    for index in 0..20u32 {
        let byte = (index % 71) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    encoder.emit_match(4, 60);
    model.copy(4, 60);
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[125]), 15).unwrap(), model.data);
}

#[test]
fn decodes_a_block_that_spans_two_frames() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 1_600);
    for index in 0..800u32 {
        let byte = (index % 131) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    encoder.end_frame();
    // The second frame continues the same block and matches back into the
    // first frame's output.
    for _ in 0..100 {
        encoder.emit_match(8, 700);
        model.copy(8, 700);
    }
    assert_eq!(model.data.len(), 1_600);
    encoder.end_frame();

    assert_eq!(
        decode(&frames(encoder, &[800, 800]), 15).unwrap(),
        model.data
    );
}

#[test]
fn decodes_a_full_32768_byte_frame() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 32_768);
    for index in 0..768u32 {
        let byte = (index % 253) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    while model.data.len() < 32_768 {
        let length = (32_768 - model.data.len()).min(200);
        encoder.emit_match(length, 700);
        model.copy(length, 700);
    }
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[32_768]), 15).unwrap(), model.data);
}

#[test]
fn decodes_an_extra_length_match() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 700);
    for index in 0..400u32 {
        let byte = (index % 149) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    encoder.emit_match(300, 400);
    model.copy(300, 400);
    assert_eq!(model.data.len(), 700);
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[700]), 15).unwrap(), model.data);
}

#[test]
fn reverses_e8_call_translation() {
    // A frame with x86 CALL instructions, translated by the encoder exactly as
    // the format documents, must come back byte-identical.
    let file_size = 0x0001_0000u32;
    let mut original = vec![0u8; 400];
    for (index, byte) in original.iter_mut().enumerate() {
        *byte = u8::try_from(index % 241).expect("bounded by the modulus");
    }
    for start in [16usize, 120, 300] {
        original[start] = 0xE8;
        original[start + 1..start + 5].copy_from_slice(&0x0000_1234i32.to_le_bytes());
    }
    let mut translated = original.clone();
    e8_forward(&mut translated, 0, file_size);
    assert_ne!(translated, original);

    let mut encoder = LzxEncoder::new(15);
    encoder.header(Some(file_size));
    encoder.begin_block(Block::Verbatim, 400);
    for &byte in &translated {
        encoder.literal(byte);
    }
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[400]), 15).unwrap(), original);
}

#[test]
fn decodes_with_the_largest_cabinet_window() {
    let mut model = Model::default();
    let mut encoder = LzxEncoder::new(21);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 500);
    for index in 0..480u32 {
        let byte = (index % 173) as u8;
        encoder.literal(byte);
        model.literal(byte);
    }
    encoder.emit_match(20, 300);
    model.copy(20, 300);
    encoder.end_frame();

    assert_eq!(decode(&frames(encoder, &[500]), 21).unwrap(), model.data);
}

#[test]
fn rejects_an_invalid_block_type() {
    // Block type 0 is "not valid".
    let payload = vec![0x00u8, 0x00, 0x00, 0x00, 0x00, 0x00];
    assert_eq!(
        decode(&[(payload, 16)], 15),
        Err(CabError::DecompressionFailed)
    );
}

#[test]
fn rejects_a_match_that_reaches_before_the_start_of_the_folder() {
    let mut encoder = LzxEncoder::new(15);
    encoder.header(None);
    encoder.begin_block(Block::Verbatim, 40);
    for index in 0..10u32 {
        encoder.literal((index % 17) as u8);
    }
    // Nothing has been produced 5,000 bytes back.
    encoder.emit_match(30, 5_000);
    encoder.end_frame();
    assert_eq!(
        decode(&frames(encoder, &[40]), 15),
        Err(CabError::DecompressionFailed)
    );
}
