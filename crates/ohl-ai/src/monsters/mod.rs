//! Per-monster definitions: package 7.7.
//!
//! - [`table`]: [`table::MonsterKind`], [`table::MonsterSpec`] and the
//!   black-box health/attack table, plus the `sk_<subject>_<property><N>`
//!   skill-table override hook and how each species' health answers each
//!   damage type ([`table::damage_response_for`]).
//! - [`brains`]: one data-driven [`crate::Brain`] ([`brains::MonsterBrain`])
//!   covering every defined kind, plus the new schedules and
//!   invented (black-box) tuning math (squad blast bonus, heal
//!   cooldown/threshold) no monster in the 7.5 default set needed.
//! - [`lifecycle`]: health intake from [`crate::DamageQueue`], the
//!   exactly-once [`crate::AiEventKind::Died`] guarantee, the corpse/gib
//!   decision, `Fade Corpse`, and `TriggerCondition`/`TriggerTarget`.
//! - [`integration`]: the minimal [`integration::Navigator`]/
//!   [`integration::RangedAttackSink`] seams for packages 7.6/7.3, which are
//!   being built concurrently, with dependency-free default
//!   implementations.
//! - [`nav_bridge`]: [`nav_bridge::NavBridge`], the real package 7.6
//!   (`ohl-nav`) implementation that fills the `Navigator` seam — attached
//!   via [`crate::world::AiWorld::attach_navigator`] — with its own richer,
//!   per-actor, per-hull, budget-bounded `next_move`.
//! - Wave 1 batch B, the bosses and aircraft: [`bigmomma`] (the Gonarch's
//!   `info_bigmomma` trail), [`nihilanth`] (the final boss's reserve,
//!   crystals and phases), [`aircraft`] (the Apache/Osprey flight plan),
//!   and [`bosses`], which attaches those components at spawn and drives
//!   them just before every ordinary think step.
//!
//! [`crate::spawner`] (at the crate root, not under this module) holds the
//! `monstermaker` spawn-count/delay/live-children bookkeeping; it is
//! monster-table-agnostic, so it did not need to live here.
//!
//! See the crate-level clean-room note and each submodule's own doc comment
//! for what is published behaviour versus this project's own placeholder.

pub mod aircraft;
pub mod bigmomma;
pub mod bosses;
pub mod brains;
pub mod integration;
pub mod lifecycle;
pub mod nav_bridge;
pub mod nihilanth;
pub mod table;

pub use aircraft::{FlightOrder, FlightPlan, FlightProgress, FlightStep};
pub use bigmomma::{GonarchTrail, Trail, TrailNode, TrailPhase};
pub use brains::MonsterBrain;
pub use integration::{Navigator, NoOpRangedAttackSink, RangedAttackSink, StraightLineNavigator};
pub use lifecycle::{
    CorpseDecision, MonsterTrigger, TriggerCondition, TriggerContext, apply_damage,
    apply_damage_effective, apply_damage_with_corpses, should_fade_corpse,
};
pub use nav_bridge::{Fallback, NavBridge, NavBridgeLimits, node_seeds_from_defs};
pub use nihilanth::{NihilanthCrystal, NihilanthPhase, NihilanthShield, ShieldProgress};
pub use table::{
    AttackSpec, BloodKind, Difficulty, MonsterFlags, MonsterKind, MonsterSpec, damage_response_for,
    spec_for,
};
