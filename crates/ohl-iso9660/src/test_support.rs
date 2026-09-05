//! Project-authored synthetic ECMA-119 image builder used only by tests.
//!
//! Every byte written here comes from the public ECMA-119 structure and the
//! public Joliet escape-sequence definition recorded in
//! `docs/FORMAT_SOURCES.md`. No name, layout, count or byte originates from
//! any real medium. This is a direct port of the C++ builder in
//! `tests/media/synthetic_media_test_support.hpp` so that both test suites
//! exercise the same fixtures.

use alloc::string::{String, ToString as _};
use alloc::vec;
use alloc::vec::Vec;
use ohl_media_archive::{BLOCK_SIZE, BLOCK_SIZE_U32};

/// Default image size in logical blocks.
pub const SECTOR_COUNT: usize = 300;
/// Logical sector holding the primary volume descriptor (ECMA-119 8.1).
pub const PRIMARY_DESCRIPTOR_SECTOR: u32 = 16;
/// Logical sector holding the minimal type-L path table.
pub const PATH_TABLE_SECTOR: u32 = 19;
/// Root directory extent of the primary tree.
pub const PRIMARY_ROOT_SECTOR: u32 = 24;
/// Child directory extent of the primary tree.
pub const PRIMARY_CHILD_SECTOR: u32 = 25;
/// Root directory extent of the Joliet tree.
pub const JOLIET_ROOT_SECTOR: u32 = 26;
/// Child directory extent of the Joliet tree.
pub const JOLIET_CHILD_SECTOR: u32 = 27;
/// Extent holding the sentinel payload.
pub const SENTINEL_DATA_SECTOR: u32 = 30;
/// Extent holding the nested payload.
pub const NESTED_DATA_SECTOR: u32 = 31;

/// Volume label written into every synthetic descriptor.
pub const VOLUME_LABEL: &str = "OHL SYNTHETIC";
/// Primary-tree subdirectory identifier.
pub const PRIMARY_DIRECTORY_NAME: &str = "FIXDIR";
/// Primary-tree file identifier, before its `;1` version suffix.
pub const PRIMARY_SENTINEL_NAME: &str = "SENTINEL.TXT";
/// Primary-tree nested file identifier.
pub const PRIMARY_NESTED_NAME: &str = "NESTED.BIN";
/// Primary-tree identifier of the deliberate cycle entry.
pub const PRIMARY_LOOP_NAME: &str = "LOOPDIR";
/// Joliet subdirectory identifier.
pub const JOLIET_DIRECTORY_NAME: &str = "FixtureDir";
/// Joliet file identifier.
pub const JOLIET_SENTINEL_NAME: &str = "Sentinel.txt";
/// Joliet nested file identifier.
pub const JOLIET_NESTED_NAME: &str = "Nested.bin";
/// Joliet identifier of the deliberate cycle entry.
pub const JOLIET_LOOP_NAME: &str = "LoopDir";
/// Sentinel payload contents.
pub const SENTINEL_CONTENTS: &str = "open-half-life synthetic sentinel payload\n";
/// Nested payload contents.
pub const NESTED_CONTENTS: &str = "open-half-life synthetic nested payload\n";

/// One deliberate structural defect, or none.
///
/// Each flag is an independent, named defect rather than an enumeration so a
/// test can combine them; that is what the many booleans express.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Image size in logical blocks.
    pub sector_count: usize,
    /// Write a Joliet supplementary volume descriptor.
    pub joliet: bool,
    /// Write the volume descriptor set terminator.
    pub terminator: bool,
    /// Recorded logical block size.
    pub logical_block_size: u16,
    /// Record a volume space size larger than the image.
    pub volume_space_too_large: bool,
    /// Point the root directory extent outside the volume.
    pub root_extent_outside_volume: bool,
    /// Point a file extent outside the volume.
    pub file_extent_outside_volume: bool,
    /// Add a child entry whose extent is its own ancestor.
    pub directory_cycle: bool,
    /// Declare a file identifier longer than the record can hold.
    pub overlong_identifier: bool,
    /// Mark a file record as a non-final multi-extent record.
    pub multi_extent_file: bool,
    /// Extra root-directory files, used to exercise paging.
    pub extra_root_files: u32,
    /// Volume sequence number recorded in the file record.
    pub file_record_volume_sequence: u16,
    /// Replace the root record's data length (0 keeps one whole block).
    pub root_size_override: u32,
    /// Replace the child directory record's data length.
    pub child_directory_size_override: u32,
    /// Write an extra, deliberately malformed supplementary descriptor that
    /// carries no Joliet escape sequence.
    pub malformed_non_joliet_supplementary: bool,
    /// Add two Joliet siblings differing only in ASCII case.
    pub joliet_case_siblings: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sector_count: SECTOR_COUNT,
            joliet: true,
            terminator: true,
            logical_block_size: 2_048,
            volume_space_too_large: false,
            root_extent_outside_volume: false,
            file_extent_outside_volume: false,
            directory_cycle: false,
            overlong_identifier: false,
            multi_extent_file: false,
            extra_root_files: 0,
            file_record_volume_sequence: 1,
            root_size_override: 0,
            child_directory_size_override: 0,
            malformed_non_joliet_supplementary: false,
            joliet_case_siblings: false,
        }
    }
}

fn write_both_u16(image: &mut [u8], offset: usize, value: u16) {
    image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    image[offset + 2..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn write_both_u32(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    image[offset + 4..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn write_ascii(image: &mut [u8], offset: usize, value: &str) {
    image[offset..offset + value.len()].copy_from_slice(value.as_bytes());
}

fn fill_ascii_field(image: &mut [u8], offset: usize, size: usize, value: &str) {
    for index in 0..size {
        image[offset + index] = value.as_bytes().get(index).copied().unwrap_or(b' ');
    }
}

/// UCS-2 big-endian field padded with U+0020, as Joliet requires.
fn fill_ucs2_field(image: &mut [u8], offset: usize, size: usize, value: &str) {
    for index in 0..size / 2 {
        let character = value.as_bytes().get(index).copied().unwrap_or(b' ');
        image[offset + index * 2] = 0;
        image[offset + index * 2 + 1] = character;
    }
}

/// Encodes a directory-record identifier, optionally as UCS-2 big endian.
pub fn identifier(name: &str, ucs2: bool) -> Vec<u8> {
    if ucs2 {
        name.bytes().flat_map(|byte| [0u8, byte]).collect()
    } else {
        name.as_bytes().to_vec()
    }
}

/// One synthetic directory record.
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// Raw identifier bytes.
    pub identifier: Vec<u8>,
    /// Extent's first logical block.
    pub extent: u32,
    /// Recorded data length in bytes.
    pub size: u32,
    /// Whether the directory flag is set.
    pub directory: bool,
    /// Extra file-flag bits, such as the multi-extent bit.
    pub extra_flags: u8,
    /// Overrides the declared identifier length when non-zero.
    pub declared_identifier_length_override: u8,
    /// Recorded volume sequence number.
    pub volume_sequence: u16,
}

fn write_record(image: &mut [u8], offset: usize, record: &Record) -> usize {
    let identifier_length = record.identifier.len();
    let mut length = 33 + identifier_length;
    if !length.is_multiple_of(2) {
        length += 1;
    }
    image[offset] = u8::try_from(length).expect("synthetic records stay under 256 bytes");
    image[offset + 1] = 0;
    write_both_u32(image, offset + 2, record.extent);
    write_both_u32(image, offset + 10, record.size);
    image[offset + 18] = 98; // years since 1900
    image[offset + 19] = 1;
    image[offset + 20] = 1;
    image[offset + 25] = if record.directory { 0x02 } else { 0x00 } | record.extra_flags;
    image[offset + 26] = 0;
    image[offset + 27] = 0;
    write_both_u16(image, offset + 28, record.volume_sequence);
    image[offset + 32] = if record.declared_identifier_length_override != 0 {
        record.declared_identifier_length_override
    } else {
        u8::try_from(identifier_length).expect("synthetic identifiers stay under 256 bytes")
    };
    image[offset + 33..offset + 33 + identifier_length].copy_from_slice(&record.identifier);
    length
}

fn write_directory(image: &mut [u8], sector: u32, parent_sector: u32, records: &[Record]) {
    let base = sector as usize * BLOCK_SIZE;
    let mut offset = base;
    let mut current = Record {
        identifier: vec![0x00],
        extent: sector,
        size: BLOCK_SIZE_U32,
        directory: true,
        volume_sequence: 1,
        ..Record::default()
    };
    offset += write_record(image, offset, &current);
    current.identifier = vec![0x01];
    current.extent = parent_sector;
    offset += write_record(image, offset, &current);
    for record in records {
        offset += write_record(image, offset, record);
    }
}

fn write_volume_descriptor(
    image: &mut [u8],
    sector: u32,
    descriptor_type: u8,
    options: &Options,
    volume_blocks: u32,
    root_sector: u32,
    joliet: bool,
) {
    let base = sector as usize * BLOCK_SIZE;
    image[base] = descriptor_type;
    write_ascii(image, base + 1, "CD001");
    image[base + 6] = 1;
    fill_ascii_field(image, base + 8, 32, "");
    if joliet {
        fill_ucs2_field(image, base + 40, 32, VOLUME_LABEL);
        image[base + 88] = 0x25;
        image[base + 89] = 0x2f;
        image[base + 90] = 0x45;
    } else {
        fill_ascii_field(image, base + 40, 32, VOLUME_LABEL);
    }
    write_both_u32(
        image,
        base + 80,
        if options.volume_space_too_large {
            volume_blocks + 100
        } else {
            volume_blocks
        },
    );
    write_both_u16(image, base + 120, 1);
    write_both_u16(image, base + 124, 1);
    write_both_u16(image, base + 128, options.logical_block_size);
    write_both_u32(image, base + 132, 10);
    image[base + 140..base + 144].copy_from_slice(&PATH_TABLE_SECTOR.to_le_bytes());
    image[base + 144..base + 148].copy_from_slice(&0u32.to_le_bytes());
    image[base + 148..base + 152].copy_from_slice(&PATH_TABLE_SECTOR.to_be_bytes());
    image[base + 152..base + 156].copy_from_slice(&0u32.to_be_bytes());

    let root = Record {
        identifier: vec![0x00],
        extent: if options.root_extent_outside_volume {
            volume_blocks + 5
        } else {
            root_sector
        },
        size: if options.root_size_override != 0 {
            options.root_size_override
        } else {
            BLOCK_SIZE_U32
        },
        directory: true,
        volume_sequence: 1,
        ..Record::default()
    };
    let _ = write_record(image, base + 156, &root);

    fill_ascii_field(image, base + 190, 128, "");
    fill_ascii_field(image, base + 318, 128, "");
    fill_ascii_field(image, base + 446, 128, "");
    image[base + 881] = 1;
}

/// Builds one complete synthetic ECMA-119 image.
///
/// # Panics
///
/// Panics when `options.sector_count` is under 64 blocks, which is smaller
/// than the fixed fixture layout requires.
pub fn make_image(options: Options) -> Vec<u8> {
    assert!(
        options.sector_count >= 64,
        "the synthetic ECMA-119 fixture layout needs at least 64 logical blocks"
    );
    let mut image = vec![0u8; options.sector_count * BLOCK_SIZE];
    let volume_blocks = u32::try_from(options.sector_count).expect("fixtures stay small");

    write_volume_descriptor(
        &mut image,
        PRIMARY_DESCRIPTOR_SECTOR,
        1,
        &options,
        volume_blocks,
        PRIMARY_ROOT_SECTOR,
        false,
    );
    let mut next_descriptor = PRIMARY_DESCRIPTOR_SECTOR + 1;
    if options.joliet {
        write_volume_descriptor(
            &mut image,
            next_descriptor,
            2,
            &options,
            volume_blocks,
            JOLIET_ROOT_SECTOR,
            true,
        );
        next_descriptor += 1;
    }
    if options.malformed_non_joliet_supplementary {
        let malformed = Options {
            logical_block_size: 512,
            root_extent_outside_volume: true,
            ..options
        };
        write_volume_descriptor(
            &mut image,
            next_descriptor,
            2,
            &malformed,
            volume_blocks,
            PRIMARY_ROOT_SECTOR,
            false,
        );
        next_descriptor += 1;
    }
    if options.terminator {
        let base = next_descriptor as usize * BLOCK_SIZE;
        image[base] = 255;
        write_ascii(&mut image, base + 1, "CD001");
        image[base + 6] = 1;
    }

    // Minimal type-L path table describing only the root directory.
    let path_table = PATH_TABLE_SECTOR as usize * BLOCK_SIZE;
    image[path_table] = 1;
    image[path_table + 1] = 0;
    image[path_table + 2..path_table + 6].copy_from_slice(&PRIMARY_ROOT_SECTOR.to_le_bytes());
    image[path_table + 6] = 1;
    image[path_table + 8] = 0;

    write_ascii(
        &mut image,
        SENTINEL_DATA_SECTOR as usize * BLOCK_SIZE,
        SENTINEL_CONTENTS,
    );
    write_ascii(
        &mut image,
        NESTED_DATA_SECTOR as usize * BLOCK_SIZE,
        NESTED_CONTENTS,
    );

    build_tree(
        &mut image,
        &options,
        volume_blocks,
        TreeNames {
            ucs2: false,
            root_sector: PRIMARY_ROOT_SECTOR,
            child_sector: PRIMARY_CHILD_SECTOR,
            directory: PRIMARY_DIRECTORY_NAME,
            sentinel: PRIMARY_SENTINEL_NAME,
            nested: PRIMARY_NESTED_NAME,
            loop_name: PRIMARY_LOOP_NAME,
        },
    );
    if options.joliet {
        build_tree(
            &mut image,
            &options,
            volume_blocks,
            TreeNames {
                ucs2: true,
                root_sector: JOLIET_ROOT_SECTOR,
                child_sector: JOLIET_CHILD_SECTOR,
                directory: JOLIET_DIRECTORY_NAME,
                sentinel: JOLIET_SENTINEL_NAME,
                nested: JOLIET_NESTED_NAME,
                loop_name: JOLIET_LOOP_NAME,
            },
        );
    }
    image
}

#[derive(Clone, Copy)]
struct TreeNames {
    ucs2: bool,
    root_sector: u32,
    child_sector: u32,
    directory: &'static str,
    sentinel: &'static str,
    nested: &'static str,
    loop_name: &'static str,
}

fn build_tree(image: &mut [u8], options: &Options, volume_blocks: u32, names: TreeNames) {
    let ucs2 = names.ucs2;
    let mut root_records = Vec::new();
    root_records.push(Record {
        identifier: identifier(names.directory, ucs2),
        extent: names.child_sector,
        size: if options.child_directory_size_override != 0 {
            options.child_directory_size_override
        } else {
            BLOCK_SIZE_U32
        },
        directory: true,
        volume_sequence: 1,
        ..Record::default()
    });

    let sentinel_identifier = identifier(&(names.sentinel.to_string() + ";1"), ucs2);
    let sentinel_identifier_length = sentinel_identifier.len();
    root_records.push(Record {
        identifier: sentinel_identifier,
        extent: if options.file_extent_outside_volume {
            volume_blocks + 5
        } else {
            SENTINEL_DATA_SECTOR
        },
        size: u32::try_from(SENTINEL_CONTENTS.len()).expect("fixture payloads are small"),
        directory: false,
        extra_flags: if options.multi_extent_file { 0x80 } else { 0 },
        declared_identifier_length_override: if options.overlong_identifier {
            u8::try_from(sentinel_identifier_length + 40).expect("fixture identifiers are short")
        } else {
            0
        },
        volume_sequence: options.file_record_volume_sequence,
    });

    if options.joliet_case_siblings && ucs2 {
        for (name, size) in [("CaseName.txt", 8u32), ("casename.txt", 9u32)] {
            root_records.push(Record {
                identifier: identifier(&(name.to_string() + ";1"), true),
                extent: SENTINEL_DATA_SECTOR,
                size,
                directory: false,
                volume_sequence: 1,
                ..Record::default()
            });
        }
    }

    for index in 0..options.extra_root_files {
        let name = String::from("EXTRA") + &index.to_string() + ".TXT;1";
        root_records.push(Record {
            identifier: identifier(&name, ucs2),
            extent: SENTINEL_DATA_SECTOR,
            size: 8,
            directory: false,
            volume_sequence: 1,
            ..Record::default()
        });
    }
    write_directory(image, names.root_sector, names.root_sector, &root_records);

    let mut child_records = vec![Record {
        identifier: identifier(&(names.nested.to_string() + ";1"), ucs2),
        extent: NESTED_DATA_SECTOR,
        size: u32::try_from(NESTED_CONTENTS.len()).expect("fixture payloads are small"),
        directory: false,
        volume_sequence: 1,
        ..Record::default()
    }];
    if options.directory_cycle {
        child_records.push(Record {
            identifier: identifier(names.loop_name, ucs2),
            extent: names.root_sector,
            size: BLOCK_SIZE_U32,
            directory: true,
            volume_sequence: 1,
            ..Record::default()
        });
    }
    write_directory(image, names.child_sector, names.root_sector, &child_records);
}
