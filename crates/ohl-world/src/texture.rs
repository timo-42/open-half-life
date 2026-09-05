//! Decoding GoldSrc indexed textures into RGBA8 images.
//!
//! Embedded BSP miptexes carry their own palette; textures stored externally
//! (all four mip offsets zero) are looked up by name in any WAD3 packages
//! the caller supplied, and fall back to a project-authored checkerboard
//! placeholder when none is available. The placeholder makes a missing WAD
//! obvious on screen without ever failing the load.

use ohl_formats::bsp30::Miptex;
use ohl_formats::palette::Palette;
use ohl_formats::wad3::{self, Wad3};

use crate::error::{Result, WorldError};

/// The largest texture edge this crate will decode.
pub const MAX_TEXTURE_EDGE: u32 = 4096;

/// The edge length of the generated placeholder checkerboard.
pub const PLACEHOLDER_EDGE: u32 = 64;

/// An owned, tightly packed RGBA8 image, top row first.
///
/// Deliberately not `Debug`: an image's bytes are map-derived, and this
/// project never lets media-derived data reach a log line.
#[derive(Clone)]
pub struct TextureImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl TextureImage {
    /// Wraps `rgba` (exactly `width * height * 4` bytes) as an image.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(WorldError::LimitExceeded)?;
        if rgba.len() != expected || width == 0 || height == 0 {
            return Err(WorldError::LimitExceeded);
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Image width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA8 pixel bytes, row-major, `width * height * 4` long.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// A magenta/black checkerboard standing in for a texture whose WAD3
    /// package was not supplied.
    #[must_use]
    pub fn placeholder() -> Self {
        let edge = PLACEHOLDER_EDGE as usize;
        let mut rgba = Vec::with_capacity(edge * edge * 4);
        for y in 0..edge {
            for x in 0..edge {
                let dark = ((x / 8) + (y / 8)) % 2 == 0;
                let pixel = if dark {
                    [16u8, 16, 16, 255]
                } else {
                    [255u8, 0, 220, 255]
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        Self {
            width: PLACEHOLDER_EDGE,
            height: PLACEHOLDER_EDGE,
            rgba,
        }
    }

    /// A one-pixel opaque white image, used as the fullbright lightmap tile.
    #[must_use]
    pub fn white_pixel() -> Self {
        Self {
            width: 1,
            height: 1,
            rgba: vec![255, 255, 255, 255],
        }
    }
}

/// Expands `width * height` palette indices into RGBA8.
///
/// GoldSrc reserves palette index 255 as the transparency key for textures
/// whose name starts with `{`; `transparent_index_255` selects that
/// behaviour, otherwise index 255 is an ordinary color.
fn expand_indexed(
    indices: &[u8],
    palette: Palette<'_>,
    width: u32,
    height: u32,
    transparent_index_255: bool,
) -> Result<TextureImage> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(WorldError::LimitExceeded)?;
    if indices.len() < pixels {
        return Err(WorldError::IndexOutOfRange);
    }
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4).ok_or(WorldError::LimitExceeded)?);
    for &index in &indices[..pixels] {
        let color = palette.get(index);
        let alpha = if transparent_index_255 && index == 255 {
            0
        } else {
            255
        };
        rgba.extend_from_slice(&[color.r, color.g, color.b, alpha]);
    }
    TextureImage::new(width, height, rgba)
}

fn edges_are_sane(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && width <= MAX_TEXTURE_EDGE && height <= MAX_TEXTURE_EDGE
}

pub(crate) fn trimmed(name: &[u8; 16]) -> &[u8] {
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    &name[..end]
}

fn is_transparent_name(name: &[u8; 16]) -> bool {
    trimmed(name).first() == Some(&b'{')
}

/// Resolves one BSP texture slot to an RGBA image, consulting `wads` for
/// externally stored textures and falling back to
/// [`TextureImage::placeholder`] whenever the texture cannot be decoded.
///
/// Never fails on map data: an unusable texture becomes the placeholder, so
/// one broken slot cannot make a whole map unloadable.
pub(crate) fn resolve(slot: Option<Miptex<'_>>, wads: &[Wad3<'_>]) -> TextureImage {
    match slot {
        Some(Miptex::Embedded {
            name,
            width,
            height,
            body,
        }) => {
            if !edges_are_sane(width, height) {
                return TextureImage::placeholder();
            }
            expand_indexed(
                body.mips[0].indices,
                body.palette,
                width,
                height,
                is_transparent_name(&name),
            )
            .unwrap_or_else(|_| TextureImage::placeholder())
        }
        Some(Miptex::External { name, .. }) => {
            from_wads(&name, wads).unwrap_or_else(TextureImage::placeholder)
        }
        None => TextureImage::placeholder(),
    }
}

fn from_wads(name: &[u8; 16], wads: &[Wad3<'_>]) -> Option<TextureImage> {
    let wanted = core::str::from_utf8(trimmed(name)).ok()?;
    for wad in wads {
        let Ok(Some(entry)) = wad.find(wanted) else {
            continue;
        };
        if entry.kind != wad3::EntryKind::Miptex {
            continue;
        }
        let Ok(decoded) = wad.decode_miptex(&entry) else {
            continue;
        };
        if !edges_are_sane(decoded.width, decoded.height) {
            continue;
        }
        if let Ok(image) = expand_indexed(
            decoded.body.mips[0].indices,
            decoded.body.palette,
            decoded.width,
            decoded.height,
            is_transparent_name(name),
        ) {
            return Some(image);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{PLACEHOLDER_EDGE, TextureImage};

    #[test]
    fn placeholder_is_a_full_rgba_checkerboard() {
        let image = TextureImage::placeholder();
        assert_eq!(image.width(), PLACEHOLDER_EDGE);
        assert_eq!(
            image.rgba().len(),
            (PLACEHOLDER_EDGE * PLACEHOLDER_EDGE * 4) as usize
        );
        assert_eq!(&image.rgba()[..4], &[16, 16, 16, 255]);
    }

    #[test]
    fn rejects_mismatched_buffer_length() {
        assert!(TextureImage::new(2, 2, vec![0; 8]).is_err());
        assert!(TextureImage::new(0, 0, Vec::new()).is_err());
    }
}
