//! Field-by-field rejection tests: every malformed cabinet must produce a
//! fixed error instead of a panic, a hang, or silent bad data.

use ohl_mscab::test_support::{
    BlockSpec, CabinetSpec, FileSpec, FolderSpec, Method, build, filler, mszip_block,
};
use ohl_mscab::{
    CabError, Cabinet, Compression, FolderSegment, FolderStream, Limits, NeverCancelled,
    SliceSource,
};

fn sample() -> ohl_mscab::test_support::BuiltCabinet {
    let mut folder = FolderSpec::new(
        Method::MsZip,
        vec![
            FileSpec::new("first.bin", filler(3_000, 5)),
            FileSpec::new("second.bin", filler(2_000, 6)),
        ],
    );
    folder.block_size = 1_024;
    build(&CabinetSpec::new(vec![folder]))
}

fn parse(bytes: &[u8]) -> Result<Cabinet, CabError> {
    Cabinet::parse(&SliceSource::new(bytes), 0, &Limits::default())
}

fn parse_with(bytes: &[u8], limits: &Limits) -> Result<Cabinet, CabError> {
    Cabinet::parse(&SliceSource::new(bytes), 0, limits)
}

/// Decodes the first folder end to end, returning the first error.
fn extract(bytes: &[u8]) -> Result<usize, CabError> {
    let source = SliceSource::new(bytes);
    let cabinet = parse(bytes)?;
    let mut stream = FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default())?;
    let mut buffer = [0u8; 4_096];
    let mut total = 0usize;
    loop {
        let read = stream.read(&mut buffer, &NeverCancelled)?;
        if read == 0 {
            return Ok(total);
        }
        total += read;
    }
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn rejects_a_bad_signature() {
    let mut built = sample();
    built.bytes[3] = b'X';
    assert_eq!(parse(&built.bytes), Err(CabError::BadSignature));
}

#[test]
fn rejects_a_source_shorter_than_the_fixed_header() {
    let built = sample();
    assert_eq!(parse(&built.bytes[..20]), Err(CabError::Truncated));
    assert_eq!(parse(&[]), Err(CabError::Truncated));
}

#[test]
fn rejects_a_cabinet_size_beyond_the_source() {
    let mut built = sample();
    let size = u32::try_from(built.bytes.len()).unwrap();
    put_u32(&mut built.bytes, 8, size + 1);
    assert_eq!(parse(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_a_cabinet_size_below_the_header() {
    let mut built = sample();
    put_u32(&mut built.bytes, 8, 12);
    assert_eq!(parse(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_a_cabinet_size_above_the_limit() {
    let built = sample();
    let limits = Limits {
        max_cabinet_bytes: 16,
        ..Limits::default()
    };
    assert_eq!(
        parse_with(&built.bytes, &limits),
        Err(CabError::LimitExceeded)
    );
}

#[test]
fn rejects_an_unsupported_major_version() {
    let mut built = sample();
    built.bytes[0x19] = 2;
    assert_eq!(parse(&built.bytes), Err(CabError::UnsupportedVersion));
}

#[test]
fn rejects_undocumented_header_flags() {
    let mut built = sample();
    put_u16(&mut built.bytes, 0x1E, 0x0008);
    assert_eq!(parse(&built.bytes), Err(CabError::InvalidField));
}

#[test]
fn rejects_a_cabinet_with_no_folders() {
    let mut built = sample();
    put_u16(&mut built.bytes, 0x1A, 0);
    assert_eq!(parse(&built.bytes), Err(CabError::InvalidField));
}

#[test]
fn rejects_coff_files_overlapping_the_folder_table() {
    let mut built = sample();
    // `coffFiles` must lie at or past the end of the folder table.
    put_u32(&mut built.bytes, 0x10, 0x24);
    assert_eq!(parse(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_coff_files_past_the_end_of_the_cabinet() {
    let mut built = sample();
    let size = u32::try_from(built.bytes.len()).unwrap();
    put_u32(&mut built.bytes, 0x10, size);
    assert_eq!(parse(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_a_folder_data_offset_outside_the_cabinet() {
    let mut built = sample();
    let size = u32::try_from(built.bytes.len()).unwrap();
    let folder = built.folder_offsets[0];
    put_u32(&mut built.bytes, folder, size);
    assert_eq!(parse(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_a_file_folder_index_out_of_range() {
    let mut built = sample();
    put_u16(&mut built.bytes, built.file_offsets[0] + 8, 9);
    assert_eq!(parse(&built.bytes), Err(CabError::FolderIndexOutOfRange));
}

#[test]
fn rejects_an_unterminated_or_over_long_file_name() {
    let built = sample();
    let limits = Limits {
        max_name_bytes: 3,
        ..Limits::default()
    };
    assert_eq!(
        parse_with(&built.bytes, &limits),
        Err(CabError::LimitExceeded)
    );

    // A name with no terminator anywhere before the end of the cabinet.
    let mut built = sample();
    let name = built.file_offsets[0] + 16;
    for byte in &mut built.bytes[name..] {
        *byte = b'A';
    }
    assert_eq!(parse(&built.bytes), Err(CabError::LimitExceeded));
}

#[test]
fn rejects_an_empty_file_name() {
    let mut built = sample();
    built.bytes[built.file_offsets[0] + 16] = 0;
    assert_eq!(parse(&built.bytes), Err(CabError::InvalidField));
}

#[test]
fn rejects_counts_above_the_limits() {
    let built = sample();
    assert_eq!(
        parse_with(
            &built.bytes,
            &Limits {
                max_folders: 0,
                ..Limits::default()
            }
        ),
        Err(CabError::LimitExceeded)
    );
    assert_eq!(
        parse_with(
            &built.bytes,
            &Limits {
                max_files: 1,
                ..Limits::default()
            }
        ),
        Err(CabError::LimitExceeded)
    );
    assert_eq!(
        parse_with(
            &built.bytes,
            &Limits {
                max_blocks_per_folder: 1,
                ..Limits::default()
            }
        ),
        Err(CabError::LimitExceeded)
    );
    assert_eq!(
        parse_with(
            &built.bytes,
            &Limits {
                max_file_bytes: 10,
                ..Limits::default()
            }
        ),
        Err(CabError::LimitExceeded)
    );
}

#[test]
fn rejects_a_header_reserve_above_the_limit() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::Stored,
        vec![FileSpec::new("r.bin", filler(10, 1))],
    )]);
    spec.header_reserve = 100;
    let built = build(&spec);
    assert!(parse(&built.bytes).is_ok());
    let limits = Limits {
        max_header_reserve_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        parse_with(&built.bytes, &limits),
        Err(CabError::LimitExceeded)
    );
}

#[test]
fn rejects_a_data_block_checksum_mismatch() {
    let mut built = sample();
    let block = built.data_offsets[0][1];
    // Flip a payload byte, leaving the stored checksum intact.
    built.bytes[block + 8] ^= 0xFF;
    assert_eq!(extract(&built.bytes), Err(CabError::ChecksumMismatch));
}

#[test]
fn rejects_an_uncompressed_size_above_the_documented_maximum() {
    let mut built = sample();
    let block = built.data_offsets[0][0];
    put_u16(&mut built.bytes, block + 6, 32_769);
    assert_eq!(extract(&built.bytes), Err(CabError::LimitExceeded));
}

#[test]
fn rejects_a_compressed_size_above_the_documented_maximum() {
    let mut built = sample();
    let block = built.data_offsets[0][0];
    put_u16(&mut built.bytes, block + 4, 32_768 + 6_145);
    assert_eq!(extract(&built.bytes), Err(CabError::LimitExceeded));
}

#[test]
fn rejects_a_truncated_data_block() {
    let mut built = sample();
    built.bytes.truncate(built.bytes.len() - 32);
    let size = u32::try_from(built.bytes.len()).unwrap();
    put_u32(&mut built.bytes, 8, size);
    assert_eq!(extract(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_corrupt_deflate_data() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::MsZip,
        vec![FileSpec::new("broken.bin", filler(4_000, 8))],
    )]);
    // Without checksums the corruption has to be caught by the decoder.
    spec.write_checksums = false;
    let mut built = build(&spec);
    let block = built.data_offsets[0][0];
    for byte in &mut built.bytes[block + 12..block + 40] {
        *byte ^= 0xA5;
    }
    assert_eq!(extract(&built.bytes), Err(CabError::DecompressionFailed));
}

#[test]
fn rejects_an_mszip_block_without_the_ck_signature() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::MsZip,
        vec![FileSpec::new("nosig.bin", filler(100, 9))],
    )]);
    spec.write_checksums = false;
    let mut built = build(&spec);
    let block = built.data_offsets[0][0];
    built.bytes[block + 8] = b'Z';
    assert_eq!(extract(&built.bytes), Err(CabError::DecompressionFailed));
}

#[test]
fn rejects_a_stored_block_whose_sizes_disagree() {
    let mut spec = CabinetSpec::new(vec![FolderSpec::new(
        Method::Stored,
        vec![FileSpec::new("bad.bin", filler(100, 10))],
    )]);
    spec.write_checksums = false;
    let mut built = build(&spec);
    let block = built.data_offsets[0][0];
    put_u16(&mut built.bytes, block + 6, 99);
    assert_eq!(extract(&built.bytes), Err(CabError::InvalidField));
}

#[test]
fn reports_quantum_and_unknown_methods_as_unsupported() {
    for raw in [2u16, 0x0007, 0x000F] {
        let built = build(&CabinetSpec::new(vec![FolderSpec::new(
            Method::Raw(raw),
            vec![FileSpec::new("q.bin", filler(64, 11))],
        )]));
        let source = SliceSource::new(&built.bytes);
        let cabinet = parse(&built.bytes).expect("headers still parse");
        assert!(matches!(
            cabinet.folders()[0].compression,
            Compression::Quantum { .. } | Compression::Unknown { .. }
        ));
        assert_eq!(
            FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).err(),
            Some(CabError::Unsupported)
        );
    }
}

#[test]
fn reports_an_out_of_range_lzx_window_as_unsupported() {
    // typeCompress = LZX with a 14-bit window, below the documented minimum.
    let built = build(&CabinetSpec::new(vec![FolderSpec::new(
        Method::Raw(0x0E03),
        vec![FileSpec::new("lzx.bin", filler(64, 12))],
    )]));
    let source = SliceSource::new(&built.bytes);
    let cabinet = parse(&built.bytes).expect("headers still parse");
    assert_eq!(
        cabinet.folders()[0].compression,
        Compression::Lzx { window_bits: 14 }
    );
    assert_eq!(
        FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default()).err(),
        Some(CabError::Unsupported)
    );
}

#[test]
fn rejects_a_block_that_claims_more_data_than_the_cabinet_holds() {
    let mut built = sample();
    let block = built.data_offsets[0][0];
    put_u16(&mut built.bytes, block + 4, 30_000);
    assert_eq!(extract(&built.bytes), Err(CabError::OutOfBounds));
}

#[test]
fn rejects_a_folder_index_the_stream_api_does_not_have() {
    let built = sample();
    let source = SliceSource::new(&built.bytes);
    let cabinet = parse(&built.bytes).expect("parse");
    assert_eq!(
        FolderStream::from_cabinet(&cabinet, &source, 0, 5, Limits::default()).err(),
        Some(CabError::FolderIndexOutOfRange)
    );
    assert_eq!(
        FolderSegment::new(&cabinet, 0, 5).err(),
        Some(CabError::FolderIndexOutOfRange)
    );
}

#[test]
fn rejects_a_volume_the_caller_did_not_supply() {
    let built = sample();
    let source = SliceSource::new(&built.bytes);
    assert_eq!(
        Cabinet::parse(&source, 1, &Limits::default()).err(),
        Some(CabError::Unsupported)
    );
}

#[test]
fn rejects_an_mszip_block_that_decodes_to_the_wrong_length() {
    let mut folder = FolderSpec::new(
        Method::MsZip,
        vec![FileSpec::new("short.bin", filler(64, 13))],
    );
    folder.blocks = Some(vec![BlockSpec {
        compressed: mszip_block(&filler(64, 13)),
        uncompressed_len: 63,
    }]);
    let built = build(&CabinetSpec::new(vec![folder]));
    assert_eq!(extract(&built.bytes), Err(CabError::DecompressionFailed));
}
