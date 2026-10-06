//! Project-authored conversion between model anchors and centered BSP queries.
//!
//! Walking models keep their authored feet origin. BSP hulls and navigation
//! paths remain centered. Studio eyes, clipping bounds and bone hitboxes are
//! model-local data and never receive the collision translation. See
//! `docs/FORMAT_SOURCES.md`, "Monster authored anchors and body frames".

use glam::Vec3;
use ohl_physics::Hull;
use ohl_world::StudioModel;

use crate::monsters::MonsterKind;

/// A derived, nonserialized interpretation of an actor's authored anchor.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum BodyFrame {
    /// A walker's authored anchor is at the bottom of its collision proxy.
    #[default]
    Feet,
    /// Player controller or point mover: the query already is centered.
    Centered,
    /// A ceiling-hung model; the proxy ends at its authored anchor.
    Ceiling,
    /// Custom model hull minimum relative to its unmodified model pivot.
    ModelBottom(f32),
    /// Mounted/rooted model, with an optional valid model hull minimum.
    /// Without metadata its diagnostic proxy remains centered. This policy
    /// does not change the brain's or a script's authority to move it.
    FixedModelAnchor(Option<f32>),
}

impl BodyFrame {
    /// Translation from an authored anchor to a centered hull query.
    #[must_use]
    pub fn offset(self, hull: Hull) -> Vec3 {
        if hull == Hull::Point {
            return Vec3::ZERO;
        }
        let (min, max) = hull.bounds();
        let z = match self {
            Self::Feet => -min.z,
            Self::Centered | Self::FixedModelAnchor(None) => 0.0,
            Self::Ceiling => -max.z,
            Self::ModelBottom(bottom) | Self::FixedModelAnchor(Some(bottom)) => bottom - min.z,
        };
        Vec3::new(0.0, 0.0, z)
    }

    /// Converts a model anchor (or anchor goal) to a centered query point.
    #[must_use]
    pub fn anchor_to_query(self, hull: Hull, anchor: Vec3) -> Vec3 {
        anchor + self.offset(hull)
    }

    /// Converts a centered trace/navigation result back to a model anchor.
    #[must_use]
    pub fn query_to_anchor(self, hull: Hull, query: Vec3) -> Vec3 {
        query - self.offset(hull)
    }

    /// Bounds of the compiled collision proxy relative to the model anchor.
    #[must_use]
    pub fn local_bounds(self, hull: Hull) -> (Vec3, Vec3) {
        let (min, max) = hull.bounds();
        let offset = self.offset(hull);
        (min + offset, max + offset)
    }

    /// Axis-aligned world bounds of the selected compiled collision proxy.
    #[must_use]
    pub fn world_bounds(self, hull: Hull, anchor: Vec3) -> (Vec3, Vec3) {
        let (min, max) = self.local_bounds(hull);
        (anchor + min, anchor + max)
    }

    /// Derives the frame without changing the actor's stored position.
    #[must_use]
    pub fn for_model(kind: &MonsterKind, hull: Hull, model: Option<&StudioModel>) -> Self {
        if *kind == MonsterKind::Barnacle {
            return Self::Ceiling;
        }
        if hull == Hull::Point {
            return Self::Centered;
        }
        let bottom = model
            .and_then(|model| valid_bounds(model.hull_min, model.hull_max).map(|(min, _)| min.z));
        match kind {
            MonsterKind::Generic => Self::ModelBottom(bottom.unwrap_or(0.0)),
            MonsterKind::Furniture | MonsterKind::Tentacle | MonsterKind::Nihilanth => {
                Self::FixedModelAnchor(bottom)
            }
            _ => Self::Feet,
        }
    }

    /// Deterministic model-local eye policy, reconstructed on attachment.
    ///
    /// The header's usable eye wins. The clipping bounds then provide a
    /// project-authored fractional-height fallback; movement hull metadata
    /// never replaces valid model geometry. Exact species anatomy and custom
    /// pivots without usable metadata remain TODO(black-box).
    #[must_use]
    pub fn eye_offset(self, hull: Hull, model: Option<&StudioModel>) -> Vec3 {
        let ceiling = self == Self::Ceiling;
        if let Some(eye) = model.map(|model| Vec3::from_array(model.eye_position))
            && eye.is_finite()
            && eye.length_squared().is_finite()
            && eye.length_squared() > f32::EPSILON
            && (!ceiling || eye.z < 0.0)
        {
            return eye;
        }
        if ceiling {
            return Vec3::new(0.0, 0.0, -16.0);
        }
        if let Some((min, max)) =
            model.and_then(|model| valid_bounds(model.bounds_min, model.bounds_max))
        {
            return fractional_eye(min, max);
        }
        if hull == Hull::Point || matches!(self, Self::FixedModelAnchor(_)) {
            return Vec3::new(0.0, 0.0, 28.0);
        }
        let (min, max) = self.local_bounds(hull);
        fractional_eye(min, max)
    }
}

/// Validates metadata before subtraction so finite extremes cannot overflow.
#[must_use]
pub fn valid_bounds(min: [f32; 3], max: [f32; 3]) -> Option<(Vec3, Vec3)> {
    let (min, max) = (Vec3::from_array(min), Vec3::from_array(max));
    let span = max - min;
    (min.is_finite()
        && max.is_finite()
        && min.length_squared().is_finite()
        && max.length_squared().is_finite()
        && span.is_finite()
        && span.min_element() > 0.0)
        .then_some((min, max))
}

fn fractional_eye(min: Vec3, max: Vec3) -> Vec3 {
    let middle = min * 0.5 + max * 0.5;
    Vec3::new(middle.x, middle.y, min.z + (max.z - min.z) * (8.0 / 9.0))
}
