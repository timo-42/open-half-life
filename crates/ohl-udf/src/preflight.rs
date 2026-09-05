//! Bounded ECMA-167 NSR02 structural preflight.
//!
//! This module is project-owned and written only from the public sources
//! recorded in `docs/FORMAT_SOURCES.md`: ECMA-167 (2nd edition, December
//! 1994) part 2 section 9 (volume recognition structures), part 3 sections
//! 7.2 (descriptor tags), 8.4.2 (volume descriptor sequences) and 10.2
//! (anchor volume descriptor pointer), and ECMA TR/71 sections 2.4 to 2.6.
//!
//! It is deliberately only a structural preflight: it does not claim full UDF
//! conformance, and it is independent of `hadris-udf`, so a defect in the
//! third-party reader cannot become the project's trust anchor. Every field is
//! read from a fixed offset inside a 2,048-byte descriptor that has already
//! been read in full, so no recorded length ever drives an allocation.

use ohl_core::SanitizedError;
use ohl_media_archive::{
    BLOCK_SIZE, BLOCK_SIZE_U32, BLOCK_SIZE_U64, Block, BlockReader, FilesystemDescription,
    MediaClass, MediaPreflight, VolumeLabel,
};

/// The logical sector holding the first anchor volume descriptor pointer
/// (ECMA-167 3/10.2).
pub const ANCHOR_LBA: u64 = 256;
/// The first logical sector of the volume recognition sequence (ECMA-167 2/9).
pub const FIRST_RECOGNITION_LBA: u64 = 16;
/// The recognition sequence scan stops here.
pub const RECOGNITION_SCAN_LIMIT: u64 = 64;
/// The main volume descriptor sequence scan stops after this many sectors.
pub const DESCRIPTOR_SCAN_LIMIT: u64 = 256;

/// Descriptor tag length (ECMA-167 3/7.2).
const TAG_BYTES: usize = 16;

const TAG_PRIMARY_VOLUME: u16 = 1;
const TAG_ANCHOR: u16 = 2;
const TAG_PARTITION: u16 = 5;
const TAG_LOGICAL_VOLUME: u16 = 6;
const TAG_TERMINATING: u16 = 8;

fn read_le_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_le_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// CRC-CCITT (ITU-T), the polynomial ECMA-167 3/7.2.4 specifies for
/// descriptor tags.
fn crc_itu_t(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            let high_bit_set = crc & 0x8000 != 0;
            crc <<= 1;
            if high_bit_set {
                crc ^= 0x1021;
            }
        }
    }
    crc
}

/// Whether the sector carries the ECMA-167 2/9.1 volume structure descriptor
/// with the given standard identifier and nothing else.
fn identifier_is(block: &Block, identifier: [u8; 5]) -> bool {
    block[0] == 0
        && block[6] == 1
        && block[1..6] == identifier
        && block[7..].iter().all(|byte| *byte == 0)
}

/// Validates one descriptor tag: identifier, version, reserved byte, recorded
/// location, tag checksum, and the exact CRC length the descriptor requires.
fn valid_descriptor_tag(block: &Block, expected_identifier: u16, expected_location: u32) -> bool {
    if read_le_u16(block, 0) != expected_identifier
        || read_le_u16(block, 2) != 2
        || block[5] != 0
        || read_le_u32(block, 12) != expected_location
    {
        return false;
    }

    let checksum = block[..TAG_BYTES]
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 4)
        .fold(0u8, |sum, (_, byte)| sum.wrapping_add(*byte));
    if checksum != block[4] {
        return false;
    }

    // Each descriptor records its own CRC length; requiring the exact value
    // stops a truncated or padded descriptor from validating.
    let expected_crc_length: u64 = match expected_identifier {
        TAG_LOGICAL_VOLUME => 424 + u64::from(read_le_u32(block, 264)),
        7 => 8 + 8 * u64::from(read_le_u32(block, 20)),
        _ => 496,
    };
    let crc_length = u64::from(read_le_u16(block, 10));
    if expected_crc_length > BLOCK_SIZE_U64 - TAG_BYTES as u64 || crc_length != expected_crc_length
    {
        return false;
    }
    let Ok(length) = usize::try_from(crc_length) else {
        return false;
    };
    crc_itu_t(&block[TAG_BYTES..TAG_BYTES + length]) == read_le_u16(block, 8)
}

/// Decodes an ECMA-167 1/7.2.12 `dstring` into a sanitized label.
fn decode_dstring(field: &[u8]) -> VolumeLabel {
    if field.len() < 2 {
        return VolumeLabel::default();
    }
    let encoded_length = usize::from(field[field.len() - 1]);
    if encoded_length < 1 || encoded_length > field.len() - 1 {
        return VolumeLabel::default();
    }
    match field[0] {
        8 => VolumeLabel::sanitize(&field[1..encoded_length]),
        16 if encoded_length % 2 == 1 => VolumeLabel::sanitize_ucs2_be(&field[1..encoded_length]),
        _ => VolumeLabel::default(),
    }
}

/// Whether an extent of `byte_length` bytes starting at `start_lba` lies
/// wholly inside a source of `block_count` logical blocks.
fn extent_is_in_bounds(byte_length: u32, start_lba: u32, block_count: u64) -> bool {
    if byte_length < 512 {
        return false;
    }
    let extent_blocks = u64::from(byte_length).div_ceil(BLOCK_SIZE_U64);
    u64::from(start_lba) < block_count
        && extent_blocks <= block_count
        && u64::from(start_lba) <= block_count - extent_blocks
}

/// The outcome of the recognition-sequence scan.
enum Recognition {
    /// A BEA01 / NSR02 / TEA01 sequence was found.
    Found,
    /// The sequence is absent, so this is not an ECMA-167 volume.
    Absent,
}

fn scan_recognition_sequence<R: BlockReader>(
    reader: &mut R,
) -> Result<Recognition, SanitizedError> {
    let mut block: Block = [0; BLOCK_SIZE];
    let mut found_beginning = false;
    let mut found_nsr02 = false;

    for lba in FIRST_RECOGNITION_LBA..RECOGNITION_SCAN_LIMIT {
        reader.read_block(lba, &mut block).map_err(Into::into)?;
        if !found_beginning {
            found_beginning = identifier_is(&block, *b"BEA01");
            continue;
        }
        if !found_nsr02 {
            if !identifier_is(&block, *b"NSR02") {
                return Ok(Recognition::Absent);
            }
            found_nsr02 = true;
            continue;
        }
        return Ok(if identifier_is(&block, *b"TEA01") {
            Recognition::Found
        } else {
            Recognition::Absent
        });
    }
    Ok(Recognition::Absent)
}

/// Walks the main volume descriptor sequence, requiring a primary volume
/// descriptor, a partition descriptor, a logical volume descriptor recording a
/// 2,048-byte block size, and a terminating descriptor.
fn inspect_volume_descriptor_sequence<R: BlockReader>(
    reader: &mut R,
    byte_length: u32,
    start_lba: u32,
) -> Result<VolumeLabel, SanitizedError> {
    let extent_blocks = u64::from(byte_length).div_ceil(BLOCK_SIZE_U64);
    let blocks_to_scan = extent_blocks.min(DESCRIPTOR_SCAN_LIMIT);

    let mut block: Block = [0; BLOCK_SIZE];
    let mut label = VolumeLabel::default();
    let mut found_primary = false;
    let mut found_partition = false;
    let mut found_logical = false;
    let mut found_terminator = false;

    for index in 0..blocks_to_scan {
        let lba = u64::from(start_lba) + index;
        reader.read_block(lba, &mut block).map_err(Into::into)?;
        let identifier = read_le_u16(&block, 0);
        if identifier == 0 {
            break;
        }
        let location = u32::try_from(lba).map_err(|_| SanitizedError::InvalidInput)?;
        if !valid_descriptor_tag(&block, identifier, location) {
            return Err(SanitizedError::InvalidInput);
        }
        match identifier {
            TAG_PRIMARY_VOLUME => {
                found_primary = true;
                label = decode_dstring(&block[24..56]);
            }
            TAG_PARTITION => found_partition = true,
            TAG_LOGICAL_VOLUME => {
                found_logical = read_le_u32(&block, 212) == BLOCK_SIZE_U32;
            }
            TAG_TERMINATING => {
                found_terminator = true;
                break;
            }
            _ => {}
        }
    }

    if found_primary && found_partition && found_logical && found_terminator {
        Ok(label)
    } else {
        Err(SanitizedError::InvalidInput)
    }
}

/// Runs the bounded ECMA-167 NSR02 preflight over `reader`.
///
/// # Errors
///
/// - [`SanitizedError::Unsupported`] when no ECMA-167 volume recognition
///   sequence is present, so the media is some other class.
/// - [`SanitizedError::InvalidInput`] when a recognition sequence exists but
///   the anchor, descriptor tags, CRCs, extents, or required descriptors are
///   malformed.
/// - The reader's own sanitized error when a block could not be read.
pub fn preflight<R: BlockReader>(reader: &mut R) -> Result<MediaPreflight, SanitizedError> {
    let block_count = reader.block_count();
    if block_count <= ANCHOR_LBA {
        return Err(SanitizedError::Unsupported);
    }

    match scan_recognition_sequence(reader)? {
        Recognition::Found => {}
        Recognition::Absent => return Err(SanitizedError::Unsupported),
    }

    let mut anchor: Block = [0; BLOCK_SIZE];
    reader
        .read_block(ANCHOR_LBA, &mut anchor)
        .map_err(Into::into)?;
    let anchor_location = u32::try_from(ANCHOR_LBA).map_err(|_| SanitizedError::Internal)?;
    if !valid_descriptor_tag(&anchor, TAG_ANCHOR, anchor_location) {
        return Err(SanitizedError::InvalidInput);
    }

    let descriptor_bytes = read_le_u32(&anchor, 16);
    let descriptor_lba = read_le_u32(&anchor, 20);
    if !extent_is_in_bounds(descriptor_bytes, descriptor_lba, block_count) {
        return Err(SanitizedError::InvalidInput);
    }

    let volume_label =
        inspect_volume_descriptor_sequence(reader, descriptor_bytes, descriptor_lba)?;
    Ok(MediaPreflight {
        media_class: MediaClass::Udf,
        filesystem: FilesystemDescription::Ecma167Nsr02,
        volume_label,
    })
}
