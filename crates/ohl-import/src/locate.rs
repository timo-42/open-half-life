//! Finding container candidates in a mounted medium.
//!
//! An installation medium does not announce where its payload lives. This
//! module walks the mounted tree under hard limits and classifies each
//! sufficiently large file from its first bytes only, so the parent can hand
//! the worker a [`SourceWindow`](crate::SourceWindow) over exactly one
//! container instead of the whole image.
//!
//! Nothing here parses a payload, decompresses anything, or executes
//! anything. It reads headers, computes one offset, and looks for two fixed
//! byte signatures.
//!
//! # What it recognises
//!
//! - **Microsoft cabinet**, by the ASCII `MSCF` signature at offset 0 of a
//!   file, and by the same signature inside a PE overlay.
//! - **InstallShield 3 Z archive**, by its little-endian `u32` signature
//!   `0x8C65_5D13` inside a PE overlay.
//!
//! The PE/COFF walk — DOS header, `e_lfanew`, the `PE\0\0` signature, the
//! COFF file header, the optional-header size, and the section table — is
//! implemented from Microsoft's public **PE Format** specification, recorded
//! in `docs/FORMAT_SOURCES.md` under "PE/COFF executable layout". The
//! overlay is everything past the largest section raw end, which is where a
//! self-extracting installer keeps its archive.
//!
//! # Bounds
//!
//! Every quantity a medium controls is bounded by [`LocateLimits`]: the walk
//! depth, the number of directories and files visited, the header prefix
//! read from each candidate, the overlay bytes scanned per file, and the
//! total bytes read. Reaching a limit stops the search and returns what was
//! already found; it is never an error, because a truncated search is a
//! correct "no candidate here" answer for the caller.
//!
//! # Logging
//!
//! [`ContainerCandidate::archive_path`] is media-derived and must never be
//! logged. `ContainerCandidate`'s own `Debug` redacts it.

use core::fmt;

use ohl_core::SanitizedError;
use ohl_vfs::{DirectoryPage, EntryType, MediaFile, Mount, normalize_path};

use crate::catalog::NormalizedPath;
use crate::io::CancellationToken;

/// The InstallShield 3 Z archive signature, as a little-endian `u32`.
pub const INSTALLSHIELD_Z_SIGNATURE: u32 = 0x8C65_5D13;

/// The Microsoft cabinet signature.
pub const CABINET_SIGNATURE: [u8; 4] = *b"MSCF";

/// The DOS `MZ` signature at offset 0 of every PE image.
const DOS_SIGNATURE: [u8; 2] = *b"MZ";

/// The offset of `e_lfanew` inside the DOS header.
const E_LFANEW_OFFSET: usize = 0x3c;

/// The `PE\0\0` signature `e_lfanew` points at.
const PE_SIGNATURE: [u8; 4] = *b"PE\0\0";

/// The size of the COFF file header that follows the PE signature.
const COFF_HEADER_BYTES: usize = 20;

/// The size of one section-table row.
const SECTION_HEADER_BYTES: usize = 40;

/// The PE image ceiling on the number of sections.
const MAXIMUM_SECTIONS: u16 = 96;

/// The smallest file worth classifying.
pub const MINIMUM_CANDIDATE_BYTES: u64 = 64 * 1024;

/// Which container a candidate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ContainerKind {
    /// An InstallShield 3 Z archive, found in a PE overlay.
    InstallShieldZ,
    /// A Microsoft cabinet, at offset 0 or in a PE overlay.
    MicrosoftCabinet,
}

/// One located container, as a byte range inside one file of the medium.
#[derive(Clone, PartialEq, Eq)]
pub struct ContainerCandidate {
    /// The medium-relative path of the file holding the container.
    ///
    /// Media-derived: never log it, never put it in an error, and never
    /// commit it.
    pub archive_path: NormalizedPath,
    /// What the signature says the container is.
    pub kind: ContainerKind,
    /// The container's first byte, as an offset inside that file.
    pub offset: u64,
    /// The container's length: everything from `offset` to the file's end.
    pub length: u64,
}

impl fmt::Debug for ContainerCandidate {
    /// Redacts the media-derived path so a `{:?}` in a log or an assertion
    /// message cannot leak an internal name.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContainerCandidate")
            .field("archive_path", &"<redacted>")
            .field("kind", &self.kind)
            .field("offset", &self.offset)
            .field("length", &self.length)
            .finish()
    }
}

/// The hard ceilings the search runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocateLimits {
    /// The deepest directory level visited; the root is depth zero.
    pub maximum_depth: u32,
    /// The largest number of directories enumerated.
    pub maximum_directories: u32,
    /// The largest number of pages taken from one directory.
    pub maximum_pages_per_directory: u32,
    /// The largest number of files classified.
    pub maximum_files: u32,
    /// The header prefix read from one candidate file.
    pub header_prefix_bytes: usize,
    /// The largest number of overlay bytes scanned in one file.
    pub overlay_scan_bytes: u64,
    /// The largest number of bytes read from the medium in total.
    pub total_read_bytes: u64,
}

impl Default for LocateLimits {
    fn default() -> Self {
        Self {
            maximum_depth: 8,
            maximum_directories: 512,
            maximum_pages_per_directory: 64,
            maximum_files: 4_096,
            header_prefix_bytes: 64 * 1024,
            overlay_scan_bytes: 64 * 1024 * 1024,
            total_read_bytes: 512 * 1024 * 1024,
        }
    }
}

/// The size of one overlay scan window.
const SCAN_WINDOW_BYTES: usize = 64 * 1024;

/// The longest signature, and therefore the overlap two consecutive windows
/// need so a signature straddling their boundary is still found.
const SIGNATURE_BYTES: usize = 4;

/// Reads the little-endian `u16` at `offset`, if it is present.
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get(offset..end)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Reads the little-endian `u32` at `offset`, if it is present.
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Whether `prefix` starts a Microsoft cabinet.
#[must_use]
pub fn is_cabinet_at_zero(prefix: &[u8]) -> bool {
    prefix.starts_with(&CABINET_SIGNATURE)
}

/// The offset of a PE image's overlay: the first byte past the last section's
/// raw data.
///
/// `prefix` must contain the whole header region — the DOS header, the PE
/// signature, the COFF header, the optional header, and the complete section
/// table. Returns `None` when `prefix` is not a PE image or the headers are
/// truncated, malformed, or claim more sections than a PE image may have.
/// A PE whose sections end exactly at the file's size has no overlay, which
/// the caller sees as an offset equal to that size.
#[must_use]
pub fn pe_overlay_offset(prefix: &[u8]) -> Option<u64> {
    if !prefix.starts_with(&DOS_SIGNATURE) {
        return None;
    }
    let e_lfanew = usize::try_from(read_u32(prefix, E_LFANEW_OFFSET)?).ok()?;
    if prefix.get(e_lfanew..e_lfanew.checked_add(PE_SIGNATURE.len())?)? != PE_SIGNATURE {
        return None;
    }

    let coff = e_lfanew.checked_add(PE_SIGNATURE.len())?;
    let section_count = read_u16(prefix, coff.checked_add(2)?)?;
    if section_count == 0 || section_count > MAXIMUM_SECTIONS {
        return None;
    }
    let optional_header_bytes = usize::from(read_u16(prefix, coff.checked_add(16)?)?);

    let table = coff
        .checked_add(COFF_HEADER_BYTES)?
        .checked_add(optional_header_bytes)?;
    // The whole table has to be inside the prefix: a partially read section
    // table could hide the section with the largest raw end.
    let table_bytes = usize::from(section_count).checked_mul(SECTION_HEADER_BYTES)?;
    let end_of_table = table.checked_add(table_bytes)?;
    if end_of_table > prefix.len() {
        return None;
    }

    let mut overlay = 0u64;
    for index in 0..usize::from(section_count) {
        let row = table.checked_add(index.checked_mul(SECTION_HEADER_BYTES)?)?;
        // Section header layout: Name[8], VirtualSize, VirtualAddress,
        // SizeOfRawData, PointerToRawData, ...
        let size_of_raw_data = u64::from(read_u32(prefix, row.checked_add(16)?)?);
        let pointer_to_raw_data = u64::from(read_u32(prefix, row.checked_add(20)?)?);
        if size_of_raw_data == 0 || pointer_to_raw_data == 0 {
            // An uninitialised-data section occupies no file bytes, so it
            // cannot bound the overlay.
            continue;
        }
        overlay = overlay.max(pointer_to_raw_data.checked_add(size_of_raw_data)?);
    }
    (overlay != 0).then_some(overlay)
}

/// The earliest container signature in `window`, and where it starts.
///
/// Ties are impossible: the two signatures cannot begin at the same offset.
#[must_use]
pub fn find_container_signature(window: &[u8]) -> Option<(usize, ContainerKind)> {
    let z = INSTALLSHIELD_Z_SIGNATURE.to_le_bytes();
    window
        .windows(SIGNATURE_BYTES)
        .enumerate()
        .find_map(|(index, candidate)| {
            if candidate == z {
                Some((index, ContainerKind::InstallShieldZ))
            } else if candidate == CABINET_SIGNATURE {
                Some((index, ContainerKind::MicrosoftCabinet))
            } else {
                None
            }
        })
}

/// A bounded random-access byte source; a seam so the window-crossing scan
/// can be tested without a mounted medium.
trait PrefixReader {
    /// Reads up to `out.len()` bytes at `offset`, returning how many.
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, SanitizedError>;
}

impl PrefixReader for MediaFile {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, SanitizedError> {
        self.seek(offset)?;
        let mut filled = 0usize;
        while filled < out.len() {
            match self.read(&mut out[filled..])? {
                0 => break,
                count => filled += count,
            }
        }
        Ok(filled)
    }
}

/// The running byte budget shared by every read the search makes.
struct ReadBudget {
    remaining: u64,
}

impl ReadBudget {
    /// Shrinks `wanted` to what the budget still allows, charging the result.
    fn take(&mut self, wanted: u64) -> u64 {
        let granted = wanted.min(self.remaining);
        self.remaining -= granted;
        granted
    }

    fn exhausted(&self) -> bool {
        self.remaining == 0
    }
}

/// Scans `[start, start + span)` of `reader` for the first container
/// signature, in bounded windows that overlap by the signature length.
fn scan_for_signature<R: PrefixReader>(
    reader: &mut R,
    start: u64,
    span: u64,
    budget: &mut ReadBudget,
    cancellation: &CancellationToken,
) -> Result<Option<(u64, ContainerKind)>, SanitizedError> {
    let mut buffer = vec![0u8; SCAN_WINDOW_BYTES];
    let mut position = start;
    let end = start.saturating_add(span);
    while position < end {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        let wanted = (end - position).min(SCAN_WINDOW_BYTES as u64);
        let granted = budget.take(wanted);
        if granted == 0 {
            return Ok(None);
        }
        let length = usize::try_from(granted).unwrap_or(SCAN_WINDOW_BYTES);
        let filled = reader.read_at(position, &mut buffer[..length])?;
        if filled < SIGNATURE_BYTES {
            return Ok(None);
        }
        if let Some((index, kind)) = find_container_signature(&buffer[..filled]) {
            return Ok(Some((position.saturating_add(index as u64), kind)));
        }
        if filled < length {
            return Ok(None);
        }
        // Step back by the signature length so a signature straddling the
        // window boundary is still seen whole.
        position = position.saturating_add((filled - (SIGNATURE_BYTES - 1)) as u64);
    }
    Ok(None)
}

/// Classifies one file, reading at most its header prefix and its overlay.
fn classify_file<R: PrefixReader>(
    reader: &mut R,
    size: u64,
    limits: &LocateLimits,
    budget: &mut ReadBudget,
    cancellation: &CancellationToken,
) -> Result<Option<(ContainerKind, u64)>, SanitizedError> {
    let prefix_len = u64::try_from(limits.header_prefix_bytes)
        .unwrap_or(u64::MAX)
        .min(size);
    let granted = budget.take(prefix_len);
    let Ok(granted) = usize::try_from(granted) else {
        return Ok(None);
    };
    if granted < SIGNATURE_BYTES {
        return Ok(None);
    }
    let mut prefix = vec![0u8; granted];
    let filled = reader.read_at(0, &mut prefix)?;
    prefix.truncate(filled);

    if is_cabinet_at_zero(&prefix) {
        return Ok(Some((ContainerKind::MicrosoftCabinet, 0)));
    }
    let Some(overlay) = pe_overlay_offset(&prefix) else {
        return Ok(None);
    };
    if overlay >= size {
        // Sections fill the file: there is no overlay to search.
        return Ok(None);
    }
    let span = (size - overlay).min(limits.overlay_scan_bytes);
    let found = scan_for_signature(reader, overlay, span, budget, cancellation)?;
    Ok(found.map(|(offset, kind)| (kind, offset)))
}

/// One pending directory in the bounded walk.
struct Pending {
    /// The normalized directory path, `/` for the root.
    path: String,
    depth: u32,
}

/// Appends `name` to the normalized directory path `parent`.
fn join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Walks `mount` and returns every container candidate it recognised.
///
/// The result is deterministic and independent of the medium's directory
/// order: candidates are sorted by kind, then by normalized path, then by
/// offset. Reaching a [`LocateLimits`] ceiling or an observed cancellation
/// truncates the search and returns what was found so far.
///
/// # Errors
/// A sanitized failure from the mount itself. An unreadable individual file
/// is not an error: it is skipped, because one damaged file must not stop the
/// search.
pub fn locate_containers(
    mount: &Mount,
    limits: &LocateLimits,
    cancellation: &CancellationToken,
) -> Result<Vec<ContainerCandidate>, SanitizedError> {
    let mut budget = ReadBudget {
        remaining: limits.total_read_bytes,
    };
    let mut found = Vec::new();
    let mut queue = vec![Pending {
        path: String::from("/"),
        depth: 0,
    }];
    let mut directories = 0u32;
    let mut files = 0u32;

    while let Some(pending) = queue.pop() {
        if cancellation.is_cancelled() || budget.exhausted() {
            break;
        }
        if directories >= limits.maximum_directories {
            break;
        }
        directories += 1;

        let mut page: DirectoryPage = mount.list_page(&pending.path)?;
        let mut pages = 1u32;
        loop {
            for entry in &page.entries {
                match entry.entry_type {
                    EntryType::Directory => {
                        if pending.depth < limits.maximum_depth {
                            queue.push(Pending {
                                path: join(&pending.path, &entry.name),
                                depth: pending.depth + 1,
                            });
                        }
                    }
                    EntryType::File => {
                        if entry.size_bytes < MINIMUM_CANDIDATE_BYTES
                            || files >= limits.maximum_files
                            || budget.exhausted()
                        {
                            continue;
                        }
                        files += 1;
                        let path = join(&pending.path, &entry.name);
                        // An unreadable or unclassifiable file is simply not
                        // a candidate.
                        let Ok(mut file) = mount.open_file(&path) else {
                            continue;
                        };
                        let classified = classify_file(
                            &mut file,
                            entry.size_bytes,
                            limits,
                            &mut budget,
                            cancellation,
                        );
                        let Ok(Some((kind, offset))) = classified else {
                            continue;
                        };
                        let Some(normalized) = normalize_path(&path) else {
                            continue;
                        };
                        found.push(ContainerCandidate {
                            archive_path: NormalizedPath::from_normalized(normalized),
                            kind,
                            offset,
                            length: entry.size_bytes.saturating_sub(offset),
                        });
                    }
                    EntryType::Unknown => {}
                }
            }
            let Some(cursor) = page.cursor.take() else {
                break;
            };
            if pages >= limits.maximum_pages_per_directory {
                break;
            }
            pages += 1;
            page = mount.continue_list(cursor)?;
        }
    }

    found.sort_unstable_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.archive_path.cmp(&right.archive_path))
            .then_with(|| left.offset.cmp(&right.offset))
    });
    found.dedup_by(|left, right| {
        left.kind == right.kind
            && left.archive_path == right.archive_path
            && left.offset == right.offset
    });
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::{
        ContainerKind, INSTALLSHIELD_Z_SIGNATURE, LocateLimits, PrefixReader, ReadBudget,
        SCAN_WINDOW_BYTES, classify_file, scan_for_signature,
    };
    use crate::io::CancellationToken;
    use ohl_core::SanitizedError;

    struct SliceReader<'a>(&'a [u8]);

    impl PrefixReader for SliceReader<'_> {
        fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, SanitizedError> {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(self.0.len());
            let available = &self.0[start..];
            let count = available.len().min(out.len());
            out[..count].copy_from_slice(&available[..count]);
            Ok(count)
        }
    }

    fn budget() -> ReadBudget {
        ReadBudget {
            remaining: u64::MAX,
        }
    }

    #[test]
    fn a_signature_across_a_window_boundary_is_still_found() {
        // Straddles the first window's last byte, which is exactly the case
        // the overlap exists for.
        let at = SCAN_WINDOW_BYTES - 2;
        let mut bytes = vec![0u8; SCAN_WINDOW_BYTES * 2];
        bytes[at..at + 4].copy_from_slice(&INSTALLSHIELD_Z_SIGNATURE.to_le_bytes());
        let mut reader = SliceReader(&bytes);
        let found = scan_for_signature(
            &mut reader,
            0,
            bytes.len() as u64,
            &mut budget(),
            &CancellationToken::default(),
        )
        .expect("scan");
        assert_eq!(found, Some((at as u64, ContainerKind::InstallShieldZ)));
    }

    #[test]
    fn a_read_budget_stops_the_scan_without_failing() {
        let mut bytes = vec![0u8; SCAN_WINDOW_BYTES * 2];
        let at = SCAN_WINDOW_BYTES + 16;
        bytes[at..at + 4].copy_from_slice(&INSTALLSHIELD_Z_SIGNATURE.to_le_bytes());
        let mut reader = SliceReader(&bytes);
        let mut budget = ReadBudget {
            remaining: SCAN_WINDOW_BYTES as u64,
        };
        let found = scan_for_signature(
            &mut reader,
            0,
            bytes.len() as u64,
            &mut budget,
            &CancellationToken::default(),
        )
        .expect("scan");
        assert_eq!(found, None);
    }

    #[test]
    fn a_cabinet_at_offset_zero_is_classified_without_a_pe_walk() {
        let mut bytes = vec![0u8; 128];
        bytes[..4].copy_from_slice(b"MSCF");
        let mut reader = SliceReader(&bytes);
        let found = classify_file(
            &mut reader,
            bytes.len() as u64,
            &LocateLimits::default(),
            &mut budget(),
            &CancellationToken::default(),
        )
        .expect("classify");
        assert_eq!(found, Some((ContainerKind::MicrosoftCabinet, 0)));
    }
}
