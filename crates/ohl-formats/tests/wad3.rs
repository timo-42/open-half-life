//! Round-trip, accessor, and malformed-field rejection tests for `wad3`,
//! using this crate's own synthetic fixture writer.

use ohl_formats::test_support::Wad3Builder;
use ohl_formats::wad3::{EntryKind, Limits, Wad3};

fn tiny_wad() -> Wad3Builder {
    let mut b = Wad3Builder::new();
    b.add_miptex("wall01", 16, 16, 3);
    b.add_miptex("WALL02", 32, 16, 9);
    b
}

#[test]
fn round_trips_header_and_directory() {
    let bytes = tiny_wad().build();
    let limits = Limits::default();
    let wad = Wad3::parse(&bytes, &limits).expect("valid synthetic WAD3 parses");
    assert_eq!(wad.len(), 2);
    let names: Vec<_> = wad.entries().map(|e| e.unwrap()).map(|e| e.kind).collect();
    assert_eq!(names, vec![EntryKind::Miptex, EntryKind::Miptex]);
}

#[test]
fn looks_up_entries_case_insensitively() {
    let bytes = tiny_wad().build();
    let limits = Limits::default();
    let wad = Wad3::parse(&bytes, &limits).unwrap();
    assert!(wad.find("WALL01").unwrap().is_some());
    assert!(wad.find("wall02").unwrap().is_some());
    assert!(wad.find("missing").unwrap().is_none());
}

#[test]
fn decodes_miptex_pixels_and_palette() {
    let bytes = tiny_wad().build();
    let limits = Limits::default();
    let wad = Wad3::parse(&bytes, &limits).unwrap();
    let entry = wad.find("wall01").unwrap().expect("present");
    let miptex = wad.decode_miptex(&entry).expect("valid miptex");
    assert_eq!(miptex.width, 16);
    assert_eq!(miptex.height, 16);
    assert_eq!(miptex.body.mips[0].indices.len(), 16 * 16);
    assert_eq!(miptex.body.mips[0].indices[0], 3);
    assert_eq!(miptex.body.mips[3].width, 2);
    assert_eq!(miptex.body.palette.get(3).g, 3);
}

// --- Malformed-field rejection tests -------------------------------------

#[test]
fn rejects_bad_magic() {
    let mut bytes = tiny_wad().build();
    bytes[0..4].copy_from_slice(b"NOPE");
    assert!(Wad3::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_directory_outside_file() {
    let mut bytes = tiny_wad().build();
    let huge = u32::try_from(bytes.len()).unwrap() + 1_000_000;
    bytes[8..12].copy_from_slice(&huge.to_le_bytes());
    assert!(Wad3::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_entry_offset_out_of_bounds() {
    let bytes = tiny_wad().build();
    let limits = Limits::default();
    // Find the directory and corrupt the first entry's offset field
    // in-place to point past the end of the file.
    let mut bytes = bytes;
    let dir_offset = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let huge = u32::try_from(bytes.len()).unwrap() + 1_000_000;
    bytes[dir_offset..dir_offset + 4].copy_from_slice(&huge.to_le_bytes());
    assert!(Wad3::parse(&bytes, &limits).is_err());
}

#[test]
fn rejects_miptex_offsets_outside_entry() {
    let mut b = tiny_wad();
    b.add_raw_entry("bad", 0x43, vec![0u8; 40]); // header only, no body/palette
    let bytes = b.build();
    let limits = Limits::default();
    // Directory validation only checks the entry's own bounds, not the
    // internal miptex layout, so parsing the file succeeds...
    let wad = Wad3::parse(&bytes, &limits).expect("directory bounds are valid");
    let entry = wad.find("bad").unwrap().expect("present");
    // ...but decoding the (all-zero, too-short) miptex body must fail.
    assert!(wad.decode_miptex(&entry).is_err());
}

#[test]
fn rejects_wrong_entry_kind_for_miptex_decode() {
    let mut b = tiny_wad();
    b.add_raw_entry("apic", 0x42, vec![0u8; 8]);
    let bytes = b.build();
    let limits = Limits::default();
    let wad = Wad3::parse(&bytes, &limits).unwrap();
    let entry = wad.find("apic").unwrap().expect("present");
    assert_eq!(entry.kind, EntryKind::Qpic);
    assert!(wad.decode_miptex(&entry).is_err());
}
