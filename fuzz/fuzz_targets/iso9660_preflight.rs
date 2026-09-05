//! The ECMA-119 preflight must terminate with a sanitized error on any input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_media_archive::block::SliceBlockReader;

fuzz_target!(|data: &[u8]| {
    let _ = ohl_iso9660::preflight(&mut SliceBlockReader::new(data));
});
