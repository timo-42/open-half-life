//! Clean-room combat foundations for Open Half-Life.
//!
//! This crate is the M7 skeleton: the parts every weapon, monster attack and
//! environmental hazard needs before any of them exist.
//!
//! - [`damage`]: the damage-type vocabulary, a [`damage::DamageInfo`] record,
//!   [`damage::Health`]/[`damage::Armor`] components, and
//!   [`damage::apply_damage`], whose suit-absorption behaviour is supplied by
//!   the caller as an [`damage::ArmorRule`] rather than baked in.
//! - [`trace`]: [`trace::trace_attack`], which resolves a shot against world
//!   geometry (`ohl_physics`' point hull) and against a caller-supplied
//!   [`trace::HitboxIndex`] of posed studio hitboxes, returning the nearest
//!   impact and its hit group. [`trace::trace_attack_filtered`] additionally
//!   takes a [`trace::TraceFilter`] to skip an attack's owner (and its
//!   weapon entity) during hitbox refinement.
//! - [`events`]: a bounded [`events::CombatEventQueue`] the host application
//!   drains each tick to drive audio, HUD and effects. Presentation is pull
//!   only; this crate has no edge to `ohl-render`, `ohl-audio` or `ohl-ui`.
//! - [`ammo`]: [`ammo::AmmoType`], Half-Life's published ammunition classes
//!   with their published carry caps, and a bounded [`ammo::AmmoPool`].
//! - [`weapons`]: [`weapons::WeaponId`] and the [`weapons::spec`] table of
//!   [`weapons::WeaponSpec`]s — published damage, clip size and ammo type
//!   per weapon, with every unpublished number wrapped in
//!   [`weapons::BlackBox`] and marked `// TODO(black-box)`.
//! - [`firing`]: [`firing::FiringState`], the per-weapon firing state
//!   machine (`tick`, driven by [`firing::WeaponInput`], producing
//!   [`firing::WeaponAction`]s), and [`firing::resolve_hitscan`], which
//!   turns a hitscan action and its `trace_attack` results into
//!   [`damage::DamageInfo`] records.
//! - [`projectile`]: a bounded [`projectile::ProjectileSet`] of simulated
//!   projectiles — crossbow bolts, guided rockets, arcing grenades, homing
//!   hornets and hopping snarks — advanced at the fixed tick by swept hull-0
//!   traces against the world and the same [`trace::HitboxIndex`], so nothing
//!   ever tunnels, and reporting [`projectile::ProjectileEvent`]s.
//! - [`explosion`]: [`explosion::radius_damage`], linear-falloff blast damage
//!   with a documented line-of-sight rule, a self-damage hook and a pushback
//!   vector.
//! - [`deployables`]: [`deployables::DeployableSet`], satchel charges the
//!   owner sets off together and tripmines that arm after the published three
//!   seconds and then watch a beam along their own normal.
//! - [`inventory`]: [`inventory::Inventory`], the player's owned weapons,
//!   per-weapon clips, per-[`ammo::AmmoType`] pools, current selection and
//!   HUD slot layout (M7.4).
//! - [`pickups`]: [`pickups::classify_classname`], mapping published
//!   `weapon_*`/`ammo_*`/`item_*`/`func_healthcharger`/`func_recharge`
//!   classnames to a [`pickups::PickupKind`]; [`pickups::try_pickup`],
//!   which resolves a touch pickup against an [`inventory::Inventory`] and
//!   the target's health/armour; and [`pickups::ChargerState`], the
//!   use-and-hold health/suit charger model (M7.4).
//!
//! # No numbers we cannot cite
//!
//! Half-Life's *published* combat vocabulary (the damage-type names, the hit
//! group names, the player's 100 health and 100 armour maxima) is documented
//! on the public wikis cited in `docs/FORMAT_SOURCES.md` under "Combat and
//! damage". The *behavioural* values behind them — the HEV absorption split,
//! per-hit-group damage multipliers, per-difficulty scaling — are not
//! reliably published, so this crate does not invent them: every such value
//! is a field of a caller-supplied parameter struct whose `Default` is the
//! neutral, no-op value and whose documentation marks it **to be black-box
//! observed** against legally obtained retail software. Tests pass explicit
//! values instead of relying on the defaults.
//!
//! No Valve SDK source, decompiled binary or leaked material was consulted;
//! see `docs/CLEAN_ROOM.md`.
//!
//! Coordinates are GoldSrc world units, matching `ohl-world` and
//! `ohl-physics`.
#![forbid(unsafe_code)]

pub mod ammo;
pub mod damage;
pub mod deployables;
pub mod events;
pub mod explosion;
pub mod firing;
pub mod inventory;
pub mod pickups;
pub mod projectile;
pub mod trace;
pub mod weapons;

pub use ammo::{AmmoPool, AmmoType};
pub use damage::{
    Armor, ArmorRule, DamageInfo, DamageOutcome, DamageType, Difficulty, DifficultyScale, Health,
    apply_damage,
};
pub use deployables::{
    DeployableEvent, DeployableId, DeployableKind, DeployableSet, DeployableTuning, MAX_SATCHELS,
    MAX_TRIPMINES, Satchel, TRIPMINE_ARM_SECONDS, Tripmine,
};
pub use events::{CombatEvent, CombatEventQueue, SurfaceKind};
pub use explosion::{BlastHit, BlastTarget, ExplosionRule, radius_damage};
pub use firing::{
    FiringState, GAUSS_CHARGE_DAMAGE_RANGE, GAUSS_OVERCHARGE_SECONDS, GAUSS_OVERCHARGE_SELF_DAMAGE,
    Sequence, SoundKind, WeaponAction, WeaponInput, resolve_hitscan, resolve_hitscan_with_amount,
};
pub use inventory::{HudSlot, Inventory, hud_slot};
pub use pickups::{
    BATTERY_AMOUNT, CHARGER_DRAIN_RATE, ChargerState, HEALTH_CHARGER_TOTAL, HEALTHKIT_AMOUNT,
    PickupKind, PickupOutcome, SUIT_CHARGER_TOTAL_BY_DIFFICULTY, ammo_pickup_amount,
    classify_classname, try_pickup, weapon_pickup_ammo,
};
pub use projectile::{
    HAND_GRENADE_FUSE_SECONDS, Projectile, ProjectileEvent, ProjectileId, ProjectileKind,
    ProjectileLimits, ProjectileSet, ProjectileTuning, ProjectileWorld, SNARK_LIFETIME_SECONDS,
};
pub use trace::{
    AttackTrace, EntityHitboxes, EntityId, HitGroup, HitGroupScale, HitboxIndex, HitboxLimits,
    HitboxVolume, TraceFilter, TraceMask, trace_attack, trace_attack_filtered,
};
pub use weapons::{BlackBox, SecondaryFire, WeaponId, WeaponKind, WeaponSpec, spec};

/// Re-exported so callers can use this crate's vector and rotation types
/// without also pinning `glam` themselves, matching `ohl_physics::Vec3`.
pub use glam::{Quat, Vec3};
