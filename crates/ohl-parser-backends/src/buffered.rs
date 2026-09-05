//! The fully buffered back ends: Microsoft cabinet and InstallShield 3 Z.
//!
//! Unlike a Wise package, whose 250 MiB-scale overlay is walked through a
//! sliding window, these two containers are read into memory whole and then
//! decoded with the crates' ordinary blocking APIs over an in-memory slice.
//! That is a deliberate trade:
//!
//! - both decoders are random-access over their table of contents, so a
//!   window would thrash where a Wise chain walks forward;
//! - the container is bounded before the first byte is read, by
//!   [`MAXIMUM_BUFFERED_BYTES`], so a hostile medium cannot make the worker
//!   allocate more than the arena holds;
//! - the fixed arena has no reclamation, so decoding one entry at a time into
//!   a bounded buffer is the allocation shape the image can actually afford.
//!
//! A container larger than the ceiling is refused as unsupported rather than
//! decoded partially.

use alloc::vec;
use alloc::vec::Vec;

use ohl_isz::{Archive, Limits as IszLimits, NeverCancelled as IszNeverCancelled};
use ohl_mscab::{
    Cabinet, FolderSegment, Limits as CabLimits, NeverCancelled as CabNeverCancelled, SliceSource,
    extract_file,
};

use crate::window::PendingRead;

/// The largest container this crate will buffer whole.
pub const MAXIMUM_BUFFERED_BYTES: u64 = 32 * 1024 * 1024;

/// One entry offered by a buffered back end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferedEntry {
    /// The opaque token, which is the entry's index in its container.
    pub token: u64,
    /// The declared uncompressed size.
    pub size_bytes: u64,
    /// The recorded name bytes, untrusted and never logged.
    pub name: Vec<u8>,
}

/// A container being read into memory, front to back.
#[derive(Debug)]
pub struct ContainerBuffer {
    bytes: Vec<u8>,
    size: u64,
}

impl ContainerBuffer {
    /// A buffer for a container of `size` bytes.
    ///
    /// # Errors
    /// `()` when the container is empty or above [`MAXIMUM_BUFFERED_BYTES`].
    pub fn new(size: u64) -> Result<Self, ()> {
        if size == 0 || size > MAXIMUM_BUFFERED_BYTES {
            return Err(());
        }
        Ok(Self {
            bytes: Vec::new(),
            size,
        })
    }

    /// Whether every byte of the container is in memory.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.bytes.len() as u64 >= self.size
    }

    /// The next read to ask the parent for, at most `maximum_read` bytes.
    #[must_use]
    pub fn next_read(&self, maximum_read: u32) -> PendingRead {
        let offset = self.bytes.len() as u64;
        let remaining = self.size.saturating_sub(offset);
        let length = remaining.min(u64::from(maximum_read.max(1)));
        PendingRead {
            offset,
            length: u32::try_from(length).unwrap_or(u32::MAX),
        }
    }

    /// Appends one answered read.
    ///
    /// Returns `false` when the reply would overrun the container, which the
    /// caller must treat as a dispatch failure.
    pub fn append(&mut self, data: &[u8]) -> bool {
        if data.is_empty() || self.bytes.len() as u64 + data.len() as u64 > self.size {
            return false;
        }
        self.bytes.extend_from_slice(data);
        true
    }

    /// The buffered bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Enumerates a cabinet that is already in memory.
///
/// # Errors
/// A fixed [`ohl_mscab::CabError`].
pub fn cabinet_entries(
    bytes: &[u8],
    limits: &CabLimits,
) -> Result<Vec<BufferedEntry>, ohl_mscab::CabError> {
    let source = SliceSource::new(bytes);
    let cabinet = Cabinet::parse(&source, 0, limits)?;
    Ok(cabinet
        .files()
        .iter()
        .enumerate()
        .map(|(index, file)| BufferedEntry {
            token: index as u64,
            size_bytes: u64::from(file.uncompressed_bytes),
            name: Vec::from(file.name_bytes()),
        })
        .collect())
}

/// Extracts one cabinet file into a fresh buffer.
///
/// # Errors
/// A fixed [`ohl_mscab::CabError`]; a file whose folder continues into
/// another volume is [`ohl_mscab::CabError::Unsupported`], because a worker
/// is handed exactly one container window.
pub fn cabinet_extract(
    bytes: &[u8],
    limits: CabLimits,
    token: u64,
) -> Result<Vec<u8>, ohl_mscab::CabError> {
    let source = SliceSource::new(bytes);
    let cabinet = Cabinet::parse(&source, 0, &limits)?;
    let index = usize::try_from(token).map_err(|_| ohl_mscab::CabError::OutOfBounds)?;
    let file = cabinet
        .files()
        .get(index)
        .ok_or(ohl_mscab::CabError::OutOfBounds)?;
    let folder = cabinet.folder_of(file)?;
    let segment = FolderSegment::new(&cabinet, 0, folder_index(&cabinet, folder)?)?;
    let mut out = Vec::new();
    extract_file(
        &source,
        folder.compression,
        limits,
        vec![segment],
        file,
        &CabNeverCancelled,
        |chunk| {
            out.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    Ok(out)
}

/// The index of `folder` inside `cabinet`'s folder table.
fn folder_index(cabinet: &Cabinet, folder: &ohl_mscab::Folder) -> Result<u16, ohl_mscab::CabError> {
    cabinet
        .folders()
        .iter()
        .position(|candidate| core::ptr::eq(candidate, folder))
        .and_then(|index| u16::try_from(index).ok())
        .ok_or(ohl_mscab::CabError::OutOfBounds)
}

/// Enumerates an InstallShield 3 Z archive that is already in memory.
///
/// Names are the directory record's name joined to the entry's own name with
/// the archive's own `\` separator, exactly as recorded.
///
/// # Errors
/// A fixed [`ohl_isz::Error`].
pub fn z_archive_entries(
    bytes: &[u8],
    limits: &IszLimits,
) -> Result<Vec<BufferedEntry>, ohl_isz::Error> {
    let mut source = ohl_isz::SliceSource::new(bytes);
    let archive = Archive::open(&mut source, 0, limits, &IszNeverCancelled)?;
    let mut entries = Vec::new();
    for (index, entry) in archive.entries().iter().enumerate() {
        let mut name = Vec::new();
        if let Some(directory) = archive.directories().get(entry.directory_index as usize)
            && !directory.name.as_bytes().is_empty()
        {
            name.extend_from_slice(directory.name.as_bytes());
            name.push(b'\\');
        }
        name.extend_from_slice(entry.name.as_bytes());
        entries.push(BufferedEntry {
            token: index as u64,
            size_bytes: u64::from(entry.expanded_size),
            name,
        });
    }
    Ok(entries)
}

/// Extracts one Z-archive entry into a fresh buffer.
///
/// # Errors
/// A fixed [`ohl_isz::Error`].
pub fn z_archive_extract(
    bytes: &[u8],
    limits: &IszLimits,
    token: u64,
) -> Result<Vec<u8>, ohl_isz::Error> {
    let mut source = ohl_isz::SliceSource::new(bytes);
    let mut archive = Archive::open(&mut source, 0, limits, &IszNeverCancelled)?;
    let index = u32::try_from(token).map_err(|_| ohl_isz::Error::OutOfRange)?;
    let mut reader = archive.open_entry(index)?;
    reader.read_to_vec(&mut source, &IszNeverCancelled)
}

#[cfg(test)]
mod tests {
    use super::{ContainerBuffer, MAXIMUM_BUFFERED_BYTES};

    #[test]
    fn a_container_is_read_front_to_back() {
        let mut buffer = ContainerBuffer::new(10).expect("a small container");
        assert!(!buffer.is_complete());
        assert_eq!(buffer.next_read(4).length, 4);
        assert!(buffer.append(&[1, 2, 3, 4]));
        assert_eq!(buffer.next_read(4).offset, 4);
        assert!(buffer.append(&[5, 6, 7, 8]));
        assert_eq!(buffer.next_read(4).length, 2);
        assert!(buffer.append(&[9, 10]));
        assert!(buffer.is_complete());
        assert_eq!(buffer.bytes(), &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert!(!buffer.append(&[11]));
    }

    #[test]
    fn an_oversize_container_is_refused_before_any_read() {
        assert!(ContainerBuffer::new(0).is_err());
        assert!(ContainerBuffer::new(MAXIMUM_BUFFERED_BYTES + 1).is_err());
        assert!(ContainerBuffer::new(MAXIMUM_BUFFERED_BYTES).is_ok());
    }
}
