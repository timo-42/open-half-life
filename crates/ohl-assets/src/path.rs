//! Game-relative asset path normalization.
//!
//! Every path this crate ever turns into a filesystem access — an asset
//! path a caller asks [`crate::AssetFs::open`] for, a loose file found while
//! walking a search path, or a member name read out of a PAK directory —
//! goes through [`normalize`] first. It is the one place that decides which
//! strings become lookup keys, and it applies a deliberately strict,
//! platform-independent policy rather than trusting any single host's
//! rules:
//!
//! - both `/` and `\` are separators (GoldSrc PAK/WAD tooling and BSP
//!   `wad` keys mix both conventions);
//! - the whole path is bounded by [`crate::Limits::max_path_bytes`], each
//!   component by [`crate::Limits::max_component_bytes`], and the component
//!   count by [`crate::Limits::max_components`];
//! - no empty component (so no leading, trailing, or doubled separator
//!   survives), no `.`, no `..` — nothing here ever escapes the search root
//!   it is being resolved against;
//! - no control byte and no `:` (rejects a Windows drive letter or an
//!   alternate-data-stream suffix);
//! - no Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`9`,
//!   `LPT1`-`9`, matched case-insensitively against the whole component),
//!   since a component this crate accepts is later joined onto a real
//!   filesystem path and handed to the platform's file-open call.
//!
//! A normalized path keeps its original casing for filesystem lookups
//! (loose files) and display, plus an ASCII-lowercased `key` used for the
//! case-insensitive index: GoldSrc content is authored with inconsistent
//! casing and every supported host must resolve it the same way.

use crate::error::{AssetError, Result};
use crate::limits::Limits;

/// Windows reserved device stems, matched case-insensitively.
const RESERVED_STEMS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A validated, game-relative path: forward-slash-joined components with no
/// traversal, empty segment, or reserved name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NormalizedPath {
    display: String,
    key: String,
}

impl NormalizedPath {
    /// The normalized path with its original casing and forward slashes,
    /// suitable for joining onto a search-path directory.
    #[must_use]
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The ASCII-lowercased index lookup key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

fn is_reserved_stem(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    RESERVED_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

fn validate_component(component: &str, limits: &Limits) -> Result<()> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(AssetError::InvalidPath);
    }
    if component.len() > limits.max_component_bytes {
        return Err(AssetError::LimitExceeded);
    }
    if component
        .bytes()
        .any(|byte| byte < 0x20 || byte == b':' || byte == 0x7f)
    {
        return Err(AssetError::InvalidPath);
    }
    if is_reserved_stem(component) {
        return Err(AssetError::InvalidPath);
    }
    Ok(())
}

/// Normalizes a caller- or media-supplied game-relative path.
pub fn normalize(path: &str, limits: &Limits) -> Result<NormalizedPath> {
    if path.is_empty() || path.len() > limits.max_path_bytes {
        return Err(AssetError::InvalidPath);
    }

    let mut components: Vec<&str> = Vec::new();
    for raw in path.split(['/', '\\']) {
        if raw.is_empty() {
            // Tolerates a leading/trailing/doubled separator by simply
            // skipping it, rather than treating it as a distinct rooted
            // path: the result is always resolved as relative to a search
            // root, never as absolute.
            continue;
        }
        validate_component(raw, limits)?;
        components.push(raw);
    }

    if components.is_empty() || components.len() > limits.max_components {
        return Err(AssetError::InvalidPath);
    }

    let display = components.join("/");
    let key = display.to_ascii_lowercase();
    Ok(NormalizedPath { display, key })
}

/// Extracts the basename (final component) from a mapper-authored absolute
/// path, such as a worldspawn `wad` key's `\quake\hlwad\halflife.wad`, and
/// normalizes it on its own. The mapper's directories are always ignored:
/// only the filename is ever looked up, in the mod's own search path.
pub fn basename(path: &str, limits: &Limits) -> Result<NormalizedPath> {
    let trimmed = path.trim();
    let tail = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .ok_or(AssetError::InvalidPath)?;
    normalize(tail, limits)
}

#[cfg(test)]
mod tests {
    use super::{basename, normalize};
    use crate::limits::Limits;

    #[test]
    fn normalizes_mixed_separators_and_case() {
        let limits = Limits::default();
        let n = normalize("Sound\\Ambience\\HUM1.wav", &limits).unwrap();
        assert_eq!(n.display(), "Sound/Ambience/HUM1.wav");
        assert_eq!(n.key(), "sound/ambience/hum1.wav");
    }

    #[test]
    fn rejects_traversal_and_empty_paths() {
        let limits = Limits::default();
        assert!(normalize("../secret", &limits).is_err());
        assert!(normalize("maps/../../etc/passwd", &limits).is_err());
        assert!(normalize("", &limits).is_err());
        assert!(normalize("///", &limits).is_err());
    }

    #[test]
    fn rejects_drive_letters_and_reserved_names() {
        let limits = Limits::default();
        assert!(normalize("C:/Windows/system32", &limits).is_err());
        assert!(normalize("aux", &limits).is_err());
        assert!(normalize("maps/con.bsp", &limits).is_err());
        assert!(normalize("maps/CON", &limits).is_err());
    }

    #[test]
    fn basename_ignores_mapper_directories() {
        let limits = Limits::default();
        let n = basename("\\quake\\hlwad\\halflife.wad", &limits).unwrap();
        assert_eq!(n.display(), "halflife.wad");
        assert_eq!(n.key(), "halflife.wad");
    }

    #[test]
    fn basename_rejects_empty_input() {
        let limits = Limits::default();
        assert!(basename("", &limits).is_err());
        assert!(basename("///", &limits).is_err());
    }
}
