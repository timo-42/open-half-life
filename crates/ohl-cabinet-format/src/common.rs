//! The 20-byte common header shared by header and volume files.

use crate::bytes::Cursor;
use crate::error::FormatError;

/// The InstallShield cabinet signature, little-endian `ISc(`.
pub const CAB_SIGNATURE: u32 = 0x2863_5349;

/// The Microsoft Cabinet signature, recognised only to report a better error.
pub const MSCF_SIGNATURE: u32 = 0x4643_534d;

/// Encoded size of the common header.
pub const COMMON_HEADER_SIZE: usize = 20;

/// The fixed prologue of every cabinet header and volume file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommonHeader {
    /// Always [`CAB_SIGNATURE`] once parsing succeeds.
    pub signature: u32,
    /// The raw version word; decode it with `Version::decode`.
    pub version: u32,
    /// Opaque volume information word.
    pub volume_info: u32,
    /// Byte offset of the cabinet descriptor within the header buffer.
    pub cab_descriptor_offset: u32,
    /// Byte length of the cabinet descriptor.
    pub cab_descriptor_size: u32,
}

impl CommonHeader {
    /// Parses a common header from the first [`COMMON_HEADER_SIZE`] bytes of
    /// `data`.
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut cursor = Cursor::new(data);
        let signature = cursor.u32()?;
        if signature != CAB_SIGNATURE {
            return Err(if signature == MSCF_SIGNATURE {
                FormatError::MicrosoftCabinet
            } else {
                FormatError::BadSignature
            });
        }

        Ok(Self {
            signature,
            version: cursor.u32()?,
            volume_info: cursor.u32()?,
            cab_descriptor_offset: cursor.u32()?,
            cab_descriptor_size: cursor.u32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CAB_SIGNATURE, CommonHeader, MSCF_SIGNATURE};
    use crate::error::FormatError;
    use alloc::vec::Vec;

    fn header_bytes(signature: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&signature.to_le_bytes());
        bytes.extend_from_slice(&0x0100_5000u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(&630u32.to_le_bytes());
        bytes
    }

    #[test]
    fn parses_a_valid_common_header() {
        let parsed = CommonHeader::parse(&header_bytes(CAB_SIGNATURE)).unwrap();
        assert_eq!(parsed.cab_descriptor_offset, 20);
        assert_eq!(parsed.cab_descriptor_size, 630);
    }

    #[test]
    fn reports_microsoft_cabinets_distinctly() {
        assert_eq!(
            CommonHeader::parse(&header_bytes(MSCF_SIGNATURE)),
            Err(FormatError::MicrosoftCabinet)
        );
        assert_eq!(
            CommonHeader::parse(&header_bytes(0)),
            Err(FormatError::BadSignature)
        );
    }

    #[test]
    fn rejects_a_short_buffer() {
        let bytes = header_bytes(CAB_SIGNATURE);
        assert_eq!(
            CommonHeader::parse(&bytes[..12]),
            Err(FormatError::Truncated)
        );
    }
}
