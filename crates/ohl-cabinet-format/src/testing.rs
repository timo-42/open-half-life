//! Synthetic cabinet **header** writer for tests and fuzzing.
//!
//! Everything here is independently authored for this project: it writes
//! structures whose layout this crate already parses, using only names the
//! caller supplies. It contains no proprietary data and must never be
//! enabled in a production build.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::common::{CAB_SIGNATURE, COMMON_HEADER_SIZE};
use crate::descriptor::MIN_CAB_DESCRIPTOR_SIZE;
use crate::file::{FILE_DESCRIPTOR_SIZE_V5, FILE_DESCRIPTOR_SIZE_V5_MD5, FILE_DESCRIPTOR_SIZE_V6};
use crate::version::{Layout, Version};

/// Version word written by [`HeaderBuilder::v5`].
pub const VERSION_WORD_V5: u32 = 0x0100_5000;
/// Version word written by [`HeaderBuilder::v6`].
pub const VERSION_WORD_V6: u32 = 0x0100_6000;
/// Version word written by [`HeaderBuilder::is2003`] (major 17, unicode).
pub const VERSION_WORD_2003: u32 = 0x0200_06a4;

/// Absolute offset at which every builder places the cabinet descriptor.
pub const DESCRIPTOR_BASE: usize = COMMON_HEADER_SIZE;

/// A file entry to write into a synthetic header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SynthFile {
    /// The file's name.
    pub name: String,
    /// Its directory table index.
    pub directory_index: u32,
    /// Its storage flags.
    pub flags: u16,
    /// Expanded size in bytes.
    pub expanded_size: u64,
    /// Stored size in bytes.
    pub compressed_size: u64,
    /// Offset of the stored bytes within the volume.
    pub data_offset: u64,
    /// Recorded digest of the expanded bytes.
    pub md5: [u8; 16],
    /// Volume number of the first stored byte.
    pub volume: u16,
    /// Previous file index in a split link.
    pub link_previous: u32,
    /// Next file index in a split link.
    pub link_next: u32,
    /// Split-link flags.
    pub link_flags: u8,
}

impl SynthFile {
    /// A minimal, valid, uncompressed entry.
    #[must_use]
    pub fn new(name: &str, expanded_size: u64, data_offset: u64) -> Self {
        Self {
            name: name.to_string(),
            expanded_size,
            compressed_size: expanded_size,
            data_offset,
            volume: 1,
            ..Self::default()
        }
    }
}

/// A file group entry to write into a synthetic header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthGroup {
    /// The group's name.
    pub name: String,
    /// First file index.
    pub first_file: u32,
    /// Last file index.
    pub last_file: u32,
}

/// A component entry to write into a synthetic header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthComponent {
    /// The component's name.
    pub name: String,
    /// Names of the file groups it selects.
    pub file_group_names: Vec<String>,
}

/// Builds a synthetic cabinet header buffer.
#[derive(Debug, Clone)]
pub struct HeaderBuilder {
    version_word: u32,
    volume_info: u32,
    directories: Vec<String>,
    files: Vec<SynthFile>,
    groups: Vec<SynthGroup>,
    components: Vec<SynthComponent>,
}

impl HeaderBuilder {
    /// A builder writing the given raw version word.
    #[must_use]
    pub fn new(version_word: u32) -> Self {
        Self {
            version_word,
            volume_info: 0,
            directories: Vec::new(),
            files: Vec::new(),
            groups: Vec::new(),
            components: Vec::new(),
        }
    }

    /// An InstallShield 5 style header.
    #[must_use]
    pub fn v5() -> Self {
        Self::new(VERSION_WORD_V5)
    }

    /// An InstallShield 6 style header.
    #[must_use]
    pub fn v6() -> Self {
        Self::new(VERSION_WORD_V6)
    }

    /// An InstallShield 2003 style header, with UTF-16LE strings.
    #[must_use]
    pub fn is2003() -> Self {
        Self::new(VERSION_WORD_2003)
    }

    /// The decoded version this builder writes.
    #[must_use]
    pub fn version(&self) -> Version {
        Version::decode(self.version_word)
    }

    /// Adds a directory name.
    #[must_use]
    pub fn directory(mut self, name: &str) -> Self {
        self.directories.push(name.to_string());
        self
    }

    /// Adds a file entry.
    #[must_use]
    pub fn file(mut self, file: SynthFile) -> Self {
        self.files.push(file);
        self
    }

    /// Adds a file group.
    #[must_use]
    pub fn group(mut self, name: &str, first_file: u32, last_file: u32) -> Self {
        self.groups.push(SynthGroup {
            name: name.to_string(),
            first_file,
            last_file,
        });
        self
    }

    /// Adds a component selecting the named file groups.
    #[must_use]
    pub fn component(mut self, name: &str, file_group_names: &[&str]) -> Self {
        self.components.push(SynthComponent {
            name: name.to_string(),
            file_group_names: file_group_names.iter().map(|n| (*n).to_string()).collect(),
        });
        self
    }

    fn encode_string(&self, value: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        if self.version().is_unicode() {
            for unit in value.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes.extend_from_slice(&0u16.to_le_bytes());
        } else {
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    /// Serialises the header.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn build(&self) -> Vec<u8> {
        let version = self.version();
        let major = version.major();
        let descriptor_base = u32::try_from(MIN_CAB_DESCRIPTOR_SIZE).expect("fits in u32");

        // Region A: everything addressed relative to the cabinet descriptor.
        let mut region_a: Vec<u8> = Vec::new();
        let mut group_descriptor_offsets = Vec::new();
        for group in &self.groups {
            let name = self.encode_string(&group.name);
            let name_offset = append(&mut region_a, descriptor_base, &name);
            let skip = if major <= 5 { 0x48 } else { 0x12 };
            let mut blob = Vec::new();
            blob.extend_from_slice(&name_offset.to_le_bytes());
            blob.extend_from_slice(&vec![0u8; skip]);
            blob.extend_from_slice(&group.first_file.to_le_bytes());
            blob.extend_from_slice(&group.last_file.to_le_bytes());
            group_descriptor_offsets.push(append(&mut region_a, descriptor_base, &blob));
        }

        let mut component_descriptor_offsets = Vec::new();
        for component in &self.components {
            let name = self.encode_string(&component.name);
            let name_offset = append(&mut region_a, descriptor_base, &name);
            let mut group_name_offsets = Vec::new();
            for group_name in &component.file_group_names {
                let encoded = self.encode_string(group_name);
                group_name_offsets.push(append(&mut region_a, descriptor_base, &encoded));
            }
            let mut table = Vec::new();
            for offset in &group_name_offsets {
                table.extend_from_slice(&offset.to_le_bytes());
            }
            let table_offset = if table.is_empty() {
                0
            } else {
                append(&mut region_a, descriptor_base, &table)
            };
            let skip = if matches!(version.layout(), Layout::V5) {
                0x6c
            } else {
                0x6b
            };
            let mut blob = Vec::new();
            blob.extend_from_slice(&name_offset.to_le_bytes());
            blob.extend_from_slice(&vec![0u8; skip]);
            let count = u16::try_from(component.file_group_names.len()).expect("bounded by test");
            blob.extend_from_slice(&count.to_le_bytes());
            blob.extend_from_slice(&table_offset.to_le_bytes());
            component_descriptor_offsets.push(append(&mut region_a, descriptor_base, &blob));
        }

        let group_head =
            append_offset_list(&mut region_a, descriptor_base, &group_descriptor_offsets);
        let component_head = append_offset_list(
            &mut region_a,
            descriptor_base,
            &component_descriptor_offsets,
        );

        // Region B: everything addressed relative to the file table.
        let directory_count = u32::try_from(self.directories.len()).expect("bounded by test");
        let file_count = u32::try_from(self.files.len()).expect("bounded by test");
        let entry_count = self.directories.len() + self.files.len();
        let mut region_b: Vec<u8> = vec![0u8; entry_count * 4];
        let mut table_entries = vec![0u32; entry_count];

        for (index, directory) in self.directories.iter().enumerate() {
            let encoded = self.encode_string(directory);
            table_entries[index] = append(&mut region_b, 0, &encoded);
        }

        let mut name_offsets = Vec::new();
        for file in &self.files {
            let encoded = self.encode_string(&file.name);
            name_offsets.push(append(&mut region_b, 0, &encoded));
        }

        let mut file_table_offset2 = 0u32;
        match version.layout() {
            Layout::V5 => {
                let with_md5 = major == 5;
                for (index, file) in self.files.iter().enumerate() {
                    let mut blob = Vec::new();
                    blob.extend_from_slice(&name_offsets[index].to_le_bytes());
                    blob.extend_from_slice(&file.directory_index.to_le_bytes());
                    blob.extend_from_slice(&file.flags.to_le_bytes());
                    blob.extend_from_slice(&low32(file.expanded_size).to_le_bytes());
                    blob.extend_from_slice(&low32(file.compressed_size).to_le_bytes());
                    blob.extend_from_slice(&[0u8; 0x14]);
                    blob.extend_from_slice(&low32(file.data_offset).to_le_bytes());
                    if with_md5 {
                        blob.extend_from_slice(&file.md5);
                        debug_assert_eq!(blob.len(), FILE_DESCRIPTOR_SIZE_V5_MD5);
                    } else {
                        debug_assert_eq!(blob.len(), FILE_DESCRIPTOR_SIZE_V5);
                    }
                    table_entries[self.directories.len() + index] = append(&mut region_b, 0, &blob);
                }
            }
            Layout::V6 => {
                file_table_offset2 = u32::try_from(region_b.len()).expect("bounded by test");
                for (index, file) in self.files.iter().enumerate() {
                    let mut blob = Vec::new();
                    blob.extend_from_slice(&file.flags.to_le_bytes());
                    blob.extend_from_slice(&file.expanded_size.to_le_bytes());
                    blob.extend_from_slice(&file.compressed_size.to_le_bytes());
                    blob.extend_from_slice(&file.data_offset.to_le_bytes());
                    blob.extend_from_slice(&file.md5);
                    blob.extend_from_slice(&[0u8; 0x10]);
                    blob.extend_from_slice(&name_offsets[index].to_le_bytes());
                    blob.extend_from_slice(
                        &u16::try_from(file.directory_index)
                            .unwrap_or(u16::MAX)
                            .to_le_bytes(),
                    );
                    blob.extend_from_slice(&[0u8; 0x0c]);
                    blob.extend_from_slice(&file.link_previous.to_le_bytes());
                    blob.extend_from_slice(&file.link_next.to_le_bytes());
                    blob.push(file.link_flags);
                    blob.extend_from_slice(&file.volume.to_le_bytes());
                    debug_assert_eq!(blob.len(), FILE_DESCRIPTOR_SIZE_V6);
                    append(&mut region_b, 0, &blob);
                }
            }
        }

        for (index, entry) in table_entries.iter().enumerate() {
            region_b[index * 4..index * 4 + 4].copy_from_slice(&entry.to_le_bytes());
        }

        // Fixed descriptor area.
        let file_table_offset =
            descriptor_base + u32::try_from(region_a.len()).expect("bounded by test");
        let file_table_size = u32::try_from(region_b.len()).expect("bounded by test");
        let mut fixed = vec![0u8; MIN_CAB_DESCRIPTOR_SIZE];
        put32(&mut fixed, 0x0c, file_table_offset);
        put32(&mut fixed, 0x14, file_table_size);
        put32(&mut fixed, 0x18, file_table_size);
        put32(&mut fixed, 0x1c, directory_count);
        put32(&mut fixed, 0x28, file_count);
        put32(&mut fixed, 0x2c, file_table_offset2);
        if let Some(head) = group_head {
            put32(&mut fixed, 0x3e, head);
        }
        if let Some(head) = component_head {
            put32(&mut fixed, 0x15a, head);
        }

        let descriptor_size = fixed.len() + region_a.len() + region_b.len();
        let mut buffer = Vec::with_capacity(COMMON_HEADER_SIZE + descriptor_size);
        buffer.extend_from_slice(&CAB_SIGNATURE.to_le_bytes());
        buffer.extend_from_slice(&self.version_word.to_le_bytes());
        buffer.extend_from_slice(&self.volume_info.to_le_bytes());
        buffer.extend_from_slice(
            &u32::try_from(DESCRIPTOR_BASE)
                .expect("fits in u32")
                .to_le_bytes(),
        );
        buffer.extend_from_slice(
            &u32::try_from(descriptor_size)
                .expect("bounded by test")
                .to_le_bytes(),
        );
        buffer.extend_from_slice(&fixed);
        buffer.extend_from_slice(&region_a);
        buffer.extend_from_slice(&region_b);
        buffer
    }
}

fn low32(value: u64) -> u32 {
    u32::try_from(value & 0xffff_ffff).expect("masked")
}

fn append(buffer: &mut Vec<u8>, base: u32, bytes: &[u8]) -> u32 {
    let offset = base + u32::try_from(buffer.len()).expect("bounded by test");
    buffer.extend_from_slice(bytes);
    offset
}

/// Appends one chained offset list and returns its head offset.
fn append_offset_list(buffer: &mut Vec<u8>, base: u32, descriptors: &[u32]) -> Option<u32> {
    if descriptors.is_empty() {
        return None;
    }
    let head = base + u32::try_from(buffer.len()).expect("bounded by test");
    let node_size = 12u32;
    for (index, descriptor) in descriptors.iter().enumerate() {
        let next = if index + 1 == descriptors.len() {
            0
        } else {
            head + node_size * u32::try_from(index + 1).expect("bounded by test")
        };
        buffer.extend_from_slice(&0u32.to_le_bytes());
        buffer.extend_from_slice(&descriptor.to_le_bytes());
        buffer.extend_from_slice(&next.to_le_bytes());
    }
    Some(head)
}

/// Overwrites a little-endian `u32` at `offset`, for malformed-input tests.
pub fn put32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Overwrites a little-endian `u32` at `offset` within the cabinet
/// descriptor of a built header, for malformed-input tests.
pub fn put_descriptor32(buffer: &mut [u8], offset: usize, value: u32) {
    put32(buffer, DESCRIPTOR_BASE + offset, value);
}
