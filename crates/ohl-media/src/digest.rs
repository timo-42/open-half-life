//! Full-content SHA-256 fingerprinting of a pinned media source.
//!
//! Fingerprinting is a *read of the user's media*, so it obeys the same rules
//! as every other read in the port: the reads are positional and bounded by
//! the pinned acquisition size, the pathname is never consulted again, and
//! the pinned object is reauthenticated with
//! [`MediaSource::verify_unchanged`] before the first byte, periodically
//! while hashing, and after the last byte. A mutation that lands in the
//! middle of a long hash is therefore reported as
//! [`MediaError::SourceChanged`] rather than silently producing a digest of a
//! mixture of two contents.
//!
//! The digest itself is the only value derived from media that this crate
//! persists or logs. That is deliberate and allowed by `docs/MEDIA_IMPORT.md`
//! ("a digest of the complete source"): it is a fixed-width value that
//! reconstructs nothing.

use core::fmt;

use ohl_core::StreamingSha256;
use ohl_platform::{MediaSource, SourceFingerprint};

use crate::error::MediaError;

/// The read window used while hashing, in bytes (64 KiB, as in the C++ tree).
pub const FINGERPRINT_CHUNK_BYTES: u64 = 64 * 1024;

/// How much content is hashed between two intermediate stability checks.
///
/// The check at the start and the check at the end are mandatory; these
/// periodic checks bound how much work a long hash can waste on content that
/// has already been invalidated, without turning the hash into one native
/// `stat` per 64 KiB.
pub const STABILITY_CHECK_INTERVAL_BYTES: u64 = 64 * 1024 * 1024;

/// The SHA-256 of one media source's complete content.
///
/// The value is deliberately opaque: it is constructed either by
/// [`fingerprint`] or from raw bytes, and it renders as 64 lowercase hex
/// characters in both [`fmt::Display`] and [`fmt::Debug`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaDigest([u8; 32]);

impl MediaDigest {
    /// The digest's hexadecimal length in characters.
    pub const HEX_LENGTH: usize = 64;

    /// Wraps 32 raw digest bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The digest as 64 lowercase hexadecimal characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(Self::HEX_LENGTH);
        for byte in self.0 {
            hex.push(hex_nibble(byte >> 4));
            hex.push(hex_nibble(byte & 0x0f));
        }
        hex
    }

    /// Parses exactly 64 lowercase hexadecimal characters.
    ///
    /// Uppercase input is rejected so that a digest has exactly one on-disk
    /// spelling and therefore exactly one cache directory name.
    #[must_use]
    pub fn parse_hex(hex: &str) -> Option<Self> {
        if hex.len() != Self::HEX_LENGTH {
            return None;
        }
        let bytes = hex.as_bytes();
        let mut digest = [0u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            let high = decode_nibble(bytes[index * 2])?;
            let low = decode_nibble(bytes[index * 2 + 1])?;
            *byte = (high << 4) | low;
        }
        Some(Self(digest))
    }
}

impl fmt::Display for MediaDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for MediaDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MediaDigest({self})")
    }
}

impl serde::Serialize for MediaDigest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for MediaDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse_hex(&hex).ok_or_else(|| {
            serde::de::Error::custom("expected 64 lowercase hexadecimal digest characters")
        })
    }
}

/// The lowercase hex character for a nibble.
const fn hex_nibble(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + (nibble - 10)) as char,
    }
}

/// Decodes one lowercase hex character.
const fn decode_nibble(character: u8) -> Option<u8> {
    match character {
        b'0'..=b'9' => Some(character - b'0'),
        b'a'..=b'f' => Some(character - b'a' + 10),
        _ => None,
    }
}

/// Hashes the complete content of `source`.
///
/// # Errors
///
/// [`MediaError::SourceChanged`] when the pinned object changed at any
/// stability boundary or was truncated mid-hash, and
/// [`MediaError::SourceReadFailed`] for any other native read failure. No
/// error carries a path, an offset, or a media-derived byte.
pub fn fingerprint(source: &MediaSource) -> Result<MediaDigest, MediaError> {
    fingerprint_with_progress(source, &mut |_| {})
}

/// [`fingerprint`] with a progress observer.
///
/// `progress` is called with the number of bytes hashed so far after every
/// completed chunk, including the final one. It exists so a caller can report
/// long hashes, and so tests can mutate the pinned object at a deterministic
/// point instead of racing it.
///
/// # Errors
///
/// See [`fingerprint`].
pub fn fingerprint_with_progress(
    source: &MediaSource,
    progress: &mut dyn FnMut(u64),
) -> Result<MediaDigest, MediaError> {
    let size_bytes = source.size();
    source.verify_unchanged()?;

    let capacity = usize::try_from(FINGERPRINT_CHUNK_BYTES.min(size_bytes.max(1)))
        .map_err(|_| MediaError::SourceReadFailed)?;
    let mut buffer = vec![0u8; capacity];
    let mut digest = StreamingSha256::new();
    let mut offset: u64 = 0;
    let mut since_check: u64 = 0;

    while offset < size_bytes {
        let count = (size_bytes - offset).min(FINGERPRINT_CHUNK_BYTES);
        let chunk =
            &mut buffer[..usize::try_from(count).map_err(|_| MediaError::SourceReadFailed)?];
        if let Err(read_error) = source.read_exact_at(offset, chunk) {
            // A failed read is ambiguous on its own: ask the pinned object
            // first, so a truncation is reported as a change and not as an
            // opaque read failure.
            source.verify_unchanged()?;
            return Err(read_error.into());
        }
        digest.update(chunk);
        offset += count;
        since_check += count;
        progress(offset);

        if offset < size_bytes && since_check >= STABILITY_CHECK_INTERVAL_BYTES {
            source.verify_unchanged()?;
            since_check = 0;
        }
    }

    source.verify_unchanged()?;
    Ok(MediaDigest::from_bytes(digest.finalize()))
}

/// The `ohl-platform` fingerprint value for a size and digest pair.
#[must_use]
pub const fn source_fingerprint(size_bytes: u64, digest: &MediaDigest) -> SourceFingerprint {
    SourceFingerprint {
        size_bytes,
        sha256: *digest.as_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::MediaDigest;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn hex_round_trips() {
        let digest = MediaDigest::parse_hex(EMPTY_SHA256).expect("valid vector");
        assert_eq!(digest.to_hex(), EMPTY_SHA256);
        assert_eq!(digest.to_string(), EMPTY_SHA256);
        assert_eq!(
            format!("{digest:?}"),
            format!("MediaDigest({EMPTY_SHA256})")
        );
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert!(MediaDigest::parse_hex("").is_none());
        assert!(MediaDigest::parse_hex(&EMPTY_SHA256[..63]).is_none());
        assert!(MediaDigest::parse_hex(&format!("{EMPTY_SHA256}0")).is_none());
        assert!(
            MediaDigest::parse_hex(&EMPTY_SHA256.to_uppercase()).is_none(),
            "one digest must have exactly one directory name"
        );
        assert!(MediaDigest::parse_hex(&"z".repeat(64)).is_none());
    }

    #[test]
    fn all_byte_values_survive_a_hex_round_trip() {
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::try_from(index * 8).unwrap_or(0xff);
        }
        let digest = MediaDigest::from_bytes(bytes);
        assert_eq!(
            MediaDigest::parse_hex(&digest.to_hex()).expect("round trip"),
            digest
        );
        assert_eq!(digest.as_bytes(), &bytes);
    }
}
