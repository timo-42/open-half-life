//! Locating the first compressed stream after the overlay header.
//!
//! # What is documented, and what is not
//!
//! The public sources recorded in `docs/FORMAT_SOURCES.md` describe the
//! overlay as "a Wise specific header" followed by raw DEFLATE data; **no
//! public source documents the header's field layout**, only that it exists
//! and that the compressed data begins after it. This module therefore
//! decodes no header field at all. It locates the first stream by a bounded
//! scan (at most `Limits::max_header_scan_bytes`, 4 KiB by default) and
//! accepts a candidate offset only when the bytes there inflate cleanly as
//! raw DEFLATE, produce at least `Limits::min_confirmed_inflated_bytes`
//! bytes, and are followed by a `u32` equal to the CRC-32 of those bytes.
//! Guessing is thereby replaced by proof.
//!
//! The header length is reported as an observation (the distance from the
//! overlay start to the confirmed stream), never interpreted.
//!
//! The documented `zip`-enabled variant, in which each entry is preceded by a
//! ZIP local file header and carries no trailing CRC-32, is detected by its
//! `PK\x03\x04` signature and rejected with
//! [`Error::ZipVariantUnsupported`] rather than misparsed.

use crate::error::{Error, Limit};
use crate::limits::Limits;
use crate::overlay::Overlay;
use crate::source::{Cancellation, Discard, ImageSource};
use crate::stream::{ChecksumStatus, StreamMetrics, inflate_stream};
use alloc::vec;

/// The ZIP local file header signature, `PK\x03\x04`.
pub const ZIP_LOCAL_FILE_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// Where the first confirmed stream lives, and how much preceded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageHeader {
    /// Offset of the first overlay byte.
    pub overlay_offset: u64,
    /// Observed number of bytes between the overlay start and the first
    /// confirmed compressed stream. Not interpreted.
    pub header_len: usize,
    /// Offset of the first confirmed compressed stream.
    pub first_stream_offset: u64,
    /// Measurements of that first stream (the DIB, which callers skip).
    pub first_stream: StreamMetrics,
}

/// Whether `byte` can begin a raw DEFLATE block: block type 3 is reserved.
const fn plausible_first_byte(byte: u8) -> bool {
    (byte >> 1) & 0b11 != 0b11
}

/// Finds the first CRC-confirmed DEFLATE stream in the overlay.
pub fn locate_first_stream<S: ImageSource, C: Cancellation>(
    source: &mut S,
    overlay: &Overlay,
    limits: &Limits,
    cancel: &C,
) -> Result<PackageHeader, Error> {
    if limits.max_header_scan_bytes == 0 {
        return Err(Error::LimitExceeded(Limit::HeaderScanBytes));
    }
    let window_len = usize::try_from(overlay.len)
        .unwrap_or(usize::MAX)
        .min(limits.max_header_scan_bytes);
    let mut window = vec![0u8; window_len];
    let read = source.read_at(overlay.offset, &mut window)?;
    window.truncate(read);
    if window.is_empty() {
        return Err(Error::Truncated);
    }
    if window
        .windows(ZIP_LOCAL_FILE_SIGNATURE.len())
        .any(|candidate| candidate == ZIP_LOCAL_FILE_SIGNATURE)
    {
        return Err(Error::ZipVariantUnsupported);
    }

    // Candidate streams are capped well below the per-stream ceiling: a
    // misaligned offset must never be able to inflate for gigabytes before it
    // is rejected.
    let scan_limits = Limits {
        max_inflated_bytes_per_stream: limits
            .max_inflated_bytes_per_stream
            .min(SCAN_STREAM_CEILING),
        max_compressed_bytes_per_stream: limits
            .max_compressed_bytes_per_stream
            .min(SCAN_STREAM_CEILING),
        ..*limits
    };

    for (skip, byte) in window.iter().copied().enumerate() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !plausible_first_byte(byte) {
            continue;
        }
        let offset = overlay.offset + skip as u64;
        let Ok(metrics) = inflate_stream(source, offset, scan_limits, &mut Discard, cancel) else {
            continue;
        };
        if metrics.checksum != ChecksumStatus::Match {
            continue;
        }
        if metrics.inflated_len < limits.min_confirmed_inflated_bytes as u64 {
            continue;
        }
        return Ok(PackageHeader {
            overlay_offset: overlay.offset,
            header_len: skip,
            first_stream_offset: offset,
            first_stream: metrics,
        });
    }

    Err(Error::HeaderNotFound)
}

/// Ceiling on one candidate stream while scanning for the header end.
const SCAN_STREAM_CEILING: u64 = 64 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::{ZIP_LOCAL_FILE_SIGNATURE, locate_first_stream, plausible_first_byte};
    use crate::crc32::crc32;
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use crate::overlay::Overlay;
    use crate::source::{NeverCancelled, SliceSource};
    use alloc::vec::Vec;

    fn overlay_bytes(header_len: usize, payload: &[u8]) -> Vec<u8> {
        let mut bytes = alloc::vec![0xffu8; header_len];
        let mut stream = miniz_oxide::deflate::compress_to_vec(payload, 6);
        stream.extend_from_slice(&crc32(payload).to_le_bytes());
        bytes.extend_from_slice(&stream);
        bytes
    }

    fn overlay(len: u64) -> Overlay {
        Overlay {
            offset: 0,
            len,
            image_len: len,
        }
    }

    #[test]
    fn finds_the_stream_after_an_undocumented_header() {
        let payload = alloc::vec![0x21u8; 512];
        let image = overlay_bytes(157, &payload);
        let mut source = SliceSource::new(&image);
        let header = locate_first_stream(
            &mut source,
            &overlay(image.len() as u64),
            &Limits::DEFAULT,
            &NeverCancelled,
        )
        .unwrap();
        assert_eq!(header.header_len, 157);
        assert_eq!(header.first_stream_offset, 157);
        assert_eq!(header.first_stream.inflated_len, 512);
    }

    #[test]
    fn rejects_the_zip_variant() {
        let mut image = alloc::vec![0u8; 32];
        image[8..12].copy_from_slice(&ZIP_LOCAL_FILE_SIGNATURE);
        let mut source = SliceSource::new(&image);
        assert_eq!(
            locate_first_stream(
                &mut source,
                &overlay(image.len() as u64),
                &Limits::DEFAULT,
                &NeverCancelled
            ),
            Err(Error::ZipVariantUnsupported)
        );
    }

    #[test]
    fn fails_closed_when_nothing_is_confirmed() {
        let image = alloc::vec![0xffu8; 600];
        let mut source = SliceSource::new(&image);
        assert_eq!(
            locate_first_stream(
                &mut source,
                &overlay(image.len() as u64),
                &Limits::DEFAULT,
                &NeverCancelled
            ),
            Err(Error::HeaderNotFound)
        );
    }

    #[test]
    fn respects_the_scan_ceiling() {
        let payload = alloc::vec![0x21u8; 512];
        let image = overlay_bytes(300, &payload);
        let mut source = SliceSource::new(&image);
        let limits = Limits {
            max_header_scan_bytes: 64,
            ..Limits::DEFAULT
        };
        assert_eq!(
            locate_first_stream(
                &mut source,
                &overlay(image.len() as u64),
                &limits,
                &NeverCancelled
            ),
            Err(Error::HeaderNotFound)
        );
        let zero = Limits {
            max_header_scan_bytes: 0,
            ..Limits::DEFAULT
        };
        assert_eq!(
            locate_first_stream(
                &mut source,
                &overlay(image.len() as u64),
                &zero,
                &NeverCancelled
            ),
            Err(Error::LimitExceeded(Limit::HeaderScanBytes))
        );
    }

    #[test]
    fn skips_reserved_block_types_quickly() {
        assert!(!plausible_first_byte(0b0000_0110));
        assert!(plausible_first_byte(0b0000_0011));
    }
}
