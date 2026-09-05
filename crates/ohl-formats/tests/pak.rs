//! Round-trip, lookup, and malformed-directory rejection tests for `pak`,
//! using this crate's own synthetic fixture writer.

use ohl_formats::pak::{Directory, ENTRY_LEN, HEADER_LEN, Limits, parse_header};
use ohl_formats::test_support::PakBuilder;

fn tiny_pak() -> PakBuilder {
    let mut b = PakBuilder::new();
    b.add_entry("maps/crossfire.bsp", vec![1, 2, 3, 4]);
    b.add_entry("SOUND/AMBIENCE/HUM1.WAV", vec![5, 6]);
    b
}

#[test]
fn round_trips_header_and_directory() {
    let bytes = tiny_pak().build();
    let limits = Limits::default();
    let dir = Directory::parse(&bytes, &limits).expect("valid synthetic PAK parses");
    assert_eq!(dir.len(), 2);
    let names: Vec<_> = dir
        .entries()
        .map(|e| String::from_utf8(e.trimmed_name().to_vec()).unwrap())
        .collect();
    assert_eq!(names, vec!["maps/crossfire.bsp", "SOUND/AMBIENCE/HUM1.WAV"]);
}

#[test]
fn looks_up_entries_case_insensitively() {
    let bytes = tiny_pak().build();
    let limits = Limits::default();
    let dir = Directory::parse(&bytes, &limits).unwrap();
    let entry = dir
        .find("sound/ambience/hum1.wav")
        .expect("case-insensitive match");
    assert_eq!(entry.size, 2);
    assert!(dir.find("missing.wav").is_none());
}

#[test]
fn duplicate_names_first_wins() {
    let mut b = PakBuilder::new();
    b.add_entry("dup.txt", vec![1]);
    b.add_entry("DUP.TXT", vec![2, 2]);
    let bytes = b.build();
    let dir = Directory::parse(&bytes, &Limits::default()).unwrap();
    let entry = dir.find("dup.txt").expect("present");
    assert_eq!(entry.size, 1, "the first occurrence must win");
}

#[test]
fn from_parts_matches_parse_over_separately_read_bytes() {
    let bytes = tiny_pak().build();
    let limits = Limits::default();

    let mut header_bytes = [0u8; HEADER_LEN];
    header_bytes.copy_from_slice(&bytes[..HEADER_LEN]);
    let (dir_offset, dir_size) = parse_header(&header_bytes).expect("valid header");
    let dir_bytes = &bytes[dir_offset as usize..dir_offset as usize + dir_size as usize];

    let dir = Directory::from_parts(bytes.len() as u64, dir_size, dir_bytes, &limits)
        .expect("from_parts matches an in-memory parse");
    assert_eq!(dir.len(), 2);
    assert!(dir.find("maps/crossfire.bsp").is_some());
}

// --- Malformed-directory rejection tests ---------------------------------

#[test]
fn rejects_bad_magic() {
    let mut bytes = tiny_pak().build();
    bytes[0..4].copy_from_slice(b"NOPE");
    assert!(Directory::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_directory_size_with_remainder() {
    let mut bytes = tiny_pak().build();
    // Directory size lives at header bytes [8..12); add one stray byte so
    // it is no longer a multiple of the 64-byte entry size.
    let dir_size = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    bytes[8..12].copy_from_slice(&(dir_size + 1).to_le_bytes());
    assert!(Directory::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_directory_outside_file() {
    let mut bytes = tiny_pak().build();
    let huge = u32::try_from(bytes.len()).unwrap() + 1_000_000;
    bytes[4..8].copy_from_slice(&huge.to_le_bytes());
    assert!(Directory::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_entry_range_outside_file() {
    let bytes = tiny_pak().build();
    let mut bytes = bytes;
    let dir_offset = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    // The first entry's offset field sits right after its 56-byte name.
    let offset_field = dir_offset + 56;
    let huge = u32::try_from(bytes.len()).unwrap() + 1_000_000;
    bytes[offset_field..offset_field + 4].copy_from_slice(&huge.to_le_bytes());
    assert!(Directory::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_entry_count_over_the_limit() {
    let mut b = PakBuilder::new();
    for i in 0..8 {
        b.add_entry(&format!("f{i}.txt"), vec![0]);
    }
    let bytes = b.build();
    let limits = Limits {
        max_entries: 4,
        ..Limits::default()
    };
    assert!(Directory::parse(&bytes, &limits).is_err());
}

#[test]
fn rejects_entry_size_over_the_limit() {
    let mut b = PakBuilder::new();
    b.add_entry("big.dat", vec![0u8; 16]);
    let bytes = b.build();
    let limits = Limits {
        max_entry_bytes: 4,
        ..Limits::default()
    };
    assert!(Directory::parse(&bytes, &limits).is_err());
}

#[test]
fn rejects_unterminated_name() {
    let mut b = PakBuilder::new();
    b.add_raw_named_entry([b'a'; ENTRY_LEN - 8], vec![0]);
    let bytes = b.build();
    assert!(Directory::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_truncated_header() {
    let bytes = vec![b'P', b'A', b'C'];
    assert!(Directory::parse(&bytes, &Limits::default()).is_err());
}
