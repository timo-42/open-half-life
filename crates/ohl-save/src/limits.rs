//! Caller-supplied bounds enforced while opening or writing a save file.
//!
//! [`SaveReader::open`](crate::SaveReader::open) validates every offset,
//! length, and count taken from a file against these limits before trusting
//! them, and [`SaveWriter::finish`](crate::SaveWriter::finish) validates the
//! container it is about to produce against the same limits, so a caller
//! that shares one `Limits` value between writing and reading never produces
//! a file its own reader would refuse.

/// Bounds a save file's section count and sizes must stay within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The maximum number of section-table entries a file may declare.
    pub max_sections: u32,
    /// The maximum length in bytes of any single section.
    pub max_section_bytes: u64,
    /// The maximum total length in bytes of the whole file.
    pub max_file_bytes: u64,
}

impl Limits {
    /// A conservative built-in default: at most 256 sections, no single
    /// section larger than 64 MiB, and no file larger than 256 MiB.
    pub const DEFAULT: Self = Self {
        max_sections: 256,
        max_section_bytes: 64 * 1024 * 1024,
        max_file_bytes: 256 * 1024 * 1024,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Limits;

    #[test]
    fn default_matches_the_documented_constant() {
        assert_eq!(Limits::default(), Limits::DEFAULT);
    }
}
