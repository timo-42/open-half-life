//! Zero-copy little-endian struct layouts for the Quake `PACK` archive
//! format, as documented in `docs/FORMAT_SOURCES.md` ("Quake PAK
//! archives").

use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The fixed 4-byte PAK signature.
pub const MAGIC: [u8; 4] = *b"PACK";

/// The fixed 56-byte name field length inside one directory entry.
pub const NAME_LEN: usize = 56;

/// The 12-byte PAK file header: magic, directory offset, directory size.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawHeader {
    pub magic: [u8; 4],
    pub dir_offset: U32,
    pub dir_size: U32,
}

/// One 64-byte directory entry: a 56-byte NUL-terminated name followed by
/// the entry's offset and size (both relative to the start of the file).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawEntry {
    pub name: [u8; NAME_LEN],
    pub offset: U32,
    pub size: U32,
}
