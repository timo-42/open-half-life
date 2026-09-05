//! A tiny, panic-free little-endian cursor over a borrowed byte buffer.
//!
//! Every cabinet field is an unaligned little-endian `u1`/`u2`/`u4`
//! ([MS-CAB] "Data type conventions"), so the whole crate reads through this
//! one bounds-checked reader instead of slicing by hand.

use crate::error::{CabError, Result};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub(crate) const fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(CabError::OutOfBounds)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(CabError::Truncated)?;
        self.position = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Reads a NUL-terminated byte string of at most `max_len` content bytes,
    /// consuming the terminator. Returns the content without the terminator.
    pub(crate) fn nul_terminated(&mut self, max_len: usize) -> Result<&'a [u8]> {
        let start = self.position;
        let available = self.bytes.len().saturating_sub(start);
        let scan_len = max_len.saturating_add(1).min(available);
        let window = self
            .bytes
            .get(start..start + scan_len)
            .ok_or(CabError::Truncated)?;
        let terminator = window
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(CabError::LimitExceeded)?;
        self.position = start + terminator + 1;
        Ok(&window[..terminator])
    }
}

#[cfg(test)]
mod tests {
    use super::Reader;
    use crate::error::CabError;

    #[test]
    fn reads_little_endian_fields_in_order() {
        let bytes = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u8().unwrap(), 0x01);
        assert_eq!(reader.u16().unwrap(), 0x0302);
        assert_eq!(reader.u32().unwrap(), 0x0706_0504);
        assert_eq!(reader.u8().unwrap_err(), CabError::Truncated);
    }

    #[test]
    fn nul_terminated_stops_at_the_terminator_and_bounds_the_scan() {
        let bytes = *b"ab\0cd";
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.nul_terminated(8).unwrap(), b"ab");
        assert_eq!(reader.position(), 3);

        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.nul_terminated(1).unwrap_err(),
            CabError::LimitExceeded
        );

        let unterminated = *b"abc";
        let mut reader = Reader::new(&unterminated);
        assert_eq!(
            reader.nul_terminated(8).unwrap_err(),
            CabError::LimitExceeded
        );
    }
}
