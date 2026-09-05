//! Integration tests exercising `ohl-save` through its public API only, as
//! an external crate would.

use ohl_save::{
    AUTOSAVE_SLOT_NAME, Header, Limits, QUICKSAVE_SLOT_NAME, SaveError, SaveReader, SaveSlot,
    SaveWriter,
};

fn sample_header(title: &str) -> Header {
    Header {
        game_version: "0.1.0".to_string(),
        created_at_unix_secs: 1_700_000_000,
        map_identity: "sample-map".to_string(),
        title: title.to_string(),
        thumbnail: Vec::new(),
    }
}

#[test]
fn round_trip_via_public_api() {
    let mut writer = SaveWriter::begin(sample_header("Round Trip"));
    writer.add_section(16, b"player-state-bytes").unwrap();
    writer
        .add_section_serde(17, &vec!["one".to_string(), "two".to_string()])
        .unwrap();
    let bytes = writer.finish(&Limits::default()).unwrap();

    let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
    assert_eq!(reader.header().title, "Round Trip");
    assert_eq!(reader.section(16).unwrap(), b"player-state-bytes");
    assert_eq!(
        reader.deserialize::<Vec<String>>(17).unwrap(),
        vec!["one".to_string(), "two".to_string()]
    );
}

#[test]
fn corrupted_file_is_rejected_not_panicked_on() {
    let mut writer = SaveWriter::begin(sample_header("Tamper"));
    writer.add_section(16, b"data").unwrap();
    let mut bytes = writer.finish(&Limits::default()).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    assert_eq!(
        SaveReader::open(&bytes, &Limits::default()).unwrap_err(),
        SaveError::TrailerMismatch
    );
}

#[test]
fn save_slot_listing_round_trips_through_a_temp_directory() {
    let dir = tempfile::tempdir().unwrap();
    let slot = SaveSlot::new(dir.path());

    let mut autosave = SaveWriter::begin(sample_header("Autosave"));
    autosave.add_section(16, b"auto").unwrap();
    slot.write(
        AUTOSAVE_SLOT_NAME,
        &autosave.finish(&Limits::default()).unwrap(),
    )
    .unwrap();

    let mut quicksave = SaveWriter::begin(sample_header("Quicksave"));
    quicksave.add_section(16, b"quick").unwrap();
    slot.write(
        QUICKSAVE_SLOT_NAME,
        &quicksave.finish(&Limits::default()).unwrap(),
    )
    .unwrap();

    let mut listing = slot.list(&Limits::default()).unwrap();
    listing.sort_by(|left, right| left.name.cmp(&right.name));
    let names: Vec<_> = listing.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec![AUTOSAVE_SLOT_NAME, QUICKSAVE_SLOT_NAME]);

    slot.delete(AUTOSAVE_SLOT_NAME).unwrap();
    let remaining = slot.list(&Limits::default()).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, QUICKSAVE_SLOT_NAME);
}
