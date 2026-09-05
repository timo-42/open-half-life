//! Adaptor from a pinned [`ohl_platform::MediaSource`] to the media-archive
//! [`BlockReader`] capability.
//!
//! This is the only place a `MediaSource` is turned into 2,048-byte logical
//! blocks for the readers in `ohl-iso9660` and `ohl-udf`. It never exposes a
//! pathname or an operating-system handle: every read is a bounds-checked
//! positional read through the pinned capability, and the source is
//! re-verified against its acquisition snapshot at construction and again
//! every [`MediaSourceBlockReader::verify_interval_blocks`] blocks read
//! afterwards, so a mount notices an in-place mutation of the pinned media
//! well before it can feed corrupted bytes to a third-party parser.

use std::sync::Arc;

use ohl_media_archive::{BLOCK_SIZE_U64, Block, BlockReader};
use ohl_platform::{MediaSource, MediaSourceError};

/// How many blocks are read between automatic re-verifications of the pinned
/// source, in addition to the one performed at construction.
///
/// This is a soft default a caller may lower (never raise unboundedly to the
/// point of never verifying) via
/// [`MediaSourceBlockReader::with_verify_interval`]; it does not gate
/// correctness the way `DirectoryLimits` does; it only trades verification
/// frequency for read throughput.
pub const DEFAULT_VERIFY_INTERVAL_BLOCKS: u64 = 4_096;

/// A [`BlockReader`] over an [`Arc<MediaSource>`], bounded by the source's
/// pinned size.
#[derive(Debug, Clone)]
pub struct MediaSourceBlockReader {
    source: Arc<MediaSource>,
    block_count: u64,
    verify_interval_blocks: u64,
    blocks_since_verify: u64,
}

impl MediaSourceBlockReader {
    /// Wraps `source`, verifying it against its acquisition snapshot once
    /// before returning, and using [`DEFAULT_VERIFY_INTERVAL_BLOCKS`] as the
    /// re-verification cadence.
    ///
    /// # Errors
    ///
    /// Returns the source's sanitized error when the initial verification
    /// fails, most notably [`MediaSourceError::Changed`] when the pinned
    /// object no longer matches the snapshot taken at acquisition.
    pub fn new(source: Arc<MediaSource>) -> Result<Self, MediaSourceError> {
        Self::with_verify_interval(source, DEFAULT_VERIFY_INTERVAL_BLOCKS)
    }

    /// Wraps `source` with an explicit re-verification cadence.
    ///
    /// A `verify_interval_blocks` of zero is treated as one: this reader
    /// always verifies at construction, and a caller cannot disable periodic
    /// re-verification entirely by supplying zero.
    ///
    /// # Errors
    ///
    /// Same as [`Self::new`].
    pub fn with_verify_interval(
        source: Arc<MediaSource>,
        verify_interval_blocks: u64,
    ) -> Result<Self, MediaSourceError> {
        source.verify_unchanged()?;
        let block_count = source.size() / BLOCK_SIZE_U64;
        Ok(Self {
            source,
            block_count,
            verify_interval_blocks: verify_interval_blocks.max(1),
            blocks_since_verify: 0,
        })
    }

    /// The configured re-verification cadence, in blocks.
    pub const fn verify_interval_blocks(&self) -> u64 {
        self.verify_interval_blocks
    }
}

impl BlockReader for MediaSourceBlockReader {
    type Error = MediaSourceError;

    fn read_block(&mut self, lba: u64, out: &mut Block) -> Result<(), Self::Error> {
        if lba >= self.block_count {
            return Err(MediaSourceError::OutOfRange);
        }
        let offset = lba
            .checked_mul(BLOCK_SIZE_U64)
            .ok_or(MediaSourceError::OutOfRange)?;
        self.source.read_exact_at(offset, out)?;

        // Overflow-safe saturating counter: reaching the interval verifies
        // and resets, and a counter that would overflow simply forces an
        // immediate verification instead of wrapping silently.
        self.blocks_since_verify = self
            .blocks_since_verify
            .checked_add(1)
            .unwrap_or(self.verify_interval_blocks);
        if self.blocks_since_verify >= self.verify_interval_blocks {
            self.blocks_since_verify = 0;
            self.source.verify_unchanged()?;
        }
        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::Arc;

    use ohl_media_archive::{BLOCK_SIZE, BlockReader};
    use ohl_platform::MediaSource;

    use super::MediaSourceBlockReader;

    fn write_temp_image(blocks: usize, byte_value: u8) -> tempfile::NamedTempFile {
        let mut temp_file = tempfile::NamedTempFile::new().expect("temp file");
        let image = vec![byte_value; blocks * BLOCK_SIZE];
        temp_file.write_all(&image).expect("write temp image");
        temp_file.flush().expect("flush temp image");
        temp_file
    }

    #[test]
    fn block_count_is_bounded_by_the_pinned_source_size() {
        let file = write_temp_image(3, 0xab);
        let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));
        let reader = MediaSourceBlockReader::new(source).expect("wrap source");
        assert_eq!(reader.block_count(), 3);
    }

    #[test]
    fn a_trailing_partial_block_is_not_counted() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(&vec![0u8; BLOCK_SIZE + 10]).expect("write");
        file.flush().expect("flush");
        let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));
        let reader = MediaSourceBlockReader::new(source).expect("wrap source");
        assert_eq!(reader.block_count(), 1);
    }

    #[test]
    fn reads_come_back_bounds_checked() {
        let file = write_temp_image(2, 0x11);
        let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));
        let mut reader = MediaSourceBlockReader::new(source).expect("wrap source");
        let mut block = [0u8; BLOCK_SIZE];
        assert!(reader.read_block(0, &mut block).is_ok());
        assert_eq!(block[0], 0x11);
        assert!(reader.read_block(2, &mut block).is_err());
        assert!(reader.read_block(u64::MAX, &mut block).is_err());
    }

    #[test]
    fn a_low_verify_interval_detects_truncation_mid_mount() {
        let file = write_temp_image(4, 0x22);
        let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));
        let mut reader =
            MediaSourceBlockReader::with_verify_interval(source, 1).expect("wrap source");
        let mut block = [0u8; BLOCK_SIZE];
        assert!(reader.read_block(0, &mut block).is_ok());

        // Truncate the pinned object after acquisition. The next read must
        // trip the verification the low interval schedules, even though the
        // requested block is still inside the pinned (stale) size.
        file.as_file()
            .set_len(BLOCK_SIZE as u64)
            .expect("truncate temp image");
        assert!(reader.read_block(1, &mut block).is_err());
    }

    #[test]
    fn a_zero_verify_interval_still_verifies_every_block() {
        let file = write_temp_image(2, 0x33);
        let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));
        let reader = MediaSourceBlockReader::with_verify_interval(source, 0).expect("wrap source");
        assert_eq!(reader.verify_interval_blocks(), 1);
    }
}
