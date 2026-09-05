//! Cabinet version decoding, expressed as an explicit decision table.

/// Major version at or above which header strings are UTF-16LE.
pub const UNICODE_MAJOR_VERSION: u16 = 17;

/// The floor applied to a scaled (InstallShield 2003 and later) version.
pub const SCALED_MAJOR_FLOOR: u16 = 5;

/// How the raw 32-bit version word encoded its major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionEncoding {
    /// Tag `0x01`: the major version is the nibble at bits 12..16
    /// (InstallShield 5 and 6).
    NibbleShifted,
    /// Tag `0x02` or `0x04`: the low 16 bits divided by 100, floored at
    /// [`SCALED_MAJOR_FLOOR`] when non-zero (InstallShield 2003 and later).
    ScaledHundred,
    /// Any other tag: no major version is encoded; the InstallShield 5
    /// structure layout is assumed.
    Untagged,
}

/// One row of the version decision table.
struct Rule {
    tag: u8,
    encoding: VersionEncoding,
}

/// The decision table, keyed on the most significant byte of the version word.
const RULES: &[Rule] = &[
    Rule {
        tag: 0x01,
        encoding: VersionEncoding::NibbleShifted,
    },
    Rule {
        tag: 0x02,
        encoding: VersionEncoding::ScaledHundred,
    },
    Rule {
        tag: 0x04,
        encoding: VersionEncoding::ScaledHundred,
    },
];

/// Which fixed structure layout a header uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    /// InstallShield 5 style file descriptors and tables.
    V5,
    /// InstallShield 6 and later style file descriptors and tables.
    V6,
}

/// A decoded cabinet version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Version {
    raw: u32,
    major: u16,
    encoding: VersionEncoding,
}

impl Version {
    /// Decodes the raw version word from the common header.
    #[must_use]
    pub fn decode(raw: u32) -> Self {
        let tag = u8::try_from(raw >> 24).unwrap_or(0);
        for rule in RULES {
            if rule.tag != tag {
                continue;
            }
            let major = match rule.encoding {
                VersionEncoding::NibbleShifted => {
                    u16::try_from((raw >> 12) & 0xf).unwrap_or_default()
                }
                VersionEncoding::ScaledHundred => {
                    let scaled = u16::try_from(raw & 0xffff).unwrap_or(u16::MAX);
                    if scaled == 0 {
                        0
                    } else {
                        let divided = scaled / 100;
                        if divided < SCALED_MAJOR_FLOOR {
                            SCALED_MAJOR_FLOOR
                        } else {
                            divided
                        }
                    }
                }
                VersionEncoding::Untagged => 0,
            };
            return Self {
                raw,
                major,
                encoding: rule.encoding,
            };
        }

        Self {
            raw,
            major: 0,
            encoding: VersionEncoding::Untagged,
        }
    }

    /// A version with an explicitly forced major version, for callers that
    /// already know which product wrote the media.
    #[must_use]
    pub fn forced(raw: u32, major: u16) -> Self {
        Self {
            raw,
            major,
            encoding: VersionEncoding::Untagged,
        }
    }

    /// The raw 32-bit version word as stored.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.raw
    }

    /// The decoded major version. Zero means "not encoded".
    #[must_use]
    pub const fn major(self) -> u16 {
        self.major
    }

    /// How the major version was encoded.
    #[must_use]
    pub const fn encoding(self) -> VersionEncoding {
        self.encoding
    }

    /// The structure layout to apply. A missing (zero) or exactly-5 major
    /// version selects [`Layout::V5`]; everything else selects
    /// [`Layout::V6`], matching the reference implementation's switch.
    #[must_use]
    pub const fn layout(self) -> Layout {
        if self.major == 0 || self.major == 5 {
            Layout::V5
        } else {
            Layout::V6
        }
    }

    /// Whether header strings are UTF-16LE rather than single-byte.
    #[must_use]
    pub const fn is_unicode(self) -> bool {
        self.major >= UNICODE_MAJOR_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, Version, VersionEncoding};

    #[test]
    fn decodes_the_nibble_shifted_encoding() {
        let version = Version::decode(0x0100_5000);
        assert_eq!(version.encoding(), VersionEncoding::NibbleShifted);
        assert_eq!(version.major(), 5);
        assert_eq!(version.layout(), Layout::V5);
        assert!(!version.is_unicode());

        let six = Version::decode(0x0100_6000);
        assert_eq!(six.major(), 6);
        assert_eq!(six.layout(), Layout::V6);
    }

    #[test]
    fn decodes_the_scaled_encoding_with_a_floor() {
        assert_eq!(Version::decode(0x0200_06a4).major(), 17);
        assert_eq!(Version::decode(0x0400_06a4).major(), 17);
        // 0x0064 == 100 -> 1, floored to 5.
        assert_eq!(Version::decode(0x0200_0064).major(), 5);
        // Zero stays zero rather than being floored.
        assert_eq!(Version::decode(0x0200_0000).major(), 0);
    }

    #[test]
    fn unicode_starts_at_major_seventeen() {
        assert!(Version::decode(0x0200_06a4).is_unicode());
        assert!(!Version::decode(0x0200_0640).is_unicode());
    }

    #[test]
    fn untagged_versions_fall_back_to_the_v5_layout() {
        let version = Version::decode(0x0900_0000);
        assert_eq!(version.encoding(), VersionEncoding::Untagged);
        assert_eq!(version.major(), 0);
        assert_eq!(version.layout(), Layout::V5);
    }

    #[test]
    fn forced_versions_keep_the_raw_word() {
        let version = Version::forced(0xdead_beef, 6);
        assert_eq!(version.raw(), 0xdead_beef);
        assert_eq!(version.layout(), Layout::V6);
    }
}
