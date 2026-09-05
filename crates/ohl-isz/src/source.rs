//! The caller-supplied byte source, the cancellation token, and the bounded
//! signature scan that locates an archive embedded in a larger file.

use alloc::vec;

use crate::error::{Error, Limit, Result, SourceError};
use crate::header::SIGNATURE;
use crate::limits::Limits;

/// Reads bytes out of whatever container holds the archive.
///
/// This crate never opens a path, builds a filename, or learns where the
/// bytes came from: mapping an offset to bytes is entirely the caller's job,
/// so the decoder holds no ambient authority. An InstallShield 3 archive is
/// commonly embedded in the overlay of an installer executable, so every
/// offset here is absolute within the caller's container and the archive's
/// own base offset is added by [`crate::Archive`].
pub trait ArchiveSource {
    /// Reads into `buf` starting at `offset`, returning the number of bytes
    /// read. A short read means the container ended.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when the underlying container fails.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> core::result::Result<usize, SourceError>;
}

impl<T: ArchiveSource + ?Sized> ArchiveSource for &mut T {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> core::result::Result<usize, SourceError> {
        (**self).read_at(offset, buf)
    }
}

/// An in-memory source over a byte slice, used by tests and by callers that
/// already hold the whole container.
#[derive(Debug, Clone, Copy)]
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceSource<'a> {
    /// A source over `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl ArchiveSource for SliceSource<'_> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> core::result::Result<usize, SourceError> {
        let Ok(start) = usize::try_from(offset) else {
            return Ok(0);
        };
        let Some(available) = self.bytes.get(start..) else {
            return Ok(0);
        };
        let taken = available.len().min(buf.len());
        buf[..taken].copy_from_slice(&available[..taken]);
        Ok(taken)
    }
}

/// A cooperative cancellation token polled between bounded work units.
///
/// The decoder checks it between scan windows, between table-of-contents
/// records and between decode steps, so a hostile archive cannot make a
/// single call run unboundedly long without the caller getting a chance to
/// stop it.
pub trait Cancellation {
    /// Returns `true` once the caller wants the operation to stop.
    fn is_cancelled(&self) -> bool;
}

/// A token that is never signalled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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

pub(crate) fn check_cancelled<C: Cancellation + ?Sized>(cancel: &C) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Reads exactly `buf.len()` bytes at `offset`, rejecting a short read.
pub(crate) fn read_exact_at<S: ArchiveSource + ?Sized>(
    source: &mut S,
    offset: u64,
    buf: &mut [u8],
) -> Result<()> {
    let mut done = 0usize;
    while done < buf.len() {
        let at = offset.checked_add(done as u64).ok_or(Error::OutOfRange)?;
        let read = source.read_at(at, &mut buf[done..])?;
        if read == 0 {
            return Err(Error::Truncated);
        }
        done = done.checked_add(read).ok_or(Error::OutOfRange)?;
        if done > buf.len() {
            return Err(Error::Truncated);
        }
    }
    Ok(())
}

/// Scans `source` from offset zero for the archive signature, reading at most
/// `limits.max_scan_bytes` bytes in bounded windows.
///
/// Returns the absolute offset of the first match, or `None` when the scan
/// budget is exhausted or the container ends without a match.
///
/// # Errors
///
/// Returns [`Error::InvalidInput`] for invalid limits, [`Error::Source`] when
/// the container fails, and [`Error::Cancelled`] when `cancel` is signalled.
pub fn find_signature<S: ArchiveSource + ?Sized, C: Cancellation + ?Sized>(
    source: &mut S,
    limits: &Limits,
    cancel: &C,
) -> Result<Option<u64>> {
    find_signature_from(source, 0, limits, cancel)
}

/// Scans `source` from `start` for the archive signature.
///
/// # Errors
///
/// As [`find_signature`].
pub fn find_signature_from<S: ArchiveSource + ?Sized, C: Cancellation + ?Sized>(
    source: &mut S,
    start: u64,
    limits: &Limits,
    cancel: &C,
) -> Result<Option<u64>> {
    limits.validate()?;
    let overlap = SIGNATURE.len() - 1;
    let window = limits.max_chunk_bytes.max(SIGNATURE.len());
    let mut buffer = vec![0u8; window];

    let mut position = start;
    let mut scanned = 0u64;
    loop {
        check_cancelled(cancel)?;
        if scanned >= limits.max_scan_bytes {
            return Ok(None);
        }
        let budget = limits.max_scan_bytes - scanned;
        let want = usize::try_from(budget).unwrap_or(usize::MAX).min(window);
        if want < SIGNATURE.len() {
            return Ok(None);
        }

        let mut filled = 0usize;
        while filled < want {
            let at = position
                .checked_add(filled as u64)
                .ok_or(Error::OutOfRange)?;
            let read = source.read_at(at, &mut buffer[filled..want])?;
            if read == 0 {
                break;
            }
            filled = filled
                .checked_add(read)
                .filter(|done| *done <= want)
                .ok_or(Error::Truncated)?;
        }
        if filled < SIGNATURE.len() {
            return Ok(None);
        }

        if let Some(index) = buffer[..filled]
            .windows(SIGNATURE.len())
            .position(|candidate| candidate == SIGNATURE)
        {
            let hit = position
                .checked_add(index as u64)
                .ok_or(Error::OutOfRange)?;
            return Ok(Some(hit));
        }

        scanned = scanned
            .checked_add(filled as u64)
            .ok_or(Error::LimitExceeded(Limit::ScanBytes))?;
        if filled < want {
            return Ok(None);
        }
        let step = (filled - overlap) as u64;
        position = position.checked_add(step).ok_or(Error::OutOfRange)?;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArchiveSource, NeverCancelled, SliceSource, find_signature, find_signature_from,
        read_exact_at,
    };
    use crate::error::Error;
    use crate::header::SIGNATURE;
    use crate::limits::Limits;
    use alloc::vec;
    use alloc::vec::Vec;

    fn tiny_limits(chunk: usize) -> Limits {
        Limits {
            max_chunk_bytes: chunk,
            ..Limits::default()
        }
    }

    #[test]
    fn finds_a_signature_at_offset_zero() {
        let mut bytes = Vec::from(SIGNATURE);
        bytes.extend_from_slice(&[0u8; 64]);
        let mut source = SliceSource::new(&bytes);
        let found = find_signature(&mut source, &Limits::default(), &NeverCancelled).unwrap();
        assert_eq!(found, Some(0));
    }

    #[test]
    fn finds_a_signature_straddling_two_windows() {
        // The signature starts three bytes before the end of the first
        // 16-byte window, so only the overlap can find it.
        let mut bytes = vec![0u8; 13];
        bytes.extend_from_slice(SIGNATURE);
        bytes.extend_from_slice(&[0u8; 40]);
        let mut source = SliceSource::new(&bytes);
        let found = find_signature(&mut source, &tiny_limits(16), &NeverCancelled).unwrap();
        assert_eq!(found, Some(13));
    }

    #[test]
    fn reports_no_match_without_reading_past_the_scan_budget() {
        let mut bytes = vec![0u8; 4096];
        bytes.extend_from_slice(SIGNATURE);
        let mut source = SliceSource::new(&bytes);
        let limits = Limits {
            max_scan_bytes: 1024,
            max_chunk_bytes: 256,
            ..Limits::default()
        };
        assert_eq!(
            find_signature(&mut source, &limits, &NeverCancelled).unwrap(),
            None
        );
    }

    #[test]
    fn scanning_from_an_offset_finds_a_later_match() {
        let mut bytes = vec![0u8; 1000];
        bytes.extend_from_slice(SIGNATURE);
        let mut source = SliceSource::new(&bytes);
        let found =
            find_signature_from(&mut source, 500, &tiny_limits(64), &NeverCancelled).unwrap();
        assert_eq!(found, Some(1000));
    }

    #[test]
    fn invalid_limits_are_rejected() {
        let mut source = SliceSource::new(&[]);
        let limits = Limits {
            max_chunk_bytes: 0,
            ..Limits::default()
        };
        assert_eq!(
            find_signature(&mut source, &limits, &NeverCancelled),
            Err(Error::InvalidInput)
        );
    }

    #[test]
    fn a_short_read_is_truncation() {
        let mut source = SliceSource::new(&[1, 2, 3]);
        let mut buffer = [0u8; 8];
        assert_eq!(
            read_exact_at(&mut source, 0, &mut buffer),
            Err(Error::Truncated)
        );
    }

    #[test]
    fn a_slice_source_past_its_end_reads_nothing() {
        let mut source = SliceSource::new(&[1, 2, 3]);
        let mut buffer = [0u8; 4];
        assert_eq!(source.read_at(99, &mut buffer), Ok(0));
    }
}
