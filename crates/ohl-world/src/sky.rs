//! Skybox classification and the six-face cubemap asset.
//!
//! GoldSrc's `sky` texture is special-cased rather than drawn as geometry: a
//! map's `skyname` worldspawn key names a set of six 24-bit TGA images (the
//! Valve Developer Community's "Skybox" article, and the GoldSrc-specific
//! "Skybox (2D)" / mapping-tutorial pages that mirror it, document the file
//! set as `<skyname><suffix>.tga` for the suffixes `bk`, `dn`, `ft`, `lf`,
//! `rt`, `up`, each 256x256). See `docs/FORMAT_SOURCES.md`, "Rendering
//! conventions", for the full citation list.

use std::io::Cursor;

use crate::error::{Result, WorldError};
use crate::texture::{MAX_TEXTURE_EDGE, TextureImage};

/// The documented skybox face-name suffixes, in the order
/// [`SkyboxAsset::build`] expects them.
pub const SKY_FACE_SUFFIXES: [&str; 6] = ["bk", "dn", "ft", "lf", "rt", "up"];

/// Whether `name` (a BSP miptex or WAD3 texture name) is GoldSrc's
/// special-cased sky surface.
///
/// The compiler leaves a `sky`-named surface unlit (it carries the
/// `TEX_SPECIAL` texture-info flag) and the engine draws the skybox in its
/// place instead of the surface's own geometry; both are documented on the
/// Valve Developer Community's BSP and Skybox articles. The match is
/// case-insensitive and only requires the `sky` prefix, matching how the
/// stock content names every sky variant (`sky`, `skybk`, ... as well as
/// custom per-map names some compilers still prefix with `sky`).
#[must_use]
pub fn is_sky_texture(name: &str) -> bool {
    name.len() >= 3 && name.as_bytes()[..3].eq_ignore_ascii_case(b"sky")
}

/// Six decoded skybox faces, in [`SKY_FACE_SUFFIXES`] order.
pub struct SkyboxAsset {
    /// One RGBA8 image per entry of [`SKY_FACE_SUFFIXES`].
    pub faces: [TextureImage; 6],
}

impl SkyboxAsset {
    /// Decodes six independently supplied TGA (or BMP) images, one per
    /// [`SKY_FACE_SUFFIXES`] entry, in that order.
    ///
    /// Never panics: a face that fails to decode, or whose dimensions do not
    /// match the others, is reported as [`WorldError::InvalidImage`] rather
    /// than propagating any decoder panic (the `image` crate only decodes
    /// its own validated structures here; TGA/BMP support is the only
    /// feature enabled).
    pub fn build(face_bytes: [&[u8]; 6]) -> Result<Self> {
        let mut decoded: Vec<TextureImage> = Vec::with_capacity(6);
        for bytes in face_bytes {
            decoded.push(decode_face(bytes)?);
        }
        let first_size = (decoded[0].width(), decoded[0].height());
        if decoded
            .iter()
            .any(|face| (face.width(), face.height()) != first_size)
        {
            return Err(WorldError::InvalidImage);
        }
        let faces: [TextureImage; 6] = decoded
            .try_into()
            .unwrap_or_else(|_| unreachable!("exactly six faces were pushed above"));
        Ok(Self { faces })
    }
}

/// Decodes one face, reading only the format's fixed-size header to check
/// its declared width/height against [`MAX_TEXTURE_EDGE`] *before* decoding
/// any pixel data, so a maliciously (or corruptly) huge declared dimension
/// is rejected instead of driving an unbounded allocation.
fn decode_face(bytes: &[u8]) -> Result<TextureImage> {
    if let Ok(decoder) = image::codecs::tga::TgaDecoder::new(Cursor::new(bytes)) {
        return decode_from_capped(decoder);
    }
    let decoder = image::codecs::bmp::BmpDecoder::new(Cursor::new(bytes))
        .map_err(|_| WorldError::InvalidImage)?;
    decode_from_capped(decoder)
}

fn decode_from_capped<D: image::ImageDecoder>(source: D) -> Result<TextureImage> {
    let (width, height) = source.dimensions();
    if width == 0 || height == 0 || width > MAX_TEXTURE_EDGE || height > MAX_TEXTURE_EDGE {
        return Err(WorldError::InvalidImage);
    }
    let image = image::DynamicImage::from_decoder(source).map_err(|_| WorldError::InvalidImage)?;
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    TextureImage::new(width, height, rgba.into_raw())
}

#[cfg(test)]
mod tests {
    use super::{SKY_FACE_SUFFIXES, SkyboxAsset, is_sky_texture};

    #[test]
    fn classifies_sky_texture_names_case_insensitively() {
        assert!(is_sky_texture("sky"));
        assert!(is_sky_texture("SKYBK"));
        assert!(is_sky_texture("Sky_Custom"));
        assert!(!is_sky_texture("brick"));
        assert!(!is_sky_texture("sk"));
        assert!(!is_sky_texture(""));
    }

    #[test]
    fn face_order_matches_the_documented_suffixes() {
        assert_eq!(SKY_FACE_SUFFIXES, ["bk", "dn", "ft", "lf", "rt", "up"]);
    }

    fn tiny_tga(color: [u8; 3]) -> Vec<u8> {
        // A minimal 1x1 24-bit uncompressed TGA: 18-byte header then one
        // little-endian BGR pixel.
        let mut bytes = vec![0u8; 18];
        bytes[2] = 2; // uncompressed true-color
        bytes[12] = 1; // width low byte
        bytes[14] = 1; // height low byte
        bytes[16] = 24; // bits per pixel
        bytes.extend_from_slice(&[color[2], color[1], color[0]]);
        bytes
    }

    #[test]
    fn builds_from_six_synthetic_tga_faces() {
        let colors: [[u8; 3]; 6] = [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [0, 255, 255],
            [255, 0, 255],
        ];
        let tgas: Vec<Vec<u8>> = colors.iter().map(|&color| tiny_tga(color)).collect();
        let refs: [&[u8]; 6] = core::array::from_fn(|i| tgas[i].as_slice());
        let skybox = SkyboxAsset::build(refs).expect("six valid faces decode");
        for (face, color) in skybox.faces.iter().zip(colors) {
            assert_eq!(face.width(), 1);
            assert_eq!(face.height(), 1);
            assert_eq!(&face.rgba()[..3], &color);
        }
    }

    #[test]
    fn rejects_invalid_face_bytes() {
        let bad = [0u8; 4];
        let good = tiny_tga([1, 2, 3]);
        let faces: [&[u8]; 6] = [&bad, &good, &good, &good, &good, &good];
        assert!(SkyboxAsset::build(faces).is_err());
    }

    #[test]
    fn rejects_a_declared_dimension_above_the_cap_without_decoding_pixels() {
        // A header claiming a 65535x65535 image (far above
        // `MAX_TEXTURE_EDGE`) but with no pixel data at all: if the cap
        // were checked only after decoding, this would either panic or
        // attempt a multi-gigabyte allocation instead of erroring cleanly.
        let mut huge = vec![0u8; 18];
        huge[2] = 2; // uncompressed true-color
        huge[12] = 0xFF; // width low byte
        huge[13] = 0xFF; // width high byte
        huge[14] = 0xFF; // height low byte
        huge[15] = 0xFF; // height high byte
        huge[16] = 24; // bits per pixel
        let good = tiny_tga([1, 2, 3]);
        let faces: [&[u8]; 6] = [&huge, &good, &good, &good, &good, &good];
        assert!(SkyboxAsset::build(faces).is_err());
    }

    #[test]
    fn rejects_mismatched_face_dimensions() {
        let small = tiny_tga([1, 2, 3]);
        let mut big = vec![0u8; 18];
        big[2] = 2;
        big[12] = 2; // width 2
        big[14] = 1;
        big[16] = 24;
        big.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let faces: [&[u8]; 6] = [&small, &small, &small, &small, &small, &big];
        assert!(SkyboxAsset::build(faces).is_err());
    }
}
