//! Zero-copy little-endian struct layouts for WAD3, as documented in
//! `docs/FORMAT_SOURCES.md` ("GoldSrc BSP v30 and WAD3"), source: "Unofficial
//! Half-Life WAD3 and SPRITE file format specification", Yuraj, rev. 05,
//! 2012-01-15.

use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The fixed 4-byte WAD3 signature.
pub const MAGIC: [u8; 4] = *b"WAD3";

/// The fixed 16-byte name field length.
pub const NAME_LEN: usize = 16;

/// The WAD3 file header.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawHeader {
    pub magic: [u8; 4],
    pub num_entries: U32,
    pub dir_offset: U32,
}

/// One 32-byte directory entry.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawDirectoryEntry {
    pub offset: U32,
    pub disk_size: U32,
    pub full_size: U32,
    pub kind: u8,
    pub compression: u8,
    pub padding: [u8; 2],
    pub name: [u8; NAME_LEN],
}

/// Documented WAD3 entry-type bytes (Yuraj spec, "Lump item info" /
/// "Texture" tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// `0x40`: spray decal / "tempdecal.wad"-style entry (miptex body
    /// layout, no palette).
    SprayDecal,
    /// `0x42`: qpic ("cached.wad"-style) image.
    Qpic,
    /// `0x43`: a world miptex, with an embedded palette.
    Miptex,
    /// `0x46`: a font entry.
    Font,
    /// Any other byte value; not decoded by this crate.
    Unknown(u8),
}

impl EntryKind {
    /// Maps a raw entry-type byte to its documented meaning.
    #[must_use]
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0x40 => Self::SprayDecal,
            0x42 => Self::Qpic,
            0x43 => Self::Miptex,
            0x46 => Self::Font,
            other => Self::Unknown(other),
        }
    }
}
