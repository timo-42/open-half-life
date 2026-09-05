//! The miptex pixel/palette body shared by WAD3 texture entries and embedded
//! BSP30 textures (see `docs/FORMAT_SOURCES.md`, "GoldSrc BSP v30 and
//! WAD3"): four indexed mip levels at half resolution each, followed by a
//! `u16` palette length (always `256`) and 256 RGB triples.
//!
//! Both callers pass `data` already sliced to start exactly at the miptex
//! header (`name`/`width`/`height`/`offsets`); the four `offsets` are
//! relative to that same start, matching both documented sources.

use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::error::{FormatError, Result};
use crate::palette::{PALETTE_LEN, Palette, Rgb8};
use crate::util::{exact_of, sub_slice};

/// The 40-byte in-lump/in-entry miptex header shared by embedded BSP30
/// textures (`BSPMIPTEX`) and WAD3 `0x43` entries: name, dimensions, and
/// four mip offsets relative to the start of this header. For an embedded
/// BSP30 texture, all four offsets are `0` when the texture is instead
/// stored externally in a WAD3 package.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct MiptexHeader {
    pub name: [u8; 16],
    pub width: U32,
    pub height: U32,
    pub offsets: [U32; 4],
}

/// One indexed mip level: its own (halved) dimensions and raw pixel indices.
#[derive(Debug, Clone, Copy)]
pub struct MipLevel<'a> {
    /// This mip level's width in pixels.
    pub width: u32,
    /// This mip level's height in pixels.
    pub height: u32,
    /// `width * height` palette indices, row-major.
    pub indices: &'a [u8],
}

/// A fully decoded miptex body: four mip levels sharing one 256-color
/// palette.
#[derive(Debug, Clone, Copy)]
pub struct DecodedMiptex<'a> {
    /// The four mip levels, full resolution first.
    pub mips: [MipLevel<'a>; 4],
    /// The palette every mip level's indices are drawn from.
    pub palette: Palette<'a>,
}

/// Decodes the body starting at `width`/`height`/`offsets` (all relative to
/// the start of `data`, i.e. the start of the 40-byte name/width/height/
/// offsets header itself).
pub(crate) fn decode_body<'a>(
    data: &'a [u8],
    width: u32,
    height: u32,
    offsets: [u32; 4],
) -> Result<DecodedMiptex<'a>> {
    let mut mips: [MipLevel<'a>; 4] = [
        MipLevel {
            width: 0,
            height: 0,
            indices: &[],
        },
        MipLevel {
            width: 0,
            height: 0,
            indices: &[],
        },
        MipLevel {
            width: 0,
            height: 0,
            indices: &[],
        },
        MipLevel {
            width: 0,
            height: 0,
            indices: &[],
        },
    ];

    let mut w = width;
    let mut h = height;
    for (level, offset) in offsets.iter().enumerate() {
        let count = (w as usize)
            .checked_mul(h as usize)
            .ok_or(FormatError::InvalidInput)?;
        let indices = sub_slice(data, *offset as usize, count)?;
        mips[level] = MipLevel {
            width: w,
            height: h,
            indices,
        };
        w /= 2;
        h /= 2;
    }

    let mip3_end = (offsets[3] as usize)
        .checked_add(mips[3].indices.len())
        .ok_or(FormatError::OutOfBounds)?;
    let count_bytes = sub_slice(data, mip3_end, 2)?;
    let palette_len = u16::from_le_bytes([count_bytes[0], count_bytes[1]]) as usize;
    if palette_len != PALETTE_LEN {
        return Err(FormatError::InvalidInput);
    }
    let palette_start = mip3_end.checked_add(2).ok_or(FormatError::OutOfBounds)?;
    let palette_bytes = sub_slice(data, palette_start, PALETTE_LEN * 3)?;
    let palette_array = exact_of::<[Rgb8; PALETTE_LEN]>(palette_bytes)?;

    Ok(DecodedMiptex {
        mips,
        palette: Palette::new(palette_array),
    })
}
