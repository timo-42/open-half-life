//! `CFDATA` block framing and the documented cabinet checksum.

extern crate alloc;

use crate::bytes::Reader;
use crate::error::{CabError, Result};
use crate::limits::{MAX_BLOCK_COMPRESSED, MAX_BLOCK_UNCOMPRESSED};

/// The size in bytes of a `CFDATA` header without its reserved area.
pub(crate) const DATA_FIXED_LEN: usize = 8;

/// The `CFDATA` header fields, already bounds-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DataHeader {
    pub(crate) checksum: u32,
    pub(crate) compressed_bytes: u16,
    pub(crate) uncompressed_bytes: u16,
}

impl DataHeader {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let checksum = reader.u32()?;
        let compressed_bytes = reader.u16()?;
        let uncompressed_bytes = reader.u16()?;
        if compressed_bytes > MAX_BLOCK_COMPRESSED || uncompressed_bytes > MAX_BLOCK_UNCOMPRESSED {
            return Err(CabError::LimitExceeded);
        }
        Ok(Self {
            checksum,
            compressed_bytes,
            uncompressed_bytes,
        })
    }
}

/// The cabinet checksum from [MS-CAB] `CSUMCompute`: fold the data into a
/// 32-bit accumulator as little-endian `ULONG`s, then fold any 1..=3 trailing
/// bytes.
///
/// The documented tail handling is a `switch` on `cb % 4` whose cases fall
/// through while advancing the byte pointer, so three trailing bytes are
/// folded as `b0 << 16 | b1 << 8 | b2`, two as `b0 << 8 | b1`, and one as
/// `b0` -- the opposite byte order from the whole-`ULONG` case.
#[must_use]
pub fn checksum(data: &[u8], seed: u32) -> u32 {
    let mut sum = seed;
    let (chunks, rest) = data.as_chunks::<4>();
    for chunk in chunks {
        sum ^= u32::from_le_bytes(*chunk);
    }
    let tail = match rest {
        [first, second, third] => {
            (u32::from(*first) << 16) | (u32::from(*second) << 8) | u32::from(*third)
        }
        [first, second] => (u32::from(*first) << 8) | u32::from(*second),
        [first] => u32::from(*first),
        _ => return sum,
    };
    sum ^ tail
}

/// Verifies a `CFDATA` checksum.
///
/// [MS-CAB] computes the block checksum over `CFDATA.ab[]` first and then over
/// the `CFDATA` header from `cbData` onwards, seeded with that partial value.
/// The document writes the header span as `sizeof(CFDATA) - sizeof(csum)`,
/// which is the 4 bytes `cbData` and `cbUncomp` for a cabinet with no
/// per-block reserved area. When a reserved area *is* present the span is
/// ambiguous, so a block is accepted if it matches either reading; blocks
/// without a reserved area have only one reading and are checked exactly.
///
/// `csum` of zero means "not supplied" and is not verified.
pub(crate) fn verify_block_checksum(
    header: DataHeader,
    reserve: &[u8],
    data: &[u8],
) -> Result<bool> {
    if header.checksum == 0 {
        return Ok(false);
    }
    let partial = checksum(data, 0);
    let mut fields = [0u8; 4];
    fields[..2].copy_from_slice(&header.compressed_bytes.to_le_bytes());
    fields[2..].copy_from_slice(&header.uncompressed_bytes.to_le_bytes());
    if checksum(&fields, partial) == header.checksum {
        return Ok(true);
    }
    if !reserve.is_empty() {
        let with_reserve = checksum(reserve, checksum(&fields, partial));
        if with_reserve == header.checksum {
            return Ok(true);
        }
    }
    Err(CabError::ChecksumMismatch)
}

#[cfg(test)]
mod tests {
    use super::{DataHeader, checksum, verify_block_checksum};
    use crate::error::CabError;

    #[test]
    fn checksum_folds_whole_words_and_the_documented_tail() {
        assert_eq!(checksum(&[], 0), 0);
        assert_eq!(checksum(&[0x01, 0x02, 0x03, 0x04], 0), 0x0403_0201);
        assert_eq!(checksum(&[0x01, 0x02, 0x03], 0), 0x0001_0203);
        assert_eq!(checksum(&[0x01, 0x02], 0), 0x0000_0102);
        assert_eq!(checksum(&[0x01], 0), 0x0000_0001);
        assert_eq!(checksum(&[0x01, 0x02, 0x03, 0x04], 0x0403_0201), 0);
    }

    #[test]
    fn zero_checksum_is_treated_as_absent() {
        let header = DataHeader {
            checksum: 0,
            compressed_bytes: 3,
            uncompressed_bytes: 3,
        };
        assert_eq!(verify_block_checksum(header, &[], b"abc"), Ok(false));
    }

    #[test]
    fn a_wrong_checksum_is_rejected() {
        let header = DataHeader {
            checksum: 0xDEAD_BEEF,
            compressed_bytes: 3,
            uncompressed_bytes: 3,
        };
        assert_eq!(
            verify_block_checksum(header, &[], b"abc"),
            Err(CabError::ChecksumMismatch)
        );
    }
}
