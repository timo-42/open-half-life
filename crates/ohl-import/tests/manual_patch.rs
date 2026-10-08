//! Opt-in native worker validation. All inputs and outputs stay local.
//! Install the parser image beside this test executable before running.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ohl_import::patch::{BasePayload, open_installer, run_patch_import};
use ohl_import::{DiscardProgress, ImportCancellation, ImportOutcome};
use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia, VolumeLabel};
use ohl_payload::SelectionRecipe;
use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, Mount};

#[test]
#[ignore = "requires OHL_TEST_ISO, OHL_TEST_PATCH, OHL_TEST_IMPORT_ROOT and an installed worker"]
fn manual_iso_plus_full_update() {
    let input = |key| PathBuf::from(std::env::var_os(key).expect("required local test input"));
    let root = input("OHL_TEST_IMPORT_ROOT");
    let source = Arc::new(MediaSource::open(&input("OHL_TEST_ISO")).expect("pin local ISO"));
    let mount =
        Mount::open(Arc::clone(&source), DirectoryLimits::default()).expect("mount local ISO");
    let class = match mount.class() {
        ohl_vfs::MediaClass::Iso9660 => MediaClass::Iso9660,
        ohl_vfs::MediaClass::Udf => MediaClass::Udf,
    };
    let media = ValidatedMedia::fingerprinting(
        source,
        MediaDescription::new(class, mount.filesystem().as_str(), VolumeLabel::empty()),
    )
    .expect("fingerprint local ISO");
    let cache = CacheLayout::with_root(root.join("cache")).expect("cache layout");
    ohl_media::prepare_import_cache(&media, &cache).expect("prepare provenance");
    let payload = root.join("payload");
    let recipe = SelectionRecipe::parse("version = 1\ndefault_decision = \"include\"\n").unwrap();
    let transport = ohl_import::CancellationToken::default();
    let staging = ohl_payload::CancellationToken::default();
    let cancel = ImportCancellation {
        transport: &transport,
        staging: &staging,
    };
    let base_identity = match ohl_import::pipeline::recorded_payload_identity(&cache, &media) {
        Some(identity) => identity,
        None => {
            ohl_import::run_import(
                &media,
                &mount,
                &recipe,
                &payload,
                &cache,
                cancel,
                &mut DiscardProgress,
            )
            .expect("import base ISO")
            .payload_identity
        }
    };
    let base_files = ohl_payload::published_files_directory(&payload, &base_identity).unwrap();
    let installer = open_installer(&input("OHL_TEST_PATCH")).expect("pin update installer");
    let base = BasePayload {
        identity: &base_identity,
        files: &base_files,
    };
    let first = run_patch_import(
        &installer,
        base,
        &recipe,
        &payload,
        &cache,
        cancel,
        &mut DiscardProgress,
    )
    .expect("import full update through confined worker");
    assert_ne!(first.payload_identity, base_identity);
    assert!(
        ohl_payload::published_files_directory(&payload, &first.payload_identity)
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        ohl_import::pipeline::recorded_payload_identity(&cache, &media),
        Some(base_identity.clone())
    );
    let combined =
        ohl_payload::published_files_directory(&payload, &first.payload_identity).unwrap();
    assert_replacements_and_retention(&base_files, &combined);
    let again = run_patch_import(
        &installer,
        base,
        &recipe,
        &payload,
        &cache,
        cancel,
        &mut DiscardProgress,
    )
    .expect("reuse full update");
    assert_eq!(again.outcome, ImportOutcome::AlreadyPublished);
    assert_eq!(again.payload_identity, first.payload_identity);
    println!(
        "native ISO and standalone patch import succeeded; combined tree reused; base preserved"
    );
}

/// No media-derived name or byte is reported, including on assertion failures.
fn assert_replacements_and_retention(base_files: &Path, combined: &Path) {
    let mut pending = vec![base_files.to_path_buf()];
    let mut changed = false;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("read local payload") {
            let entry = entry.expect("read local entry");
            if entry.file_type().expect("entry type").is_dir() {
                pending.push(entry.path());
                continue;
            }
            let original = entry.path();
            let relative = original
                .strip_prefix(base_files)
                .unwrap()
                .to_str()
                .expect("portable path")
                .to_ascii_lowercase();
            let before = MediaSource::open(&original).expect("pin base file");
            let after = MediaSource::open(&combined.join(relative)).expect("base file retained");
            changed |= ohl_media::fingerprint(&before).expect("base digest")
                != ohl_media::fingerprint(&after).expect("combined digest");
        }
    }
    assert!(
        changed,
        "the selected full update must replace existing content"
    );
}
