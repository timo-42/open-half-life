//! Tasks, schedules and the schedule runner.
//!
//! A [`Schedule`] is a named, static list of [`Task`]s plus an interrupt
//! mask. [`ScheduleRunner`] advances one task per tick, checking the mask
//! against the current [`Conditions`] first, and reports why it stopped.
//!
//! The *concept* — a monster runs one schedule at a time, a schedule is a
//! list of tasks, an interrupt condition abandons it — is the published
//! vocabulary from the TWHL "Monsters Programming" concept pages. **The task
//! set, the schedule contents, the interrupt masks and the runner's rules
//! below are this project's own design.** No SDK schedule or task table was
//! consulted, transcribed or adapted; see `docs/CLEAN_ROOM.md`.

use crate::rng::Pcg32;
use crate::state::{Conditions, MonsterState};

/// The animation intent a task sets; the renderer maps it to a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Activity {
    /// Standing around.
    #[default]
    Idle,
    /// Standing around, but aware.
    Alert,
    /// Walking.
    Walk,
    /// Running.
    Run,
    /// Crouched.
    Crouch,
    /// Threatening without attacking.
    Threat,
    /// Swinging or biting.
    Melee,
    /// Firing.
    Range,
    /// Reloading.
    Reload,
    /// Reacting to damage.
    Flinch,
    /// Moving into cover.
    Cover,
    /// Dying.
    Die,
}

impl Activity {
    /// A stable byte tag for determinism hashes and save files.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Alert => 1,
            Self::Walk => 2,
            Self::Run => 3,
            Self::Crouch => 4,
            Self::Threat => 5,
            Self::Melee => 6,
            Self::Range => 7,
            Self::Reload => 8,
            Self::Flinch => 9,
            Self::Cover => 10,
            Self::Die => 11,
        }
    }
}

/// One step of a schedule.
///
/// Tasks are deliberately small and orthogonal so schedules read as
/// behaviour. Timing tasks are executed by [`ScheduleRunner`] itself;
/// everything else is handed to a [`TaskExecutor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Task {
    /// Stand still for a fixed number of seconds.
    Wait(f32),
    /// Stand still for a uniformly random number of seconds in `[min, max)`.
    WaitRandom {
        /// The shortest wait, in seconds.
        min: f32,
        /// The exclusive longest wait, in seconds.
        max: f32,
    },
    /// Wait until the current route is finished (or fails).
    WaitForMovement,
    /// Turn to face the acquired enemy.
    FaceEnemy,
    /// Turn to face the current move target.
    FaceTarget,
    /// Turn to face the last known enemy position.
    FaceLastKnownPosition,
    /// Build a route to the acquired enemy and stop `within` units short.
    MoveToEnemy {
        /// How close to get, in world units.
        within: f32,
    },
    /// Build a route to the current move target and stop `within` short.
    MoveToTarget {
        /// How close to get, in world units.
        within: f32,
    },
    /// Build a route to the last known enemy position.
    MoveToLastKnownPosition,
    /// Build a route to a node-graph node (package 7.6 fills the graph in;
    /// today this is a straight line to the node's recorded position).
    MoveToNode(u32),
    /// Follow the current route at running speed.
    RunPath,
    /// Follow the current route at walking speed.
    WalkPath,
    /// Stop following the route.
    StopMoving,
    /// Play a named animation sequence to completion.
    PlaySequence(&'static str),
    /// Set the animation intent.
    SetActivity(Activity),
    /// Choose a cover position away from the enemy and store it as the move
    /// target.
    FindCover,
    /// Move to the stored cover position.
    TakeCover,
    /// Perform the primary melee attack.
    MeleeAttack1,
    /// Perform the secondary melee attack.
    MeleeAttack2,
    /// Perform the primary ranged attack.
    RangeAttack1,
    /// Perform the secondary ranged attack.
    RangeAttack2,
    /// Reload.
    Reload,
    /// Emit a sound of the given kind with the given radius.
    EmitSound(crate::senses::SoundKind, f32),
    /// Switch to another monster state.
    SetState(MonsterState),
    /// Forget the acquired enemy.
    ClearEnemy,
    /// Die.
    Die,
    /// Fail immediately; useful as the body of a "cannot do this" schedule.
    Fail,
}

impl Task {
    /// Whether [`ScheduleRunner`] executes this task itself rather than
    /// delegating it to a [`TaskExecutor`].
    #[must_use]
    pub const fn is_timing_task(self) -> bool {
        matches!(self, Self::Wait(_) | Self::WaitRandom { .. })
    }

    /// A stable discriminant byte for determinism hashes and save files.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Wait(_) => 0,
            Self::WaitRandom { .. } => 1,
            Self::WaitForMovement => 2,
            Self::FaceEnemy => 3,
            Self::FaceTarget => 4,
            Self::FaceLastKnownPosition => 5,
            Self::MoveToEnemy { .. } => 6,
            Self::MoveToTarget { .. } => 7,
            Self::MoveToLastKnownPosition => 8,
            Self::MoveToNode(_) => 9,
            Self::RunPath => 10,
            Self::WalkPath => 11,
            Self::StopMoving => 12,
            Self::PlaySequence(_) => 13,
            Self::SetActivity(_) => 14,
            Self::FindCover => 15,
            Self::TakeCover => 16,
            Self::MeleeAttack1 => 17,
            Self::MeleeAttack2 => 18,
            Self::RangeAttack1 => 19,
            Self::RangeAttack2 => 20,
            Self::Reload => 21,
            Self::EmitSound(_, _) => 22,
            Self::SetState(_) => 23,
            Self::ClearEnemy => 24,
            Self::Die => 25,
            Self::Fail => 26,
        }
    }
}

/// A named list of tasks with the conditions that abandon it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Schedule {
    /// A stable identifier, used by events, save files and diagnostics.
    /// Schedule identity is a string precisely so adding a schedule cannot
    /// invalidate an existing save.
    pub name: &'static str,
    /// The tasks, run in order.
    pub tasks: &'static [Task],
    /// Any of these conditions abandons the schedule at the start of a tick.
    pub interrupt_mask: Conditions,
}

impl Schedule {
    /// A schedule with the given name, tasks and interrupt mask.
    #[must_use]
    pub const fn new(
        name: &'static str,
        tasks: &'static [Task],
        interrupt_mask: Conditions,
    ) -> Self {
        Self {
            name,
            tasks,
            interrupt_mask,
        }
    }

    /// Whether `conditions` interrupts this schedule.
    #[must_use]
    pub const fn is_interrupted_by(&self, conditions: Conditions) -> bool {
        conditions.intersects(self.interrupt_mask)
    }
}

/// How one task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Still going; call again next tick.
    Running,
    /// Finished successfully; advance to the next task.
    Complete,
    /// Could not be done; abandon the schedule.
    Failed,
}

/// Executes the tasks the runner does not handle itself.
///
/// `begin` is called once when a task starts and `resume` on every later
/// tick, so an implementation can set up movement in `begin` and poll for
/// arrival in `resume`.
pub trait TaskExecutor {
    /// Starts `task`.
    fn begin(&mut self, task: &Task) -> TaskStatus;

    /// Continues a task that previously returned [`TaskStatus::Running`].
    ///
    /// The default implementation completes immediately, which is right for
    /// executors whose tasks are all instantaneous.
    fn resume(&mut self, task: &Task, dt: f32) -> TaskStatus {
        let _ = (task, dt);
        TaskStatus::Complete
    }
}

/// Why [`ScheduleRunner::tick`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// A task is still running; nothing to decide.
    Running,
    /// A task completed and the next one starts on the following tick.
    Advanced,
    /// The interrupt mask fired; select a new schedule.
    Interrupted,
    /// Every task completed; select a new schedule.
    Done,
    /// A task failed; select a new schedule, usually a failure schedule.
    Failed,
    /// There is no schedule to run; select one.
    Idle,
}

impl RunOutcome {
    /// Whether the caller must pick a new schedule.
    #[must_use]
    pub const fn needs_new_schedule(self) -> bool {
        matches!(
            self,
            Self::Interrupted | Self::Done | Self::Failed | Self::Idle
        )
    }

    /// The condition this outcome contributes to the next tick's bitset.
    #[must_use]
    pub const fn condition(self) -> Conditions {
        match self {
            Self::Done => Conditions::SCHEDULE_DONE,
            Self::Failed => Conditions::TASK_FAILED,
            Self::Running | Self::Advanced | Self::Interrupted | Self::Idle => Conditions::EMPTY,
        }
    }
}

/// Runs one schedule at a time.
///
/// The runner owns the task cursor, the "has this task been started" flag
/// and the wait timer, and nothing else; all monster state lives outside it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScheduleRunner {
    schedule: Option<&'static Schedule>,
    task_index: usize,
    started: bool,
    timer: f32,
}

impl ScheduleRunner {
    /// A runner with no schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The running schedule, if any.
    #[must_use]
    pub fn schedule(&self) -> Option<&'static Schedule> {
        self.schedule
    }

    /// The running schedule's name, or `""`.
    #[must_use]
    pub fn schedule_name(&self) -> &'static str {
        self.schedule.map_or("", |schedule| schedule.name)
    }

    /// The index of the task being run.
    #[must_use]
    pub fn task_index(&self) -> usize {
        self.task_index
    }

    /// The task being run, if any.
    #[must_use]
    pub fn task(&self) -> Option<Task> {
        self.schedule
            .and_then(|schedule| schedule.tasks.get(self.task_index))
            .copied()
    }

    /// The remaining wait, in seconds; zero outside a timing task.
    #[must_use]
    pub fn timer(&self) -> f32 {
        self.timer
    }

    /// Whether a schedule is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.schedule.is_some()
    }

    /// Starts `schedule` from its first task, discarding any previous one.
    pub fn start(&mut self, schedule: &'static Schedule) {
        self.schedule = Some(schedule);
        self.task_index = 0;
        self.started = false;
        self.timer = 0.0;
    }

    /// Abandons the running schedule.
    pub fn clear(&mut self) {
        self.schedule = None;
        self.task_index = 0;
        self.started = false;
        self.timer = 0.0;
    }

    /// Advances the schedule by one tick.
    ///
    /// The interrupt mask is checked first, so a schedule never runs a task
    /// during the tick that interrupts it. `rng` seeds
    /// [`Task::WaitRandom`]; passing the same generator state and the same
    /// inputs always produces the same outcome.
    pub fn tick(
        &mut self,
        dt: f32,
        conditions: Conditions,
        rng: &mut Pcg32,
        executor: &mut impl TaskExecutor,
    ) -> RunOutcome {
        let Some(schedule) = self.schedule else {
            return RunOutcome::Idle;
        };
        if schedule.is_interrupted_by(conditions) {
            self.clear();
            return RunOutcome::Interrupted;
        }
        let Some(&task) = schedule.tasks.get(self.task_index) else {
            self.clear();
            return RunOutcome::Done;
        };

        let status = if task.is_timing_task() {
            self.tick_timing_task(task, dt, rng)
        } else if self.started {
            executor.resume(&task, dt)
        } else {
            self.started = true;
            executor.begin(&task)
        };

        match status {
            TaskStatus::Running => RunOutcome::Running,
            TaskStatus::Failed => {
                self.clear();
                RunOutcome::Failed
            }
            TaskStatus::Complete => {
                self.task_index += 1;
                self.started = false;
                self.timer = 0.0;
                if self.task_index >= schedule.tasks.len() {
                    self.clear();
                    RunOutcome::Done
                } else {
                    RunOutcome::Advanced
                }
            }
        }
    }

    fn tick_timing_task(&mut self, task: Task, dt: f32, rng: &mut Pcg32) -> TaskStatus {
        if !self.started {
            self.started = true;
            self.timer = match task {
                Task::Wait(seconds) => seconds.max(0.0),
                Task::WaitRandom { min, max } => rng.range_f32(min.max(0.0), max.max(0.0)),
                _ => 0.0,
            };
            if !self.timer.is_finite() {
                return TaskStatus::Failed;
            }
        }
        self.timer -= dt.max(0.0);
        if self.timer > 0.0 {
            TaskStatus::Running
        } else {
            TaskStatus::Complete
        }
    }
}

/// Chooses the schedule a monster runs, given its state and conditions.
///
/// One implementation per monster kind; package 7.7 adds the concrete ones.
pub trait Brain: Send + Sync {
    /// The monster's faction.
    fn classification(&self) -> crate::state::Classification;

    /// The monster's sensory parameters.
    fn senses(&self) -> crate::senses::Senses {
        crate::senses::Senses::default()
    }

    /// The schedule to run now.
    fn select_schedule(&self, state: MonsterState, conditions: Conditions) -> &'static Schedule;

    /// The state to be in now.
    ///
    /// The default is the crate's own general transition rule
    /// ([`crate::brain::default_next_state`]); override it for monsters that
    /// behave differently.
    fn next_state(&self, state: MonsterState, conditions: Conditions) -> MonsterState {
        crate::brain::default_next_state(state, conditions)
    }

    /// How much damage in one tick counts as [`Conditions::HEAVY_DAMAGE`]
    /// rather than [`Conditions::LIGHT_DAMAGE`].
    ///
    /// **Provisional**: 20 points, to be black-box observed.
    fn heavy_damage_threshold(&self) -> f32 {
        20.0
    }

    /// The reach of the primary melee attack, in world units.
    ///
    /// **Provisional**: 64 units, to be black-box observed.
    fn melee_range(&self) -> f32 {
        64.0
    }

    /// Whether this monster has a melee attack at all.
    ///
    /// Senses only raise [`Conditions::CAN_MELEE_ATTACK1`] when it does.
    fn has_melee_attack(&self) -> bool {
        true
    }

    /// Whether this monster has a ranged attack at all.
    ///
    /// Senses only raise [`Conditions::CAN_RANGE_ATTACK1`] when it does.
    fn has_range_attack(&self) -> bool {
        false
    }

    /// The reach of the primary ranged attack, in world units.
    ///
    /// **Provisional**: 1024 units, to be black-box observed.
    fn range_attack_range(&self) -> f32 {
        1_024.0
    }

    /// Walking and running speeds, in units per second.
    ///
    /// **Provisional**: 40 and 160, to be black-box observed.
    fn speeds(&self) -> (f32, f32) {
        (40.0, 160.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Activity, RunOutcome, Schedule, ScheduleRunner, Task, TaskExecutor, TaskStatus};
    use crate::rng::Pcg32;
    use crate::state::Conditions;

    #[derive(Default)]
    struct Recorder {
        begun: Vec<u8>,
        resumed: usize,
        running_for: usize,
        fail_next: bool,
    }

    impl TaskExecutor for Recorder {
        fn begin(&mut self, task: &Task) -> TaskStatus {
            self.begun.push(task.tag());
            if self.fail_next {
                return TaskStatus::Failed;
            }
            if self.running_for > 0 {
                TaskStatus::Running
            } else {
                TaskStatus::Complete
            }
        }

        fn resume(&mut self, _task: &Task, _dt: f32) -> TaskStatus {
            self.resumed += 1;
            self.running_for -= 1;
            if self.running_for == 0 {
                TaskStatus::Complete
            } else {
                TaskStatus::Running
            }
        }
    }

    static PATROL: Schedule = Schedule::new(
        "test/patrol",
        &[
            Task::SetActivity(Activity::Walk),
            Task::WalkPath,
            Task::SetActivity(Activity::Idle),
        ],
        Conditions::SEE_ENEMY,
    );

    static NAP: Schedule = Schedule::new("test/nap", &[Task::Wait(0.05)], Conditions::EMPTY);

    #[test]
    fn an_empty_runner_is_idle() {
        let mut runner = ScheduleRunner::new();
        let mut rng = Pcg32::new(1);
        let mut executor = Recorder::default();
        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Idle
        );
        assert!(!runner.is_running());
        assert_eq!(runner.schedule_name(), "");
        assert!(runner.task().is_none());
    }

    #[test]
    fn tasks_run_in_order_and_the_schedule_finishes() {
        let mut runner = ScheduleRunner::new();
        runner.start(&PATROL);
        assert_eq!(runner.schedule_name(), "test/patrol");
        let mut rng = Pcg32::new(1);
        let mut executor = Recorder::default();

        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Advanced
        );
        assert_eq!(runner.task_index(), 1);
        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Advanced
        );
        let outcome = runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor);
        assert_eq!(outcome, RunOutcome::Done);
        assert!(outcome.needs_new_schedule());
        assert_eq!(outcome.condition(), Conditions::SCHEDULE_DONE);
        assert_eq!(executor.begun.len(), 3);
        assert!(!runner.is_running());
    }

    #[test]
    fn a_running_task_is_resumed_until_it_completes() {
        let mut runner = ScheduleRunner::new();
        runner.start(&PATROL);
        let mut rng = Pcg32::new(1);
        let mut executor = Recorder {
            running_for: 3,
            ..Recorder::default()
        };
        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Running
        );
        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Running
        );
        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Running
        );
        assert_eq!(
            runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor),
            RunOutcome::Advanced
        );
        assert_eq!(executor.resumed, 3);
    }

    #[test]
    fn the_interrupt_mask_abandons_the_schedule_before_running_a_task() {
        let mut runner = ScheduleRunner::new();
        runner.start(&PATROL);
        let mut rng = Pcg32::new(1);
        let mut executor = Recorder::default();
        let outcome = runner.tick(0.01, Conditions::SEE_ENEMY, &mut rng, &mut executor);
        assert_eq!(outcome, RunOutcome::Interrupted);
        assert!(executor.begun.is_empty());
        assert!(!runner.is_running());
        assert!(PATROL.is_interrupted_by(Conditions::SEE_ENEMY));
        assert!(!PATROL.is_interrupted_by(Conditions::HEAR_SOUND));
    }

    #[test]
    fn a_failed_task_abandons_the_schedule() {
        let mut runner = ScheduleRunner::new();
        runner.start(&PATROL);
        let mut rng = Pcg32::new(1);
        let mut executor = Recorder {
            fail_next: true,
            ..Recorder::default()
        };
        let outcome = runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor);
        assert_eq!(outcome, RunOutcome::Failed);
        assert_eq!(outcome.condition(), Conditions::TASK_FAILED);
    }

    #[test]
    fn the_runner_times_waits_itself() {
        let mut runner = ScheduleRunner::new();
        runner.start(&NAP);
        let mut rng = Pcg32::new(1);
        let mut executor = Recorder::default();
        let mut ticks = 0;
        loop {
            let outcome = runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor);
            ticks += 1;
            if outcome == RunOutcome::Done {
                break;
            }
            assert_eq!(outcome, RunOutcome::Running);
            assert!(ticks < 100);
        }
        assert_eq!(ticks, 5);
        assert!(executor.begun.is_empty(), "Wait is not delegated");
    }

    #[test]
    fn random_waits_are_reproducible_and_bounded() {
        static DOZE: Schedule = Schedule::new(
            "test/doze",
            &[Task::WaitRandom { min: 0.1, max: 0.3 }],
            Conditions::EMPTY,
        );
        let run = |seed| {
            let mut runner = ScheduleRunner::new();
            runner.start(&DOZE);
            let mut rng = Pcg32::new(seed);
            let mut executor = Recorder::default();
            let mut ticks = 0;
            while runner.tick(0.01, Conditions::EMPTY, &mut rng, &mut executor) != RunOutcome::Done
            {
                ticks += 1;
                assert!(ticks < 100);
            }
            ticks + 1
        };
        let first = run(99);
        assert_eq!(first, run(99));
        assert!((10..=30).contains(&first), "{first} ticks is out of range");
    }

    #[test]
    fn every_task_tag_is_distinct() {
        let tasks = [
            Task::Wait(0.0),
            Task::WaitRandom { min: 0.0, max: 1.0 },
            Task::WaitForMovement,
            Task::FaceEnemy,
            Task::FaceTarget,
            Task::FaceLastKnownPosition,
            Task::MoveToEnemy { within: 0.0 },
            Task::MoveToTarget { within: 0.0 },
            Task::MoveToLastKnownPosition,
            Task::MoveToNode(0),
            Task::RunPath,
            Task::WalkPath,
            Task::StopMoving,
            Task::PlaySequence("x"),
            Task::SetActivity(Activity::Idle),
            Task::FindCover,
            Task::TakeCover,
            Task::MeleeAttack1,
            Task::MeleeAttack2,
            Task::RangeAttack1,
            Task::RangeAttack2,
            Task::Reload,
            Task::EmitSound(crate::senses::SoundKind::Combat, 1.0),
            Task::SetState(crate::state::MonsterState::Idle),
            Task::ClearEnemy,
            Task::Die,
            Task::Fail,
        ];
        let mut tags: Vec<u8> = tasks.iter().map(|task| task.tag()).collect();
        let count = tags.len();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), count);
        assert_eq!(Activity::Die.tag(), 11);
    }
}
