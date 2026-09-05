//! Per-monster [`Brain`] implementations.
//!
//! One [`MonsterBrain`] type, parameterized by [`MonsterKind`] and
//! [`MonsterSpec`], covers all sixteen defined monsters: the *data* (health,
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
//! sound, and a gargantua's flame/stomp attacks — see
//! `docs/FORMAT_SOURCES.md`, "Monster definitions". The exact timings,
//! ranges, damage numbers, squad-bonus formula and heal cooldown/threshold
//! are this project's own **`TODO(black-box)`** placeholders; nothing here
//! reproduces a decompiled schedule or AI routine.

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

/// Every schedule this module adds, for lookup by name (joins
/// `crate::brain::ALL` at the `schedule_by_name` call site).
pub static ALL: &[&Schedule] = &[
    &HOUNDEYE_PACK_BLAST,
    &BULLSQUID_SPIT,
    &SLAVE_ZAP,
    &GRUNT_SUPPRESS,
    &GRUNT_FLANK,
    &GRUNT_GRENADE,
    &FOLLOW_PLAYER,
    &SCIENTIST_HEAL,
    &TURRET_DEPLOY,
    &TURRET_RETRACT,
    &TURRET_TRACK,
    &TENTACLE_STRIKE,
    &ROOTED_LISTEN,
    &GARG_FLAME,
    &GARG_STOMP,
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

// --- Invented (black-box) tuning constants ---------------------------------

/// How much a houndeye's blast damage scales per squad member beyond
/// itself, e.g. 3 packmates blasting together hit harder than one alone.
/// **`TODO(black-box)`**: the existence of pack behaviour is published; this
/// formula and its coefficient are not.
pub const HOUNDEYE_PACK_BONUS_PER_MEMBER: f32 = 0.25;

/// The largest multiplier [`houndeye_pack_bonus`] returns, so an
/// unreasonably large squad cannot make the attack unbounded.
pub const HOUNDEYE_PACK_BONUS_CAP: f32 = 2.5;

/// The health fraction below which a scientist will heal an ally.
/// **`TODO(black-box)`**.
pub const SCIENTIST_HEAL_THRESHOLD_FRACTION: f32 = 0.5;

/// How far away a scientist can still administer first aid, in world units.
/// **`TODO(black-box)`**.
pub const SCIENTIST_HEAL_RANGE: f32 = 128.0;

/// How much health one heal restores. **`TODO(black-box)`**.
pub const SCIENTIST_HEAL_AMOUNT: f32 = 25.0;

/// The shortest time between two heals from the same scientist, in seconds.
/// **`TODO(black-box)`**.
pub const SCIENTIST_HEAL_COOLDOWN: f32 = 10.0;

/// The blast damage a houndeye (or its pack) actually deals, given the base
/// per-hit damage from [`MonsterSpec::ranged`] and the number of *other*
/// squadmates joining the blast.
///
/// Project-owned formula (`TODO(black-box)`): `base * (1 + bonus *
/// packmates)`, capped at [`HOUNDEYE_PACK_BONUS_CAP`] so the multiplier
/// cannot run away.
#[must_use]
pub fn houndeye_pack_bonus(base_damage: f32, packmates: u32) -> f32 {
    // Squads are bounded by `MAX_SQUAD_SIZE` (four), so this narrowing never
    // loses meaningful precision; `as` is used rather than `f32::from`
    // because there is no lossless `From<u32> for f32` in `core`.
    #[allow(clippy::cast_precision_loss)]
    let packmates = packmates as f32;
    let multiplier =
        (1.0 + HOUNDEYE_PACK_BONUS_PER_MEMBER * packmates).min(HOUNDEYE_PACK_BONUS_CAP);
    base_damage * multiplier
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
                if conditions.contains(Conditions::CAN_RANGE_ATTACK2) {
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
            K::Ichthyosaur | K::Leech | K::Zombie | K::Headcrab | K::Unknown(_) => {
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
            K::Turret | K::MiniTurret | K::Sentry => {
                Senses::omnidirectional(self.range_attack_range())
            }
            K::Tentacle => Senses {
                hearing_sensitivity: TENTACLE_HEARING_SENSITIVITY,
                ..Senses::default()
            },
            _ => Senses::default(),
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

        match state {
            MonsterState::Combat => self.combat_schedule(conditions),
            MonsterState::Hunt => match self.kind {
                MonsterKind::Tentacle
                | MonsterKind::Turret
                | MonsterKind::MiniTurret
                | MonsterKind::Sentry => &ROOTED_LISTEN,
                _ => &crate::brain::HUNT_ENEMY,
            },
            MonsterState::Alert => match self.kind {
                MonsterKind::Tentacle => &ROOTED_LISTEN,
                MonsterKind::Turret | MonsterKind::MiniTurret | MonsterKind::Sentry => {
                    &TURRET_DEPLOY
                }
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

#[cfg(test)]
mod tests {
    use super::{
        HOUNDEYE_PACK_BONUS_CAP, MonsterBrain, SCIENTIST_HEAL_COOLDOWN,
        SCIENTIST_HEAL_THRESHOLD_FRACTION, apply_heal, houndeye_pack_bonus, scientist_heal_ready,
    };
    use crate::monsters::table::MonsterKind;
    use crate::schedule::Brain;
    use crate::state::{Conditions, MonsterState};

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

    #[test]
    fn houndeye_pack_bonus_scales_with_squad_size_and_is_capped() {
        let solo = houndeye_pack_bonus(10.0, 0);
        assert!((solo - 10.0).abs() < 1e-6);
        let trio = houndeye_pack_bonus(10.0, 3);
        assert!(trio > solo);
        let huge = houndeye_pack_bonus(10.0, 1_000);
        assert!((huge - 10.0 * HOUNDEYE_PACK_BONUS_CAP).abs() < 1e-4);
    }

    /// The houndeye's blast reach is `MonsterBrain::range_attack_range`,
    /// resolved from its table row's `ranged.range` (an invented, checked-in
    /// fixture value — see `crate::monsters::table`'s black-box note).
    #[test]
    fn a_houndeyes_blast_radius_matches_its_table_row() {
        let brain = MonsterBrain::for_kind(MonsterKind::Houndeye).expect("defined");
        assert!(brain.has_range_attack());
        // Invented, checked-in fixture value (see `crate::monsters::table`'s
        // black-box note); `AiWorld::tick_one` gates
        // `Conditions::CAN_RANGE_ATTACK1` on exactly this distance.
        let expected_radius = 384.0;
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
}
