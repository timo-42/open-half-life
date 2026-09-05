//! Property tests: arbitrary bytes never panic any entry point.
//!
//! Every input is randomly generated here; nothing is derived from media.

use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};
use ohl_wise::{Chain, Discard, FileMap, Limits, NeverCancelled, SliceSource, find_overlay};
use proptest::prelude::*;

fn tight_limits() -> Limits {
    Limits {
        max_streams: 64,
        max_inflated_bytes_per_stream: 1 << 20,
        max_total_inflated_bytes: 1 << 22,
        max_compressed_bytes_per_stream: 1 << 20,
        max_script_bytes: 1 << 16,
        max_file_records: 512,
        ..Limits::DEFAULT
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_bytes_never_panic_the_package_reader(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let mut source = SliceSource::new(&bytes);
        let _ = ohl_wise::read_package(&mut source, None, tight_limits(), &NeverCancelled);
        let _ = ohl_wise::read_package(&mut source, Some(0), tight_limits(), &NeverCancelled);
        let _ = find_overlay(&mut source, &tight_limits());
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_header_scan(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut source = SliceSource::new(&bytes);
        if let Ok(overlay) = ohl_wise::overlay_at(&mut source, 0) {
            let _ = ohl_wise::locate_first_stream(&mut source, &overlay, &tight_limits(), &NeverCancelled);
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_chain(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let mut source = SliceSource::new(&bytes);
        let mut chain = Chain::new(0, bytes.len() as u64, tight_limits());
        let mut steps = 0;
        while let Some(event) = chain.next_event(&mut source, &NeverCancelled) {
            if event.is_err() {
                break;
            }
            steps += 1;
            prop_assert!(steps <= 4096);
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_script_parser(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let mut source = SliceSource::new(&bytes);
        if let Ok(overlay) = ohl_wise::overlay_at(&mut source, 0)
            && let Ok(table) = ohl_wise::script::parse(&bytes, &overlay, &tight_limits())
        {
            {
                let map = FileMap::build(&table, &[], &overlay);
                prop_assert_eq!(map.mapped_count(), 0);
                for index in 0..table.len() {
                    prop_assert!(map.open_file(&table, index, tight_limits()).is_err());
                }
            }
        }
    }

    #[test]
    fn corrupting_one_byte_never_panics(
        seed in 0..64u8,
        position in 0..4096usize,
        replacement in any::<u8>(),
    ) {
        let built = build_package(&PackageOptions::with_files(vec![SyntheticFile::new(
            b"one.dat",
            vec![seed; 512],
        )]));
        let mut image = built.image;
        if position < image.len() {
            image[position] = replacement;
        }
        let mut source = SliceSource::new(&image);
        let _ = ohl_wise::read_package(&mut source, None, tight_limits(), &NeverCancelled);
        let mut sink = Discard;
        let _ = ohl_wise::stream::inflate_stream(
            &mut source,
            built.first_stream_offset,
            tight_limits(),
            &mut sink,
            &NeverCancelled,
        );
    }
}
