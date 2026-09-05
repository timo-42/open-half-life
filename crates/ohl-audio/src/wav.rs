//! Clean-room, bounds-checked decoding of RIFF/WAVE (`.wav`) sound assets.
//!
//! GoldSrc's own sound assets are conventional RIFF/WAVE files: PCM 8-bit
//! unsigned or 16-bit signed, mono or stereo, at one of the common legacy
//! sample rates (8 kHz, 11.025 kHz, 22.05 kHz, 44.1 kHz), and looping sounds
//! additionally carry a `cue ` and/or `smpl` chunk marking the loop point.
//! All of this is exactly what the public RIFF ("Resource Interchange File
//! Format") and WAVE specifications describe; nothing here was learned from
//! GoldSrc or Valve source.
//!
//! Every chunk is walked with an explicit bounds check against the actual
//! input slice before it is read, every count (chunks, cue points, sample
//! loops, decoded frames) is capped by a fixed limit below, and no path
//! panics on malformed input: every fallible step returns
//! [`crate::error::AudioError`] instead. This mirrors `ohl-formats`'s BSP30
//! and WAD3 decoders.

use crate::error::{AudioError, Result};

/// Chunks scanned in a single WAV file.
pub const MAX_CHUNKS: usize = 64;
/// Cue points accepted from one `cue ` chunk.
pub const MAX_CUE_POINTS: usize = 1024;
/// Sample loops accepted from one `smpl` chunk.
pub const MAX_SAMPLE_LOOPS: usize = 256;
/// The largest `data` chunk this decoder accepts, in bytes (64 MiB is far
/// larger than any GoldSrc sound asset; this bound exists purely to keep a
/// hostile `data` chunk size field from driving an unbounded allocation).
pub const MAX_DATA_BYTES: u32 = 64 * 1024 * 1024;

const RIFF_HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;
const FMT_CHUNK_MIN_LEN: usize = 16;
const CUE_POINT_LEN: usize = 24;
const SMPL_HEADER_LEN: usize = 36;
const SAMPLE_LOOP_LEN: usize = 24;

const WAVE_FORMAT_PCM: u16 = 1;

/// Sample rates this decoder accepts, matching the common legacy rates
/// documented for GoldSrc sound assets (8/11.025/22.05/44.1 kHz), plus
/// 48 kHz for parity with modern capture hardware.
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[8_000, 11_025, 22_050, 44_100, 48_000];

/// The decoded `fmt ` chunk fields this decoder validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
}

/// One entry of a `cue ` chunk: a named marker into the `data` chunk.
///
/// `sample_offset` is the cue point's `dwSampleOffset` field: a frame index
/// (a frame is one sample per channel) from the start of the audio data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CuePoint {
    pub id: u32,
    pub sample_offset: u32,
}

/// One entry of a `smpl` chunk's sample-loop array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleLoop {
    pub cue_point_id: u32,
    pub loop_type: u32,
    pub start_frame: u32,
    pub end_frame: u32,
}

/// A fully decoded WAV asset: PCM samples normalized to `f32` in `[-1, 1]`,
/// interleaved by channel, plus any loop-point metadata found.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    pub format: WavFormat,
    /// Interleaved samples, `frame_count() * format.channels` long.
    pub samples: Vec<f32>,
    pub cue_points: Vec<CuePoint>,
    pub sample_loops: Vec<SampleLoop>,
}

impl DecodedWav {
    /// The number of decoded frames (one frame is one sample per channel).
    #[must_use]
    pub fn frame_count(&self) -> u32 {
        let channels = u32::from(self.format.channels);
        if channels == 0 {
            0
        } else {
            u32::try_from(self.samples.len())
                .unwrap_or(u32::MAX)
                .wrapping_div(channels)
        }
    }

    /// The GoldSrc-style effective loop range, in frames: `(start, end)`
    /// with `end` exclusive and clamped to `frame_count()`.
    ///
    /// Prefers the first `smpl` sample loop when present (it carries an
    /// explicit end frame). Otherwise, when a `cue ` chunk is present, loops
    /// from its first cue point's sample offset to the end of the data —
    /// the documented convention for a single-cue-point looping sound.
    /// Returns `None` when neither chunk is present (the sound does not
    /// loop).
    #[must_use]
    pub fn effective_loop(&self) -> Option<(u32, u32)> {
        let frame_count = self.frame_count();
        if let Some(sample_loop) = self.sample_loops.first() {
            let start = sample_loop.start_frame.min(frame_count);
            let end = sample_loop.end_frame.min(frame_count);
            if end > start {
                return Some((start, end));
            }
            return None;
        }
        if let Some(cue) = self.cue_points.first() {
            let start = cue.sample_offset.min(frame_count);
            if frame_count > start {
                return Some((start, frame_count));
            }
        }
        None
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16> {
    let end = offset.checked_add(2).ok_or(AudioError::OutOfBounds)?;
    let slice = bytes.get(offset..end).ok_or(AudioError::OutOfBounds)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).ok_or(AudioError::OutOfBounds)?;
    let slice = bytes.get(offset..end).ok_or(AudioError::OutOfBounds)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_tag(bytes: &[u8], offset: usize) -> Result<[u8; 4]> {
    let end = offset.checked_add(4).ok_or(AudioError::OutOfBounds)?;
    let slice = bytes.get(offset..end).ok_or(AudioError::OutOfBounds)?;
    Ok([slice[0], slice[1], slice[2], slice[3]])
}

struct RawChunk {
    id: [u8; 4],
    /// Byte range of the chunk's data, within `bytes`.
    start: usize,
    end: usize,
}

/// Walks the top-level RIFF chunk list, bounds-checking every header and
/// returning the raw `(id, byte range)` of each chunk found, capped at
/// [`MAX_CHUNKS`].
fn scan_chunks(bytes: &[u8]) -> Result<Vec<RawChunk>> {
    if bytes.len() < RIFF_HEADER_LEN {
        return Err(AudioError::Truncated);
    }
    if read_tag(bytes, 0)? != *b"RIFF" {
        return Err(AudioError::BadSignature);
    }
    if read_tag(bytes, 8)? != *b"WAVE" {
        return Err(AudioError::BadSignature);
    }

    let mut chunks = Vec::new();
    let mut offset = RIFF_HEADER_LEN;
    while offset + CHUNK_HEADER_LEN <= bytes.len() {
        if chunks.len() >= MAX_CHUNKS {
            return Err(AudioError::LimitExceeded);
        }
        let id = read_tag(bytes, offset)?;
        let size = read_u32_le(bytes, offset + 4)?;
        let data_start = offset + CHUNK_HEADER_LEN;
        let size = size as usize;
        let data_end = data_start
            .checked_add(size)
            .ok_or(AudioError::OutOfBounds)?;
        if data_end > bytes.len() {
            return Err(AudioError::OutOfBounds);
        }
        chunks.push(RawChunk {
            id,
            start: data_start,
            end: data_end,
        });
        // RIFF chunks are padded to an even byte boundary.
        let padded_size = size + (size & 1);
        offset = data_start
            .checked_add(padded_size)
            .ok_or(AudioError::OutOfBounds)?;
    }
    Ok(chunks)
}

fn parse_fmt(bytes: &[u8], chunk: &RawChunk) -> Result<WavFormat> {
    let data = &bytes[chunk.start..chunk.end];
    if data.len() < FMT_CHUNK_MIN_LEN {
        return Err(AudioError::Truncated);
    }
    let audio_format = read_u16_le(data, 0)?;
    let channels = read_u16_le(data, 2)?;
    let sample_rate = read_u32_le(data, 4)?;
    let block_align = read_u16_le(data, 12)?;
    let bits_per_sample = read_u16_le(data, 14)?;

    if audio_format != WAVE_FORMAT_PCM {
        return Err(AudioError::UnsupportedFormat);
    }
    if channels == 0 || channels > 2 {
        return Err(AudioError::UnsupportedChannelCount);
    }
    if bits_per_sample != 8 && bits_per_sample != 16 {
        return Err(AudioError::UnsupportedFormat);
    }
    if !SUPPORTED_SAMPLE_RATES.contains(&sample_rate) {
        return Err(AudioError::UnsupportedSampleRate);
    }
    let expected_block_align = channels * (bits_per_sample / 8);
    if block_align != expected_block_align {
        return Err(AudioError::InvalidInput);
    }

    Ok(WavFormat {
        channels,
        sample_rate,
        bits_per_sample,
    })
}

fn decode_samples(bytes: &[u8], chunk: &RawChunk, format: WavFormat) -> Result<Vec<f32>> {
    let data = &bytes[chunk.start..chunk.end];
    let data_len = u32::try_from(data.len()).map_err(|_| AudioError::LimitExceeded)?;
    if data_len > MAX_DATA_BYTES {
        return Err(AudioError::LimitExceeded);
    }

    let bytes_per_sample = usize::from(format.bits_per_sample / 8);
    let frame_bytes = bytes_per_sample * usize::from(format.channels);
    if frame_bytes == 0 || !data.len().is_multiple_of(frame_bytes) {
        return Err(AudioError::InvalidInput);
    }

    let mut samples = Vec::with_capacity(data.len() / bytes_per_sample);
    match format.bits_per_sample {
        8 => {
            for byte in data {
                samples.push((f32::from(*byte) - 128.0) / 128.0);
            }
        }
        16 => {
            let (pairs, _remainder) = data.as_chunks::<2>();
            for pair in pairs {
                let raw = i16::from_le_bytes(*pair);
                samples.push(f32::from(raw) / 32_768.0);
            }
        }
        _ => return Err(AudioError::UnsupportedFormat),
    }
    Ok(samples)
}

fn parse_cue(bytes: &[u8], chunk: &RawChunk) -> Result<Vec<CuePoint>> {
    let data = &bytes[chunk.start..chunk.end];
    if data.len() < 4 {
        return Err(AudioError::Truncated);
    }
    let count = read_u32_le(data, 0)? as usize;
    if count > MAX_CUE_POINTS {
        return Err(AudioError::LimitExceeded);
    }
    let required = 4 + count
        .checked_mul(CUE_POINT_LEN)
        .ok_or(AudioError::OutOfBounds)?;
    if data.len() < required {
        return Err(AudioError::OutOfBounds);
    }

    let mut points = Vec::with_capacity(count);
    for index in 0..count {
        let base = 4 + index * CUE_POINT_LEN;
        let id = read_u32_le(data, base)?;
        // Field layout: dwName(0), dwPosition(4), fccChunk(8),
        // dwChunkStart(12), dwBlockStart(16), dwSampleOffset(20).
        let sample_offset = read_u32_le(data, base + 20)?;
        points.push(CuePoint { id, sample_offset });
    }
    Ok(points)
}

fn parse_smpl(bytes: &[u8], chunk: &RawChunk) -> Result<Vec<SampleLoop>> {
    let data = &bytes[chunk.start..chunk.end];
    if data.len() < SMPL_HEADER_LEN {
        return Err(AudioError::Truncated);
    }
    // dwNumSampleLoops is the 8th u32 field (offset 28) of the smpl header.
    let count = read_u32_le(data, 28)? as usize;
    if count > MAX_SAMPLE_LOOPS {
        return Err(AudioError::LimitExceeded);
    }
    let required = SMPL_HEADER_LEN
        + count
            .checked_mul(SAMPLE_LOOP_LEN)
            .ok_or(AudioError::OutOfBounds)?;
    if data.len() < required {
        return Err(AudioError::OutOfBounds);
    }

    let mut loops = Vec::with_capacity(count);
    for index in 0..count {
        let base = SMPL_HEADER_LEN + index * SAMPLE_LOOP_LEN;
        let cue_point_id = read_u32_le(data, base)?;
        let loop_type = read_u32_le(data, base + 4)?;
        let start_frame = read_u32_le(data, base + 8)?;
        let end_frame = read_u32_le(data, base + 12)?;
        loops.push(SampleLoop {
            cue_point_id,
            loop_type,
            start_frame,
            end_frame,
        });
    }
    Ok(loops)
}

/// Decodes a RIFF/WAVE buffer into PCM samples plus any loop metadata.
///
/// Accepts only PCM 8-bit unsigned or 16-bit signed, mono or stereo, at one
/// of [`SUPPORTED_SAMPLE_RATES`]. Every chunk, cue point, and sample loop is
/// bounds-checked against `bytes` before being read; nothing here indexes
/// past the input or allocates in proportion to an unvalidated field.
pub fn decode(bytes: &[u8]) -> Result<DecodedWav> {
    let chunks = scan_chunks(bytes)?;

    let fmt_chunk = chunks
        .iter()
        .find(|chunk| chunk.id == *b"fmt ")
        .ok_or(AudioError::InvalidInput)?;
    let format = parse_fmt(bytes, fmt_chunk)?;

    let data_chunk = chunks
        .iter()
        .find(|chunk| chunk.id == *b"data")
        .ok_or(AudioError::InvalidInput)?;
    let samples = decode_samples(bytes, data_chunk, format)?;

    let cue_points = match chunks.iter().find(|chunk| chunk.id == *b"cue ") {
        Some(chunk) => parse_cue(bytes, chunk)?,
        None => Vec::new(),
    };
    let sample_loops = match chunks.iter().find(|chunk| chunk.id == *b"smpl") {
        Some(chunk) => parse_smpl(bytes, chunk)?,
        None => Vec::new(),
    };

    Ok(DecodedWav {
        format,
        samples,
        cue_points,
        sample_loops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn build_wav(
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        pcm_samples: &[i32],
    ) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("wav writer");
            for &sample in pcm_samples {
                if bits_per_sample == 8 {
                    let sample = i8::try_from(sample).expect("test sample fits i8");
                    writer.write_sample(sample).expect("write sample");
                } else {
                    let sample = i16::try_from(sample).expect("test sample fits i16");
                    writer.write_sample(sample).expect("write sample");
                }
            }
            writer.finalize().expect("finalize wav");
        }
        cursor.into_inner()
    }

    /// Appends a `cue ` chunk with one cue point at `sample_offset`, fixing
    /// up the RIFF size header. hound does not write this chunk itself.
    fn append_cue_chunk(mut wav: Vec<u8>, cue_id: u32, sample_offset: u32) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"cue ");
        chunk.extend_from_slice(&28u32.to_le_bytes()); // chunk size: count + one point
        chunk.extend_from_slice(&1u32.to_le_bytes()); // dwCuePoints
        chunk.extend_from_slice(&cue_id.to_le_bytes()); // dwName
        chunk.extend_from_slice(&0u32.to_le_bytes()); // dwPosition
        chunk.extend_from_slice(b"data"); // fccChunk
        chunk.extend_from_slice(&0u32.to_le_bytes()); // dwChunkStart
        chunk.extend_from_slice(&0u32.to_le_bytes()); // dwBlockStart
        chunk.extend_from_slice(&sample_offset.to_le_bytes()); // dwSampleOffset
        append_chunk(wav.as_mut(), &chunk);
        wav
    }

    fn append_smpl_chunk(mut wav: Vec<u8>, start_frame: u32, end_frame: u32) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"smpl");
        chunk.extend_from_slice(&60u32.to_le_bytes()); // chunk size: header + one loop
        // smpl header: manufacturer, product, samplePeriod, midiUnityNote,
        // midiPitchFraction, smpteFormat, smpteOffset, numSampleLoops (=1
        // here, the 8th field), samplerData.
        for field_index in 0..9 {
            let value = u32::from(field_index == 7);
            chunk.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(chunk.len(), 8 + SMPL_HEADER_LEN);
        chunk.extend_from_slice(&0u32.to_le_bytes()); // cuePointID
        chunk.extend_from_slice(&0u32.to_le_bytes()); // type: forward loop
        chunk.extend_from_slice(&start_frame.to_le_bytes());
        chunk.extend_from_slice(&end_frame.to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes()); // fraction
        chunk.extend_from_slice(&0u32.to_le_bytes()); // playCount
        append_chunk(&mut wav, &chunk);
        wav
    }

    fn append_chunk(wav: &mut Vec<u8>, chunk: &[u8]) {
        wav.extend_from_slice(chunk);
        if !chunk.len().is_multiple_of(2) {
            wav.push(0);
        }
        let new_riff_size = u32::try_from(wav.len() - 8).expect("test fixture fits u32");
        wav[4..8].copy_from_slice(&new_riff_size.to_le_bytes());
    }

    #[test]
    fn decodes_mono_16bit() {
        let wav = build_wav(1, 44_100, 16, &[0, 16_384, -16_384, 32_767, -32_768]);
        let decoded = decode(&wav).expect("decode");
        assert_eq!(decoded.format.channels, 1);
        assert_eq!(decoded.format.sample_rate, 44_100);
        assert_eq!(decoded.format.bits_per_sample, 16);
        assert_eq!(decoded.frame_count(), 5);
        assert!((decoded.samples[0] - 0.0).abs() < 1e-6);
        assert!((decoded.samples[3] - 0.999_969_5).abs() < 1e-5);
        assert!((decoded.samples[4] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn decodes_stereo_8bit() {
        // hound's 8-bit writer takes centered `i8` samples (`-128..=127`)
        // and biases them by +128 into the file's unsigned byte, the same
        // convention this decoder reverses.
        let wav = build_wav(2, 22_050, 8, &[-128, 127, 0, 0]);
        let decoded = decode(&wav).expect("decode");
        assert_eq!(decoded.format.channels, 2);
        assert_eq!(decoded.frame_count(), 2);
        assert!((decoded.samples[0] - (-1.0)).abs() < 1e-6);
        assert!((decoded.samples[1] - 0.992_187_5).abs() < 1e-6);
        assert!((decoded.samples[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_bad_signature() {
        let mut wav = build_wav(1, 8_000, 8, &[0, 1, 2]);
        wav[0] = b'X';
        assert_eq!(decode(&wav), Err(AudioError::BadSignature));
    }

    #[test]
    fn rejects_truncated_buffer() {
        assert_eq!(decode(b"RI"), Err(AudioError::Truncated));
    }

    #[test]
    fn rejects_unsupported_sample_rate() {
        let wav = build_wav(1, 12_345, 16, &[0, 1, 2]);
        assert_eq!(decode(&wav), Err(AudioError::UnsupportedSampleRate));
    }

    #[test]
    fn parses_cue_chunk_as_loop_start() {
        let wav = build_wav(1, 8_000, 16, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let wav = append_cue_chunk(wav, 1, 2);
        let decoded = decode(&wav).expect("decode");
        assert_eq!(decoded.cue_points.len(), 1);
        assert_eq!(decoded.cue_points[0].sample_offset, 2);
        assert_eq!(decoded.effective_loop(), Some((2, 8)));
    }

    #[test]
    fn parses_smpl_chunk_as_loop_range() {
        let wav = build_wav(1, 8_000, 16, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let wav = append_smpl_chunk(wav, 1, 6);
        let decoded = decode(&wav).expect("decode");
        assert_eq!(decoded.sample_loops.len(), 1);
        assert_eq!(decoded.sample_loops[0].start_frame, 1);
        assert_eq!(decoded.sample_loops[0].end_frame, 6);
        assert_eq!(decoded.effective_loop(), Some((1, 6)));
    }

    #[test]
    fn no_loop_metadata_means_no_loop() {
        let wav = build_wav(1, 8_000, 16, &[0, 1, 2, 3]);
        let decoded = decode(&wav).expect("decode");
        assert_eq!(decoded.effective_loop(), None);
    }

    #[test]
    fn smpl_takes_precedence_over_cue() {
        let wav = build_wav(1, 8_000, 16, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let wav = append_cue_chunk(wav, 1, 5);
        let wav = append_smpl_chunk(wav, 1, 4);
        let decoded = decode(&wav).expect("decode");
        assert_eq!(decoded.effective_loop(), Some((1, 4)));
    }
}
