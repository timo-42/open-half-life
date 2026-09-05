//! The rotate/XOR keystream applied to obfuscated file bytes.

/// The XOR constant.
const XOR_KEY: u8 = 0xd5;
/// The keystream modulus.
const SEED_MODULUS: u32 = 0x47;

/// Removes the obfuscation from `buffer`, advancing `seed` by one per byte.
///
/// The seed is a running per-file counter: consecutive calls must pass the
/// same `seed` so the keystream continues across volume boundaries.
pub fn deobfuscate(buffer: &mut [u8], seed: &mut u32) {
    let mut current = *seed;
    for byte in buffer.iter_mut() {
        let key = u8::try_from(current % SEED_MODULUS).unwrap_or(0);
        *byte = (*byte ^ XOR_KEY).rotate_right(2).wrapping_sub(key);
        current = current.wrapping_add(1);
    }
    *seed = current;
}

/// Applies the obfuscation, the exact inverse of [`deobfuscate`].
///
/// Provided so synthetic test cabinets can be written without a second,
/// separately maintained implementation of the keystream.
pub fn obfuscate(buffer: &mut [u8], seed: &mut u32) {
    let mut current = *seed;
    for byte in buffer.iter_mut() {
        let key = u8::try_from(current % SEED_MODULUS).unwrap_or(0);
        *byte = byte.wrapping_add(key).rotate_left(2) ^ XOR_KEY;
        current = current.wrapping_add(1);
    }
    *seed = current;
}

#[cfg(test)]
mod tests {
    use super::{deobfuscate, obfuscate};
    use alloc::vec::Vec;

    #[test]
    fn obfuscation_round_trips() {
        let plain: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let mut buffer = plain.clone();
        let mut seed = 0;
        obfuscate(&mut buffer, &mut seed);
        assert_eq!(seed, 1000);
        assert_ne!(buffer, plain);
        let mut seed = 0;
        deobfuscate(&mut buffer, &mut seed);
        assert_eq!(buffer, plain);
    }

    #[test]
    fn the_keystream_continues_across_calls() {
        let plain: Vec<u8> = (0..200u8).collect();
        let mut whole = plain.clone();
        let mut seed = 0;
        obfuscate(&mut whole, &mut seed);

        let mut split = plain.clone();
        let mut seed = 0;
        let (head, tail) = split.split_at_mut(37);
        obfuscate(head, &mut seed);
        obfuscate(tail, &mut seed);
        assert_eq!(split, whole);
    }
}
