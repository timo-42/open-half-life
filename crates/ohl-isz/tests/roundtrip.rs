//! Round-trips over project-authored synthetic archives.
//!
//! Every name and every byte here is invented. Nothing from any real medium
//! appears in this file.

use ohl_isz::testing::{ArchiveBuilder, implode, sample_archive};
use ohl_isz::{Archive, Limits, NeverCancelled, SliceSource, explode_to_vec, find_signature};

fn open(bytes: &[u8], base: u64) -> Archive {
    let mut source = SliceSource::new(bytes);
    Archive::open(&mut source, base, &Limits::default(), &NeverCancelled).expect("archive opens")
}

fn extract(archive: &mut Archive, bytes: &[u8], index: u32) -> Vec<u8> {
    let mut source = SliceSource::new(bytes);
    archive
        .open_entry(index)
        .expect("entry opens")
        .read_to_vec(&mut source, &NeverCancelled)
        .expect("entry extracts")
}

#[test]
fn implode_round_trips_for_every_dictionary_and_literal_mode() {
    let plain: Vec<u8> = (0..4096u32)
        .map(|index| u8::try_from((index * 7 + index / 13) % 251).unwrap_or(0))
        .collect();
    for dictionary in 4u8..=6 {
        for coded in [false, true] {
            let stream = implode(&plain, dictionary, coded);
            let out = explode_to_vec(&stream, 1 << 20).expect("stream explodes");
            assert_eq!(
                out, plain,
                "dictionary {dictionary}, coded literals {coded}"
            );
        }
    }
}

#[test]
fn implode_round_trips_highly_repetitive_data() {
    let plain = b"ABCABCABC".repeat(2000);
    let stream = implode(&plain, 6, false);
    assert!(stream.len() < plain.len(), "matches were actually emitted");
    assert_eq!(explode_to_vec(&stream, 1 << 20).unwrap(), plain);
}

#[test]
fn implode_round_trips_an_empty_input() {
    let stream = implode(&[], 6, false);
    assert_eq!(explode_to_vec(&stream, 1 << 20).unwrap(), Vec::<u8>::new());
}

#[test]
fn implode_round_trips_a_single_byte() {
    let stream = implode(b"Z", 4, true);
    assert_eq!(explode_to_vec(&stream, 1 << 20).unwrap(), b"Z");
}

#[test]
fn the_sample_archive_lists_its_directories_and_entries() {
    let fixture = sample_archive();
    let archive = open(&fixture.bytes, 0);
    assert_eq!(archive.header().directory_count, 2);
    assert_eq!(archive.header().entry_count, 3);
    assert_eq!(archive.directories().len(), 2);
    assert_eq!(archive.entries().len(), 3);
    assert_eq!(archive.directories()[0].name.as_bytes(), b"");
    assert_eq!(archive.directories()[1].name.as_bytes(), b"MAPS");
    assert_eq!(archive.entries()[0].name.as_bytes(), b"NOTES.TXT");
    assert_eq!(archive.entries()[0].directory_index, 0);
    assert_eq!(archive.entries()[2].directory_index, 1);
    assert!(archive.entries()[1].is_stored());
    assert!(!archive.entries()[0].is_stored());
}

#[test]
fn the_sample_archive_extracts_every_entry() {
    let fixture = sample_archive();
    let mut archive = open(&fixture.bytes, 0);
    assert_eq!(
        extract(&mut archive, &fixture.bytes, 0),
        b"orange crate notes, invented\n"
    );
    assert_eq!(extract(&mut archive, &fixture.bytes, 1), vec![0xa5u8; 300]);
    assert_eq!(
        extract(&mut archive, &fixture.bytes, 2),
        b"repeat repeat repeat repeat repeat repeat repeat"
    );
}

#[test]
fn an_archive_embedded_at_a_base_offset_is_found_and_read() {
    let fixture = sample_archive();
    // An invented "overlay": filler bytes, then the archive, then more.
    let mut container = vec![0x90u8; 5_000];
    container.extend_from_slice(&fixture.bytes);
    container.extend_from_slice(&[0x00u8; 1_234]);

    let mut source = SliceSource::new(&container);
    let base = find_signature(&mut source, &Limits::default(), &NeverCancelled)
        .expect("scan succeeds")
        .expect("signature is present");
    assert_eq!(base, 5_000);

    let mut archive = open(&container, base);
    assert_eq!(archive.base_offset(), 5_000);
    assert_eq!(extract(&mut archive, &container, 1), vec![0xa5u8; 300]);
}

#[test]
fn streaming_reads_produce_the_same_bytes_as_one_shot_reads() {
    let mut builder = ArchiveBuilder::new();
    let root = builder.directory(b"");
    let payload: Vec<u8> = (0..200_000u32)
        .map(|index| u8::try_from((index / 97) % 253).unwrap_or(0))
        .collect();
    builder.entry(root, b"BIG.DAT", &payload, false);
    let fixture = builder.build();

    let mut archive = open(&fixture.bytes, 0);
    let mut source = SliceSource::new(&fixture.bytes);
    let mut reader = archive.open_entry(0).expect("entry opens");
    let mut out = Vec::new();
    let mut buffer = [0u8; 997];
    loop {
        let read = reader
            .read(&mut source, &NeverCancelled, &mut buffer)
            .expect("read succeeds");
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..read]);
    }
    assert_eq!(out, payload);
    assert_eq!(reader.written(), payload.len() as u64);
    assert_eq!(reader.expanded_size(), payload.len() as u64);
}

#[test]
fn entry_names_and_extensions_stay_bounded_byte_strings() {
    let fixture = sample_archive();
    let archive = open(&fixture.bytes, 0);
    let entry = &archive.entries()[2];
    assert_eq!(entry.name.extension_bytes(), b"bsp".to_vec());
    // The `Debug` rendering of a name never contains the name itself.
    let rendered = format!("{:?}", entry.name);
    assert_eq!(rendered, "Name(9 bytes)");
}

#[test]
fn a_dictionary_smaller_than_the_window_still_round_trips_through_an_archive() {
    let mut builder = ArchiveBuilder::new()
        .dictionary_code(4)
        .coded_literals(true);
    let root = builder.directory(b"DATA");
    let payload = b"small dictionary payload ".repeat(400);
    builder.entry(root, b"S.TXT", &payload, false);
    let fixture = builder.build();
    let mut archive = open(&fixture.bytes, 0);
    assert_eq!(extract(&mut archive, &fixture.bytes, 0), payload);
}
