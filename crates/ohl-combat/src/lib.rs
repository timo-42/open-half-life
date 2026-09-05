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

pub mod damage;
pub mod events;
pub mod trace;

pub use damage::{
    Armor, ArmorRule, DamageInfo, DamageOutcome, DamageType, Difficulty, DifficultyScale, Health,
    apply_damage,
};
pub use events::{CombatEvent, CombatEventQueue, SurfaceKind};
pub use trace::{
    AttackTrace, EntityHitboxes, EntityId, HitGroup, HitGroupScale, HitboxIndex, HitboxLimits,
    HitboxVolume, TraceFilter, TraceMask, trace_attack, trace_attack_filtered,
};

/// Re-exported so callers can use this crate's vector and rotation types
/// without also pinning `glam` themselves, matching `ohl_physics::Vec3`.
pub use glam::{Quat, Vec3};
