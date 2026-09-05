//! 8-bit indexed color and the shared 256-entry RGB palette used by both
//! WAD3 miptexes and embedded BSP30 textures.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The fixed palette size used by GoldSrc indexed textures.
pub const PALETTE_LEN: usize = 256;

/// One 24-bit RGB color, stored as three bytes with no padding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned,
)]
#[repr(C)]
pub struct Rgb8 {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb8 {
    /// Constructs a color from its channels.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// A 256-entry RGB palette, borrowed from the underlying file bytes.
#[derive(Debug, Clone, Copy)]
pub struct Palette<'a> {
    entries: &'a [Rgb8; PALETTE_LEN],
}

impl<'a> Palette<'a> {
    /// Wraps an already-validated, exactly-256-entry palette slice.
    #[must_use]
    pub(crate) fn new(entries: &'a [Rgb8; PALETTE_LEN]) -> Self {
        Self { entries }
    }

    /// Returns the color for `index`, or `None` if out of range (indices are
    /// always `0..256` so this only fails for values that cannot occur from
    /// an 8-bit index, but callers should still treat it as fallible).
    #[must_use]
    pub fn get(&self, index: u8) -> Rgb8 {
        self.entries[index as usize]
    }

    /// Returns the full 256-entry table.
    #[must_use]
    pub fn entries(&self) -> &'a [Rgb8; PALETTE_LEN] {
        self.entries
    }
}

/// An 8-bit indexed image: raw palette indices plus the palette to resolve
/// them against.
#[derive(Debug, Clone, Copy)]
pub struct Indexed8<'a> {
    /// Palette index per pixel, row-major, `width * height` entries.
    pub indices: &'a [u8],
    /// The palette the indices are drawn from.
    pub palette: Palette<'a>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

impl Indexed8<'_> {
    /// Resolves the pixel at `(x, y)` to an RGB color.
    ///
    /// Returns `None` if `(x, y)` is outside `width`/`height` rather than
    /// panicking.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgb8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?;
        let index = *self.indices.get(offset)?;
        Some(self.palette.get(index))
    }
}
