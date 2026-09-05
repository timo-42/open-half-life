//! `cargo fuzz` target for the compressed-block decoders: the fuzz input is
//! used verbatim as one `CFDATA` payload in a synthetic cabinet, once as
//! MSZIP and once as LZX. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_mscab::test_support::{BlockSpec, CabinetSpec, FileSpec, FolderSpec, Method, build};
use ohl_mscab::{Cabinet, FolderStream, Limits, NeverCancelled, SliceSource};

/// `typeCompress` for LZX with a 32 KiB window.
const LZX_32K: u16 = 3 | (15 << 8);

fn run(method: Method, payload: &[u8], uncompressed_len: u16) {
    let mut folder = FolderSpec::new(
        method,
        vec![FileSpec::new(
            "fuzz.bin",
            vec![0u8; usize::from(uncompressed_len)],
        )],
    );
    folder.blocks = Some(vec![BlockSpec {
        compressed: payload.to_vec(),
        uncompressed_len,
    }]);
    let built = build(&CabinetSpec::new(vec![folder]));
    let source = SliceSource::new(&built.bytes);
    let Ok(cabinet) = Cabinet::parse(&source, 0, &Limits::default()) else {
        return;
    };
    let Ok(mut stream) = FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default())
    else {
        return;
    };
    let mut buffer = [0u8; 4_096];
    let mut guard = 0u32;
    while let Ok(read) = stream.read(&mut buffer, &NeverCancelled) {
        guard += 1;
        if read == 0 || guard > 64 {
            break;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 || data.len() > 32_768 + 6_144 {
        return;
    }
    // The first two bytes choose the claimed uncompressed length, so the
    // decoders see both agreeing and disagreeing block sizes.
    let claimed = u16::from_le_bytes([data[0], data[1]]) % (32_768 + 1);
    let payload = &data[2..];
    run(Method::MsZip, payload, claimed);
    run(Method::Raw(LZX_32K), payload, claimed);
});
