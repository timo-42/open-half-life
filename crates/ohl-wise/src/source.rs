//! The caller-supplied byte source, cancellation hook and output sink.
//!
//! This crate never opens a path, never learns a file name and never executes
//! anything: mapping an image to bytes is entirely the caller's job, so the
//! reader holds no ambient authority.

use alloc::vec::Vec;

use crate::error::Error;

/// A read-only, random-access source of image bytes.
pub trait ImageSource {
    /// Reads into `buf` from `offset`, returning the number of bytes read. A
    /// short read means the image ended.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SourceFailed`] when the underlying source fails.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Error>;

    /// The total length of the image in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SourceFailed`] when the underlying source fails.
    fn len(&mut self) -> Result<u64, Error>;

    /// Whether the image is empty.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SourceFailed`] when the underlying source fails.
    fn is_empty(&mut self) -> Result<bool, Error> {
        Ok(self.len()? == 0)
    }
}

impl<T: ImageSource + ?Sized> ImageSource for &mut T {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        (**self).read_at(offset, buf)
    }

    fn len(&mut self) -> Result<u64, Error> {
        (**self).len()
    }
}

/// An in-memory image, mostly useful for tests and fuzz targets.
#[derive(Clone, Copy)]
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl core::fmt::Debug for SliceSource<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SliceSource(<redacted>)")
    }
}

impl<'a> SliceSource<'a> {
    /// A source over `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl ImageSource for SliceSource<'_> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        let Ok(start) = usize::try_from(offset) else {
            return Ok(0);
        };
        let Some(tail) = self.bytes.get(start..) else {
            return Ok(0);
        };
        let count = tail.len().min(buf.len());
        buf[..count].copy_from_slice(&tail[..count]);
        Ok(count)
    }

    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
}

/// A cooperative cancellation hook, polled between chunks.
pub trait Cancellation {
    /// Whether the caller has asked the current walk to stop.
    fn is_cancelled(&self) -> bool;
}

/// A hook that never cancels.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancelled;

impl Cancellation for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl<T: Cancellation + ?Sized> Cancellation for &T {
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}

/// Receives inflated bytes in bounded chunks.
pub trait Sink {
    /// Accepts one bounded chunk.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] chosen by the implementation when the chunk
    /// cannot be accepted.
    fn write(&mut self, chunk: &[u8]) -> Result<(), Error>;
}

/// A sink that counts bytes and discards them, used when walking the chain.
#[derive(Debug, Clone, Copy, Default)]
pub struct Discard;

impl Sink for Discard {
    fn write(&mut self, _chunk: &[u8]) -> Result<(), Error> {
        Ok(())
    }
}

impl Sink for Vec<u8> {
    fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.extend_from_slice(chunk);
        Ok(())
    }
}

impl<T: Sink + ?Sized> Sink for &mut T {
    fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        (**self).write(chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cancellation, Discard, ImageSource, NeverCancelled, Sink, SliceSource};
    use alloc::vec::Vec;

    #[test]
    fn slice_source_reads_and_reports_short_reads() {
        let mut source = SliceSource::new(&[1, 2, 3, 4]);
        let mut buf = [0u8; 3];
        assert_eq!(source.read_at(2, &mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], &[3, 4]);
        assert_eq!(source.read_at(9, &mut buf).unwrap(), 0);
        assert_eq!(source.len().unwrap(), 4);
        assert!(!source.is_empty().unwrap());
    }

    #[test]
    fn debug_never_reveals_bytes() {
        let text = alloc::format!("{:?}", SliceSource::new(b"secret"));
        assert_eq!(text, "SliceSource(<redacted>)");
    }

    #[test]
    fn sinks_and_cancellation_behave() {
        assert!(!NeverCancelled.is_cancelled());
        let mut discard = Discard;
        discard.write(&[1, 2, 3]).unwrap();
        let mut vector: Vec<u8> = Vec::new();
        vector.write(&[1, 2]).unwrap();
        assert_eq!(vector, alloc::vec![1, 2]);
    }
}
