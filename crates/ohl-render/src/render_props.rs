//! Brush-entity and studio-model render-mode semantics.
//!
//! GoldSrc entities carry `rendermode`/`renderamt`/`rendercolor`/`renderfx`
//! keys that select one of a fixed set of blending behaviours, documented on
//! the Valve Developer Community's "Render modes" article (see
//! `docs/FORMAT_SOURCES.md`, "Rendering conventions").

/// The documented `rendermode` values this renderer implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// `kRenderNormal`: opaque, depth-written, unmodified colour.
    #[default]
    Normal,
    /// `kRenderTransColor`: `rendercolor` tinted, alpha-blended by
    /// `renderamt`, ignoring the entity's own texture colour.
    Color,
    /// `kRenderTransTexture`: the entity's own texture, alpha-blended by
    /// `renderamt`.
    Texture,
    /// `kRenderGlow`: additive, unaffected by depth (treated the same as
    /// [`Self::Additive`] at this milestone; see the module docs).
    Glow,
    /// `kRenderTransAlpha`: opaque and depth-written, but alpha-tested
    /// against a masked (`{`-prefixed) texture's alpha channel.
    Solid,
    /// `kRenderTransAdd`: additive, not depth-written.
    Additive,
}

/// The blend family a [`RenderMode`] maps to, i.e. which of a renderer's
/// small set of precompiled pipelines it draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendKind {
    /// Depth-written, no colour blending (masked-texture alpha-testing still
    /// applies, as it does for ordinary world/brush geometry).
    Opaque,
    /// Standard "over" alpha blending, not depth-written.
    AlphaBlend,
    /// Additive blending, not depth-written.
    Additive,
}

/// Render-mode parameters for one brush entity or studio model instance,
/// mirroring the documented `rendermode`/`renderamt`/`rendercolor`/
/// `renderfx` entity keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderProps {
    /// The `rendermode` value.
    pub mode: RenderMode,
    /// The `renderamt` value (`0..=255`); read as alpha/blend strength by
    /// every mode except [`RenderMode::Normal`] and [`RenderMode::Solid`].
    pub amount: u8,
    /// The `rendercolor` value, used only by [`RenderMode::Color`].
    pub color: [u8; 3],
    /// The `renderfx` value. Not yet interpreted by this renderer (no
    /// documented `renderfx` animation is implemented at this milestone);
    /// carried through so a future milestone does not need an API change.
    pub fx: u8,
}

impl Default for RenderProps {
    fn default() -> Self {
        Self {
            mode: RenderMode::Normal,
            amount: 255,
            color: [255, 255, 255],
            fx: 0,
        }
    }
}

impl RenderProps {
    /// Which precompiled pipeline family this mode draws with.
    #[must_use]
    pub fn blend_kind(self) -> BlendKind {
        match self.mode {
            RenderMode::Normal | RenderMode::Solid => BlendKind::Opaque,
            RenderMode::Color | RenderMode::Texture => BlendKind::AlphaBlend,
            RenderMode::Glow | RenderMode::Additive => BlendKind::Additive,
        }
    }

    /// The effective alpha (`0.0..=1.0`) this mode applies, for the shader
    /// uniform: [`RenderMode::Normal`] and [`RenderMode::Solid`] always
    /// draw fully opaque (their translucency, if any, comes from a masked
    /// texture's own alpha channel instead), and every other mode scales by
    /// [`Self::amount`].
    #[must_use]
    pub fn alpha(self) -> f32 {
        match self.mode {
            RenderMode::Normal | RenderMode::Solid => 1.0,
            _ => f32::from(self.amount) / 255.0,
        }
    }

    /// Whether the fragment shader should substitute [`Self::color`] for the
    /// texture's own colour ([`RenderMode::Color`] only).
    #[must_use]
    pub fn uses_render_color(self) -> bool {
        self.mode == RenderMode::Color
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{BlendKind, RenderMode, RenderProps};

    #[test]
    fn each_mode_maps_to_the_documented_blend_family() {
        let cases = [
            (RenderMode::Normal, BlendKind::Opaque),
            (RenderMode::Solid, BlendKind::Opaque),
            (RenderMode::Color, BlendKind::AlphaBlend),
            (RenderMode::Texture, BlendKind::AlphaBlend),
            (RenderMode::Glow, BlendKind::Additive),
            (RenderMode::Additive, BlendKind::Additive),
        ];
        for (mode, expected) in cases {
            let props = RenderProps {
                mode,
                ..RenderProps::default()
            };
            assert_eq!(props.blend_kind(), expected);
        }
    }

    #[test]
    fn normal_and_solid_are_always_fully_opaque() {
        let props = RenderProps {
            mode: RenderMode::Normal,
            amount: 40,
            ..RenderProps::default()
        };
        assert_eq!(props.alpha(), 1.0);
        let props = RenderProps {
            mode: RenderMode::Solid,
            amount: 40,
            ..RenderProps::default()
        };
        assert_eq!(props.alpha(), 1.0);
    }

    #[test]
    fn other_modes_scale_alpha_by_amount() {
        let props = RenderProps {
            mode: RenderMode::Texture,
            amount: 128,
            ..RenderProps::default()
        };
        assert!((props.alpha() - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn only_color_mode_substitutes_render_color() {
        assert!(
            RenderProps {
                mode: RenderMode::Color,
                ..RenderProps::default()
            }
            .uses_render_color()
        );
        assert!(
            !RenderProps {
                mode: RenderMode::Texture,
                ..RenderProps::default()
            }
            .uses_render_color()
        );
    }

    #[test]
    fn default_is_normal_and_fully_opaque() {
        let props = RenderProps::default();
        assert_eq!(props.mode, RenderMode::Normal);
        assert_eq!(props.amount, 255);
        assert_eq!(props.alpha(), 1.0);
    }
}
