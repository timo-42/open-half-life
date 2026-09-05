//! The bounded result catalog and the layout planner that fills it.
//!
//! Port of the C++ `media::PayloadLayout`/`plan_payload_layout` pair, reduced
//! to what the parent needs to promote a worker enumeration: every
//! archive-controlled value is validated *before* any destination could be
//! opened, and the promoted catalog is keyed by a [`CatalogGeneration`] — the
//! `(worker_epoch, enumeration)` identity that never comes from the worker.
//!
//! The untrusted spelling ([`ohl_parser_protocol::ArchiveSpelling`]) and the
//! validated result ([`NormalizedPath`]) are distinct types, and
//! [`NormalizedPath`] deliberately offers no `Deref` to `Path`: a catalog
//! entry cannot be handed to the filesystem by accident.

use std::collections::BTreeSet;
use std::num::NonZeroU64;

use ohl_parser_protocol::messages::{
    MAXIMUM_ENUMERATED_ENTRIES, MAXIMUM_ENUMERATED_ENTRY_BYTES, MAXIMUM_ENUMERATED_PATH_BYTES,
    MAXIMUM_ENUMERATED_TOTAL_BYTES,
};
use thiserror::Error;

/// An opaque worker-assigned entry identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceToken(pub u64);

/// The trusted, parent-assigned identity of one worker lifetime.
///
/// It must be unique across every worker/session lifetime in which a catalog
/// handle could still be reachable; [`crate::SessionIdAllocator`] guarantees
/// that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerEpoch(NonZeroU64);

impl WorkerEpoch {
    /// Wraps a non-zero epoch.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The epoch value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// The identity of one promoted catalog.
///
/// A stream request must present this exact value, so a stale catalog user is
/// rejected even when a restarted worker reuses the same source token and
/// enumeration number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CatalogGeneration {
    epoch: WorkerEpoch,
    enumeration: NonZeroU64,
}

impl CatalogGeneration {
    pub(crate) const fn new(epoch: WorkerEpoch, enumeration: NonZeroU64) -> Self {
        Self { epoch, enumeration }
    }

    /// The worker epoch this catalog was promoted in.
    #[must_use]
    pub const fn epoch(self) -> WorkerEpoch {
        self.epoch
    }

    /// The enumeration sequence number within that epoch.
    #[must_use]
    pub const fn enumeration(self) -> u64 {
        self.enumeration.get()
    }
}

/// A path that passed the shared media-path rules.
///
/// Built only by the layout planner. There is deliberately no `Deref`, no
/// `AsRef<Path>` and no public constructor from a string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NormalizedPath(String);

impl NormalizedPath {
    /// The normalized, absolute, separator-canonical spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps a spelling that already passed [`ohl_vfs::normalize_path`].
    ///
    /// Crate-internal on purpose: the invariant is "this string is the
    /// output of the shared normalizer", and only code that just called it
    /// can promise that.
    pub(crate) const fn from_normalized(path: String) -> Self {
        Self(path)
    }
}

/// One entry as the worker described it, before planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMetadata {
    /// The worker-assigned token.
    pub source_token: SourceToken,
    /// The untrusted archive spelling.
    pub archive_path: String,
    /// The declared size in bytes.
    pub size_bytes: u64,
}

/// One planned, validated catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEntry {
    source_token: SourceToken,
    relative_path: NormalizedPath,
    size_bytes: u64,
}

impl PlannedEntry {
    /// The worker-assigned token.
    #[must_use]
    pub const fn source_token(&self) -> SourceToken {
        self.source_token
    }

    /// The validated path.
    #[must_use]
    pub const fn relative_path(&self) -> &NormalizedPath {
        &self.relative_path
    }

    /// The declared size in bytes.
    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// Cumulative quotas applied to one enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportLimits {
    entries: u32,
    path_bytes: u64,
    entry_bytes: u64,
    total_bytes: u64,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            entries: MAXIMUM_ENUMERATED_ENTRIES,
            path_bytes: MAXIMUM_ENUMERATED_PATH_BYTES,
            entry_bytes: MAXIMUM_ENUMERATED_ENTRY_BYTES,
            total_bytes: MAXIMUM_ENUMERATED_TOTAL_BYTES,
        }
    }
}

impl ImportLimits {
    /// Validates one set of quotas.
    ///
    /// # Errors
    /// [`LayoutError::InvalidLimits`] for a zero quota, a quota above the
    /// protocol ceiling, or a per-entry ceiling above the total ceiling.
    pub const fn new(
        maximum_entries: u32,
        maximum_path_bytes: u64,
        maximum_entry_bytes: u64,
        maximum_total_bytes: u64,
    ) -> Result<Self, LayoutError> {
        if maximum_entries == 0
            || maximum_entries > MAXIMUM_ENUMERATED_ENTRIES
            || maximum_path_bytes == 0
            || maximum_path_bytes > MAXIMUM_ENUMERATED_PATH_BYTES
            || maximum_entry_bytes == 0
            || maximum_entry_bytes > MAXIMUM_ENUMERATED_ENTRY_BYTES
            || maximum_total_bytes == 0
            || maximum_total_bytes > MAXIMUM_ENUMERATED_TOTAL_BYTES
            || maximum_entry_bytes > maximum_total_bytes
        {
            return Err(LayoutError::InvalidLimits);
        }
        Ok(Self {
            entries: maximum_entries,
            path_bytes: maximum_path_bytes,
            entry_bytes: maximum_entry_bytes,
            total_bytes: maximum_total_bytes,
        })
    }

    /// The entry-count quota.
    #[must_use]
    pub const fn maximum_entries(self) -> u32 {
        self.entries
    }

    /// The cumulative path-byte quota.
    #[must_use]
    pub const fn maximum_path_bytes(self) -> u64 {
        self.path_bytes
    }

    /// The per-entry byte ceiling.
    #[must_use]
    pub const fn maximum_entry_bytes(self) -> u64 {
        self.entry_bytes
    }

    /// The cumulative byte quota.
    #[must_use]
    pub const fn maximum_total_bytes(self) -> u64 {
        self.total_bytes
    }
}

/// Every way an enumeration can fail validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum LayoutError {
    /// The supplied quotas are not usable.
    #[error("invalid import limits")]
    InvalidLimits,
    /// More entries than the quota allows.
    #[error("too many entries")]
    TooManyEntries,
    /// More cumulative path bytes than the quota allows.
    #[error("too many path bytes")]
    TooManyPathBytes,
    /// Two entries claimed the same source token.
    #[error("source token conflict")]
    SourceTokenConflict,
    /// A spelling violated the shared media-path rules.
    #[error("invalid archive path")]
    InvalidPath,
    /// Two entries resolve to the same or to aliasing paths.
    #[error("archive path conflict")]
    PathConflict,
    /// One entry exceeds the per-entry ceiling.
    #[error("entry too large")]
    EntryTooLarge,
    /// The enumeration exceeds the cumulative byte quota.
    #[error("payload too large")]
    PayloadTooLarge,
}

/// A promoted, read-only catalog.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: Vec<PlannedEntry>,
    by_token: Vec<(SourceToken, usize)>,
    total_bytes: u64,
}

impl Catalog {
    /// The planned entries, in enumeration order.
    #[must_use]
    pub fn entries(&self) -> &[PlannedEntry] {
        &self.entries
    }

    /// The sum of every entry's declared size.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Looks one entry up by its opaque token.
    #[must_use]
    pub fn find(&self, token: SourceToken) -> Option<&PlannedEntry> {
        let index = self
            .by_token
            .binary_search_by_key(&token, |(token, _)| *token)
            .ok()?;
        self.entries.get(self.by_token[index].1)
    }
}

/// A read-only view of a promoted catalog, tagged with its generation.
///
/// The view borrows storage owned by the result session: any new enumeration,
/// cancellation, failure, source invalidation, shutdown or worker retirement
/// invalidates it — which the borrow checker enforces here rather than leaving
/// to a comment.
#[derive(Debug, Clone, Copy)]
pub struct CatalogView<'session> {
    generation: CatalogGeneration,
    catalog: &'session Catalog,
}

impl<'session> CatalogView<'session> {
    pub(crate) const fn new(generation: CatalogGeneration, catalog: &'session Catalog) -> Self {
        Self {
            generation,
            catalog,
        }
    }

    /// The identity a later stream request must present.
    #[must_use]
    pub const fn generation(self) -> CatalogGeneration {
        self.generation
    }

    /// The planned entries.
    #[must_use]
    pub const fn entries(self) -> &'session [PlannedEntry] {
        self.catalog.entries.as_slice()
    }

    /// The sum of every entry's declared size.
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        self.catalog.total_bytes
    }

    /// Looks one entry up by its opaque token.
    #[must_use]
    pub fn find(self, token: SourceToken) -> Option<&'session PlannedEntry> {
        self.catalog.find(token)
    }
}

/// Validates every archive-controlled value and plans the catalog.
///
/// Conflicts include exact duplicates, ASCII case-only aliases, and file
/// versus directory prefix aliases.
///
/// # Errors
/// The first [`LayoutError`] any entry violates.
pub fn plan_catalog(
    entries: &[EntryMetadata],
    limits: ImportLimits,
) -> Result<Catalog, LayoutError> {
    if entries.len() > limits.entries as usize {
        return Err(LayoutError::TooManyEntries);
    }

    let mut planned = Vec::with_capacity(entries.len());
    let mut tokens = BTreeSet::new();
    let mut folded: BTreeSet<String> = BTreeSet::new();
    let mut path_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;

    for entry in entries {
        if !tokens.insert(entry.source_token) {
            return Err(LayoutError::SourceTokenConflict);
        }
        if entry.size_bytes > limits.entry_bytes {
            return Err(LayoutError::EntryTooLarge);
        }
        total_bytes = total_bytes
            .checked_add(entry.size_bytes)
            .filter(|total| *total <= limits.total_bytes)
            .ok_or(LayoutError::PayloadTooLarge)?;
        path_bytes = path_bytes
            .checked_add(entry.archive_path.len() as u64)
            .filter(|bytes| *bytes <= limits.path_bytes)
            .ok_or(LayoutError::TooManyPathBytes)?;

        let normalized =
            ohl_vfs::normalize_path(&entry.archive_path).ok_or(LayoutError::InvalidPath)?;
        if normalized == "/" {
            return Err(LayoutError::InvalidPath);
        }
        if !folded.insert(normalized.to_ascii_lowercase()) {
            return Err(LayoutError::PathConflict);
        }
        planned.push(PlannedEntry {
            source_token: entry.source_token,
            relative_path: NormalizedPath(normalized),
            size_bytes: entry.size_bytes,
        });
    }

    // A file may not alias a directory prefix of another entry. The folded set
    // is sorted, so an alias is always an immediate neighbour.
    let mut previous: Option<&String> = None;
    for path in &folded {
        if let Some(previous) = previous
            && path.len() > previous.len()
            && path.starts_with(previous.as_str())
            && path.as_bytes()[previous.len()] == b'/'
        {
            return Err(LayoutError::PathConflict);
        }
        previous = Some(path);
    }

    let mut by_token: Vec<(SourceToken, usize)> = planned
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.source_token, index))
        .collect();
    by_token.sort_unstable_by_key(|(token, _)| *token);

    Ok(Catalog {
        entries: planned,
        by_token,
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        EntryMetadata, ImportLimits, LayoutError, PlannedEntry, SourceToken, plan_catalog,
    };

    fn entry(token: u64, path: &str, size: u64) -> EntryMetadata {
        EntryMetadata {
            source_token: SourceToken(token),
            archive_path: path.to_owned(),
            size_bytes: size,
        }
    }

    #[test]
    fn a_canonical_enumeration_plans_and_indexes() {
        let catalog = plan_catalog(
            &[
                entry(7, "valve/halflife.wad", 10),
                entry(3, "\\valve\\pak0.pak", 5),
            ],
            ImportLimits::default(),
        )
        .expect("canonical entries plan");
        assert_eq!(catalog.total_bytes(), 15);
        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(
            catalog.entries()[1].relative_path().as_str(),
            "/valve/pak0.pak"
        );
        assert_eq!(
            catalog.find(SourceToken(7)).map(PlannedEntry::size_bytes),
            Some(10)
        );
        assert!(catalog.find(SourceToken(9)).is_none());
    }

    #[test]
    fn conflicting_and_out_of_budget_entries_are_rejected() {
        let limits = ImportLimits::default();
        for (entries, expected) in [
            (
                vec![entry(1, "a", 1), entry(1, "b", 1)],
                LayoutError::SourceTokenConflict,
            ),
            (
                vec![entry(1, "a", 1), entry(2, "A", 1)],
                LayoutError::PathConflict,
            ),
            (
                vec![entry(1, "a", 1), entry(2, "a/b", 1)],
                LayoutError::PathConflict,
            ),
            (vec![entry(1, "../a", 1)], LayoutError::InvalidPath),
            (vec![entry(1, "/", 1)], LayoutError::InvalidPath),
            (vec![entry(1, "a", u64::MAX)], LayoutError::EntryTooLarge),
        ] {
            assert_eq!(plan_catalog(&entries, limits).unwrap_err(), expected);
        }
    }

    #[test]
    fn quotas_bound_entries_paths_and_bytes() {
        let limits = ImportLimits::new(1, 8, 4, 8).expect("valid limits");
        assert_eq!(
            plan_catalog(&[entry(1, "a", 1), entry(2, "b", 1)], limits).unwrap_err(),
            LayoutError::TooManyEntries
        );
        assert_eq!(
            plan_catalog(&[entry(1, "abcdefghi", 1)], limits).unwrap_err(),
            LayoutError::TooManyPathBytes
        );
        assert_eq!(
            plan_catalog(&[entry(1, "a", 5)], limits).unwrap_err(),
            LayoutError::EntryTooLarge
        );
        assert_eq!(
            ImportLimits::new(0, 8, 4, 8),
            Err(LayoutError::InvalidLimits)
        );
        assert_eq!(
            ImportLimits::new(1, 8, 9, 8),
            Err(LayoutError::InvalidLimits)
        );
    }
}
