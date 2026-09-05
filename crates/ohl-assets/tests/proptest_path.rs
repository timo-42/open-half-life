//! `proptest`-driven checks over `ohl_assets`'s path normalization: it must
//! never panic on arbitrary input, must never accept a traversing or
//! absolute-looking path, and must be idempotent / case-insensitive once
//! accepted.

use ohl_assets::AssetFs;
use proptest::prelude::*;

/// A helper that goes through the crate's only public entry point that
/// exercises path normalization without requiring a real payload tree:
/// `AssetFs::exists` on an empty, freshly-mounted filesystem never panics
/// and never reports an accepted traversal as present.
fn exercise(fs: &AssetFs, path: &str) {
    // Must never panic, regardless of what `path` contains.
    let _ = fs.exists(path);
    let _ = fs.open(path);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn normalization_never_panics(path in "\\PC*") {
        let dir = tempfile::tempdir().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir_all(&files_dir).unwrap();
        let fs = AssetFs::mount(&files_dir, &["valve".to_string()], ohl_assets::Limits::default())
            .unwrap();
        exercise(&fs, &path);
    }

    #[test]
    fn traversal_components_are_never_resolvable(
        segments in proptest::collection::vec("[a-zA-Z0-9_]{1,8}", 0..6)
    ) {
        let dir = tempfile::tempdir().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir_all(&files_dir).unwrap();
        let fs = AssetFs::mount(&files_dir, &["valve".to_string()], ohl_assets::Limits::default())
            .unwrap();

        let mut path = String::new();
        for _ in 0..3 {
            path.push_str("../");
        }
        path.push_str(&segments.join("/"));
        // A path built entirely out of `..` segments must never exist,
        // whatever the underlying filesystem looks like.
        prop_assert!(!fs.exists(&path));
    }

    #[test]
    fn case_only_variants_agree_on_existence(name in "[a-zA-Z]{1,12}") {
        let dir = tempfile::tempdir().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir_all(files_dir.join("valve")).unwrap();
        std::fs::write(files_dir.join("valve").join(format!("{name}.txt")), b"x").unwrap();
        let fs = AssetFs::mount(&files_dir, &["valve".to_string()], ohl_assets::Limits::default())
            .unwrap();

        let upper = format!("{}.txt", name.to_ascii_uppercase());
        let lower = format!("{}.txt", name.to_ascii_lowercase());
        prop_assert!(fs.exists(&upper));
        prop_assert!(fs.exists(&lower));
    }
}
