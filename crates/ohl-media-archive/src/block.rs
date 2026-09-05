//! The only byte source these crates accept.

use ohl_core::SanitizedError;

/// The logical block size every supported optical media class uses.
///
/// ECMA-119 section 6.2 and ECMA-167 part 3 both allow other sizes; the
/// project deliberately supports only 2,048-byte logical blocks, and both
/// preflights reject anything else rather than reinterpreting geometry.
pub const BLOCK_SIZE: usize = 2_048;

/// [`BLOCK_SIZE`] as a `u32`, so geometry arithmetic needs no cast.
pub const BLOCK_SIZE_U32: u32 = 2_048;

/// [`BLOCK_SIZE`] as a `u64`, so extent arithmetic needs no cast.
pub const BLOCK_SIZE_U64: u64 = 2_048;

/// One logical block.
pub type Block = [u8; BLOCK_SIZE];

/// A bounded, positional source of 2,048-byte logical blocks.
///
/// The trait deliberately exposes no pathname, file descriptor, or other
/// operating-system handle: a reader is a capability that was acquired and
/// pinned elsewhere. Implementations must treat `lba` as untrusted and report
/// an error rather than panicking when it is out of range.
pub trait BlockReader {
    /// The source's own error type. It is converted into a sanitized code
    /// before it can reach any diagnostic, so a source may keep richer
    /// internal detail without risking disclosure.
    type Error: Into<SanitizedError>;

    /// Reads the whole logical block at `lba` into `out`.
    ///
    /// # Errors
    ///
    /// Returns the source error when `lba` is outside the source or the read
    /// could not be completed exactly.
    fn read_block(&mut self, lba: u64, out: &mut Block) -> Result<(), Self::Error>;

    /// The number of whole logical blocks the source contains.
    ///
    /// A source whose length is not a whole number of logical blocks reports
    /// only the whole blocks; the trailing partial block is never readable.
    fn block_count(&self) -> u64;
}

impl<T: BlockReader + ?Sized> BlockReader for &mut T {
    type Error = T::Error;

    fn read_block(&mut self, lba: u64, out: &mut Block) -> Result<(), Self::Error> {
        (**self).read_block(lba, out)
    }

    fn block_count(&self) -> u64 {
        (**self).block_count()
    }
}

/// A [`BlockReader`] over an in-memory image, used by tests and by callers
/// that already hold the whole medium.
#[derive(Debug, Clone, Copy)]
pub struct SliceBlockReader<'a> {
    image: &'a [u8],
}

impl<'a> SliceBlockReader<'a> {
    /// Wraps `image`. Trailing bytes that do not complete a logical block are
    /// never readable.
    pub const fn new(image: &'a [u8]) -> Self {
        Self { image }
    }
}

impl BlockReader for SliceBlockReader<'_> {
    type Error = SanitizedError;

    fn read_block(&mut self, lba: u64, out: &mut Block) -> Result<(), Self::Error> {
        if lba >= self.block_count() {
            return Err(SanitizedError::InvalidInput);
        }
        let start = usize::try_from(lba)
            .ok()
            .and_then(|lba| lba.checked_mul(BLOCK_SIZE))
            .ok_or(SanitizedError::ArithmeticOverflow)?;
        let end = start
            .checked_add(BLOCK_SIZE)
            .ok_or(SanitizedError::ArithmeticOverflow)?;
        out.copy_from_slice(&self.image[start..end]);
        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.image.len() as u64 / BLOCK_SIZE_U64
    }
}

#[cfg(test)]
mod tests {
    use super::{BLOCK_SIZE, BlockReader, SliceBlockReader};
    use ohl_core::SanitizedError;

    #[test]
    fn a_partial_trailing_block_is_not_readable() {
        let image = alloc::vec![7u8; BLOCK_SIZE + 5];
        let mut reader = SliceBlockReader::new(&image);
        assert_eq!(reader.block_count(), 1);
        let mut block = [0u8; BLOCK_SIZE];
        assert!(reader.read_block(0, &mut block).is_ok());
        assert_eq!(block[0], 7);
        assert_eq!(
            reader.read_block(1, &mut block),
            Err(SanitizedError::InvalidInput)
        );
    }

    #[test]
    fn an_out_of_range_block_is_rejected_rather_than_panicking() {
        let mut reader = SliceBlockReader::new(&[]);
        let mut block = [0u8; BLOCK_SIZE];
        assert!(reader.read_block(u64::MAX, &mut block).is_err());
    }
}
