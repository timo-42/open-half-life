//! Provenance-cache publication behaviour over synthetic files.
//!
//! The lifecycle covered here is the one `docs/MILESTONES.md` lists for M2:
//! create, reuse, tampered manifest, foreign schema, publication-lock
//! contention, and an explicit `--cache` override.

mod support;

use std::sync::mpsc;

use fs4::fs_std::FileExt as _;
use ohl_media::{
    CacheLayout, CacheManifest, CacheReport, ENTRIES_DIRECTORY_NAME, ImportCacheError,
    MANIFEST_FILE_NAME, MANIFEST_SCHEMA_VERSION, PAYLOAD_STATE_NOT_IMPORTED, ValidatedMedia,
    prepare_import_cache,
};
use support::{TemporaryRoot, pinned_source, synthetic_bytes, validated};

/// A cache root inside a temporary directory, plus a proof for one synthetic
/// source.
struct Fixture {
    root: TemporaryRoot,
    layout: CacheLayout,
    media: ValidatedMedia,
}

fn fixture() -> Fixture {
    let root = TemporaryRoot::new();
    let source = pinned_source(
        root.path(),
        "private-source-name.iso",
        &synthetic_bytes(200_000),
    );
    let media = validated(source);
    let layout = CacheLayout::with_root(root.path().join("cache")).expect("absolute root");
    Fixture {
        root,
        layout,
        media,
    }
}

#[test]
fn a_new_entry_is_created_then_reused_without_being_rewritten() {
    let fixture = fixture();
    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect("first publication"),
        CacheReport::Created
    );

    let manifest_path = fixture.layout.manifest_path(fixture.media.digest());
    assert!(manifest_path.is_file());
    let published = std::fs::read(&manifest_path).expect("manifest");
    let modified = std::fs::metadata(&manifest_path)
        .expect("metadata")
        .modified()
        .expect("modification time");

    for _ in 0..3 {
        assert_eq!(
            prepare_import_cache(&fixture.media, &fixture.layout).expect("reuse"),
            CacheReport::Reused
        );
    }
    assert_eq!(
        std::fs::read(&manifest_path).expect("manifest"),
        published,
        "a cache hit must not rewrite the manifest"
    );
    assert_eq!(
        std::fs::metadata(&manifest_path)
            .expect("metadata")
            .modified()
            .expect("modification time"),
        modified
    );
}

#[test]
fn the_entry_is_content_addressed_and_metadata_only() {
    let fixture = fixture();
    prepare_import_cache(&fixture.media, &fixture.layout).expect("publication");

    let entry = fixture.layout.entry_directory(fixture.media.digest());
    assert!(entry.ends_with(fixture.media.digest().to_hex()));

    // The entry holds the manifest and nothing else: no payload, no copy of
    // the source, no leftover staging tree.
    let entries: Vec<_> = std::fs::read_dir(&entry)
        .expect("entry directory")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from(MANIFEST_FILE_NAME)]);

    let text = std::fs::read_to_string(entry.join(MANIFEST_FILE_NAME)).expect("manifest");
    assert!(text.contains(&fixture.media.digest().to_hex()));
    assert!(text.contains("200000"));
    assert!(text.contains("SYNTHETIC"));
    assert!(text.contains(PAYLOAD_STATE_NOT_IMPORTED));
    assert!(
        !text.contains("private-source-name"),
        "the manifest must never record a source path or name"
    );
    assert!(!text.contains(".iso"));
    assert!(!text.contains('/') && !text.contains('\\'));

    let manifest = CacheManifest::parse(text.as_bytes()).expect("well-formed manifest");
    assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);
    assert!(manifest.describes(&fixture.media));
    assert!(manifest.created_unix_seconds > 0);
}

#[test]
fn no_staging_tree_survives_a_publication() {
    let fixture = fixture();
    prepare_import_cache(&fixture.media, &fixture.layout).expect("publication");

    let leftovers: Vec<_> = std::fs::read_dir(fixture.layout.entries_directory())
        .expect("entries directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.contains("staging"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging trees must not be left behind"
    );
}

#[test]
fn a_tampered_manifest_is_a_conflict_and_is_never_overwritten() {
    let fixture = fixture();
    prepare_import_cache(&fixture.media, &fixture.layout).expect("publication");

    let manifest_path = fixture.layout.manifest_path(fixture.media.digest());
    std::fs::write(&manifest_path, "tampered\n").expect("tamper");

    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect_err("conflict"),
        ImportCacheError::ManifestConflict
    );
    assert_eq!(
        std::fs::read_to_string(&manifest_path).expect("manifest"),
        "tampered\n",
        "a conflicting manifest must be left exactly as it was"
    );
}

#[test]
fn a_manifest_describing_other_media_is_a_conflict() {
    let fixture = fixture();
    prepare_import_cache(&fixture.media, &fixture.layout).expect("publication");

    let manifest_path = fixture.layout.manifest_path(fixture.media.digest());
    let mut manifest =
        CacheManifest::parse(&std::fs::read(&manifest_path).expect("manifest")).expect("parsed");
    manifest.size_bytes += 1;
    std::fs::write(&manifest_path, manifest.to_json().expect("json")).expect("rewrite");

    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect_err("conflict"),
        ImportCacheError::ManifestConflict
    );
}

#[test]
fn a_foreign_schema_is_rejected_by_its_own_code() {
    let fixture = fixture();
    prepare_import_cache(&fixture.media, &fixture.layout).expect("publication");

    let manifest_path = fixture.layout.manifest_path(fixture.media.digest());
    std::fs::write(
        &manifest_path,
        format!(
            "{{\n  \"schema_version\": {},\n  \"unknown\": true\n}}\n",
            MANIFEST_SCHEMA_VERSION + 1
        ),
    )
    .expect("foreign manifest");

    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect_err("foreign schema"),
        ImportCacheError::ManifestSchemaUnsupported
    );
}

#[test]
fn an_entry_without_a_manifest_is_a_conflict() {
    let fixture = fixture();
    let entry = fixture.layout.entry_directory(fixture.media.digest());
    std::fs::create_dir_all(&entry).expect("half-published entry");

    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect_err("no manifest"),
        ImportCacheError::ManifestConflict
    );
}

#[test]
fn a_contended_publication_lock_is_reported_and_publishes_nothing() {
    let fixture = fixture();
    let lock_path = fixture.layout.lock_path(fixture.media.digest());
    std::fs::create_dir_all(fixture.layout.entries_directory()).expect("entries directory");

    let (locked_sender, locked_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .expect("lock file");
        assert!(file.try_lock_exclusive().expect("lock"), "lock was free");
        locked_sender.send(()).expect("signal");
        // Hold the lock until the main thread has observed the contention.
        let _ = release_receiver.recv();
    });

    locked_receiver.recv().expect("lock acquired");
    let error = prepare_import_cache(&fixture.media, &fixture.layout)
        .expect_err("the contended lock must be reported");
    assert_eq!(error, ImportCacheError::CacheBusy);
    assert!(
        !fixture
            .layout
            .entry_directory(fixture.media.digest())
            .exists(),
        "a refused publication must not create an entry"
    );

    release_sender.send(()).expect("release");
    holder.join().expect("holder thread");

    // Once the lock is free the same request succeeds.
    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect("publication"),
        CacheReport::Created
    );
}

#[test]
fn an_explicit_cache_root_override_is_honoured() {
    let fixture = fixture();
    let override_root = fixture.root.path().join("explicit-cache-override");
    let overridden = CacheLayout::with_root(&override_root).expect("absolute override");

    assert_eq!(
        prepare_import_cache(&fixture.media, &overridden).expect("publication"),
        CacheReport::Created
    );
    assert!(
        overridden
            .manifest_path(fixture.media.digest())
            .starts_with(&override_root)
    );
    assert!(
        !fixture.layout.root().exists(),
        "the override must be the only root that is touched"
    );
    assert_eq!(
        prepare_import_cache(&fixture.media, &overridden).expect("reuse"),
        CacheReport::Reused
    );
}

#[test]
fn a_relative_override_is_refused_before_anything_is_created() {
    assert_eq!(
        CacheLayout::with_root("relative-cache").expect_err("relative"),
        ImportCacheError::UnsafeCachePath
    );
}

#[test]
fn a_cache_root_component_that_is_not_a_directory_is_refused() {
    let fixture = fixture();
    let blocked = fixture.root.path().join("blocking-file");
    std::fs::write(&blocked, b"not a directory").expect("blocking file");
    let layout = CacheLayout::with_root(blocked.join("cache")).expect("absolute root");

    assert_eq!(
        prepare_import_cache(&fixture.media, &layout).expect_err("unsafe path"),
        ImportCacheError::UnsafeCachePath
    );
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_ancestor_of_the_cache_root_is_resolved_and_accepted() {
    // This is the macOS `--cache` scenario `CacheLayout::with_root`'s doc
    // comment describes: `/tmp` and `/var` are themselves symbolic links to
    // `/private/tmp` and `/private/var`, so a completely ordinary cache root
    // has a symlinked ancestor. `with_root` must resolve it rather than
    // refuse it, the way `a_cache_root_component_that_is_not_a_directory_is_refused`
    // still refuses a *non-directory* blocking a root component.
    let fixture = fixture();
    let target = fixture.root.path().join("link-target");
    std::fs::create_dir(&target).expect("link target");
    let link = fixture.root.path().join("linked-cache");
    std::os::unix::fs::symlink(&target, &link).expect("symbolic link");
    let layout = CacheLayout::with_root(link.join("cache")).expect("absolute root");

    // The symlinked ancestor is resolved to its real target at construction
    // time, before any no-follow check runs.
    assert_eq!(layout.root(), target.join("cache"));
    assert_eq!(
        prepare_import_cache(&fixture.media, &layout).expect("publication"),
        CacheReport::Created
    );
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_created_beneath_the_resolved_root_is_still_refused() {
    // Resolving a pre-existing symlinked ancestor of the root must not weaken
    // no-follow enforcement for anything this process itself goes on to
    // create beneath that resolved root: `ensure_directory_tree` still walks
    // every component fresh, on every call, with its own `symlink_metadata`
    // check.
    let fixture = fixture();
    let layout = CacheLayout::with_root(fixture.root.path().join("cache")).expect("absolute root");
    std::fs::create_dir_all(layout.root()).expect("cache root");
    let outside = fixture.root.path().join("outside-sources");
    std::fs::create_dir(&outside).expect("outside target");
    std::os::unix::fs::symlink(&outside, layout.root().join(ENTRIES_DIRECTORY_NAME))
        .expect("symbolic link");

    assert_eq!(
        prepare_import_cache(&fixture.media, &layout).expect_err("unsafe path"),
        ImportCacheError::UnsafeCachePath
    );
}

#[cfg(unix)]
#[test]
fn a_manifest_replaced_by_a_symbolic_link_is_a_conflict() {
    let fixture = fixture();
    let target = fixture.root.path().join("outside-manifest.json");
    let entry = fixture.layout.entry_directory(fixture.media.digest());
    std::fs::create_dir_all(&entry).expect("entry directory");
    prepare_import_cache(&fixture.media, &fixture.layout).ok();
    std::fs::write(
        &target,
        CacheManifest::for_media(&fixture.media, 0)
            .expect("manifest")
            .to_json()
            .expect("json"),
    )
    .expect("outside manifest");
    let manifest_path = entry.join(MANIFEST_FILE_NAME);
    let _ = std::fs::remove_file(&manifest_path);
    std::os::unix::fs::symlink(&target, &manifest_path).expect("symbolic link");

    assert_eq!(
        prepare_import_cache(&fixture.media, &fixture.layout).expect_err("linked manifest"),
        ImportCacheError::ManifestConflict
    );
}

#[test]
fn a_source_truncated_after_validation_publishes_nothing() {
    let root = TemporaryRoot::new();
    let path = root.path().join("shrinking.iso");
    std::fs::write(&path, synthetic_bytes(100_000)).expect("fixture");
    let source = std::sync::Arc::new(ohl_platform::MediaSource::open(&path).expect("pinned"));
    let media = validated(source);
    let layout = CacheLayout::with_root(root.path().join("cache")).expect("absolute root");

    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_len(50_000)
        .expect("truncate");

    assert_eq!(
        prepare_import_cache(&media, &layout).expect_err("changed source"),
        ImportCacheError::SourceChanged
    );
    assert!(!layout.entry_directory(media.digest()).exists());
}

#[test]
fn two_sources_with_different_content_get_different_entries() {
    let root = TemporaryRoot::new();
    let layout = CacheLayout::with_root(root.path().join("cache")).expect("absolute root");
    let first = validated(pinned_source(root.path(), "a.iso", &synthetic_bytes(4_096)));
    let second = validated(pinned_source(root.path(), "b.iso", &synthetic_bytes(8_192)));

    assert_eq!(
        prepare_import_cache(&first, &layout).expect("first"),
        CacheReport::Created
    );
    assert_eq!(
        prepare_import_cache(&second, &layout).expect("second"),
        CacheReport::Created
    );
    assert_ne!(first.digest(), second.digest());
    // Entry directories are content addressed; the sibling lock files are
    // named after the same digests and are not entries.
    let published: Vec<_> = std::fs::read_dir(layout.entries_directory())
        .expect("entries")
        .map(|entry| entry.expect("entry"))
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(published.len(), 2);
    assert!(published.contains(&std::ffi::OsString::from(first.digest().to_hex())));
    assert!(published.contains(&std::ffi::OsString::from(second.digest().to_hex())));
}
