//! Round-trip and malformed-input extraction tests over synthetic cabinets.
//!
//! No proprietary data is used: every byte comes from the crates' own
//! `testing` writers.

use ohl_cabinet::testing::{CabinetBuilder, SynthCabinet, SynthEntry};
use ohl_cabinet::{CabinetReader, Error, Limit, Limits, VolumeSource};
use ohl_cabinet_format::{CabinetHeader, LINK_PREV, Limits as FormatLimits};

fn read_four<S: VolumeSource>(mut source: S) -> usize {
    let mut buffer = [0u8; 4];
    source.read_at(1, 0, &mut buffer).unwrap()
}

fn payload(len: usize, salt: u8) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from(index % 251).unwrap().wrapping_mul(7) ^ salt)
        .collect()
}

fn extract(cabinet: &SynthCabinet, index: u32, limits: Limits) -> Result<Vec<u8>, Error> {
    let header_bytes = cabinet.header.clone();
    let header = CabinetHeader::parse(&header_bytes, &FormatLimits::default())?;
    let mut source = cabinet.clone();
    let mut reader = CabinetReader::new(&header, limits);
    reader.extract_to_vec(index, &mut source)
}

#[test]
fn extracts_stored_files_from_a_v5_cabinet() {
    let first = payload(300, 0x11);
    let second = payload(64, 0x22);
    let cabinet = CabinetBuilder::v5()
        .directory("bin")
        .entry(SynthEntry::new("first", &first))
        .entry(SynthEntry::new("second", &second))
        .build();

    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), first);
    assert_eq!(extract(&cabinet, 1, Limits::default()).unwrap(), second);
}

#[test]
fn extracts_stored_files_from_a_v6_cabinet() {
    let data = payload(4096, 0x5a);
    let cabinet = CabinetBuilder::v6()
        .directory("bin")
        .entry(SynthEntry::new("only", &data))
        .build();
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn extracts_compressed_files() {
    let data = payload(50_000, 0x3c);
    let cabinet = CabinetBuilder::v6()
        .chunk_bytes(4096)
        .entry(SynthEntry::new("packed", &data).compressed())
        .build();
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn extracts_obfuscated_files() {
    let data = payload(2_000, 0x77);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("hidden", &data).obfuscated())
        .build();
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn extracts_compressed_and_obfuscated_files() {
    let data = payload(30_000, 0x0f);
    let cabinet = CabinetBuilder::v6()
        .chunk_bytes(2048)
        .entry(SynthEntry::new("both", &data).compressed().obfuscated())
        .build();
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn extracts_unicode_named_files_from_a_2003_cabinet() {
    let data = payload(9_000, 0x91);
    let cabinet = CabinetBuilder::is2003()
        .directory("données")
        .chunk_bytes(1024)
        .entry(SynthEntry::new("café.bin", &data).compressed())
        .build();

    let header = CabinetHeader::parse(&cabinet.header, &FormatLimits::default()).unwrap();
    assert!(header.is_unicode());
    assert_eq!(header.file_name(0).unwrap(), "café.bin");
    assert_eq!(header.directory_name(0).unwrap(), "données");
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn reassembles_a_file_split_across_two_volumes() {
    let data = payload(10_000, 0x1d);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("split", &data).split_after(4_321))
        .build();
    assert_eq!(cabinet.volumes.len(), 2);
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn reassembles_a_compressed_obfuscated_split_file() {
    let data = payload(40_000, 0x64);
    let cabinet = CabinetBuilder::v6()
        .chunk_bytes(1024)
        .entry(
            SynthEntry::new("split", &data)
                .compressed()
                .obfuscated()
                .split_after(3_000),
        )
        .build();
    assert_eq!(cabinet.volumes.len(), 2);
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[test]
fn reassembles_a_v5_split_file_whose_flag_is_inferred() {
    let first = payload(100, 0x01);
    let split = payload(8_000, 0x02);
    let cabinet = CabinetBuilder::v5()
        .entry(SynthEntry::new("first", &first))
        .entry(SynthEntry::new("split", &split).split_after(2_500))
        .build();
    assert_eq!(cabinet.volumes.len(), 2);
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), first);
    assert_eq!(extract(&cabinet, 1, Limits::default()).unwrap(), split);
}

#[test]
fn follows_a_previous_split_link_to_its_head() {
    let head = payload(512, 0x40);
    let tail = payload(512, 0x41);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("head", &head).with_link(0, 1, 2))
        .entry(SynthEntry::new("tail", &tail).with_link(0, 0, LINK_PREV))
        .build();

    // Opening the continuation resolves back to the head descriptor.
    assert_eq!(extract(&cabinet, 1, Limits::default()).unwrap(), head);
}

#[test]
fn rejects_a_split_link_cycle() {
    let data = payload(64, 0x50);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("a", &data).with_link(1, 0, LINK_PREV))
        .entry(SynthEntry::new("b", &data).with_link(0, 0, LINK_PREV))
        .build();
    assert_eq!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::LinkCycle)
    );
}

#[test]
fn rejects_a_self_referential_split_link() {
    let data = payload(64, 0x51);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("a", &data).with_link(0, 0, LINK_PREV))
        .build();
    assert_eq!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::LinkCycle)
    );
}

#[test]
fn enforces_the_link_step_limit() {
    let data = payload(16, 0x52);
    let mut builder = CabinetBuilder::v6();
    for index in 0..8u32 {
        // Every descriptor continues the next one, so the walk never ends
        // within the step budget.
        builder =
            builder.entry(SynthEntry::new("chain", &data).with_link((index + 1) % 8, 0, LINK_PREV));
    }
    let cabinet = builder.build();
    let limits = Limits {
        max_link_steps: 3,
        ..Limits::default()
    };
    assert!(matches!(
        extract(&cabinet, 0, limits),
        Err(Error::LimitExceeded(Limit::LinkSteps) | Error::LinkCycle)
    ));
}

#[test]
fn enforces_the_per_file_expanded_limit() {
    let data = payload(4_096, 0x60);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("big", &data))
        .build();
    let limits = Limits {
        max_expanded_bytes_per_file: 1_024,
        ..Limits::default()
    };
    assert_eq!(
        extract(&cabinet, 0, limits),
        Err(Error::LimitExceeded(Limit::ExpandedBytesPerFile))
    );
}

#[test]
fn enforces_the_total_expanded_limit() {
    let data = payload(4_096, 0x61);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("a", &data))
        .entry(SynthEntry::new("b", &data))
        .build();

    let header_bytes = cabinet.header.clone();
    let header = CabinetHeader::parse(&header_bytes, &FormatLimits::default()).unwrap();
    let mut source = cabinet.clone();
    let limits = Limits {
        max_total_expanded_bytes: 5_000,
        ..Limits::default()
    };
    let mut reader = CabinetReader::new(&header, limits);
    assert_eq!(reader.extract_to_vec(0, &mut source).unwrap().len(), 4_096);
    assert_eq!(
        reader.extract_to_vec(1, &mut source),
        Err(Error::LimitExceeded(Limit::TotalExpandedBytes))
    );
}

#[test]
fn enforces_the_chunk_size_limit() {
    let data = payload(20_000, 0x62);
    let cabinet = CabinetBuilder::v6()
        .chunk_bytes(8_192)
        .entry(SynthEntry::new("packed", &data).compressed())
        .build();
    let limits = Limits {
        max_chunk_bytes: 64,
        ..Limits::default()
    };
    assert!(matches!(
        extract(&cabinet, 0, limits),
        Err(Error::LimitExceeded(Limit::ChunkBytes) | Error::DecompressionFailed)
    ));
}

#[test]
fn enforces_the_volume_limit_when_reassembling() {
    let data = payload(8_000, 0x63);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("split", &data).split_after(2_000))
        .build();
    let limits = Limits {
        max_volumes: 1,
        ..Limits::default()
    };
    assert_eq!(
        extract(&cabinet, 0, limits),
        Err(Error::LimitExceeded(Limit::Volumes))
    );
}

#[test]
fn reports_a_truncated_volume() {
    let data = payload(4_000, 0x70);
    let mut cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("cut", &data))
        .build();
    cabinet.volumes[0].truncate(1_000);
    assert_eq!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::TruncatedVolume)
    );
}

#[test]
fn reports_a_volume_without_a_volume_header() {
    let data = payload(64, 0x71);
    let mut cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("cut", &data))
        .build();
    cabinet.volumes[0].truncate(24);
    assert_eq!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::MalformedVolumeHeader)
    );
}

#[test]
fn reports_a_volume_without_a_common_header() {
    let data = payload(64, 0x72);
    let mut cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("cut", &data))
        .build();
    cabinet.volumes[0].truncate(4);
    assert_eq!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::TruncatedVolume)
    );
}

#[test]
fn reports_a_volume_with_a_foreign_signature() {
    let data = payload(64, 0x73);
    let mut cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("cut", &data))
        .build();
    cabinet.volumes[0][..4].copy_from_slice(&0x4643_534du32.to_le_bytes());
    assert!(matches!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::Header(_))
    ));
}

#[test]
fn reports_a_missing_volume() {
    let data = payload(64, 0x74);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("elsewhere", &data).in_volume(4))
        .build();
    let mut trimmed = cabinet.clone();
    trimmed.volumes.truncate(1);
    assert!(matches!(
        extract(&trimmed, 0, Limits::default()),
        Err(Error::Volume(_))
    ));
}

#[test]
fn reports_an_expanded_size_mismatch() {
    let data = payload(1_000, 0x80);
    let mut cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("short", &data))
        .build();
    // Shrink the stored bytes without touching the descriptor.
    let shorter = cabinet.volumes[0].len() - 400;
    cabinet.volumes[0].truncate(shorter);
    assert!(matches!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::TruncatedVolume | Error::SizeMismatch)
    ));
}

#[test]
fn streams_in_caller_sized_chunks() {
    let data = payload(9_999, 0x90);
    let cabinet = CabinetBuilder::v6()
        .chunk_bytes(1_024)
        .entry(SynthEntry::new("stream", &data).compressed())
        .build();

    let header_bytes = cabinet.header.clone();
    let header = CabinetHeader::parse(&header_bytes, &FormatLimits::default()).unwrap();
    let mut source = cabinet.clone();
    let mut reader = CabinetReader::new(&header, Limits::default());
    let mut file = reader.open(0, &mut source).unwrap();
    assert_eq!(file.index(), 0);
    assert_eq!(file.volume(), 1);

    let mut output = Vec::new();
    let mut buffer = [0u8; 7];
    loop {
        let read = file.read(&mut source, &mut buffer).unwrap();
        if read == 0 {
            break;
        }
        assert!(read <= buffer.len());
        output.extend_from_slice(&buffer[..read]);
    }
    assert_eq!(file.expanded_bytes(), data.len() as u64);
    file.finish().unwrap();
    assert_eq!(output, data);
}

#[test]
fn reads_the_volume_source_through_a_mutable_reference() {
    let data = payload(128, 0xa0);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("x", &data))
        .build();
    let mut source = cabinet.clone();
    // Exercises the blanket `impl VolumeSource for &mut T`.
    assert_eq!(read_four(&mut source), 4);
}

#[cfg(feature = "md5")]
#[test]
fn verifies_the_recorded_digest() {
    let data = payload(3_000, 0xb0);
    let cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("checked", &data))
        .build();
    assert_eq!(extract(&cabinet, 0, Limits::default()).unwrap(), data);
}

#[cfg(feature = "md5")]
#[test]
fn rejects_a_wrong_digest() {
    let data = payload(3_000, 0xb1);
    let mut cabinet = CabinetBuilder::v6()
        .entry(SynthEntry::new("checked", &data))
        .build();
    // Corrupt the stored bytes so the digest no longer matches, keeping the
    // length intact so the size check still passes.
    let last = cabinet.volumes[0].len() - 1;
    cabinet.volumes[0][last] ^= 0xff;
    assert_eq!(
        extract(&cabinet, 0, Limits::default()),
        Err(Error::DigestMismatch)
    );
}
