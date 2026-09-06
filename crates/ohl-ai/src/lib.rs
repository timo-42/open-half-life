//! Clean-room monster AI core for Open Half-Life.
//!
//! This crate is the decision-making half of the monsters: what they
//! perceive, what state that puts them in, which schedule of tasks they run,
//! how squads share an enemy, and the movement glue that turns a task into
//! clip-hull-traced motion. It renders nothing, plays nothing, and resolves
//! no damage — [`world::AiEvent`]s are pushed out for `ohl-app` and, later,
//! `ohl-combat` to act on.
//!
//! ## Layout
//!
//! - [`state`]: [`MonsterState`], [`Classification`], [`Relationship`], the
//!   data-driven [`RelationshipTable`], and the [`Conditions`] bitset.
//! - [`senses`]: [`senses::look`] and [`senses::listen`], the [`Senses`]
//!   parameters, the bounded [`SoundList`], enemy selection and
//!   [`EnemyMemory`].
//! - [`schedule`]: [`Task`], [`Schedule`], [`ScheduleRunner`] and the
//!   [`Brain`] trait.
//! - [`brain`]: this project's own default schedule set and state machine.
//! - [`movement`]: [`Route`], [`movement::move_toward`] and
//!   [`StuckDetector`].
//! - [`squad`]: [`SquadRoster`], leader recruitment and enemy sharing.
//! - [`scripts`]: the `scripted_sequence`/`aiscripted_sequence` state
//!   machine and the [`ScriptHold`] marker that suspends a monster's brain
//!   while a script owns it.
//! - [`follow`]: the `monster_scientist`/`monster_barney` follow layer.
//! - [`spawn`]: attaching AI components to `ohl-game`'s entity registry.
//! - [`damage`]: the minimal damage input, to be replaced by `ohl-combat`'s
//!   richer `DamageInfo` (see the module docs).
//! - [`monsters`]: package 7.7's per-monster `MonsterKind`/`MonsterSpec`
//!   table, `MonsterBrain` and lifecycle (health intake, death, corpse/gib,
//!   `TriggerCondition`).
//! - [`spawner`]: `Spawner`, the `monstermaker` spawn-count/delay/
//!   live-children bookkeeping.
//! - [`rng`]: a project-owned seeded [`Pcg32`].
//! - [`world`]: [`AiWorld`], the deterministic fixed-tick simulation over a
//!   [`hecs::World`].
//!
//! ## Clean room
//!
//! Only the *behavioural vocabulary* is borrowed, from the public TWHL wiki
//! "Monsters Programming" concept pages recorded in `docs/FORMAT_SOURCES.md`
//! under "Monster AI behaviour": the state and condition names, the
//! relationship values, sight originating at `origin + view_ofs` through a
//! view cone, enemy choice by relationship then distance, an occluded enemy
//! inside 256 units staying tracked, a route refreshing when its goal moves
//! more than 80 units, a hearing-sensitivity multiplier (2 for the
//! tentacle), and a squad leader recruiting up to three members.
//!
//! **The task set, every schedule, every interrupt mask, the runner, the
//! bit layout and every numeric default here are this project's own.** No
//! SDK source, schedule table or decompilation was consulted; see
//! `docs/CLEAN_ROOM.md`. Values that no public page gives — view cone
//! angles, look distances, movement speeds, turn rates, attack ranges,
//! damage thresholds — are placeholders marked in their documentation as
//! black-box observations still to be made.
//!
//! ## Determinism
//!
//! [`AiWorld::tick`] processes entities in ascending [`hecs::Entity::id`]
//! order over a sensory snapshot taken before anything moves, and draws
//! every random number from one seeded [`Pcg32`]. [`AiWorld::state_hash`]
//! digests the whole simulation so a replay test can assert that a fixed
//! seed reproduces a run exactly.
#![forbid(unsafe_code)]

pub mod brain;
pub mod damage;
pub mod follow;
pub mod monsters;
pub mod movement;
pub mod rng;
pub mod schedule;
pub mod scripts;
pub mod senses;
pub mod spawn;
pub mod spawner;
pub mod squad;
pub mod state;
pub mod world;

pub use brain::{DefaultBrain, default_next_state, schedule_by_name};
pub use damage::{DamageEvent, DamageQueue, DamageSink};
pub use follow::{FollowChange, FollowRoster, Follower};
pub use monsters::{
    CorpseDecision, MonsterBrain, MonsterKind, MonsterSpec, MonsterTrigger, NavBridge,
    NavBridgeLimits, Navigator, NoOpRangedAttackSink, RangedAttackSink, StraightLineNavigator,
    TriggerCondition, TriggerContext, apply_damage as apply_monster_damage, node_seeds_from_defs,
};
pub use movement::{MoveResult, Route, StuckDetector, move_toward};
pub use rng::Pcg32;
pub use schedule::{
    Activity, Brain, RunOutcome, Schedule, ScheduleRunner, Task, TaskExecutor, TaskStatus,
};
pub use scripts::{ScriptAction, ScriptHold, ScriptPhase, ScriptRunner, ScriptSense, ScriptStep};
pub use senses::{
    Candidate, EnemyMemory, ListenResult, LookResult, Senses, SightContext, Sighting, SoundEvent,
    SoundKind, SoundList, Viewer, listen, look,
};
pub use spawn::{MonsterSpawn, MonsterSpawnRules, attach_monsters};
pub use spawner::Spawner;
pub use squad::{MAX_RECRUITS, MAX_SQUAD_SIZE, Squad, SquadCandidate, SquadRoster};
pub use state::{
    CLASSIFICATION_COUNT, Classification, Conditions, MonsterState, Relationship, RelationshipTable,
};
pub use world::{
    Actor, AiEvent, AiEventKind, AiWorld, AttackKind, BrainId, MonsterAi, SquadTag, spawn_actor,
    spawn_monster, spawn_squad_monster,
};

/// Re-exported so callers can use this crate's vector type without pinning
/// `glam` themselves, matching `ohl-physics`.
pub use glam::Vec3;
