//! Round trips over the synthetic package writer.
//!
//! Every byte under test is produced by `ohl_wise::testing`, which invents
//! its own stub, header filler, bitmap, script and file contents. No byte,
//! name or layout comes from any real medium.

use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};
use ohl_wise::{
    Chain, ChainEvent, ChecksumStatus, Error, Limits, NeverCancelled, OffsetOrigin, SliceSource,
    find_overlay, locate_first_stream, read_package,
};

fn files() -> Vec<SyntheticFile> {
    vec![
        SyntheticFile::new(b"maps\\alpha.dat", vec![0xa5u8; 8192]),
        SyntheticFile::new(b"cfg\\beta.cfg", b"one two three four ".repeat(64)),
        SyntheticFile::new(b"gamma.bin", (0..=255u8).cycle().take(3000).collect()),
    ]
}

#[test]
fn walks_a_synthetic_package_and_extracts_every_file() {
    let files = files();
    let built = build_package(&PackageOptions::with_files(files.clone()));
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("the synthetic package reads");

    assert_eq!(package.summary().streams as usize, built.stream_count);
    assert_eq!(package.summary().crc_matches as usize, built.stream_count);
    assert_eq!(package.summary().crc_mismatches, 0);
    assert_eq!(package.summary().resyncs, 0);
    assert_eq!(package.file_table().len(), files.len());
    assert_eq!(package.file_map().mapped_count(), files.len());
    assert_eq!(package.file_map().content_matched_count(), files.len());
    // Only the bitmap and the script stay unclaimed.
    assert_eq!(package.file_map().unnamed_streams(), &[0, 1]);

    for (index, file) in files.iter().enumerate() {
        let entry = package.file_map().list()[index];
        assert_eq!(entry.path_len, file.path.len());
        assert_eq!(entry.declared_inflated_size as usize, file.content.len());
        let mut reader = package.open_file(index).expect("record maps to a stream");
        let mut out = Vec::new();
        reader
            .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
            .expect("the file verifies");
        assert_eq!(out, file.content);
    }
}

#[test]
fn extracts_by_stream_index_when_the_script_names_nothing() {
    let files = files();
    let built = build_package(&PackageOptions {
        omit_records: true,
        ..PackageOptions::with_files(files.clone())
    });
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("the package still reads");

    assert!(package.file_table().is_empty());
    assert_eq!(
        package.file_map().unnamed_streams().len(),
        built.stream_count
    );

    // The bitmap is stream 0 and the script stream 1, so the files follow.
    for (index, file) in files.iter().enumerate() {
        let mut reader = package
            .open_stream(u32::try_from(index).unwrap() + 2)
            .expect("stream index resolves");
        let mut out = Vec::new();
        reader
            .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
            .expect("the stream verifies");
        assert_eq!(out, file.content);
    }
}

#[test]
fn maps_by_stored_offsets_when_records_declare_no_checksum() {
    let files = files();
    let built = build_package(&PackageOptions {
        omit_record_checksums: true,
        ..PackageOptions::with_files(files.clone())
    });
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("the package reads");
    assert_eq!(package.file_map().content_matched_count(), 0);
    assert_eq!(package.file_map().mapped_count(), files.len());
    assert_eq!(
        package.file_map().origin(),
        Some(OffsetOrigin::OverlayRelative)
    );

    let mut reader = package.open_file(0).expect("record maps to a stream");
    let mut out = Vec::new();
    reader
        .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
        .expect("the file verifies");
    assert_eq!(out, files[0].content);
}

#[test]
fn accepts_absolute_script_offsets() {
    let files = files();
    let built = build_package(&PackageOptions {
        absolute_offsets: true,
        omit_record_checksums: true,
        ..PackageOptions::with_files(files.clone())
    });
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("the package reads");
    assert_eq!(
        package.file_map().origin(),
        Some(OffsetOrigin::ImageAbsolute)
    );
    assert_eq!(package.file_map().mapped_count(), files.len());
}

#[test]
fn accepts_a_caller_supplied_overlay_offset() {
    let built = build_package(&PackageOptions::with_files(files()));
    let mut source = SliceSource::new(&built.image);
    let package = read_package(
        &mut source,
        Some(built.overlay_offset),
        Limits::DEFAULT,
        &NeverCancelled,
    )
    .expect("the package reads at the supplied offset");
    assert_eq!(package.overlay().offset, built.overlay_offset);
    assert_eq!(package.file_table().len(), 3);
}

#[test]
fn resynchronises_over_a_pad_byte_and_still_extracts() {
    let files = files();
    let built = build_package(&PackageOptions {
        pad_after_stream: Some(1),
        ..PackageOptions::with_files(files.clone())
    });
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("the package reads across the pad");

    assert_eq!(package.summary().resyncs, 1);
    assert_eq!(package.summary().crc_matches as usize, built.stream_count);
    assert_eq!(package.file_map().mapped_count(), files.len());

    let mut reader = package.open_file(0).expect("record maps to a stream");
    let mut out = Vec::new();
    reader
        .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
        .expect("the file verifies");
    assert_eq!(out, files[0].content);
}

#[test]
fn the_first_stream_is_the_bitmap_and_the_header_is_measured() {
    let built = build_package(&PackageOptions::with_files(files()));
    let mut source = SliceSource::new(&built.image);
    let overlay = find_overlay(&mut source, &Limits::DEFAULT).expect("overlay found");
    let header = locate_first_stream(&mut source, &overlay, &Limits::DEFAULT, &NeverCancelled)
        .expect("header located");
    assert_eq!(header.header_len, 157);
    assert_eq!(header.first_stream_offset, built.first_stream_offset);
    assert_eq!(header.first_stream.checksum, ChecksumStatus::Match);

    let mut chain = Chain::new(header.first_stream_offset, overlay.end(), Limits::DEFAULT);
    let mut first = Vec::new();
    while let Some(event) = chain.next_event(&mut source, &NeverCancelled) {
        if let ChainEvent::Stream(record) = event.expect("the chain walks") {
            first.push(record);
        }
    }
    assert_eq!(first.len(), built.stream_count);
    // The bitmap leads with a BITMAPINFOHEADER, whose first field is 40.
    let mut reader = ohl_wise::StreamReader::new(first[0].offset(), Limits::DEFAULT);
    let mut buffer = vec![0u8; 64];
    let read = reader
        .read(&mut source, &NeverCancelled, &mut buffer)
        .expect("the bitmap inflates");
    assert!(read >= 4);
    assert_eq!(u32::from_le_bytes(buffer[..4].try_into().unwrap()), 40);
}

#[test]
fn reports_the_zip_variant_instead_of_guessing() {
    let built = build_package(&PackageOptions {
        zip_variant: true,
        ..PackageOptions::with_files(files())
    });
    let mut source = SliceSource::new(&built.image);
    assert_eq!(
        read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled).unwrap_err(),
        Error::ZipVariantUnsupported
    );
}
