//! GoldSrc sprite ("SPR") decoding.
//!
//! See `docs/FORMAT_SOURCES.md` ("GoldSrc MDL v10 and SPR") for the public
//! documentation this module was implemented from ("Unofficial Half-Life
//! WAD3 and SPRITE file format specification", Yuraj, rev. 05). As with the
//! rest of this crate, [`Spr`] is a borrowing, zero-copy view: every count
//! is validated against the actual buffer before use, and no accessor
//! panics on malformed input.
//!
//! Unlike WAD3/BSP30 miptexes (always a fixed 256-entry palette), the
//! documented sprite palette is size-prefixed (`u16` color count then that
//! many RGB triples), so this module does not reuse
//! [`crate::palette::Palette`]/[`crate::palette::Indexed8`] (both fixed to
//! exactly 256 entries) and instead exposes the palette as a plain
//! `&[Rgb8]` slice with its own bounds-checked pixel lookup.

use core::mem::size_of;

use zerocopy::byteorder::little_endian::{F32, I32, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::error::{FormatError, Result};
use crate::palette::Rgb8;
use crate::util::{checked_pixel_count, prefix_of, slice_of, sub_slice};

/// The fixed 4-byte signature ("IDSP").
pub const MAGIC: [u8; 4] = *b"IDSP";
/// The documented sprite format version.
pub const VERSION: i32 = 2;

/// Documented sprite orientation types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteType {
    /// `0`: faces the camera, staying upright.
    ParallelUpright,
    /// `1`: faces the camera around the vertical axis only.
    FacingUpright,
    /// `2`: faces the camera on all axes.
    Parallel,
    /// `3`: a fixed orientation, ignoring the camera.
    Oriented,
    /// `4`: faces the camera but keeps a fixed roll.
    ParallelOriented,
    /// Any other value; not decoded further by this crate.
    Unknown(i32),
}

impl SpriteType {
    #[must_use]
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::ParallelUpright,
            1 => Self::FacingUpright,
            2 => Self::Parallel,
            3 => Self::Oriented,
            4 => Self::ParallelOriented,
            other => Self::Unknown(other),
        }
    }
}

/// Documented sprite texture (render) formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// `0`: opaque.
    Normal,
    /// `1`: additive blending.
    Additive,
    /// `2`: alpha taken from palette index intensity.
    IndexAlpha,
    /// `3`: binary alpha test.
    AlphaTest,
    /// Any other value; not decoded further by this crate.
    Unknown(i32),
}

impl TextureFormat {
    #[must_use]
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Normal,
            1 => Self::Additive,
            2 => Self::IndexAlpha,
            3 => Self::AlphaTest,
            other => Self::Unknown(other),
        }
    }
}

/// Documented sprite animation synchronization types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncType {
    /// `0`: all instances of this sprite animate in lockstep.
    Synchronized,
    /// `1`: each instance's animation phase is randomized.
    Random,
    /// Any other value; not decoded further by this crate.
    Unknown(i32),
}

impl SyncType {
    #[must_use]
    fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Synchronized,
            1 => Self::Random,
            other => Self::Unknown(other),
        }
    }
}

/// Bounds this crate enforces while decoding an SPR file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest number of frames a sprite may declare.
    pub max_frames: usize,
    /// The largest single frame's `width * height` pixel count.
    pub max_frame_pixels: u32,
    /// The largest declared palette color count this crate will decode
    /// (real GoldSrc sprites always declare `256`; this bound exists only
    /// to reject an adversarial oversized count cheaply).
    pub max_palette_colors: usize,
}

impl Limits {
    /// Conservative defaults.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_frames: 8192,
            max_frame_pixels: 8 * 1024 * 1024,
            max_palette_colors: 65_535,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// The fixed 40-byte sprite header.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawHeader {
    pub id: [u8; 4],
    pub version: I32,
    pub kind: I32,
    pub texture_format: I32,
    pub bounding_radius: F32,
    pub max_width: U32,
    pub max_height: U32,
    pub num_frames: U32,
    pub beam_length: F32,
    pub sync_type: I32,
}

/// One frame's fixed 20-byte header (excludes pixel data).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawFrame {
    pub group: U32,
    pub origin_x: I32,
    pub origin_y: I32,
    pub width: U32,
    pub height: U32,
}

/// A palette-indexed image borrowed from a sprite frame, resolved against
/// that sprite's (possibly non-256-entry) palette.
#[derive(Debug, Clone, Copy)]
pub struct SprImage<'a> {
    /// Palette index per pixel, row-major, `width * height` entries.
    pub indices: &'a [u8],
    /// The sprite's shared palette (see the module-level note on why this
    /// is a plain slice rather than [`crate::palette::Palette`]).
    pub palette: &'a [Rgb8],
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

impl SprImage<'_> {
    /// Resolves the pixel at `(x, y)` to an RGB color, or `None` if out of
    /// range or if the palette index has no matching palette entry (a
    /// malformed file could declare a palette shorter than 256 colors).
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgb8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?;
        let index = *self.indices.get(offset)?;
        self.palette.get(index as usize).copied()
    }
}

/// One decoded sprite frame.
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// The documented "frame group" field. The reviewed specification
    /// (Yuraj rev. 05) describes only this flat per-frame layout — a
    /// `group` field followed directly by origin/width/height/pixel data —
    /// without a separate nested "group frame" (interval array plus
    /// repeated single frames) sub-structure, so this project decodes
    /// exactly the documented flat layout and exposes `group` unmodified
    /// rather than inventing an undocumented grouped form.
    pub group: u32,
    /// Frame origin X (signed, per the documented `int` type).
    pub origin_x: i32,
    /// Frame origin Y.
    pub origin_y: i32,
    /// The frame's 8-bit indexed pixel data and the palette it indexes
    /// into.
    pub image: SprImage<'a>,
}

/// A validated, zero-copy view over one SPR file.
pub struct Spr<'a> {
    data: &'a [u8],
    header: RawHeader,
    palette: &'a [Rgb8],
    frames_start: usize,
}

fn header_end() -> usize {
    size_of::<RawHeader>()
}

impl<'a> Spr<'a> {
    /// Parses and validates a sprite file's header and palette, and walks
    /// (without allocating) far enough to confirm every declared frame's
    /// header and pixel data fits within `data`.
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self> {
        let (header, _): (&RawHeader, _) = prefix_of(data)?;
        if header.id != MAGIC {
            return Err(FormatError::BadSignature);
        }
        if header.version.get() != VERSION {
            return Err(FormatError::BadSignature);
        }
        let header = *header;
        let num_frames = header.num_frames.get() as usize;
        if num_frames > limits.max_frames {
            return Err(FormatError::LimitExceeded);
        }

        let count_bytes = sub_slice(data, header_end(), 2)?;
        let palette_len = u16::from_le_bytes([count_bytes[0], count_bytes[1]]) as usize;
        if palette_len > limits.max_palette_colors {
            return Err(FormatError::LimitExceeded);
        }
        let palette_start = header_end() + 2;
        let palette_byte_len = palette_len.checked_mul(3).ok_or(FormatError::OutOfBounds)?;
        let palette_bytes = sub_slice(data, palette_start, palette_byte_len)?;
        let palette = slice_of::<Rgb8>(palette_bytes)?;

        let frames_start = palette_start + palette_byte_len;
        let mut cursor = frames_start;
        for _ in 0..num_frames {
            let (frame_header, _): (&RawFrame, _) =
                prefix_of(data.get(cursor..).ok_or(FormatError::OutOfBounds)?)?;
            let width = frame_header.width.get();
            let height = frame_header.height.get();
            let pixel_count = checked_pixel_count(width, height, limits.max_frame_pixels)?;
            let after_header = cursor
                .checked_add(size_of::<RawFrame>())
                .ok_or(FormatError::OutOfBounds)?;
            sub_slice(data, after_header, pixel_count)?;
            cursor = after_header
                .checked_add(pixel_count)
                .ok_or(FormatError::OutOfBounds)?;
        }

        Ok(Self {
            data,
            header,
            palette,
            frames_start,
        })
    }

    /// The sprite's orientation type.
    #[must_use]
    pub fn kind(&self) -> SpriteType {
        SpriteType::from_i32(self.header.kind.get())
    }

    /// The sprite's render/texture format.
    #[must_use]
    pub fn texture_format(&self) -> TextureFormat {
        TextureFormat::from_i32(self.header.texture_format.get())
    }

    /// The sprite's animation synchronization type.
    #[must_use]
    pub fn sync_type(&self) -> SyncType {
        SyncType::from_i32(self.header.sync_type.get())
    }

    /// The bounding radius, as declared in the header.
    #[must_use]
    pub fn bounding_radius(&self) -> f32 {
        self.header.bounding_radius.get()
    }

    /// The declared maximum frame width/height.
    #[must_use]
    pub fn max_size(&self) -> (u32, u32) {
        (self.header.max_width.get(), self.header.max_height.get())
    }

    /// The beam length (used only by beam-type sprites).
    #[must_use]
    pub fn beam_length(&self) -> f32 {
        self.header.beam_length.get()
    }

    /// The number of frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.header.num_frames.get() as usize
    }

    /// The shared palette (its declared length, which is not required to
    /// be exactly 256 by the reviewed specification).
    #[must_use]
    pub fn palette(&self) -> &'a [Rgb8] {
        self.palette
    }

    /// Decodes frame `index`.
    pub fn frame(&self, index: usize, limits: &Limits) -> Result<Frame<'a>> {
        if index >= self.frame_count() {
            return Err(FormatError::IndexOutOfRange);
        }
        let mut cursor = self.frames_start;
        for i in 0..=index {
            let (frame_header, _): (&RawFrame, _) =
                prefix_of(self.data.get(cursor..).ok_or(FormatError::OutOfBounds)?)?;
            let width = frame_header.width.get();
            let height = frame_header.height.get();
            let pixel_count = checked_pixel_count(width, height, limits.max_frame_pixels)?;
            let after_header = cursor
                .checked_add(size_of::<RawFrame>())
                .ok_or(FormatError::OutOfBounds)?;
            let pixels = sub_slice(self.data, after_header, pixel_count)?;
            if i == index {
                return Ok(Frame {
                    group: frame_header.group.get(),
                    origin_x: frame_header.origin_x.get(),
                    origin_y: frame_header.origin_y.get(),
                    image: SprImage {
                        indices: pixels,
                        palette: self.palette,
                        width,
                        height,
                    },
                });
            }
            cursor = after_header
                .checked_add(pixel_count)
                .ok_or(FormatError::OutOfBounds)?;
        }
        Err(FormatError::IndexOutOfRange)
    }
}
