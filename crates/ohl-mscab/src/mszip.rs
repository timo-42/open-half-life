//! MSZIP block decoding.
//!
//! From [MS-MCI] section 2: "Each MSZIP block MUST consist of a 2-byte MSZIP
//! signature and one or more RFC 1951 blocks. The 2-byte MSZIP signature MUST
//! consist of the bytes 0x43 and 0x4B" (`CK`), "the last RFC 1951 block in
//! each MSZIP block MUST be marked as the 'end' of the stream", "decoding
//! trees MUST be discarded after each RFC 1951 block, but the history buffer
//! MUST be maintained", and "each MSZIP block MUST represent no more than 32
//! KB of uncompressed data".
//!
//! The history buffer is therefore carried across the blocks of one cabinet
//! folder: this decoder keeps a single 32 KiB window across every `CFDATA`
//! block of the folder and restarts only the RFC 1951 stream state for each
//! block. `miniz_oxide`'s wrapping output-buffer mode implements exactly that
//! window, so the previous block's bytes remain addressable as match history.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress};

use crate::error::{CabError, Result};
use crate::limits::MAX_BLOCK_UNCOMPRESSED;

/// The MSZIP block signature, `CK`.
pub const SIGNATURE: [u8; 2] = [0x43, 0x4B];

/// The window size the format fixes, and the size of the ring buffer that
/// carries history between blocks.
const WINDOW: usize = MAX_BLOCK_UNCOMPRESSED as usize;

pub(crate) struct MsZipDecoder {
    state: DecompressorOxide,
    window: Vec<u8>,
    position: usize,
}

impl MsZipDecoder {
    pub(crate) fn new() -> Self {
        Self {
            state: DecompressorOxide::new(),
            window: vec![0u8; WINDOW],
            position: 0,
        }
    }

    /// Decodes one `CFDATA` payload into `out`, which is cleared first.
    ///
    /// `expected` is `CFDATA.cbUncomp`; a block that decodes to a different
    /// length is rejected rather than silently accepted.
    pub(crate) fn decode_block(
        &mut self,
        input: &[u8],
        expected: usize,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        out.clear();
        if expected > WINDOW {
            return Err(CabError::LimitExceeded);
        }
        let payload = input
            .split_at_checked(SIGNATURE.len())
            .filter(|(signature, _)| *signature == SIGNATURE)
            .map(|(_, payload)| payload)
            .ok_or(CabError::DecompressionFailed)?;

        self.state.init();
        let mut consumed = 0usize;
        loop {
            let start = self.position;
            let (status, used, written) = decompress(
                &mut self.state,
                &payload[consumed..],
                &mut self.window,
                start,
                0,
            );
            consumed = consumed
                .checked_add(used)
                .ok_or(CabError::DecompressionFailed)?;
            if written > 0 {
                if out.len() + written > expected {
                    return Err(CabError::DecompressionFailed);
                }
                copy_from_ring(&self.window, start, written, out)?;
                self.position = (start + written) % WINDOW;
            }
            match status {
                TINFLStatus::Done => break,
                TINFLStatus::HasMoreOutput => {
                    if written == 0 {
                        return Err(CabError::DecompressionFailed);
                    }
                }
                _ => return Err(CabError::DecompressionFailed),
            }
        }
        if out.len() != expected {
            return Err(CabError::DecompressionFailed);
        }
        Ok(())
    }
}

fn copy_from_ring(window: &[u8], start: usize, len: usize, out: &mut Vec<u8>) -> Result<()> {
    out.try_reserve(len).map_err(|_| CabError::LimitExceeded)?;
    let first = len.min(WINDOW - start);
    out.extend_from_slice(window.get(start..start + first).ok_or(CabError::Internal)?);
    if first < len {
        out.extend_from_slice(window.get(..len - first).ok_or(CabError::Internal)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec::Vec;

    use super::{MsZipDecoder, SIGNATURE};
    use crate::error::CabError;

    fn block(data: &[u8]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(data, 6));
        out
    }

    #[test]
    fn round_trips_a_single_block() {
        let payload: Vec<u8> = (0..5_000u32).map(|value| (value % 251) as u8).collect();
        let mut decoder = MsZipDecoder::new();
        let mut out = Vec::new();
        decoder
            .decode_block(&block(&payload), payload.len(), &mut out)
            .expect("decodes");
        assert_eq!(out, payload);
    }

    #[test]
    fn rejects_a_missing_ck_signature() {
        let mut decoder = MsZipDecoder::new();
        let mut out = Vec::new();
        let mut bad = block(b"hello");
        bad[0] = b'X';
        assert_eq!(
            decoder.decode_block(&bad, 5, &mut out),
            Err(CabError::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_a_length_that_disagrees_with_cbuncomp() {
        let mut decoder = MsZipDecoder::new();
        let mut out = Vec::new();
        assert_eq!(
            decoder.decode_block(&block(b"hello"), 4, &mut out),
            Err(CabError::DecompressionFailed)
        );
    }

    #[test]
    fn rejects_truncated_deflate_data() {
        let mut decoder = MsZipDecoder::new();
        let mut out = Vec::new();
        let full = block(b"hello world, hello world");
        let truncated = &full[..full.len() - 2];
        assert_eq!(
            decoder.decode_block(truncated, 24, &mut out),
            Err(CabError::DecompressionFailed)
        );
    }
}
