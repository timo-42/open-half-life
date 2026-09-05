//! A `Read + Seek` adaptor over a [`BlockReader`].
//!
//! `hadris-iso` reads through its own portable `Read + Seek` traits. This
//! adaptor is the only place where the project's block capability is turned
//! into a byte stream, and it is deliberately the narrowest possible bridge:
//! it never exposes a pathname or an operating-system handle, it clamps every
//! access to the block source's declared length, and it converts the source's
//! error into a fixed, sanitized I/O kind so no media-derived detail can leak
//! into a `hadris` diagnostic.

use hadris_iso::{Error as IsoError, ErrorKind, IoResult as IsoResult, Read, Seek, SeekFrom};
use ohl_media_archive::{BLOCK_SIZE, BLOCK_SIZE_U64, Block, BlockReader};

/// A byte-stream view of a [`BlockReader`], caching one logical block.
#[derive(Debug)]
pub struct BlockCursor<R: BlockReader> {
    reader: R,
    length: u64,
    position: u64,
    cached_lba: Option<u64>,
    cache: Block,
}

impl<R: BlockReader> BlockCursor<R> {
    /// Wraps `reader`. The stream length is the source's whole-block length.
    pub fn new(reader: R) -> Self {
        let length = reader.block_count().saturating_mul(BLOCK_SIZE_U64);
        Self {
            reader,
            length,
            position: 0,
            cached_lba: None,
            cache: [0; BLOCK_SIZE],
        }
    }

    /// The stream length in bytes.
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Borrows the underlying block reader.
    pub const fn get_ref(&self) -> &R {
        &self.reader
    }

    /// Mutably borrows the underlying block reader.
    pub const fn get_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    fn fill_cache(&mut self, lba: u64) -> Result<(), ErrorKind> {
        if self.cached_lba == Some(lba) {
            return Ok(());
        }
        let mut block = [0u8; BLOCK_SIZE];
        // The source's error is discarded on purpose: only a fixed kind
        // crosses into `hadris`, so no media-derived detail can be formatted
        // into a third-party diagnostic.
        self.reader
            .read_block(lba, &mut block)
            .map_err(|_| ErrorKind::InvalidData)?;
        self.cache = block;
        self.cached_lba = Some(lba);
        Ok(())
    }
}

impl<R: BlockReader> Read for BlockCursor<R> {
    type Error = ErrorKind;

    fn read(&mut self, buf: &mut [u8]) -> IsoResult<usize, Self::Error> {
        if self.position >= self.length || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.length - self.position;
        let wanted = (buf.len() as u64).min(remaining);
        let lba = self.position / BLOCK_SIZE_U64;
        let offset = usize::try_from(self.position % BLOCK_SIZE_U64)
            .map_err(|_| IsoError::from_kind(ErrorKind::InvalidInput))?;
        let available = BLOCK_SIZE_U64 - self.position % BLOCK_SIZE_U64;
        let count = usize::try_from(wanted.min(available))
            .map_err(|_| IsoError::from_kind(ErrorKind::InvalidInput))?;
        self.fill_cache(lba).map_err(IsoError::from_source)?;
        buf[..count].copy_from_slice(&self.cache[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: BlockReader> Seek for BlockCursor<R> {
    type Error = ErrorKind;

    fn seek(&mut self, pos: SeekFrom) -> IsoResult<u64, Self::Error> {
        let target = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.length.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| IsoError::from_kind(ErrorKind::InvalidInput))?;
        self.position = target;
        Ok(self.position)
    }
}
