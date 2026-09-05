//! The plain value type a preflight crate hands to [`crate::ValidatedMedia`].
//!
//! The ISO 9660 and UDF preflight crates own structure parsing; this crate
//! owns only the *shape* of what they report, so that the proof and the cache
//! manifest depend on a small, sanitized, `Copy` value rather than on either
//! parser. A description contains exactly three things:
//!
//! - a project-owned [`MediaClass`] discriminant;
//! - a `&'static str` filesystem name, which is a project constant and can
//!   therefore never be media-derived;
//! - a [`BoundedAsciiLabel`] volume label, which *is* media-derived and is
//!   consequently length-bounded and restricted to printable ASCII at
//!   construction.

use core::fmt;

use crate::error::MediaError;

/// The container class a preflight crate recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MediaClass {
    /// An ECMA-167/UDF image.
    Udf,
    /// An ISO 9660 image.
    Iso9660,
}

impl MediaClass {
    /// The fixed, project-owned name used in manifests and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Udf => "udf",
            Self::Iso9660 => "iso9660",
        }
    }
}

impl fmt::Display for MediaClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A length-bounded, printable-ASCII label.
///
/// This is the only shape in which media-derived text is allowed to cross
/// into the proof, the manifest, or a log line. Construction rejects anything
/// that is not `0x20..=0x7e` and anything longer than `N` bytes, so a label
/// can never carry a control character that would forge a log line, a NUL
/// that would truncate a native call, or an unbounded allocation.
#[derive(Clone, Copy)]
pub struct BoundedAsciiLabel<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> BoundedAsciiLabel<N> {
    /// The maximum number of characters the label can hold.
    pub const CAPACITY: usize = N;

    /// An empty label.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bytes: [0u8; N],
            length: 0,
        }
    }

    /// Creates a label from `text`.
    ///
    /// # Errors
    ///
    /// [`MediaError::InvalidLabel`] when `text` is longer than `N` bytes or
    /// contains a byte outside printable ASCII.
    pub fn new(text: &str) -> Result<Self, MediaError> {
        let source = text.as_bytes();
        if source.len() > N {
            return Err(MediaError::InvalidLabel);
        }
        let mut bytes = [0u8; N];
        for (slot, byte) in bytes.iter_mut().zip(source) {
            if !byte.is_ascii_graphic() && *byte != b' ' {
                return Err(MediaError::InvalidLabel);
            }
            *slot = *byte;
        }
        Ok(Self {
            bytes,
            length: source.len(),
        })
    }

    /// Creates a label from `text`, dropping every byte that is not
    /// acceptable and truncating to `N` characters.
    ///
    /// This is the lossy entry point for media-derived text: a preflight
    /// crate that reads a volume identifier can never guarantee it is
    /// printable, and refusing to cache an otherwise valid image because its
    /// label has an odd byte would be worse than recording a sanitized one.
    #[must_use]
    pub fn sanitized(text: &str) -> Self {
        let mut bytes = [0u8; N];
        let mut length = 0;
        for byte in text.bytes() {
            if length == N {
                break;
            }
            if byte.is_ascii_graphic() || byte == b' ' {
                bytes[length] = byte;
                length += 1;
            }
        }
        Self { bytes, length }
    }

    /// The label's characters.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Every stored byte was checked to be printable ASCII, which is
        // valid UTF-8, so this conversion cannot fail.
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }

    /// The label's length in characters.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.length
    }

    /// Whether the label has no characters.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

impl<const N: usize> Default for BoundedAsciiLabel<N> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<const N: usize> PartialEq for BoundedAsciiLabel<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<const N: usize> Eq for BoundedAsciiLabel<N> {}

impl<const N: usize> fmt::Display for BoundedAsciiLabel<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<const N: usize> fmt::Debug for BoundedAsciiLabel<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Quoted, so a label is never mistaken for surrounding diagnostic
        // text; the content is already bounded printable ASCII.
        write!(formatter, "{:?}", self.as_str())
    }
}

impl<const N: usize> serde::Serialize for BoundedAsciiLabel<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de, const N: usize> serde::Deserialize<'de> for BoundedAsciiLabel<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(&text).map_err(|_| serde::de::Error::custom("expected bounded printable ASCII"))
    }
}

/// The maximum number of characters a volume label keeps.
pub const LABEL_CAPACITY: usize = 32;

/// A volume label as it is carried through this crate.
pub type VolumeLabel = BoundedAsciiLabel<LABEL_CAPACITY>;

/// What a preflight crate recognised about one media source.
///
/// This is a plain value type on purpose: the application maps its own
/// preflight result onto it, and nothing in this crate parses media
/// structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaDescription {
    /// The recognised container class.
    pub class: MediaClass,
    /// The project-owned filesystem name. Always a compile-time constant, so
    /// it can never be media-derived.
    pub filesystem: &'static str,
    /// The sanitized volume label.
    pub label: VolumeLabel,
}

impl MediaDescription {
    /// Creates a description.
    #[must_use]
    pub const fn new(class: MediaClass, filesystem: &'static str, label: VolumeLabel) -> Self {
        Self {
            class,
            filesystem,
            label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedAsciiLabel, MediaClass, VolumeLabel};
    use crate::error::MediaError;

    #[test]
    fn class_names_are_fixed() {
        assert_eq!(MediaClass::Udf.as_str(), "udf");
        assert_eq!(MediaClass::Iso9660.as_str(), "iso9660");
        assert_eq!(MediaClass::Udf.to_string(), "udf");
    }

    #[test]
    fn a_label_accepts_bounded_printable_ascii() {
        let label = VolumeLabel::new("CACHE\"TEST 929").expect("printable");
        assert_eq!(label.as_str(), "CACHE\"TEST 929");
        assert_eq!(label.len(), 14);
        assert!(!label.is_empty());
        assert!(VolumeLabel::empty().is_empty());
    }

    #[test]
    fn a_label_rejects_control_bytes_and_overlong_text() {
        for rejected in ["line\nbreak", "nul\0byte", "tab\there", "non-ascii-\u{e9}"] {
            assert_eq!(
                VolumeLabel::new(rejected).expect_err("rejected"),
                MediaError::InvalidLabel
            );
        }
        assert_eq!(
            VolumeLabel::new(&"A".repeat(33)).expect_err("too long"),
            MediaError::InvalidLabel
        );
        assert_eq!(
            VolumeLabel::new(&"A".repeat(32)).expect("exact fit").len(),
            32
        );
    }

    #[test]
    fn sanitizing_drops_unacceptable_bytes_and_truncates() {
        assert_eq!(
            VolumeLabel::sanitized("HALF\nLIFE\u{e9}").as_str(),
            "HALFLIFE"
        );
        assert_eq!(BoundedAsciiLabel::<4>::sanitized("ABCDEF").as_str(), "ABCD");
        assert_eq!(VolumeLabel::sanitized("").as_str(), "");
    }

    #[test]
    fn labels_compare_by_content_only() {
        let short = VolumeLabel::new("AB").expect("printable");
        let other = VolumeLabel::sanitized("AB");
        assert_eq!(short, other);
        assert_ne!(short, VolumeLabel::new("ABC").expect("printable"));
        assert_eq!(format!("{short:?}"), "\"AB\"");
    }
}
