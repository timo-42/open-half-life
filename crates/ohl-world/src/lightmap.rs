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

/// The documented GoldSrc lighting-ramp parameters applied to compiled
/// lightmap samples at atlas-build time.
///
/// GoldSrc does not composite raw luxels: the documented client cvars
/// `texgamma`, `lightgamma`, `brightness` and `gamma` describe a load-time
/// ramp between the compiled lightmap and the gamma space the diffuse
/// texture is multiplied in (Valve Developer Community, "Lightmap
/// (GoldSrc)"; MetaHookSv renderer documentation, which states that
/// `texgamma` "convert[s] textures from gamma color space to linear color
/// space", `lightgamma` "convert[s] lightmaps from gamma color space to
/// linear color space", `brightness` "shift[s] up the lightgamma and make[s]
/// lightmaps brighter" and `gamma` "control[s] the final output gamma"; see
/// `docs/FORMAT_SOURCES.md`, "Rendering conventions"). The published
/// documentation gives the conventions and the cvar defaults but no
/// transcribable formula, so the composition below is this project's own
/// parameterised reconstruction of them, calibrated black-box against
/// public screenshots; no engine source was consulted.
///
/// For a stored luxel `l` in `0.0..=1.0` the ramp is
///
/// ```text
/// display = l ^ (1 / lightgamma)                       // lightgamma ramp
/// shifted = clamp(display * overbright + brightness, 0, 1)
/// out     = shifted ^ (lightgamma / texgamma)          // into texture space
/// ```
///
/// Step 1 encodes the compiled sample (an accumulated, linear light
/// intensity) through the documented `lightgamma` ramp. Step 2 is the
/// documented `brightness` shift and the overbright/lightscale multiplier,
/// with the documented clamp to the representable range. Step 3 rebases the
/// result from `lightgamma` space into the `texgamma` space the decoded
/// 8-bit diffuse texture is already stored in, so the shader's product of
/// the two samples is a same-space product and `world.wgsl` stays a plain
/// multiply.
///
/// At the documented defaults the composition reduces to `l ^ (1 /
/// texgamma)`, a ~2.1x lift on mid-tone luxels and the identity at both
/// endpoints; `brightness`/`overbright` are the only stages that clip.
///
/// ## `overbright`'s default (fidelity round 4, finding E5)
///
/// A black-box fidelity review (`.plan/fidelity-round-4.md`, "E5") measured
/// this project's mean scene luma at roughly 1.7x below public reference
/// screenshots across six clean viewpoints and no code path in this ramp
/// changed between rounds, so the review asked whether the wider deficit
/// has a documented, non-fitted explanation. GoldSrc's OpenGL renderer does
/// implement a real lightmap "overbright" convention, inherited from the
/// wider Quake engine family: the Valve Developer Community's "GoldSrc"
/// article and public gameplay/modding references (`gl_overbright`, a
/// client cvar toggling "maximum brightness mode") describe it as doubling
/// lit-surface brightness beyond the ordinary compiled range, matching
/// Quake's own documented software-renderer overbright behaviour (up to
/// 200% lightmap brightness) that GLQuake's original hardware renderer
/// dropped and later `gl_overbright`-style hardware renderers restored via
/// a gamma-ramp doubling trick. Critically, though, every source found also
/// documents `gl_overbright` as **disabled by default** in stock,
/// unmodified Half-Life (community console-command references consistently
/// give its default cvar value as `0`; the VDC article separately notes it
/// was frequently non-functional in practice due to an unrelated
/// multitexturing-detection bug until the 25th Anniversary Update). No
/// source found pins a specific default multiplier this project should
/// adopt as its own shipped default — adopting one would be exactly the
/// screenshot-fitting this project's rules forbid. `LightRamp::default()`
/// therefore keeps `overbright` at `1.0` (unchanged); `ohl-app`'s
/// `--overbright` flag exposes the documented 2x convention (or the round
/// 4 measured 1.7x ratio) as a user choice instead. See
/// `docs/FORMAT_SOURCES.md`, "Rendering conventions", for the full source
/// list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightRamp {
    /// The documented `texgamma` cvar (default `2.2`): the gamma space the
    /// diffuse texture samples are stored in, and therefore the space the
    /// ramp's output is rebased into.
    pub texgamma: f32,
    /// The documented `lightgamma` cvar (default `2.5`, matching the
    /// 1998-era default `gamma 2.5`): the gamma of the ramp applied to the
    /// compiled lightmap sample.
    pub lightgamma: f32,
    /// The documented `brightness` cvar, as an additive shift in `0.0..=1.0`
    /// of the ramped value (default `0.0`, i.e. no shift).
    pub brightness: f32,
    /// The documented overbright/lightscale multiplier applied to the ramped
    /// value before the clamp (default `1.0`, i.e. no overbright).
    pub overbright: f32,
}

impl Default for LightRamp {
    fn default() -> Self {
        Self {
            texgamma: 2.2,
            lightgamma: 2.5,
            brightness: 0.0,
            overbright: 1.0,
        }
    }
}

impl LightRamp {
    /// A ramp that leaves every sample untouched, for callers (and tests)
    /// that want the raw compiled luxels.
    #[must_use]
    pub fn identity() -> Self {
        Self {
            texgamma: 1.0,
            lightgamma: 1.0,
            brightness: 0.0,
            overbright: 1.0,
        }
    }

    /// Evaluates the ramp for one normalised sample in `0.0..=1.0`.
    #[must_use]
    pub fn evaluate(&self, sample: f32) -> f32 {
        let lightgamma = if self.lightgamma.is_finite() && self.lightgamma > 0.0 {
            self.lightgamma
        } else {
            1.0
        };
        let texgamma = if self.texgamma.is_finite() && self.texgamma > 0.0 {
            self.texgamma
        } else {
            1.0
        };
        let brightness = if self.brightness.is_finite() {
            self.brightness
        } else {
            0.0
        };
        let overbright = if self.overbright.is_finite() {
            self.overbright.max(0.0)
        } else {
            1.0
        };
        let display = sample.clamp(0.0, 1.0).powf(1.0 / lightgamma);
        let shifted = (display * overbright + brightness).clamp(0.0, 1.0);
        shifted.powf(lightgamma / texgamma).clamp(0.0, 1.0)
    }

    /// Bakes the ramp into a 256-entry lookup table over 8-bit code values.
    #[must_use]
    pub fn table(&self) -> LightRampTable {
        let mut entries = [0u8; 256];
        for (code, entry) in entries.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let sample = code as f32 / 255.0;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                *entry = (self.evaluate(sample) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
        LightRampTable { entries }
    }
}

/// A [`LightRamp`] baked into a 256-entry 8-bit lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightRampTable {
    entries: [u8; 256],
}

impl Default for LightRampTable {
    fn default() -> Self {
        LightRamp::default().table()
    }
}

impl LightRampTable {
    /// Maps one 8-bit lightmap code value through the ramp.
    #[must_use]
    pub fn apply(&self, code: u8) -> u8 {
        self.entries[code as usize]
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
    use super::{LUXEL_SIZE, LightRamp, ShelfPacker, lightmap_extents};

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
    fn default_ramp_fixes_the_endpoints_and_lifts_mid_tones() {
        let table = LightRamp::default().table();
        assert_eq!(table.apply(0), 0, "black stays black");
        assert_eq!(table.apply(255), 255, "a fully lit luxel stays fully lit");
        // A typical corridor luxel must land well above its raw code value:
        // the whole point of the ramp is that the 8-bit atlas spends more
        // than the ~20 code values a raw copy uses.
        let lifted = table.apply(64);
        assert!(
            (130..=142).contains(&lifted),
            "a 0x40 luxel ramps to about 2.1x, got {lifted}"
        );
        assert!(table.apply(128) > 180);
    }

    #[test]
    fn ramp_is_monotone_and_identity_is_a_no_op() {
        let table = LightRamp::default().table();
        let mut previous = 0;
        for code in 0..=255u8 {
            let value = table.apply(code);
            assert!(value >= previous, "the ramp never darkens as input rises");
            previous = value;
        }
        let identity = LightRamp::identity().table();
        for code in 0..=255u8 {
            assert_eq!(identity.apply(code), code);
        }
    }

    #[test]
    fn default_ramp_leaves_overbright_at_one() {
        // Fidelity round 4 (E5) found a real, publicly documented GoldSrc
        // "overbright" lightmap convention but no source pinning a default
        // multiplier value; the documented `LightRamp` default must stay
        // unchanged (`1.0`, i.e. no multiplier) so this stays a user
        // choice (`--overbright`), not a fitted default.
        assert!((LightRamp::default().overbright - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn overbright_two_matches_the_documented_full_bright_convention_at_code_128() {
        // The documented Quake-family overbright convention treats a
        // compiled lightmap value of 128 (of 255) as the "fully lit"
        // reference point and doubles it, saturating to the maximum
        // representable output (see `LightRamp`'s doc comment, "E5"). A
        // caller who opts into that convention via `overbright: 2.0` must
        // see exactly that saturation at code value 128.
        let table = LightRamp {
            overbright: 2.0,
            ..LightRamp::default()
        }
        .table();
        assert_eq!(
            table.apply(128),
            255,
            "overbright 2.0 must saturate the documented full-bright code 128 to white"
        );
    }

    #[test]
    fn brightness_and_overbright_shift_and_clamp() {
        let dark = LightRamp {
            overbright: 0.0,
            ..LightRamp::default()
        }
        .table();
        assert_eq!(dark.apply(200), 0, "a zero overbright blacks the ramp out");
        let bright = LightRamp {
            brightness: 1.0,
            ..LightRamp::default()
        }
        .table();
        assert_eq!(
            bright.apply(0),
            255,
            "the documented clamp caps the brightness shift at white"
        );
    }

    #[test]
    fn ramp_never_panics_on_degenerate_parameters() {
        for ramp in [
            LightRamp {
                texgamma: 0.0,
                lightgamma: -1.0,
                brightness: f32::NAN,
                overbright: f32::INFINITY,
            },
            LightRamp {
                texgamma: f32::NAN,
                lightgamma: f32::NAN,
                brightness: -10.0,
                overbright: -3.0,
            },
        ] {
            let table = ramp.table();
            for code in 0..=255u8 {
                let _ = table.apply(code);
            }
        }
    }

    #[test]
    fn packer_rejects_oversized_rectangles() {
        let mut packer = ShelfPacker::new(8, 8, 1);
        assert!(packer.insert(9, 1).is_none());
        assert!(packer.insert(1, 9).is_none());
    }
}
