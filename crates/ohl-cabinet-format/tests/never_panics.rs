//! Property test: no arbitrary byte string can make the header parser panic.

use ohl_cabinet_format::testing::{HeaderBuilder, SynthFile};
use ohl_cabinet_format::{CAB_SIGNATURE, CabinetHeader, Limits};
use proptest::prelude::*;

fn exercise(bytes: &[u8]) {
    let limits = Limits {
        max_header_bytes: 1 << 20,
        ..Limits::default()
    };
    let Ok(header) = CabinetHeader::parse(bytes, &limits) else {
        return;
    };
    for name in header.directories() {
        let _ = name;
    }
    for descriptor in header.file_descriptors() {
        let _ = descriptor;
    }
    for index in 0..header.file_count().min(1024) {
        let _ = header.file_name(index);
    }
    let _ = header.file_groups().len();
    let _ = header.components().len();
    let _ = header.file_table().len();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise(&bytes);
    }

    #[test]
    fn arbitrary_bytes_behind_a_valid_signature_never_panic(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096)
    ) {
        let mut framed = CAB_SIGNATURE.to_le_bytes().to_vec();
        framed.extend_from_slice(&bytes);
        exercise(&framed);
    }

    #[test]
    fn corrupting_a_real_header_never_panics(
        index in 0usize..4096,
        value in any::<u8>(),
        version in 0u8..3,
    ) {
        let builder = match version {
            0 => HeaderBuilder::v5(),
            1 => HeaderBuilder::v6(),
            _ => HeaderBuilder::is2003(),
        };
        let mut bytes = builder
            .directory("d")
            .file(SynthFile::new("f", 4, 0x40))
            .group("g", 0, 0)
            .component("c", &["g"])
            .build();
        if index < bytes.len() {
            bytes[index] = value;
        }
        exercise(&bytes);
    }
}
