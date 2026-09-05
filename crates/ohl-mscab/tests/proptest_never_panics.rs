//! Property tests: no input, however malformed, may panic the parser or the
//! extractor. Only fixed errors or correct output are acceptable outcomes.

use ohl_mscab::test_support::{CabinetSpec, FileSpec, FolderSpec, Method, build, filler};
use ohl_mscab::{Cabinet, FolderStream, Limits, NeverCancelled, SliceSource};
use proptest::prelude::*;

fn drain(bytes: &[u8]) {
    let source = SliceSource::new(bytes);
    let Ok(cabinet) = Cabinet::parse(&source, 0, &Limits::default()) else {
        return;
    };
    for file in cabinet.files() {
        let _ = file.name_utf8();
        let _ = file.date_time();
        let _ = cabinet.folder_of(file);
    }
    for folder_index in 0..cabinet.header().folder_count {
        let Ok(mut stream) =
            FolderStream::from_cabinet(&cabinet, &source, 0, folder_index, Limits::default())
        else {
            continue;
        };
        let mut buffer = [0u8; 512];
        let mut guard = 0u32;
        while let Ok(read) = stream.read(&mut buffer, &NeverCancelled) {
            guard += 1;
            if read == 0 || guard > 4_096 {
                break;
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Arbitrary bytes, including ones that happen to start with `MSCF`.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4_096)) {
        drain(&bytes);
    }

    #[test]
    fn arbitrary_bytes_behind_a_signature_never_panic(
        tail in proptest::collection::vec(any::<u8>(), 0..2_048),
    ) {
        let mut bytes = b"MSCF".to_vec();
        bytes.extend_from_slice(&tail);
        drain(&bytes);
    }

    /// Single-byte corruptions of a valid cabinet.
    #[test]
    fn corrupted_cabinets_never_panic(
        offset in 0usize..2_048,
        value in any::<u8>(),
        blocks in 1usize..4,
    ) {
        let mut folder = FolderSpec::new(
            Method::MsZip,
            vec![
                FileSpec::new("alpha.bin", filler(700 * blocks, 3)),
                FileSpec::new("beta.bin", filler(300, 4)),
            ],
        );
        folder.block_size = 512;
        let mut built = build(&CabinetSpec::new(vec![folder]));
        let index = offset % built.bytes.len();
        built.bytes[index] = value;
        drain(&built.bytes);
    }

    /// Arbitrary limits must not change "never panics", only what is refused.
    #[test]
    fn arbitrary_limits_never_panic(
        max_folders in 0u32..8,
        max_files in 0u32..8,
        max_name_bytes in 0usize..32,
        max_blocks in 0u32..8,
    ) {
        let built = build(&CabinetSpec::new(vec![FolderSpec::new(
            Method::Stored,
            vec![FileSpec::new("only.bin", filler(400, 5))],
        )]));
        let limits = Limits {
            max_folders,
            max_files,
            max_name_bytes,
            max_blocks_per_folder: max_blocks,
            ..Limits::default()
        };
        let source = SliceSource::new(&built.bytes);
        let _ = Cabinet::parse(&source, 0, &limits);
    }
}
