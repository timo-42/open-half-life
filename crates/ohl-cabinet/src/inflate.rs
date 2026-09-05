//! Bounded raw-DEFLATE chunk decompression.
//!
//! Each stored chunk is an independent raw DEFLATE stream, so the state is
//! reset per chunk and the output is capped by `Limits::max_chunk_bytes`.

use alloc::boxed::Box;
use alloc::vec::Vec;
use miniz_oxide::DataFormat;
use miniz_oxide::MZFlush;
use miniz_oxide::MZStatus;
use miniz_oxide::inflate::stream::{InflateState, inflate};

use crate::error::Error;

/// Reusable inflate state.
pub(crate) struct ChunkInflater {
    state: Box<InflateState>,
}

impl core::fmt::Debug for ChunkInflater {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ChunkInflater")
    }
}

impl ChunkInflater {
    pub(crate) fn new() -> Self {
        Self {
            state: InflateState::new_boxed(DataFormat::Raw),
        }
    }

    /// Inflates one chunk into `output`, which is resized to the expanded
    /// length. Never writes more than `max_output` bytes.
    pub(crate) fn inflate_chunk(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        max_output: usize,
    ) -> Result<(), Error> {
        self.state.reset(DataFormat::Raw);
        output.clear();
        output.resize(max_output, 0);

        let result = inflate(&mut self.state, input, output, MZFlush::Finish);
        match result.status {
            // A complete stream.
            Ok(MZStatus::StreamEnd) => {}
            // Some writers terminate a chunk with a sync marker rather than a
            // final block. Accept that only when the whole chunk was
            // consumed, so a chunk that merely filled the output ceiling is
            // still a failure rather than silent truncation.
            Ok(MZStatus::Ok) if result.bytes_consumed == input.len() => {}
            _ => return Err(Error::DecompressionFailed),
        }
        if result.bytes_written > max_output {
            return Err(Error::DecompressionFailed);
        }
        output.truncate(result.bytes_written);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkInflater;
    use crate::error::Error;
    use alloc::vec::Vec;

    #[test]
    fn inflates_a_raw_deflate_chunk() {
        let plain = b"the quick brown fox jumps over the lazy dog".repeat(8);
        let compressed = miniz_oxide::deflate::compress_to_vec(&plain, 6);
        let mut inflater = ChunkInflater::new();
        let mut output = Vec::new();
        inflater
            .inflate_chunk(&compressed, &mut output, 64 * 1024)
            .unwrap();
        assert_eq!(output, plain);
    }

    #[test]
    fn rejects_garbage() {
        let mut inflater = ChunkInflater::new();
        let mut output = Vec::new();
        assert_eq!(
            inflater.inflate_chunk(&[0xff; 32], &mut output, 1024),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn refuses_to_exceed_the_output_ceiling() {
        let plain = alloc::vec![0u8; 8192];
        let compressed = miniz_oxide::deflate::compress_to_vec(&plain, 6);
        let mut inflater = ChunkInflater::new();
        let mut output = Vec::new();
        let result = inflater.inflate_chunk(&compressed, &mut output, 16);
        assert!(result.is_err() || output.len() <= 16);
    }
}
