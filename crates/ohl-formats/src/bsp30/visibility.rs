//! Potentially-visible-set (PVS) decoding.
//!
//! The visibility lump is a run-length-encoded bit vector per leaf: a `0`
//! byte begins a run of empty (all-zero) decompressed bytes whose count is
//! the following byte (Unofficial Quake Specs section 4, lump 4
//! "visilist"; GoldSrc BSP v30 keeps the same encoding).
//!
//! Decoding here is lazy and bounded: [`is_visible`] decodes only as many
//! compressed bytes as needed to reach the requested bit, and never reads or
//! iterates past `decompressed_len_bytes`.

use crate::error::{FormatError, Result};

/// Decodes whether `bit_index` (a 0-based index into the decompressed PVS
/// bit vector for one leaf's visibility lump slice) is set.
///
/// `decompressed_len_bytes` bounds how many decompressed bytes the caller
/// considers valid (typically `ceil(leaf_count / 8)`); a run that would
/// produce more than that is rejected as an overrun rather than silently
/// truncated.
pub fn is_visible(
    vis: &[u8],
    start: usize,
    bit_index: usize,
    decompressed_len_bytes: usize,
) -> Result<bool> {
    let target_byte = bit_index / 8;
    if target_byte >= decompressed_len_bytes {
        return Err(FormatError::IndexOutOfRange);
    }
    let bit_in_byte = bit_index % 8;

    let mut pos = start;
    let mut byte_index = 0usize;
    loop {
        if byte_index > decompressed_len_bytes {
            return Err(FormatError::InvalidInput);
        }
        let marker = *vis.get(pos).ok_or(FormatError::OutOfBounds)?;
        pos = pos.checked_add(1).ok_or(FormatError::OutOfBounds)?;
        if marker == 0 {
            let run_len = *vis.get(pos).ok_or(FormatError::OutOfBounds)? as usize;
            pos = pos.checked_add(1).ok_or(FormatError::OutOfBounds)?;
            if run_len == 0 {
                // A zero-length run can never advance; treat as malformed
                // rather than looping forever.
                return Err(FormatError::InvalidInput);
            }
            let run_end = byte_index
                .checked_add(run_len)
                .ok_or(FormatError::OutOfBounds)?;
            if run_end > decompressed_len_bytes {
                return Err(FormatError::InvalidInput);
            }
            if target_byte < run_end {
                return Ok(false);
            }
            byte_index = run_end;
        } else {
            if byte_index == target_byte {
                return Ok((marker >> bit_in_byte) & 1 != 0);
            }
            byte_index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_visible;

    #[test]
    fn decodes_a_literal_byte() {
        let vis = [0b0000_0101u8];
        assert!(is_visible(&vis, 0, 0, 1).unwrap());
        assert!(!is_visible(&vis, 0, 1, 1).unwrap());
        assert!(is_visible(&vis, 0, 2, 1).unwrap());
    }

    #[test]
    fn decodes_a_zero_run() {
        // 0x00 0x03 => three zero bytes, i.e. bits 0..24 all clear.
        let vis = [0x00u8, 0x03];
        for bit in 0..24 {
            assert!(!is_visible(&vis, 0, bit, 3).unwrap());
        }
    }

    #[test]
    fn rejects_overrun() {
        let vis = [0x00u8, 0xFF]; // claims 255 zero bytes.
        assert!(is_visible(&vis, 0, 0, 3).is_err());
    }

    #[test]
    fn rejects_zero_length_run() {
        let vis = [0x00u8, 0x00];
        assert!(is_visible(&vis, 0, 0, 3).is_err());
    }

    #[test]
    fn rejects_truncated_stream() {
        let vis: [u8; 0] = [];
        assert!(is_visible(&vis, 0, 0, 1).is_err());
    }
}
