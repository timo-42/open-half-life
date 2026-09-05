//! Sanitized classification vocabulary produced by the bounded preflights.

use alloc::string::String;
use core::fmt;

/// The read-only media classes the project can mount.
///
/// The value is decided by a bounded structural preflight, never guessed from
/// a pathname or a file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaClass {
    /// An ECMA-119 (ISO 9660) volume, optionally carrying Joliet names.
    Iso9660,
    /// An ECMA-167 volume that passed the bounded NSR02 preflight.
    Udf,
}

impl MediaClass {
    /// A fixed, media-independent identifier for logs and manifests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iso9660 => "iso9660",
            Self::Udf => "udf",
        }
    }
}

impl fmt::Display for MediaClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The complete set of filesystem descriptions a preflight may report.
///
/// The set is closed on purpose: a description is a project-authored constant,
/// so no media-derived byte can ever reach a log through this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilesystemDescription {
    /// ECMA-119 primary volume descriptor set with no Joliet escape sequence.
    Iso9660,
    /// ECMA-119 with a supplementary descriptor carrying a Joliet escape.
    Iso9660Joliet,
    /// ECMA-167 volume recognition sequence with an NSR02 structure.
    Ecma167Nsr02,
}

impl FilesystemDescription {
    /// The fixed description string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iso9660 => "ECMA-119 ISO 9660",
            Self::Iso9660Joliet => "ECMA-119 ISO 9660 + Joliet",
            Self::Ecma167Nsr02 => "ECMA-167 NSR02 candidate",
        }
    }

    /// The media class this description belongs to.
    pub const fn media_class(self) -> MediaClass {
        match self {
            Self::Iso9660 | Self::Iso9660Joliet => MediaClass::Iso9660,
            Self::Ecma167Nsr02 => MediaClass::Udf,
        }
    }
}

impl fmt::Display for FilesystemDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The largest sanitized volume label the project keeps.
///
/// ECMA-119 section 8.4 records a 32-byte volume identifier and ECMA-167
/// part 3 records a 32-byte `dstring`, so 32 bytes is also the natural cap.
pub const MAX_VOLUME_LABEL_BYTES: usize = 32;

/// A volume label reduced to printable ASCII and capped at
/// [`MAX_VOLUME_LABEL_BYTES`].
///
/// Every byte outside `0x20..=0x7e` is replaced by `?`, so the label can never
/// smuggle control characters, terminal escape sequences, or non-UTF-8 bytes
/// into a log line. Trailing spaces are removed, matching the padded fixed
/// width both standards use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct VolumeLabel(String);

impl VolumeLabel {
    /// Sanitizes `raw` into a label.
    pub fn sanitize(raw: &[u8]) -> Self {
        let mut label = String::new();
        for byte in raw.iter().copied().take(MAX_VOLUME_LABEL_BYTES) {
            label.push(if (0x20..=0x7e).contains(&byte) {
                char::from(byte)
            } else {
                '?'
            });
        }
        while label.ends_with(' ') {
            label.pop();
        }
        Self(label)
    }

    /// Sanitizes a UCS-2 big-endian label such as a Joliet volume identifier.
    ///
    /// Code units outside printable ASCII become `?`, so the result stays
    /// within the same guarantee as [`Self::sanitize`].
    pub fn sanitize_ucs2_be(raw: &[u8]) -> Self {
        let mut bytes = [0u8; MAX_VOLUME_LABEL_BYTES];
        let mut length = 0;
        for pair in raw.as_chunks::<2>().0.iter().take(MAX_VOLUME_LABEL_BYTES) {
            bytes[length] = if pair[0] == 0 { pair[1] } else { b'?' };
            length += 1;
        }
        Self::sanitize(&bytes[..length])
    }

    /// The sanitized label.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the label is empty after sanitizing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The sanitized label's length in bytes, which is also its length in
    /// printable ASCII characters.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for VolumeLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The sanitized result of a bounded structural preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPreflight {
    /// The recognized media class.
    pub media_class: MediaClass,
    /// The fixed description of the recognized structure.
    pub filesystem: FilesystemDescription,
    /// The sanitized volume label, which may be empty.
    pub volume_label: VolumeLabel,
}

#[cfg(test)]
mod tests {
    use super::{FilesystemDescription, MediaClass, VolumeLabel};

    #[test]
    fn labels_are_printable_ascii_and_bounded() {
        let label = VolumeLabel::sanitize(b"OHL\x07SYNTHETIC\x1b[31m    ");
        assert_eq!(label.as_str(), "OHL?SYNTHETIC?[31m");
        let long = VolumeLabel::sanitize(&[b'A'; 200]);
        assert_eq!(long.len(), 32);
    }

    #[test]
    fn ucs2_labels_fold_non_ascii_to_a_placeholder() {
        let mut raw = alloc::vec::Vec::new();
        for character in "OHL".chars() {
            raw.push(0);
            raw.push(character as u8);
        }
        raw.extend_from_slice(&[0x30, 0x42]);
        assert_eq!(VolumeLabel::sanitize_ucs2_be(&raw).as_str(), "OHL?");
    }

    #[test]
    fn descriptions_are_a_fixed_set() {
        assert_eq!(
            FilesystemDescription::Iso9660Joliet.as_str(),
            "ECMA-119 ISO 9660 + Joliet"
        );
        assert_eq!(
            FilesystemDescription::Ecma167Nsr02.media_class(),
            MediaClass::Udf
        );
    }
}
