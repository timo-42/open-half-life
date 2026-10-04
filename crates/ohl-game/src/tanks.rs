//! Bounded turret definitions, map-use intent and shared brush/barrel pose.
//!
//! Published contract: TWHL `func_tank`, plus Sven's tank variant and controls
//! pages, recorded in FORMAT_SOURCES, "Turret definitions and simulation".
//! Defaults are from the published Sven mapping FGD; caps, local right-axis
//! sign and control release policy are project choices, TODO(black-box).
//! This module has no combat or renderer dependency.
//! It is deliberately not registered until the shared integration handoff.

use glam::{Quat, Vec3};
use hecs::Entity;

use crate::keyvalues::EntityDef;
use crate::registry::TriggerUse;

/// Published Active flag.
pub const ACTIVE: u32 = 1;
/// Published Only Direct flag.
pub const ONLY_DIRECT: u32 = 16;
/// Published Controllable flag.
pub const CONTROLLABLE: u32 = 32;
/// Project bound on a parsed target, master or presentation reference.
pub const MAX_REFERENCE_BYTES: usize = 256;
/// Project spatial bound; an absent/zero maximum range uses this ceiling.
pub const MAX_RANGE: f32 = 65_536.0;
/// Project upper bound on one authored damage/magnitude value.
pub const MAX_DAMAGE: f32 = 100_000.0;
/// Project bound on persistence after losing sight/range.
pub const MAX_PERSISTENCE: f32 = 60.0;
/// Project cap on scheduled shots per second, not a retail constant.
pub const MAX_FIRE_RATE: f32 = 100.0;

/// The four documented brush turret variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TankVariant {
    Bullets,
    Rocket,
    Laser,
    Mortar,
}

impl TankVariant {
    /// Recognizes only the original four tank classnames.
    #[must_use]
    pub fn from_classname(classname: &str) -> Option<Self> {
        Some(match classname {
            "func_tank" => Self::Bullets,
            "func_tankrocket" => Self::Rocket,
            "func_tanklaser" => Self::Laser,
            "func_tankmortar" => Self::Mortar,
            _ => return None,
        })
    }
}

/// TWHL's published `bullet` choices. None still allows shot target/presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TankBullet {
    None,
    NineMillimeter,
    Mp5,
    TwelveMillimeter,
}

/// Immutable authored configuration; live aim never changes the map transform.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TankDef {
    pub variant: TankVariant,
    /// Pitch, yaw, roll in degrees, with positive pitch looking down.
    pub authored_angles: Vec3,
    pub starts_active: bool,
    pub only_direct: bool,
    pub controllable: bool,
    pub yaw_rate: f32,
    pub yaw_range: f32,
    pub yaw_tolerance: f32,
    pub pitch_rate: f32,
    pub pitch_range: f32,
    pub pitch_tolerance: f32,
    /// Forward, right, up distances from the compiled origin-brush pivot.
    pub barrel: Vec3,
    pub fire_rate: f32,
    pub bullet: TankBullet,
    /// Explicit zero remains zero; no remembered retail damage defaults.
    pub damage: f32,
    /// Distinguishes an omitted rocket damage key from an authored zero. The
    /// runtime's omitted-rocket fallback is explicitly project policy.
    pub damage_key_present: bool,
    pub persistence: f32,
    /// Published ordinal 0..4; cone angles are engine policy, not sourced values.
    pub spread: u8,
    pub min_range: f32,
    pub max_range: f32,
    pub magnitude: f32,
    pub master: Option<String>,
    pub target: Option<String>,
    pub laser_entity: Option<String>,
    pub smoke_sprite: Option<String>,
    pub flash_sprite: Option<String>,
    pub sprite_scale: f32,
    pub rotate_sound: Option<String>,
}

impl TankDef {
    /// Reject malformed manually constructed/extension-restored definitions before
    /// they can reach trigonometry, clamps or combat command construction.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let bounded = |value: f32, max: f32| value.is_finite() && value >= 0.0 && value <= max;
        self.authored_angles.is_finite()
            && self.authored_angles.abs().cmple(Vec3::splat(180.0)).all()
            && self.barrel.is_finite()
            && self.barrel.abs().cmple(Vec3::splat(4096.0)).all()
            && bounded(self.yaw_rate, 720.0)
            && bounded(self.pitch_rate, 720.0)
            && bounded(self.yaw_range, 180.0)
            && bounded(self.pitch_range, 180.0)
            && bounded(self.yaw_tolerance, 180.0)
            && bounded(self.pitch_tolerance, 180.0)
            && bounded(self.fire_rate, MAX_FIRE_RATE)
            && bounded(self.damage, MAX_DAMAGE)
            && bounded(self.magnitude, MAX_DAMAGE)
            && bounded(self.persistence, MAX_PERSISTENCE)
            && bounded(self.min_range, self.max_range)
            && bounded(self.max_range, MAX_RANGE)
            && self.max_range > 0.0
            && self.spread <= 4
            && bounded(self.sprite_scale, 64.0)
            && [
                &self.master,
                &self.target,
                &self.laser_entity,
                &self.smoke_sprite,
                &self.flash_sprite,
                &self.rotate_sound,
            ]
            .into_iter()
            .flatten()
            .all(|name| name.len() <= MAX_REFERENCE_BYTES)
    }

    /// Parse finite values with explicit project caps. Missing tuning uses the
    /// published Sven mapping FGD defaults, not an original-build parity claim.
    /// TWHL independently documents base `bullet_damage` defaulting to zero.
    #[must_use]
    pub fn from_entity(entity: &EntityDef) -> Option<Self> {
        let variant = TankVariant::from_classname(&entity.classname)?;
        let max_range = number(entity, "maxRange", MAX_RANGE, 0.0, MAX_RANGE);
        let max_range = if max_range > 0.0 {
            max_range
        } else {
            MAX_RANGE
        };
        let persistence_key = if entity.keyvalues.contains_key("persistance") {
            "persistance"
        } else {
            "persistence"
        };
        Some(Self {
            variant,
            authored_angles: Vec3::new(
                wrap_degrees(entity.angles[0]),
                wrap_degrees(entity.angles[1]),
                wrap_degrees(entity.angles[2]),
            ),
            starts_active: entity.spawnflags & ACTIVE != 0,
            only_direct: entity.spawnflags & ONLY_DIRECT != 0,
            controllable: entity.spawnflags & CONTROLLABLE != 0,
            yaw_rate: number(entity, "yawrate", 30.0, 0.0, 720.0),
            yaw_range: number(entity, "yawrange", 180.0, 0.0, 180.0),
            yaw_tolerance: number(entity, "yawtolerance", 15.0, 0.0, 180.0),
            pitch_rate: number(entity, "pitchrate", 0.0, 0.0, 720.0),
            pitch_range: number(entity, "pitchrange", 0.0, 0.0, 180.0),
            pitch_tolerance: number(entity, "pitchtolerance", 5.0, 0.0, 180.0),
            barrel: Vec3::new(
                number(entity, "barrel", 0.0, -4096.0, 4096.0),
                number(entity, "barrely", 0.0, -4096.0, 4096.0),
                number(entity, "barrelz", 0.0, -4096.0, 4096.0),
            ),
            fire_rate: number(entity, "firerate", 1.0, 0.0, MAX_FIRE_RATE),
            bullet: match integer(entity, "bullet") {
                1 => TankBullet::NineMillimeter,
                2 => TankBullet::Mp5,
                3 => TankBullet::TwelveMillimeter,
                _ => TankBullet::None,
            },
            damage: number(entity, "bullet_damage", 0.0, 0.0, MAX_DAMAGE),
            damage_key_present: entity.keyvalues.contains_key("bullet_damage"),
            persistence: number(entity, persistence_key, 1.0, 0.0, MAX_PERSISTENCE),
            spread: u8::try_from(integer(entity, "firespread").clamp(0, 4)).unwrap_or(0),
            min_range: number(entity, "minRange", 0.0, 0.0, max_range),
            max_range,
            magnitude: number(
                entity,
                "iMagnitude",
                if entity.keyvalues.contains_key("iMagnitude") {
                    0.0
                } else {
                    100.0
                },
                0.0,
                MAX_DAMAGE,
            ),
            master: reference(entity.keyvalues.get("master").map(String::as_str)),
            target: reference(entity.target.as_deref()),
            laser_entity: reference(entity.keyvalues.get("laserentity").map(String::as_str)),
            smoke_sprite: reference(entity.keyvalues.get("spritesmoke").map(String::as_str)),
            flash_sprite: reference(entity.keyvalues.get("spriteflash").map(String::as_str)),
            sprite_scale: number(entity, "spritescale", 1.0, 0.0, 64.0),
            rotate_sound: reference(entity.keyvalues.get("rotatesound").map(String::as_str)),
        })
    }
}

/// An invisible controls brush names the controllable tank it operates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TankControls {
    pub target: String,
}

impl TankControls {
    #[must_use]
    pub fn from_entity(entity: &EntityDef) -> Option<Self> {
        if entity.classname != "func_tankcontrols" {
            return None;
        }
        Some(Self {
            target: reference(entity.target.as_deref())?,
        })
    }
}

/// Last player aim point observed in range with line of sight; no entity handles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TankMemory {
    pub point: Vec3,
    pub remaining: f32,
}

/// Per-tank continuation. No serde derive or old save layout is changed here.
/// Future optional tag 44 must bound and revalidate these values on restore.
#[derive(Debug, Clone, PartialEq)]
pub struct TankState {
    pub active: bool,
    /// Live pitch/yaw offsets from the authored angles, in degrees.
    pub relative_pitch: f32,
    pub relative_yaw: f32,
    /// Seconds until the next shot, retaining fractional phase without a backlog.
    pub shot_wait: f64,
    pub memory: Option<TankMemory>,
    pub rng: u64,
}

impl TankState {
    #[must_use]
    pub fn spawn(def: &TankDef, seed: u64) -> Self {
        Self {
            active: def.starts_active,
            relative_pitch: 0.0,
            relative_yaw: 0.0,
            shot_wait: 0.0,
            memory: None,
            rng: seed.max(1),
        }
    }

    /// The original game accepts a real player activator through a relay.
    /// Foreign/no activator only updates automatic on/off state; it cannot
    /// manufacture a local-player control claim. Master denial changes nothing.
    pub fn use_by(
        &mut self,
        def: &TankDef,
        tank: Entity,
        use_type: TriggerUse,
        activator: Option<Entity>,
        player: Entity,
        master_open: bool,
    ) -> Option<TankControlIntent> {
        if !master_open {
            return None;
        }
        if def.controllable && activator == Some(player) {
            return Some(TankControlIntent {
                tank,
                player,
                use_type,
            });
        }
        self.active = match use_type {
            TriggerUse::Off => false,
            TriggerUse::On => true,
            TriggerUse::Toggle => !self.active,
        };
        if !self.active {
            self.memory = None;
            self.shot_wait = self.shot_wait.max(0.0);
        }
        None
    }

    /// Reapply bounds at the runtime/save boundary; invalid memory is forgotten.
    pub fn sanitize(&mut self, def: &TankDef) {
        if !def.is_valid() {
            *self = Self {
                active: false,
                ..Self::spawn(def, 1)
            };
            return;
        }
        self.relative_pitch =
            finite(self.relative_pitch, 0.0).clamp(-def.pitch_range, def.pitch_range);
        self.relative_yaw = finite(self.relative_yaw, 0.0).clamp(-def.yaw_range, def.yaw_range);
        let interval = if def.fire_rate > 0.0 {
            1.0 / f64::from(def.fire_rate)
        } else {
            0.0
        };
        self.shot_wait = if self.shot_wait.is_finite() {
            self.shot_wait.clamp(0.0, interval)
        } else {
            0.0
        };
        self.memory = self
            .memory
            .filter(|m| {
                m.point.is_finite()
                    && m.remaining.is_finite()
                    && m.remaining > 0.0
                    && def.persistence > 0.0
            })
            .map(|m| TankMemory {
                remaining: m.remaining.min(def.persistence),
                ..m
            });
        self.rng = self.rng.max(1);
    }
}

/// Transient remote request, consumed before weapons on the following tick.
/// Runtime handles only; tag 44 must remap references rather than encode these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TankControlIntent {
    pub tank: Entity,
    pub player: Entity,
    pub use_type: TriggerUse,
}

/// One pose for geometry, collision, use center and barrel. Positive local
/// right points along -Y in the engine's +X-forward/+Z-up coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TankPose {
    pub rotation: Quat,
    pub placement_origin: Vec3,
    pub pivot_local: Vec3,
}

impl TankPose {
    /// `placement_origin` and `pivot_local` come from the common brush adapter.
    /// The authored angles are composed exactly once, never baked into Transform.
    #[must_use]
    pub fn new(
        def: &TankDef,
        state: &TankState,
        placement_origin: Vec3,
        pivot_local: Vec3,
    ) -> Option<Self> {
        if !placement_origin.is_finite()
            || !pivot_local.is_finite()
            || !(placement_origin + pivot_local).is_finite()
        {
            return None;
        }
        let pitch = (def.authored_angles.x + state.relative_pitch).to_radians();
        let yaw = (def.authored_angles.y + state.relative_yaw).to_radians();
        let roll = def.authored_angles.z.to_radians();
        let rotation =
            Quat::from_rotation_z(yaw) * Quat::from_rotation_y(pitch) * Quat::from_rotation_x(roll);
        rotation.is_finite().then_some(Self {
            rotation,
            placement_origin,
            pivot_local,
        })
    }

    #[must_use]
    pub fn pivot_world(self) -> Vec3 {
        self.placement_origin + self.pivot_local
    }

    #[must_use]
    pub fn world_point(self, compiled_point: Vec3) -> Vec3 {
        self.pivot_world() + self.rotation * (compiled_point - self.pivot_local)
    }

    #[must_use]
    pub fn muzzle(self, def: &TankDef) -> Vec3 {
        self.pivot_world() + self.rotation * Vec3::new(def.barrel.x, -def.barrel.y, def.barrel.z)
    }

    #[must_use]
    pub fn forward(self) -> Vec3 {
        self.rotation * Vec3::X
    }

    /// Direct input to the existing common `brush_pose_rotation` convention.
    #[must_use]
    pub fn axis_angle(self) -> (Vec3, f32, Vec3) {
        let (axis, angle) = self.rotation.to_axis_angle();
        (axis, angle.to_degrees(), self.pivot_local)
    }
}

/// Principal signed angle, with nonfinite input becoming a harmless zero.
#[must_use]
pub fn wrap_degrees(value: f32) -> f32 {
    (finite(value, 0.0) + 180.0).rem_euclid(360.0) - 180.0
}

fn finite(value: f32, default: f32) -> f32 {
    if value.is_finite() { value } else { default }
}

fn number(entity: &EntityDef, key: &str, default: f32, min: f32, max: f32) -> f32 {
    entity
        .keyvalues
        .get(key)
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
        .clamp(min, max)
}

fn integer(entity: &EntityDef, key: &str) -> i32 {
    entity
        .keyvalues
        .get(key)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn reference(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let mut end = value.len().min(MAX_REFERENCE_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].to_owned())
}

#[cfg(test)]
mod tests;
