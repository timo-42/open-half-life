//! A tiny, panic-free little-endian cursor over a borrowed byte buffer, plus
//! a matching append-only writer.
//!
//! Every container field is fixed-width little-endian or a length-prefixed
//! blob, so the whole crate reads and writes through these two small helpers
//! instead of slicing or pushing by hand.

use crate::error::{Result, SaveError};

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
            .ok_or(SaveError::Truncated)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(SaveError::Truncated)?;
        self.position = end;
        Ok(slice)
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub(crate) fn array32(&mut self) -> Result<[u8; 32]> {
        let bytes = self.take(32)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    /// Reads a `u16`-length-prefixed UTF-8 string bounded to `max_len` bytes.
    pub(crate) fn bounded_string(&mut self, max_len: usize) -> Result<String> {
        let len = usize::from(self.u16()?);
        if len > max_len {
            return Err(SaveError::HeaderInvalid);
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SaveError::HeaderInvalid)
    }

    /// Reads a `u32`-length-prefixed opaque byte blob bounded to `max_len`.
    pub(crate) fn bounded_bytes(&mut self, max_len: usize) -> Result<Vec<u8>> {
        let len = usize::try_from(self.u32()?).map_err(|_| SaveError::HeaderInvalid)?;
        if len > max_len {
            return Err(SaveError::HeaderInvalid);
        }
        Ok(self.take(len)?.to_vec())
    }
}

#[derive(Debug, Default)]
pub(crate) struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn array32(&mut self, value: &[u8; 32]) {
        self.bytes.extend_from_slice(value);
    }

    /// Writes a `u16`-length-prefixed UTF-8 string. The caller must have
    /// already validated `value.len() <= u16::MAX as usize`.
    pub(crate) fn bounded_string(&mut self, value: &str) -> Result<()> {
        let len = u16::try_from(value.len()).map_err(|_| SaveError::HeaderInvalid)?;
        self.u16(len);
        self.raw(value.as_bytes());
        Ok(())
    }

    /// Writes a `u32`-length-prefixed opaque byte blob.
    pub(crate) fn bounded_bytes(&mut self, value: &[u8]) -> Result<()> {
        let len = u32::try_from(value.len()).map_err(|_| SaveError::HeaderInvalid)?;
        self.u32(len);
        self.raw(value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Reader, Writer};

    #[test]
    fn round_trips_every_fixed_width_field() {
        let mut writer = Writer::new();
        writer.u16(0x0102);
        writer.u32(0x0304_0506);
        writer.u64(0x0708_090a_0b0c_0d0e);
        writer.array32(&[7u8; 32]);
        let bytes = writer.into_bytes();

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u16().unwrap(), 0x0102);
        assert_eq!(reader.u32().unwrap(), 0x0304_0506);
        assert_eq!(reader.u64().unwrap(), 0x0708_090a_0b0c_0d0e);
        assert_eq!(reader.array32().unwrap(), [7u8; 32]);
        assert_eq!(reader.position(), bytes.len());
    }

    #[test]
    fn bounded_string_round_trips_and_rejects_over_length() {
        let mut writer = Writer::new();
        writer.bounded_string("hello").unwrap();
        let bytes = writer.into_bytes();
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.bounded_string(16).unwrap(), "hello");

        let mut reader = Reader::new(&bytes);
        assert!(reader.bounded_string(2).is_err());
    }

    #[test]
    fn truncated_input_never_panics_and_reports_truncated() {
        let bytes = [1u8, 2, 3];
        let mut reader = Reader::new(&bytes);
        assert!(reader.u64().is_err());
        assert!(reader.array32().is_err());
    }
}
