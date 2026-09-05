//! Light-style intensity evaluation.
//!
//! GoldSrc (following id Software's Quake convention) drives each light
//! style from a pattern string of `a`..`z` characters, where `a` is fully
//! dark, `m` is the compiled ("normal") brightness, and `z` is double
//! brightness; the pattern is stepped at a fixed 10 Hz. See
//! `docs/FORMAT_SOURCES.md`, "Rendering conventions", for the citations.

/// The number of light-style slots this table holds. BSP30's `Face::styles`
/// entries are single bytes, but real content only uses a small prefix of
/// that range; this bound is generous while staying cheap to allocate.
pub const MAX_LIGHT_STYLES: usize = 64;

/// How many pattern characters a light style advances through per second.
pub const STYLE_HZ: f32 = 10.0;

/// The style id meaning "this face does not use this style slot" (BSP30's
/// `Face::styles` sentinel).
pub const STYLE_NONE: u8 = 0xFF;

/// Maps one documented pattern character (`a`..`z`, case-insensitive) to an
/// intensity in `0.0..=2.0`.
///
/// Any other byte (a malformed or empty pattern) reads as `1.0`, the neutral
/// "compiled brightness" value, so an unrecognised pattern dims or brightens
/// nothing rather than turning a surface black.
#[must_use]
pub fn char_intensity(c: u8) -> f32 {
    let lower = c.to_ascii_lowercase();
    if !lower.is_ascii_lowercase() {
        return 1.0;
    }
    f32::from(lower - b'a') * (2.0 / 25.0)
}

/// A table of `a`..`z` pattern strings, one per light-style id, evaluated at
/// a caller-supplied time to drive [`ohl_world::WorldModel::blend_lightmap`].
pub struct LightStyles {
    patterns: Vec<String>,
}

impl Default for LightStyles {
    fn default() -> Self {
        let mut patterns = vec![String::new(); MAX_LIGHT_STYLES];
        // Style 0's documented default is the constant pattern "m" (normal,
        // unmodulated brightness).
        if let Some(style0) = patterns.first_mut() {
            style0.push('m');
        }
        Self { patterns }
    }
}

impl LightStyles {
    /// A table with only style 0 set (to the documented constant "m"),
    /// matching the brightness every face already renders at before any
    /// style animation is configured.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `style`'s pattern. Out-of-range style ids are silently ignored:
    /// a face referencing an unconfigured style already reads back as
    /// neutral (`1.0`) from [`Self::intensity`].
    pub fn set_pattern(&mut self, style: u8, pattern: &str) {
        if let Some(slot) = self.patterns.get_mut(usize::from(style)) {
            slot.clear();
            slot.push_str(pattern);
        }
    }

    /// This style's intensity at `time_seconds`, stepping through its
    /// pattern at [`STYLE_HZ`].
    ///
    /// [`STYLE_NONE`] and any style id with no (or an empty) configured
    /// pattern both read as `1.0`, the neutral compiled brightness, so an
    /// unconfigured style never goes dark.
    #[must_use]
    pub fn intensity(&self, style: u8, time_seconds: f32) -> f32 {
        if style == STYLE_NONE {
            return 1.0;
        }
        let Some(pattern) = self.patterns.get(usize::from(style)) else {
            return 1.0;
        };
        if pattern.is_empty() {
            return 1.0;
        }
        let bytes = pattern.as_bytes();
        let elapsed = if time_seconds.is_finite() {
            time_seconds.max(0.0)
        } else {
            0.0
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let step = (elapsed * STYLE_HZ) as usize % bytes.len();
        char_intensity(bytes[step])
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{LightStyles, STYLE_NONE, char_intensity};

    #[test]
    fn char_intensity_matches_the_documented_endpoints() {
        assert_eq!(char_intensity(b'a'), 0.0);
        assert_eq!(char_intensity(b'z'), 2.0);
        // `m` is documented as the "normal" brightness; the linear a..z
        // mapping puts it close to, but not required to be exactly, 1.0.
        assert!((char_intensity(b'm') - 0.96).abs() < 1e-6);
        assert_eq!(char_intensity(b'A'), char_intensity(b'a'));
    }

    #[test]
    fn non_letters_are_neutral() {
        assert_eq!(char_intensity(b'0'), 1.0);
        assert_eq!(char_intensity(b' '), 1.0);
    }

    #[test]
    fn default_table_only_configures_style_zero() {
        let styles = LightStyles::new();
        assert!((styles.intensity(0, 0.0) - char_intensity(b'm')).abs() < 1e-6);
        // An unconfigured style, and the "unused" sentinel, both read
        // neutral rather than dark.
        assert_eq!(styles.intensity(5, 0.0), 1.0);
        assert_eq!(styles.intensity(STYLE_NONE, 0.0), 1.0);
    }

    #[test]
    fn pattern_cycles_at_ten_hertz() {
        let mut styles = LightStyles::new();
        styles.set_pattern(1, "az");
        assert_eq!(styles.intensity(1, 0.0), char_intensity(b'a'));
        assert_eq!(styles.intensity(1, 0.1), char_intensity(b'z'));
        // One full cycle (0.2s at 10 Hz for a 2-character pattern) returns
        // to the start.
        assert_eq!(styles.intensity(1, 0.2), char_intensity(b'a'));
        // Many cycles later, still in range.
        assert_eq!(styles.intensity(1, 123.1), char_intensity(b'z'));
    }

    #[test]
    fn never_panics_on_non_finite_time_or_out_of_range_style() {
        let styles = LightStyles::new();
        let _ = styles.intensity(0, f32::NAN);
        let _ = styles.intensity(0, f32::NEG_INFINITY);
        let _ = styles.intensity(255, 0.0);
    }

    #[test]
    fn set_pattern_on_an_out_of_range_style_is_ignored_not_panicking() {
        let mut styles = LightStyles::new();
        styles.set_pattern(255, "az");
        assert_eq!(styles.intensity(255, 0.1), 1.0);
    }
}
