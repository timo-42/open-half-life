//! What the host feeds the player systems each tick.

use ohl_physics::{LiquidKind, MoveConfig, MoveEvents, Vec3, WaterLevel};

use crate::damage::{DamageKind, damage_kind_from_bits};

/// One damaging volume the player is inside this tick, straight from an
/// `ohl_game::TriggerHurt` component.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HurtInput {
    /// `dmg`: damage per second.
    pub damage_per_second: f32,
    /// `damagetype`, the documented additive bit field.
    pub damage_type: u32,
}

impl HurtInput {
    /// The hurt this `ohl_game` component describes.
    #[must_use]
    pub fn from_trigger_hurt(hurt: &ohl_game::TriggerHurt) -> Self {
        Self {
            damage_per_second: hurt.damage_per_second,
            damage_type: hurt.damage_type,
        }
    }

    /// This volume's damage kind.
    #[must_use]
    pub fn kind(&self) -> DamageKind {
        damage_kind_from_bits(self.damage_type)
    }
}

/// The most damaging volumes one tick considers. A map cannot stack more
/// than this many on the player without the extras simply being ignored,
/// which keeps the per-tick work bounded.
pub const MAX_HURT_VOLUMES: usize = 16;

/// The player's button state for this tick.
///
/// Movement keys are `ohl-physics`' business; this is the part the player
/// *systems* care about.
// Four independent key states; grouping them into a bit field would only
// make the call sites harder to read.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerInput {
    /// Whether the flashlight key went down this tick (an edge, not a
    /// hold): each `true` toggles the light.
    pub flashlight_pressed: bool,
    /// Whether the use key is held.
    pub use_held: bool,
    /// Whether the jump key is held.
    pub jump: bool,
    /// Whether the duck key is held.
    pub duck: bool,
    /// Every damaging volume the player is standing in, bounded to
    /// [`MAX_HURT_VOLUMES`] by [`Self::push_hurt`].
    pub hurt: Vec<HurtInput>,
}

impl PlayerInput {
    /// Adds a damaging volume, ignoring it once [`MAX_HURT_VOLUMES`] are
    /// already recorded.
    pub fn push_hurt(&mut self, hurt: HurtInput) {
        if self.hurt.len() < MAX_HURT_VOLUMES {
            self.hurt.push(hurt);
        }
    }

    /// Whether this tick holds the duck-and-jump combination the long jump
    /// module reacts to. Whether it is *early enough* in the crouch to be
    /// a long jump rather than a crouch jump is decided by `ohl-physics`,
    /// which owns the duck timer.
    #[must_use]
    pub fn is_long_jump_combo(&self) -> bool {
        self.jump && self.duck
    }
}

/// What the movement step produced this tick, in the form the player
/// systems consume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsOutput {
    /// The player's entity origin.
    pub origin: Vec3,
    /// The player's eye position.
    pub eye: Vec3,
    /// How deep in a liquid the player is.
    pub water_level: WaterLevel,
    /// Which liquid, if any.
    pub liquid: LiquidKind,
    /// Whether the player is standing on something.
    pub on_ground: bool,
    /// Whether the player is on a ladder.
    pub on_ladder: bool,
    /// The impact speed of a landing this tick, if there was one.
    pub landed_speed: Option<f32>,
    /// Whether a long jump fired this tick.
    pub long_jumped: bool,
}

impl Default for PhysicsOutput {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            eye: Vec3::ZERO,
            water_level: WaterLevel::Dry,
            liquid: LiquidKind::None,
            on_ground: true,
            on_ladder: false,
            landed_speed: None,
            long_jumped: false,
        }
    }
}

impl PhysicsOutput {
    /// Reads a movement state and the events its last step reported.
    #[must_use]
    pub fn from_move(
        state: &ohl_physics::PlayerState,
        config: &MoveConfig,
        events: &MoveEvents,
    ) -> Self {
        Self {
            origin: state.origin,
            eye: state.eye_position(config),
            water_level: state.water_level,
            liquid: state.liquid,
            on_ground: state.on_ground,
            on_ladder: state.on_ladder,
            landed_speed: events.landed_speed,
            long_jumped: events.long_jumped,
        }
    }

    /// Whether the player's head is under a liquid, which is what starts
    /// the air timer.
    #[must_use]
    pub fn is_submerged(&self) -> bool {
        self.water_level == WaterLevel::Eyes
    }
}

/// Anything that can answer "what contents value is at this point", so the
/// player systems can classify a hazard without depending on how the map is
/// stored. `ohl_physics::CollisionModel` implements it.
pub trait ContentsQuery {
    /// The contents value at `point`, one of `ohl_physics::contents`.
    fn point_contents(&self, point: Vec3) -> i32;
}

impl ContentsQuery for ohl_physics::CollisionModel {
    fn point_contents(&self, point: Vec3) -> i32 {
        Self::point_contents(self, point)
    }
}

/// A contents query for a world with nothing in it, useful in tests and as
/// a placeholder before a map is loaded.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyWorld;

impl ContentsQuery for EmptyWorld {
    fn point_contents(&self, _point: Vec3) -> i32 {
        ohl_physics::contents::EMPTY
    }
}
