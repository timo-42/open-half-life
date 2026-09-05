//! Bounds-checked little-endian reads.
//!
//! Unshield reads header fields through a raw `uint8_t*` walked with `p +=`,
//! and its `unshield_header_get_buffer` adds an untrusted 32-bit offset to a
//! base pointer without any check at all. Every read in this crate goes
//! through [`Cursor`], which can only fail, never over-read.

use crate::error::FormatError;

/// A bounds-checked forward reader over a byte slice.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// A cursor positioned at the start of `data`.
    pub(crate) const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// A cursor positioned at `offset`, which must lie inside `data`.
    pub(crate) fn at(data: &'a [u8], offset: usize) -> Result<Self, FormatError> {
        if offset > data.len() {
            return Err(FormatError::OffsetOutOfRange);
        }
        Ok(Self { data, pos: offset })
    }

    /// The current absolute position within the underlying slice.
    pub(crate) const fn position(&self) -> usize {
        self.pos
    }

    /// Skips `count` bytes, failing if that would leave the slice.
    pub(crate) fn skip(&mut self, count: usize) -> Result<(), FormatError> {
        self.take(count).map(|_| ())
    }

    /// Borrows the next `count` bytes.
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(count).ok_or(FormatError::Truncated)?;
        let slice = self.data.get(self.pos..end).ok_or(FormatError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    /// Reads one byte.
    pub(crate) fn u8(&mut self) -> Result<u8, FormatError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little-endian `u16`.
    pub(crate) fn u16(&mut self) -> Result<u16, FormatError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Reads a little-endian `u32`.
    pub(crate) fn u32(&mut self) -> Result<u32, FormatError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads a little-endian `u64`.
    pub(crate) fn u64(&mut self) -> Result<u64, FormatError> {
        let bytes = self.take(8)?;
        let mut value = [0u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(value))
    }

    /// Reads a fixed 16-byte digest field.
    pub(crate) fn md5(&mut self) -> Result<[u8; 16], FormatError> {
        let bytes = self.take(16)?;
        let mut value = [0u8; 16];
        value.copy_from_slice(bytes);
        Ok(value)
    }
}

/// Adds `offset` to `base`, failing rather than wrapping.
pub(crate) fn add(base: usize, offset: usize) -> Result<usize, FormatError> {
    base.checked_add(offset)
        .ok_or(FormatError::OffsetOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::{Cursor, add};
    use crate::error::FormatError;

    #[test]
    fn reads_little_endian_fields() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut cursor = Cursor::new(&data);
        assert_eq!(cursor.u16().unwrap(), 0x0201);
        assert_eq!(cursor.u32().unwrap(), 0x0605_0403);
        assert_eq!(cursor.position(), 6);
    }

    #[test]
    fn refuses_to_read_past_the_end() {
        let data = [1u8, 2, 3];
        let mut cursor = Cursor::new(&data);
        assert_eq!(cursor.u32(), Err(FormatError::Truncated));
    }

    #[test]
    fn refuses_an_out_of_range_start() {
        let data = [1u8, 2, 3];
        assert_eq!(
            Cursor::at(&data, 4).map(|_| ()),
            Err(FormatError::OffsetOutOfRange)
        );
    }

    #[test]
    fn addition_does_not_wrap() {
        assert_eq!(add(usize::MAX, 1), Err(FormatError::OffsetOutOfRange));
    }
}
