//! Round-trip and malformed-input tests over synthetic headers.
//!
//! No proprietary data is used: every byte comes from the crate's own
//! `testing` writer.

use ohl_cabinet_format::testing::{
    DESCRIPTOR_BASE, HeaderBuilder, SynthFile, put_descriptor32, put32,
};
use ohl_cabinet_format::{CabinetHeader, FormatError, Layout, Limit, Limits};

fn sample(builder: HeaderBuilder) -> HeaderBuilder {
    builder
        .directory("bin")
        .directory("data")
        .file(SynthFile::new("first.txt", 10, 0x100))
        .file(SynthFile {
            directory_index: 1,
            flags: ohl_cabinet_format::FILE_COMPRESSED,
            compressed_size: 7,
            ..SynthFile::new("second.bin", 20, 0x200)
        })
        .group("Program Files", 0, 1)
        .component("Engine", &["Program Files"])
}

fn assert_round_trip(builder: &HeaderBuilder) {
    let bytes = builder.build();
    let limits = Limits::default();
    let header = CabinetHeader::parse(&bytes, &limits).expect("parses");

    assert_eq!(header.directory_count(), 2);
    assert_eq!(header.file_count(), 2);
    assert_eq!(header.directory_name(0).unwrap(), "bin");
    assert_eq!(header.directory_name(1).unwrap(), "data");
    let names: Vec<String> = header.directories().map(Result::unwrap).collect();
    assert_eq!(names, ["bin", "data"]);

    assert_eq!(header.file_name(0).unwrap(), "first.txt");
    assert_eq!(header.file_name(1).unwrap(), "second.bin");

    let first = header.file_descriptor(0).unwrap();
    assert_eq!(first.expanded_size, 10);
    assert_eq!(first.data_offset, 0x100);
    assert_eq!(first.directory_index, 0);
    assert!(!first.flags.is_compressed());

    let second = header.file_descriptor(1).unwrap();
    assert!(second.flags.is_compressed());
    assert_eq!(second.compressed_size, 7);
    assert_eq!(second.directory_index, 1);

    let descriptors: Vec<_> = header.file_descriptors().map(Result::unwrap).collect();
    assert_eq!(descriptors.len(), 2);

    assert_eq!(header.file_groups().len(), 1);
    assert_eq!(header.file_group(0).unwrap().name, "Program Files");
    assert_eq!(header.file_group(0).unwrap().first_file, 0);
    assert_eq!(header.file_group(0).unwrap().last_file, 1);

    assert_eq!(header.components().len(), 1);
    assert_eq!(header.component(0).unwrap().name, "Engine");
    assert_eq!(
        header.component(0).unwrap().file_group_names,
        ["Program Files"]
    );
}

#[test]
fn round_trips_an_installshield_5_header() {
    let builder = sample(HeaderBuilder::v5());
    assert_eq!(builder.version().layout(), Layout::V5);
    assert!(!builder.version().is_unicode());
    assert_round_trip(&builder);
}

#[test]
fn round_trips_an_installshield_6_header() {
    let builder = sample(HeaderBuilder::v6());
    assert_eq!(builder.version().layout(), Layout::V6);
    assert_round_trip(&builder);
}

#[test]
fn round_trips_a_2003_unicode_header() {
    let builder = sample(HeaderBuilder::is2003());
    assert_eq!(builder.version().major(), 17);
    assert!(builder.version().is_unicode());
    assert_round_trip(&builder);
}

#[test]
fn decodes_non_ascii_unicode_names() {
    let bytes = HeaderBuilder::is2003()
        .directory("données")
        .file(SynthFile::new("café.txt", 1, 0x10))
        .build();
    let header = CabinetHeader::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(header.directory_name(0).unwrap(), "données");
    assert_eq!(header.file_name(0).unwrap(), "café.txt");
}

#[test]
fn rejects_a_foreign_signature() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put32(&mut bytes, 0, 0x4643_534d);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::MicrosoftCabinet)
    );
    put32(&mut bytes, 0, 0x1234_5678);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::BadSignature)
    );
}

#[test]
fn rejects_a_descriptor_offset_past_the_buffer() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put32(&mut bytes, 12, 0xffff_0000);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::OffsetOutOfRange)
    );
}

#[test]
fn rejects_a_descriptor_size_past_the_buffer() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put32(&mut bytes, 16, 0x7fff_ffff);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::OffsetOutOfRange)
    );
}

#[test]
fn rejects_a_zero_descriptor_size() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put32(&mut bytes, 16, 0);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::Malformed)
    );
}

#[test]
fn rejects_a_descriptor_smaller_than_the_fixed_arrays() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put32(&mut bytes, 16, 0x100);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::Truncated)
    );
}

#[test]
fn rejects_a_file_table_offset_past_the_buffer() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put_descriptor32(&mut bytes, 0x0c, 0xffff_0000);
    assert!(matches!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::OffsetOutOfRange | FormatError::Truncated)
    ));
}

#[test]
fn rejects_an_oversize_file_count() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put_descriptor32(&mut bytes, 0x28, 1_000_000);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::LimitExceeded(Limit::Files))
    );
}

#[test]
fn rejects_an_oversize_directory_count() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put_descriptor32(&mut bytes, 0x1c, 1_000_000);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::LimitExceeded(Limit::Directories))
    );
}

#[test]
fn rejects_a_file_count_whose_table_does_not_fit() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put_descriptor32(&mut bytes, 0x28, 100_000);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::Truncated)
    );
}

#[test]
fn rejects_an_oversize_header_buffer() {
    let bytes = sample(HeaderBuilder::v5()).build();
    let limits = Limits {
        max_header_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        CabinetHeader::parse(&bytes, &limits),
        Err(FormatError::LimitExceeded(Limit::HeaderBytes))
    );
}

#[test]
fn rejects_a_self_referential_offset_list() {
    let bytes = sample(HeaderBuilder::v5()).build();
    let head = u32::from_le_bytes(
        bytes[DESCRIPTOR_BASE + 0x3e..DESCRIPTOR_BASE + 0x42]
            .try_into()
            .unwrap(),
    );
    let mut bytes = bytes;
    // Point the node's `next` at itself.
    put32(&mut bytes, DESCRIPTOR_BASE + head as usize + 8, head);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::LinkCycle)
    );
}

#[test]
fn rejects_an_offset_list_head_past_the_buffer() {
    let mut bytes = sample(HeaderBuilder::v5()).build();
    put_descriptor32(&mut bytes, 0x3e, 0xffff_0000);
    assert_eq!(
        CabinetHeader::parse(&bytes, &Limits::default()),
        Err(FormatError::OffsetOutOfRange)
    );
}

#[test]
fn rejects_an_out_of_range_index() {
    let bytes = sample(HeaderBuilder::v5()).build();
    let header = CabinetHeader::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(header.directory_name(9), Err(FormatError::IndexOutOfRange));
    assert_eq!(header.file_descriptor(9), Err(FormatError::IndexOutOfRange));
    assert_eq!(header.file_name(9), Err(FormatError::IndexOutOfRange));
}

#[test]
fn rejects_a_name_longer_than_the_limit() {
    let bytes = HeaderBuilder::v5()
        .directory("a-fairly-long-directory-name")
        .build();
    let limits = Limits {
        max_name_bytes: 4,
        ..Limits::default()
    };
    let header = CabinetHeader::parse(&bytes, &limits).unwrap();
    assert_eq!(
        header.directory_name(0),
        Err(FormatError::LimitExceeded(Limit::NameBytes))
    );
}

#[test]
fn rejects_a_null_string_offset() {
    let bytes = sample(HeaderBuilder::v5()).build();
    let header = CabinetHeader::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(
        header.descriptor_string(0),
        Err(FormatError::OffsetOutOfRange)
    );
}

#[test]
fn truncating_the_buffer_never_panics() {
    let bytes = sample(HeaderBuilder::v6()).build();
    for length in 0..bytes.len() {
        let _ = CabinetHeader::parse(&bytes[..length], &Limits::default());
    }
}

#[test]
fn flipping_any_single_byte_never_panics() {
    let bytes = sample(HeaderBuilder::v6()).build();
    for index in 0..bytes.len() {
        let mut corrupt = bytes.clone();
        corrupt[index] ^= 0xff;
        if let Ok(header) = CabinetHeader::parse(&corrupt, &Limits::default()) {
            for name in header.directories() {
                let _ = name;
            }
            for descriptor in header.file_descriptors() {
                let _ = descriptor;
            }
            for index in 0..header.file_count() {
                let _ = header.file_name(index);
            }
        }
    }
}

#[test]
fn forced_versions_override_the_stored_word() {
    let bytes = sample(HeaderBuilder::v5()).build();
    // Forcing 6 makes the V5 body parse under the V6 layout, which must fail
    // cleanly rather than read out of bounds.
    let forced = CabinetHeader::parse_forced_version(&bytes, &Limits::default(), 6);
    if let Ok(header) = forced {
        for descriptor in header.file_descriptors() {
            let _ = descriptor;
        }
    }
}

#[test]
fn header_index_sets_the_volume_of_v5_descriptors() {
    let bytes = sample(HeaderBuilder::v5()).build();
    let header = CabinetHeader::parse(&bytes, &Limits::default())
        .unwrap()
        .with_header_index(3);
    assert_eq!(header.file_descriptor(0).unwrap().volume, 3);
}
