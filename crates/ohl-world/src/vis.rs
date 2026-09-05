//! Decompressed potentially-visible sets.
//!
//! [`ohl_formats::bsp30::Bsp::is_leaf_visible`] answers one bit at a time by
//! decoding only as far as it must, which is what a bounds-checking decoder
//! should do. A renderer instead needs a whole row per frame, so this module
//! decompresses each leaf's run-length-encoded row once at load time into a
//! flat bitset. The encoding is the same one documented for the visibility
//! lump: a non-zero byte is eight literal bits, and a zero byte introduces a
//! run of that many all-zero bytes given by the following byte.

use crate::error::{Result, WorldError};

/// The largest decompressed PVS this crate will materialise. Beyond it the
/// set degrades to "everything visible" rather than allocating without
/// bound; the frustum test still culls.
pub const MAX_VIS_BYTES: usize = 16 * 1024 * 1024;

/// A leaf-to-leaf visibility bitset.
#[derive(Debug, Clone)]
pub struct VisibilitySet {
    leaf_count: usize,
    row_bytes: usize,
    rows: Vec<u8>,
    /// `false` when the map had no usable visibility data and every query
    /// answers "visible".
    decoded: bool,
}

impl VisibilitySet {
    /// A set in which every leaf sees every other leaf.
    #[must_use]
    pub fn all_visible(leaf_count: usize) -> Self {
        Self {
            leaf_count,
            row_bytes: 0,
            rows: Vec::new(),
            decoded: false,
        }
    }

    /// Builds the set from the raw visibility lump and each leaf's
    /// `vis_offset`.
    ///
    /// A leaf with a negative offset (the shared outside leaf, or a map
    /// compiled without visibility) sees everything. A row that fails to
    /// decode is likewise treated as "sees everything", so a partially
    /// damaged lump degrades instead of failing the load.
    pub fn build(vis_lump: &[u8], vis_offsets: &[i32]) -> Result<Self> {
        let leaf_count = vis_offsets.len();
        let row_bytes = leaf_count.div_ceil(8);
        let total = row_bytes
            .checked_mul(leaf_count)
            .ok_or(WorldError::LimitExceeded)?;
        if total > MAX_VIS_BYTES {
            return Ok(Self::all_visible(leaf_count));
        }

        let mut rows = vec![0u8; total];
        for (leaf, &offset) in vis_offsets.iter().enumerate() {
            let row = &mut rows[leaf * row_bytes..(leaf + 1) * row_bytes];
            let start = usize::try_from(offset).ok();
            let decoded = match start {
                Some(start) if offset >= 0 => decompress_row(vis_lump, start, row).is_ok(),
                _ => false,
            };
            if !decoded {
                row.fill(0xFF);
            }
        }

        Ok(Self {
            leaf_count,
            row_bytes,
            rows,
            decoded: true,
        })
    }

    /// The number of leaves the set was built for.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.leaf_count
    }

    /// Whether `to_leaf` is potentially visible from `from_leaf`.
    ///
    /// Leaf 0 is the shared "outside" leaf and has no visibility row, so a
    /// query from it conservatively answers `true`. Bit `n` of a row refers
    /// to leaf `n + 1`, matching the lump's leaf-1-based bit numbering.
    #[must_use]
    pub fn is_visible(&self, from_leaf: usize, to_leaf: usize) -> bool {
        if !self.decoded || from_leaf == 0 || from_leaf >= self.leaf_count {
            return true;
        }
        let Some(bit) = to_leaf.checked_sub(1) else {
            return true;
        };
        let row = &self.rows[from_leaf * self.row_bytes..(from_leaf + 1) * self.row_bytes];
        match row.get(bit / 8) {
            Some(byte) => (byte >> (bit % 8)) & 1 != 0,
            None => false,
        }
    }
}

/// Decompresses one run-length-encoded PVS row into `out`, which is filled
/// completely (any tail the encoding does not reach is left zeroed).
fn decompress_row(vis: &[u8], start: usize, out: &mut [u8]) -> Result<()> {
    out.fill(0);
    let mut pos = start;
    let mut written = 0usize;
    while written < out.len() {
        let marker = *vis.get(pos).ok_or(WorldError::IndexOutOfRange)?;
        pos = pos.checked_add(1).ok_or(WorldError::IndexOutOfRange)?;
        if marker == 0 {
            let run = *vis.get(pos).ok_or(WorldError::IndexOutOfRange)? as usize;
            pos = pos.checked_add(1).ok_or(WorldError::IndexOutOfRange)?;
            if run == 0 {
                // A zero-length run can never advance; reject rather than
                // spin.
                return Err(WorldError::IndexOutOfRange);
            }
            written = written.saturating_add(run).min(out.len());
        } else {
            out[written] = marker;
            written += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{VisibilitySet, decompress_row};

    #[test]
    fn decompresses_literals_and_runs() {
        let mut out = [0u8; 5];
        // 0xAB, three zero bytes, 0xCD
        decompress_row(&[0xAB, 0x00, 0x03, 0xCD], 0, &mut out).expect("valid row");
        assert_eq!(out, [0xAB, 0x00, 0x00, 0x00, 0xCD]);
    }

    #[test]
    fn rejects_truncated_and_zero_runs() {
        let mut out = [0u8; 4];
        assert!(decompress_row(&[0xAB], 0, &mut out).is_err());
        assert!(decompress_row(&[0x00, 0x00], 0, &mut out).is_err());
    }

    #[test]
    fn negative_offsets_see_everything() {
        let set = VisibilitySet::build(&[], &[-1, -1, -1]).expect("builds");
        assert!(set.is_visible(1, 2));
        assert_eq!(set.leaf_count(), 3);
    }

    #[test]
    fn all_visible_answers_true() {
        let set = VisibilitySet::all_visible(4);
        assert!(set.is_visible(1, 3));
    }

    #[test]
    fn decoded_rows_hide_invisible_leaves() {
        // Two leaves plus leaf 0: leaf 1's row has only bit 0 set, so it
        // sees leaf 1 but not leaf 2.
        let vis = [0b0000_0001u8, 0b0000_0011u8];
        let set = VisibilitySet::build(&vis, &[-1, 0, 1]).expect("builds");
        assert!(set.is_visible(1, 1));
        assert!(!set.is_visible(1, 2));
        assert!(set.is_visible(2, 1));
        assert!(set.is_visible(2, 2));
        // Leaf 0 is the outside leaf and never culls.
        assert!(set.is_visible(0, 2));
    }
}
