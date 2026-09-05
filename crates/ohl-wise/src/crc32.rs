//! CRC-32 (ITU-T V.42 / RFC 1952 section 8), implemented independently from
//! the published polynomial and table algorithm.
//!
//! Reflected polynomial `0xEDB8_8320`, initial value `0xFFFF_FFFF`, final
//! exclusive-or with `0xFFFF_FFFF`. The 256-entry lookup table is derived at
//! compile time from the polynomial, so no table constant is transcribed.

const POLYNOMIAL: u32 = 0xEDB8_8320;

#[allow(clippy::cast_possible_truncation, reason = "index is bounded by 256")]
const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut value = index as u32; // 0..256 always fits
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ POLYNOMIAL
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static TABLE: [u32; 256] = build_table();

/// An incremental CRC-32 over a byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// A checksum over the empty byte string.
    #[must_use]
    pub const fn new() -> Self {
        Self { state: u32::MAX }
    }

    /// Folds `bytes` into the checksum.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut state = self.state;
        for byte in bytes {
            let index = ((state ^ u32::from(*byte)) & 0xff) as usize;
            state = (state >> 8) ^ TABLE[index];
        }
        self.state = state;
    }

    /// The checksum of everything folded in so far.
    #[must_use]
    pub const fn finish(&self) -> u32 {
        self.state ^ u32::MAX
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// The CRC-32 of `bytes`.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

#[cfg(test)]
mod tests {
    use super::{Crc32, crc32};

    #[test]
    fn matches_the_published_check_values() {
        // RFC 1952 / ITU-T V.42 check value for the nine-byte ASCII string
        // "123456789" is 0xCBF43926, and the empty string is 0.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(&[0u8; 32]), 0x190A_55AD);
    }

    #[test]
    fn incremental_updates_match_a_single_update() {
        let data: [u8; 64] = core::array::from_fn(|index| u8::try_from(index * 7 % 251).unwrap());
        let mut split = Crc32::new();
        split.update(&data[..17]);
        split.update(&data[17..]);
        assert_eq!(split.finish(), crc32(&data));
    }
}
