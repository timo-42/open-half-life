//! Read-only ECMA-119 archive over `hadris-iso`.
//!
//! The wrapper is deliberately thin: `hadris-iso` performs the directory
//! record decoding, and this module adds the project's trust boundary around
//! it — the project-owned preflight is re-run at mount time, every recorded
//! extent is re-checked against the volume geometry, every walk is bounded by
//! [`DirectoryLimits`], directory cycles are detected by extent, and every
//! failure is reduced to an `ohl_core::SanitizedError` that carries no name.

use crate::adaptor::BlockCursor;
use crate::preflight::{self, Iso9660Preflight};
use alloc::string::String;
use alloc::vec::Vec;
use hadris_iso::directory::DirectoryRef;
use hadris_iso::io::LogicalSector;
use hadris_iso::read::{DirEntry, IsoImage};
use ohl_core::SanitizedError;
use ohl_media_archive::path::components;
use ohl_media_archive::{
    BLOCK_SIZE_U64, BlockReader, DirectoryCursor, DirectoryEntry, DirectoryLimits, DirectoryPage,
    EntryType, FilesystemDescription, MediaArchive, MediaClass, MediaFileHandle, MountId,
    VolumeLabel, is_single_path_component, normalize_path,
};

/// A read-only file inside a mounted ECMA-119 volume.
///
/// The handle records only the validated extent it may read, so it can never
/// be steered outside the volume by a later call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso9660File {
    mount: MountId,
    start_lba: u64,
    size: u64,
    position: u64,
}

impl MediaFileHandle for Iso9660File {
    fn size(&self) -> u64 {
        self.size
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn seek(&mut self, offset: u64) -> Result<(), SanitizedError> {
        if offset > self.size {
            return Err(SanitizedError::InvalidInput);
        }
        self.position = offset;
        Ok(())
    }
}

/// A mounted, read-only ECMA-119 volume.
///
/// When a valid Joliet supplementary descriptor is present its directory tree
/// is preferred; otherwise the primary tree is used. ASCII case folding is
/// applied only when resolving names in the primary tree, because Joliet
/// identifiers preserve case and may differ only by case.
pub struct Iso9660Archive<R: BlockReader> {
    image: IsoImage<BlockCursor<R>>,
    result: Iso9660Preflight,
    limits: DirectoryLimits,
    mount: MountId,
    root: DirectoryRef,
}

impl<R: BlockReader> Iso9660Archive<R> {
    /// Mounts `reader`, re-running the project-owned preflight first.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] for invalid limits or an
    /// invalid descriptor set, [`SanitizedError::Unsupported`] when the media
    /// is not an ECMA-119 volume, and [`SanitizedError::Internal`] when the
    /// third-party reader rejects a descriptor set the preflight accepted.
    pub fn open(reader: R, limits: DirectoryLimits) -> Result<Self, SanitizedError> {
        limits.validate()?;
        let mut reader = reader;
        // The preflight result recorded during import is never trusted as a
        // parsing input: the descriptor set is validated again here, against
        // the same source the reader will use.
        let result = preflight::preflight(&mut reader)?;

        let image =
            IsoImage::open(BlockCursor::new(reader)).map_err(|_| SanitizedError::InvalidInput)?;
        let geometry = result.preferred();
        let root = DirectoryRef {
            extent: LogicalSector(
                usize::try_from(geometry.root_extent).map_err(|_| SanitizedError::InvalidInput)?,
            ),
            size: usize::try_from(geometry.root_length)
                .map_err(|_| SanitizedError::InvalidInput)?,
        };

        Ok(Self {
            image,
            result,
            limits,
            mount: MountId::allocate(),
            root,
        })
    }

    /// Whether the Joliet tree was selected.
    pub fn uses_joliet(&self) -> bool {
        self.result.uses_joliet()
    }

    /// The validated preflight classification of the mounted volume.
    pub fn preflight(&self) -> &Iso9660Preflight {
        &self.result
    }

    fn volume_blocks(&self) -> u64 {
        u64::from(self.result.preferred().volume_blocks)
    }

    /// Rejects a directory extent that is not a whole number of logical
    /// blocks (ECMA-119 9.1.4), exceeds the configured ceiling, or leaves the
    /// volume.
    fn check_directory_ref(&self, directory: DirectoryRef) -> Result<(), SanitizedError> {
        let size = directory.size as u64;
        if size == 0 || !size.is_multiple_of(BLOCK_SIZE_U64) {
            return Err(SanitizedError::InvalidInput);
        }
        if size > self.limits.max_directory_extent_bytes {
            return Err(SanitizedError::InvalidInput);
        }
        let blocks = size / BLOCK_SIZE_U64;
        let extent = directory.extent.0 as u64;
        if extent
            .checked_add(blocks)
            .is_none_or(|end| end > self.volume_blocks())
        {
            return Err(SanitizedError::InvalidInput);
        }
        Ok(())
    }

    /// Reads one directory's usable entries, applying every per-record check
    /// the project performs on top of `hadris-iso`.
    fn read_entries(&self, directory: DirectoryRef) -> Result<Vec<DirEntry>, SanitizedError> {
        self.check_directory_ref(directory)?;
        let raw = self
            .image
            .open_dir(directory)
            .read_entries()
            .map_err(|_| SanitizedError::InvalidInput)?;
        let mut entries = Vec::new();
        for entry in raw {
            if entry.is_special() {
                continue;
            }
            self.check_record(&entry)?;
            if entries.len() as u64 >= self.limits.max_total_entries {
                return Err(SanitizedError::InvalidInput);
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Applies the ECMA-119 9.1 record checks the project requires.
    fn check_record(&self, entry: &DirEntry) -> Result<(), SanitizedError> {
        let header = entry.header();
        // Extended attribute records, interleaved layouts, and multi-extent
        // files are deliberately not interpreted.
        if header.extended_attr_record != 0
            || header.file_unit_size != 0
            || header.interleave_gap_size != 0
            || header.flags & 0x80 != 0
            || entry.is_multi_extent()
        {
            return Err(SanitizedError::InvalidInput);
        }
        // Only the first volume of a volume set is readable here.
        if header.volume_sequence_number.read() != 1 {
            return Err(SanitizedError::InvalidInput);
        }
        // The declared identifier length must fit inside the record.
        let identifier_length = entry.name().len();
        if identifier_length == 0
            || identifier_length > self.limits.max_entry_name_bytes as usize
            || 33 + identifier_length > entry.size()
        {
            return Err(SanitizedError::InvalidInput);
        }
        // The recorded extent must lie inside the volume.
        let extent = u64::from(header.extent.read());
        let length = u64::from(header.data_len.read());
        // ECMA-119 9.1.4: a directory's data length is a whole number of
        // logical blocks.
        if entry.is_directory() && (length == 0 || !length.is_multiple_of(BLOCK_SIZE_U64)) {
            return Err(SanitizedError::InvalidInput);
        }
        let blocks = length.div_ceil(BLOCK_SIZE_U64);
        if extent >= self.volume_blocks()
            || extent
                .checked_add(blocks)
                .is_none_or(|end| end > self.volume_blocks())
        {
            return Err(SanitizedError::InvalidInput);
        }
        Ok(())
    }

    /// Decodes one identifier into a single path component.
    fn decode_name(&self, entry: &DirEntry) -> Result<String, SanitizedError> {
        let raw = entry.name();
        let decoded = if self.uses_joliet() {
            if !raw.len().is_multiple_of(2) {
                return Err(SanitizedError::InvalidInput);
            }
            let mut units = Vec::with_capacity(raw.len() / 2);
            for pair in raw.as_chunks::<2>().0 {
                units.push(u16::from_be_bytes([pair[0], pair[1]]));
            }
            String::from_utf16(&units).map_err(|_| SanitizedError::InvalidInput)?
        } else {
            core::str::from_utf8(raw)
                .map_err(|_| SanitizedError::InvalidInput)?
                .into()
        };
        let decoded: String = decoded
            .chars()
            .filter(|character| *character != '\0')
            .collect();
        // ECMA-119 7.5/7.6: strip the `;N` file version number.
        let stripped = match decoded.rsplit_once(';') {
            Some((base, version))
                if !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                base
            }
            _ => decoded.as_str(),
        };
        if stripped.len() > self.limits.max_entry_name_bytes as usize
            || !is_single_path_component(stripped)
        {
            return Err(SanitizedError::InvalidInput);
        }
        Ok(String::from(stripped))
    }

    fn names_match(&self, decoded: &str, wanted: &str) -> bool {
        if self.uses_joliet() {
            decoded == wanted
        } else {
            decoded.eq_ignore_ascii_case(wanted)
        }
    }

    /// Resolves a normalized path to a directory, detecting extent cycles.
    fn resolve_directory(&self, normalized: &str) -> Result<DirectoryRef, SanitizedError> {
        let mut current = self.root;
        let mut visited: Vec<u64> = alloc::vec![current.extent.0 as u64];
        let mut depth = 0u32;

        for component in components(normalized) {
            depth += 1;
            if depth > self.limits.max_path_components {
                return Err(SanitizedError::InvalidInput);
            }
            let entries = self.read_entries(current)?;
            let mut found = None;
            for entry in &entries {
                if self.names_match(&self.decode_name(entry)?, component) {
                    found = Some(entry.clone());
                    break;
                }
            }
            let entry = found.ok_or(SanitizedError::NotFound)?;
            if !entry.is_directory() {
                return Err(SanitizedError::NotFound);
            }
            let next = entry
                .as_dir_ref(&self.image)
                .map_err(|_| SanitizedError::InvalidInput)?;
            let extent = next.extent.0 as u64;
            if visited.contains(&extent) {
                return Err(SanitizedError::InvalidInput);
            }
            if visited.len() >= self.limits.max_directories_visited as usize {
                return Err(SanitizedError::InvalidInput);
            }
            visited.push(extent);
            current = next;
        }
        Ok(current)
    }

    /// Resolves a normalized path to a file entry inside its parent.
    fn resolve_file(&self, normalized: &str) -> Result<Iso9660File, SanitizedError> {
        let (parent, name) = match normalized.rsplit_once('/') {
            Some((parent, name)) if !name.is_empty() => {
                (if parent.is_empty() { "/" } else { parent }, name)
            }
            _ => return Err(SanitizedError::InvalidInput),
        };
        let directory = self.resolve_directory(parent)?;
        for entry in self.read_entries(directory)? {
            if !self.names_match(&self.decode_name(&entry)?, name) {
                continue;
            }
            if entry.is_directory() {
                return Err(SanitizedError::InvalidInput);
            }
            let header = entry.header();
            return Ok(Iso9660File {
                mount: self.mount,
                start_lba: u64::from(header.extent.read()),
                size: u64::from(header.data_len.read()),
                position: 0,
            });
        }
        Err(SanitizedError::NotFound)
    }

    fn page_from(
        &self,
        directory: DirectoryRef,
        start_index: u64,
        already_returned: u64,
        pages_emitted: u32,
    ) -> Result<DirectoryPage, SanitizedError> {
        if pages_emitted >= self.limits.max_page_count {
            return Err(SanitizedError::InvalidInput);
        }
        let entries = self.read_entries(directory)?;
        let mut page = Vec::new();
        let mut name_bytes = 0u64;
        let mut index = start_index;
        let total = entries.len() as u64;
        while index < total {
            let entry = &entries[usize::try_from(index).map_err(|_| SanitizedError::Internal)?];
            let name = self.decode_name(entry)?;
            if page.len() as u64 >= u64::from(self.limits.max_page_entries) {
                break;
            }
            let next_bytes = name_bytes
                .checked_add(name.len() as u64)
                .ok_or(SanitizedError::ArithmeticOverflow)?;
            if next_bytes > self.limits.max_page_name_bytes {
                if page.is_empty() {
                    return Err(SanitizedError::InvalidInput);
                }
                break;
            }
            if already_returned + page.len() as u64 >= self.limits.max_total_entries {
                return Err(SanitizedError::InvalidInput);
            }
            name_bytes = next_bytes;
            page.push(DirectoryEntry {
                name,
                entry_type: if entry.is_directory() {
                    EntryType::Directory
                } else {
                    EntryType::File
                },
                size_bytes: if entry.is_directory() {
                    0
                } else {
                    u64::from(entry.header().data_len.read())
                },
            });
            index += 1;
        }

        let cursor = (index < total).then(|| DirectoryCursor {
            mount: self.mount,
            directory_extent: directory.extent.0 as u64,
            directory_length: directory.size as u64,
            next_index: index,
            returned_entries: already_returned + page.len() as u64,
            pages_emitted: pages_emitted + 1,
        });
        Ok(DirectoryPage {
            entries: page,
            cursor,
        })
    }
}

impl<R: BlockReader> MediaArchive for Iso9660Archive<R> {
    type File = Iso9660File;

    fn media_class(&self) -> MediaClass {
        MediaClass::Iso9660
    }

    fn filesystem(&self) -> FilesystemDescription {
        self.result.media.filesystem
    }

    fn volume_label(&self) -> &VolumeLabel {
        &self.result.media.volume_label
    }

    fn list_page(&mut self, path: &str) -> Result<DirectoryPage, SanitizedError> {
        let normalized = normalize_path(path).ok_or(SanitizedError::InvalidInput)?;
        let directory = self.resolve_directory(&normalized)?;
        self.page_from(directory, 0, 0, 0)
    }

    fn continue_list(&mut self, cursor: DirectoryCursor) -> Result<DirectoryPage, SanitizedError> {
        if cursor.mount != self.mount {
            return Err(SanitizedError::InvalidInput);
        }
        let directory = DirectoryRef {
            extent: LogicalSector(
                usize::try_from(cursor.directory_extent)
                    .map_err(|_| SanitizedError::InvalidInput)?,
            ),
            size: usize::try_from(cursor.directory_length)
                .map_err(|_| SanitizedError::InvalidInput)?,
        };
        self.page_from(
            directory,
            cursor.next_index,
            cursor.returned_entries,
            cursor.pages_emitted,
        )
    }

    fn open_file(&mut self, path: &str) -> Result<Self::File, SanitizedError> {
        let normalized = normalize_path(path).ok_or(SanitizedError::InvalidInput)?;
        self.resolve_file(&normalized)
    }

    fn open_file_at(
        &mut self,
        directory: &str,
        entry_name: &str,
    ) -> Result<Self::File, SanitizedError> {
        if !is_single_path_component(entry_name) {
            return Err(SanitizedError::InvalidInput);
        }
        let normalized = normalize_path(directory).ok_or(SanitizedError::InvalidInput)?;
        let joined = if normalized == "/" {
            alloc::format!("/{entry_name}")
        } else {
            alloc::format!("{normalized}/{entry_name}")
        };
        self.resolve_file(&joined)
    }

    fn read_file(
        &mut self,
        file: &mut Self::File,
        out: &mut [u8],
    ) -> Result<usize, SanitizedError> {
        if file.mount != self.mount {
            return Err(SanitizedError::InvalidInput);
        }
        if file.position >= file.size || out.is_empty() {
            return Ok(0);
        }
        let remaining = file.size - file.position;
        let count = usize::try_from(remaining.min(out.len() as u64))
            .map_err(|_| SanitizedError::Internal)?;
        let offset = file
            .start_lba
            .checked_mul(BLOCK_SIZE_U64)
            .and_then(|base| base.checked_add(file.position))
            .ok_or(SanitizedError::ArithmeticOverflow)?;
        self.image
            .read_bytes_at(offset, &mut out[..count])
            .map_err(|_| SanitizedError::InvalidInput)?;
        file.position += count as u64;
        Ok(count)
    }
}
