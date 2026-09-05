//! `CFHEADER`, `CFFOLDER` and `CFFILE` parsing.
//!
//! Field names, offsets and semantics come from [MS-CAB], "Microsoft Cabinet
//! Format" (see `docs/FORMAT_SOURCES.md`). Nothing here trusts a field before
//! it has been checked against [`Limits`] and against the pinned source.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::bytes::Reader;
use crate::error::{CabError, Result};
use crate::limits::Limits;
use crate::source::VolumeSource;

/// `CFHEADER.signature`: the ASCII bytes `MSCF`.
pub const SIGNATURE: [u8; 4] = *b"MSCF";

/// The size in bytes of the fixed part of a `CFHEADER`.
const HEADER_FIXED_LEN: usize = 36;
/// The size in bytes of a `CFFOLDER` without its per-folder reserved area.
const FOLDER_FIXED_LEN: usize = 8;
/// The size in bytes of a `CFFILE` without its `szName`.
const FILE_FIXED_LEN: usize = 16;

/// `cfhdrPREV_CABINET`: `szCabinetPrev`/`szDiskPrev` are present.
pub const FLAG_PREV_CABINET: u16 = 0x0001;
/// `cfhdrNEXT_CABINET`: `szCabinetNext`/`szDiskNext` are present.
pub const FLAG_NEXT_CABINET: u16 = 0x0002;
/// `cfhdrRESERVE_PRESENT`: the three reserved-area size fields are present.
pub const FLAG_RESERVE_PRESENT: u16 = 0x0004;
const FLAGS_KNOWN: u16 = FLAG_PREV_CABINET | FLAG_NEXT_CABINET | FLAG_RESERVE_PRESENT;

/// `ifoldCONTINUED_FROM_PREV`.
pub const IFOLD_CONTINUED_FROM_PREV: u16 = 0xFFFD;
/// `ifoldCONTINUED_TO_NEXT`.
pub const IFOLD_CONTINUED_TO_NEXT: u16 = 0xFFFE;
/// `ifoldCONTINUED_PREV_AND_NEXT`.
pub const IFOLD_CONTINUED_PREV_AND_NEXT: u16 = 0xFFFF;

/// `_A_RDONLY`.
pub const ATTR_READ_ONLY: u16 = 0x01;
/// `_A_HIDDEN`.
pub const ATTR_HIDDEN: u16 = 0x02;
/// `_A_SYSTEM`.
pub const ATTR_SYSTEM: u16 = 0x04;
/// `_A_ARCH`.
pub const ATTR_ARCHIVE: u16 = 0x20;
/// `_A_EXEC`: "run after extraction". This crate never acts on it.
pub const ATTR_EXEC: u16 = 0x40;
/// `_A_NAME_IS_UTF`: `szName` holds UTF-8 rather than an OEM code page.
pub const ATTR_NAME_IS_UTF: u16 = 0x80;

/// The compression method a folder's `CFDATA` blocks use.
///
/// [MS-CAB] documents `CFFOLDER.typeCompress` as "the valid values are defined
/// in each compression format's specification" and does not restate the
/// numeric codes, so this decoder reads the low nibble as the method code and,
/// for LZX, bits 8..=12 as the window size in bits. That split is what the
/// public `TCOMPfromLZXWindow` documentation implies (an LZX type combined
/// with a window value in the range 15..=21) and every value outside the
/// documented ranges is rejected rather than guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compression {
    /// `tcompTYPE_NONE`: `CFDATA.ab` is the uncompressed data.
    None,
    /// `tcompTYPE_MSZIP`: each block is an MSZIP block ([MS-MCI]).
    MsZip,
    /// Quantum. Out of scope for this crate; extraction reports
    /// [`CabError::Unsupported`].
    Quantum {
        /// The raw `typeCompress` value, retained for diagnostics only.
        raw: u16,
    },
    /// LZX with the given window size in bits (15..=21 for cabinets).
    Lzx {
        /// `log2` of the sliding window size in bytes.
        window_bits: u8,
    },
    /// A method code this crate does not know.
    Unknown {
        /// The raw `typeCompress` value.
        raw: u16,
    },
}

impl Compression {
    const TYPE_MASK: u16 = 0x000F;
    const LZX_WINDOW_MASK: u16 = 0x1F00;
    const LZX_WINDOW_SHIFT: u32 = 8;

    /// Interprets a raw `CFFOLDER.typeCompress` value.
    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        match raw & Self::TYPE_MASK {
            0 => Self::None,
            1 => Self::MsZip,
            2 => Self::Quantum { raw },
            3 => Self::Lzx {
                window_bits: ((raw & Self::LZX_WINDOW_MASK) >> Self::LZX_WINDOW_SHIFT) as u8,
            },
            _ => Self::Unknown { raw },
        }
    }
}

/// A parsed `CFHEADER`.
///
/// The four optional cabinet/disk name strings are deliberately **not**
/// exposed by content: a cabinet set's continuation is resolved by the caller
/// through volume indices, so only the presence and byte length of each string
/// is reported. That keeps media-derived text out of this crate's API and out
/// of anything that formats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Header {
    /// `cbCabinet`: the total size of this cabinet in bytes.
    pub cabinet_bytes: u32,
    /// `coffFiles`: the offset of the first `CFFILE`.
    pub first_file_offset: u32,
    /// `versionMajor`.
    pub version_major: u8,
    /// `versionMinor`.
    pub version_minor: u8,
    /// `cFolders`.
    pub folder_count: u16,
    /// `cFiles`.
    pub file_count: u16,
    /// `flags`.
    pub flags: u16,
    /// `setID`: identical across every cabinet of a set.
    pub set_id: u16,
    /// `iCabinet`: this cabinet's zero-based index within its set.
    pub cabinet_index: u16,
    /// `cbCFHeader`: bytes of per-cabinet reserved area (0 when absent).
    pub header_reserve_bytes: u16,
    /// `cbCFFolder`: bytes of per-folder reserved area (0 when absent).
    pub folder_reserve_bytes: u8,
    /// `cbCFData`: bytes of per-block reserved area (0 when absent).
    pub data_reserve_bytes: u8,
    /// Byte length of `szCabinetPrev`, if present.
    pub previous_cabinet_name_len: Option<usize>,
    /// Byte length of `szDiskPrev`, if present.
    pub previous_disk_name_len: Option<usize>,
    /// Byte length of `szCabinetNext`, if present.
    pub next_cabinet_name_len: Option<usize>,
    /// Byte length of `szDiskNext`, if present.
    pub next_disk_name_len: Option<usize>,
}

impl Header {
    /// Whether `cfhdrPREV_CABINET` is set.
    #[must_use]
    pub const fn has_previous_cabinet(&self) -> bool {
        self.flags & FLAG_PREV_CABINET != 0
    }

    /// Whether `cfhdrNEXT_CABINET` is set.
    #[must_use]
    pub const fn has_next_cabinet(&self) -> bool {
        self.flags & FLAG_NEXT_CABINET != 0
    }

    /// Whether `cfhdrRESERVE_PRESENT` is set.
    #[must_use]
    pub const fn has_reserved_areas(&self) -> bool {
        self.flags & FLAG_RESERVE_PRESENT != 0
    }
}

/// A parsed `CFFOLDER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Folder {
    /// `coffCabStart`: offset of this folder's first `CFDATA`.
    pub first_data_offset: u32,
    /// `cCFData`: the number of `CFDATA` blocks in this cabinet.
    pub data_block_count: u16,
    /// `typeCompress`, interpreted.
    pub compression: Compression,
}

/// How a `CFFILE` refers to its folder, including the documented
/// continuation values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRef {
    /// An ordinary index into this cabinet's folder table.
    Index(u16),
    /// `ifoldCONTINUED_FROM_PREV`: folder 0, starting in the previous cabinet.
    ContinuedFromPrevious,
    /// `ifoldCONTINUED_TO_NEXT`: the last folder, continuing into the next
    /// cabinet.
    ContinuedToNext,
    /// `ifoldCONTINUED_PREV_AND_NEXT`: folder 0 of a cabinet whose folder
    /// spans both neighbours.
    ContinuedPreviousAndNext,
}

impl FolderRef {
    const fn from_raw(raw: u16) -> Self {
        match raw {
            IFOLD_CONTINUED_FROM_PREV => Self::ContinuedFromPrevious,
            IFOLD_CONTINUED_TO_NEXT => Self::ContinuedToNext,
            IFOLD_CONTINUED_PREV_AND_NEXT => Self::ContinuedPreviousAndNext,
            index => Self::Index(index),
        }
    }

    /// The folder index within *this* cabinet, per [MS-CAB]: the
    /// continued-from-previous values mean folder 0 and the
    /// continued-to-next values mean `folder_count - 1`.
    #[must_use]
    pub const fn index_in(self, folder_count: u16) -> Option<u16> {
        match self {
            Self::Index(index) if index < folder_count => Some(index),
            Self::Index(_) => None,
            Self::ContinuedFromPrevious | Self::ContinuedPreviousAndNext => {
                if folder_count > 0 {
                    Some(0)
                } else {
                    None
                }
            }
            Self::ContinuedToNext => {
                if folder_count > 0 {
                    Some(folder_count - 1)
                } else {
                    None
                }
            }
        }
    }

    /// Whether the file's data begins in a preceding cabinet of the set.
    #[must_use]
    pub const fn continues_from_previous(self) -> bool {
        matches!(
            self,
            Self::ContinuedFromPrevious | Self::ContinuedPreviousAndNext
        )
    }

    /// Whether the file's data runs on into the next cabinet of the set.
    #[must_use]
    pub const fn continues_to_next(self) -> bool {
        matches!(self, Self::ContinuedToNext | Self::ContinuedPreviousAndNext)
    }
}

/// A decoded MS-DOS date/time stamp (`CFFILE.date` and `CFFILE.time`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    /// Full year, `1980 + (date >> 9)`.
    pub year: u16,
    /// Month, 1..=12 as stored (not validated).
    pub month: u8,
    /// Day, 1..=31 as stored (not validated).
    pub day: u8,
    /// Hour, 0..=23 as stored.
    pub hour: u8,
    /// Minute, 0..=59 as stored.
    pub minute: u8,
    /// Second, `2 * (time & 0x1F)`.
    pub second: u8,
}

impl DateTime {
    /// Decodes the packed `date`/`time` pair documented by [MS-CAB]:
    /// `date = ((year-1980) << 9) + (month << 5) + day` and
    /// `time = (hour << 11) + (minute << 5) + (seconds/2)`.
    #[must_use]
    pub const fn decode(date: u16, time: u16) -> Self {
        // Every extracted field is at most 6 bits wide, so its little-endian
        // low byte is the whole value; taking it avoids a lossy cast.
        Self {
            year: 1980 + (date >> 9),
            month: ((date >> 5) & 0x0F).to_le_bytes()[0],
            day: (date & 0x1F).to_le_bytes()[0],
            hour: (time >> 11).to_le_bytes()[0],
            minute: ((time >> 5) & 0x3F).to_le_bytes()[0],
            second: ((time & 0x1F) * 2).to_le_bytes()[0],
        }
    }
}

/// A parsed `CFFILE`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileEntry {
    /// `cbFile`: the uncompressed size of the file.
    pub uncompressed_bytes: u32,
    /// `uoffFolderStart`: the file's offset within the folder's uncompressed
    /// byte stream.
    pub folder_offset: u32,
    /// `iFolder`, interpreted.
    pub folder: FolderRef,
    /// `date`.
    pub date: u16,
    /// `time`.
    pub time: u16,
    /// `attribs`.
    pub attributes: u16,
    name: Vec<u8>,
}

impl FileEntry {
    /// The raw `szName` bytes, without the terminating NUL.
    ///
    /// The bytes are cabinet-derived and untrusted: they are neither
    /// normalised nor interpreted as a path here. Callers must run them
    /// through their own path policy before touching a filesystem.
    #[must_use]
    pub fn name_bytes(&self) -> &[u8] {
        &self.name
    }

    /// Whether `_A_NAME_IS_UTF` is set, meaning `szName` is UTF-8.
    #[must_use]
    pub const fn name_is_utf8(&self) -> bool {
        self.attributes & ATTR_NAME_IS_UTF != 0
    }

    /// The name as UTF-8, or `None` when `_A_NAME_IS_UTF` is clear or the
    /// bytes are not valid UTF-8. Non-UTF-8 names are left to the caller,
    /// which owns the code-page policy.
    #[must_use]
    pub fn name_utf8(&self) -> Option<&str> {
        if self.name_is_utf8() {
            core::str::from_utf8(&self.name).ok()
        } else {
            None
        }
    }

    /// The decoded MS-DOS timestamp.
    #[must_use]
    pub const fn date_time(&self) -> DateTime {
        DateTime::decode(self.date, self.time)
    }
}

/// A parsed cabinet: its header, folder table and file table.
///
/// Parsing reads only the tables; no `CFDATA` block is touched until
/// extraction, so a cabinet with a hostile data area is still safely
/// enumerable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cabinet {
    header: Header,
    folders: Vec<Folder>,
    files: Vec<FileEntry>,
}

impl Cabinet {
    /// Parses the cabinet that starts at offset 0 of `volume`.
    pub fn parse<S: VolumeSource + ?Sized>(
        source: &S,
        volume: u32,
        limits: &Limits,
    ) -> Result<Self> {
        let (header, table_offset) = parse_header(source, volume, limits)?;
        let folders = parse_folders(source, volume, limits, &header, table_offset)?;
        let files = parse_files(source, volume, limits, &header)?;
        Ok(Self {
            header,
            folders,
            files,
        })
    }

    /// The parsed `CFHEADER`.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// The parsed folder table, in cabinet order.
    #[must_use]
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// The parsed file table, in cabinet order.
    #[must_use]
    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    /// The files that belong to `folder_index`, in cabinet order, which is
    /// also the order in which their bytes appear in the folder stream.
    pub fn files_in_folder(&self, folder_index: u16) -> impl Iterator<Item = &FileEntry> + '_ {
        let folder_count = self.header.folder_count;
        self.files
            .iter()
            .filter(move |file| file.folder.index_in(folder_count) == Some(folder_index))
    }

    /// The folder record `file` lives in.
    pub fn folder_of(&self, file: &FileEntry) -> Result<&Folder> {
        let index = file
            .folder
            .index_in(self.header.folder_count)
            .ok_or(CabError::FolderIndexOutOfRange)?;
        self.folders
            .get(index as usize)
            .ok_or(CabError::FolderIndexOutOfRange)
    }
}

/// Reads and validates the `CFHEADER`, returning it and the offset at which
/// the folder table begins.
fn parse_header<S: VolumeSource + ?Sized>(
    source: &S,
    volume: u32,
    limits: &Limits,
) -> Result<(Header, u64)> {
    let volume_len = source.volume_len(volume)?;
    let mut fixed = [0u8; HEADER_FIXED_LEN];
    if volume_len < HEADER_FIXED_LEN as u64 {
        return Err(CabError::Truncated);
    }
    source.read_at(volume, 0, &mut fixed)?;

    let mut reader = Reader::new(&fixed);
    if reader.take(4)? != SIGNATURE {
        return Err(CabError::BadSignature);
    }
    let _reserved1 = reader.u32()?;
    let cabinet_bytes = reader.u32()?;
    let _reserved2 = reader.u32()?;
    let first_file_offset = reader.u32()?;
    let _reserved3 = reader.u32()?;
    let version_minor = reader.u8()?;
    let version_major = reader.u8()?;
    let folder_count = reader.u16()?;
    let file_count = reader.u16()?;
    let flags = reader.u16()?;
    let set_id = reader.u16()?;
    let cabinet_index = reader.u16()?;

    // The only documented cabinet format version is 1.3; refuse a different
    // major version rather than guessing at a layout.
    if version_major != 1 {
        return Err(CabError::UnsupportedVersion);
    }
    if flags & !FLAGS_KNOWN != 0 {
        return Err(CabError::InvalidField);
    }
    if u64::from(cabinet_bytes) > limits.max_cabinet_bytes {
        return Err(CabError::LimitExceeded);
    }
    if u64::from(cabinet_bytes) > volume_len || (cabinet_bytes as usize) < HEADER_FIXED_LEN {
        return Err(CabError::OutOfBounds);
    }
    if u32::from(folder_count) > limits.max_folders || u32::from(file_count) > limits.max_files {
        return Err(CabError::LimitExceeded);
    }
    if folder_count == 0 {
        return Err(CabError::InvalidField);
    }

    let mut offset = HEADER_FIXED_LEN as u64;
    let mut header_reserve_bytes = 0u16;
    let mut folder_reserve_bytes = 0u8;
    let mut data_reserve_bytes = 0u8;
    if flags & FLAG_RESERVE_PRESENT != 0 {
        let mut sizes = [0u8; 4];
        read_within(source, volume, offset, &mut sizes, cabinet_bytes)?;
        let mut sizes = Reader::new(&sizes);
        header_reserve_bytes = sizes.u16()?;
        folder_reserve_bytes = sizes.u8()?;
        data_reserve_bytes = sizes.u8()?;
        if u32::from(header_reserve_bytes) > limits.max_header_reserve_bytes {
            return Err(CabError::LimitExceeded);
        }
        offset = advance(offset, 4 + u64::from(header_reserve_bytes), cabinet_bytes)?;
    }

    let (
        previous_cabinet_name_len,
        previous_disk_name_len,
        next_cabinet_name_len,
        next_disk_name_len,
    ) = read_optional_names(source, volume, limits, flags, &mut offset, cabinet_bytes)?;

    Ok((
        Header {
            cabinet_bytes,
            first_file_offset,
            version_major,
            version_minor,
            folder_count,
            file_count,
            flags,
            set_id,
            cabinet_index,
            header_reserve_bytes,
            folder_reserve_bytes,
            data_reserve_bytes,
            previous_cabinet_name_len,
            previous_disk_name_len,
            next_cabinet_name_len,
            next_disk_name_len,
        },
        offset,
    ))
}

/// Reads the four optional `CFHEADER` cabinet/disk names, returning only
/// their byte lengths: the contents are cabinet-derived text this crate has
/// no reason to keep or to expose.
type OptionalNameLengths = (Option<usize>, Option<usize>, Option<usize>, Option<usize>);

fn read_optional_names<S: VolumeSource + ?Sized>(
    source: &S,
    volume: u32,
    limits: &Limits,
    flags: u16,
    offset: &mut u64,
    cabinet_bytes: u32,
) -> Result<OptionalNameLengths> {
    let mut scratch = vec![0u8; limits.max_header_string_bytes.saturating_add(1)];
    let mut lengths = [None; 4];
    if flags & FLAG_PREV_CABINET != 0 {
        lengths[0] = Some(read_string_len(
            source,
            volume,
            offset,
            cabinet_bytes,
            &mut scratch,
        )?);
        lengths[1] = Some(read_string_len(
            source,
            volume,
            offset,
            cabinet_bytes,
            &mut scratch,
        )?);
    }
    if flags & FLAG_NEXT_CABINET != 0 {
        lengths[2] = Some(read_string_len(
            source,
            volume,
            offset,
            cabinet_bytes,
            &mut scratch,
        )?);
        lengths[3] = Some(read_string_len(
            source,
            volume,
            offset,
            cabinet_bytes,
            &mut scratch,
        )?);
    }
    Ok((lengths[0], lengths[1], lengths[2], lengths[3]))
}

/// Reads the `CFFOLDER` table, which sits between the header and `coffFiles`.
fn parse_folders<S: VolumeSource + ?Sized>(
    source: &S,
    volume: u32,
    limits: &Limits,
    header: &Header,
    table_offset: u64,
) -> Result<Vec<Folder>> {
    let cabinet_bytes = header.cabinet_bytes;
    let folder_entry_len = FOLDER_FIXED_LEN as u64 + u64::from(header.folder_reserve_bytes);
    let folder_area_len = folder_entry_len
        .checked_mul(u64::from(header.folder_count))
        .ok_or(CabError::OutOfBounds)?;
    let folder_area_end = table_offset
        .checked_add(folder_area_len)
        .ok_or(CabError::OutOfBounds)?;
    if folder_area_end > u64::from(cabinet_bytes) {
        return Err(CabError::OutOfBounds);
    }
    if u64::from(header.first_file_offset) < folder_area_end
        || u64::from(header.first_file_offset) >= u64::from(cabinet_bytes)
    {
        return Err(CabError::OutOfBounds);
    }

    let mut folders = Vec::new();
    folders
        .try_reserve(header.folder_count as usize)
        .map_err(|_| CabError::LimitExceeded)?;
    let mut entry =
        vec![0u8; usize::try_from(folder_entry_len).map_err(|_| CabError::LimitExceeded)?];
    let mut offset = table_offset;
    for _ in 0..header.folder_count {
        read_within(source, volume, offset, &mut entry, cabinet_bytes)?;
        let mut reader = Reader::new(&entry);
        let first_data_offset = reader.u32()?;
        let data_block_count = reader.u16()?;
        let type_compress = reader.u16()?;
        if u32::from(data_block_count) > limits.max_blocks_per_folder {
            return Err(CabError::LimitExceeded);
        }
        if u64::from(first_data_offset) >= u64::from(cabinet_bytes) {
            return Err(CabError::OutOfBounds);
        }
        folders.push(Folder {
            first_data_offset,
            data_block_count,
            compression: Compression::from_raw(type_compress),
        });
        offset += folder_entry_len;
    }
    Ok(folders)
}

/// Reads the variable-length `CFFILE` table starting at `coffFiles`.
fn parse_files<S: VolumeSource + ?Sized>(
    source: &S,
    volume: u32,
    limits: &Limits,
    header: &Header,
) -> Result<Vec<FileEntry>> {
    let cabinet_bytes = header.cabinet_bytes;
    let mut files = Vec::new();
    files
        .try_reserve(header.file_count as usize)
        .map_err(|_| CabError::LimitExceeded)?;
    let mut offset = u64::from(header.first_file_offset);
    let mut fixed = [0u8; FILE_FIXED_LEN];
    let mut name_scratch = vec![0u8; limits.max_name_bytes.saturating_add(1)];
    for _ in 0..header.file_count {
        read_within(source, volume, offset, &mut fixed, cabinet_bytes)?;
        let mut reader = Reader::new(&fixed);
        let uncompressed_bytes = reader.u32()?;
        let folder_offset = reader.u32()?;
        let folder = FolderRef::from_raw(reader.u16()?);
        let date = reader.u16()?;
        let time = reader.u16()?;
        let attributes = reader.u16()?;
        offset = advance(offset, FILE_FIXED_LEN as u64, cabinet_bytes)?;

        if u64::from(uncompressed_bytes) > limits.max_file_bytes {
            return Err(CabError::LimitExceeded);
        }
        if u64::from(folder_offset) + u64::from(uncompressed_bytes)
            > limits.max_folder_uncompressed_bytes
        {
            return Err(CabError::LimitExceeded);
        }
        if folder.index_in(header.folder_count).is_none() {
            return Err(CabError::FolderIndexOutOfRange);
        }

        let available = u64::from(cabinet_bytes) - offset;
        let scan = usize::try_from(available)
            .unwrap_or(usize::MAX)
            .min(name_scratch.len());
        let window = &mut name_scratch[..scan];
        source.read_at(volume, offset, window)?;
        let mut reader = Reader::new(window);
        let name = reader.nul_terminated(limits.max_name_bytes)?;
        if name.is_empty() {
            return Err(CabError::InvalidField);
        }
        let mut owned = Vec::new();
        owned
            .try_reserve(name.len())
            .map_err(|_| CabError::LimitExceeded)?;
        owned.extend_from_slice(name);
        offset = advance(offset, reader.position() as u64, cabinet_bytes)?;

        files.push(FileEntry {
            uncompressed_bytes,
            folder_offset,
            folder,
            date,
            time,
            attributes,
            name: owned,
        });
    }
    Ok(files)
}

/// Reads `buf` fully from `offset`, refusing to cross `cabinet_bytes`.
fn read_within<S: VolumeSource + ?Sized>(
    source: &S,
    volume: u32,
    offset: u64,
    buf: &mut [u8],
    cabinet_bytes: u32,
) -> Result<()> {
    let end = offset
        .checked_add(buf.len() as u64)
        .ok_or(CabError::OutOfBounds)?;
    if end > u64::from(cabinet_bytes) {
        return Err(CabError::OutOfBounds);
    }
    source.read_at(volume, offset, buf)
}

fn advance(offset: u64, delta: u64, cabinet_bytes: u32) -> Result<u64> {
    let next = offset.checked_add(delta).ok_or(CabError::OutOfBounds)?;
    if next > u64::from(cabinet_bytes) {
        return Err(CabError::OutOfBounds);
    }
    Ok(next)
}

/// Reads one NUL-terminated header string, returning only its byte length.
fn read_string_len<S: VolumeSource + ?Sized>(
    source: &S,
    volume: u32,
    offset: &mut u64,
    cabinet_bytes: u32,
    scratch: &mut [u8],
) -> Result<usize> {
    let available = u64::from(cabinet_bytes).saturating_sub(*offset);
    let scan = usize::try_from(available)
        .unwrap_or(usize::MAX)
        .min(scratch.len());
    let window = &mut scratch[..scan];
    source.read_at(volume, *offset, window)?;
    let mut reader = Reader::new(window);
    let value = reader.nul_terminated(scan.saturating_sub(1))?;
    let len = value.len();
    *offset = advance(*offset, reader.position() as u64, cabinet_bytes)?;
    Ok(len)
}
