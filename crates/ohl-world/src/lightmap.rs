//! Lightmap extents and a shelf-packed lightmap atlas.
//!
//! GoldSrc stores one light sample ("luxel") every 16 texture units across a
//! face. As documented by the Valve Developer Community's "BSP (GoldSrc)"
//! article, a face's lightmap size is derived from the *snapped* bounds of
//! its texture coordinates: the minimum is rounded down to a multiple of 16
//! and the maximum rounded up, and the sample grid covers both endpoints, so
//! it is one luxel wider and taller than the number of 16-unit cells.

use crate::error::{Result, WorldError};

/// The luxel-grid size and snapped texture-space origin of one face's
/// lightmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightmapExtents {
    /// Snapped minimum S in texture units (always a multiple of 16).
    pub min_s: i32,
    /// Snapped minimum T in texture units (always a multiple of 16).
    pub min_t: i32,
    /// Luxel columns, at least 1.
    pub width: u32,
    /// Luxel rows, at least 1.
    pub height: u32,
}

impl LightmapExtents {
    /// The number of light samples the lighting lump stores for this face
    /// per light style.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// The luxel spacing GoldSrc bakes lightmaps at, in texture units.
pub const LUXEL_SIZE: f32 = 16.0;

/// The largest luxel grid a single face may occupy on either axis. Real
/// GoldSrc faces are capped at 16x16 by the compilers; this ceiling is
/// deliberately looser but still bounds the work a malformed face can force.
pub const MAX_FACE_LUXELS: u32 = 64;

/// Computes a face's lightmap extents from the minimum and maximum texture
/// coordinates (in texture units, i.e. before dividing by the texture size)
/// of its vertices.
///
/// Returns [`WorldError::NonFiniteGeometry`] for non-finite inputs and
/// [`WorldError::LimitExceeded`] when the resulting grid would exceed
/// [`MAX_FACE_LUXELS`] on either axis.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn lightmap_extents(min_s: f32, max_s: f32, min_t: f32, max_t: f32) -> Result<LightmapExtents> {
    if !(min_s.is_finite() && max_s.is_finite() && min_t.is_finite() && max_t.is_finite()) {
        return Err(WorldError::NonFiniteGeometry);
    }
    // Clamp to a range that survives the f32 -> i32 conversion below with
    // room for the +1 luxel, so a wild texture axis cannot wrap.
    let limit = 1_048_576.0f32;
    let clamp = |v: f32| v.clamp(-limit, limit);
    let (min_s, max_s) = (clamp(min_s), clamp(max_s));
    let (min_t, max_t) = (clamp(min_t), clamp(max_t));

    let floor_cell = |v: f32| (v / LUXEL_SIZE).floor() as i32;
    let ceil_cell = |v: f32| (v / LUXEL_SIZE).ceil() as i32;

    let s_lo = floor_cell(min_s);
    let s_hi = ceil_cell(max_s).max(s_lo);
    let t_lo = floor_cell(min_t);
    let t_hi = ceil_cell(max_t).max(t_lo);

    let width = (s_hi - s_lo) as u32 + 1;
    let height = (t_hi - t_lo) as u32 + 1;
    if width > MAX_FACE_LUXELS || height > MAX_FACE_LUXELS {
        return Err(WorldError::LimitExceeded);
    }

    Ok(LightmapExtents {
        min_s: s_lo * 16,
        min_t: t_lo * 16,
        width,
        height,
    })
}

/// One packed rectangle in the atlas, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShelfRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A minimal shelf (row) packer: rectangles are placed left to right on the
/// current shelf and a new shelf is opened, at the height of the tallest
/// rectangle so far, once the current one is full.
///
/// It is deliberately simple rather than optimal; lightmap tiles are small
/// and similarly sized, which is the case shelf packing handles well.
#[derive(Debug)]
pub struct ShelfPacker {
    width: u32,
    max_height: u32,
    padding: u32,
    cursor_x: u32,
    shelf_y: u32,
    shelf_height: u32,
    used_height: u32,
}

impl ShelfPacker {
    /// Creates a packer for an atlas `width` pixels wide that may grow to at
    /// most `max_height` pixels tall, leaving `padding` pixels between
    /// rectangles so bilinear filtering cannot bleed between tiles.
    #[must_use]
    pub fn new(width: u32, max_height: u32, padding: u32) -> Self {
        Self {
            width,
            max_height,
            padding,
            cursor_x: 0,
            shelf_y: 0,
            shelf_height: 0,
            used_height: 0,
        }
    }

    /// Packs a `width` x `height` rectangle, returning its position, or
    /// `None` when the atlas is full.
    pub fn insert(&mut self, width: u32, height: u32) -> Option<ShelfRect> {
        if width > self.width || height > self.max_height {
            return None;
        }
        let advance = width.checked_add(self.padding)?;
        if self.cursor_x.checked_add(width)? > self.width {
            // Open a new shelf below the tallest rectangle on this one.
            self.shelf_y = self.shelf_y.checked_add(self.shelf_height)?;
            self.shelf_height = 0;
            self.cursor_x = 0;
        }
        let y_end = self.shelf_y.checked_add(height)?;
        if y_end > self.max_height {
            return None;
        }
        let rect = ShelfRect {
            x: self.cursor_x,
            y: self.shelf_y,
            width,
            height,
        };
        self.cursor_x = self.cursor_x.checked_add(advance)?;
        self.shelf_height = self.shelf_height.max(height.checked_add(self.padding)?);
        self.used_height = self.used_height.max(y_end);
        Some(rect)
    }

    /// The number of pixel rows actually used so far.
    #[must_use]
    pub fn used_height(&self) -> u32 {
        self.used_height
    }

    /// The atlas width this packer was created with.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }
}

#[cfg(test)]
mod tests {
    use super::{LUXEL_SIZE, ShelfPacker, lightmap_extents};

    #[test]
    fn a_single_luxel_face_is_one_by_one() {
        let extents = lightmap_extents(0.0, 0.0, 0.0, 0.0).expect("finite");
        assert_eq!((extents.width, extents.height), (1, 1));
        assert_eq!(extents.sample_count(), 1);
    }

    #[test]
    fn extents_span_both_endpoints() {
        // 0..64 texture units is four 16-unit cells, so five luxels.
        let extents = lightmap_extents(0.0, 4.0 * LUXEL_SIZE, 0.0, 2.0 * LUXEL_SIZE).expect("ok");
        assert_eq!((extents.width, extents.height), (5, 3));
        assert_eq!(extents.min_s, 0);
    }

    #[test]
    fn extents_snap_outwards() {
        let extents = lightmap_extents(-1.0, 17.0, -33.0, -1.0).expect("ok");
        assert_eq!(extents.min_s, -16);
        assert_eq!(extents.min_t, -48);
        // S spans cells -1..2 and T cells -3..0, and the grid covers both
        // endpoints, so each axis is four luxels.
        assert_eq!((extents.width, extents.height), (4, 4));
    }

    #[test]
    fn rejects_non_finite_and_oversized() {
        assert!(lightmap_extents(f32::NAN, 0.0, 0.0, 0.0).is_err());
        assert!(lightmap_extents(0.0, 100_000.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn packer_opens_new_shelves_and_reports_height() {
        let mut packer = ShelfPacker::new(8, 32, 1);
        let a = packer.insert(4, 3).expect("fits");
        let b = packer.insert(4, 2).expect("fits after padding-free tail");
        assert_eq!((a.x, a.y), (0, 0));
        assert_eq!(b.y, 4, "second rect must start a new shelf");
        assert_eq!(packer.used_height(), 6);
    }

    #[test]
    fn packer_rejects_oversized_rectangles() {
        let mut packer = ShelfPacker::new(8, 8, 1);
        assert!(packer.insert(9, 1).is_none());
        assert!(packer.insert(1, 9).is_none());
    }
}
