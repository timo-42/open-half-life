//! Bounded ECMA-119 structural preflight.
//!
//! This module is project-owned and written only from the public sources
//! recorded in `docs/FORMAT_SOURCES.md`: ECMA-119 (4th edition, June 2019)
//! sections 6.2, 7.2.3, 8.1, 8.3, 8.4, 8.5 and 9.1, and Microsoft's public
//! Joliet specification for the reserved `%/@`, `%/C` and `%/E` escape
//! sequences. It is intentionally independent of `hadris-iso`: the preflight
//! decides whether media may be used at all, and the archive re-validates
//! everything it later parses, so a defect in one cannot silently become the
//! other's trust anchor.
//!
//! Every field is read from a fixed offset inside one 2,048-byte descriptor
//! that has already been read in full, so no recorded length ever drives an
//! allocation, and every both-byte-order pair (ECMA-119 7.2.3) must agree
//! before its value is used.

use ohl_core::SanitizedError;
use ohl_media_archive::{
    BLOCK_SIZE, BLOCK_SIZE_U32, BLOCK_SIZE_U64, Block, BlockReader, FilesystemDescription,
    MediaClass, MediaPreflight, VolumeLabel,
};

/// The first logical sector of the volume descriptor set (ECMA-119 8.1).
pub const FIRST_DESCRIPTOR_LBA: u64 = 16;

/// The largest number of volume descriptors the preflight examines.
pub const MAX_DESCRIPTORS_SCANNED: u64 = 32;

/// The size of a root directory record inside a volume descriptor.
const ROOT_RECORD_BYTES: usize = 34;

const TYPE_PRIMARY: u8 = 1;
const TYPE_SUPPLEMENTARY: u8 = 2;
const TYPE_TERMINATOR: u8 = 255;

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

/// Reads an ECMA-119 7.2.3 both-byte-order 32-bit value, requiring the two
/// recordings to agree.
fn read_both_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let little = read_le_u32(bytes, offset);
    let big = u32::from_be_bytes([
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ]);
    (little == big).then_some(little)
}

/// Reads an ECMA-119 7.2.3 both-byte-order 16-bit value.
fn read_both_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let little = read_le_u16(bytes, offset);
    let big = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]);
    (little == big).then_some(little)
}

/// Whether the descriptor carries the ECMA-119 8.1 standard identifier and
/// descriptor version.
fn is_volume_descriptor(block: &Block) -> bool {
    &block[1..6] == b"CD001" && block[6] == 1
}

/// Whether a supplementary descriptor records one of the three escape
/// sequences the public Joliet specification reserves.
fn has_joliet_escape(block: &Block) -> bool {
    block[88] == 0x25 && block[89] == 0x2f && matches!(block[90], 0x40 | 0x43 | 0x45)
}

/// The geometry a validated descriptor contributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorGeometry {
    /// The volume space size in logical blocks (ECMA-119 8.4.8).
    pub volume_blocks: u32,
    /// The root directory extent's first logical block.
    pub root_extent: u32,
    /// The root directory's recorded data length in bytes.
    pub root_length: u32,
}

/// Validates one primary or supplementary descriptor against the pinned image
/// geometry.
///
/// `block_count` is the number of whole 2,048-byte blocks the source actually
/// contains, so no accepted extent can point outside it.
fn validate_descriptor(block: &Block, block_count: u64) -> Option<DescriptorGeometry> {
    // ECMA-119 6.2: the project supports only 2,048-byte logical blocks.
    if u32::from(read_both_u16(block, 128)?) != BLOCK_SIZE_U32 {
        return None;
    }

    // ECMA-119 8.4.6/8.4.7: a volume set must have at least one volume and
    // this descriptor must describe the first of them.
    let volume_set_size = read_both_u16(block, 120)?;
    let volume_sequence = read_both_u16(block, 124)?;
    if volume_set_size == 0 || volume_sequence != 1 || volume_sequence > volume_set_size {
        return None;
    }

    // ECMA-119 8.4.8: the recorded volume space must fit inside the source.
    let volume_blocks = read_both_u32(block, 80)?;
    if volume_blocks == 0 || u64::from(volume_blocks) > block_count {
        return None;
    }

    // ECMA-119 8.4.10 through 8.4.15: the path tables must lie in the volume.
    let path_table_bytes = read_both_u32(block, 132)?;
    if path_table_bytes == 0
        || u64::from(path_table_bytes) > u64::from(volume_blocks) * BLOCK_SIZE_U64
    {
        return None;
    }
    let path_table_blocks = u64::from(path_table_bytes).div_ceil(BLOCK_SIZE_U64);
    let path_table_in_bounds = |location: u32| {
        // Optional tables are recorded as zero.
        location == 0 || u64::from(location) + path_table_blocks <= u64::from(volume_blocks)
    };
    let type_l = read_le_u32(block, 140);
    if type_l == 0
        || !path_table_in_bounds(type_l)
        || !path_table_in_bounds(read_le_u32(block, 144))
    {
        return None;
    }

    // ECMA-119 8.4.18 and 9.1: the root directory record.
    let root = &block[156..156 + ROOT_RECORD_BYTES];
    let flags = root[25];
    if root[0] as usize != ROOT_RECORD_BYTES
        || root[1] != 0 // extended attribute records are not interpreted
        || flags & 0x02 == 0 // must be a directory
        || flags & 0x80 != 0 // must not be a non-final multi-extent record
        || root[26] != 0 // file unit size: no interleaving
        || root[27] != 0
    // interleave gap size: no interleaving
    {
        return None;
    }
    let root_extent = read_both_u32(root, 2)?;
    let root_length = read_both_u32(root, 10)?;
    let root_sequence = read_both_u16(root, 28)?;
    // ECMA-119 9.1.4: a directory's data length is a whole number of blocks,
    // and only the first volume of a volume set is readable here.
    if root_length == 0
        || !u64::from(root_length).is_multiple_of(BLOCK_SIZE_U64)
        || root_sequence != 1
        || root_sequence > volume_set_size
    {
        return None;
    }
    let root_blocks = u64::from(root_length).div_ceil(BLOCK_SIZE_U64);
    if u64::from(root_extent) + root_blocks > u64::from(volume_blocks) {
        return None;
    }

    Some(DescriptorGeometry {
        volume_blocks,
        root_extent,
        root_length,
    })
}

/// The trees a validated descriptor set offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso9660Preflight {
    /// The sanitized classification.
    pub media: MediaPreflight,
    /// The primary (ECMA-119) directory tree.
    pub primary: DescriptorGeometry,
    /// The Joliet tree, when a supplementary descriptor recorded one.
    pub joliet: Option<DescriptorGeometry>,
}

impl Iso9660Preflight {
    /// Whether a Joliet tree is present and therefore preferred.
    pub fn uses_joliet(&self) -> bool {
        self.joliet.is_some()
    }

    /// The geometry of the preferred tree.
    pub fn preferred(&self) -> DescriptorGeometry {
        self.joliet.unwrap_or(self.primary)
    }
}

/// Runs the bounded ECMA-119 preflight over `reader`.
///
/// # Errors
///
/// - [`SanitizedError::Unsupported`] when logical sector 16 carries no
///   ECMA-119 primary volume descriptor, so the media is some other class.
/// - [`SanitizedError::InvalidInput`] when a descriptor set exists but is
///   truncated, inconsistent, or describes geometry outside the source.
/// - The reader's own sanitized error when a block could not be read.
pub fn preflight<R: BlockReader>(reader: &mut R) -> Result<Iso9660Preflight, SanitizedError> {
    let block_count = reader.block_count();
    if block_count <= FIRST_DESCRIPTOR_LBA {
        return Err(SanitizedError::Unsupported);
    }

    let mut block: Block = [0; BLOCK_SIZE];
    reader
        .read_block(FIRST_DESCRIPTOR_LBA, &mut block)
        .map_err(Into::into)?;
    if !is_volume_descriptor(&block) || block[0] != TYPE_PRIMARY {
        return Err(SanitizedError::Unsupported);
    }

    let mut primary: Option<DescriptorGeometry> = None;
    let mut joliet: Option<DescriptorGeometry> = None;
    let mut primary_label = VolumeLabel::default();
    let mut joliet_label = VolumeLabel::default();
    let mut found_terminator = false;

    for index in 0..MAX_DESCRIPTORS_SCANNED {
        let lba = FIRST_DESCRIPTOR_LBA + index;
        if lba >= block_count {
            return Err(SanitizedError::InvalidInput);
        }
        if index != 0 {
            reader.read_block(lba, &mut block).map_err(Into::into)?;
        }
        if !is_volume_descriptor(&block) {
            return Err(SanitizedError::InvalidInput);
        }
        match block[0] {
            TYPE_TERMINATOR => {
                found_terminator = true;
                break;
            }
            TYPE_PRIMARY => {
                let geometry =
                    validate_descriptor(&block, block_count).ok_or(SanitizedError::InvalidInput)?;
                primary = Some(geometry);
                primary_label = VolumeLabel::sanitize(&block[40..72]);
            }
            TYPE_SUPPLEMENTARY if has_joliet_escape(&block) => {
                let geometry =
                    validate_descriptor(&block, block_count).ok_or(SanitizedError::InvalidInput)?;
                joliet = Some(geometry);
                joliet_label = VolumeLabel::sanitize_ucs2_be(&block[40..72]);
            }
            // A supplementary descriptor without a Joliet escape sequence
            // describes an encoding this reader does not interpret. It is
            // skipped rather than validated, so a defect in one cannot reject
            // an otherwise valid primary volume.
            _ => {}
        }
    }

    let Some(primary) = primary else {
        return Err(SanitizedError::InvalidInput);
    };
    if !found_terminator {
        return Err(SanitizedError::InvalidInput);
    }

    let volume_label = if joliet.is_some() && !joliet_label.is_empty() {
        joliet_label
    } else {
        primary_label
    };
    let filesystem = if joliet.is_some() {
        FilesystemDescription::Iso9660Joliet
    } else {
        FilesystemDescription::Iso9660
    };

    Ok(Iso9660Preflight {
        media: MediaPreflight {
            media_class: MediaClass::Iso9660,
            filesystem,
            volume_label,
        },
        primary,
        joliet,
    })
}
