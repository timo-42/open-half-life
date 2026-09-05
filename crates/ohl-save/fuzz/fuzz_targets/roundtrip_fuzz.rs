//! `cargo fuzz` target proving round-trip identity: whatever header and
//! section bytes the fuzz input derives, `SaveWriter::finish` followed by
//! `SaveReader::open` must reproduce them exactly. Must never panic on any
//! input, and never silently disagree with what was written.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_save::{Header, Limits, SaveReader, SaveWriter, MIN_APPLICATION_TAG};

fn run(data: &[u8]) {
    if data.len() < 2 {
        return;
    }
    let (tag_byte, rest) = data.split_at(1);
    let tag = MIN_APPLICATION_TAG + u32::from(tag_byte[0]);
    let split = rest.len() / 2;
    let (section, title_bytes) = rest.split_at(split);

    let title: String = String::from_utf8_lossy(title_bytes).chars().take(64).collect();
    let header = Header {
        game_version: String::new(),
        created_at_unix_secs: 0,
        map_identity: String::new(),
        title,
        thumbnail: Vec::new(),
    };

    let mut writer = SaveWriter::begin(header.clone());
    if writer.add_section(tag, section).is_err() {
        return;
    }
    let Ok(bytes) = writer.finish(&Limits::default()) else {
        return;
    };

    let reader =
        SaveReader::open(&bytes, &Limits::default()).expect("a file this crate just wrote must reopen");
    assert_eq!(reader.header(), &header);
    assert_eq!(reader.section(tag).expect("section must be present"), section);
}

fuzz_target!(|data: &[u8]| {
    run(data);
});
