//! Fuzz target: the PKWARE DCL explode decoder must never panic, never read
//! outside its window, and never exceed its output ceiling.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_isz::{Explode, explode_to_vec};

const MAX_OUTPUT: usize = 1 << 18;

fuzz_target!(|data: &[u8]| {
    if let Ok(out) = explode_to_vec(data, MAX_OUTPUT) {
        assert!(out.len() <= MAX_OUTPUT);
    }

    // The same input again, fed in small increments, must behave the same
    // way: never panic, and never exceed the output ceiling.
    let mut decoder = Explode::new();
    let mut out = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    for (index, byte) in data.iter().enumerate().take(1 << 16) {
        pending.push(*byte);
        let last = index + 1 == data.len();
        let Ok(progress) = decoder.decode(&pending, last, &mut out, 1 << 16) else {
            return;
        };
        pending.drain(..progress.consumed);
        assert!(out.len() <= 1 << 16);
        if progress.finished {
            return;
        }
        out.clear();
    }
});
