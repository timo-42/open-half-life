//! A renderable sprite asset built from `ohl_formats::spr`.
//!
//! Decodes every frame of a validated SPR file into an owned RGBA8 image,
//! resolving the documented per-frame alpha convention for each of the four
//! `TextureFormat` values (already decoded by `ohl_formats::spr` from the
//! "Unofficial Half-Life WAD3 and SPRITE file format specification", see
//! `docs/FORMAT_SOURCES.md`, "GoldSrc MDL v10 and SPR"): `Normal` and
//! `Additive` are opaque, `AlphaTest` keys palette index 255 to zero alpha
//! (the same convention BSP/WAD3 `{`-masked textures use), and `IndexAlpha`
//! uses the palette index itself as the alpha value.

use ohl_formats::spr::{Limits as SprLimits, Spr, SpriteType, SyncType, TextureFormat};

use crate::error::{Result, WorldError};
use crate::texture::TextureImage;

/// The documented cap on sprite animation speed: the engine only advances
/// sprite animation state on a fixed 10 Hz tick (Valve Developer Community
/// `env_sprite` article; see `docs/FORMAT_SOURCES.md`, "Rendering
/// conventions"), so a declared `framerate` above this is never actually
/// reached.
pub const MAX_SPRITE_FRAMERATE: f32 = 10.0;

/// A decoded sprite: its orientation/blend metadata plus one RGBA8 image per
/// frame.
pub struct SpriteAsset {
    /// The documented billboard/orientation type.
    pub kind: SpriteType,
    /// The documented per-pixel alpha convention already applied to
    /// [`Self::frames`].
    pub texture_format: TextureFormat,
    /// Whether every instance of this sprite animates in lockstep or with a
    /// randomized phase.
    pub sync_type: SyncType,
    /// One decoded RGBA8 image per sprite frame, in file order.
    pub frames: Vec<TextureImage>,
}

impl SpriteAsset {
    /// Decodes every frame of `bytes` as a validated SPR file.
    pub fn build(bytes: &[u8], limits: &SprLimits) -> Result<Self> {
        let spr = Spr::parse(bytes, limits).map_err(WorldError::Format)?;
        let texture_format = spr.texture_format();
        let mut frames = Vec::with_capacity(spr.frame_count());
        for index in 0..spr.frame_count() {
            let frame = spr.frame(index, limits).map_err(WorldError::Format)?;
            frames.push(decode_frame(
                frame.image.indices,
                frame.image.palette,
                frame.image.width,
                frame.image.height,
                texture_format,
            )?);
        }
        if frames.is_empty() {
            return Err(WorldError::InvalidImage);
        }
        Ok(Self {
            kind: spr.kind(),
            texture_format,
            sync_type: spr.sync_type(),
            frames,
        })
    }

    /// The frame index to display at `time_seconds`, for a sprite whose
    /// entity declares `declared_framerate` frames per second (`0` or
    /// negative selects the documented default of [`MAX_SPRITE_FRAMERATE`]).
    ///
    /// Never panics and never indexes out of range: an empty frame list
    /// reads as frame `0`.
    #[must_use]
    pub fn frame_at(&self, time_seconds: f32, declared_framerate: f32) -> usize {
        if self.frames.is_empty() {
            return 0;
        }
        let framerate = if declared_framerate.is_finite() && declared_framerate > 0.0 {
            declared_framerate.min(MAX_SPRITE_FRAMERATE)
        } else {
            MAX_SPRITE_FRAMERATE
        };
        let elapsed = if time_seconds.is_finite() {
            time_seconds.max(0.0)
        } else {
            0.0
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let step = (elapsed * framerate) as usize;
        step % self.frames.len()
    }
}

fn decode_frame(
    indices: &[u8],
    palette: &[ohl_formats::palette::Rgb8],
    width: u32,
    height: u32,
    format: TextureFormat,
) -> Result<TextureImage> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(WorldError::LimitExceeded)?;
    if indices.len() < pixel_count || width == 0 || height == 0 {
        return Err(WorldError::IndexOutOfRange);
    }
    let mut rgba = Vec::with_capacity(
        pixel_count
            .checked_mul(4)
            .ok_or(WorldError::LimitExceeded)?,
    );
    for &index in &indices[..pixel_count] {
        let color = palette
            .get(index as usize)
            .copied()
            .unwrap_or(ohl_formats::palette::Rgb8 { r: 0, g: 0, b: 0 });
        let alpha = match format {
            TextureFormat::IndexAlpha => index,
            TextureFormat::AlphaTest if index == 255 => 0,
            _ => 255,
        };
        rgba.extend_from_slice(&[color.r, color.g, color.b, alpha]);
    }
    TextureImage::new(width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::{MAX_SPRITE_FRAMERATE, SpriteAsset};
    use ohl_formats::spr::Limits;
    use ohl_formats::test_support::build_minimal_spr;

    #[test]
    fn builds_frames_from_a_synthetic_sprite() {
        let bytes = build_minimal_spr();
        let asset = SpriteAsset::build(&bytes, &Limits::default()).expect("synthetic spr decodes");
        assert!(!asset.frames.is_empty());
        for frame in &asset.frames {
            assert_eq!(frame.rgba().len() % 4, 0);
        }
    }

    #[test]
    fn frame_timing_advances_and_wraps() {
        let bytes = build_minimal_spr();
        let asset = SpriteAsset::build(&bytes, &Limits::default()).expect("decodes");
        let count = asset.frames.len();
        assert_eq!(asset.frame_at(0.0, 10.0), 0);
        if count > 1 {
            assert_eq!(asset.frame_at(0.1, 10.0), 1 % count);
        }
        // A non-positive or absurd declared rate falls back to the
        // documented 10 Hz cap rather than never advancing or overflowing.
        assert_eq!(
            asset.frame_at(0.1, 0.0),
            asset.frame_at(0.1, MAX_SPRITE_FRAMERATE)
        );
        assert_eq!(
            asset.frame_at(0.1, 1_000_000.0),
            asset.frame_at(0.1, MAX_SPRITE_FRAMERATE)
        );
        // Time far beyond one cycle still wraps into range.
        assert!(asset.frame_at(1_000.0, 10.0) < count.max(1));
    }

    #[test]
    fn frame_timing_never_panics_on_non_finite_input() {
        let bytes = build_minimal_spr();
        let asset = SpriteAsset::build(&bytes, &Limits::default()).expect("decodes");
        let _ = asset.frame_at(f32::NAN, f32::NAN);
        let _ = asset.frame_at(f32::INFINITY, -1.0);
    }
}
