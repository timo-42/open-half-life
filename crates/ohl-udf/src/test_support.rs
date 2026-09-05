//! Project-authored synthetic ECMA-167 image builder used only by tests.
//!
//! Every byte written here comes from the public ECMA-167 structure recorded
//! in `docs/FORMAT_SOURCES.md`. No name, layout, count or byte originates from
//! any real medium. This is a direct port of the C++ builder in
//! `tests/media/synthetic_media_test_support.hpp`.

use alloc::vec;
use alloc::vec::Vec;
use ohl_media_archive::{BLOCK_SIZE, BLOCK_SIZE_U32};

/// Default image size in logical blocks. The anchor lives at block 256, so a
/// smaller image cannot carry a recognizable ECMA-167 volume.
pub const SECTOR_COUNT: usize = 300;
/// Volume recognition sequence blocks (ECMA-167 2/9).
pub const BEA01_SECTOR: u32 = 18;
/// NSR02 recognition block.
pub const NSR02_SECTOR: u32 = 19;
/// TEA01 recognition block.
pub const TEA01_SECTOR: u32 = 20;
/// First block of the main volume descriptor sequence.
pub const PRIMARY_SECTOR: u32 = 32;
/// Partition descriptor block.
pub const PARTITION_SECTOR: u32 = 33;
/// Logical volume descriptor block.
pub const LOGICAL_SECTOR: u32 = 34;
/// Terminating descriptor block.
pub const TERMINATOR_SECTOR: u32 = 35;
/// Anchor volume descriptor pointer block (ECMA-167 3/10.2).
pub const ANCHOR_SECTOR: u32 = 256;
/// Volume label recorded in the synthetic primary volume descriptor.
pub const VOLUME_LABEL: &str = "PROJECT SYNTHETIC";

/// One deliberate structural defect, or none.
///
/// Each flag is an independent, named defect rather than an enumeration so a
/// test can combine them; that is what the many booleans express.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Omit the whole volume recognition sequence.
    pub no_recognition_sequence: bool,
    /// Record a recognition identifier the project does not accept.
    pub wrong_nsr_identifier: bool,
    /// Corrupt the anchor's tag checksum.
    pub corrupt_anchor_checksum: bool,
    /// Record a zero descriptor CRC length in the anchor.
    pub zero_anchor_crc_length: bool,
    /// Corrupt one payload byte so the anchor's CRC no longer matches.
    pub anchor_crc_mismatch: bool,
    /// Point the main descriptor sequence outside the image.
    pub descriptor_extent_out_of_bounds: bool,
    /// Record a main descriptor sequence length near `u32::MAX`.
    pub descriptor_extent_near_u32_max: bool,
    /// Erase the partition descriptor.
    pub missing_partition_descriptor: bool,
    /// Erase the terminating descriptor.
    pub missing_terminator: bool,
    /// Record a logical block size other than 2,048.
    pub logical_block_size: u32,
}

fn write_le_u16(image: &mut [u8], offset: usize, value: u16) {
    image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_le_u32(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// CRC-CCITT (ITU-T), as ECMA-167 3/7.2.4 specifies.
pub fn crc_itu_t(bytes: &[u8]) -> u16 {
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

/// Completes one descriptor tag in place: identifier, version, CRC and the
/// tag checksum over the other fifteen tag bytes.
pub fn finish_tag(image: &mut [u8], sector: u32, identifier: u16, crc_length: u16) {
    let base = sector as usize * BLOCK_SIZE;
    write_le_u16(image, base, identifier);
    write_le_u16(image, base + 2, 2);
    write_le_u16(image, base + 6, 1);
    write_le_u16(image, base + 10, crc_length);
    write_le_u32(image, base + 12, sector);
    let crc = crc_itu_t(&image[base + 16..base + 16 + crc_length as usize]);
    write_le_u16(image, base + 8, crc);

    let checksum = (0..16)
        .filter(|index| *index != 4)
        .fold(0u8, |sum, index| sum.wrapping_add(image[base + index]));
    image[base + 4] = checksum;
}

fn set_recognition_identifier(image: &mut [u8], sector: u32, identifier: [u8; 5]) {
    let base = sector as usize * BLOCK_SIZE;
    image[base] = 0;
    image[base + 1..base + 6].copy_from_slice(&identifier);
    image[base + 6] = 1;
}

/// Builds one synthetic ECMA-167 image with `sector_count` logical blocks.
///
/// The result passes the project's bounded NSR02 preflight unless `options`
/// asks for a defect. It is not a complete UDF filesystem: the project's
/// preflight deliberately stops at the volume descriptor sequence.
///
/// # Panics
///
/// Panics when `sector_count` cannot hold the anchor at block 256.
pub fn make_image(sector_count: usize, options: Options) -> Vec<u8> {
    assert!(
        sector_count > ANCHOR_SECTOR as usize,
        "an ECMA-167 fixture must be able to hold the anchor at block 256"
    );
    let mut image = vec![0u8; sector_count * BLOCK_SIZE];

    if !options.no_recognition_sequence {
        set_recognition_identifier(&mut image, BEA01_SECTOR, *b"BEA01");
        set_recognition_identifier(
            &mut image,
            NSR02_SECTOR,
            if options.wrong_nsr_identifier {
                *b"NSR03"
            } else {
                *b"NSR02"
            },
        );
        set_recognition_identifier(&mut image, TEA01_SECTOR, *b"TEA01");
    }

    // Primary volume descriptor with a CS0 compression-ID-8 dstring label.
    let primary = PRIMARY_SECTOR as usize * BLOCK_SIZE;
    image[primary + 24] = 8;
    image[primary + 25..primary + 25 + VOLUME_LABEL.len()].copy_from_slice(VOLUME_LABEL.as_bytes());
    image[primary + 55] = u8::try_from(VOLUME_LABEL.len() + 1).expect("the label is short");
    finish_tag(&mut image, PRIMARY_SECTOR, 1, 496);

    if !options.missing_partition_descriptor {
        finish_tag(&mut image, PARTITION_SECTOR, 5, 496);
    }

    let logical = LOGICAL_SECTOR as usize * BLOCK_SIZE;
    write_le_u32(
        &mut image,
        logical + 212,
        if options.logical_block_size == 0 {
            BLOCK_SIZE_U32
        } else {
            options.logical_block_size
        },
    );
    finish_tag(&mut image, LOGICAL_SECTOR, 6, 424);

    if !options.missing_terminator {
        finish_tag(&mut image, TERMINATOR_SECTOR, 8, 496);
    }

    let anchor = ANCHOR_SECTOR as usize * BLOCK_SIZE;
    let descriptor_bytes = if options.descriptor_extent_near_u32_max {
        u32::MAX - 2_047
    } else {
        16 * 2_048
    };
    let descriptor_sector = if options.descriptor_extent_out_of_bounds {
        u32::try_from(sector_count + 1).expect("fixtures stay small")
    } else {
        PRIMARY_SECTOR
    };
    write_le_u32(&mut image, anchor + 16, descriptor_bytes);
    write_le_u32(&mut image, anchor + 20, descriptor_sector);
    write_le_u32(&mut image, anchor + 24, 16 * 2_048);
    write_le_u32(&mut image, anchor + 28, 48);
    finish_tag(
        &mut image,
        ANCHOR_SECTOR,
        2,
        if options.zero_anchor_crc_length {
            0
        } else {
            496
        },
    );
    if options.corrupt_anchor_checksum {
        image[anchor + 4] ^= 1;
    }
    if options.anchor_crc_mismatch {
        image[anchor + 100] ^= 0xff;
    }

    image
}

/// The default valid fixture.
pub fn valid_image() -> Vec<u8> {
    make_image(SECTOR_COUNT, Options::default())
}
