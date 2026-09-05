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

impl RenderMode {
    /// The documented `rendermode` index for this mode.
    #[must_use]
    pub fn index(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Color => 1,
            Self::Texture => 2,
            Self::Glow => 3,
            Self::Solid => 4,
            Self::Additive => 5,
        }
    }

    /// The mode a `rendermode` keyvalue selects, or [`Self::Normal`] for any
    /// value outside the documented enum (the same fallback the documented
    /// default has: an entity with no `rendermode` key draws normally).
    #[must_use]
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Color,
            2 => Self::Texture,
            3 => Self::Glow,
            4 => Self::Solid,
            5 => Self::Additive,
            _ => Self::Normal,
        }
    }
}

impl RenderProps {
    /// Builds render properties from an entity's raw
    /// `rendermode`/`renderamt`/`rendercolor`/`renderfx` keyvalues, applying
    /// the documented defaults for absent or out-of-range values.
    ///
    /// `amount` is deliberately *ignored* for [`RenderMode::Normal`] and
    /// [`RenderMode::Solid`]: per the "Render Modes" article those modes draw
    /// opaque, and mappers routinely leave `renderamt` at its `0` default on
    /// a mode-0 entity. Taking `renderamt` verbatim there makes an ordinary
    /// opaque brush entity (a `func_train` car, say) render fully
    /// transparent, i.e. invisible.
    #[must_use]
    pub fn from_entity(mode: i32, amount: i32, color: [u8; 3], fx: i32) -> Self {
        let mode = RenderMode::from_index(mode);
        let amount = match mode {
            RenderMode::Normal | RenderMode::Solid => 255,
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            _ => amount.clamp(0, 255) as u8,
        };
        Self {
            mode,
            amount,
            color,
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            fx: fx.clamp(0, 255) as u8,
        }
    }

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
    fn mode_zero_ignores_renderamt() {
        // A mapper leaving `renderamt` at its 0 default on a mode-0 brush
        // entity must still draw opaque, not invisible.
        let props = RenderProps::from_entity(0, 0, [255, 255, 255], 0);
        assert_eq!(props.mode, RenderMode::Normal);
        assert_eq!(props.amount, 255);
        assert_eq!(props.alpha(), 1.0);
        assert_eq!(props.blend_kind(), BlendKind::Opaque);
        // The same holds for `kRenderTransAlpha`, which is also opaque.
        let props = RenderProps::from_entity(4, 0, [255, 255, 255], 0);
        assert_eq!(props.mode, RenderMode::Solid);
        assert_eq!(props.alpha(), 1.0);
    }

    #[test]
    fn from_entity_maps_modes_and_clamps_amount() {
        assert_eq!(RenderProps::from_entity(2, 128, [1, 2, 3], 7).amount, 128);
        assert_eq!(RenderProps::from_entity(2, -5, [1, 2, 3], 0).amount, 0);
        assert_eq!(RenderProps::from_entity(2, 900, [1, 2, 3], 0).amount, 255);
        // An unknown mode falls back to the documented default.
        assert_eq!(
            RenderProps::from_entity(99, 0, [1, 2, 3], 0).mode,
            RenderMode::Normal
        );
        for mode in [
            RenderMode::Normal,
            RenderMode::Color,
            RenderMode::Texture,
            RenderMode::Glow,
            RenderMode::Solid,
            RenderMode::Additive,
        ] {
            assert_eq!(RenderMode::from_index(i32::from(mode.index())), mode);
        }
    }

    #[test]
    fn default_is_normal_and_fully_opaque() {
        let props = RenderProps::default();
        assert_eq!(props.mode, RenderMode::Normal);
        assert_eq!(props.amount, 255);
        assert_eq!(props.alpha(), 1.0);
    }
}
