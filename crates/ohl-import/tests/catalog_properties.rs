//! Property tests for the catalog quota invariants.
//!
//! The worker controls every value fed to [`plan_catalog`], so the properties
//! below are stated the way an attacker would probe them: whatever arbitrary
//! entries arrive, planning either refuses them or produces a catalog that
//! respects every quota exactly.

use ohl_import::{EntryMetadata, ImportLimits, LayoutError, SourceToken, plan_catalog};
use proptest::prelude::*;

fn entries() -> impl Strategy<Value = Vec<EntryMetadata>> {
    proptest::collection::vec(
        (
            0..16_u64,
            proptest::collection::vec("[a-cA-C./\\\\]{0,6}", 1..3),
            0..64_u64,
        )
            .prop_map(|(token, components, size)| EntryMetadata {
                source_token: SourceToken(token),
                archive_path: components.join("/"),
                size_bytes: size,
            }),
        0..8,
    )
}

fn limits() -> impl Strategy<Value = ImportLimits> {
    (1..8_u32, 1..64_u64, 1..64_u64).prop_map(|(entries, path_bytes, entry_bytes)| {
        ImportLimits::new(entries, path_bytes, entry_bytes, entry_bytes * 8)
            .expect("in-range property limits")
    })
}

proptest! {
    /// A promoted catalog never exceeds any quota it was planned under.
    #[test]
    fn a_planned_catalog_respects_every_quota(entries in entries(), limits in limits()) {
        let Ok(catalog) = plan_catalog(&entries, limits) else {
            return Ok(());
        };
        prop_assert!(catalog.entries().len() <= limits.maximum_entries() as usize);
        prop_assert!(catalog.total_bytes() <= limits.maximum_total_bytes());
        let mut total = 0_u64;
        for entry in catalog.entries() {
            prop_assert!(entry.size_bytes() <= limits.maximum_entry_bytes());
            total += entry.size_bytes();
        }
        prop_assert_eq!(total, catalog.total_bytes());
    }

    /// Every promoted path is absolute, canonical, and unique after folding.
    #[test]
    fn planned_paths_are_canonical_and_unique(entries in entries(), limits in limits()) {
        let Ok(catalog) = plan_catalog(&entries, limits) else {
            return Ok(());
        };
        let mut folded = std::collections::BTreeSet::new();
        for entry in catalog.entries() {
            let path = entry.relative_path().as_str();
            prop_assert!(path.starts_with('/'));
            prop_assert!(!path.contains('\\'));
            prop_assert!(!path.contains("//"));
            prop_assert!(!path.split('/').any(|part| part == "." || part == ".."));
            prop_assert!(folded.insert(path.to_ascii_lowercase()));
        }
        // No promoted path is a directory prefix of another.
        let paths: Vec<&String> = folded.iter().collect();
        for pair in paths.windows(2) {
            let prefix = format!("{}/", pair[0]);
            prop_assert!(!pair[1].starts_with(&prefix));
        }
    }

    /// Every promoted token is unique and looks its own entry back up.
    #[test]
    fn planned_tokens_index_their_own_entries(entries in entries(), limits in limits()) {
        let Ok(catalog) = plan_catalog(&entries, limits) else {
            return Ok(());
        };
        let mut tokens = std::collections::BTreeSet::new();
        for entry in catalog.entries() {
            prop_assert!(tokens.insert(entry.source_token()));
            let found = catalog.find(entry.source_token()).expect("indexed entry");
            prop_assert_eq!(found.size_bytes(), entry.size_bytes());
            prop_assert_eq!(found.relative_path(), entry.relative_path());
        }
        for token in 0..32_u64 {
            let expected = tokens.contains(&SourceToken(token));
            prop_assert_eq!(catalog.find(SourceToken(token)).is_some(), expected);
        }
    }

    /// Planning is deterministic: the same input always plans the same way.
    #[test]
    fn planning_is_deterministic(entries in entries(), limits in limits()) {
        let first = plan_catalog(&entries, limits);
        let second = plan_catalog(&entries, limits);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                prop_assert_eq!(first.entries(), second.entries());
                prop_assert_eq!(first.total_bytes(), second.total_bytes());
            }
            (Err(first), Err(second)) => prop_assert_eq!(first, second),
            _ => prop_assert!(false, "planning must not vary between calls"),
        }
    }

    /// A quota that cannot hold the input is always the reported reason.
    #[test]
    fn an_over_count_enumeration_is_always_refused(entries in entries(), limits in limits()) {
        prop_assume!(entries.len() > limits.maximum_entries() as usize);
        prop_assert_eq!(
            plan_catalog(&entries, limits).unwrap_err(),
            LayoutError::TooManyEntries
        );
    }
}
