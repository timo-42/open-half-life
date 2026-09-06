//! `cargo fuzz` target proving round-trip identity and mutation safety.
//!
//! An `Arbitrary`-derived input builds a valid container through
//! `SaveWriter` with a handful of random sections, then flips random bytes
//! in the serialized output. Two properties are asserted: (a) the
//! unmodified writer output always reopens and reports back every section
//! exactly as written, and (b) the mutated bytes never cause
//! `SaveReader::open` (or `section` on every entry it reports) to panic,
//! even though a broken digest or table entry may now be rejected.

#![no_main]

use std::collections::HashSet;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ohl_save::{Header, Limits, MIN_APPLICATION_TAG, SaveReader, SaveWriter};

/// Most sections one fuzz input builds; bounded so one input cannot spend
/// its whole time budget only growing the container.
const MAX_SECTIONS: usize = 16;
/// Most bytes kept from one section's arbitrary payload.
const MAX_SECTION_BYTES: usize = 256;
/// Most byte-flip mutations applied to the serialized output.
const MAX_FLIPS: usize = 64;

#[derive(Debug, Arbitrary)]
struct FuzzSection {
    tag_offset: u8,
    bytes: Vec<u8>,
}

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    sections: Vec<FuzzSection>,
    title: String,
    flips: Vec<(u16, u8)>,
}

fuzz_target!(|input: FuzzInput| {
    let header = Header {
        game_version: String::new(),
        created_at_unix_secs: 0,
        map_identity: String::new(),
        title: input.title.chars().take(64).collect(),
        thumbnail: Vec::new(),
    };

    let mut writer = SaveWriter::begin(header.clone());
    let mut used_tags: HashSet<u32> = HashSet::new();
    let mut expected: Vec<(u32, Vec<u8>)> = Vec::new();
    for section in input.sections.iter().take(MAX_SECTIONS) {
        let tag = MIN_APPLICATION_TAG + u32::from(section.tag_offset);
        if !used_tags.insert(tag) {
            continue;
        }
        let end = section.bytes.len().min(MAX_SECTION_BYTES);
        let bytes = &section.bytes[..end];
        if writer.add_section(tag, bytes).is_ok() {
            expected.push((tag, bytes.to_vec()));
        }
    }

    let Ok(bytes) = writer.finish(&Limits::default()) else {
        return;
    };

    // (a) the unmodified writer output must parse successfully and
    // reproduce every section exactly.
    let reader = SaveReader::open(&bytes, &Limits::default())
        .expect("a file this crate just wrote must reopen");
    assert_eq!(reader.header(), &header);
    for (tag, section_bytes) in &expected {
        assert_eq!(
            reader.section(*tag).expect("section must be present"),
            section_bytes.as_slice()
        );
    }

    // (b) flipping random bytes must never cause a panic, whatever
    // `open`/`section` decide about the mutated bytes' validity.
    let mut mutated = bytes;
    if !mutated.is_empty() {
        for (index, xor) in input.flips.iter().take(MAX_FLIPS) {
            if *xor == 0 {
                continue;
            }
            let position = usize::from(*index) % mutated.len();
            mutated[position] ^= xor;
        }
    }
    if let Ok(mutated_reader) = SaveReader::open(&mutated, &Limits::default()) {
        for entry in mutated_reader.sections() {
            let _ = mutated_reader.section(entry.tag);
        }
    }
});
