//! `proptest`-driven fuzzing of `ohl_audio::wav::decode`: no arbitrary byte
//! string, and no single-byte mutation of an otherwise valid synthetic WAV,
//! may ever make it panic. This mirrors the same-shaped harnesses in
//! `ohl-formats` (`tests/proptest_never_panics.rs`) and
//! `ohl-cabinet-format` (`tests/never_panics.rs`).

use ohl_audio::wav::decode;
use proptest::prelude::*;
use std::io::Cursor;

/// Runs the decoder and touches every accessor on a successful result;
/// malformed input is expected and ignored, only a panic is a failure.
fn exercise(bytes: &[u8]) {
    let Ok(wav) = decode(bytes) else {
        return;
    };
    let _ = wav.frame_count();
    let _ = wav.effective_loop();
    let _ = wav.format;
    for cue in &wav.cue_points {
        let _ = cue.sample_offset;
    }
    for sample_loop in &wav.sample_loops {
        let _ = (sample_loop.start_frame, sample_loop.end_frame);
    }
}

/// Builds a valid, decodable mono 16-bit WAV, then appends a `cue ` and a
/// `smpl` chunk by hand (the same byte layout as `src/wav.rs`'s own unit
/// tests), so mutation has a realistic, fully-populated file to corrupt.
fn valid_synthetic_wav() -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("wav writer");
        for sample in [0i16, 1000, -1000, 32_767, -32_768, 42, -42, 7] {
            writer.write_sample(sample).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
    }
    let mut wav = cursor.into_inner();

    append_chunk(&mut wav, &cue_chunk(1, 2));
    append_chunk(&mut wav, &smpl_chunk(1, 6));
    wav
}

fn cue_chunk(cue_id: u32, sample_offset: u32) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"cue ");
    chunk.extend_from_slice(&28u32.to_le_bytes());
    chunk.extend_from_slice(&1u32.to_le_bytes());
    chunk.extend_from_slice(&cue_id.to_le_bytes());
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk.extend_from_slice(b"data");
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk.extend_from_slice(&sample_offset.to_le_bytes());
    chunk
}

fn smpl_chunk(start_frame: u32, end_frame: u32) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"smpl");
    chunk.extend_from_slice(&60u32.to_le_bytes());
    for field_index in 0..9 {
        let value = u32::from(field_index == 7);
        chunk.extend_from_slice(&value.to_le_bytes());
    }
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk.extend_from_slice(&start_frame.to_le_bytes());
    chunk.extend_from_slice(&end_frame.to_le_bytes());
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk.extend_from_slice(&0u32.to_le_bytes());
    chunk
}

fn append_chunk(wav: &mut Vec<u8>, chunk: &[u8]) {
    wav.extend_from_slice(chunk);
    if !chunk.len().is_multiple_of(2) {
        wav.push(0);
    }
    let new_riff_size = u32::try_from(wav.len() - 8).expect("test fixture fits u32");
    wav[4..8].copy_from_slice(&new_riff_size.to_le_bytes());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise(&bytes);
    }

    #[test]
    fn corrupting_a_real_wav_never_panics(
        index in 0usize..256,
        value in any::<u8>(),
    ) {
        let mut bytes = valid_synthetic_wav();
        if index < bytes.len() {
            bytes[index] = value;
        }
        exercise(&bytes);
    }
}

#[test]
fn valid_synthetic_wav_actually_decodes() {
    // Sanity check for the fixture itself: the mutation test above is only
    // meaningful if the unmutated fixture decodes successfully.
    let wav = valid_synthetic_wav();
    let decoded = decode(&wav).expect("synthetic fixture decodes");
    assert_eq!(decoded.frame_count(), 8);
    assert_eq!(decoded.effective_loop(), Some((1, 6)));
}
