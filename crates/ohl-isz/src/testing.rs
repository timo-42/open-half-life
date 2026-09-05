//! Project-authored synthetic fixtures: a Z-archive writer and a minimal
//! PKWARE DCL implode encoder.
//!
//! Everything here is generated from invented names and invented bytes. No
//! byte, name, listing or layout from any real medium appears in this file or
//! in anything it produces. It exists so the decoder can be round-tripped and
//! fuzzed without a real archive; never enable `test-support` in a production
//! build.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::explode::{
    MAX_MATCH, MAX_WINDOW, distance_code, length_base, length_code, length_extra, literal_code,
};
use crate::header::{ArchiveHeader, DATA_START, HEADER_SIZE};
use crate::toc::{ATTRIBUTE_UNCOMPRESSED, DIRECTORY_RECORD_FIXED, ENTRY_RECORD_FIXED};

/// Writes bits least-significant bit first, as the format requires.
#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    buffer: u32,
    count: u32,
}

impl BitWriter {
    /// An empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes `bits` low bits of `value`, least-significant bit first.
    pub fn write_raw(&mut self, value: u32, bits: u32) {
        for index in 0..bits {
            self.write_bit((value >> index) & 1);
        }
    }

    /// Writes a canonical Huffman code: `bits` bits of `code`, most
    /// significant first, each inverted, which is how the decoder rebuilds
    /// the natural ordering.
    pub fn write_code(&mut self, code: u32, bits: u32) {
        for index in (0..bits).rev() {
            self.write_bit(((code >> index) & 1) ^ 1);
        }
    }

    fn write_bit(&mut self, bit: u32) {
        self.buffer |= (bit & 1) << self.count;
        self.count += 1;
        if self.count == 8 {
            self.bytes
                .push(u8::try_from(self.buffer & 0xff).unwrap_or(0));
            self.buffer = 0;
            self.count = 0;
        }
    }

    /// Flushes any partial byte with zero padding and returns the bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes
                .push(u8::try_from(self.buffer & 0xff).unwrap_or(0));
        }
        self.bytes
    }
}

/// How many earlier positions the fixture matcher considers per prefix.
const MAX_CHAIN: usize = 24;

/// The length symbol, and its extra-bit payload, for a match of `length`.
fn length_symbol_for(length: u32) -> Option<(u8, u32)> {
    for symbol in 0u8..16 {
        let base = u32::from(length_base(symbol));
        let extra = u32::from(length_extra(symbol));
        if length >= base && length - base < (1u32 << extra) {
            return Some((symbol, length - base));
        }
    }
    None
}

/// Encodes `input` as a PKWARE DCL imploded stream.
///
/// `dictionary_code` selects the window (4, 5 or 6 for 1, 2 or 4 KiB) and
/// `coded_literals` selects Huffman-coded rather than raw literals. The
/// matcher is deliberately simple: it is a fixture generator, not a
/// competitive compressor.
///
/// # Panics
///
/// Panics if `dictionary_code` is outside `4..=6`, which only a test can do.
#[must_use]
pub fn implode(input: &[u8], dictionary_code: u8, coded_literals: bool) -> Vec<u8> {
    assert!(
        (4..=6).contains(&dictionary_code),
        "dictionary code must be 4, 5 or 6"
    );
    let window = 1usize << (u32::from(dictionary_code) + 6);
    debug_assert!(window <= MAX_WINDOW);

    let mut writer = BitWriter::new();
    writer.write_raw(u32::from(coded_literals), 8);
    writer.write_raw(u32::from(dictionary_code), 8);

    // Chained positions of every three-byte prefix seen so far, newest first.
    let mut chains: BTreeMap<[u8; 3], Vec<usize>> = BTreeMap::new();

    let mut position = 0usize;
    while position < input.len() {
        let mut best_length = 0usize;
        let mut best_distance = 0usize;
        if position + 3 <= input.len() {
            let key = [input[position], input[position + 1], input[position + 2]];
            if let Some(candidates) = chains.get(&key) {
                for candidate in candidates.iter().take(MAX_CHAIN) {
                    let distance = position - candidate;
                    if distance == 0 || distance > window {
                        continue;
                    }
                    let limit = (input.len() - position).min(MAX_MATCH);
                    let mut length = 0usize;
                    while length < limit && input[candidate + length] == input[position + length] {
                        length += 1;
                    }
                    if length > best_length {
                        best_length = length;
                        best_distance = distance;
                    }
                }
            }
        }

        let emitted = if best_length >= 3 {
            emit_match(&mut writer, best_length, best_distance, dictionary_code);
            best_length
        } else {
            emit_literal(&mut writer, input[position], coded_literals);
            1
        };

        for step in 0..emitted {
            let at = position + step;
            if at + 3 <= input.len() {
                let key = [input[at], input[at + 1], input[at + 2]];
                let chain = chains.entry(key).or_default();
                chain.insert(0, at);
                chain.truncate(MAX_CHAIN);
            }
        }
        position += emitted;
    }

    // End code: the length symbol whose value is 519.
    let (symbol, extra_value) = length_symbol_for(519).expect("519 is the end code");
    writer.write_raw(1, 1);
    let (code, bits) = length_code(symbol).expect("length symbol is coded");
    writer.write_code(code, bits);
    writer.write_raw(extra_value, u32::from(length_extra(symbol)));
    writer.finish()
}

fn emit_literal(writer: &mut BitWriter, byte: u8, coded_literals: bool) {
    writer.write_raw(0, 1);
    if coded_literals {
        let (code, bits) = literal_code(byte).expect("every byte has a literal code");
        writer.write_code(code, bits);
    } else {
        writer.write_raw(u32::from(byte), 8);
    }
}

fn emit_match(writer: &mut BitWriter, length: usize, distance: usize, dictionary_code: u8) {
    let length = u32::try_from(length).unwrap_or(3);
    let (symbol, extra_value) = length_symbol_for(length).expect("match length is representable");
    writer.write_raw(1, 1);
    let (code, bits) = length_code(symbol).expect("length symbol is coded");
    writer.write_code(code, bits);
    writer.write_raw(extra_value, u32::from(length_extra(symbol)));

    let shift = if length == 2 {
        2
    } else {
        u32::from(dictionary_code)
    };
    let encoded = u32::try_from(distance - 1).unwrap_or(0);
    let distance_symbol = u8::try_from(encoded >> shift).expect("distance fits the window");
    let (code, bits) = distance_code(distance_symbol).expect("distance symbol is coded");
    writer.write_code(code, bits);
    writer.write_raw(encoded & ((1u32 << shift) - 1), shift);
}

/// One synthetic entry.
#[derive(Debug, Clone)]
pub struct EntrySpec {
    /// The entry's invented name.
    pub name: Vec<u8>,
    /// The entry's invented expanded bytes.
    pub data: Vec<u8>,
    /// Whether to store the bytes verbatim rather than implode them.
    pub stored: bool,
    /// Extra padding appended to the entry's table-of-contents record.
    pub record_padding: u16,
}

/// One synthetic directory and the entries it holds.
#[derive(Debug, Clone, Default)]
pub struct DirectorySpec {
    /// The directory's invented name; empty means the archive root.
    pub name: Vec<u8>,
    /// Entries in this directory.
    pub entries: Vec<EntrySpec>,
    /// Extra padding appended to the directory's record.
    pub record_padding: u16,
}

/// A built synthetic archive and the offsets a test needs to corrupt it.
#[derive(Debug, Clone, Default)]
pub struct SyntheticArchive {
    /// The archive bytes.
    pub bytes: Vec<u8>,
    /// Offset of the table of contents within [`Self::bytes`].
    pub toc_offset: usize,
    /// Offset of each directory record within [`Self::bytes`].
    pub directory_records: Vec<usize>,
    /// Offset of each entry record within [`Self::bytes`].
    pub entry_records: Vec<usize>,
}

/// Builds synthetic InstallShield 3 archives from invented data.
#[derive(Debug, Clone, Default)]
pub struct ArchiveBuilder {
    directories: Vec<DirectorySpec>,
    dictionary_code: u8,
    coded_literals: bool,
    multi_volume: u16,
}

impl ArchiveBuilder {
    /// An empty builder using a 4 KiB dictionary and raw literals.
    #[must_use]
    pub fn new() -> Self {
        Self {
            directories: Vec::new(),
            dictionary_code: 6,
            coded_literals: false,
            multi_volume: 0,
        }
    }

    /// Selects the dictionary code (4, 5 or 6) used when imploding.
    #[must_use]
    pub const fn dictionary_code(mut self, code: u8) -> Self {
        self.dictionary_code = code;
        self
    }

    /// Selects Huffman-coded literals when imploding.
    #[must_use]
    pub const fn coded_literals(mut self, coded: bool) -> Self {
        self.coded_literals = coded;
        self
    }

    /// Sets the header's multi-volume marker.
    #[must_use]
    pub const fn multi_volume(mut self, marker: u16) -> Self {
        self.multi_volume = marker;
        self
    }

    /// Appends a directory and returns its index.
    pub fn directory(&mut self, name: &[u8]) -> usize {
        self.directories.push(DirectorySpec {
            name: Vec::from(name),
            entries: Vec::new(),
            record_padding: 0,
        });
        self.directories.len() - 1
    }

    /// Appends an entry to directory `directory`.
    ///
    /// # Panics
    ///
    /// Panics when `directory` is not a directory index this builder issued.
    pub fn entry(&mut self, directory: usize, name: &[u8], data: &[u8], stored: bool) {
        self.directories
            .get_mut(directory)
            .expect("directory index came from `directory`")
            .entries
            .push(EntrySpec {
                name: Vec::from(name),
                data: Vec::from(data),
                stored,
                record_padding: 0,
            });
    }

    /// Serializes the archive.
    ///
    /// # Panics
    ///
    /// Panics when the invented fixture does not fit the format's 32-bit
    /// fields, which only a test can arrange.
    #[must_use]
    pub fn build(&self) -> SyntheticArchive {
        let mut bytes = vec![0u8; usize::try_from(DATA_START).expect("data start fits usize")];
        let mut stored_entries: Vec<(u32, u32, u32)> = Vec::new();
        let mut expanded_total = 0u64;

        for directory in &self.directories {
            for entry in &directory.entries {
                let offset = u32::try_from(bytes.len()).expect("fixture fits a u32 offset");
                let payload = if entry.stored {
                    entry.data.clone()
                } else {
                    implode(&entry.data, self.dictionary_code, self.coded_literals)
                };
                let stored_size = u32::try_from(payload.len()).expect("fixture fits a u32 size");
                let expanded = u32::try_from(entry.data.len()).expect("fixture fits a u32 size");
                bytes.extend_from_slice(&payload);
                expanded_total += u64::from(expanded);
                stored_entries.push((offset, stored_size, expanded));
            }
        }

        let toc_offset = bytes.len();
        let mut directory_records = Vec::new();
        for directory in &self.directories {
            directory_records.push(bytes.len());
            let name_len = u16::try_from(directory.name.len()).expect("fixture name fits a u16");
            let record_size = u16::try_from(DIRECTORY_RECORD_FIXED)
                .expect("constant fits")
                .checked_add(name_len)
                .and_then(|size| size.checked_add(directory.record_padding))
                .expect("fixture record fits a u16");
            let entry_count =
                u16::try_from(directory.entries.len()).expect("fixture fits a u16 count");
            bytes.extend_from_slice(&entry_count.to_le_bytes());
            bytes.extend_from_slice(&record_size.to_le_bytes());
            bytes.extend_from_slice(&name_len.to_le_bytes());
            bytes.extend_from_slice(&directory.name);
            bytes.extend(core::iter::repeat_n(0u8, directory.record_padding as usize));
        }

        let mut entry_records = Vec::new();
        let mut flat = 0usize;
        for directory in &self.directories {
            for entry in &directory.entries {
                entry_records.push(bytes.len());
                let (offset, stored_size, expanded) = stored_entries[flat];
                let name_len = u8::try_from(entry.name.len()).expect("fixture name fits a u8");
                let record_size = u16::try_from(ENTRY_RECORD_FIXED)
                    .expect("constant fits")
                    .checked_add(u16::from(name_len))
                    .and_then(|size| size.checked_add(entry.record_padding))
                    .expect("fixture record fits a u16");
                let attributes = if entry.stored {
                    ATTRIBUTE_UNCOMPRESSED
                } else {
                    0x20
                };
                bytes.push(1); // last volume holding this entry
                bytes.extend_from_slice(&u16::try_from(flat).unwrap_or(0).to_le_bytes());
                bytes.extend_from_slice(&expanded.to_le_bytes());
                bytes.extend_from_slice(&stored_size.to_le_bytes());
                bytes.extend_from_slice(&offset.to_le_bytes());
                bytes.extend_from_slice(&0x2a2a_2a2au32.to_le_bytes()); // date/time
                bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved
                bytes.extend_from_slice(&record_size.to_le_bytes());
                bytes.push(attributes);
                bytes.push(0); // not split
                bytes.push(0); // reserved
                bytes.push(1); // first volume holding this entry
                bytes.push(name_len);
                bytes.extend_from_slice(&entry.name);
                bytes.extend(core::iter::repeat_n(0u8, entry.record_padding as usize));
                flat += 1;
            }
        }

        let header = ArchiveHeader {
            entry_count: u16::try_from(stored_entries.len()).expect("fixture fits a u16 count"),
            directory_count: u16::try_from(self.directories.len())
                .expect("fixture fits a u16 count"),
            archive_size: u32::try_from(bytes.len()).expect("fixture fits a u32 size"),
            expanded_size: u32::try_from(expanded_total).unwrap_or(u32::MAX),
            datetime: 0x2a2a_2a2a,
            multi_volume: self.multi_volume,
            volume_count: 1,
            volume_number: 1,
            split_begin: 0,
            split_end: 0,
            toc_offset: u32::try_from(toc_offset).expect("fixture fits a u32 offset"),
        };
        bytes[..HEADER_SIZE].copy_from_slice(&header.encode());

        SyntheticArchive {
            bytes,
            toc_offset,
            directory_records,
            entry_records,
        }
    }
}

/// A small, representative archive: one root directory and one subdirectory,
/// stored and imploded entries, all names and bytes invented.
#[must_use]
pub fn sample_archive() -> SyntheticArchive {
    let mut builder = ArchiveBuilder::new();
    let root = builder.directory(b"");
    let sub = builder.directory(b"MAPS");
    builder.entry(root, b"NOTES.TXT", b"orange crate notes, invented\n", false);
    builder.entry(root, b"RAW.BIN", &[0xa5u8; 300], true);
    builder.entry(
        sub,
        b"ALPHA.BSP",
        b"repeat repeat repeat repeat repeat repeat repeat".as_slice(),
        false,
    );
    builder.build()
}
