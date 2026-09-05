//! Volume headers and the caller-supplied volume source.

use ohl_cabinet_format::{COMMON_HEADER_SIZE, CommonHeader, Layout};

use crate::error::{Error, VolumeError};

/// Encoded size of an InstallShield 5 volume header.
pub const VOLUME_HEADER_SIZE_V5: usize = 40;
/// Encoded size of an InstallShield 6 and later volume header.
pub const VOLUME_HEADER_SIZE_V6: usize = 64;

/// Sentinel written by InstallShield 5 when a volume records no trailing
/// file offset.
pub const NO_LAST_FILE_OFFSET: u64 = 0x7fff_ffff;

/// Byte offset of the volume header within a volume file.
pub const VOLUME_HEADER_AT: u64 = COMMON_HEADER_SIZE as u64;

/// Reads bytes out of numbered cabinet volumes.
///
/// This crate never opens a path, builds a filename, or learns a volume's
/// name: mapping a volume number to bytes is entirely the caller's job, so
/// the extractor holds no ambient authority.
pub trait VolumeSource {
    /// Reads into `buf` from `offset` within `volume`, returning the number
    /// of bytes read. A short read means the volume ended.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeError`] when the underlying source fails.
    fn read_at(&mut self, volume: u16, offset: u64, buf: &mut [u8]) -> Result<usize, VolumeError>;
}

impl<T: VolumeSource + ?Sized> VolumeSource for &mut T {
    fn read_at(&mut self, volume: u16, offset: u64, buf: &mut [u8]) -> Result<usize, VolumeError> {
        (**self).read_at(volume, offset, buf)
    }
}

/// The per-volume header describing which files start and end in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VolumeHeader {
    /// Offset of the volume's first data byte.
    pub data_offset: u64,
    /// Index of the first file with bytes in this volume.
    pub first_file_index: u32,
    /// Index of the last file with bytes in this volume.
    pub last_file_index: u32,
    /// Offset of the first file's bytes in this volume.
    pub first_file_offset: u64,
    /// Expanded size of the first file's portion in this volume.
    pub first_file_size_expanded: u64,
    /// Stored size of the first file's portion in this volume.
    pub first_file_size_compressed: u64,
    /// Offset of the last file's bytes in this volume.
    pub last_file_offset: u64,
    /// Expanded size of the last file's portion in this volume.
    pub last_file_size_expanded: u64,
    /// Stored size of the last file's portion in this volume.
    pub last_file_size_compressed: u64,
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64_pair(bytes: &[u8], low_at: usize) -> u64 {
    u64::from(le32(bytes, low_at)) | (u64::from(le32(bytes, low_at + 4)) << 32)
}

impl VolumeHeader {
    /// The encoded size for `layout`.
    #[must_use]
    pub const fn encoded_size(layout: Layout) -> usize {
        match layout {
            Layout::V5 => VOLUME_HEADER_SIZE_V5,
            Layout::V6 => VOLUME_HEADER_SIZE_V6,
        }
    }

    /// Parses a volume header, validating `bytes` against the layout's fixed
    /// length.
    pub fn parse(bytes: &[u8], layout: Layout) -> Result<Self, Error> {
        if bytes.len() < Self::encoded_size(layout) {
            return Err(Error::MalformedVolumeHeader);
        }

        Ok(match layout {
            Layout::V5 => {
                let last_file_offset = u64::from(le32(bytes, 0x1c));
                Self {
                    data_offset: u64::from(le32(bytes, 0x00)),
                    // 0x04 is unused in this layout.
                    first_file_index: le32(bytes, 0x08),
                    last_file_index: le32(bytes, 0x0c),
                    first_file_offset: u64::from(le32(bytes, 0x10)),
                    first_file_size_expanded: u64::from(le32(bytes, 0x14)),
                    first_file_size_compressed: u64::from(le32(bytes, 0x18)),
                    last_file_offset: if last_file_offset == 0 {
                        NO_LAST_FILE_OFFSET
                    } else {
                        last_file_offset
                    },
                    last_file_size_expanded: u64::from(le32(bytes, 0x20)),
                    last_file_size_compressed: u64::from(le32(bytes, 0x24)),
                }
            }
            Layout::V6 => Self {
                data_offset: le64_pair(bytes, 0x00),
                first_file_index: le32(bytes, 0x08),
                last_file_index: le32(bytes, 0x0c),
                first_file_offset: le64_pair(bytes, 0x10),
                first_file_size_expanded: le64_pair(bytes, 0x18),
                first_file_size_compressed: le64_pair(bytes, 0x20),
                last_file_offset: le64_pair(bytes, 0x28),
                last_file_size_expanded: le64_pair(bytes, 0x30),
                last_file_size_compressed: le64_pair(bytes, 0x38),
            },
        })
    }

    /// Encodes the header for `layout`, for synthetic test cabinets.
    #[must_use]
    pub fn encode(&self, layout: Layout) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec![0u8; Self::encoded_size(layout)];
        let put32 = |bytes: &mut [u8], at: usize, value: u64| {
            let low = u32::try_from(value & 0xffff_ffff).unwrap_or(u32::MAX);
            bytes[at..at + 4].copy_from_slice(&low.to_le_bytes());
        };
        let put64 = |bytes: &mut [u8], at: usize, value: u64| {
            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        };
        match layout {
            Layout::V5 => {
                put32(&mut bytes, 0x00, self.data_offset);
                bytes[0x08..0x0c].copy_from_slice(&self.first_file_index.to_le_bytes());
                bytes[0x0c..0x10].copy_from_slice(&self.last_file_index.to_le_bytes());
                put32(&mut bytes, 0x10, self.first_file_offset);
                put32(&mut bytes, 0x14, self.first_file_size_expanded);
                put32(&mut bytes, 0x18, self.first_file_size_compressed);
                put32(&mut bytes, 0x1c, self.last_file_offset);
                put32(&mut bytes, 0x20, self.last_file_size_expanded);
                put32(&mut bytes, 0x24, self.last_file_size_compressed);
            }
            Layout::V6 => {
                put64(&mut bytes, 0x00, self.data_offset);
                bytes[0x08..0x0c].copy_from_slice(&self.first_file_index.to_le_bytes());
                bytes[0x0c..0x10].copy_from_slice(&self.last_file_index.to_le_bytes());
                put64(&mut bytes, 0x10, self.first_file_offset);
                put64(&mut bytes, 0x18, self.first_file_size_expanded);
                put64(&mut bytes, 0x20, self.first_file_size_compressed);
                put64(&mut bytes, 0x28, self.last_file_offset);
                put64(&mut bytes, 0x30, self.last_file_size_expanded);
                put64(&mut bytes, 0x38, self.last_file_size_compressed);
            }
        }
        bytes
    }
}

/// Reads and validates one volume's common header and volume header.
pub(crate) fn read_volume_header<S: VolumeSource>(
    source: &mut S,
    volume: u16,
    layout: Layout,
) -> Result<VolumeHeader, Error> {
    let mut common = [0u8; COMMON_HEADER_SIZE];
    if source.read_at(volume, 0, &mut common)? != COMMON_HEADER_SIZE {
        return Err(Error::TruncatedVolume);
    }
    let _ = CommonHeader::parse(&common)?;

    let size = VolumeHeader::encoded_size(layout);
    let mut buffer = [0u8; VOLUME_HEADER_SIZE_V6];
    let target = &mut buffer[..size];
    if source.read_at(volume, VOLUME_HEADER_AT, target)? != size {
        return Err(Error::MalformedVolumeHeader);
    }
    VolumeHeader::parse(target, layout)
}

#[cfg(test)]
mod tests {
    use super::{Layout, VOLUME_HEADER_SIZE_V5, VOLUME_HEADER_SIZE_V6, VolumeHeader};
    use crate::error::Error;
    use alloc::vec;

    #[test]
    fn rejects_a_short_v5_header() {
        let short = vec![0u8; VOLUME_HEADER_SIZE_V5 - 1];
        assert_eq!(
            VolumeHeader::parse(&short, Layout::V5),
            Err(Error::MalformedVolumeHeader)
        );
    }

    #[test]
    fn rejects_a_v6_header_of_v5_length() {
        let short = vec![0u8; VOLUME_HEADER_SIZE_V5];
        assert_eq!(
            VolumeHeader::parse(&short, Layout::V6),
            Err(Error::MalformedVolumeHeader)
        );
    }

    #[test]
    fn v5_round_trips_through_encode() {
        let header = VolumeHeader {
            data_offset: 60,
            first_file_index: 1,
            last_file_index: 4,
            first_file_offset: 100,
            first_file_size_expanded: 10,
            first_file_size_compressed: 8,
            last_file_offset: 200,
            last_file_size_expanded: 20,
            last_file_size_compressed: 16,
        };
        let encoded = header.encode(Layout::V5);
        assert_eq!(encoded.len(), VOLUME_HEADER_SIZE_V5);
        assert_eq!(VolumeHeader::parse(&encoded, Layout::V5).unwrap(), header);
    }

    #[test]
    fn v6_round_trips_through_encode_with_high_words() {
        let header = VolumeHeader {
            data_offset: 0x1_0000_0060,
            first_file_index: 0,
            last_file_index: 0,
            first_file_offset: 0x2_0000_0000,
            first_file_size_expanded: 0x3_0000_0000,
            first_file_size_compressed: 0x4_0000_0000,
            last_file_offset: 0x5_0000_0000,
            last_file_size_expanded: 0x6_0000_0000,
            last_file_size_compressed: 0x7_0000_0000,
        };
        let encoded = header.encode(Layout::V6);
        assert_eq!(encoded.len(), VOLUME_HEADER_SIZE_V6);
        assert_eq!(VolumeHeader::parse(&encoded, Layout::V6).unwrap(), header);
    }

    #[test]
    fn a_zero_v5_last_file_offset_becomes_the_sentinel() {
        let zeroes = vec![0u8; VOLUME_HEADER_SIZE_V5];
        let parsed = VolumeHeader::parse(&zeroes, Layout::V5).unwrap();
        assert_eq!(parsed.last_file_offset, super::NO_LAST_FILE_OFFSET);
    }
}
