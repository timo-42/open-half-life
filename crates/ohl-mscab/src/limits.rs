//! Anti-abuse limits for cabinet parsing and extraction.
//!
//! Cabinet headers are attacker-controlled: every count and size in them is a
//! 16- or 32-bit field that a hostile image can set to its maximum. Nothing in
//! this crate allocates or loops proportionally to such a field before it has
//! been checked against a [`Limits`] value and against the pinned source size.

/// The maximum number of uncompressed bytes a single `CFDATA` block may
/// represent, from [MS-CAB] ("32K of uncompressed input").
pub const MAX_BLOCK_UNCOMPRESSED: u16 = 32_768;

/// The maximum number of compressed bytes a single `CFDATA` block may occupy,
/// from [MS-CAB]: "the compressed size of a CFDATA block may not occupy more
/// than 32768+6144 bytes".
pub const MAX_BLOCK_COMPRESSED: u16 = 32_768 + 6_144;

/// Bounds applied to every count, offset, and size read from a cabinet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum accepted `CFHEADER.cFolders`.
    pub max_folders: u32,
    /// Maximum accepted `CFHEADER.cFiles`.
    pub max_files: u32,
    /// Maximum accepted length in bytes of one `CFFILE.szName`, excluding the
    /// terminating NUL.
    pub max_name_bytes: usize,
    /// Maximum accepted length in bytes of one `CFHEADER` cabinet/disk name,
    /// excluding the terminating NUL. The contents are never retained.
    pub max_header_string_bytes: usize,
    /// Maximum accepted `CFHEADER.cbCFHeader` (the per-cabinet reserved area).
    pub max_header_reserve_bytes: u32,
    /// Maximum accepted `CFFOLDER.cCFData` for one folder.
    pub max_blocks_per_folder: u32,
    /// Maximum number of uncompressed bytes one folder may expand to.
    pub max_folder_uncompressed_bytes: u64,
    /// Maximum accepted `CFFILE.cbFile`.
    pub max_file_bytes: u64,
    /// Maximum accepted `CFHEADER.cbCabinet`.
    pub max_cabinet_bytes: u64,
    /// Maximum LZX window size in bits that will be accepted, and therefore
    /// the largest LZX window this decoder will allocate. [MS-CAB] folders
    /// use 15..=21 (32 KiB..2 MiB).
    pub max_lzx_window_bits: u8,
}

impl Limits {
    /// Conservative defaults sized for game-media cabinets: they admit every
    /// structure the format documents at a scale a desktop import can hold,
    /// while refusing images that would force multi-gigabyte work.
    pub const DEFAULT: Self = Self {
        max_folders: 4_096,
        max_files: 65_535,
        max_name_bytes: 255,
        max_header_string_bytes: 255,
        max_header_reserve_bytes: 60_000,
        max_blocks_per_folder: 1 << 20,
        max_folder_uncompressed_bytes: 4 << 30,
        max_file_bytes: 4 << 30,
        max_cabinet_bytes: 1 << 32,
        max_lzx_window_bits: 21,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
