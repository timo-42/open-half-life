//! The crate's own default state machine and schedule set.
//!
//! **Entirely project-authored.** The schedules below were written from the
//! behaviour a monster is publicly described as showing — stand around until
//! something is perceived, look toward a noise, close on and attack an
//! enemy, hunt an enemy that broke line of sight, run away from something
//! feared — and not from any table in any SDK. They exist so the crate is
//! usable and testable before package 7.7 authors per-monster brains; a
//! concrete monster is expected to supply its own [`Brain`].

use crate::schedule::{Activity, Brain, Schedule, Task};
use crate::senses::{Senses, SoundKind};
use crate::state::{Classification, Conditions, MonsterState};

/// Anything worth reacting to while standing idle.
const NOTICE: Conditions = Conditions::EMPTY
    .union(Conditions::ALL_SIGHT)
    .union(Conditions::ALL_SOUND)
    .union(Conditions::ALL_DAMAGE)
    .union(Conditions::PROVOKED);

/// Anything worth reacting to while already fighting.
const COMBAT_NOTICE: Conditions = Conditions::EMPTY
    .union(Conditions::ALL_ATTACK)
    .union(Conditions::ALL_DAMAGE)
    .union(Conditions::HEAR_DANGER)
    .union(Conditions::ENEMY_DEAD)
    .union(Conditions::NEW_ENEMY)
    .union(Conditions::GENERAL_INTERRUPTS);

/// Stand around, occasionally pausing for a random spell.
pub static IDLE_STAND: Schedule = Schedule::new(
    "ohl/idle_stand",
    &[
        Task::SetActivity(Activity::Idle),
        Task::StopMoving,
        Task::WaitRandom { min: 1.0, max: 4.0 },
    ],
    NOTICE,
);

/// Stand around, but alert, watching whatever was last noticed.
pub static ALERT_STAND: Schedule = Schedule::new(
    "ohl/alert_stand",
    &[
        Task::SetActivity(Activity::Alert),
        Task::StopMoving,
        Task::FaceLastKnownPosition,
        Task::WaitRandom { min: 1.0, max: 3.0 },
    ],
    NOTICE.union(Conditions::GENERAL_INTERRUPTS),
);

/// Turn toward a noise and wait to see whether anything follows.
pub static INVESTIGATE_SOUND: Schedule = Schedule::new(
    "ohl/investigate_sound",
    &[
        Task::SetActivity(Activity::Alert),
        Task::StopMoving,
        Task::FaceTarget,
        Task::Wait(0.5),
    ],
    Conditions::ALL_SIGHT
        .union(Conditions::ALL_DAMAGE)
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// Get away from something dangerous, then look back at it.
pub static TAKE_COVER_FROM_DANGER: Schedule = Schedule::new(
    "ohl/take_cover_from_danger",
    &[
        Task::SetActivity(Activity::Cover),
        Task::FindCover,
        Task::TakeCover,
        Task::RunPath,
        Task::WaitForMovement,
        Task::FaceLastKnownPosition,
    ],
    Conditions::GENERAL_INTERRUPTS.union(Conditions::HEAVY_DAMAGE),
);

/// Close on the acquired enemy.
pub static CHASE_ENEMY: Schedule = Schedule::new(
    "ohl/chase_enemy",
    &[
        Task::SetActivity(Activity::Run),
        Task::MoveToEnemy { within: 56.0 },
        Task::RunPath,
        Task::WaitForMovement,
        Task::FaceEnemy,
    ],
    COMBAT_NOTICE,
);

/// Move to where the enemy was last seen.
pub static HUNT_ENEMY: Schedule = Schedule::new(
    "ohl/hunt_enemy",
    &[
        Task::SetActivity(Activity::Run),
        Task::MoveToLastKnownPosition,
        Task::RunPath,
        Task::WaitForMovement,
        Task::FaceLastKnownPosition,
        Task::Wait(0.5),
    ],
    Conditions::SEE_ENEMY
        .union(Conditions::ALL_DAMAGE)
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// Face and hit the enemy.
pub static MELEE_ATTACK: Schedule = Schedule::new(
    "ohl/melee_attack",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Melee),
        Task::MeleeAttack1,
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// Face and shoot the enemy.
pub static RANGE_ATTACK: Schedule = Schedule::new(
    "ohl/range_attack",
    &[
        Task::StopMoving,
        Task::FaceEnemy,
        Task::SetActivity(Activity::Range),
        Task::RangeAttack1,
        Task::Wait(0.2),
    ],
    Conditions::ENEMY_DEAD
        .union(Conditions::ENEMY_OCCLUDED)
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::HEAVY_DAMAGE)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// Reload, out of the enemy's way if possible.
pub static RELOAD: Schedule = Schedule::new(
    "ohl/reload",
    &[
        Task::StopMoving,
        Task::SetActivity(Activity::Reload),
        Task::Reload,
    ],
    Conditions::HEAVY_DAMAGE
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::GENERAL_INTERRUPTS),
);

/// Run from something feared.
pub static FLEE: Schedule = Schedule::new(
    "ohl/flee",
    &[
        Task::SetActivity(Activity::Run),
        Task::EmitSound(SoundKind::Combat, 384.0),
        Task::FindCover,
        Task::TakeCover,
        Task::RunPath,
        Task::WaitForMovement,
    ],
    Conditions::GENERAL_INTERRUPTS,
);

/// Die.
pub static DIE: Schedule = Schedule::new(
    "ohl/die",
    &[
        Task::StopMoving,
        Task::SetActivity(Activity::Die),
        Task::Die,
        Task::SetState(MonsterState::Dead),
    ],
    Conditions::EMPTY,
);

/// Stand still briefly after something went wrong, so a failing schedule
/// cannot spin.
pub static FAIL: Schedule = Schedule::new(
    "ohl/fail",
    &[
        Task::StopMoving,
        Task::SetActivity(Activity::Alert),
        Task::Wait(0.5),
    ],
    Conditions::EMPTY,
);

/// Nothing at all; the terminal schedule for a dead monster.
pub static INERT: Schedule = Schedule::new("ohl/inert", &[Task::Wait(1.0)], Conditions::EMPTY);

/// Every schedule this module publishes, for lookup by name on load.
pub static ALL: &[&Schedule] = &[
    &IDLE_STAND,
    &ALERT_STAND,
    &INVESTIGATE_SOUND,
    &TAKE_COVER_FROM_DANGER,
    &CHASE_ENEMY,
    &HUNT_ENEMY,
    &MELEE_ATTACK,
    &RANGE_ATTACK,
    &RELOAD,
    &FLEE,
    &DIE,
    &FAIL,
    &INERT,
];

/// Looks a schedule up by its stable name.
#[must_use]
pub fn schedule_by_name(name: &str) -> Option<&'static Schedule> {
    ALL.iter().copied().find(|schedule| schedule.name == name)
}

/// The crate's own general state transition rule.
///
/// Dead stays dead and scripted stays scripted; otherwise seeing an enemy
/// means combat, losing sight of one means hunting, perceiving anything else
/// means alert, and perceiving nothing decays back to idle.
#[must_use]
pub fn default_next_state(state: MonsterState, conditions: Conditions) -> MonsterState {
    match state {
        MonsterState::Dead | MonsterState::Script | MonsterState::PlayDead => state,
        _ if conditions.contains(Conditions::SEE_ENEMY) => MonsterState::Combat,
        _ if conditions.intersects(Conditions::ENEMY_OCCLUDED) => MonsterState::Alert,
        _ if conditions.intersects(
            Conditions::ALL_SIGHT
                .union(Conditions::ALL_SOUND)
                .union(Conditions::ALL_DAMAGE)
                .union(Conditions::PROVOKED),
        ) =>
        {
            MonsterState::Alert
        }
        MonsterState::Combat | MonsterState::Hunt | MonsterState::Alert => MonsterState::Alert,
        MonsterState::None => MonsterState::Idle,
        other => other,
    }
}

/// A general-purpose brain over the schedules in this module.
///
/// Useful on its own for entities that have no special behaviour, and as the
/// reference for what a per-monster brain in package 7.7 has to provide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultBrain {
    /// The faction this brain reports.
    pub classification: Classification,
    /// The senses this brain reports.
    pub senses: Senses,
    /// Whether the monster has a ranged attack at all.
    pub has_range_attack: bool,
}

impl DefaultBrain {
    /// A melee-only brain of the given faction.
    #[must_use]
    pub fn melee(classification: Classification) -> Self {
        Self {
            classification,
            senses: Senses::default(),
            has_range_attack: false,
        }
    }

    /// A brain of the given faction that also shoots.
    #[must_use]
    pub fn ranged(classification: Classification) -> Self {
        Self {
            has_range_attack: true,
            ..Self::melee(classification)
        }
    }
}

impl Default for DefaultBrain {
    fn default() -> Self {
        Self::melee(Classification::AlienMonster)
    }
}

impl Brain for DefaultBrain {
    fn classification(&self) -> Classification {
        self.classification
    }

    fn senses(&self) -> Senses {
        self.senses
    }

    fn has_range_attack(&self) -> bool {
        self.has_range_attack
    }

    fn select_schedule(&self, state: MonsterState, conditions: Conditions) -> &'static Schedule {
        if conditions.contains(Conditions::HEAR_DANGER) {
            return &TAKE_COVER_FROM_DANGER;
        }
        if conditions.contains(Conditions::SEE_FEAR) {
            return &FLEE;
        }
        match state {
            MonsterState::Combat => self.combat_schedule(conditions),
            MonsterState::Hunt => &HUNT_ENEMY,
            MonsterState::Alert => {
                if conditions.contains(Conditions::ENEMY_OCCLUDED) {
                    &HUNT_ENEMY
                } else if conditions.intersects(Conditions::ALL_SOUND) {
                    &INVESTIGATE_SOUND
                } else if conditions.contains(Conditions::TASK_FAILED) {
                    &FAIL
                } else {
                    &ALERT_STAND
                }
            }
            MonsterState::None | MonsterState::Idle => {
                if conditions.contains(Conditions::TASK_FAILED) {
                    &FAIL
                } else {
                    &IDLE_STAND
                }
            }
            MonsterState::Dead
            | MonsterState::Prone
            | MonsterState::PlayDead
            | MonsterState::Script => &INERT,
        }
    }
}

impl DefaultBrain {
    fn combat_schedule(&self, conditions: Conditions) -> &'static Schedule {
        if conditions.contains(Conditions::NO_AMMO_LOADED) {
            return &RELOAD;
        }
        if conditions.contains(Conditions::CAN_MELEE_ATTACK1) {
            return &MELEE_ATTACK;
        }
        if self.has_range_attack && conditions.contains(Conditions::CAN_RANGE_ATTACK1) {
            return &RANGE_ATTACK;
        }
        &CHASE_ENEMY
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHASE_ENEMY, DefaultBrain, FLEE, HUNT_ENEMY, IDLE_STAND, MELEE_ATTACK, RANGE_ATTACK,
        TAKE_COVER_FROM_DANGER, default_next_state, schedule_by_name,
    };
    use crate::schedule::Brain;
    use crate::state::{Classification, Conditions, MonsterState};

    #[test]
    fn seeing_an_enemy_moves_to_combat_and_losing_it_to_alert() {
        assert_eq!(
            default_next_state(MonsterState::Idle, Conditions::SEE_ENEMY),
            MonsterState::Combat
        );
        assert_eq!(
            default_next_state(MonsterState::Combat, Conditions::ENEMY_OCCLUDED),
            MonsterState::Alert
        );
        assert_eq!(
            default_next_state(MonsterState::Idle, Conditions::HEAR_SOUND),
            MonsterState::Alert
        );
        assert_eq!(
            default_next_state(MonsterState::Combat, Conditions::EMPTY),
            MonsterState::Alert
        );
        assert_eq!(
            default_next_state(MonsterState::Idle, Conditions::EMPTY),
            MonsterState::Idle
        );
        assert_eq!(
            default_next_state(MonsterState::None, Conditions::EMPTY),
            MonsterState::Idle
        );
        assert_eq!(
            default_next_state(MonsterState::Dead, Conditions::SEE_ENEMY),
            MonsterState::Dead
        );
    }

    #[test]
    fn schedule_selection_covers_every_state() {
        let brain = DefaultBrain::ranged(Classification::HumanMilitary);
        assert_eq!(
            brain
                .select_schedule(MonsterState::Idle, Conditions::EMPTY)
                .name,
            IDLE_STAND.name
        );
        assert_eq!(
            brain
                .select_schedule(MonsterState::Combat, Conditions::SEE_ENEMY)
                .name,
            CHASE_ENEMY.name
        );
        assert_eq!(
            brain
                .select_schedule(
                    MonsterState::Combat,
                    Conditions::SEE_ENEMY | Conditions::CAN_MELEE_ATTACK1
                )
                .name,
            MELEE_ATTACK.name
        );
        assert_eq!(
            brain
                .select_schedule(
                    MonsterState::Combat,
                    Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1
                )
                .name,
            RANGE_ATTACK.name
        );
        assert_eq!(
            brain
                .select_schedule(MonsterState::Alert, Conditions::ENEMY_OCCLUDED)
                .name,
            HUNT_ENEMY.name
        );
        assert_eq!(
            brain
                .select_schedule(MonsterState::Idle, Conditions::HEAR_DANGER)
                .name,
            TAKE_COVER_FROM_DANGER.name
        );
        assert_eq!(
            brain
                .select_schedule(MonsterState::Idle, Conditions::SEE_FEAR)
                .name,
            FLEE.name
        );
        for state in MonsterState::ALL {
            let _ = brain.select_schedule(state, Conditions::EMPTY);
        }
    }

    #[test]
    fn a_melee_brain_never_picks_the_ranged_schedule() {
        let brain = DefaultBrain::melee(Classification::AlienMonster);
        assert_eq!(
            brain
                .select_schedule(
                    MonsterState::Combat,
                    Conditions::SEE_ENEMY | Conditions::CAN_RANGE_ATTACK1
                )
                .name,
            CHASE_ENEMY.name
        );
        assert_eq!(brain.classification(), Classification::AlienMonster);
        assert!(brain.melee_range() > 0.0);
        assert!(brain.range_attack_range() > brain.melee_range());
        assert!(brain.heavy_damage_threshold() > 0.0);
        let (walk, run) = brain.speeds();
        assert!(run > walk);
    }

    #[test]
    fn every_published_schedule_has_a_unique_resolvable_name() {
        let mut names: Vec<&str> = super::ALL.iter().map(|s| s.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        for schedule in super::ALL {
            assert_eq!(
                schedule_by_name(schedule.name).expect("registered").name,
                schedule.name
            );
        }
        assert!(schedule_by_name("ohl/does_not_exist").is_none());
    }
}
