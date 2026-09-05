//! Liquid ("water") surface classification.
//!
//! GoldSrc's map-authoring convention marks a liquid surface by prefixing
//! its texture name with `!` (documented on the303.org's "GoldSrc Map
//! Texture Tutorial", part 6, "Water textures", and mirrored by the wider
//! mapping-community documentation this project's clean-room notes cite in
//! `docs/FORMAT_SOURCES.md`). The stock Half-Life content also ships liquid
//! families named `laser` and `water` without the `!` prefix (for example
//! `!lava`, `laser1`, `water1`); this crate treats a case-insensitive prefix
//! match on any of the three as a liquid surface.

/// Whether `name` (a BSP miptex or WAD3 texture name) names a liquid
/// surface that should render in the translucent, depth-write-disabled
/// water pass instead of the opaque world pass.
#[must_use]
pub fn is_liquid_texture(name: &str) -> bool {
    if name.starts_with('!') {
        return true;
    }
    let lower_len = name.len().min(16);
    let mut lower = [0u8; 16];
    for (dst, &src) in lower[..lower_len].iter_mut().zip(name.as_bytes()) {
        *dst = src.to_ascii_lowercase();
    }
    let lower = &lower[..lower_len];
    lower.starts_with(b"laser") || lower.starts_with(b"water")
}

#[cfg(test)]
mod tests {
    use super::is_liquid_texture;

    #[test]
    fn classifies_bang_prefixed_names() {
        assert!(is_liquid_texture("!water1"));
        assert!(is_liquid_texture("!LAVA"));
    }

    #[test]
    fn classifies_documented_unprefixed_families_case_insensitively() {
        assert!(is_liquid_texture("water1"));
        assert!(is_liquid_texture("WATER_BLUE"));
        assert!(is_liquid_texture("laser1"));
        assert!(is_liquid_texture("Laser_Red"));
    }

    #[test]
    fn rejects_ordinary_textures() {
        assert!(!is_liquid_texture("brick01"));
        assert!(!is_liquid_texture(""));
        assert!(!is_liquid_texture("{glass"));
    }
}
