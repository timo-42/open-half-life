//! Round-trip tests over synthetic cabinets.
//!
//! Every byte here is produced by this crate's own writer
//! (`ohl_mscab::test_support`); no cabinet, name, or payload from any real
//! medium appears in this repository.

use ohl_mscab::test_support::{
    BlockSpec, CabinetSpec, Continuation, FileSpec, FolderSpec, Method, build, filler, mszip_block,
};
use ohl_mscab::{
    ATTR_ARCHIVE, ATTR_NAME_IS_UTF, CabError, Cabinet, Compression, DateTime, FileStream,
    FolderRef, FolderSegment, FolderStream, Limits, NeverCancelled, SliceSetSource, SliceSource,
    extract_file,
};

/// Reads a whole folder stream into a vector.
fn drain(stream: &mut FolderStream<'_, SliceSource<'_>>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = [0u8; 777];
    loop {
        let read = stream.read(&mut buffer, &NeverCancelled).expect("read");
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..read]);
    }
    out
}

fn read_file(bytes: &[u8], index: usize) -> Vec<u8> {
    let source = SliceSource::new(bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let file = &cabinet.files()[index];
    let folder_index = file.folder.index_in(cabinet.header().folder_count).unwrap();
    let compression = cabinet.folders()[folder_index as usize].compression;
    let segment = FolderSegment::new(&cabinet, 0, folder_index).expect("segment");
    let mut out = Vec::new();
    extract_file(
        &source,
        compression,
        Limits::default(),
        vec![segment],
        file,
        &NeverCancelled,
        |chunk| {
            out.extend_from_slice(chunk);
            Ok(())
        },
    )
    .expect("extract");
    out
}

#[test]
fn parses_a_stored_cabinet_and_round_trips_every_file() {
    let files = vec![
        FileSpec::new("alpha.dat", filler(1_000, 1)),
        FileSpec::new("beta.dat", filler(2_500, 2)),
        FileSpec::new("gamma.dat", Vec::new()),
    ];
    let built = build(&CabinetSpec::new(vec![FolderSpec::new(
        Method::Stored,
        files.clone(),
    )]));

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    assert_eq!(cabinet.header().version_major, 1);
    assert_eq!(cabinet.header().version_minor, 3);
    assert_eq!(cabinet.header().folder_count, 1);
    assert_eq!(cabinet.header().file_count, 3);
    assert!(!cabinet.header().has_reserved_areas());
    assert_eq!(cabinet.folders()[0].compression, Compression::None);
    assert_eq!(cabinet.files()[1].name_bytes(), b"beta.dat");
    assert_eq!(cabinet.files()[1].folder, FolderRef::Index(0));
    assert_eq!(cabinet.files_in_folder(0).count(), 3);

    let mut stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    assert_eq!(drain(&mut stream), built.folder_streams[0]);

    for (index, file) in files.iter().enumerate() {
        assert_eq!(read_file(&built.bytes, index), file.data);
    }
}

#[test]
fn round_trips_a_multi_block_mszip_folder() {
    let files = vec![
        FileSpec::new("one.bin", filler(40_000, 7)),
        FileSpec::new("two.bin", filler(30_000, 9)),
    ];
    let mut folder = FolderSpec::new(Method::MsZip, files.clone());
    folder.block_size = 4_096;
    let built = build(&CabinetSpec::new(vec![folder]));

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    assert_eq!(cabinet.folders()[0].compression, Compression::MsZip);
    assert!(cabinet.folders()[0].data_block_count > 15);

    let mut stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    let decoded = drain(&mut stream);
    assert_eq!(decoded, built.folder_streams[0]);
    let stats = stream.stats();
    assert_eq!(
        stats.blocks_read,
        u64::from(cabinet.folders()[0].data_block_count)
    );
    assert_eq!(stats.checksums_verified, stats.blocks_read);
    assert_eq!(stats.checksums_absent, 0);
    assert_eq!(stats.uncompressed_bytes, 70_000);

    assert_eq!(read_file(&built.bytes, 0), files[0].data);
    assert_eq!(read_file(&built.bytes, 1), files[1].data);
}

#[test]
fn round_trips_several_folders_with_different_methods() {
    let spec = CabinetSpec::new(vec![
        FolderSpec::new(
            Method::MsZip,
            vec![FileSpec::new("first.bin", filler(9_000, 3))],
        ),
        FolderSpec::new(
            Method::Stored,
            vec![
                FileSpec::new("second.bin", filler(120, 4)),
                FileSpec::new("third.bin", filler(300, 5)),
            ],
        ),
    ]);
    let built = build(&spec);
    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    assert_eq!(cabinet.header().folder_count, 2);
    assert_eq!(cabinet.files_in_folder(1).count(), 2);

    for folder_index in 0..2u16 {
        let mut stream =
            FolderStream::from_cabinet(&cabinet, &source, 0, folder_index, Limits::default())
                .expect("folder");
        assert_eq!(
            drain(&mut stream),
            built.folder_streams[folder_index as usize]
        );
    }
    for index in 0..3 {
        assert_eq!(
            read_file(&built.bytes, index),
            spec.folders
                .iter()
                .flat_map(|folder| folder.files.iter())
                .nth(index)
                .unwrap()
                .data
        );
    }
}

#[test]
fn honours_reserved_areas_in_the_header_folders_and_blocks() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::MsZip,
        vec![FileSpec::new("reserved.bin", filler(5_000, 11))],
    )]);
    spec.header_reserve = 40;
    spec.folder_reserve = 6;
    spec.data_reserve = 4;
    spec.folders[0].block_size = 1_024;
    let built = build(&spec);

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    assert!(cabinet.header().has_reserved_areas());
    assert_eq!(cabinet.header().header_reserve_bytes, 40);
    assert_eq!(cabinet.header().folder_reserve_bytes, 6);
    assert_eq!(cabinet.header().data_reserve_bytes, 4);

    let mut stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    assert_eq!(drain(&mut stream), built.folder_streams[0]);
}

#[test]
fn reports_only_the_length_of_the_optional_cabinet_names() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::Stored,
        vec![FileSpec::new("named.bin", filler(64, 13))],
    )]);
    spec.previous_names = Some((b"prev-volume.cab".to_vec(), b"Disc One".to_vec()));
    spec.next_names = Some((b"next-volume.cab".to_vec(), b"Disc Two".to_vec()));
    let built = build(&spec);

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let header = cabinet.header();
    assert!(header.has_previous_cabinet() && header.has_next_cabinet());
    assert_eq!(header.previous_cabinet_name_len, Some(15));
    assert_eq!(header.previous_disk_name_len, Some(8));
    assert_eq!(header.next_cabinet_name_len, Some(15));
    assert_eq!(header.next_disk_name_len, Some(8));
}

#[test]
fn resolves_a_file_that_spans_two_cabinets_of_a_set() {
    // The first cabinet holds the head of the folder; the second continues it.
    let head = filler(20_000, 21);
    let tail = filler(15_000, 22);
    let mut whole = head.clone();
    whole.extend_from_slice(&tail);

    let mut first_file = FileSpec::new("spanning.bin", head.clone());
    first_file.continuation = Continuation::ToNext;
    let mut first = CabinetSpec::new(vec![FolderSpec::new(Method::MsZip, vec![first_file])]);
    first.folders[0].block_size = 8_192;
    first.next_names = Some((b"volume-two.cab".to_vec(), b"Disc Two".to_vec()));
    first.cabinet_index = 0;
    let cabinet_a = build(&first);

    let mut second_file = FileSpec::new("spanning.bin", tail.clone());
    second_file.continuation = Continuation::FromPrevious;
    let mut second = CabinetSpec::new(vec![FolderSpec::new(Method::MsZip, vec![second_file])]);
    second.folders[0].block_size = 8_192;
    second.previous_names = Some((b"volume-one.cab".to_vec(), b"Disc One".to_vec()));
    second.cabinet_index = 1;
    let cabinet_b = build(&second);

    let volumes: [&[u8]; 2] = [&cabinet_a.bytes, &cabinet_b.bytes];
    let source = SliceSetSource::new(&volumes);
    let parsed_a = Cabinet::parse(&source, 0, &Limits::default()).expect("parse a");
    let parsed_b = Cabinet::parse(&source, 1, &Limits::default()).expect("parse b");
    assert_eq!(parsed_a.header().set_id, parsed_b.header().set_id);
    assert!(parsed_a.files()[0].folder.continues_to_next());
    assert!(parsed_b.files()[0].folder.continues_from_previous());

    // The caller maps the continuation onto its own volume indices.
    let segments = vec![
        FolderSegment::new(&parsed_a, 0, 0).expect("segment a"),
        FolderSegment::new(&parsed_b, 1, 0).expect("segment b"),
    ];
    let mut stream = FolderStream::new(
        &source,
        Compression::MsZip,
        Limits::default(),
        segments.clone(),
    )
    .expect("stream");
    let mut out = Vec::new();
    let mut buffer = [0u8; 4_096];
    loop {
        let read = stream.read(&mut buffer, &NeverCancelled).expect("read");
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..read]);
    }
    assert_eq!(out, whole);

    // A caller that does not supply the continuation volume gets the head
    // only, and the file read reports truncation rather than silent short
    // data.
    let head_only = SliceSource::new(&cabinet_a.bytes);
    let stream =
        FolderStream::from_cabinet(&parsed_a, &head_only, 0, 0, Limits::default()).expect("stream");
    let mut file =
        FileStream::seek_to(stream, &parsed_a.files()[0], &NeverCancelled).expect("seek");
    let mut sink = Vec::new();
    let mut buffer = [0u8; 4_096];
    loop {
        match file.read(&mut buffer, &NeverCancelled) {
            Ok(0) => break,
            Ok(read) => sink.extend_from_slice(&buffer[..read]),
            Err(error) => {
                assert_eq!(error, CabError::Truncated);
                break;
            }
        }
    }
    assert_eq!(sink, head);
}

#[test]
fn mszip_history_is_carried_across_blocks() {
    // A hand-built second MSZIP block whose only token is a match pointing
    // 100 bytes back, i.e. into the *previous* block's output. It can only
    // decode if the DEFLATE history buffer is maintained across MSZIP blocks,
    // as [MS-MCI] requires.
    let first = filler(200, 31);
    let mut writer = DeflateBits::default();
    writer.bits(1, 1); // BFINAL
    writer.bits(1, 2); // BTYPE = 01, fixed Huffman
    writer.code(0b000_0001, 7); // length symbol 257 => match length 3
    writer.code(13, 5); // distance symbol 13 => base 97, 5 extra bits
    writer.bits(3, 5); // extra bits => distance 100
    writer.code(0, 7); // end-of-block symbol 256
    let mut second = ohl_mscab::MSZIP_SIGNATURE.to_vec();
    second.extend_from_slice(&writer.finish());

    let expected_tail = first[100..103].to_vec();
    let mut whole = first.clone();
    whole.extend_from_slice(&expected_tail);

    let mut folder = FolderSpec::new(
        Method::MsZip,
        vec![FileSpec::new("hist.bin", whole.clone())],
    );
    folder.blocks = Some(vec![
        BlockSpec {
            compressed: mszip_block(&first),
            uncompressed_len: 200,
        },
        BlockSpec {
            compressed: second,
            uncompressed_len: 3,
        },
    ]);
    let built = build(&CabinetSpec::new(vec![folder]));

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let mut stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    assert_eq!(drain(&mut stream), whole);
}

/// A minimal DEFLATE bit writer (LSB-first, Huffman codes MSB-first), used
/// only to hand-build the history test above.
#[derive(Default)]
struct DeflateBits {
    bytes: Vec<u8>,
    accumulator: u32,
    bits: u32,
}

impl DeflateBits {
    fn bits(&mut self, value: u32, count: u32) {
        self.accumulator |= value << self.bits;
        self.bits += count;
        while self.bits >= 8 {
            self.bytes.push((self.accumulator & 0xFF) as u8);
            self.accumulator >>= 8;
            self.bits -= 8;
        }
    }

    fn code(&mut self, value: u32, count: u32) {
        let mut reversed = 0u32;
        for index in 0..count {
            reversed |= ((value >> index) & 1) << (count - 1 - index);
        }
        self.bits(reversed, count);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.bytes.push((self.accumulator & 0xFF) as u8);
        }
        self.bytes
    }
}

#[test]
fn absent_checksums_are_reported_rather_than_rejected() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::Stored,
        vec![FileSpec::new("nosum.bin", filler(500, 41))],
    )]);
    spec.write_checksums = false;
    let built = build(&spec);
    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let mut stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    assert_eq!(drain(&mut stream), built.folder_streams[0]);
    assert_eq!(stream.stats().checksums_verified, 0);
    assert_eq!(stream.stats().checksums_absent, 1);
}

#[test]
fn cancellation_stops_between_blocks() {
    struct Always;
    impl ohl_mscab::Cancellation for Always {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    let mut folder = FolderSpec::new(
        Method::MsZip,
        vec![FileSpec::new("cancel.bin", filler(20_000, 51))],
    );
    folder.block_size = 1_024;
    let built = build(&CabinetSpec::new(vec![folder]));
    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let mut stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    let mut buffer = [0u8; 64];
    assert_eq!(stream.read(&mut buffer, &Always), Err(CabError::Cancelled));
}

#[test]
fn exposes_utf8_names_and_decoded_timestamps() {
    let mut utf8 = FileSpec::new("uni-\u{00e9}.bin", filler(16, 61));
    utf8.attributes = ATTR_ARCHIVE | ATTR_NAME_IS_UTF;
    // 1997-03-12 11:13:52 in the documented packed encoding.
    utf8.date = (17 << 9) | (3 << 5) | 0x0C;
    utf8.time = (11 << 11) | (13 << 5) | 0x1A;
    let oem = FileSpec::new("oem.bin", filler(16, 62));
    let built = build(&CabinetSpec::new(vec![FolderSpec::new(
        Method::Stored,
        vec![utf8, oem],
    )]));

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let first = &cabinet.files()[0];
    assert!(first.name_is_utf8());
    assert_eq!(first.name_utf8(), Some("uni-\u{00e9}.bin"));
    assert_eq!(
        first.date_time(),
        DateTime {
            year: 1997,
            month: 3,
            day: 12,
            hour: 11,
            minute: 13,
            second: 52,
        }
    );
    let second = &cabinet.files()[1];
    assert!(!second.name_is_utf8());
    assert_eq!(second.name_utf8(), None);
    assert_eq!(second.name_bytes(), b"oem.bin");
}

#[test]
fn seeks_to_a_single_file_inside_a_folder() {
    let files = vec![
        FileSpec::new("head.bin", filler(5_000, 71)),
        FileSpec::new("middle.bin", filler(6_000, 72)),
        FileSpec::new("tail.bin", filler(7_000, 73)),
    ];
    let mut folder = FolderSpec::new(Method::MsZip, files.clone());
    folder.block_size = 2_048;
    let built = build(&CabinetSpec::new(vec![folder]));

    let source = SliceSource::new(&built.bytes);
    let cabinet = Cabinet::parse(&source, 0, &Limits::default()).expect("parse");
    let stream =
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).expect("folder");
    let mut middle =
        FileStream::seek_to(stream, &cabinet.files()[1], &NeverCancelled).expect("seek");
    assert_eq!(middle.remaining(), 6_000);
    let mut out = Vec::new();
    let mut buffer = [0u8; 512];
    loop {
        let read = middle.read(&mut buffer, &NeverCancelled).expect("read");
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..read]);
    }
    assert_eq!(out, files[1].data);
    assert_eq!(middle.remaining(), 0);
}
