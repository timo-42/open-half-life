//! Bounded, allocation-free payload readers and writers.
//!
//! Both types are *sticky*: the first failure is recorded and every later
//! operation fails with that same error without touching the buffer, so a
//! caller may write or read a whole schema and check once at the end without
//! risking a partially applied message.

use core::fmt;

use crate::{MAXIMUM_FRAME_PAYLOAD_BYTES, ProtocolError, ProtocolPhase, ProtocolStatus};

/// Writes canonical little-endian payload fields into a caller buffer.
pub struct PayloadWriter<'destination> {
    destination: &'destination mut [u8],
    capacity: usize,
    position: usize,
    error: Option<ProtocolError>,
}

impl fmt::Debug for PayloadWriter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print buffered bytes.
        f.debug_struct("PayloadWriter")
            .field("capacity", &self.capacity)
            .field("position", &self.position)
            .field("error", &self.error)
            .finish()
    }
}

impl<'destination> PayloadWriter<'destination> {
    /// Wraps `destination`, capping usable capacity at the frame ceiling.
    #[must_use]
    pub fn new(destination: &'destination mut [u8]) -> Self {
        let capacity = destination.len().min(MAXIMUM_FRAME_PAYLOAD_BYTES as usize);
        Self {
            destination,
            capacity,
            position: 0,
            error: None,
        }
    }

    fn reserve(&mut self, size: usize) -> Result<usize, ProtocolError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if size > self.capacity - self.position {
            self.error = Some(ProtocolError::OutputTooSmall);
            return Err(ProtocolError::OutputTooSmall);
        }
        let start = self.position;
        self.position += size;
        Ok(start)
    }

    /// Writes one byte.
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_u8(&mut self, value: u8) -> Result<(), ProtocolError> {
        let at = self.reserve(1)?;
        self.destination[at] = value;
        Ok(())
    }

    /// Writes a little-endian `u16`.
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_u16(&mut self, value: u16) -> Result<(), ProtocolError> {
        let at = self.reserve(2)?;
        self.destination[at..at + 2].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Writes a little-endian `u32`.
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_u32(&mut self, value: u32) -> Result<(), ProtocolError> {
        let at = self.reserve(4)?;
        self.destination[at..at + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Writes a little-endian `u64`.
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_u64(&mut self, value: u64) -> Result<(), ProtocolError> {
        let at = self.reserve(8)?;
        self.destination[at..at + 8].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    /// Writes a canonical boolean (`0` or `1`).
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_bool(&mut self, value: bool) -> Result<(), ProtocolError> {
        self.write_u8(u8::from(value))
    }

    /// Writes a [`ProtocolStatus`].
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_status(&mut self, value: ProtocolStatus) -> Result<(), ProtocolError> {
        self.write_u16(value.to_wire())
    }

    /// Writes a [`ProtocolPhase`].
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_phase(&mut self, value: ProtocolPhase) -> Result<(), ProtocolError> {
        self.write_u16(value.to_wire())
    }

    /// Writes raw bytes.
    ///
    /// # Errors
    /// [`ProtocolError::OutputTooSmall`], or the sticky earlier error.
    pub fn write_bytes(&mut self, value: &[u8]) -> Result<(), ProtocolError> {
        let at = self.reserve(value.len())?;
        self.destination[at..at + value.len()].copy_from_slice(value);
        Ok(())
    }

    /// The first recorded failure, if any.
    #[must_use]
    pub const fn error(&self) -> Option<ProtocolError> {
        self.error
    }

    /// The number of bytes written so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.position
    }

    /// Whether nothing has been written yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.position == 0
    }

    /// The bytes written so far.
    #[must_use]
    pub fn written(&self) -> &[u8] {
        &self.destination[..self.position]
    }
}

/// Reads canonical little-endian payload fields out of a borrowed payload.
pub struct PayloadReader<'payload> {
    payload: &'payload [u8],
    position: usize,
    error: Option<ProtocolError>,
}

impl fmt::Debug for PayloadReader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print payload bytes.
        f.debug_struct("PayloadReader")
            .field("len", &self.payload.len())
            .field("position", &self.position)
            .field("error", &self.error)
            .finish()
    }
}

impl<'payload> PayloadReader<'payload> {
    /// Wraps `payload`, rejecting anything above the frame ceiling up front.
    #[must_use]
    pub const fn new(payload: &'payload [u8]) -> Self {
        let error = if payload.len() > MAXIMUM_FRAME_PAYLOAD_BYTES as usize {
            Some(ProtocolError::PayloadTooLarge)
        } else {
            None
        };
        Self {
            payload,
            position: 0,
            error,
        }
    }

    fn take(&mut self, size: usize) -> Result<&'payload [u8], ProtocolError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if size > self.payload.len() - self.position {
            self.error = Some(ProtocolError::PayloadUnderflow);
            return Err(ProtocolError::PayloadUnderflow);
        }
        let start = self.position;
        self.position += size;
        Ok(&self.payload[start..start + size])
    }

    /// Reads one byte.
    ///
    /// # Errors
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little-endian `u16`.
    ///
    /// # Errors
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_u16(&mut self) -> Result<u16, ProtocolError> {
        let bytes: [u8; 2] = self.take(2)?.try_into().unwrap_or([0; 2]);
        Ok(u16::from_le_bytes(bytes))
    }

    /// Reads a little-endian `u32`.
    ///
    /// # Errors
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_u32(&mut self) -> Result<u32, ProtocolError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().unwrap_or([0; 4]);
        Ok(u32::from_le_bytes(bytes))
    }

    /// Reads a little-endian `u64`.
    ///
    /// # Errors
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_u64(&mut self) -> Result<u64, ProtocolError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().unwrap_or([0; 8]);
        Ok(u64::from_le_bytes(bytes))
    }

    /// Reads a canonical boolean; any byte above `1` is rejected.
    ///
    /// # Errors
    /// [`ProtocolError::NoncanonicalValue`],
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_bool(&mut self) -> Result<bool, ProtocolError> {
        let encoded = self.read_u8()?;
        if encoded > 1 {
            self.error = Some(ProtocolError::NoncanonicalValue);
            return Err(ProtocolError::NoncanonicalValue);
        }
        Ok(encoded != 0)
    }

    /// Reads a [`ProtocolStatus`]; unknown values are rejected.
    ///
    /// # Errors
    /// [`ProtocolError::NoncanonicalValue`],
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_status(&mut self) -> Result<ProtocolStatus, ProtocolError> {
        let encoded = self.read_u16()?;
        ProtocolStatus::from_wire(encoded).inspect_err(|error| {
            self.error = Some(*error);
        })
    }

    /// Reads a [`ProtocolPhase`]; unknown values are rejected.
    ///
    /// # Errors
    /// [`ProtocolError::NoncanonicalValue`],
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_phase(&mut self) -> Result<ProtocolPhase, ProtocolError> {
        let encoded = self.read_u16()?;
        ProtocolPhase::from_wire(encoded).inspect_err(|error| {
            self.error = Some(*error);
        })
    }

    /// Borrows the next `size` payload bytes.
    ///
    /// # Errors
    /// [`ProtocolError::PayloadUnderflow`], or the sticky earlier error.
    pub fn read_bytes(&mut self, size: usize) -> Result<&'payload [u8], ProtocolError> {
        self.take(size)
    }

    /// Requires that the payload was consumed exactly.
    ///
    /// # Errors
    /// [`ProtocolError::PayloadTrailingBytes`], or the sticky earlier error.
    pub fn finish(&mut self) -> Result<(), ProtocolError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.position != self.payload.len() {
            self.error = Some(ProtocolError::PayloadTrailingBytes);
            return Err(ProtocolError::PayloadTrailingBytes);
        }
        Ok(())
    }

    /// The first recorded failure, if any.
    #[must_use]
    pub const fn error(&self) -> Option<ProtocolError> {
        self.error
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.payload.len() - self.position
    }
}

#[cfg(test)]
mod tests {
    use super::{PayloadReader, PayloadWriter};
    use crate::ProtocolError;

    #[test]
    fn debug_never_prints_buffered_bytes() {
        let mut storage = [0_u8; 4];
        let mut writer = PayloadWriter::new(&mut storage);
        writer.write_u16(0xbeef).expect("fits");
        let rendered = format!("{writer:?}");
        assert!(rendered.contains("position: 2"));
        assert!(!rendered.contains("239"));

        let reader = PayloadReader::new(&[0xaa, 0xbb]);
        let rendered = format!("{reader:?}");
        assert!(rendered.contains("len: 2"));
        assert!(!rendered.contains("170"));
    }

    #[test]
    fn reader_reports_remaining_bytes() {
        let mut reader = PayloadReader::new(&[1, 2, 3]);
        assert_eq!(reader.remaining(), 3);
        assert_eq!(reader.read_u8(), Ok(1));
        assert_eq!(reader.remaining(), 2);
        assert_eq!(reader.finish(), Err(ProtocolError::PayloadTrailingBytes));
    }
}
