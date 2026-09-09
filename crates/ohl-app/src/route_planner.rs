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

/// The most ticks one `forward` line may schedule, so a nonsensical
/// distance cannot produce a script that runs for hours.
const MAX_SEGMENT_TICKS: u32 = 6_000;

/// The comment block every written route carries. Project-authored words
/// only; see this module's own doc comment.
const HEADER: &str = "\
# A machine-planned chain-walk route (--plan-route / cargo xtask
# plan-chain-hop), not a hand-authored one.
#
# A bounded breadth-first walk over this map's live collision model
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// How many script ticks it takes to cover `distance` world units on
/// foot, by replaying the engine's own ground-acceleration rule in one
/// dimension from a standstill: the player accelerates at
/// `accelerate * max_speed` per second up to `max_speed`, so a short run
/// never reaches top speed and "distance over top speed" always
/// undershoots it.
///
/// Deliberately conservative (it assumes the player starts from rest at
/// every segment, which a player who has just turned nearly does): a
/// slight overshoot ends against the wall the next turn faces away from,
/// while an undershoot leaves the next segment aimed from the wrong
/// place.
#[must_use]
pub fn ticks_for_distance(distance: f32, max_speed: f32, accelerate: f32) -> u32 {
    if !(distance.is_finite() && distance > 0.0) || max_speed <= 0.0 {
        return 0;
    }
    let step = TICK_SECONDS;
    let mut speed = 0.0f32;
    let mut travelled = 0.0f32;
    let mut seconds = 0.0f32;
    while travelled < distance && seconds < f32::from(u16::MAX) {
        speed = (speed + accelerate * max_speed * step).min(max_speed);
        travelled += speed * step;
        seconds += step;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a tick count bounded by MAX_SEGMENT_TICKS just below"
    )]
    let ticks = (seconds / CAPTURE_STEP).ceil() as u32;
    ticks.clamp(1, MAX_SEGMENT_TICKS)
}

/// The first `segments` walk-forward actions of `actions`, with every
/// door press among them; `0` keeps the whole plan.
#[must_use]
pub fn first_segments(actions: &[PlanAction], segments: usize) -> &[PlanAction] {
    if segments == 0 {
        return actions;
    }
    let mut moves = 0usize;
    for (index, action) in actions.iter().enumerate() {
        if matches!(action, PlanAction::Move { .. }) {
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
/// `max_speed`/`accelerate` come from the live
/// `ohl_physics::MoveConfig` the game moves by, never restated here.
#[must_use]
pub fn script_text(
    start_yaw: f32,
    actions: &[PlanAction],
    max_speed: f32,
    accelerate: f32,
) -> String {
    let mut lines = String::new();
    let mut facing = start_yaw.rem_euclid(360.0);
    for action in actions {
        match *action {
            PlanAction::Move {
                yaw,
                distance,
                jump,
            } => {
                facing = turn_toward(&mut lines, facing, yaw);
                let ticks = ticks_for_distance(distance, max_speed, accelerate);
                if ticks == 0 {
                    continue;
                }
                if jump {
                    let _ = writeln!(lines, "{ticks} forward jump");
                } else {
                    let _ = writeln!(lines, "{ticks} forward");
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
            let (max_speed, accelerate) = {
                let move_config = scratch.move_config();
                (move_config.max_speed, move_config.accelerate)
            };
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
            let text = script_text(facing, committed, max_speed, accelerate);
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
    use ohl_engine::test_support::{PLAN_TURN_MAP, plan_turn_bsp};

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
            }],
            320.0,
            10.0,
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

    /// A run from a standstill never reaches top speed, so the tick count
    /// has to exceed the naive "distance over top speed" estimate.
    #[test]
    fn a_short_run_is_given_more_ticks_than_top_speed_alone_implies() {
        let naive = 100.0f32 / 320.0 / CAPTURE_STEP;
        let ticks = f32::from(u16::try_from(ticks_for_distance(100.0, 320.0, 10.0)).unwrap_or(0));
        assert!(
            ticks > naive,
            "expected more than the top-speed estimate ({naive}), got {ticks}"
        );
        assert_eq!(ticks_for_distance(0.0, 320.0, 10.0), 0);
        assert_eq!(ticks_for_distance(f32::NAN, 320.0, 10.0), 0);
    }
}
