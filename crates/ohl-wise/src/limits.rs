//! Caller-supplied anti-abuse ceilings.
//!
//! Nothing in a Wise package bounds itself: the stream chain simply runs to
//! end of file and the script binary's counts are attacker-controlled. Every
//! walk in this crate is therefore bounded by one of these values.

/// Default staging buffer used for one inflate step.
pub const DEFAULT_CHUNK_BYTES: usize = 64 * 1024;

/// The documented header-scan ceiling: the largest distance from the start of
/// the overlay at which the first compressed stream is looked for.
pub const DEFAULT_HEADER_SCAN_BYTES: usize = 4 * 1024;

/// Ceilings applied to every parse and every walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest accepted `e_lfanew` plus PE header span.
    pub max_pe_header_bytes: u64,
    /// Largest accepted section count.
    pub max_sections: u16,
    /// Largest distance scanned from the overlay start for the first stream.
    pub max_header_scan_bytes: usize,
    /// Smallest number of inflated bytes a candidate stream must produce
    /// before the scan will consider it a real stream.
    pub min_confirmed_inflated_bytes: usize,
    /// Largest accepted number of streams in one chain.
    pub max_streams: u32,
    /// Largest accepted compressed size of one stream.
    pub max_compressed_bytes_per_stream: u64,
    /// Largest accepted inflated size of one stream.
    pub max_inflated_bytes_per_stream: u64,
    /// Largest accepted inflated size across one chain walk.
    pub max_total_inflated_bytes: u64,
    /// Largest accepted script binary.
    pub max_script_bytes: usize,
    /// Largest accepted number of file records in the script binary.
    pub max_file_records: u32,
    /// Largest accepted destination path, in stored bytes.
    pub max_path_bytes: usize,
    /// Largest byte skip tried when resynchronising after a bad stream.
    pub max_resync_skip: u8,
    /// Staging buffer size, and the ceiling on one inflate step.
    pub max_chunk_bytes: usize,
}

impl Limits {
    /// The default ceilings, also returned by [`Default::default`].
    pub const DEFAULT: Self = Self {
        max_pe_header_bytes: 16 * 1024 * 1024,
        max_sections: 96,
        max_header_scan_bytes: DEFAULT_HEADER_SCAN_BYTES,
        min_confirmed_inflated_bytes: 32,
        max_streams: 262_144,
        max_compressed_bytes_per_stream: 4 * 1024 * 1024 * 1024,
        max_inflated_bytes_per_stream: 4 * 1024 * 1024 * 1024,
        max_total_inflated_bytes: 64 * 1024 * 1024 * 1024,
        max_script_bytes: 64 * 1024 * 1024,
        max_file_records: 200_000,
        max_path_bytes: 4_096,
        max_resync_skip: 3,
        max_chunk_bytes: DEFAULT_CHUNK_BYTES,
    };

    /// The staging buffer size, never zero.
    #[must_use]
    pub const fn chunk_bytes(&self) -> usize {
        if self.max_chunk_bytes == 0 {
            1
        } else {
            self.max_chunk_bytes
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Limits;

    #[test]
    fn chunk_bytes_is_never_zero() {
        let limits = Limits {
            max_chunk_bytes: 0,
            ..Limits::DEFAULT
        };
        assert_eq!(limits.chunk_bytes(), 1);
    }
}
