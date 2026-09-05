//! Layout planning: typed entry metadata to a bounded, ordered destination plan.
//!
//! Planning is the last point at which a payload is still only data. It runs
//! before any destination is opened, so every archive-controlled quantity —
//! entry count, path bytes, entry size, total size — is checked against
//! [`PayloadImportLimits`] while rejecting costs nothing, and every ambiguity
//! is resolved before a single native call could act on it.
//!
//! Three families of conflict are refused, all of them things that a
//! filesystem would silently resolve for us in the wrong direction:
//!
//! - an exact duplicate destination;
//! - a *case-only alias*, which is two distinct names on Linux and one name on
//!   Windows and macOS (`Alpha/One` versus `ALPHA/ONE`);
//! - a *file/directory alias*, where one entry's destination is another
//!   entry's parent directory (`Leaf` and `Leaf/child`), including when the
//!   two spellings differ only by case.
//!
//! A duplicate source token is refused too: tokens are the opaque handles a
//! [`crate::stream::PayloadSource`] is later asked to stream, and two entries
//! claiming one token means the plan does not describe what it will produce.
//!
//! The accepted plan is sorted by [`PayloadPath::portability_key`], so the
//! same input set always yields the same order — the property staging identity
//! and cache reuse both depend on.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::path::{PayloadPath, PayloadPathError};

/// One archive-controlled entry offered for planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadEntryMetadata {
    /// The opaque, transport-local handle the source will be asked to stream.
    pub source_token: u64,
    /// The archive-relative name, before any normalisation.
    pub archive_path: String,
    /// The size the archive declares for this entry.
    pub size_bytes: u64,
}

/// The bounded resource envelope a plan may consume.
///
/// Every field is a hard ceiling, and the defaults are the C++ ones. A limit
/// of zero is meaningless rather than restrictive, and an entry ceiling above
/// the total ceiling is self-contradictory; both are
/// [`PayloadLayoutError::InvalidLimits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadImportLimits {
    /// The largest accepted number of entries.
    pub maximum_entries: usize,
    /// The largest accepted sum of unnormalised archive path lengths.
    pub maximum_path_bytes: u64,
    /// The largest accepted single entry.
    pub maximum_entry_bytes: u64,
    /// The largest accepted sum of entry sizes.
    pub maximum_total_bytes: u64,
}

impl Default for PayloadImportLimits {
    fn default() -> Self {
        Self {
            maximum_entries: 50_000,
            maximum_path_bytes: 64 * 1_024 * 1_024,
            maximum_entry_bytes: 8 * 1_024 * 1_024 * 1_024,
            maximum_total_bytes: 32 * 1_024 * 1_024 * 1_024,
        }
    }
}

impl PayloadImportLimits {
    /// Whether the limits describe a satisfiable envelope.
    pub(crate) const fn coherent(&self) -> bool {
        self.maximum_entries != 0
            && self.maximum_path_bytes != 0
            && self.maximum_entry_bytes != 0
            && self.maximum_total_bytes != 0
            && self.maximum_entry_bytes <= self.maximum_total_bytes
    }
}

/// Why a set of entries may not become a destination plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadLayoutError {
    /// The supplied limits are not a satisfiable envelope.
    InvalidLimits,
    /// More entries than [`PayloadImportLimits::maximum_entries`].
    TooManyEntries,
    /// The accumulated archive path bytes exceeded the ceiling.
    TooManyPathBytes,
    /// Two entries claimed the same source token.
    SourceTokenConflict,
    /// An archive path is not a legal payload path.
    InvalidPath(PayloadPathError),
    /// Two entries would resolve to one destination, exactly, by ASCII case,
    /// or as a file against a directory.
    PathConflict,
    /// One entry exceeded [`PayloadImportLimits::maximum_entry_bytes`].
    EntryTooLarge,
    /// The accumulated entry sizes exceeded the ceiling.
    PayloadTooLarge,
}

impl PayloadLayoutError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidLimits => "payload import limits are not satisfiable",
            Self::TooManyEntries => "payload has more entries than the accepted limit",
            Self::TooManyPathBytes => "payload path bytes exceed the accepted limit",
            Self::SourceTokenConflict => "payload entries share one source token",
            Self::InvalidPath(_) => "payload entry path is not a legal destination",
            Self::PathConflict => "payload entries resolve to one destination",
            Self::EntryTooLarge => "payload entry exceeds the accepted size limit",
            Self::PayloadTooLarge => "payload exceeds the accepted total size limit",
        }
    }
}

impl core::fmt::Display for PayloadLayoutError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for PayloadLayoutError {}

impl From<PayloadLayoutError> for ohl_core::SanitizedError {
    fn from(_: PayloadLayoutError) -> Self {
        Self::InvalidInput
    }
}

/// A refused plan: the rule that refused it, and which entry tripped it.
///
/// `entry_index` indexes the *input* slice, not the sorted plan, and is
/// `None` for a whole-set rule such as [`PayloadLayoutError::InvalidLimits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadLayoutRejection {
    /// The rule that refused the set.
    pub error: PayloadLayoutError,
    /// The offending input index, when one entry is responsible.
    pub entry_index: Option<usize>,
}

impl core::fmt::Display for PayloadLayoutRejection {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.error, formatter)
    }
}

impl core::error::Error for PayloadLayoutRejection {}

impl From<PayloadLayoutRejection> for ohl_core::SanitizedError {
    fn from(rejection: PayloadLayoutRejection) -> Self {
        rejection.error.into()
    }
}

/// One accepted destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPayloadEntry {
    /// The opaque handle to stream.
    pub source_token: u64,
    /// The validated destination.
    pub path: PayloadPath,
    /// The declared size, which streaming must match exactly.
    pub size_bytes: u64,
}

/// An accepted, ordered, bounded destination plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadLayout {
    /// Entries in ascending [`PayloadPath::portability_key`] order.
    entries: Vec<PlannedPayloadEntry>,
    /// The exact sum of the entries' declared sizes.
    total_bytes: u64,
}

impl PayloadLayout {
    /// The planned entries, in their deterministic order.
    pub fn entries(&self) -> &[PlannedPayloadEntry] {
        &self.entries
    }

    /// The exact sum of the planned entries' declared sizes.
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// The number of planned entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the plan has no entries. An empty payload is legal.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Consumes the plan and returns its entries.
    pub fn into_entries(self) -> Vec<PlannedPayloadEntry> {
        self.entries
    }
}

/// Validates every archive-controlled quantity and produces the ordered plan.
///
/// # Errors
///
/// See [`PayloadLayoutError`]. Entries are examined in input order and the
/// first refusal wins, so the reported index is the earliest offender.
pub fn plan_payload_layout(
    entries: &[PayloadEntryMetadata],
    limits: &PayloadImportLimits,
) -> Result<PayloadLayout, PayloadLayoutRejection> {
    let whole_set = |error| PayloadLayoutRejection {
        error,
        entry_index: None,
    };
    if !limits.coherent() {
        return Err(whole_set(PayloadLayoutError::InvalidLimits));
    }
    if entries.len() > limits.maximum_entries {
        return Err(whole_set(PayloadLayoutError::TooManyEntries));
    }

    let mut source_tokens = BTreeSet::new();
    let mut file_keys = BTreeSet::<String>::new();
    // Case-folded ancestor key to the one spelling that ancestor may have.
    let mut directory_spellings = BTreeMap::<String, String>::new();
    let mut planned = Vec::with_capacity(entries.len());
    let mut total_bytes = 0u64;
    let mut total_path_bytes = 0u64;

    for (index, source) in entries.iter().enumerate() {
        let at = |error| PayloadLayoutRejection {
            error,
            entry_index: Some(index),
        };
        if !source_tokens.insert(source.source_token) {
            return Err(at(PayloadLayoutError::SourceTokenConflict));
        }
        // Each accumulator is compared against its remaining headroom rather
        // than summed first, so no check can be defeated by an overflow.
        let path_bytes = u64::try_from(source.archive_path.len()).unwrap_or(u64::MAX);
        if path_bytes > limits.maximum_path_bytes - total_path_bytes {
            return Err(at(PayloadLayoutError::TooManyPathBytes));
        }
        if source.size_bytes > limits.maximum_entry_bytes {
            return Err(at(PayloadLayoutError::EntryTooLarge));
        }
        if source.size_bytes > limits.maximum_total_bytes - total_bytes {
            return Err(at(PayloadLayoutError::PayloadTooLarge));
        }

        let path = PayloadPath::parse(&source.archive_path)
            .map_err(|error| at(PayloadLayoutError::InvalidPath(error)))?;
        if file_keys.contains(path.portability_key())
            || directory_spellings.contains_key(path.portability_key())
        {
            return Err(at(PayloadLayoutError::PathConflict));
        }
        for (ancestor_key, ancestor_spelling) in path.ancestor_keys() {
            if file_keys.contains(ancestor_key) {
                return Err(at(PayloadLayoutError::PathConflict));
            }
            match directory_spellings.get(ancestor_key) {
                Some(existing) if existing != ancestor_spelling => {
                    return Err(at(PayloadLayoutError::PathConflict));
                }
                Some(_) => {}
                None => {
                    directory_spellings
                        .insert(String::from(ancestor_key), String::from(ancestor_spelling));
                }
            }
        }
        file_keys.insert(String::from(path.portability_key()));
        total_bytes += source.size_bytes;
        total_path_bytes += path_bytes;
        planned.push(PlannedPayloadEntry {
            source_token: source.source_token,
            path,
            size_bytes: source.size_bytes,
        });
    }

    // Keys are unique by construction, so this order is total and stable.
    planned.sort_by(|first, second| {
        first
            .path
            .portability_key()
            .cmp(second.path.portability_key())
    });

    Ok(PayloadLayout {
        entries: planned,
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PayloadEntryMetadata, PayloadImportLimits, PayloadLayoutError, plan_payload_layout,
    };
    use crate::path::{PayloadPath, PayloadPathError};
    use alloc::string::{String, ToString as _};
    use alloc::vec;
    use alloc::vec::Vec;

    fn entry(token: u64, path: &str, size: u64) -> PayloadEntryMetadata {
        PayloadEntryMetadata {
            source_token: token,
            archive_path: String::from(path),
            size_bytes: size,
        }
    }

    fn refuses(
        entries: &[PayloadEntryMetadata],
        limits: &PayloadImportLimits,
    ) -> PayloadLayoutError {
        plan_payload_layout(entries, limits)
            .expect_err("refused")
            .error
    }

    #[test]
    fn a_valid_set_is_planned_in_deterministic_order() {
        let entries = vec![
            entry(2, "ProjectFixture/Tiles/AmberBlob.dat", 2_048),
            entry(3, "ProjectFixture\\Settings.note", 512),
            entry(1, "AuthoredManifest.note", 0),
        ];
        let planned =
            plan_payload_layout(&entries, &PayloadImportLimits::default()).expect("planned");
        assert_eq!(planned.len(), 3);
        assert!(!planned.is_empty());
        assert_eq!(planned.total_bytes(), 2_560);
        assert_eq!(planned.entries()[0].source_token, 1);
        assert_eq!(
            planned.entries()[1].path.as_str(),
            "ProjectFixture/Settings.note"
        );
        assert_eq!(
            planned.entries()[2].path.as_str(),
            "ProjectFixture/Tiles/AmberBlob.dat"
        );
    }

    #[test]
    fn the_order_does_not_depend_on_the_input_order() {
        let forward = vec![
            entry(1, "Alpha/one.bin", 1),
            entry(2, "Beta/two.bin", 2),
            entry(3, "alphabet.bin", 3),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();
        let first = plan_payload_layout(&forward, &PayloadImportLimits::default()).expect("plan");
        let second = plan_payload_layout(&reversed, &PayloadImportLimits::default()).expect("plan");
        assert_eq!(first, second);
        assert_eq!(
            first
                .entries()
                .iter()
                .map(|planned| planned.path.as_str())
                .collect::<Vec<_>>(),
            ["Alpha/one.bin", "alphabet.bin", "Beta/two.bin"]
        );
    }

    #[test]
    fn an_empty_set_is_a_legal_empty_plan() {
        let planned = plan_payload_layout(&[], &PayloadImportLimits::default()).expect("planned");
        assert!(planned.is_empty());
        assert_eq!(planned.total_bytes(), 0);
        assert!(planned.into_entries().is_empty());
    }

    #[test]
    fn unsafe_or_ambiguous_sets_are_refused() {
        let limits = PayloadImportLimits::default();
        assert_eq!(
            refuses(&[entry(1, "../escape", 1)], &limits),
            PayloadLayoutError::InvalidPath(PayloadPathError::Traversal)
        );
        for ambiguous in [
            vec![
                entry(1, "SyntheticBranch/Alpha.item", 1),
                entry(2, "SYNTHETICBRANCH/ALPHA.ITEM", 1),
            ],
            vec![
                entry(1, "FixtureRoot/alpha", 1),
                entry(2, "fixtureroot/beta", 1),
            ],
            vec![entry(1, "FixtureLeaf", 1), entry(2, "FixtureLeaf/child", 1)],
            vec![entry(1, "FixtureTree/child", 1), entry(2, "FIXTURETREE", 1)],
            vec![entry(1, "Exact/Same", 1), entry(2, "Exact/Same", 1)],
        ] {
            assert_eq!(
                refuses(&ambiguous, &limits),
                PayloadLayoutError::PathConflict
            );
        }
    }

    #[test]
    fn a_consistent_shared_directory_spelling_is_accepted() {
        let entries = vec![
            entry(1, "SharedRoot/first.bin", 1),
            entry(2, "SharedRoot/second.bin", 2),
            entry(3, "SharedRoot\\third.bin", 3),
        ];
        let planned =
            plan_payload_layout(&entries, &PayloadImportLimits::default()).expect("planned");
        assert_eq!(planned.len(), 3);
        assert_eq!(planned.total_bytes(), 6);
    }

    #[test]
    fn every_resource_limit_is_enforced_and_reports_its_entry() {
        let limits = PayloadImportLimits {
            maximum_entries: 2,
            maximum_path_bytes: 10,
            maximum_entry_bytes: 10,
            maximum_total_bytes: 15,
        };
        assert_eq!(
            refuses(
                &[entry(1, "a", 1), entry(2, "b", 1), entry(3, "c", 1)],
                &limits
            ),
            PayloadLayoutError::TooManyEntries
        );
        let rejection =
            plan_payload_layout(&[entry(1, "123456", 1), entry(2, "12345", 1)], &limits)
                .expect_err("refused");
        assert_eq!(rejection.error, PayloadLayoutError::TooManyPathBytes);
        assert_eq!(rejection.entry_index, Some(1));
        assert_eq!(
            refuses(&[entry(1, "a", 1), entry(1, "b", 1)], &limits),
            PayloadLayoutError::SourceTokenConflict
        );
        assert_eq!(
            refuses(&[entry(1, "a", 11)], &limits),
            PayloadLayoutError::EntryTooLarge
        );
        assert_eq!(
            refuses(&[entry(1, "a", 8), entry(2, "b", 8)], &limits),
            PayloadLayoutError::PayloadTooLarge
        );
    }

    #[test]
    fn a_size_near_the_ceiling_cannot_overflow_the_accumulator() {
        let limits = PayloadImportLimits {
            maximum_entries: 4,
            maximum_path_bytes: 64,
            maximum_entry_bytes: u64::MAX,
            maximum_total_bytes: u64::MAX,
        };
        assert_eq!(
            refuses(&[entry(1, "a", u64::MAX), entry(2, "b", 1)], &limits),
            PayloadLayoutError::PayloadTooLarge
        );
    }

    #[test]
    fn incoherent_limits_are_refused_before_any_entry() {
        for limits in [
            PayloadImportLimits {
                maximum_entries: 1,
                maximum_path_bytes: 1,
                maximum_entry_bytes: 2,
                maximum_total_bytes: 1,
            },
            PayloadImportLimits {
                maximum_entries: 0,
                ..PayloadImportLimits::default()
            },
            PayloadImportLimits {
                maximum_path_bytes: 0,
                ..PayloadImportLimits::default()
            },
            PayloadImportLimits {
                maximum_entry_bytes: 0,
                ..PayloadImportLimits::default()
            },
            PayloadImportLimits {
                maximum_total_bytes: 0,
                ..PayloadImportLimits::default()
            },
        ] {
            let rejection = plan_payload_layout(&[], &limits).expect_err("refused");
            assert_eq!(rejection.error, PayloadLayoutError::InvalidLimits);
            assert_eq!(rejection.entry_index, None);
        }
    }

    proptest::proptest! {
        /// Planning cannot depend on the order entries arrive in: an importer
        /// that enumerates an archive differently must still stage the same
        /// tree under the same identity.
        #[test]
        fn planning_is_independent_of_input_order(
            paths in proptest::collection::vec(
                proptest::string::string_regex("[a-z]{1,6}(/[a-z]{1,6}){0,3}")
                    .expect("strategy"),
                0..8,
            )
        ) {
            let forward = paths
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let size = u64::try_from(index).expect("index");
                    entry(size, path, size)
                })
                .collect::<Vec<_>>();
            let mut reversed = forward.clone();
            reversed.reverse();
            let limits = PayloadImportLimits::default();
            proptest::prop_assert_eq!(
                plan_payload_layout(&forward, &limits).map_err(|rejection| rejection.error),
                plan_payload_layout(&reversed, &limits).map_err(|rejection| rejection.error),
            );
        }

        /// Any two entries whose paths differ only by ASCII case collide, and
        /// planning must always say so rather than letting a case-insensitive
        /// filesystem decide after extraction has begun.
        #[test]
        fn a_case_only_alias_is_always_a_conflict(
            path in proptest::string::string_regex("[a-z]{1,6}(/[a-z]{1,6}){0,3}")
                .expect("strategy")
        ) {
            let aliased = path.to_ascii_uppercase();
            proptest::prop_assume!(PayloadPath::parse(&path).is_ok());
            proptest::prop_assume!(PayloadPath::parse(&aliased).is_ok());
            let rejection = plan_payload_layout(
                &[entry(1, &path, 1), entry(2, &aliased, 1)],
                &PayloadImportLimits::default(),
            );
            proptest::prop_assert_eq!(
                rejection.map(|_| ()).map_err(|rejection| rejection.error),
                Err(PayloadLayoutError::PathConflict),
            );
        }
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            PayloadLayoutError::InvalidLimits,
            PayloadLayoutError::TooManyEntries,
            PayloadLayoutError::TooManyPathBytes,
            PayloadLayoutError::SourceTokenConflict,
            PayloadLayoutError::InvalidPath(PayloadPathError::Empty),
            PayloadLayoutError::PathConflict,
            PayloadLayoutError::EntryTooLarge,
            PayloadLayoutError::PayloadTooLarge,
        ] {
            assert!(!error.to_string().is_empty());
            assert_eq!(
                ohl_core::SanitizedError::from(error),
                ohl_core::SanitizedError::InvalidInput
            );
        }
    }
}
