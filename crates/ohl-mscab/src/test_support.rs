//! A synthetic cabinet **writer**, used only by this crate's tests and fuzz
//! targets.
//!
//! Every byte a test sees comes from this writer. No cabinet, name, layout, or
//! payload from any real medium is committed to the repository. The writer
//! follows the same public documentation as the reader (see
//! `docs/FORMAT_SOURCES.md`); it is deliberately independent code so that a
//! shared mistake in a helper cannot hide a reader bug.

// Test-only fixture code: sizes are all small, invented constants, so the
// `as` conversions below cannot lose information.
#![allow(clippy::cast_possible_truncation)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::cabinet::{
    FLAG_NEXT_CABINET, FLAG_PREV_CABINET, FLAG_RESERVE_PRESENT, IFOLD_CONTINUED_FROM_PREV,
    IFOLD_CONTINUED_PREV_AND_NEXT, IFOLD_CONTINUED_TO_NEXT, SIGNATURE,
};
use crate::data::checksum;
use crate::mszip::SIGNATURE as MSZIP_SIGNATURE;

/// The compression a synthetic folder is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `tcompTYPE_NONE`.
    Stored,
    /// `tcompTYPE_MSZIP`, one independent DEFLATE stream per block.
    MsZip,
    /// An arbitrary raw `typeCompress`, for rejection tests.
    Raw(u16),
}

impl Method {
    fn type_compress(self) -> u16 {
        match self {
            Self::Stored => 0,
            Self::MsZip => 1,
            Self::Raw(raw) => raw,
        }
    }
}

/// How a file's `iFolder` is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Continuation {
    /// An ordinary folder index.
    #[default]
    None,
    /// `ifoldCONTINUED_FROM_PREV`.
    FromPrevious,
    /// `ifoldCONTINUED_TO_NEXT`.
    ToNext,
    /// `ifoldCONTINUED_PREV_AND_NEXT`.
    PreviousAndNext,
}

/// One synthetic file.
#[derive(Debug, Clone)]
pub struct FileSpec {
    /// `szName`, an invented name.
    pub name: Vec<u8>,
    /// The bytes that appear in the folder stream for this file.
    pub data: Vec<u8>,
    /// `attribs`.
    pub attributes: u16,
    /// `date`.
    pub date: u16,
    /// `time`.
    pub time: u16,
    /// How `iFolder` is written.
    pub continuation: Continuation,
    /// When set, `uoffFolderStart` is written as this value instead of the
    /// natural running offset (used by "file data is not present" tests).
    pub folder_offset_override: Option<u32>,
}

impl FileSpec {
    /// A file with the given invented name and contents.
    #[must_use]
    pub fn new(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            data,
            attributes: crate::ATTR_ARCHIVE,
            date: 0x2F1A,
            time: 0x4A21,
            continuation: Continuation::None,
            folder_offset_override: None,
        }
    }
}

/// A block written verbatim, bypassing the default encoder.
#[derive(Debug, Clone)]
pub struct BlockSpec {
    /// `CFDATA.ab`.
    pub compressed: Vec<u8>,
    /// `CFDATA.cbUncomp`.
    pub uncompressed_len: u16,
}

/// One synthetic folder.
#[derive(Debug, Clone)]
pub struct FolderSpec {
    /// Compression method written to `typeCompress`.
    pub method: Method,
    /// Uncompressed bytes per `CFDATA` block.
    pub block_size: usize,
    /// The files whose bytes make up the folder stream, in order.
    pub files: Vec<FileSpec>,
    /// When set, these blocks are written instead of encoding `files`.
    pub blocks: Option<Vec<BlockSpec>>,
}

impl FolderSpec {
    /// A folder holding `files`, encoded with `method`.
    #[must_use]
    pub fn new(method: Method, files: Vec<FileSpec>) -> Self {
        Self {
            method,
            block_size: 32_768,
            files,
            blocks: None,
        }
    }
}

/// A whole synthetic cabinet.
#[derive(Debug, Clone)]
pub struct CabinetSpec {
    /// The folders, in order.
    pub folders: Vec<FolderSpec>,
    /// `cbCFHeader` bytes of per-cabinet reserved area.
    pub header_reserve: u16,
    /// `cbCFFolder` bytes of per-folder reserved area.
    pub folder_reserve: u8,
    /// `cbCFData` bytes of per-block reserved area.
    pub data_reserve: u8,
    /// `setID`.
    pub set_id: u16,
    /// `iCabinet`.
    pub cabinet_index: u16,
    /// Invented `szCabinetPrev`/`szDiskPrev` names.
    pub previous_names: Option<(Vec<u8>, Vec<u8>)>,
    /// Invented `szCabinetNext`/`szDiskNext` names.
    pub next_names: Option<(Vec<u8>, Vec<u8>)>,
    /// Whether to write real `CFDATA.csum` values (`false` writes zero, the
    /// documented "not supplied" value).
    pub write_checksums: bool,
}

impl CabinetSpec {
    /// A cabinet with the given folders and no optional areas.
    #[must_use]
    pub fn new(folders: Vec<FolderSpec>) -> Self {
        Self {
            folders,
            header_reserve: 0,
            folder_reserve: 0,
            data_reserve: 0,
            set_id: 0x4F48,
            cabinet_index: 0,
            previous_names: None,
            next_names: None,
            write_checksums: true,
        }
    }
}

/// A written cabinet plus the offsets a test needs in order to corrupt one
/// specific field.
#[derive(Debug, Clone)]
pub struct BuiltCabinet {
    /// The cabinet bytes.
    pub bytes: Vec<u8>,
    /// Offset of each `CFFOLDER`.
    pub folder_offsets: Vec<usize>,
    /// Offset of each `CFFILE`.
    pub file_offsets: Vec<usize>,
    /// Offset of each `CFDATA`, per folder.
    pub data_offsets: Vec<Vec<usize>>,
    /// The uncompressed stream of each folder.
    pub folder_streams: Vec<Vec<u8>>,
}

/// Encodes one MSZIP block: the `CK` signature followed by a single DEFLATE
/// stream, as [MS-MCI] requires.
#[must_use]
pub fn mszip_block(chunk: &[u8]) -> Vec<u8> {
    let mut block = MSZIP_SIGNATURE.to_vec();
    block.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(chunk, 6));
    block
}

/// Builds a cabinet from `spec`.
///
/// # Panics
///
/// Panics if the specification cannot be represented (for example a block size
/// above 32,768 or more folders than `cFolders` can hold). Test-only code, so
/// a panic is the right failure mode.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build(spec: &CabinetSpec) -> BuiltCabinet {
    assert!(!spec.folders.is_empty(), "a cabinet needs a folder");

    let mut folder_streams = Vec::new();
    let mut folder_blocks = Vec::new();
    for folder in &spec.folders {
        assert!(
            folder.block_size > 0 && folder.block_size <= 32_768,
            "block size must be within the documented maximum"
        );
        let mut stream = Vec::new();
        for file in &folder.files {
            stream.extend_from_slice(&file.data);
        }
        let blocks = match &folder.blocks {
            Some(blocks) => blocks.clone(),
            None => encode_blocks(folder, &stream),
        };
        folder_streams.push(stream);
        folder_blocks.push(blocks);
    }

    let mut header_len = 36usize;
    if spec.header_reserve != 0 || spec.folder_reserve != 0 || spec.data_reserve != 0 {
        header_len += 4 + usize::from(spec.header_reserve);
    }
    if let Some((cabinet, disk)) = &spec.previous_names {
        header_len += cabinet.len() + 1 + disk.len() + 1;
    }
    if let Some((cabinet, disk)) = &spec.next_names {
        header_len += cabinet.len() + 1 + disk.len() + 1;
    }

    let folder_entry_len = 8 + usize::from(spec.folder_reserve);
    let folder_area = folder_entry_len * spec.folders.len();
    let first_file_offset = header_len + folder_area;

    let mut file_area = 0usize;
    for folder in &spec.folders {
        for file in &folder.files {
            file_area += 16 + file.name.len() + 1;
        }
    }

    let mut data_offsets = Vec::new();
    let mut folder_data_offsets = Vec::new();
    let mut cursor = first_file_offset + file_area;
    for blocks in &folder_blocks {
        folder_data_offsets.push(cursor);
        let mut offsets = Vec::new();
        for block in blocks {
            offsets.push(cursor);
            cursor += 8 + usize::from(spec.data_reserve) + block.compressed.len();
        }
        data_offsets.push(offsets);
    }
    let total_len = cursor;

    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(&SIGNATURE);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(total_len as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&(first_file_offset as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.push(3); // versionMinor
    bytes.push(1); // versionMajor
    bytes.extend_from_slice(&(spec.folders.len() as u16).to_le_bytes());
    let file_count: usize = spec.folders.iter().map(|folder| folder.files.len()).sum();
    bytes.extend_from_slice(&(file_count as u16).to_le_bytes());

    let mut flags = 0u16;
    if spec.header_reserve != 0 || spec.folder_reserve != 0 || spec.data_reserve != 0 {
        flags |= FLAG_RESERVE_PRESENT;
    }
    if spec.previous_names.is_some() {
        flags |= FLAG_PREV_CABINET;
    }
    if spec.next_names.is_some() {
        flags |= FLAG_NEXT_CABINET;
    }
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&spec.set_id.to_le_bytes());
    bytes.extend_from_slice(&spec.cabinet_index.to_le_bytes());
    if flags & FLAG_RESERVE_PRESENT != 0 {
        bytes.extend_from_slice(&spec.header_reserve.to_le_bytes());
        bytes.push(spec.folder_reserve);
        bytes.push(spec.data_reserve);
        bytes.extend(core::iter::repeat_n(
            0xA5u8,
            usize::from(spec.header_reserve),
        ));
    }
    if let Some((cabinet, disk)) = &spec.previous_names {
        bytes.extend_from_slice(cabinet);
        bytes.push(0);
        bytes.extend_from_slice(disk);
        bytes.push(0);
    }
    if let Some((cabinet, disk)) = &spec.next_names {
        bytes.extend_from_slice(cabinet);
        bytes.push(0);
        bytes.extend_from_slice(disk);
        bytes.push(0);
    }
    assert_eq!(bytes.len(), header_len);

    let mut folder_offsets = Vec::new();
    for (index, folder) in spec.folders.iter().enumerate() {
        folder_offsets.push(bytes.len());
        bytes.extend_from_slice(&(folder_data_offsets[index] as u32).to_le_bytes());
        bytes.extend_from_slice(&(folder_blocks[index].len() as u16).to_le_bytes());
        bytes.extend_from_slice(&folder.method.type_compress().to_le_bytes());
        bytes.extend(core::iter::repeat_n(
            0x5Au8,
            usize::from(spec.folder_reserve),
        ));
    }
    assert_eq!(bytes.len(), first_file_offset);

    let mut file_offsets = Vec::new();
    for (index, folder) in spec.folders.iter().enumerate() {
        let mut running = 0u32;
        for file in &folder.files {
            file_offsets.push(bytes.len());
            bytes.extend_from_slice(&(file.data.len() as u32).to_le_bytes());
            let folder_offset = file.folder_offset_override.unwrap_or(running);
            bytes.extend_from_slice(&folder_offset.to_le_bytes());
            let i_folder = match file.continuation {
                Continuation::None => index as u16,
                Continuation::FromPrevious => IFOLD_CONTINUED_FROM_PREV,
                Continuation::ToNext => IFOLD_CONTINUED_TO_NEXT,
                Continuation::PreviousAndNext => IFOLD_CONTINUED_PREV_AND_NEXT,
            };
            bytes.extend_from_slice(&i_folder.to_le_bytes());
            bytes.extend_from_slice(&file.date.to_le_bytes());
            bytes.extend_from_slice(&file.time.to_le_bytes());
            bytes.extend_from_slice(&file.attributes.to_le_bytes());
            bytes.extend_from_slice(&file.name);
            bytes.push(0);
            running += file.data.len() as u32;
        }
    }

    for blocks in &folder_blocks {
        for block in blocks {
            let reserve = vec![0x3Cu8; usize::from(spec.data_reserve)];
            let mut fields = [0u8; 4];
            fields[..2].copy_from_slice(&(block.compressed.len() as u16).to_le_bytes());
            fields[2..].copy_from_slice(&block.uncompressed_len.to_le_bytes());
            let csum = if spec.write_checksums {
                checksum(&fields, checksum(&block.compressed, 0))
            } else {
                0
            };
            bytes.extend_from_slice(&csum.to_le_bytes());
            bytes.extend_from_slice(&(block.compressed.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&block.uncompressed_len.to_le_bytes());
            bytes.extend_from_slice(&reserve);
            bytes.extend_from_slice(&block.compressed);
        }
    }
    assert_eq!(bytes.len(), total_len);

    BuiltCabinet {
        bytes,
        folder_offsets,
        file_offsets,
        data_offsets,
        folder_streams,
    }
}

fn encode_blocks(folder: &FolderSpec, stream: &[u8]) -> Vec<BlockSpec> {
    let mut blocks = Vec::new();
    if stream.is_empty() {
        return blocks;
    }
    for chunk in stream.chunks(folder.block_size) {
        let compressed = match folder.method {
            Method::Stored | Method::Raw(_) => chunk.to_vec(),
            Method::MsZip => mszip_block(chunk),
        };
        blocks.push(BlockSpec {
            compressed,
            uncompressed_len: chunk.len() as u16,
        });
    }
    blocks
}

/// Deterministic pseudo-random but compressible bytes, so synthetic payloads
/// exercise real DEFLATE matches without embedding any real content.
#[must_use]
pub fn filler(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let byte = (state >> 16) as u8;
        let run = usize::from(byte % 7) + 1;
        for _ in 0..run {
            if out.len() == len {
                break;
            }
            out.push(byte);
        }
    }
    out
}
