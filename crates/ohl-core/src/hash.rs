//! SHA-256 via the pinned `sha2` crate.
//!
//! [`StreamingSha256`] is a thin, project-owned seam around `sha2::Sha256` so
//! call sites depend on this crate rather than `sha2` directly, matching the
//! architecture's requirement for a single owned digest wrapper that can grow
//! bounded-input policies later without touching call sites.

use sha2::{Digest, Sha256 as Sha2Sha256};

/// Re-export of the underlying `sha2` digest type for callers that need the
/// `Digest` trait surface directly.
pub use sha2::Sha256;

/// A streaming SHA-256 hasher.
#[derive(Clone, Default)]
pub struct StreamingSha256 {
    inner: Sha2Sha256,
}

impl StreamingSha256 {
    /// Creates a new, empty hasher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Sha2Sha256::new(),
        }
    }

    /// Feeds `bytes` into the running digest.
    pub fn update(&mut self, bytes: &[u8]) {
        self.inner.update(bytes);
    }

    /// Consumes the hasher and returns the final 32-byte digest.
    #[must_use]
    pub fn finalize(self) -> [u8; 32] {
        self.inner.finalize().into()
    }

    /// Convenience one-shot digest of a single buffer.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> [u8; 32] {
        let mut hasher = Self::new();
        hasher.update(bytes);
        hasher.finalize()
    }
}

#[cfg(test)]
mod tests {
    use super::StreamingSha256;

    fn decode_hex(hex: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (index, byte) in out.iter_mut().enumerate() {
            let start = index * 2;
            *byte = u8::from_str_radix(&hex[start..start + 2], 16).expect("valid hex vector");
        }
        out
    }

    #[test]
    fn fips_180_4_empty_string() {
        let expected =
            decode_hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(StreamingSha256::digest(b""), expected);
    }

    #[test]
    fn fips_180_4_abc() {
        let expected =
            decode_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(StreamingSha256::digest(b"abc"), expected);
    }

    #[test]
    fn fips_180_4_two_block_message() {
        // "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        let message = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        let expected =
            decode_hex("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        let mut hasher = StreamingSha256::new();
        // Exercise the streaming path with a mid-message split, not just the
        // one-shot convenience function.
        hasher.update(&message[..20]);
        hasher.update(&message[20..]);
        assert_eq!(hasher.finalize(), expected);
    }
}
