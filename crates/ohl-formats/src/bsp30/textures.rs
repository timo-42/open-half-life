//! The `LUMP_TEXTURES` (miptex) lump: a directory of per-texture offsets
//! into the same lump, each pointing at a `MiptexHeader` that is either
//! embedded (mip offsets non-zero, body present) or external (all four mip
//! offsets `0`, meaning the texture is stored in a WAD3 package instead;
//! Valve Developer Community "BSP (GoldSrc)").

use zerocopy::byteorder::little_endian::U32;

use crate::bsp30::Limits;
use crate::error::{FormatError, Result};
use crate::miptex_body::{DecodedMiptex, MiptexHeader, decode_body};
use crate::util::{prefix_of, sub_slice};

/// A decoded texture-lump entry.
#[derive(Debug, Clone, Copy)]
pub enum Miptex<'a> {
    /// The texture's pixel data lives in an external WAD3 package; only its
    /// name and declared dimensions are stored here.
    External {
        /// Null-padded 16-byte texture name.
        name: [u8; 16],
        /// Declared width.
        width: u32,
        /// Declared height.
        height: u32,
    },
    /// The texture's pixel data and palette are embedded in this lump.
    Embedded {
        /// Null-padded 16-byte texture name.
        name: [u8; 16],
        /// Declared width.
        width: u32,
        /// Declared height.
        height: u32,
        /// The decoded mip levels and shared palette.
        body: DecodedMiptex<'a>,
    },
}

/// The texture directory: `numtex` followed by `numtex` `i32` offsets, each
/// relative to the start of this lump.
pub struct TextureDirectory<'a> {
    lump: &'a [u8],
    offsets: &'a [U32],
}

/// Parses the texture-lump directory header (`numtex` and its offset
/// table), without yet decoding any individual texture.
pub fn parse_directory<'a>(lump: &'a [u8], limits: &Limits) -> Result<TextureDirectory<'a>> {
    let (count, rest) = prefix_of::<U32>(lump)?;
    let count = count.get() as usize;
    if count > limits.max_textures {
        return Err(FormatError::LimitExceeded);
    }
    let offsets_bytes = sub_slice(
        rest,
        0,
        count.checked_mul(4).ok_or(FormatError::OutOfBounds)?,
    )?;
    let offsets = crate::util::slice_of::<U32>(offsets_bytes)?;
    Ok(TextureDirectory { lump, offsets })
}

impl<'a> TextureDirectory<'a> {
    /// The number of texture slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    /// Whether there are no texture slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Decodes texture slot `index`. Returns `Ok(None)` for the documented
    /// "missing slot" sentinel offset (`0xFFFF_FFFF`), matching common
    /// GoldSrc tooling convention for a texture directory entry that was
    /// never filled in.
    pub fn get(&self, index: usize) -> Result<Option<Miptex<'a>>> {
        let raw_offset = self
            .offsets
            .get(index)
            .ok_or(FormatError::IndexOutOfRange)?
            .get();
        if raw_offset == u32::MAX {
            return Ok(None);
        }
        let start = raw_offset as usize;
        let texture_bytes = self.lump.get(start..).ok_or(FormatError::OutOfBounds)?;
        let (header, _) = prefix_of::<MiptexHeader>(texture_bytes)?;
        let width = header.width.get();
        let height = header.height.get();
        let offsets = [
            header.offsets[0].get(),
            header.offsets[1].get(),
            header.offsets[2].get(),
            header.offsets[3].get(),
        ];
        if offsets == [0, 0, 0, 0] {
            return Ok(Some(Miptex::External {
                name: header.name,
                width,
                height,
            }));
        }
        let body = decode_body(texture_bytes, width, height, offsets)?;
        Ok(Some(Miptex::Embedded {
            name: header.name,
            width,
            height,
            body,
        }))
    }
}
