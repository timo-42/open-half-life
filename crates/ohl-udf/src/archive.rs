//! Read-only ECMA-167 archive over `hadris-udf`.
//!
//! As in the ECMA-119 wrapper, `hadris-udf` performs the descriptor decoding
//! and this module supplies the project's trust boundary: the project-owned
//! preflight is re-run at mount time, every walk is bounded by
//! [`DirectoryLimits`], directory cycles are detected by the ICB logical block
//! number, cursors are bound to their own mount, and every failure is reduced
//! to an `ohl_core::SanitizedError` that carries no name.

use crate::adaptor::BlockCursor;
use crate::preflight;
use alloc::vec::Vec;
use hadris_udf::descriptor::LongAllocationDescriptor;
use hadris_udf::fs::UdfVolume;
use ohl_core::SanitizedError;
use ohl_media_archive::path::components;
use ohl_media_archive::{
    BlockReader, DirectoryCursor, DirectoryEntry, DirectoryLimits, DirectoryPage, EntryType,
    FilesystemDescription, MediaArchive, MediaClass, MediaFileHandle, MediaPreflight, MountId,
    VolumeLabel, is_single_path_component, normalize_path,
};

/// The most enumerations one mount remembers so a cursor can be continued.
///
/// A cursor records the directory's ICB logical block number; this table maps
/// that number back to the full allocation descriptor the third-party reader
/// needs. It is a small fixed ring, so an adversarial caller cannot grow it.
const MAX_REMEMBERED_DIRECTORIES: usize = 16;

/// A read-only file inside a mounted ECMA-167 volume.
///
/// `hadris-udf` exposes a file's allocation descriptors only through a whole
/// read, so an open handle buffers the file's bytes once, bounded by
/// [`DirectoryLimits::max_buffered_file_bytes`]. Seeks and reads are then
/// served from that bounded buffer, which cannot address anything outside the
/// extent the reader validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdfFile {
    mount: MountId,
    contents: Vec<u8>,
    position: u64,
}

impl MediaFileHandle for UdfFile {
    fn size(&self) -> u64 {
        self.contents.len() as u64
    }

    fn position(&self) -> u64 {
        self.position
    }

    fn seek(&mut self, offset: u64) -> Result<(), SanitizedError> {
        if offset > self.size() {
            return Err(SanitizedError::InvalidInput);
        }
        self.position = offset;
        Ok(())
    }
}

/// Which directory a walk is standing in.
///
/// `hadris-udf` exposes the root only as an already-read directory, not as an
/// allocation descriptor, so the root is represented separately rather than
/// by synthesizing a descriptor the medium never recorded.
#[derive(Debug, Clone, Copy)]
enum DirRef {
    /// The file set's root directory.
    Root,
    /// A directory reached through its ICB allocation descriptor.
    Icb(LongAllocationDescriptor),
}

impl DirRef {
    /// The identity a cursor records. The root cannot collide with an ICB
    /// because a logical block number is a `u32`.
    const ROOT_KEY: u64 = u64::MAX;

    fn key(self) -> u64 {
        match self {
            Self::Root => Self::ROOT_KEY,
            Self::Icb(icb) => u64::from(icb.logical_block_num),
        }
    }

    fn length(self) -> u64 {
        match self {
            Self::Root => 0,
            Self::Icb(icb) => u64::from(icb.length()),
        }
    }
}

/// A mounted, read-only ECMA-167 volume.
pub struct UdfArchive<R: BlockReader> {
    volume: UdfVolume<BlockCursor<R>>,
    result: MediaPreflight,
    limits: DirectoryLimits,
    mount: MountId,
    remembered: Vec<(u64, DirRef)>,
}

impl<R: BlockReader> UdfArchive<R> {
    /// Mounts `reader`, re-running the project-owned preflight first.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] for invalid limits or an
    /// invalid descriptor set, [`SanitizedError::Unsupported`] when the media
    /// carries no ECMA-167 recognition sequence, and the reader's own
    /// sanitized error when a block could not be read.
    pub fn open(reader: R, limits: DirectoryLimits) -> Result<Self, SanitizedError> {
        limits.validate()?;
        let mut reader = reader;
        let result = preflight::preflight(&mut reader)?;
        let volume =
            UdfVolume::open(BlockCursor::new(reader)).map_err(|_| SanitizedError::InvalidInput)?;
        if volume.info().block_size as usize != ohl_media_archive::BLOCK_SIZE {
            return Err(SanitizedError::InvalidInput);
        }
        Ok(Self {
            volume,
            result,
            limits,
            mount: MountId::allocate(),
            remembered: Vec::new(),
        })
    }

    /// The validated preflight classification of the mounted volume.
    pub fn preflight(&self) -> &MediaPreflight {
        &self.result
    }

    fn remember(&mut self, directory: DirRef) {
        let key = directory.key();
        if self.remembered.iter().any(|(known, _)| *known == key) {
            return;
        }
        if self.remembered.len() == MAX_REMEMBERED_DIRECTORIES {
            self.remembered.remove(0);
        }
        self.remembered.push((key, directory));
    }

    fn recall(&self, key: u64) -> Option<DirRef> {
        self.remembered
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, directory)| *directory)
    }

    /// Reads one directory through the third-party reader.
    fn read_dir(&self, directory: DirRef) -> Result<hadris_udf::UdfDir, SanitizedError> {
        match directory {
            DirRef::Root => self.volume.root_dir(),
            DirRef::Icb(icb) => self.volume.read_directory(&icb),
        }
        .map_err(|_| SanitizedError::InvalidInput)
    }

    /// Reads one directory's usable entries, applying the project's bounds.
    fn read_entries(&self, directory: DirRef) -> Result<Vec<DirectoryEntry>, SanitizedError> {
        if directory.length() > self.limits.max_directory_extent_bytes {
            return Err(SanitizedError::InvalidInput);
        }
        let directory = self.read_dir(directory)?;
        let mut entries = Vec::new();
        for entry in directory.entries() {
            let name = entry.name();
            if name.len() > self.limits.max_entry_name_bytes as usize
                || !is_single_path_component(name)
            {
                return Err(SanitizedError::InvalidInput);
            }
            if entries.len() as u64 >= self.limits.max_total_entries {
                return Err(SanitizedError::InvalidInput);
            }
            entries.push(DirectoryEntry {
                name: alloc::string::String::from(name),
                entry_type: if entry.is_dir() {
                    EntryType::Directory
                } else {
                    EntryType::File
                },
                size_bytes: if entry.is_dir() { 0 } else { entry.size },
            });
        }
        Ok(entries)
    }

    /// Resolves a normalized path to a directory ICB, detecting cycles by the
    /// ICB's logical block number.
    fn resolve_directory(&self, normalized: &str) -> Result<DirRef, SanitizedError> {
        let mut current = DirRef::Root;
        let mut visited: Vec<u64> = alloc::vec![current.key()];
        let mut depth = 0u32;

        for component in components(normalized) {
            depth += 1;
            if depth > self.limits.max_path_components {
                return Err(SanitizedError::InvalidInput);
            }
            let directory = self.read_dir(current)?;
            let entry = directory
                .entries()
                .find(|entry| entry.name() == component)
                .ok_or(SanitizedError::NotFound)?;
            if !entry.is_dir() {
                return Err(SanitizedError::NotFound);
            }
            let next = DirRef::Icb(entry.icb);
            let extent = next.key();
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

    fn resolve_file(&self, normalized: &str) -> Result<UdfFile, SanitizedError> {
        let (parent, name) = match normalized.rsplit_once('/') {
            Some((parent, name)) if !name.is_empty() => {
                (if parent.is_empty() { "/" } else { parent }, name)
            }
            _ => return Err(SanitizedError::InvalidInput),
        };
        let parent = self.resolve_directory(parent)?;
        let directory = self.read_dir(parent)?;
        let entry = directory
            .entries()
            .find(|entry| entry.name() == name)
            .ok_or(SanitizedError::NotFound)?;
        if entry.is_dir() {
            return Err(SanitizedError::InvalidInput);
        }
        if entry.size > self.limits.max_buffered_file_bytes {
            return Err(SanitizedError::Unsupported);
        }
        let contents = self
            .volume
            .read_file(entry)
            .map_err(|_| SanitizedError::InvalidInput)?;
        if contents.len() as u64 != entry.size {
            return Err(SanitizedError::InvalidInput);
        }
        Ok(UdfFile {
            mount: self.mount,
            contents,
            position: 0,
        })
    }

    fn page_from(
        &self,
        directory: DirRef,
        start_index: u64,
        already_returned: u64,
        pages_emitted: u32,
    ) -> Result<DirectoryPage, SanitizedError> {
        if pages_emitted >= self.limits.max_page_count {
            return Err(SanitizedError::InvalidInput);
        }
        let entries = self.read_entries(directory)?;
        let total = entries.len() as u64;
        let mut page = Vec::new();
        let mut name_bytes = 0u64;
        let mut index = start_index;
        while index < total {
            let entry = &entries[usize::try_from(index).map_err(|_| SanitizedError::Internal)?];
            if page.len() as u64 >= u64::from(self.limits.max_page_entries) {
                break;
            }
            let next_bytes = name_bytes
                .checked_add(entry.name.len() as u64)
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
            page.push(entry.clone());
            index += 1;
        }

        let cursor = (index < total).then(|| DirectoryCursor {
            mount: self.mount,
            directory_extent: directory.key(),
            directory_length: directory.length(),
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

impl<R: BlockReader> MediaArchive for UdfArchive<R> {
    type File = UdfFile;

    fn media_class(&self) -> MediaClass {
        MediaClass::Udf
    }

    fn filesystem(&self) -> FilesystemDescription {
        self.result.filesystem
    }

    fn volume_label(&self) -> &VolumeLabel {
        &self.result.volume_label
    }

    fn list_page(&mut self, path: &str) -> Result<DirectoryPage, SanitizedError> {
        let normalized = normalize_path(path).ok_or(SanitizedError::InvalidInput)?;
        let directory = self.resolve_directory(&normalized)?;
        let page = self.page_from(directory, 0, 0, 0)?;
        if page.cursor.is_some() {
            self.remember(directory);
        }
        Ok(page)
    }

    fn continue_list(&mut self, cursor: DirectoryCursor) -> Result<DirectoryPage, SanitizedError> {
        if cursor.mount != self.mount {
            return Err(SanitizedError::InvalidInput);
        }
        let directory = self
            .recall(cursor.directory_extent)
            .ok_or(SanitizedError::InvalidInput)?;
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
        let position = usize::try_from(file.position).map_err(|_| SanitizedError::Internal)?;
        if position >= file.contents.len() || out.is_empty() {
            return Ok(0);
        }
        let count = (file.contents.len() - position).min(out.len());
        out[..count].copy_from_slice(&file.contents[position..position + count]);
        file.position += count as u64;
        Ok(count)
    }
}
