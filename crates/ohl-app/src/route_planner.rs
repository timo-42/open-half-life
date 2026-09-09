//! Turns [`ohl_engine::route_plan`]'s planned route into a scripted-input
//! file, and refuses to write one that does not actually work.
//!
//! The engine plans in world terms (a heading, a distance, a door's own
//! open time); the scripted-input grammar (`crate::script`) is this
//! crate's, so the conversion lives here:
//!
//! - a heading becomes a `look 0 <degrees>` line, always a *relative*
//!   turn from the yaw the player is actually facing, tracked line by
//!   line exactly the way the parser applies it;
//! - a distance becomes `<n> forward`, with `n` from a one-dimensional
//!   replay of the engine's own ground-acceleration rule
//!   ([`ticks_for_distance`]) rather than from "distance over top speed",
//!   which a player starting from rest never achieves;
//! - a door becomes a turn, a `1 use` press, and a wait as long as the
//!   door's own documented open time.
//!
//! # The closed loop
//!
//! A planned script is a guess: the walk's grid path is not a motion
//! model, and a run that clips a corner arrives somewhere the next
//! segment was not planned from. So nothing is written until the script
//! has been *replayed*, in-process, from the very state it was planned
//! from — a [`ohl_engine::GameSave`] snapshot, restored fresh for every
//! attempt — and the replay actually reached a level change. When it does
//! not, the planner restores that same snapshot, runs the script it has
//! so far, and plans again *from where the player actually ended up*,
//! appending the continuation ([`refine`]). That is the whole point: the
//! drift a hand-authored route hides is exactly what this loop measures
//! and corrects, up to [`PlanOptions::attempts`] times.
//!
//! The accepted script is then replayed once more on the caller's own
//! live `Game` — the state a snapshot round trip can only approximate —
//! and only written if it fires there too.
//!
//! # What is written, and what is printed
//!
//! The output file holds script commands and project-authored comment
//! words. Not one coordinate, entity name or map name from the planner's
//! search reaches it, or any log line: the caller prints aggregates only
//! (`docs/CLEAN_ROOM.md`).

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::Path;

use ohl_engine::{
    AssetSource, Game, GameConfig, GameEvent, GameSave, PlanAction, PlanConfig, PlanError,
    PlanRejection, RoutePlan, TICK_SECONDS,
};

use ohl_physics::MoveConfig;

use crate::game_run::CAPTURE_STEP;
use crate::script::Script;

/// How many ticks of standing still one settle round waits out (five
/// simulated seconds), when a search from the current state finds no
/// route at all — see [`PlanOptions::settle_rounds`].
pub const SETTLE_ROUND_TICKS: u32 = 300;

/// How many settle rounds [`PlanOptions::default`] allows: two minutes of
/// simulated waiting, in five-second steps.
pub const DEFAULT_SETTLE_ROUNDS: usize = 24;

/// How many plan/replay attempts [`PlanOptions::default`] allows.
pub const DEFAULT_ATTEMPTS: usize = 24;

/// The most plan/replay attempts a caller may ask for. Each one replays
/// the whole accumulated script twice, so an unbounded count would make a
/// single planner run unbounded work.
pub const MAX_ATTEMPTS: usize = 96;

/// How many of a plan's own segments one attempt commits to the script
/// before the loop replays and plans again ([`PlanOptions::segments_per_attempt`]).
pub const DEFAULT_SEGMENTS_PER_ATTEMPT: usize = 1;

/// The most segments one attempt may commit, so a caller cannot ask for
/// a script that is one long open loop.
pub const MAX_SEGMENTS_PER_ATTEMPT: usize = 16;

/// How many ticks a turn-in-place line is given. Long enough that the
/// turn is a smooth sweep rather than a snap (`look` spreads its
/// degrees evenly across its own line), short enough to cost nothing.
const TURN_TICKS: u32 = 6;

/// How many extra ticks a door wait is padded by, on top of the door's
/// own open time: the press itself takes a tick, and a leaf that has
/// only just finished moving is still worth a moment's slack.
const DOOR_WAIT_PADDING_TICKS: u32 = 12;

/// How many ticks of standing still every planned chunk ends with, so a
/// trigger volume the last run has just entered gets a tick to fire in
/// and any overshoot settles before the next attempt plans from here.
const SETTLE_TICKS: u32 = 30;

/// How many extra ticks a ladder climb is padded by; see
/// [`ticks_for_climb`].
const CLIMB_PADDING_TICKS: u32 = 12;

/// How many extra ticks a planned fall's wait is padded by, on top of the
/// fall's own time: the player leaves the ledge a moment after the run's
/// last tick, and a landing is worth a moment's slack of its own.
const FALL_WAIT_PADDING_TICKS: u32 = 12;

/// The most ticks one `forward` line may schedule, so a nonsensical
/// distance cannot produce a script that runs for hours.
const MAX_SEGMENT_TICKS: u32 = 6_000;

/// The comment block every written route carries. Project-authored words
/// only; see this module's own doc comment.
const HEADER: &str = "\
# A machine-planned chain-walk route (--plan-route / cargo xtask
# plan-chain-hop), not a hand-authored one.
#
# A bounded, cost-ordered walk over this map's live collision model
# (ohl_engine::route_plan, the same edge model --reachability-report
# triages with) found a path from the arrival point this route starts at
# to the map's own level-change trigger, straightened it into runs,
# converted the runs into the lines below, and replayed them in-process
# until the level change actually fired. The route below is the one that
# fired; nothing else was written.
#
# Commands only: no name, path or coordinate from any payload appears
# here, and none was ever printed (docs/CLEAN_ROOM.md). The planner's own
# search coordinates never left the process that produced this file.
#
# A line's leading number is a count of ticks, each one CAPTURE_STEP
# (1/60 s) of simulated time -- not seconds.
";

/// How to plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanOptions {
    /// How many plan/replay attempts the closed loop may take, bounded by
    /// [`MAX_ATTEMPTS`].
    pub attempts: usize,
    /// How many times a failed search may wait [`SETTLE_ROUND_TICKS`] and
    /// look again before giving up.
    ///
    /// A map that opens its own way out on a schedule — an arrival
    /// sequence that ends by opening the car the player rode in on, a
    /// door another character opens once the player is standing at it —
    /// has no route at all from the state the planner first sees, and one
    /// a few seconds later. Waiting is the one thing a script can always
    /// do, so a search that finds nothing waits and looks again rather
    /// than concluding the map is shut.
    ///
    /// This is an *optimistic* assumption, and it is safe here for the
    /// same reason the rest of the planner's guesses are: nothing is
    /// written until the whole script has been replayed and the level
    /// change actually fired. A wait that changed nothing simply costs
    /// the replay a few idle seconds and the search still fails.
    pub settle_rounds: usize,
    /// How many of a plan's walk-forward segments one attempt commits
    /// before replaying and planning again; `0` commits the whole plan.
    ///
    /// A planned segment is an *open-loop* guess: nothing corrects the
    /// player's course while a `forward` line runs, so a run that clips a
    /// corner leaves every later segment aimed from the wrong place, and
    /// an eleven-segment route committed in one go drifts eleven
    /// segments' worth. Committing a few at a time and re-planning from
    /// where the player actually ended up is what makes this a closed
    /// loop rather than a long open one — and it costs only one more
    /// search per handful of segments.
    pub segments_per_attempt: usize,
    /// The engine-side search configuration.
    pub plan: PlanConfig,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_ATTEMPTS,
            settle_rounds: DEFAULT_SETTLE_ROUNDS,
            segments_per_attempt: DEFAULT_SEGMENTS_PER_ATTEMPT,
            plan: PlanConfig::default(),
        }
    }
}

/// A validated route: the script text, and the bounded aggregates a
/// caller may report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRoute {
    /// The script lines, without the comment header.
    pub text: String,
    /// How many plan/replay attempts it took.
    pub attempts: usize,
    /// How many grid cells the last search reached.
    pub cells: usize,
    /// How many walk-forward segments the script holds.
    pub segments: usize,
    /// How many ladder climbs the script holds, counted from the planned
    /// actions: a climb up is a held `forward` in the text, the same line
    /// a walk-forward run emits.
    pub climbs: usize,
    /// How many door presses the script holds.
    pub doors: usize,
    /// How many ticks the script schedules.
    pub ticks: u64,
}

impl PlannedRoute {
    /// How many seconds of simulated time the script schedules.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        #[allow(clippy::cast_precision_loss, reason = "a tick count for a report line")]
        let ticks = self.ticks as f32;
        ticks * CAPTURE_STEP
    }
}

/// Why a route could not be planned, converted, validated or written.
/// Every variant has one fixed message carrying nothing map-derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFailure {
    /// The engine's own search failed; see [`PlanRejection`].
    Search(PlanRejection),
    /// The planner's state snapshot could not be restored.
    Restore,
    /// The generated script did not parse, or exceeded the grammar's own
    /// documented limits.
    ScriptRejected,
    /// A planning attempt produced no further commands at all, so the
    /// loop could make no progress.
    NoProgress,
    /// Every attempt was spent without a replay reaching the goal.
    NeverReached,
    /// The accepted script did not reach the goal on the caller's own
    /// live game, only on the restored snapshot.
    LiveReplayFailed,
    /// The route so far left the player dead. A corpse does not walk, so
    /// every further plan is the plan this one already was: there is
    /// nothing left for the loop to try, and saying so is more honest
    /// than spending every remaining attempt on a state that cannot
    /// move.
    PlayerDied,
    /// The route file could not be written.
    Unwritable,
}

impl std::fmt::Display for PlanFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Search(error) => write!(formatter, "the route search failed: {error}"),
            Self::Restore => {
                formatter.write_str("the planner's state snapshot could not be restored")
            }
            Self::ScriptRejected => {
                formatter.write_str("the planned script did not parse within the grammar's limits")
            }
            Self::NoProgress => {
                formatter.write_str("a planning attempt produced no further commands")
            }
            Self::NeverReached => {
                formatter.write_str("no replay of the planned script reached the goal")
            }
            Self::LiveReplayFailed => formatter
                .write_str("the planned script reached the goal only on a restored snapshot"),
            Self::PlayerDied => formatter.write_str("the route walked so far left the player dead"),
            Self::Unwritable => formatter.write_str("the route file could not be written"),
        }
    }
}

impl std::error::Error for PlanFailure {}

/// The shortest signed turn, in degrees, from `from` to `to`.
#[must_use]
pub fn shortest_turn(from: f32, to: f32) -> f32 {
    let delta = (to - from).rem_euclid(360.0);
    if delta > 180.0 { delta - 360.0 } else { delta }
}

/// How far the player coasts to a halt from `speed`, by replaying the
/// engine's own ground-friction rule
/// (`ohl_physics::MoveConfig::friction`/`stop_speed`) one tick at a time.
///
/// A `forward` line is a *held key*: the tick it stops on is not the tick
/// the player stops on. Releasing at top speed still carries them the
/// better part of a corridor's width, which is why a run planned by
/// acceleration alone always ends past the point it was planned for.
#[must_use]
pub fn coast_distance(speed: f32, config: &MoveConfig) -> f32 {
    if !speed.is_finite() || speed <= 0.0 {
        return 0.0;
    }
    let step = TICK_SECONDS;
    let mut speed = speed;
    let mut travelled = 0.0f32;
    // Bounded by the friction rule itself: `stop_speed * friction` is a
    // fixed floor on the per-tick drop, so this cannot run long. The
    // guard is there so a degenerate config cannot loop forever.
    for _ in 0..MAX_SEGMENT_TICKS {
        if speed < 0.1 {
            break;
        }
        let drop = speed.max(config.stop_speed) * config.friction * step;
        speed = (speed - drop).max(0.0);
        travelled += speed * step;
    }
    travelled
}

/// How many script ticks of held `forward` it takes to *stop* `distance`
/// world units away, by replaying the engine's own ground move in one
/// dimension from a standstill: friction, then acceleration toward
/// `max_speed`, then the coast the release leaves behind
/// ([`coast_distance`]).
///
/// Both halves matter, and for opposite reasons. "Distance over top
/// speed" undershoots, because a player starting from rest never travels
/// at top speed; counting only the held ticks *overshoots*, because the
/// player keeps sliding once the key is released. A route is re-planned
/// from wherever the player actually stands, so an undershoot costs one
/// more segment — but an overshoot is what walks a planned route off a
/// ledge, and the walk this route came from never planned the fall.
///
/// So the key is released on the last tick from which the coast still
/// lands short of `distance`.
#[must_use]
pub fn ticks_for_distance(distance: f32, config: &MoveConfig) -> u32 {
    if !(distance.is_finite() && distance > 0.0) || config.max_speed <= 0.0 {
        return 0;
    }
    let step = TICK_SECONDS;
    let mut speed = 0.0f32;
    let mut travelled = 0.0f32;
    let mut ticks = 0u32;
    while ticks < MAX_SEGMENT_TICKS {
        // Would one more held tick still leave the player stopping short
        // of the target? A tick is only taken when the answer is yes: the
        // key is released on the last tick whose own coast lands inside
        // the distance, never on the first one that lands past it.
        let drop = speed.max(config.stop_speed) * config.friction * step;
        let next_speed = ((speed - drop).max(0.0) + config.accelerate * config.max_speed * step)
            .min(config.max_speed);
        let next_travelled = travelled + next_speed * step;
        if ticks > 0 && next_travelled + coast_distance(next_speed, config) > distance {
            break;
        }
        speed = next_speed;
        travelled = next_travelled;
        ticks += 1;
    }
    ticks.clamp(1, MAX_SEGMENT_TICKS)
}

/// How many script ticks of held `forward`/`back` it takes to climb
/// `distance` world units along a ladder.
///
/// A climb has no acceleration and no friction to model: the engine's
/// ladder step *sets* the velocity to the climb speed
/// (`ohl_physics::MoveConfig::ladder_speed`) rather than accelerating
/// toward it, so the count is the distance over that speed — the one
/// place where the naive estimate is the right one. A few ticks of slack
/// are added because a climb that stops short leaves the player hanging
/// where the next segment cannot be walked from, while one that runs on
/// simply presses them against the top or the foot of the ladder.
#[must_use]
pub fn ticks_for_climb(distance: f32, config: &MoveConfig) -> u32 {
    if !(distance.is_finite() && distance > 0.0) || config.ladder_speed <= 0.0 {
        return 0;
    }
    let seconds = distance / config.ladder_speed;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a tick count clamped just below"
    )]
    let ticks = (seconds / CAPTURE_STEP).ceil() as u32;
    ticks
        .saturating_add(CLIMB_PADDING_TICKS)
        .clamp(1, MAX_SEGMENT_TICKS)
}

/// How many script ticks a fall of `height` world units takes, by the
/// engine's own gravity (`ohl_physics::MoveConfig::gravity`), plus a
/// margin.
///
/// A `forward` line stops when its ticks run out, not when the player
/// lands: a run that ends by stepping off a ledge leaves them in the air,
/// and the next line — a turn, another run, a `use` press — would run
/// while they are still falling, from a place the plan never described.
/// The plan says how far that fall is (`PlanAction::Move::fall`, measured
/// by the walk that planned it), and a fall from `h` under constant
/// gravity takes `sqrt(2h/g)`.
#[must_use]
pub fn ticks_for_fall(height: f32, config: &MoveConfig) -> u32 {
    if !(height.is_finite() && height > 0.0) || config.gravity <= 0.0 {
        return 0;
    }
    let seconds = (2.0 * height / config.gravity).sqrt();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a tick count clamped just below"
    )]
    let ticks = (seconds / CAPTURE_STEP).ceil() as u32;
    ticks
        .saturating_add(FALL_WAIT_PADDING_TICKS)
        .clamp(1, MAX_SEGMENT_TICKS)
}

/// How many ladder climbs `actions` holds — counted from the plan, not
/// from the script text it becomes: a climb *up* is a held `forward`,
/// exactly like a walk-forward run, so the text cannot tell the two
/// apart.
#[must_use]
pub fn count_climb_actions(actions: &[PlanAction]) -> usize {
    actions
        .iter()
        .filter(|action| matches!(action, PlanAction::Climb { .. }))
        .count()
}

/// `actions` truncated after its first ladder climb.
///
/// A climb is the one action whose commands mean something else entirely
/// when the player is not where the plan thinks they are: `back` against
/// a ladder descends it, and `back` on open floor walks away from
/// everything the route just gained. So a climb always ends a committed
/// chunk — the loop replays, sees where the player actually is (on the
/// ladder, at its foot, or still on the ledge) and plans the rest from
/// there.
#[must_use]
pub fn through_first_climb(actions: &[PlanAction]) -> &[PlanAction] {
    match actions
        .iter()
        .position(|action| matches!(action, PlanAction::Climb { .. }))
    {
        Some(index) => &actions[..=index],
        None => actions,
    }
}

/// The first `segments` travelling actions of `actions` (a walk-forward
/// run or a ladder climb), with every door press among them; `0` keeps
/// the whole plan.
#[must_use]
pub fn first_segments(actions: &[PlanAction], segments: usize) -> &[PlanAction] {
    if segments == 0 {
        return actions;
    }
    let mut moves = 0usize;
    for (index, action) in actions.iter().enumerate() {
        if matches!(action, PlanAction::Move { .. } | PlanAction::Climb { .. }) {
            moves += 1;
            if moves >= segments {
                return &actions[..=index];
            }
        }
    }
    actions
}

/// Emits the `look` line that turns from `facing` to `yaw`, and returns
/// the yaw the player is left facing (the *rounded* turn is what the
/// script actually applies, so the tracked yaw has to use it too).
fn turn_toward(lines: &mut String, facing: f32, yaw: f32) -> f32 {
    let delta = shortest_turn(facing, yaw);
    let rounded = (delta * 100.0).round() / 100.0;
    if rounded.abs() < 0.01 {
        return facing;
    }
    let _ = writeln!(lines, "{TURN_TICKS} look 0 {rounded:.2}");
    (facing + rounded).rem_euclid(360.0)
}

/// Converts one planned route into script lines, starting from the yaw
/// the player is currently facing.
///
/// The movement tunables come from the live `ohl_physics::MoveConfig` the
/// game moves by, never restated here.
#[must_use]
pub fn script_text(start_yaw: f32, actions: &[PlanAction], config: &MoveConfig) -> String {
    let mut lines = String::new();
    let mut facing = start_yaw.rem_euclid(360.0);
    for action in actions {
        match *action {
            PlanAction::Move {
                yaw,
                distance,
                jump,
                fall,
            } => {
                facing = turn_toward(&mut lines, facing, yaw);
                let ticks = ticks_for_distance(distance, config);
                if ticks == 0 {
                    continue;
                }
                if jump {
                    let _ = writeln!(lines, "{ticks} forward jump");
                } else {
                    let _ = writeln!(lines, "{ticks} forward");
                }
                // A run that ends by stepping off a ledge is not over
                // when its ticks are: the plan after it was made from
                // where the player lands ([`ticks_for_fall`]).
                let landing = ticks_for_fall(fall, config);
                if landing > 0 {
                    let _ = writeln!(lines, "{landing} wait");
                }
            }
            PlanAction::Climb { yaw, distance, up } => {
                facing = turn_toward(&mut lines, facing, yaw);
                let ticks = ticks_for_climb(distance, config);
                if ticks == 0 {
                    continue;
                }
                // Facing into the ladder, `forward` climbs and `back`
                // descends: the engine's own ladder step resolves the
                // wished-for direction against the volume's outward
                // normal, and this is that rule read back, not a second
                // one (`ohl_physics::movement`).
                if up {
                    let _ = writeln!(lines, "{ticks} forward");
                } else {
                    let _ = writeln!(lines, "{ticks} back");
                }
            }
            PlanAction::UseDoor { yaw, open_seconds } => {
                facing = turn_toward(&mut lines, facing, yaw);
                lines.push_str("1 use\n");
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a door's own open time, clamped just below"
                )]
                let wait = (open_seconds.max(0.0) / CAPTURE_STEP).ceil() as u32;
                let wait = wait
                    .saturating_add(DOOR_WAIT_PADDING_TICKS)
                    .min(MAX_SEGMENT_TICKS);
                let _ = writeln!(lines, "{wait} wait");
            }
        }
    }
    if !lines.is_empty() {
        let _ = writeln!(lines, "{SETTLE_TICKS} wait");
    }
    lines
}

/// The closed loop itself, with its two halves injected: `step` plans a
/// continuation from wherever the script so far leaves the player, and
/// `replay` reports whether the whole script reaches the goal.
///
/// Returns the accepted script and how many attempts it took.
///
/// # Errors
/// [`PlanFailure::NoProgress`] when an attempt adds nothing, and
/// [`PlanFailure::NeverReached`] when `attempts` are spent without a
/// replay reaching the goal; anything `step` itself fails with is passed
/// through.
pub fn refine(
    mut step: impl FnMut(&str) -> Result<String, PlanFailure>,
    mut replay: impl FnMut(&str) -> bool,
    attempts: usize,
) -> Result<(String, usize), PlanFailure> {
    let mut script = String::new();
    for attempt in 1..=attempts.clamp(1, MAX_ATTEMPTS) {
        let appended = step(&script)?;
        if appended.trim().is_empty() {
            return Err(PlanFailure::NoProgress);
        }
        script.push_str(&appended);
        if replay(&script) {
            return Ok((script, attempt));
        }
    }
    Err(PlanFailure::NeverReached)
}

/// Parses accumulated script text, treating "nothing yet" as `None`
/// rather than as the grammar's own "empty script" error.
fn parse(text: &str) -> Result<Option<Script>, PlanFailure> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    Script::parse(text.as_bytes())
        .map(Some)
        .map_err(|_| PlanFailure::ScriptRejected)
}

/// The most settle rounds any caller may ask for, so waiting cannot make
/// a planner run unbounded.
const MAX_SETTLE_ROUNDS: usize = 64;

/// How many ticks a planning attempt waits for the player to come back
/// down before it plans (two simulated seconds). A route may legitimately
/// step off a low ledge; the plan that follows it has to be made from
/// where the player lands, not from the floor they are still falling
/// toward.
const LANDING_TICKS: u32 = 120;

/// Stands still for `ticks`, returning `false` when a level change fired
/// while waiting (nothing left to plan).
fn idle(game: &mut Game, ticks: u32) -> bool {
    let input = ohl_engine::Input::default();
    for _ in 0..ticks {
        for event in game.tick(CAPTURE_STEP, &input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                return false;
            }
        }
    }
    true
}

/// Ticks `script` through `game`, stopping at the first level change (it
/// is never followed: reaching it is the whole answer).
fn run_ticks(game: &mut Game, script: &Script) -> bool {
    for input in script.inputs() {
        for event in game.tick(CAPTURE_STEP, input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                return true;
            }
        }
    }
    false
}

/// Restores a fresh game from the planner's own snapshot.
fn restore(
    source: &dyn AssetSource,
    save: &GameSave,
    config: GameConfig,
) -> Result<Game, PlanFailure> {
    Game::from_save_with(source, save, &config).map_err(|_| PlanFailure::Restore)
}

/// How many `forward` lines a script holds.
fn count_segments(text: &str) -> usize {
    text.lines()
        .filter(|line| line.split_ascii_whitespace().any(|word| word == "forward"))
        .count()
}

/// How many `use` presses a script holds.
fn count_doors(text: &str) -> usize {
    text.lines()
        .filter(|line| line.split_ascii_whitespace().any(|word| word == "use"))
        .count()
}

/// The line one settle round appends: stand still for as long as this
/// round has decided to wait, cumulatively.
fn settle_line(round: usize) -> String {
    format!(
        "{} wait\n",
        SETTLE_ROUND_TICKS * u32::try_from(round + 1).unwrap_or(1)
    )
}

/// Everything one planning attempt works against, so the attempt itself
/// stays a function rather than a closure over half this module.
struct Planner<'a> {
    /// Where the game's own assets come from.
    source: &'a dyn AssetSource,
    /// The state every attempt restores from.
    base: &'a GameSave,
    /// The display settings a restore keeps (difficulty is save state).
    config: GameConfig,
    /// How to plan.
    options: &'a PlanOptions,
    /// The last plan made, for the caller's aggregates.
    last: RefCell<Option<RoutePlan>>,
    /// How many ladder climbs the committed script holds so far, counted
    /// from the actions themselves ([`count_climb_actions`]).
    climbs: RefCell<usize>,
    /// The last chunk of commands committed. A plan that produces the
    /// very same chunk again has stopped making progress — the player
    /// walked it and ended up somewhere it plans identically from — and
    /// the one thing left to try there is waiting.
    previous: RefCell<String>,
}

impl Planner<'_> {
    /// Reports one planning attempt: bounded aggregates only — how far
    /// the search got, how much of a route came out of it, and whether
    /// that route ends at the goal or only closer to it. No coordinate,
    /// entity name or map name (`docs/CLEAN_ROOM.md`).
    fn report(plan: &RoutePlan) {
        tracing::info!(
            "Route plan search: {} cell(s), {} round(s), {} segment(s), {} door press(es), \
~{:.0} units to go, {}.",
            plan.cells,
            plan.rounds,
            plan.segments(),
            plan.doors,
            plan.start_distance_rounded,
            if plan.reaches_goal {
                "reaching the goal"
            } else {
                "partial: closer to the goal only"
            }
        );
    }

    /// Restores the planner's snapshot, runs `prefix` on it and waits out
    /// `settle` further ticks. `None` when the prefix already reached the
    /// goal, leaving nothing to plan.
    fn state_after(&self, prefix: &str, settle: u32) -> Result<Option<Game>, PlanFailure> {
        // Every search runs on a *fresh* restore: the engine's own search
        // detaches the doors it decides to open, so a scratch game that
        // has already been searched once is no longer the state the next
        // search would see.
        let mut scratch = restore(self.source, self.base, self.config)?;
        if let Some(script) = parse(prefix)?
            && run_ticks(&mut scratch, &script)
        {
            return Ok(None);
        }
        if settle > 0 && !idle(&mut scratch, settle) {
            return Ok(None);
        }
        // Never plan in mid-air: the engine's own search settles onto
        // the floor below the player before it walks, so a plan made
        // while they are still falling describes a route from a place
        // they have not arrived at, and the script for it runs during
        // the fall.
        for _ in 0..LANDING_TICKS {
            // A climber never lands: `ohl_physics::movement` deliberately
            // reports no ground while the player is attached to a ladder,
            // so waiting for one here would spend the whole wait every
            // attempt and still plan from a body hanging in a shaft.
            // Hanging on a ladder *is* an arrived state, and
            // `ohl_engine::route_plan` plans from it directly rather than
            // from the floor far below.
            if scratch.player_on_ground() || scratch.player_on_ladder() {
                break;
            }
            if !idle(&mut scratch, 1) {
                return Ok(None);
            }
        }
        // A dead player stands still whatever the script says, so the
        // search from here would plan the very same route again, every
        // attempt, until the loop ran out — see [`PlanFailure::PlayerDied`].
        if scratch.player_health() <= 0.0 {
            return Err(PlanFailure::PlayerDied);
        }
        Ok(Some(scratch))
    }

    /// Plans the continuation from wherever `prefix` leaves the player,
    /// waiting and looking again as [`PlanOptions::settle_rounds`]
    /// allows.
    fn step(&self, prefix: &str) -> Result<String, PlanFailure> {
        let mut waited = String::new();
        let mut last_rejection: Option<PlanRejection> = None;
        for round in 0..=self.options.settle_rounds.min(MAX_SETTLE_ROUNDS) {
            let settle = SETTLE_ROUND_TICKS * u32::try_from(round).unwrap_or(1);
            let Some(mut scratch) = self.state_after(prefix, settle)? else {
                return Ok(String::new());
            };
            let facing = scratch.camera().yaw;
            let move_config = *scratch.move_config();
            let plan = match ohl_engine::plan_route(&mut scratch, &self.options.plan) {
                Ok(plan) => plan,
                Err(rejection) if rejection.error == PlanError::GoalUnreachable => {
                    last_rejection = Some(rejection);
                    waited = settle_line(round);
                    continue;
                }
                Err(rejection) => return Err(PlanFailure::Search(rejection)),
            };
            // A partial plan (`RoutePlan::reaches_goal` false) is still
            // progress: walk it, and the next attempt looks again from
            // there. One that walks nowhere at all, or that plans exactly
            // what the last one did, is not — and is treated like a
            // search that found nothing: wait, and look again.
            //
            // A plan that reaches the goal commits only its first few
            // segments, so the loop can correct the drift the rest of it
            // would otherwise inherit. A partial plan commits whole: its
            // point is to stand at the closest reachable point and look
            // again from there, and stopping part-way along a detour
            // would leave the player somewhere neither closer nor planned
            // for.
            let committed = if plan.reaches_goal {
                first_segments(&plan.actions, self.options.segments_per_attempt)
            } else {
                &plan.actions
            };
            let committed = through_first_climb(committed);
            let text = script_text(facing, committed, &move_config);
            // An identical chunk means the last one changed nothing: the
            // player is somewhere the same plan comes out of, which is
            // what standing in a map's own arrival sequence looks like
            // from here. Appending it again would change nothing either,
            // so wait and look again instead.
            if text.is_empty() || text == *self.previous.borrow() {
                last_rejection = Some(PlanRejection::new(
                    PlanError::GoalUnreachable,
                    plan.cells,
                    plan.rounds,
                ));
                waited = settle_line(round);
                continue;
            }
            Self::report(&plan);
            *self.climbs.borrow_mut() += count_climb_actions(committed);
            self.previous.borrow_mut().clone_from(&text);
            *self.last.borrow_mut() = Some(plan);
            waited.push_str(&text);
            return Ok(waited);
        }
        Err(PlanFailure::Search(last_rejection.take().unwrap_or_else(
            || PlanRejection::new(PlanError::GoalUnreachable, 0, 0),
        )))
    }

    /// Whether the whole accumulated script reaches the goal, replayed
    /// from the planner's own snapshot.
    fn replay(&self, text: &str) -> bool {
        let Ok(Some(script)) = parse(text) else {
            return false;
        };
        let Ok(mut scratch) = restore(self.source, self.base, self.config) else {
            return false;
        };
        run_ticks(&mut scratch, &script)
    }
}

/// Plans, validates and returns a route from `game`'s current state to
/// the goal, without changing `game` other than by the accepted script's
/// own ticks (the final live confirmation this module's doc comment
/// describes).
///
/// # Errors
/// One of [`PlanFailure`]'s fixed reasons.
pub fn plan(
    game: &mut Game,
    source: &dyn AssetSource,
    options: &PlanOptions,
) -> Result<PlannedRoute, PlanFailure> {
    let base = game.to_save(0);
    let planner = Planner {
        source,
        base: &base,
        config: GameConfig {
            difficulty: game.difficulty(),
            overbright: game.overbright(),
        },
        options,
        last: RefCell::new(None),
        previous: RefCell::new(String::new()),
        climbs: RefCell::new(0),
    };

    let (text, attempts) = refine(
        |prefix| planner.step(prefix),
        |text| planner.replay(text),
        options.attempts,
    )?;
    let script = parse(&text)?.ok_or(PlanFailure::NoProgress)?;
    let ticks = script.len() as u64;
    if !run_ticks(game, &script) {
        return Err(PlanFailure::LiveReplayFailed);
    }
    Ok(PlannedRoute {
        segments: count_segments(&text),
        climbs: *planner.climbs.borrow(),
        doors: count_doors(&text),
        cells: planner.last.borrow().as_ref().map_or(0, |plan| plan.cells),
        attempts,
        ticks,
        text,
    })
}

/// Writes a validated route to `path`, header first.
///
/// # Errors
/// [`PlanFailure::Unwritable`] when the file could not be written. The
/// path is never logged: it is caller-supplied, untrusted input like
/// every other path this binary takes.
pub fn write_route(path: &Path, route: &PlannedRoute) -> Result<(), PlanFailure> {
    let mut contents = String::from(HEADER);
    contents.push('\n');
    contents.push_str(&route.text);
    std::fs::write(path, contents).map_err(|_| PlanFailure::Unwritable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ohl_engine::MemoryAssets;
    use ohl_engine::test_support::{
        PLAN_LADDER_MAP, PLAN_TURN_MAP, plan_ladder_bsp, plan_pit_bsp, plan_turn_bsp,
    };

    fn fixture() -> (MemoryAssets, Game) {
        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{PLAN_TURN_MAP}.bsp"),
            plan_turn_bsp("ohlplannext"),
        );
        let game = Game::load(&assets as &dyn AssetSource, PLAN_TURN_MAP).expect("fixture loads");
        (assets, game)
    }

    /// The end-to-end promise: on a corridor with a turn and a closed
    /// door, the planner writes a script that *actually walks it* — the
    /// replay reaches the level change, which is the only reason a route
    /// is ever written.
    #[test]
    fn a_planned_route_replays_to_the_level_change() {
        let (assets, mut game) = fixture();
        let route = plan(
            &mut game,
            &assets as &dyn AssetSource,
            &PlanOptions::default(),
        )
        .expect("the fixture's route plans, replays and validates");
        assert!(route.segments >= 2, "a turn means at least two runs");
        assert_eq!(route.doors, 1, "the corridor's one door is pressed");
        assert!(route.cells > 1);
        assert!(route.ticks > 0);
        assert!(route.seconds() > 0.0);
        assert!(route.text.contains(" use\n"), "the script presses the door");
        assert!(route.text.contains("look 0 "), "the script turns");
        Script::parse(route.text.as_bytes()).expect("the written script parses");
    }

    /// A route is written only after a replay reached the goal, so the
    /// written file must itself replay from the same start.
    #[test]
    fn the_written_file_is_the_script_that_replayed() {
        let (assets, mut game) = fixture();
        let route = plan(
            &mut game,
            &assets as &dyn AssetSource,
            &PlanOptions::default(),
        )
        .expect("the fixture's route plans");
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("planned.txt");
        write_route(&path, &route).expect("the route writes");
        let written = std::fs::read_to_string(&path).expect("the route reads back");
        assert!(written.starts_with("# A machine-planned"));
        assert!(written.ends_with(&route.text));
        let script = Script::parse(written.as_bytes()).expect("the written file parses");

        let mut fresh = Game::load(&assets as &dyn AssetSource, PLAN_TURN_MAP).expect("loads");
        assert!(
            run_ticks(&mut fresh, &script),
            "the written route reaches the level change from the same start"
        );
    }

    /// The closed loop: when the first attempt's script drifts short of
    /// the goal, the planner plans again *from the drift point* and
    /// appends the continuation rather than starting over.
    #[test]
    fn a_drift_is_replanned_from_the_drift_point() {
        let seen = RefCell::new(Vec::<String>::new());
        let step = |prefix: &str| -> Result<String, PlanFailure> {
            seen.borrow_mut().push(prefix.to_string());
            if prefix.is_empty() {
                Ok("10 forward\n".to_string())
            } else {
                Ok("20 forward\n".to_string())
            }
        };
        // Only the two-line script (the drifted first attempt plus the
        // continuation planned from where it stopped) reaches the goal.
        let replay = |text: &str| -> bool { text == "10 forward\n20 forward\n" };
        let (script, attempts) =
            refine(step, replay, DEFAULT_ATTEMPTS).expect("the loop converges");
        assert_eq!(attempts, 2, "the first attempt drifted and was re-planned");
        assert_eq!(script, "10 forward\n20 forward\n");
        assert_eq!(
            seen.borrow().as_slice(),
            [String::new(), "10 forward\n".to_string()],
            "the second attempt planned from the first attempt's own end state"
        );
    }

    /// The loop gives up with a fixed reason rather than spinning when no
    /// attempt ever reaches the goal.
    #[test]
    fn a_route_that_never_arrives_fails_after_its_attempts() {
        let step = |_: &str| Ok("10 forward\n".to_string());
        let replay = |_: &str| false;
        assert_eq!(refine(step, replay, 3), Err(PlanFailure::NeverReached));
    }

    /// An attempt that adds nothing is a failure, not an infinite loop.
    #[test]
    fn an_attempt_that_adds_nothing_fails() {
        let step = |_: &str| Ok(String::new());
        let replay = |_: &str| false;
        assert_eq!(refine(step, replay, 3), Err(PlanFailure::NoProgress));
    }

    /// A turn is emitted as the shortest relative arc, and the tracked
    /// facing follows the script's own rounded value.
    #[test]
    fn turns_take_the_shortest_arc() {
        assert!((shortest_turn(350.0, 10.0) - 20.0).abs() < 1e-3);
        assert!((shortest_turn(10.0, 350.0) + 20.0).abs() < 1e-3);
        assert!((shortest_turn(0.0, 180.0)).abs() - 180.0 < 1e-3);

        let text = script_text(
            350.0,
            &[PlanAction::Move {
                yaw: 10.0,
                distance: 100.0,
                jump: false,
                fall: 0.0,
            }],
            &MoveConfig::default(),
        );
        assert!(text.starts_with("6 look 0 20.00\n"), "got {text:?}");
        assert!(text.contains(" forward\n"));
    }

    /// Only the first few segments of a goal-reaching plan are
    /// committed, and a door press among them goes with them.
    #[test]
    fn only_the_first_segments_of_a_plan_are_committed() {
        let step = PlanAction::Move {
            yaw: 0.0,
            distance: 64.0,
            jump: false,
            fall: 0.0,
        };
        let door = PlanAction::UseDoor {
            yaw: 0.0,
            open_seconds: 1.0,
        };
        let actions = [step, door, step, step, step];
        assert_eq!(first_segments(&actions, 2).len(), 3, "the door comes too");
        assert_eq!(first_segments(&actions, 1).len(), 1);
        assert_eq!(first_segments(&actions, 0), &actions, "0 keeps the plan");
        assert_eq!(first_segments(&actions, 99), &actions);
    }

    /// Where a `forward` line stops the player: held ticks plus the
    /// coast the release leaves behind, replayed the same way the engine
    /// moves them. The run must end *at or before* the distance it was
    /// planned for — an overshoot is what walks a planned route off a
    /// ledge — and not far short of it either.
    #[test]
    fn a_run_stops_at_the_distance_it_was_planned_for() {
        let config = MoveConfig::default();
        // One held tick is the shortest run the grammar can schedule, so
        // a run shorter than the distance that tick and its own coast
        // cover overshoots by construction; every longer one must not.
        let one_tick_speed = config.accelerate * config.max_speed * ohl_engine::TICK_SECONDS;
        let floor =
            one_tick_speed * ohl_engine::TICK_SECONDS + coast_distance(one_tick_speed, &config);
        for distance in [16.0f32, 48.0, 100.0, 256.0, 1_024.0] {
            let ticks = ticks_for_distance(distance, &config);
            assert!(ticks >= 1, "every run holds at least one tick");
            // Replay the held ticks, then let go.
            let step = ohl_engine::TICK_SECONDS;
            let mut speed = 0.0f32;
            let mut travelled = 0.0f32;
            for _ in 0..ticks {
                let drop = speed.max(config.stop_speed) * config.friction * step;
                speed = (speed - drop).max(0.0);
                speed = (speed + config.accelerate * config.max_speed * step).min(config.max_speed);
                travelled += speed * step;
            }
            let stopped = travelled + coast_distance(speed, &config);
            assert!(
                stopped <= distance.max(floor) + f32::EPSILON,
                "a {distance}-unit run stopped {stopped} units on"
            );
            assert!(
                stopped >= distance - coast_distance(config.max_speed, &config),
                "a {distance}-unit run stopped only {stopped} units on"
            );
        }
        assert_eq!(ticks_for_distance(0.0, &config), 0);
        assert_eq!(ticks_for_distance(f32::NAN, &config), 0);
    }

    /// A climb is timed by the ladder's own constant speed, not by the
    /// ground move's acceleration, and a longer climb takes longer.
    #[test]
    fn a_climb_is_timed_by_the_ladder_speed() {
        let config = MoveConfig::default();
        let short = ticks_for_climb(160.0, &config);
        let long = ticks_for_climb(320.0, &config);
        assert!(long > short, "twice the shaft, more ticks");
        let seconds = f32::from(u16::try_from(short).unwrap_or(0)) * CAPTURE_STEP;
        assert!(
            seconds >= 1.0,
            "a climb of the ladder speed's own distance takes at least a second, got {seconds}"
        );
        assert_eq!(ticks_for_climb(0.0, &config), 0);
        assert_eq!(ticks_for_climb(f32::NAN, &config), 0);
    }

    /// A climb becomes a turn toward the ladder and a held key: forward
    /// to go up it, back to come down, which is the engine's own ladder
    /// step read back rather than a second rule.
    #[test]
    fn a_climb_becomes_a_turn_and_a_held_key() {
        let config = MoveConfig::default();
        let up = script_text(
            0.0,
            &[PlanAction::Climb {
                yaw: 180.0,
                distance: 160.0,
                up: true,
            }],
            &config,
        );
        assert!(up.starts_with("6 look 0 180.00\n"), "got {up:?}");
        assert!(up.contains(" forward\n"), "got {up:?}");
        assert!(!up.contains(" back\n"), "got {up:?}");

        let down = script_text(
            180.0,
            &[PlanAction::Climb {
                yaw: 180.0,
                distance: 160.0,
                up: false,
            }],
            &config,
        );
        assert!(
            !down.contains("look"),
            "already facing the ladder: {down:?}"
        );
        assert!(down.contains(" back\n"), "got {down:?}");
    }

    /// A committed chunk never runs past a climb: `back` means "down the
    /// ladder" only while the player is on one, and the loop has to see
    /// whether they actually are.
    #[test]
    fn a_committed_chunk_stops_at_the_first_climb() {
        let walk = PlanAction::Move {
            yaw: 0.0,
            distance: 64.0,
            jump: false,
            fall: 0.0,
        };
        let climb = PlanAction::Climb {
            yaw: 180.0,
            distance: 160.0,
            up: false,
        };
        let actions = [walk, walk, climb, walk, climb];
        assert_eq!(through_first_climb(&actions).len(), 3);
        assert_eq!(through_first_climb(&actions[..2]).len(), 2, "no climb");
        assert_eq!(first_segments(&actions, 3).len(), 3, "a climb is a segment");
    }

    /// A route that leaves the player dead is refused with its own fixed
    /// reason rather than replanned from a body that cannot move.
    #[test]
    fn a_dead_player_is_its_own_refusal() {
        assert_eq!(
            PlanFailure::PlayerDied.to_string(),
            "the route walked so far left the player dead"
        );
    }
    /// The shelf-over-a-shaft fixture, with a `bsp` chosen by the caller
    /// (with its ladder, or with the lethal pit at its foot).
    fn shaft_fixture(bsp: Vec<u8>) -> (MemoryAssets, Game) {
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{PLAN_LADDER_MAP}.bsp"), bsp);
        let game = Game::load(&assets as &dyn AssetSource, PLAN_LADDER_MAP).expect("fixture loads");
        (assets, game)
    }

    /// A planner over `game`, with nothing planned yet: what the two
    /// `state_after` tests below drive directly.
    fn planner_over<'a>(
        game: &mut Game,
        source: &'a dyn AssetSource,
        base: &'a ohl_engine::GameSave,
        options: &'a PlanOptions,
    ) -> Planner<'a> {
        Planner {
            source,
            base,
            config: GameConfig {
                difficulty: game.difficulty(),
                overbright: game.overbright(),
            },
            options,
            last: RefCell::new(None),
            previous: RefCell::new(String::new()),
            climbs: RefCell::new(0),
        }
    }

    /// The script that walks this fixture's player off the shelf: they
    /// spawn facing along the shaft, so one short run is all it takes —
    /// short enough that they land at the shelf's foot rather than
    /// sailing on into the level-change volume at the far end.
    const OFF_THE_SHELF: &str = "30 forward\n";

    /// A route that has walked the player into something fatal is
    /// refused with its own reason. Without that, the next search runs
    /// from a body that cannot move, plans exactly what it planned last
    /// time, and every remaining attempt goes to it.
    #[test]
    fn a_route_that_kills_the_player_is_refused_from_the_state_it_left() {
        let (assets, mut game) = shaft_fixture(plan_pit_bsp("ohlplannext"));
        let source = &assets as &dyn AssetSource;
        let base = game.to_save(0);
        let options = PlanOptions::default();
        let planner = planner_over(&mut game, source, &base, &options);

        // Long enough for the fall, the landing and the pit's own hits.
        let fatal = format!("{OFF_THE_SHELF}240 wait\n");
        assert_eq!(
            planner.state_after(&fatal, 0).err(),
            Some(PlanFailure::PlayerDied)
        );
        // The same prefix without the walk leaves the player alive on the
        // shelf, so it is the pit that is being detected, not the fixture.
        let scratch = planner
            .state_after("60 wait\n", 0)
            .expect("standing still is survivable")
            .expect("nothing reached a level change");
        assert!(scratch.player_health() > 0.0);
    }

    /// A plan is never made in mid-air: an attempt waits for the player
    /// to land first, because the walk it plans with starts by settling
    /// onto the floor beneath them — a floor they have not reached yet
    /// while they are still falling.
    #[test]
    fn a_state_to_plan_from_is_always_a_landed_one() {
        // No ladder in this one: the player falls the shaft's own height
        // and lands, which is the state the wait exists to reach. (With
        // a ladder they would grab it on the way past and hang there,
        // which is neither falling nor standing.)
        let (assets, mut game) = shaft_fixture(plan_ladder_bsp("ohlplannext", false));
        let source = &assets as &dyn AssetSource;
        let base = game.to_save(0);
        let options = PlanOptions::default();
        let planner = planner_over(&mut game, source, &base, &options);

        // The run ends the moment the player leaves the shelf, so without
        // the wait this state is a falling one, hundreds of units above
        // the floor the plan would be made from.
        let scratch = planner
            .state_after(OFF_THE_SHELF, 0)
            .expect("the shaft is survivable")
            .expect("nothing reached a level change");
        assert!(
            scratch.player_on_ground(),
            "the attempt planned from mid-air"
        );
        let landed = scratch.player_origin()[2];
        assert!(
            landed < 200.0,
            "the player is on the shaft floor, not still up by the shelf (z {landed})"
        );
    }

    /// [`Game::player_on_ground`] is what that wait watches: a player
    /// dropped in above their own floor is airborne until they reach it.
    #[test]
    fn a_falling_player_is_not_on_the_ground() {
        let (_assets, mut game) = shaft_fixture(plan_ladder_bsp("ohlplannext", true));
        assert!(
            !game.player_on_ground(),
            "this fixture spawns the player above the shelf, as maps do"
        );
        let input = ohl_engine::Input::default();
        for _ in 0..30 {
            let _ = game.tick(CAPTURE_STEP, &input);
        }
        assert!(game.player_on_ground(), "they land on the shelf");
    }

    /// A run that ends by stepping off a ledge is not over when its ticks
    /// are: the script waits out the fall the plan measured, so the next
    /// chunk replays from the landing the plan was made from.
    #[test]
    fn a_run_that_ends_in_a_fall_waits_the_fall_out() {
        let config = MoveConfig::default();
        let flat = script_text(
            0.0,
            &[PlanAction::Move {
                yaw: 0.0,
                distance: 64.0,
                jump: false,
                fall: 0.0,
            }],
            &config,
        );
        let dropping = script_text(
            0.0,
            &[PlanAction::Move {
                yaw: 0.0,
                distance: 64.0,
                jump: false,
                fall: 192.0,
            }],
            &config,
        );
        let waits = |text: &str| text.lines().filter(|line| line.ends_with(" wait")).count();
        assert_eq!(waits(&flat), 1, "only the trailing settle: {flat:?}");
        assert_eq!(waits(&dropping), 2, "the fall's own wait too: {dropping:?}");

        // The wait is the fall's own time under this build's gravity, not
        // a fixed number: a taller fall waits longer, and 192 units takes
        // more than the trailing settle covers.
        let ticks = ticks_for_fall(192.0, &config);
        assert!(ticks > SETTLE_TICKS, "{ticks} vs {SETTLE_TICKS}");
        assert!(ticks_for_fall(768.0, &config) > ticks);
        assert_eq!(ticks_for_fall(0.0, &config), 0);
        assert_eq!(ticks_for_fall(f32::NAN, &config), 0);
        let seconds = f32::from(u16::try_from(ticks).unwrap_or(0)) * CAPTURE_STEP;
        let expected = (2.0 * 192.0 / config.gravity).sqrt();
        assert!(
            seconds >= expected,
            "a {expected}s fall is waited out, got {seconds}s"
        );
    }

    /// A climb counts as a climb whichever way it goes: up one is a held
    /// `forward`, the same line a walk-forward run emits, so counting the
    /// text alone reports every upward climb as a walk.
    #[test]
    fn climbs_are_counted_from_the_plan_not_from_the_text() {
        let up = PlanAction::Climb {
            yaw: 0.0,
            distance: 64.0,
            up: true,
        };
        let down = PlanAction::Climb {
            yaw: 0.0,
            distance: 64.0,
            up: false,
        };
        let walk = PlanAction::Move {
            yaw: 0.0,
            distance: 64.0,
            jump: false,
            fall: 0.0,
        };
        assert_eq!(count_climb_actions(&[up, walk, down]), 2);
        assert_eq!(count_climb_actions(&[walk]), 0);
        let text = script_text(0.0, &[up], &MoveConfig::default());
        assert!(
            text.contains(" forward\n") && !text.contains(" back\n"),
            "a climb up is a held forward, which is why the text cannot be counted: {text:?}"
        );
    }
}
