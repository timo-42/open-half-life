//! `cargo fuzz` target that exercises `GameSave::from_bytes`'s per-section
//! `postcard` decoders through a validly-framed container.
//!
//! An `Arbitrary`-derived list of (tag, bytes) pairs is written through
//! `ohl_save::SaveWriter` into a container whose framing, table and digests
//! are all correct, but whose section *contents* are arbitrary bytes.
//! Occasionally a tag lands on one of `ohl_engine::save`'s own section tags
//! (16 through 27 — the M7.9 P4b sections 23-27 included), which drives
//! `postcard` decoding of that section with adversarial bytes; other tags
//! are simply carried as unknown sections. Either way `GameSave::from_bytes`
//! must never panic.

#![no_main]

use std::collections::HashSet;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ohl_engine::GameSave;
use ohl_save::{Header, Limits, MIN_APPLICATION_TAG, SaveWriter};

/// Most sections one fuzz input builds; bounded so one input cannot spend
/// its whole time budget only growing the container.
const MAX_SECTIONS: usize = 16;
/// Most bytes kept from one section's arbitrary payload.
const MAX_SECTION_BYTES: usize = 512;
/// Most tag offsets tried, so a good share of inputs land on the real
/// section tags (16 through 27) `ohl_engine::save` interprets.
const MAX_TAG_OFFSET: u8 = 11;

#[derive(Debug, Arbitrary)]
struct FuzzSection {
    tag_offset: u8,
    bytes: Vec<u8>,
}

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    sections: Vec<FuzzSection>,
}

fuzz_target!(|input: FuzzInput| {
    let header = Header {
        game_version: String::new(),
        created_at_unix_secs: 0,
        map_identity: String::new(),
        title: String::new(),
        thumbnail: Vec::new(),
    };

    let mut writer = SaveWriter::begin(header);
    let mut used_tags: HashSet<u32> = HashSet::new();
    for section in input.sections.iter().take(MAX_SECTIONS) {
        let tag = MIN_APPLICATION_TAG + u32::from(section.tag_offset % (MAX_TAG_OFFSET + 1));
        if !used_tags.insert(tag) {
            continue;
        }
        let end = section.bytes.len().min(MAX_SECTION_BYTES);
        let _ = writer.add_section(tag, &section.bytes[..end]);
    }

    let Ok(bytes) = writer.finish(&Limits::default()) else {
        return;
    };

    let _ = GameSave::from_bytes(&bytes);
});
