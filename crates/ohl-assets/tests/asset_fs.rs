//! End-to-end tests over a synthetic payload tree: loose files mixed with a
//! synthetic PAK archive, laid out the way an imported Half-Life payload
//! looks (`<files_dir>/<mod>/...`).

use std::fs;
use std::io::Read;
use std::path::Path;

use ohl_assets::{AssetFs, Limits};
use ohl_formats::test_support::PakBuilder;

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn write_pak(root: &Path, relative: &str, entries: &[(&str, &[u8])]) {
    let mut builder = PakBuilder::new();
    for (name, body) in entries {
        builder.add_entry(name, (*body).to_vec());
    }
    write(root, relative, &builder.build());
}

#[test]
fn loose_files_beat_pak_entries_of_the_same_name() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write(&files_dir, "valve/sound/foo.wav", b"loose-wins");
    write_pak(
        &files_dir,
        "valve/pak0.pak",
        &[("sound/foo.wav", b"pak-loses")],
    );

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();

    let mut asset = fs.open("sound/foo.wav").unwrap();
    let mut contents = Vec::new();
    asset.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"loose-wins");
}

#[test]
fn earlier_mod_dir_wins_over_later() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write(&files_dir, "custom/maps/x.bsp", b"custom-map");
    write(&files_dir, "valve/maps/x.bsp", b"base-map");

    let fs = AssetFs::mount(
        &files_dir,
        &["custom".to_string(), "valve".to_string()],
        Limits::default(),
    )
    .unwrap();

    let mut asset = fs.open("maps/x.bsp").unwrap();
    let mut contents = Vec::new();
    asset.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"custom-map");
}

#[test]
fn earlier_pak_wins_over_later_pak() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write_pak(&files_dir, "valve/pak0.pak", &[("dup.txt", b"first")]);
    write_pak(&files_dir, "valve/pak1.pak", &[("dup.txt", b"second")]);

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    let mut asset = fs.open("dup.txt").unwrap();
    let mut contents = Vec::new();
    asset.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"first");
}

#[test]
fn resolves_paths_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write(&files_dir, "valve/Sprites/Fire.spr", b"sprite-bytes");

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    assert!(fs.exists("sprites/fire.spr"));
    assert!(fs.exists("SPRITES/FIRE.SPR"));
    let mut asset = fs.open("sprites\\fire.spr").unwrap();
    let mut contents = Vec::new();
    asset.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"sprite-bytes");
}

#[test]
fn missing_asset_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    fs::create_dir_all(files_dir.join("valve")).unwrap();
    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    assert!(!fs.exists("nope.wav"));
    assert!(fs.open("nope.wav").is_err());
}

#[test]
fn list_dir_is_bounded_and_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write(&files_dir, "valve/sound/a.wav", b"a");
    write(&files_dir, "valve/sound/b.wav", b"b");
    write(&files_dir, "valve/sound/sub/c.wav", b"c");
    write(&files_dir, "valve/maps/x.bsp", b"x");

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    let listing = fs.list_dir("sound").unwrap();
    assert_eq!(
        listing,
        vec!["sound/a.wav", "sound/b.wav", "sound/sub/c.wav"]
    );

    let mut everything = fs.list_dir("").unwrap();
    everything.sort();
    assert_eq!(everything.len(), 4);
}

#[test]
fn resolves_wads_by_basename_ignoring_mapper_directories() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write(&files_dir, "valve/halflife.wad", b"wad-bytes");

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    let mut wads = fs.resolve_wads("\\quake\\hlwad\\halflife.wad;\\quake\\hlwad\\missing.wad;");
    assert_eq!(wads.len(), 1);
    let mut contents = Vec::new();
    wads[0].read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"wad-bytes");
}

#[test]
fn numbered_paks_load_ascending_then_other_paks_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write_pak(&files_dir, "valve/pak1.pak", &[("only_in_pak1.txt", b"1")]);
    write_pak(&files_dir, "valve/pak0.pak", &[("only_in_pak0.txt", b"0")]);
    write_pak(
        &files_dir,
        "valve/zz_extra.pak",
        &[("only_in_extra.txt", b"z")],
    );

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    assert!(fs.exists("only_in_pak0.txt"));
    assert!(fs.exists("only_in_pak1.txt"));
    assert!(fs.exists("only_in_extra.txt"));
}

#[test]
fn a_pak_archive_is_never_indexed_as_an_asset_itself() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write_pak(&files_dir, "valve/pak0.pak", &[("a.txt", b"a")]);

    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    assert!(!fs.exists("pak0.pak"));
}

#[test]
fn nonexistent_search_path_is_skipped_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    fs::create_dir_all(&files_dir).unwrap();
    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default());
    assert!(fs.is_ok());
}

#[test]
fn malformed_pak_is_reported_as_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    write(&files_dir, "valve/pak0.pak", b"NOTAPAK!!");
    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default());
    assert!(fs.is_err());
}

#[test]
fn rejects_traversal_in_a_requested_path() {
    let dir = tempfile::tempdir().unwrap();
    let files_dir = dir.path().join("files");
    fs::create_dir_all(&files_dir).unwrap();
    let fs = AssetFs::mount(&files_dir, &["valve".to_string()], Limits::default()).unwrap();
    assert!(fs.open("../../etc/passwd").is_err());
    assert!(!fs.exists("../../etc/passwd"));
}
