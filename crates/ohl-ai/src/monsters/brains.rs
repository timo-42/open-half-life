//! Per-monster [`Brain`] implementations.
//!
//! One [`MonsterBrain`] type, parameterized by [`MonsterKind`] and
//! [`MonsterSpec`], covers every defined monster: the *data* (health,
//! attack reach, senses) comes from [`crate::monsters::table`], and the
//! *behaviour* — which schedule each kind runs in combat, and the handful of
//! schedules no monster in package 7.5's default set needed — is switched on
//! `kind`.
//!
//! ## Clean room
//!
//! The schedules below are entirely project-authored, exactly like
//! `crate::brain`'s default set: no SDK schedule/task table was consulted.
//! They were written from the public *descriptions* of what each monster is
//! known to do — a houndeye's blast attack, a bullsquid's ranged spit, an
//! alien slave's zap, a grunt squad's suppress/flank/grenade behaviour, a
//! barney/scientist following the player, a scientist healing a hurt ally, a
//! sentry turret deploying/retracting/tracking, a tentacle striking toward a
//! sound, a gargantua's flame/stomp attacks, a barnacle waiting under a
//! ceiling for something to touch its tongue, an alien controller firing
//! and repositioning in the air, an assassin firing and relocating, a
//! scripted prop standing where it was put, and a critter wandering — see
//! `docs/FORMAT_SOURCES.md`, "Monster definitions". The houndeye
//! squad-blast-bonus formula's *existence* is published (squads bonus a
//! blast, capped, halved without line of sight) and its numbers are cited
//! there too, as are the scientist's heal amount/cooldown/range/threshold;
//! attack reach/range and every schedule's own timing remain this project's
//! own **`TODO(black-box)`** placeholders. Nothing here reproduces a
//! decompiled schedule or AI routine.

use crate::schedule::{Activity, Brain, Schedule, Task};
use crate::senses::{Senses, TENTACLE_HEARING_SENSITIVITY};
use crate::state::{Classification, Conditions, MonsterState};

use super::table::{MonsterFlags, MonsterKind, MonsterSpec, spec_for};

// --- New, own-authored schedules -------------------------------------------

/// A pack member fires its blast attack; houndeyes are documented as pack
/// hunters, so [`houndeye_pack_bonus`] scales the resulting damage by squad
/// size rather than this schedule doing anything squad-aware itself.
pub static HOUNDEYE_PACK_BLAST: Schedule = Schedule::new(
    "ohl/monsters/houndeye_pack_blast",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Threat),
        Task::RangeAttack1,
        Task::Wait(0.4),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A bullsquid's ranged spit attack.
pub static BULLSQUID_SPIT: Schedule = Schedule::new(
    "ohl/monsters/bullsquid_spit",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(0.6),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// An alien slave's zap (hivehand-style) ranged attack.
pub static SLAVE_ZAP: Schedule = Schedule::new(
    "ohl/monsters/slave_zap",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::Wait(0.3),
        Task::RangeAttack1,
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A grunt lays down suppressing fire without closing distance.
pub static GRUNT_SUPPRESS: Schedule = Schedule::new(
    "ohl/monsters/grunt_suppress",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::RangeAttack1,
        Task::Wait(0.2),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::NO_AMMO_LOADED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A grunt repositions to a flanking spot before re-engaging.
pub static GRUNT_FLANK: Schedule = Schedule::new(
    "ohl/monsters/grunt_flank",
    &[
        Task::SetActivity(Activity::Run),
        Task::FindCover,
        Task::TakeCover,
        Task::RunPath,
        Task::WaitForMovement,
        Task::FaceEnemy,
    ],
    Conditions::GENERAL_INTERRUPTS.union(Conditions::HEAVY_DAMAGE),
);

/// A grunt throws its secondary (grenade) attack.
pub static GRUNT_GRENADE: Schedule = Schedule::new(
    "ohl/monsters/grunt_grenade",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Threat),
        Task::RangeAttack2,
        Task::Wait(1.0),
    ],
    Conditions::GENERAL_INTERRUPTS,
);

/// Project-authored secondary schedule; stable name is save-compatible.
pub static ASSASSIN_GRENADE: Schedule = Schedule::new(
    "ohl/monsters/assassin_grenade",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::RangeAttack2,
        Task::Wait(1.0),
    ],
    Conditions::GENERAL_INTERRUPTS.union(Conditions::ENEMY_OCCLUDED),
);

/// Project-authored secondary schedule; stable name is save-compatible.
pub static CONTROLLER_HEAD_BALL: Schedule = Schedule::new(
    "ohl/monsters/controller_head_ball",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::RangeAttack2,
        Task::Wait(1.0),
    ],
    Conditions::GENERAL_INTERRUPTS.union(Conditions::ENEMY_OCCLUDED),
);

/// Project-authored secondary schedule; stable name is save-compatible.
pub static APACHE_ROCKET: Schedule = Schedule::new(
    "ohl/monsters/apache_rocket",
    &[Task::FaceEnemy, Task::RangeAttack2, Task::Wait(1.0)],
    Conditions::GENERAL_INTERRUPTS.union(Conditions::ENEMY_OCCLUDED),
);

/// Barney/scientist walking after the player they are following.
pub static FOLLOW_PLAYER: Schedule = Schedule::new(
    "ohl/monsters/follow_player",
    &[
        Task::SetActivity(Activity::Walk),
        Task::MoveToTarget { within: 96.0 },
        Task::WalkPath,
        Task::WaitForMovement,
        Task::FaceTarget,
    ],
    Conditions::ALL_ATTACK
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A scientist administers first aid to a hurt ally (`SPECIAL1`, set by
/// [`scientist_heal_ready`]).
pub static SCIENTIST_HEAL: Schedule = Schedule::new(
    "ohl/monsters/scientist_heal",
    &[
        Task::StopMoving,
        Task::FaceTarget,
        Task::SetActivity(Activity::Melee),
        Task::MeleeAttack2,
        Task::Wait(1.0),
    ],
    Conditions::GENERAL_INTERRUPTS.union(Conditions::HEAR_DANGER),
);

/// A turret racks up out of its housing.
pub static TURRET_DEPLOY: Schedule = Schedule::new(
    "ohl/monsters/turret_deploy",
    &[
        Task::PlaySequence("turret_deploy"),
        Task::SetActivity(Activity::Alert),
    ],
    Conditions::EMPTY,
);

/// A turret racks back down into its housing.
pub static TURRET_RETRACT: Schedule = Schedule::new(
    "ohl/monsters/turret_retract",
    &[
        Task::PlaySequence("turret_retract"),
        Task::SetActivity(Activity::Idle),
        Task::WaitRandom { min: 1.0, max: 3.0 },
    ],
    Conditions::ALL_SIGHT.union(Conditions::ALL_SOUND),
);

/// A turret tracks and fires on its acquired enemy without ever moving.
pub static TURRET_TRACK: Schedule = Schedule::new(
    "ohl/monsters/turret_track",
    &[
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
    ],
    Conditions::ENEMY_DEAD.union(Conditions::GENERAL_INTERRUPTS),
);

/// A tentacle strikes toward the loudest recent sound rather than a seen
/// enemy — published behaviour (it has no eyes) — using the last noise
/// position left in `move_target` by [`crate::senses::listen`].
pub static TENTACLE_STRIKE: Schedule = Schedule::new(
    "ohl/monsters/tentacle_strike",
    &[
        Task::FaceTarget,
        Task::SetActivity(Activity::Melee),
        Task::MeleeAttack1,
        Task::Wait(0.5),
    ],
    Conditions::GENERAL_INTERRUPTS,
);

/// A rooted monster (tentacle) waiting for a sound to react to.
pub static ROOTED_LISTEN: Schedule = Schedule::new(
    "ohl/monsters/rooted_listen",
    &[
        Task::SetActivity(Activity::Idle),
        Task::StopMoving,
        Task::Wait(0.5),
    ],
    Conditions::ALL_SOUND.union(Conditions::ALL_DAMAGE),
);

/// A gargantua's flame-thrower sweep.
pub static GARG_FLAME: Schedule = Schedule::new(
    "ohl/monsters/garg_flame",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(1.0),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A gargantua's ground-stomp shockwave, a scripted-set-piece placeholder
/// (see the crate doc comment): the real game ties this to specific map
/// triggers this crate does not yet model.
pub static GARG_STOMP: Schedule = Schedule::new(
    "ohl/monsters/garg_stomp",
    &[
        Task::StopMoving,
        Task::SetActivity(Activity::Threat),
        Task::MeleeAttack2,
        Task::Wait(1.5),
    ],
    Conditions::GENERAL_INTERRUPTS,
);

// --- Wave 1 batch A --------------------------------------------------------

/// A barnacle waiting, mouth down, for something to touch its tongue: the
/// only thing that ends the wait is a sighting (the tongue test is
/// [`MonsterBrain::melee_in_reach`]) or being hit.
pub static BARNACLE_LURK: Schedule = Schedule::new(
    "ohl/monsters/barnacle_lurk",
    &[
        Task::SetActivity(Activity::Idle),
        Task::StopMoving,
        Task::Wait(0.5),
    ],
    Conditions::ALL_SIGHT.union(Conditions::ALL_DAMAGE),
);

/// A barnacle feeding on whatever its tongue caught: one bite, then the
/// published cadence's pause. Re-selected for as long as the victim stays
/// on the tongue, so a victim that steps off the line below the barnacle
/// (`CAN_MELEE_ATTACK1` clears) is let go rather than chased.
pub static BARNACLE_FEED: Schedule = Schedule::new(
    "ohl/monsters/barnacle_feed",
    &[
        Task::StopMoving,
        Task::SetActivity(Activity::Melee),
        Task::MeleeAttack1,
        Task::Wait(BARNACLE_BITE_INTERVAL),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// An alien controller's hand-launched volley, then a reposition: the
/// published "volleys of small energy balls" plus "constant evasive
/// maneuvering and tendency to stay at a distance", read as fire-then-move.
/// The move reuses the cover tasks, which back away from the enemy, so a
/// controller that fires drifts out of reach and its chase brings it back:
/// the hover-at-a-distance this crate can express without a dedicated
/// strafe. Flight itself is the hull (`crate::movement::flies`).
pub static CONTROLLER_VOLLEY: Schedule = Schedule::new(
    "ohl/monsters/controller_volley",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(CONTROLLER_VOLLEY_INTERVAL),
        Task::RangeAttack1,
        Task::Wait(CONTROLLER_VOLLEY_INTERVAL),
        Task::RangeAttack1,
        Task::SetActivity(Activity::Run),
        Task::FindCover,
        Task::TakeCover,
        Task::RunPath,
        Task::WaitForMovement,
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// An assassin's hit-and-run: a two-shot burst, then a run to somewhere
/// else before the next — the published "run and jump in order to avoid
/// the players fire and ... attack from multiple directions". The jump is
/// not modeled (see the milestone entry).
pub static ASSASSIN_HIT_AND_RUN: Schedule = Schedule::new(
    "ohl/monsters/assassin_hit_and_run",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(0.15),
        Task::RangeAttack1,
        Task::SetActivity(Activity::Run),
        Task::FindCover,
        Task::TakeCover,
        Task::RunPath,
        Task::WaitForMovement,
        Task::FaceEnemy,
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::NO_AMMO_LOADED)
        .union(Conditions::HEAVY_DAMAGE)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// An assassin breaking off after a hard hit: out of sight, then back to
/// facing where the enemy was — the published "hide-and-seek".
pub static ASSASSIN_RETREAT: Schedule = Schedule::new(
    "ohl/monsters/assassin_retreat",
    &[
        Task::SetActivity(Activity::Run),
        Task::FindCover,
        Task::TakeCover,
        Task::RunPath,
        Task::WaitForMovement,
        Task::FaceLastKnownPosition,
        Task::Wait(0.5),
    ],
    Conditions::GENERAL_INTERRUPTS,
);

/// A scripted prop (`monster_generic`, `monster_furniture`) standing where
/// it was put. Nothing interrupts it: a prop that is looked at, shot or
/// shouted at stays a prop, and re-selection at the end of each spell is
/// how it notices a state change at all.
pub static PASSIVE_STAND: Schedule = Schedule::new(
    "ohl/monsters/passive_stand",
    &[
        Task::SetActivity(Activity::Idle),
        Task::StopMoving,
        Task::WaitRandom { min: 1.0, max: 4.0 },
    ],
    Conditions::EMPTY,
);

/// A `monster_generic` that has noticed something: alert, where it stands,
/// facing the way its map put it. Like [`PASSIVE_STAND`] nothing
/// interrupts it, and re-selection at the end of each spell is how it
/// notices that whatever it saw has gone.
///
/// The ordinary stand-and-look (`crate::brain::ALERT_STAND`) will not do
/// for a prop: it is interrupted by any sighting, which is right for a
/// monster that will pick a fighting schedule next and wrong for one that
/// never does — a prop in combat with an enemy in view re-selected it
/// every tick, ran none of its tasks and never settled. It also turns to
/// face the enemy, which a posed set piece should not.
pub static PROP_ALERT: Schedule = Schedule::new(
    "ohl/monsters/prop_alert",
    &[
        Task::SetActivity(Activity::Alert),
        Task::StopMoving,
        Task::WaitRandom { min: 1.0, max: 3.0 },
    ],
    Conditions::EMPTY,
);

/// A critter (rat, cockroach) ambling somewhere nearby and pausing.
pub static CRITTER_WANDER: Schedule = Schedule::new(
    "ohl/monsters/critter_wander",
    &[
        Task::SetActivity(Activity::Walk),
        Task::Wander {
            distance: CRITTER_WANDER_DISTANCE,
        },
        Task::WalkPath,
        Task::WaitForMovement,
        Task::SetActivity(Activity::Idle),
        Task::WaitRandom { min: 0.5, max: 3.0 },
    ],
    Conditions::ALL_SOUND
        .union(Conditions::ALL_DAMAGE)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A critter scurrying off after a noise — the cockroach's published
/// "easily scared" — at running speed, further than a wander.
pub static CRITTER_SCATTER: Schedule = Schedule::new(
    "ohl/monsters/critter_scatter",
    &[
        Task::SetActivity(Activity::Run),
        Task::Wander {
            distance: CRITTER_SCATTER_DISTANCE,
        },
        Task::RunPath,
        Task::WaitForMovement,
        Task::SetActivity(Activity::Idle),
        Task::Wait(0.5),
    ],
    Conditions::ALL_DAMAGE.union(Conditions::GENERAL_INTERRUPTS),
);

// --- Wave 1 batch B: bosses and aircraft ------------------------------------
//
// Project-authored, like everything above, from the published descriptions
// cited in `docs/FORMAT_SOURCES.md` ("Monster definitions", "Wave 1 batch
// B"): the Gonarch launches acid at a distance and charges to claw, and
// walks a node trail it is invulnerable on; the Nihilanth attacks with
// electrical particles, and not at all until it is activated; the Apache
// circles its route firing its machine gun, which "can rotate freely"; the
// Osprey only circles. The three that hover ([`MonsterBrain::hovers`])
// never select a schedule that moves *or stops* them: an aircraft's
// position is its flight plan's (`crate::monsters::aircraft`), handed to
// the movement step by `crate::monsters::bosses` as a route, and
// `Task::StopMoving` would wipe that route every time a schedule started.
// The Gonarch's travel leg likewise only waits on a route the trail driver
// already set.

/// The Gonarch walking a trail leg (`SPECIAL1`, set by the trail driver
/// while it has a destination). Nothing interrupts it: it is shielded on
/// the way and fights only once it arrives.
pub static GONARCH_TRAVEL: Schedule = Schedule::new(
    "ohl/monsters/gonarch_travel",
    &[
        Task::SetActivity(Activity::Run),
        Task::WaitForMovement,
        Task::Wait(0.1),
    ],
    Conditions::EMPTY,
);

/// The Gonarch's acid mortar, resolved by the engine like every ranged
/// attack it has no projectile for.
pub static GONARCH_SPIT: Schedule = Schedule::new(
    "ohl/monsters/gonarch_spit",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(GONARCH_SPIT_INTERVAL),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// The Nihilanth before activation (`SPECIAL2`, set by the shield driver):
/// hangs there and attacks nothing.
pub static NIHILANTH_DORMANT: Schedule = Schedule::new(
    "ohl/monsters/nihilanth_dormant",
    &[Task::SetActivity(Activity::Idle), Task::Wait(0.5)],
    Conditions::EMPTY,
);

/// The Nihilanth's electrical attack.
pub static NIHILANTH_ZAP: Schedule = Schedule::new(
    "ohl/monsters/nihilanth_zap",
    &[
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(NIHILANTH_ZAP_INTERVAL),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// A hovering kind with nothing to shoot at: keeps flying (or hanging
/// there) and keeps watch. Neither moves nor stops it. Its interrupts are
/// the things that change what it would *do* — an attack coming into
/// reach, an enemy gained or lost — not every sighting, since in combat
/// with an enemy out of reach a sighting is the steady state.
pub static HOVER_WATCH: Schedule = Schedule::new(
    "ohl/monsters/hover_watch",
    &[Task::SetActivity(Activity::Alert), Task::Wait(0.5)],
    Conditions::ALL_ATTACK
        .union(Conditions::NEW_ENEMY)
        .union(Conditions::ENEMY_DEAD)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// The Apache's machine gun: a three-round burst without turning the
/// airframe, since the gun is published as rotating freely.
pub static APACHE_GUN: Schedule = Schedule::new(
    "ohl/monsters/apache_gun",
    &[
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(APACHE_GUN_INTERVAL),
        Task::RangeAttack1,
        Task::Wait(APACHE_GUN_INTERVAL),
        Task::RangeAttack1,
        Task::Wait(APACHE_BURST_PAUSE),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// Every schedule this module adds, for lookup by name (joins
/// `crate::brain::ALL` at the `schedule_by_name` call site).
pub static ALL: &[&Schedule] = &[
    &HOUNDEYE_PACK_BLAST,
    &BULLSQUID_SPIT,
    &SLAVE_ZAP,
    &GRUNT_SUPPRESS,
    &GRUNT_FLANK,
    &GRUNT_GRENADE,
    &APACHE_ROCKET,
    &CONTROLLER_HEAD_BALL,
    &ASSASSIN_GRENADE,
    &FOLLOW_PLAYER,
    &SCIENTIST_HEAL,
    &TURRET_DEPLOY,
    &TURRET_RETRACT,
    &TURRET_TRACK,
    &TENTACLE_STRIKE,
    &ROOTED_LISTEN,
    &GARG_FLAME,
    &GARG_STOMP,
    // Wave 1 batch A.
    &BARNACLE_LURK,
    &BARNACLE_FEED,
    &CONTROLLER_VOLLEY,
    &ASSASSIN_HIT_AND_RUN,
    &ASSASSIN_RETREAT,
    &PASSIVE_STAND,
    &PROP_ALERT,
    &CRITTER_WANDER,
    &CRITTER_SCATTER,
    // Wave 1 batch B.
    &GONARCH_TRAVEL,
    &GONARCH_SPIT,
    &NIHILANTH_DORMANT,
    &NIHILANTH_ZAP,
    &HOVER_WATCH,
    &APACHE_GUN,
];

/// Looks a schedule up by name across both this module's set and
/// `crate::brain`'s default set.
#[must_use]
pub fn schedule_by_name(name: &str) -> Option<&'static Schedule> {
    ALL.iter()
        .copied()
        .find(|schedule| schedule.name == name)
        .or_else(|| crate::brain::schedule_by_name(name))
}

// --- Tuning constants -------------------------------------------------------
//
// The houndeye/scientist numbers below are cited to the same TWHL pages as
// their `table::spec_for` rows (`TWHL:Houndeye`, `TWHL:Scientist`; see
// `docs/FORMAT_SOURCES.md`, "Monster definitions"). Anything not published —
// marked `TODO(black-box)` — is this project's own placeholder.

/// `TWHL:Houndeye`'s published per-squad-member blast damage bonus: +10%
/// per packmate.
pub const HOUNDEYE_PACK_BONUS_PER_MEMBER: f32 = 0.10;

/// `TWHL:Houndeye`'s published cap on the blast bonus: +30% (a squad of up
/// to four, so at most three *other* packmates).
pub const HOUNDEYE_PACK_BONUS_CAP: f32 = 1.30;

/// `TWHL:Scientist`'s published heal threshold: the target must be below
/// half health.
pub const SCIENTIST_HEAL_THRESHOLD_FRACTION: f32 = 0.5;

/// `TWHL:Scientist`'s published heal range, in world units.
pub const SCIENTIST_HEAL_RANGE: f32 = 128.0;

/// `TWHL:Scientist`'s published heal amount, in health points.
pub const SCIENTIST_HEAL_AMOUNT: f32 = 25.0;

/// `TWHL:Scientist`'s published heal cooldown, in seconds.
pub const SCIENTIST_HEAL_COOLDOWN: f32 = 60.0;

// Wave 1 batch A.

/// How many bites a barnacle's feed is read as taking to kill its prey:
/// `TWHL:Monster_barnacle` publishes "killed in 1 to 3 bites" for a
/// player; three, the slow end, paces the cadence below. **`TODO(black-
/// box)`**: the bite count for a given prey is not published beyond that
/// range.
pub const BARNACLE_BITES_TO_KILL: f32 = 3.0;

/// The pause between a barnacle's bites, in seconds: the published ten
/// seconds to kill its prey (`table::BARNACLE_KILL_SECONDS`) spread over
/// [`BARNACLE_BITES_TO_KILL`] bites. The spreading is this project's
/// reading; only the ten seconds is cited.
pub const BARNACLE_BITE_INTERVAL: f32 =
    super::table::BARNACLE_KILL_SECONDS / BARNACLE_BITES_TO_KILL;

/// The pause between the shots of an alien controller's volley, in
/// seconds. **`TODO(black-box)`**: a volley is published, its rate is not.
pub const CONTROLLER_VOLLEY_INTERVAL: f32 = 0.2;

/// An alien controller's flight speeds, walking and running, in units per
/// second. **`TODO(black-box)`**: published only as "evasive"; these are
/// placeholders faster than a walker's defaults.
pub const CONTROLLER_SPEEDS: (f32, f32) = (120.0, 240.0);

/// A human assassin's speeds, walking and running, in units per second.
/// **`TODO(black-box)`**: published only as "fast" and "extremely agile";
/// the run is a placeholder well above a grunt's.
pub const ASSASSIN_SPEEDS: (f32, f32) = (80.0, 320.0);

/// A critter's speeds, walking and running, in units per second.
/// **`TODO(black-box)`**: not published; placeholders for something small.
/// (The route stuck check measures a mover against its own step,
/// `crate::movement::StuckDetector::record_step`, so a walk this slow
/// still finishes its legs at the engine's 100 Hz tick.)
pub const CRITTER_SPEEDS: (f32, f32) = (40.0, 120.0);

/// How far a critter wanders in one spell, in world units.
/// **`TODO(black-box)`**: not published.
pub const CRITTER_WANDER_DISTANCE: f32 = 96.0;

/// How far a startled critter scatters, in world units.
/// **`TODO(black-box)`**: not published.
pub const CRITTER_SCATTER_DISTANCE: f32 = 256.0;

// Wave 1 batch B. None of the four pages publishes an attack rate.

/// The pause after a Gonarch's acid launch, in seconds.
/// **`TODO(black-box)`**: not published.
pub const GONARCH_SPIT_INTERVAL: f32 = 1.5;

/// The pause after a Nihilanth's zap, in seconds. **`TODO(black-box)`**:
/// not published.
pub const NIHILANTH_ZAP_INTERVAL: f32 = 1.0;

/// The pause between the rounds of an Apache's machine-gun burst, in
/// seconds. **`TODO(black-box)`**: not published.
pub const APACHE_GUN_INTERVAL: f32 = 0.1;

/// The pause after an Apache's burst, in seconds. **`TODO(black-box)`**:
/// not published.
pub const APACHE_BURST_PAUSE: f32 = 0.5;

/// The blast damage a houndeye (or its pack) actually deals, given the base
/// per-hit damage from [`MonsterSpec::ranged`], the number of *other*
/// squadmates joining the blast, and whether the blast has line of sight to
/// its target.
///
/// Published formula: `base * (1 + bonus * packmates)`, capped at
/// [`HOUNDEYE_PACK_BONUS_CAP`], then halved
/// ([`crate::monsters::table::HOUNDEYE_BLAST_NO_LOS_MULTIPLIER`]) when
/// `has_line_of_sight` is `false`.
#[must_use]
pub fn houndeye_pack_bonus(base_damage: f32, packmates: u32, has_line_of_sight: bool) -> f32 {
    // Squads are bounded by `MAX_SQUAD_SIZE` (four), so this narrowing never
    // loses meaningful precision; `as` is used rather than `f32::from`
    // because there is no lossless `From<u32> for f32` in `core`.
    #[allow(clippy::cast_precision_loss)]
    let packmates = packmates as f32;
    let multiplier =
        (1.0 + HOUNDEYE_PACK_BONUS_PER_MEMBER * packmates).min(HOUNDEYE_PACK_BONUS_CAP);
    let los_multiplier = if has_line_of_sight {
        1.0
    } else {
        super::table::HOUNDEYE_BLAST_NO_LOS_MULTIPLIER
    };
    base_damage * multiplier * los_multiplier
}

/// Whether a scientist may heal now: the target is hurt below the threshold
/// fraction, is within [`SCIENTIST_HEAL_RANGE`], and the cooldown has
/// elapsed since the last heal.
#[must_use]
pub fn scientist_heal_ready(
    target_health: f32,
    target_max_health: f32,
    distance: f32,
    time_since_last_heal: f32,
) -> bool {
    target_max_health > 0.0
        && target_health > 0.0
        && (target_health / target_max_health) < SCIENTIST_HEAL_THRESHOLD_FRACTION
        && distance <= SCIENTIST_HEAL_RANGE
        && time_since_last_heal >= SCIENTIST_HEAL_COOLDOWN
}

/// The health a target has after one heal, clamped to `target_max_health`.
#[must_use]
pub fn apply_heal(target_health: f32, target_max_health: f32) -> f32 {
    (target_health + SCIENTIST_HEAL_AMOUNT).min(target_max_health)
}

// --- The brain itself -------------------------------------------------------

/// A data-driven [`Brain`]: behaviour is switched on [`MonsterKind`], data
/// comes from [`MonsterSpec`].
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterBrain {
    /// Which monster this is.
    pub kind: MonsterKind,
    /// Its stat table row.
    pub spec: &'static MonsterSpec,
}

impl MonsterBrain {
    /// The brain for `kind`, or `None` for an [`MonsterKind::Unknown`]
    /// classname this table has no row for.
    #[must_use]
    pub fn for_kind(kind: MonsterKind) -> Option<Self> {
        let spec = spec_for(&kind)?;
        Some(Self { kind, spec })
    }

    fn never_flees(&self) -> bool {
        self.spec.flags.contains(MonsterFlags::NEVER_FLEES)
    }

    fn rooted(&self) -> bool {
        self.spec.flags.contains(MonsterFlags::ROOTED)
    }

    /// The two scripted-prop kinds, which never fight or investigate.
    fn is_passive_prop(&self) -> bool {
        matches!(self.kind, MonsterKind::Generic | MonsterKind::Furniture)
    }

    /// The two harmless critters, which wander instead of standing.
    fn is_critter(&self) -> bool {
        matches!(self.kind, MonsterKind::Rat | MonsterKind::Cockroach)
    }

    /// The three kinds whose position is not their schedules' to change:
    /// the two aircraft, which their flight plan moves, and the Nihilanth,
    /// which never moves. None of them ever selects a schedule that moves
    /// or stops it.
    pub fn hovers(&self) -> bool {
        matches!(
            self.kind,
            MonsterKind::Nihilanth | MonsterKind::Apache | MonsterKind::Osprey
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per defined monster kind's combat behaviour"
    )]
    fn combat_schedule(&self, conditions: Conditions) -> &'static Schedule {
        use MonsterKind as K;
        match self.kind {
            K::Houndeye => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &HOUNDEYE_PACK_BLAST
                } else if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::Bullsquid => {
                if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &BULLSQUID_SPIT
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::AlienSlave => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &SLAVE_ZAP
                } else if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::AlienGrunt | K::HumanGrunt => {
                if self.kind == K::HumanGrunt && conditions.contains(Conditions::CAN_RANGE_ATTACK2)
                {
                    &GRUNT_GRENADE
                } else if conditions.contains(Conditions::SPECIAL2) {
                    &GRUNT_SUPPRESS
                } else if conditions.contains(Conditions::SPECIAL1) {
                    &GRUNT_FLANK
                } else if conditions.contains(Conditions::NO_AMMO_LOADED) {
                    &crate::brain::RELOAD
                } else if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &crate::brain::RANGE_ATTACK
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::Barney => {
                if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &crate::brain::RANGE_ATTACK
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::Scientist => {
                if conditions.contains(Conditions::SPECIAL1) {
                    &SCIENTIST_HEAL
                } else {
                    &crate::brain::FLEE
                }
            }
            K::Turret | K::MiniTurret | K::Sentry => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &TURRET_TRACK
                } else {
                    &TURRET_DEPLOY
                }
            }
            K::Gargantua => {
                if conditions.contains(Conditions::SPECIAL1) {
                    &GARG_STOMP
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &GARG_FLAME
                } else if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::Tentacle => {
                if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &TENTACLE_STRIKE
                } else {
                    &ROOTED_LISTEN
                }
            }
            // Wave 1 batch A.
            K::Barnacle => {
                if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &BARNACLE_FEED
                } else {
                    &BARNACLE_LURK
                }
            }
            K::AlienController => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK2) {
                    &CONTROLLER_HEAD_BALL
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &CONTROLLER_VOLLEY
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::HumanAssassin => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK2) {
                    &ASSASSIN_GRENADE
                } else if conditions.contains(Conditions::HEAVY_DAMAGE) {
                    &ASSASSIN_RETREAT
                } else if conditions.contains(Conditions::NO_AMMO_LOADED) {
                    &crate::brain::RELOAD
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &ASSASSIN_HIT_AND_RUN
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            // Wave 1 batch B.
            K::BigMomma => {
                if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &GONARCH_SPIT
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
            K::Nihilanth => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &NIHILANTH_ZAP
                } else {
                    &HOVER_WATCH
                }
            }
            K::Apache => {
                if conditions.contains(Conditions::CAN_RANGE_ATTACK2) {
                    &APACHE_ROCKET
                } else if conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
                    &APACHE_GUN
                } else {
                    &HOVER_WATCH
                }
            }
            K::Osprey => &HOVER_WATCH,
            K::Generic => &PROP_ALERT,
            K::Furniture => &PASSIVE_STAND,
            K::Rat | K::Cockroach => &CRITTER_SCATTER,
            K::Ichthyosaur | K::Leech | K::Zombie | K::Headcrab | K::Babycrab | K::Unknown(_) => {
                if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
                    &crate::brain::MELEE_ATTACK
                } else {
                    &crate::brain::CHASE_ENEMY
                }
            }
        }
    }
}

impl Brain for MonsterBrain {
    fn classification(&self) -> Classification {
        self.spec.classification
    }

    fn senses(&self) -> Senses {
        use MonsterKind as K;
        match self.kind {
            // Turrets are published with 360-degree vision. Wave 1 batch B
            // reads the same into the three that hover: an aircraft's gun
            // "can rotate freely", and a boss hanging in its chamber is
            // given no front to be crept up on. `TODO(black-box)`: no page
            // says either outright, and the look distance is the table's
            // own range placeholder.
            K::Turret | K::MiniTurret | K::Sentry | K::Nihilanth | K::Apache | K::Osprey => {
                Senses::omnidirectional(self.range_attack_range())
            }
            K::Tentacle => Senses {
                hearing_sensitivity: TENTACLE_HEARING_SENSITIVITY,
                ..Senses::default()
            },
            // A barnacle has no eyes to point: whatever comes within the
            // tongue's published length, in any direction, is a sighting,
            // and `melee_in_reach` is what narrows that to "on the tongue".
            K::Barnacle => Senses::omnidirectional(super::table::BARNACLE_TONGUE_LENGTH),
            _ => Senses::default(),
        }
    }

    /// The barnacle's tongue is a vertical line, not a sphere: a victim
    /// is on it when it stands within [`super::table::BARNACLE_TONGUE_
    /// RADIUS`] of the line straight below the barnacle's origin and no
    /// further below than the tongue's published length. Everything else
    /// keeps the spherical default.
    fn melee_in_reach(&self, origin: glam::Vec3, enemy_origin: glam::Vec3, distance: f32) -> bool {
        if self.kind != MonsterKind::Barnacle {
            return distance <= self.melee_range();
        }
        let horizontal = glam::Vec2::new(enemy_origin.x - origin.x, enemy_origin.y - origin.y);
        let drop = origin.z - enemy_origin.z;
        horizontal.length() <= super::table::BARNACLE_TONGUE_RADIUS
            && (0.0..=super::table::BARNACLE_TONGUE_LENGTH).contains(&drop)
    }

    /// The barnacle can only ever bite what is on its tongue, so that is
    /// its enemy whenever anything is.
    fn chooses_enemy_in_reach(&self) -> bool {
        self.kind == MonsterKind::Barnacle
    }

    fn speeds(&self) -> (f32, f32) {
        match self.kind {
            // Wave 1 batch A.
            MonsterKind::AlienController => CONTROLLER_SPEEDS,
            MonsterKind::HumanAssassin => ASSASSIN_SPEEDS,
            MonsterKind::Rat | MonsterKind::Cockroach => CRITTER_SPEEDS,
            _ => (40.0, 160.0),
        }
    }

    fn has_melee_attack(&self) -> bool {
        self.spec.melee.is_some()
    }

    fn has_range_attack(&self) -> bool {
        self.spec.ranged.is_some()
    }

    fn melee_range(&self) -> f32 {
        self.spec.melee.map_or(64.0, |melee| melee.range)
    }

    fn range_attack_range(&self) -> f32 {
        self.spec.ranged.map_or(1_024.0, |ranged| ranged.range)
    }

    fn select_schedule(&self, state: MonsterState, conditions: Conditions) -> &'static Schedule {
        if self.rooted() {
            return self.rooted_schedule(state, conditions);
        }
        if self.is_passive_prop() {
            return self.prop_schedule(state, conditions);
        }
        if self.is_critter() {
            return Self::critter_schedule(state, conditions);
        }
        if conditions.contains(Conditions::HEAR_DANGER) && !self.never_flees() {
            return &crate::brain::TAKE_COVER_FROM_DANGER;
        }
        if conditions.contains(Conditions::SEE_FEAR) && !self.never_flees() {
            return &crate::brain::FLEE;
        }
        if matches!(self.kind, MonsterKind::Barney | MonsterKind::Scientist)
            && conditions.contains(Conditions::SPECIAL2)
            && !conditions.contains(Conditions::SEE_ENEMY)
        {
            return &FOLLOW_PLAYER;
        }
        // Wave 1 batch B: a Gonarch with a trail leg to walk walks it
        // whatever else it perceives (it is shielded on the way), and a
        // Nihilanth nobody has activated yet does nothing at all.
        if matches!(
            state,
            MonsterState::None
                | MonsterState::Idle
                | MonsterState::Alert
                | MonsterState::Combat
                | MonsterState::Hunt
        ) {
            if self.kind == MonsterKind::BigMomma && conditions.contains(Conditions::SPECIAL1) {
                return &GONARCH_TRAVEL;
            }
            if self.kind == MonsterKind::Nihilanth && conditions.contains(Conditions::SPECIAL2) {
                return &NIHILANTH_DORMANT;
            }
        }

        match state {
            MonsterState::Combat => self.combat_schedule(conditions),
            MonsterState::Hunt => match self.kind {
                MonsterKind::Tentacle
                | MonsterKind::Turret
                | MonsterKind::MiniTurret
                | MonsterKind::Sentry => &ROOTED_LISTEN,
                _ if self.hovers() => &HOVER_WATCH,
                _ => &crate::brain::HUNT_ENEMY,
            },
            MonsterState::Alert => match self.kind {
                MonsterKind::Tentacle => &ROOTED_LISTEN,
                MonsterKind::Turret | MonsterKind::MiniTurret | MonsterKind::Sentry => {
                    &TURRET_DEPLOY
                }
                _ if self.hovers() => &HOVER_WATCH,
                _ => {
                    if conditions.intersects(Conditions::ALL_SOUND) {
                        &crate::brain::INVESTIGATE_SOUND
                    } else if conditions.contains(Conditions::TASK_FAILED) {
                        &crate::brain::FAIL
                    } else {
                        &crate::brain::ALERT_STAND
                    }
                }
            },
            MonsterState::None | MonsterState::Idle => match self.kind {
                MonsterKind::Tentacle => &ROOTED_LISTEN,
                MonsterKind::Turret | MonsterKind::MiniTurret | MonsterKind::Sentry => {
                    &TURRET_RETRACT
                }
                _ if self.hovers() => &HOVER_WATCH,
                _ if conditions.contains(Conditions::TASK_FAILED) => &crate::brain::FAIL,
                _ => &crate::brain::IDLE_STAND,
            },
            MonsterState::Dead
            | MonsterState::Prone
            | MonsterState::PlayDead
            | MonsterState::Script => &crate::brain::INERT,
        }
    }
}

impl MonsterBrain {
    /// Schedules for a [`MonsterFlags::ROOTED`] kind: nothing here ever
    /// carries a movement task. The barnacle lurks and feeds; furniture
    /// only ever stands (it is passive too, and passivity wins).
    fn rooted_schedule(&self, state: MonsterState, conditions: Conditions) -> &'static Schedule {
        match state {
            MonsterState::Dead
            | MonsterState::Prone
            | MonsterState::PlayDead
            | MonsterState::Script => &crate::brain::INERT,
            MonsterState::Combat if self.kind == MonsterKind::Barnacle => {
                self.combat_schedule(conditions)
            }
            _ if self.kind == MonsterKind::Barnacle => &BARNACLE_LURK,
            _ => &PASSIVE_STAND,
        }
    }

    /// Schedules for a scripted prop: it stands, and — for the generic
    /// monster, which is published as a player ally with a working set of
    /// senses — goes alert where it stands when it notices something
    /// ([`PROP_ALERT`]), without turning. It never fights, flees,
    /// investigates or takes cover, since a prop that walks off is a
    /// broken set piece, and a `scripted_sequence` drives it from outside
    /// the schedule system anyway (`crate::scripts`).
    fn prop_schedule(&self, state: MonsterState, conditions: Conditions) -> &'static Schedule {
        match state {
            MonsterState::Dead
            | MonsterState::Prone
            | MonsterState::PlayDead
            | MonsterState::Script => &crate::brain::INERT,
            MonsterState::Combat | MonsterState::Hunt | MonsterState::Alert
                if self.kind == MonsterKind::Generic =>
            {
                if conditions.contains(Conditions::TASK_FAILED) {
                    &crate::brain::FAIL
                } else {
                    &PROP_ALERT
                }
            }
            _ if self.kind == MonsterKind::Generic => &crate::brain::IDLE_STAND,
            _ => &PASSIVE_STAND,
        }
    }

    /// Schedules for a harmless critter: wander while nothing is going on,
    /// scatter when something is (a noise, a hit, a sighting), never fight.
    fn critter_schedule(state: MonsterState, conditions: Conditions) -> &'static Schedule {
        match state {
            MonsterState::Dead
            | MonsterState::Prone
            | MonsterState::PlayDead
            | MonsterState::Script => &crate::brain::INERT,
            _ if conditions.contains(Conditions::TASK_FAILED) => &crate::brain::FAIL,
            MonsterState::Combat | MonsterState::Hunt | MonsterState::Alert => &CRITTER_SCATTER,
            MonsterState::None | MonsterState::Idle => &CRITTER_WANDER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HOUNDEYE_PACK_BONUS_CAP, MonsterBrain, SCIENTIST_HEAL_COOLDOWN,
        SCIENTIST_HEAL_THRESHOLD_FRACTION, apply_heal, houndeye_pack_bonus, scientist_heal_ready,
    };
    use crate::monsters::table::MonsterKind;
    use crate::schedule::Brain;
    use crate::state::{Classification, Conditions, MonsterState};

    #[test]
    fn every_defined_kind_builds_a_brain_and_covers_every_state() {
        for kind in MonsterKind::defined() {
            let brain = MonsterBrain::for_kind(kind.clone()).expect("defined kind has a spec");
            for state in MonsterState::ALL {
                let _ = brain.select_schedule(state, Conditions::EMPTY);
            }
            assert!(brain.melee_range() > 0.0);
            assert!(brain.range_attack_range() > 0.0);
        }
    }

    #[test]
    fn an_unknown_classname_has_no_brain() {
        assert!(MonsterBrain::for_kind(MonsterKind::Unknown("monster_x".into())).is_none());
    }

    #[test]
    fn houndeyes_prefer_the_pack_blast_schedule_when_ranged_is_available() {
        let brain = MonsterBrain::for_kind(MonsterKind::Houndeye).expect("defined");
        let schedule = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(schedule.name, super::HOUNDEYE_PACK_BLAST.name);
    }

    #[test]
    fn a_grunt_with_a_grenade_opportunity_throws_it_over_everything_else() {
        let brain = MonsterBrain::for_kind(MonsterKind::HumanGrunt).expect("defined");
        let schedule = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1 | Conditions::CAN_RANGE_ATTACK2,
        );
        assert_eq!(schedule.name, super::GRUNT_GRENADE.name);
    }

    #[test]
    fn a_scientist_heals_instead_of_fighting_when_special1_is_set() {
        let brain = MonsterBrain::for_kind(MonsterKind::Scientist).expect("defined");
        let schedule = brain.select_schedule(MonsterState::Combat, Conditions::SPECIAL1);
        assert_eq!(schedule.name, super::SCIENTIST_HEAL.name);
    }

    #[test]
    fn a_never_flees_monster_ignores_danger_and_fear() {
        let brain = MonsterBrain::for_kind(MonsterKind::Gargantua).expect("defined");
        let danger = brain.select_schedule(MonsterState::Idle, Conditions::HEAR_DANGER);
        assert_ne!(danger.name, crate::brain::TAKE_COVER_FROM_DANGER.name);
        let fear = brain.select_schedule(MonsterState::Idle, Conditions::SEE_FEAR);
        assert_ne!(fear.name, crate::brain::FLEE.name);
    }

    #[test]
    fn a_tentacle_never_selects_a_schedule_that_moves_it() {
        let brain = MonsterBrain::for_kind(MonsterKind::Tentacle).expect("defined");
        for state in MonsterState::ALL {
            for conditions in [
                Conditions::EMPTY,
                Conditions::HEAR_SOUND,
                Conditions::SEE_ENEMY,
                Conditions::CAN_MELEE_ATTACK1,
            ] {
                let schedule = brain.select_schedule(state, conditions);
                assert!(
                    !schedule.tasks.iter().any(|task| matches!(
                        task,
                        crate::schedule::Task::RunPath | crate::schedule::Task::WalkPath
                    )),
                    "{} moves a rooted tentacle",
                    schedule.name
                );
            }
        }
    }

    // Wave 1 batch B.

    /// A spread of conditions every state is tried against.
    const SPREAD: [Conditions; 13] = [
        Conditions::EMPTY,
        Conditions::HEAR_SOUND,
        Conditions::HEAR_DANGER,
        Conditions::SEE_FEAR,
        Conditions::SEE_ENEMY,
        Conditions::SEE_ENEMY.union(Conditions::CAN_RANGE_ATTACK1),
        Conditions::SEE_ENEMY.union(Conditions::CAN_RANGE_ATTACK2),
        Conditions::SEE_ENEMY.union(Conditions::CAN_MELEE_ATTACK1),
        Conditions::ENEMY_OCCLUDED,
        Conditions::HEAVY_DAMAGE,
        Conditions::TASK_FAILED,
        Conditions::SPECIAL1,
        Conditions::SPECIAL2,
    ];

    /// A hovering kind's position is not its schedules' to change: no
    /// schedule it can ever select moves it, and none stops it either —
    /// `StopMoving` would wipe the route an aircraft's flight plan hands
    /// the movement step every tick.
    #[test]
    fn a_hovering_kind_never_selects_a_schedule_that_moves_or_stops_it() {
        let mut hovering = 0;
        for kind in MonsterKind::defined() {
            let brain = MonsterBrain::for_kind(kind.clone()).expect("defined");
            if !brain.hovers() {
                continue;
            }
            hovering += 1;
            for state in MonsterState::ALL {
                for conditions in SPREAD {
                    let schedule = brain.select_schedule(state, conditions);
                    assert!(
                        !moves(schedule)
                            && !schedule.tasks.contains(&crate::schedule::Task::StopMoving),
                        "{kind:?} in {state:?} with {conditions} runs {}",
                        schedule.name
                    );
                }
            }
        }
        assert_eq!(hovering, 3, "the Nihilanth and both aircraft");
    }

    /// The Gonarch walks its trail leg over everything else while the
    /// trail driver marks one, and fights normally otherwise.
    #[test]
    fn a_gonarch_walks_its_trail_leg_over_combat_and_fights_otherwise() {
        let brain = MonsterBrain::for_kind(MonsterKind::BigMomma).expect("defined");
        assert!(!brain.hovers());
        for state in [
            MonsterState::Idle,
            MonsterState::Alert,
            MonsterState::Combat,
            MonsterState::Hunt,
        ] {
            let travelling = brain.select_schedule(
                state,
                Conditions::SPECIAL1
                    | Conditions::SEE_ENEMY
                    | Conditions::CAN_MELEE_ATTACK1
                    | Conditions::HEAVY_DAMAGE,
            );
            assert_eq!(travelling.name, super::GONARCH_TRAVEL.name, "{state:?}");
            assert!(!moves(travelling), "the driver owns the route");
        }
        assert_eq!(
            brain
                .select_schedule(MonsterState::Dead, Conditions::SPECIAL1)
                .name,
            crate::brain::INERT.name
        );
        let claw = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1 | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(claw.name, crate::brain::MELEE_ATTACK.name);
        let spit = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(spit.name, super::GONARCH_SPIT.name);
        let chase = brain.select_schedule(MonsterState::Combat, Conditions::SEE_ENEMY);
        assert_eq!(chase.name, crate::brain::CHASE_ENEMY.name);
    }

    /// The Nihilanth attacks nothing until activated, then zaps whatever it
    /// can reach, from a chamber it watches all of.
    #[test]
    fn a_nihilanth_attacks_nothing_until_activated_and_then_zaps() {
        let brain = MonsterBrain::for_kind(MonsterKind::Nihilanth).expect("defined");
        for state in [
            MonsterState::Idle,
            MonsterState::Alert,
            MonsterState::Combat,
        ] {
            let dormant = brain.select_schedule(
                state,
                Conditions::SPECIAL2 | Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
            );
            assert_eq!(dormant.name, super::NIHILANTH_DORMANT.name, "{state:?}");
            assert!(!dormant.tasks.iter().any(|task| matches!(
                task,
                crate::schedule::Task::RangeAttack1 | crate::schedule::Task::RangeAttack2
            )));
        }
        let zap = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(zap.name, super::NIHILANTH_ZAP.name);
        let out_of_reach = brain.select_schedule(MonsterState::Combat, Conditions::SEE_ENEMY);
        assert_eq!(out_of_reach.name, super::HOVER_WATCH.name);
        assert!(brain.senses().fov_cos <= -1.0, "watches its whole chamber");
    }

    /// The Apache fires its gun in bursts without turning; the Osprey never
    /// attacks at all.
    #[test]
    fn an_apache_fires_bursts_without_turning_and_an_osprey_never_attacks() {
        let apache = MonsterBrain::for_kind(MonsterKind::Apache).expect("defined");
        let gun = apache.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(gun.name, super::APACHE_GUN.name);
        assert!(!gun.tasks.contains(&crate::schedule::Task::FaceEnemy));
        let rounds = gun
            .tasks
            .iter()
            .filter(|task| **task == crate::schedule::Task::RangeAttack1)
            .count();
        assert_eq!(rounds, 3);
        assert!(apache.has_range_attack());
        assert!(!apache.has_melee_attack());

        let osprey = MonsterBrain::for_kind(MonsterKind::Osprey).expect("defined");
        assert!(!osprey.has_range_attack());
        assert!(!osprey.has_melee_attack());
        for state in MonsterState::ALL {
            let schedule = osprey.select_schedule(
                state,
                Conditions::SEE_ENEMY
                    | Conditions::CAN_RANGE_ATTACK1
                    | Conditions::CAN_MELEE_ATTACK1,
            );
            assert!(
                !schedule.tasks.iter().any(|task| matches!(
                    task,
                    crate::schedule::Task::RangeAttack1
                        | crate::schedule::Task::RangeAttack2
                        | crate::schedule::Task::MeleeAttack1
                        | crate::schedule::Task::MeleeAttack2
                )),
                "{} attacks",
                schedule.name
            );
        }
    }

    #[test]
    fn houndeye_pack_bonus_scales_with_squad_size_and_is_capped() {
        let solo = houndeye_pack_bonus(10.0, 0, true);
        assert!((solo - 10.0).abs() < 1e-6);
        let trio = houndeye_pack_bonus(10.0, 3, true);
        assert!(trio > solo);
        assert!((trio - 10.0 * HOUNDEYE_PACK_BONUS_CAP).abs() < 1e-4);
        let huge = houndeye_pack_bonus(10.0, 1_000, true);
        assert!((huge - 10.0 * HOUNDEYE_PACK_BONUS_CAP).abs() < 1e-4);
    }

    #[test]
    fn houndeye_pack_bonus_is_halved_without_line_of_sight() {
        let seen = houndeye_pack_bonus(10.0, 2, true);
        let occluded = houndeye_pack_bonus(10.0, 2, false);
        assert!((occluded - seen * 0.5).abs() < 1e-6);
    }

    /// The houndeye's blast reach is `MonsterBrain::range_attack_range`,
    /// resolved from its table row's `ranged.range` — `TWHL:Houndeye`'s
    /// published 192-unit blast radius (`table::HOUNDEYE_BLAST_RADIUS`).
    #[test]
    fn a_houndeyes_blast_radius_matches_its_table_row() {
        let brain = MonsterBrain::for_kind(MonsterKind::Houndeye).expect("defined");
        assert!(brain.has_range_attack());
        // Invented, checked-in fixture value (see `crate::monsters::table`'s
        // black-box note); `AiWorld::tick_one` gates
        // `Conditions::CAN_RANGE_ATTACK1` on exactly this distance.
        let expected_radius = crate::monsters::table::HOUNDEYE_BLAST_RADIUS;
        assert!((brain.range_attack_range() - expected_radius).abs() < 1e-6);
    }

    #[test]
    fn scientist_heal_respects_threshold_range_and_cooldown() {
        assert!(scientist_heal_ready(
            10.0,
            100.0,
            64.0,
            SCIENTIST_HEAL_COOLDOWN
        ));
        assert!(!scientist_heal_ready(
            60.0,
            100.0,
            64.0,
            SCIENTIST_HEAL_COOLDOWN
        ));
        assert!(!scientist_heal_ready(
            10.0,
            100.0,
            999.0,
            SCIENTIST_HEAL_COOLDOWN
        ));
        assert!(!scientist_heal_ready(10.0, 100.0, 64.0, 0.0));
        assert!(!scientist_heal_ready(
            0.0,
            100.0,
            64.0,
            SCIENTIST_HEAL_COOLDOWN
        ));
        let threshold_edge = 100.0 * SCIENTIST_HEAL_THRESHOLD_FRACTION;
        assert!(!scientist_heal_ready(
            threshold_edge,
            100.0,
            64.0,
            SCIENTIST_HEAL_COOLDOWN
        ));
    }

    #[test]
    fn a_heal_never_overshoots_max_health() {
        assert!((apply_heal(10.0, 100.0) - 35.0).abs() < 1e-6);
        assert!((apply_heal(95.0, 100.0) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn every_added_schedule_has_a_unique_resolvable_name() {
        let mut names: Vec<&str> = super::ALL.iter().map(|s| s.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        for schedule in super::ALL {
            assert_eq!(
                super::schedule_by_name(schedule.name)
                    .expect("registered")
                    .name,
                schedule.name
            );
        }
        // The default set is still reachable through this module's lookup.
        assert!(super::schedule_by_name("ohl/idle_stand").is_some());
    }

    // --- Wave 1 batch A ---------------------------------------------------

    fn moves(schedule: &crate::schedule::Schedule) -> bool {
        schedule.tasks.iter().any(|task| {
            matches!(
                task,
                crate::schedule::Task::RunPath
                    | crate::schedule::Task::WalkPath
                    | crate::schedule::Task::TakeCover
                    | crate::schedule::Task::Wander { .. }
                    | crate::schedule::Task::MoveToEnemy { .. }
                    | crate::schedule::Task::MoveToTarget { .. }
                    | crate::schedule::Task::MoveToLastKnownPosition
            )
        })
    }

    /// The barnacle's tongue is a vertical line: a victim straight below
    /// it, up to the published length, is on it; one beside it, or above
    /// it, or too far below, is not — whatever the straight-line distance
    /// says.
    #[test]
    fn a_barnacles_tongue_reaches_straight_down_and_nowhere_else() {
        use crate::monsters::table::{BARNACLE_TONGUE_LENGTH, BARNACLE_TONGUE_RADIUS};
        use glam::Vec3;
        let brain = MonsterBrain::for_kind(MonsterKind::Barnacle).expect("defined");
        let ceiling = Vec3::new(0.0, 0.0, 256.0);
        let below = Vec3::new(8.0, -8.0, 0.0);
        assert!(brain.melee_in_reach(ceiling, below, ceiling.distance(below)));
        let far_below = Vec3::new(0.0, 0.0, 256.0 - BARNACLE_TONGUE_LENGTH + 1.0);
        assert!(brain.melee_in_reach(ceiling, far_below, ceiling.distance(far_below)));
        let too_far_below = Vec3::new(0.0, 0.0, 256.0 - BARNACLE_TONGUE_LENGTH - 1.0);
        assert!(!brain.melee_in_reach(ceiling, too_far_below, ceiling.distance(too_far_below)));
        let beside = Vec3::new(BARNACLE_TONGUE_RADIUS + 1.0, 0.0, 200.0);
        assert!(!brain.melee_in_reach(ceiling, beside, ceiling.distance(beside)));
        let above = Vec3::new(0.0, 0.0, 300.0);
        assert!(!brain.melee_in_reach(ceiling, above, ceiling.distance(above)));
        // Its senses reach the whole tongue length in every direction, so
        // the reach test above is what decides, not the view cone.
        let senses = brain.senses();
        assert!((senses.look_distance - BARNACLE_TONGUE_LENGTH).abs() < 1e-6);
        assert!(senses.fov_cos <= -1.0);
        // Every other kind keeps the spherical default.
        let zombie = MonsterBrain::for_kind(MonsterKind::Zombie).expect("defined");
        assert!(zombie.melee_in_reach(Vec3::ZERO, Vec3::X * 10.0, zombie.melee_range()));
        assert!(!zombie.melee_in_reach(Vec3::ZERO, Vec3::X * 10.0, zombie.melee_range() + 1.0));
    }

    /// A barnacle never selects a schedule that moves it, feeds only while
    /// something is on its tongue, and lets go when it is not.
    #[test]
    fn a_barnacle_lurks_feeds_and_never_moves() {
        let brain = MonsterBrain::for_kind(MonsterKind::Barnacle).expect("defined");
        for state in MonsterState::ALL {
            for conditions in [
                Conditions::EMPTY,
                Conditions::HEAR_DANGER,
                Conditions::SEE_FEAR,
                Conditions::SEE_ENEMY,
                Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1,
                Conditions::TASK_FAILED,
            ] {
                let schedule = brain.select_schedule(state, conditions);
                assert!(!moves(schedule), "{} moves a barnacle", schedule.name);
            }
        }
        let feeding = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1,
        );
        assert_eq!(feeding.name, super::BARNACLE_FEED.name);
        let released = brain.select_schedule(MonsterState::Combat, Conditions::SEE_ENEMY);
        assert_eq!(released.name, super::BARNACLE_LURK.name);
        assert_eq!(
            brain
                .select_schedule(MonsterState::Idle, Conditions::EMPTY)
                .name,
            super::BARNACLE_LURK.name
        );
        // The published ten seconds spread over the slow end of "1 to 3
        // bites".
        assert!(
            (super::BARNACLE_BITE_INTERVAL * super::BARNACLE_BITES_TO_KILL - 10.0).abs() < 1e-5
        );
    }

    /// An alien controller fires a volley when it can and otherwise closes;
    /// it flies (the point hull) at its own speeds.
    #[test]
    fn a_controller_volleys_in_range_and_flies() {
        let brain = MonsterBrain::for_kind(MonsterKind::AlienController).expect("defined");
        assert!(brain.has_range_attack());
        assert!(!brain.has_melee_attack());
        let volley = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(volley.name, super::CONTROLLER_VOLLEY.name);
        assert!(moves(volley), "a volley ends by repositioning");
        let closing = brain.select_schedule(MonsterState::Combat, Conditions::SEE_ENEMY);
        assert_eq!(closing.name, crate::brain::CHASE_ENEMY.name);
        assert_eq!(brain.speeds(), super::CONTROLLER_SPEEDS);
        assert!(crate::movement::flies(brain.spec.hull));
    }

    /// Every mover flies a point hull (`crate::movement::flies`, and
    /// `ohl-nav`'s steering), so a point-hull kind that ever walked would
    /// fly. The alien controller is meant to; every other point-hull kind
    /// (the turrets, and the two aircraft, which only their flight plan
    /// moves) must never select a schedule that moves it.
    #[test]
    fn only_the_controller_moves_on_the_point_hull() {
        let mut point_hull_kinds = 0;
        for kind in MonsterKind::defined() {
            let brain = MonsterBrain::for_kind(kind.clone()).expect("defined");
            if !crate::movement::flies(brain.spec.hull) || *kind == MonsterKind::AlienController {
                continue;
            }
            point_hull_kinds += 1;
            for state in MonsterState::ALL {
                for conditions in [
                    Conditions::EMPTY,
                    Conditions::HEAR_SOUND,
                    Conditions::HEAR_DANGER,
                    Conditions::SEE_FEAR,
                    Conditions::SEE_ENEMY,
                    Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
                    Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1,
                    Conditions::HEAVY_DAMAGE,
                    Conditions::TASK_FAILED,
                ] {
                    let schedule = brain.select_schedule(state, conditions);
                    assert!(
                        !moves(schedule),
                        "{kind:?} on the point hull would fly {} ({state:?}, {conditions:?})",
                        schedule.name
                    );
                }
            }
        }
        assert!(point_hull_kinds > 0, "the turrets are on the point hull");
    }

    /// An assassin fires and relocates, retreats when hit hard, and runs
    /// faster than a grunt.
    #[test]
    fn an_assassin_hits_and_runs_and_retreats_when_hurt() {
        let brain = MonsterBrain::for_kind(MonsterKind::HumanAssassin).expect("defined");
        let burst = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1,
        );
        assert_eq!(burst.name, super::ASSASSIN_HIT_AND_RUN.name);
        assert!(moves(burst), "a burst ends by relocating");
        let retreat = brain.select_schedule(
            MonsterState::Combat,
            Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1 | Conditions::HEAVY_DAMAGE,
        );
        assert_eq!(retreat.name, super::ASSASSIN_RETREAT.name);
        assert!(moves(retreat));
        let grunt = MonsterBrain::for_kind(MonsterKind::HumanGrunt).expect("defined");
        assert!(brain.speeds().1 > grunt.speeds().1);
        assert!(
            brain
                .spec
                .flags
                .contains(crate::monsters::MonsterFlags::SQUAD_MONSTER)
        );
    }

    /// A babycrab runs the headcrab's brain: the same schedule for the
    /// same state and conditions, every time.
    #[test]
    fn a_babycrab_runs_the_headcrab_brain() {
        let baby = MonsterBrain::for_kind(MonsterKind::Babycrab).expect("defined");
        let adult = MonsterBrain::for_kind(MonsterKind::Headcrab).expect("defined");
        for state in MonsterState::ALL {
            for conditions in [
                Conditions::EMPTY,
                Conditions::HEAR_SOUND,
                Conditions::SEE_ENEMY,
                Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1,
                Conditions::SEE_FEAR,
                Conditions::HEAR_DANGER,
                Conditions::TASK_FAILED,
            ] {
                assert_eq!(
                    baby.select_schedule(state, conditions).name,
                    adult.select_schedule(state, conditions).name,
                    "{state:?} {conditions:?}"
                );
            }
        }
        assert_eq!(baby.classification(), adult.classification());
        assert!((baby.melee_range() - adult.melee_range()).abs() < 1e-6);
    }

    /// A scripted prop never fights, flees, investigates or takes cover:
    /// furniture only ever stands, and a generic monster at most turns to
    /// look. Neither ever selects a schedule with an attack in it.
    #[test]
    fn scripted_props_stand_and_never_fight_or_walk_off() {
        use crate::schedule::Task;
        for kind in [MonsterKind::Generic, MonsterKind::Furniture] {
            let brain = MonsterBrain::for_kind(kind.clone()).expect("defined");
            assert!(!brain.has_melee_attack());
            assert!(!brain.has_range_attack());
            for state in MonsterState::ALL {
                for conditions in [
                    Conditions::EMPTY,
                    Conditions::HEAR_DANGER,
                    Conditions::SEE_FEAR,
                    Conditions::SEE_ENEMY,
                    Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1,
                    Conditions::HEAR_SOUND,
                    Conditions::HEAVY_DAMAGE,
                ] {
                    let schedule = brain.select_schedule(state, conditions);
                    assert!(!moves(schedule), "{} walks a {kind:?} off", schedule.name);
                    assert!(
                        !schedule.tasks.iter().any(|task| matches!(
                            task,
                            Task::MeleeAttack1
                                | Task::MeleeAttack2
                                | Task::RangeAttack1
                                | Task::RangeAttack2
                        )),
                        "{} makes a {kind:?} attack",
                        schedule.name
                    );
                }
            }
        }
        let furniture = MonsterBrain::for_kind(MonsterKind::Furniture).expect("defined");
        for state in [
            MonsterState::Idle,
            MonsterState::Alert,
            MonsterState::Combat,
        ] {
            assert_eq!(
                furniture.select_schedule(state, Conditions::SEE_ENEMY).name,
                super::PASSIVE_STAND.name
            );
        }
        let generic = MonsterBrain::for_kind(MonsterKind::Generic).expect("defined");
        assert_eq!(
            generic
                .select_schedule(MonsterState::Alert, Conditions::HEAR_SOUND)
                .name,
            super::PROP_ALERT.name
        );
        assert_eq!(
            generic
                .select_schedule(MonsterState::Idle, Conditions::EMPTY)
                .name,
            crate::brain::IDLE_STAND.name
        );
        assert_eq!(generic.classification(), Classification::PlayerAlly);
    }

    /// A critter wanders when nothing is going on, scatters when something
    /// is, and never attacks.
    #[test]
    fn critters_wander_scatter_and_never_attack() {
        use crate::schedule::Task;
        for kind in [MonsterKind::Rat, MonsterKind::Cockroach] {
            let brain = MonsterBrain::for_kind(kind.clone()).expect("defined");
            assert!(!brain.has_melee_attack());
            assert!(!brain.has_range_attack());
            assert_eq!(
                brain
                    .select_schedule(MonsterState::Idle, Conditions::EMPTY)
                    .name,
                super::CRITTER_WANDER.name
            );
            assert_eq!(
                brain
                    .select_schedule(MonsterState::Alert, Conditions::HEAR_SOUND)
                    .name,
                super::CRITTER_SCATTER.name
            );
            assert_eq!(
                brain
                    .select_schedule(MonsterState::Idle, Conditions::TASK_FAILED)
                    .name,
                crate::brain::FAIL.name
            );
            for state in MonsterState::ALL {
                let schedule = brain.select_schedule(state, Conditions::SEE_ENEMY);
                assert!(
                    !schedule.tasks.iter().any(|task| matches!(
                        task,
                        Task::MeleeAttack1
                            | Task::MeleeAttack2
                            | Task::RangeAttack1
                            | Task::RangeAttack2
                    )),
                    "{} makes a {kind:?} attack",
                    schedule.name
                );
            }
            assert_eq!(brain.speeds(), super::CRITTER_SPEEDS);
        }
        let scatter = super::CRITTER_SCATTER
            .tasks
            .iter()
            .find_map(|task| match task {
                crate::schedule::Task::Wander { distance } => Some(*distance),
                _ => None,
            })
            .expect("a scatter wanders");
        let wander = super::CRITTER_WANDER
            .tasks
            .iter()
            .find_map(|task| match task {
                crate::schedule::Task::Wander { distance } => Some(*distance),
                _ => None,
            })
            .expect("a wander wanders");
        assert!(scatter > wander, "a scatter goes further than a wander");
    }

    /// A ticked critter actually goes somewhere, at the engine's own tick
    /// length: each wander leg is walked to its end and the pause after it
    /// is reached (no `critter_wander` spell ends in failure), the rat
    /// moves, and a run with a critter in it replays exactly.
    #[test]
    fn a_wandering_critter_finishes_its_legs_at_the_engine_tick() {
        use crate::schedule::RunOutcome;
        use crate::senses::SightContext;
        use crate::world::{Actor, AiEventKind, AiWorld, spawn_monster};
        use glam::Vec3;
        let run = || {
            let mut ai = AiWorld::new(7);
            let brain = ai.register_brain(Box::new(
                MonsterBrain::for_kind(MonsterKind::Rat).expect("defined"),
            ));
            let mut world = hecs::World::new();
            let rat = spawn_monster(
                &mut world,
                Actor::new(Classification::None, Vec3::ZERO),
                brain,
            );
            let (mut finished, mut failed) = (0, 0);
            for _ in 0..2_000 {
                let events = ai.tick(
                    &mut world,
                    &SightContext::empty(),
                    ohl_physics::controller::TICK_SECONDS,
                );
                for event in events {
                    if let AiEventKind::ScheduleEnded { name, outcome } = event.kind
                        && name == super::CRITTER_WANDER.name
                    {
                        match outcome {
                            RunOutcome::Done => finished += 1,
                            RunOutcome::Failed => failed += 1,
                            _ => {}
                        }
                    }
                }
            }
            let origin = world.get::<&Actor>(rat).expect("actor").origin;
            (origin, finished, failed, ai.state_hash(&world))
        };
        let (origin, finished, failed, hash) = run();
        assert!(origin.length() > 1.0, "the rat never moved: {origin:?}");
        assert!(
            finished >= 2,
            "a wander leg and its pause completed: {finished}"
        );
        assert_eq!(failed, 0, "a wander leg was given up as stuck");
        assert_eq!(hash, run().3);
    }

    /// Every attack `attacker` makes over `seconds` of ticks at the
    /// engine's own rate, as (seconds since start, target).
    fn attacks_by(
        ai: &mut crate::world::AiWorld,
        world: &mut hecs::World,
        attacker: hecs::Entity,
        seconds: f32,
    ) -> Vec<(f32, Option<hecs::Entity>)> {
        use crate::senses::SightContext;
        use crate::world::AiEventKind;
        let dt = ohl_physics::controller::TICK_SECONDS;
        let mut attacks = Vec::new();
        let mut elapsed = 0.0;
        while elapsed < seconds {
            for event in ai.tick(world, &SightContext::empty(), dt) {
                if event.entity == attacker
                    && let AiEventKind::Attack { target, .. } = event.kind
                {
                    attacks.push((elapsed, target));
                }
            }
            elapsed += dt;
        }
        attacks
    }

    /// A barnacle hung at 256 with a player standing straight under its
    /// tongue, and returns (world, barnacle, player).
    fn barnacle_over_a_player() -> (
        crate::world::AiWorld,
        hecs::World,
        hecs::Entity,
        hecs::Entity,
    ) {
        use crate::world::{Actor, AiWorld, spawn_actor, spawn_monster};
        use glam::Vec3;
        let mut ai = AiWorld::new(7);
        let brain = ai.register_brain(Box::new(
            MonsterBrain::for_kind(MonsterKind::Barnacle).expect("defined"),
        ));
        let mut world = hecs::World::new();
        let mut hung = Actor::new(Classification::Barnacle, Vec3::new(0.0, 0.0, 256.0));
        hung.view_ofs = MonsterKind::Barnacle.view_offset();
        let barnacle = spawn_monster(&mut world, hung, brain);
        let player = spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::ZERO).as_client(),
        );
        (ai, world, barnacle, player)
    }

    /// Wave 1 batch A review: the barnacle's enemy is what is on its
    /// tongue. A player's ally standing nearer to it but off the tongue is
    /// what sight alone would choose (equal hatred, nearer); the barnacle
    /// bites the player under it instead, and never aims at the ally.
    #[test]
    fn a_barnacle_bites_what_is_on_its_tongue_past_a_nearer_hated_thing() {
        use crate::world::{Actor, spawn_actor};
        use glam::Vec3;
        let (mut ai, mut world, barnacle, player) = barnacle_over_a_player();
        let ally = spawn_actor(
            &mut world,
            Actor::new(Classification::PlayerAlly, Vec3::new(80.0, 0.0, 180.0)),
        );
        let attacks = attacks_by(&mut ai, &mut world, barnacle, 2.0);
        assert!(
            attacks.iter().any(|(_, target)| *target == Some(player)),
            "the player on the tongue was never bitten: {attacks:?}"
        );
        assert!(
            attacks.iter().all(|(_, target)| *target != Some(ally)),
            "the barnacle aimed at something off its tongue"
        );
    }

    /// Wave 1 batch A review: the bite cadence. Over ten seconds — the
    /// published time to kill its prey — a barnacle with a player on its
    /// tongue bites at `BARNACLE_BITE_INTERVAL`: once at once, then once
    /// per interval, so three or four times, not once and not every tick.
    #[test]
    fn a_barnacle_bites_at_its_cadence() {
        let (mut ai, mut world, barnacle, player) = barnacle_over_a_player();
        let attacks = attacks_by(&mut ai, &mut world, barnacle, 10.0);
        assert!(
            (3..=4).contains(&attacks.len()),
            "{} bites in ten seconds",
            attacks.len()
        );
        assert!(attacks.iter().all(|(_, target)| *target == Some(player)));
        for pair in attacks.windows(2) {
            let gap = pair[1].0 - pair[0].0;
            assert!(
                gap >= super::BARNACLE_BITE_INTERVAL - 0.05,
                "two bites {gap} s apart"
            );
        }
    }

    /// Wave 1 batch A review: the assassin's burst is two shots. An
    /// assassin with the player in view runs one `assassin_hit_and_run`
    /// spell — the burst, then the relocation — and fires exactly twice in
    /// it.
    #[test]
    fn an_assassins_burst_is_two_shots() {
        use crate::senses::SightContext;
        use crate::world::{Actor, AiEventKind, AiWorld, spawn_actor, spawn_monster};
        use glam::Vec3;
        let mut ai = AiWorld::new(7);
        let brain = ai.register_brain(Box::new(
            MonsterBrain::for_kind(MonsterKind::HumanAssassin).expect("defined"),
        ));
        let mut world = hecs::World::new();
        let assassin = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
        );
        spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(300.0, 0.0, 0.0)).as_client(),
        );
        let (mut in_spell, mut shots, mut spells) = (false, 0, 0);
        for _ in 0..1_000 {
            for event in ai.tick(
                &mut world,
                &SightContext::empty(),
                ohl_physics::controller::TICK_SECONDS,
            ) {
                if event.entity != assassin {
                    continue;
                }
                match event.kind {
                    AiEventKind::ScheduleStarted(name) => {
                        in_spell = name == super::ASSASSIN_HIT_AND_RUN.name;
                    }
                    AiEventKind::ScheduleEnded { name, .. }
                        if in_spell && name == super::ASSASSIN_HIT_AND_RUN.name =>
                    {
                        in_spell = false;
                        spells += 1;
                    }
                    AiEventKind::Attack { .. } if in_spell && spells == 0 => shots += 1,
                    _ => {}
                }
            }
            if spells > 0 {
                break;
            }
        }
        assert_eq!(spells, 1, "the assassin finished a hit-and-run spell");
        assert_eq!(shots, 2, "its burst was {shots} shots");
    }

    /// Wave 1 batch A review: a `monster_generic` with an enemy in plain
    /// view settles into one alert spell after another instead of
    /// re-selecting a schedule every tick, and keeps the yaw it was given.
    #[test]
    fn a_generic_in_view_of_an_enemy_settles_and_does_not_turn() {
        use crate::senses::SightContext;
        use crate::world::{Actor, AiEventKind, AiWorld, spawn_actor, spawn_monster};
        use glam::Vec3;
        let mut ai = AiWorld::new(7);
        let brain = ai.register_brain(Box::new(
            MonsterBrain::for_kind(MonsterKind::Generic).expect("defined"),
        ));
        let mut world = hecs::World::new();
        let prop = spawn_monster(
            &mut world,
            Actor::new(Classification::PlayerAlly, Vec3::ZERO).facing(0.0),
            brain,
        );
        spawn_actor(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::new(100.0, 100.0, 0.0)),
        );
        let mut started = 0;
        for _ in 0..200 {
            for event in ai.tick(
                &mut world,
                &SightContext::empty(),
                ohl_physics::controller::TICK_SECONDS,
            ) {
                if event.entity == prop && matches!(event.kind, AiEventKind::ScheduleStarted(_)) {
                    started += 1;
                }
            }
        }
        let state = world.get::<&crate::world::MonsterAi>(prop).expect("ai");
        assert_eq!(
            state.activity,
            crate::schedule::Activity::Alert,
            "it noticed"
        );
        assert!(started <= 4, "{started} schedules in two seconds");
        let yaw = world.get::<&Actor>(prop).expect("actor").yaw;
        assert!(yaw.abs() < 1e-3, "it turned to {yaw}");
    }
}
