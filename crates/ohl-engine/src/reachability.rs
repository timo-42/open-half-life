//! A bounded, deterministic reachability/route-triage walk over the live
//! collision model.
//!
//! Two throwaway investigations (kept only as local, uncommitted `cargo run
//! --example` scratch tools while they were done) each rebuilt the same
//! breadth-first walk over a loaded map's real collision hulls to answer
//! one question: what, if anything, blocks the player from reaching a
//! map's level-change trigger from its own spawn point. This module is
//! that walk, promoted into a reusable, testable piece of the engine
//! instead of a one-off `cargo run --example`.
//!
//! The walk itself mirrors [`ohl_physics::movement::step_move`]'s own
//! step-up/move/drop shape (`docs/FORMAT_SOURCES.md`'s `sv_stepsize 18`),
//! but over a fixed [`CELL_SIZE`] grid from every visited cell in all
//! eight compass directions, using [`ohl_physics::Hull::Standing`] and the
//! engine's own live [`ohl_physics::CollisionModel`] — the same hulls the
//! walking player actually collides with, not a separate approximation of
//! them. A closed `func_door`/`func_door_rotating` blocks the walk exactly
//! as it blocks the player, because it is the same attached brush; nothing
//! here special-cases doors during the walk itself.
//!
//! Two edge shapes are tried from every visited cell in every direction:
//!
//! - **A plain step**: ascend at most [`STEP_UP`] (unchanged from the
//!   original walk), move [`CELL_SIZE`] horizontally, then descend to
//!   whatever floor is found within [`MAX_FALL`] — a one-way drop of any
//!   height is now a legal edge, not just one within the old, much smaller
//!   bound; a landing that falls further than [`DROP`] (the walk's old,
//!   conservative bound) is counted separately as [`RoundReport::long_drop_cells`]
//!   so a report reader can tell "this route needs a real fall" from "this
//!   is a normal step down a stair."
//! - **A jump**, tried only when the plain step fails (blocked ascending,
//!   blocked moving across, or no floor found at all): ascend at most the
//!   walking player's own jump apex plus [`STEP_UP`] (`v² / (2g)` from
//!   [`ohl_physics::MoveConfig::jump_velocity`] and
//!   [`ohl_physics::MoveConfig::gravity`], read from the live [`Game`] this
//!   walk is running against — never a restated copy of those constants),
//!   move up to the horizontal distance the player's own run speed covers
//!   over a full jump's airtime
//!   (`max_speed * 2 * jump_velocity / gravity`), then descend the same
//!   [`MAX_FALL`]-bounded way a plain step does. This is a deliberately
//!   coarse, single-hop approximation of a running jump — not a simulated
//!   arc — so it is documented as approximate, not as parity with
//!   [`ohl_physics::movement::player_move`]'s own physics.
//!
//! [`compute_reachability_report`] then runs that walk for up to
//! [`ReachabilityConfig::max_rounds`] rounds: each round reports how many
//! cells were reached, which brush-entity classnames sit on the
//! unreached frontier (with a count of distinct entities and whether the
//! engine's own use-proximity path — [`ohl_game::find_usable_within`]'s
//! radius, [`crate::USE_RADIUS`] — could open each one from a cell the
//! walk already reached), and whether any `trigger_changelevel` volume was
//! reached (and, if so, its straight-line distance from spawn, rounded to
//! the nearest ten units). Between rounds, every closed door the walk
//! found both on the frontier *and* use-openable from a reached cell is
//! detached from the collision model (`CollisionModel::detach_brush`) —
//! simulating it having been opened — before the next round's walk runs,
//! so a route that needs several doors opened in sequence is reported one
//! round at a time.
//!
//! Every field this module reports is either a classname (part of this
//! project's own documented entity vocabulary, not a per-map secret), an
//! aggregate count, or a distance rounded to [`DISTANCE_ROUNDING`] units —
//! never a targetname, a raw coordinate, or a map name (the caller already
//! knows which map it asked for). See `docs/CLEAN_ROOM.md`.

use std::collections::{BTreeMap, HashSet, VecDeque};

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::registry::{
    Breakable, BrushBounds, ChangeLevel, ClassName, Door, MoverState, Pushable,
};
use ohl_physics::{BrushId, CollisionModel, Hull};

use crate::{Game, USE_RADIUS};

/// The grid spacing the walk steps in, in world units.
pub const CELL_SIZE: f32 = 16.0;

/// The documented step-up height (`sv_stepsize`; see
/// `ohl_physics::movement`'s own module doc and `docs/FORMAT_SOURCES.md`).
pub const STEP_UP: f32 = 18.0;

/// The walk's old, conservative drop bound. A one-way fall is no longer
/// rejected past this distance (see this module's own doc comment and
/// [`MAX_FALL`]); it is kept only as the threshold
/// [`RoundReport::long_drop_cells`] reports against, so a route that only
/// works because of a real fall — not a stair step — is called out
/// separately rather than silently folded into the ordinary reachable
/// count.
pub const DROP: f32 = 72.0;

/// The largest one-way fall (or jump landing) the walk will follow before
/// giving up on finding a floor below. This is not a documented map-format
/// fact — it exists only so a genuine bottomless void (a kill volume, the
/// edge of the world) cannot make the walk search downward forever; it is
/// set generously larger than any drop a real level's own vertical layout
/// would ever ask a route to take.
///
/// [`settle_start`] reuses it for the one downward trace it takes before
/// the walk begins, for the same reason and with the same bound: a spawn
/// point hanging above its own floor is one more fall this module has to
/// follow, and no legitimate one is further than this.
pub const MAX_FALL: f32 = 8_192.0;

/// Distances this module reports are rounded to the nearest multiple of
/// this many units (see this module's own doc comment).
pub const DISTANCE_ROUNDING: f32 = 10.0;

/// The eight compass directions the walk tries from every visited cell.
const DIRECTIONS: [(f32, f32); 8] = [
    (1.0, 0.0),
    (1.0, 1.0),
    (0.0, 1.0),
    (-1.0, 1.0),
    (-1.0, 0.0),
    (-1.0, -1.0),
    (0.0, -1.0),
    (1.0, -1.0),
];

/// Bounds how much work one walk (and so one round) can do, so a
/// pathological or enormous map cannot make this run unbounded: the walk
/// stops enqueuing new cells once it has visited this many, and reports
/// what it found so far rather than continuing.
#[derive(Debug, Clone, Copy)]
pub struct ReachabilityConfig {
    /// The largest number of distinct grid cells one round's walk visits.
    pub cell_cap: usize,
    /// The largest number of door-opening rounds
    /// [`compute_reachability_report`] runs.
    pub max_rounds: usize,
    /// Treats a `func_breakable` on the frontier with `health > 0` (not
    /// the documented "Only Trigger" flag, and not already broken) as
    /// openable-by-damage between rounds, the same way a closed,
    /// use-openable door is opened: its brush is detached from the
    /// collision model before the next round runs (see
    /// [`RoundReport::breakables_opened`]).
    ///
    /// This never checks the walk's own (nonexistent) inventory or fires
    /// any actual damage — it is a caller-supplied assumption ("assume the
    /// player is carrying a weapon capable of breaking it") documented by
    /// this flag's own name, not a claim that a specific weapon is
    /// present. `false` by default, matching a cold map load, which by
    /// this project's own `ohl_combat::Inventory::new` grants no weapon at
    /// all, not even the crowbar.
    pub assume_armed: bool,
    /// Adds a third, longer-reaching edge attempt (tried only when both
    /// the plain step and the ordinary running-jump edge fail): a long
    /// jump (`item_longjump`), bounded by
    /// [`ohl_physics::MoveConfig::long_jump_forward_speed`]/
    /// [`ohl_physics::MoveConfig::long_jump_up_speed`] read live from the
    /// [`Game`] this walk runs against, the same way the ordinary jump
    /// edge is bounded by that config's own `jump_velocity`/`max_speed`
    /// (never a restated literal). Like [`Self::assume_armed`], this is a
    /// caller-supplied assumption ("assume the player owns the long jump
    /// module") — a cold map load owns no items at all
    /// (`ohl_combat::Inventory::has_long_jump`), so this never checks or
    /// fabricates that ownership. `false` by default.
    pub assume_longjump: bool,
}

impl Default for ReachabilityConfig {
    fn default() -> Self {
        Self {
            cell_cap: 40_000,
            max_rounds: 6,
            assume_armed: false,
            assume_longjump: false,
        }
    }
}

/// One brush-entity classname found on a round's unreached frontier.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontierClass {
    /// The blocking entities' `classname` (for example `func_door`).
    pub classname: String,
    /// How many distinct entities of this classname sit on the frontier.
    pub instance_count: usize,
    /// Whether at least one of them could be opened by the engine's own
    /// use-proximity path from a cell this round's walk already reached.
    pub use_openable: bool,
    /// Whether at least one of them is a `func_breakable` (or a
    /// `func_pushable` with its own "Breakable" flag set) with `health >
    /// 0`, not `trigger_only`, and not already broken — openable-by-damage
    /// next round, but only when [`ReachabilityConfig::assume_armed`] was
    /// set for this walk (see that field's own doc comment). Always
    /// `false` without it, even when a breakable genuinely sits here.
    pub damage_openable: bool,
    /// Whether at least one of them is a `func_pushable` — openable-by-push
    /// next round. Unlike [`Self::damage_openable`] this needs no assumed
    /// weapon (walking into a pushable and shoving it needs nothing but
    /// the player's own body), so it is reported regardless of
    /// [`ReachabilityConfig::assume_armed`]. Approximate: this walk does
    /// not simulate the real push distance or direction, it only reports
    /// that a pushable sits on the frontier and detaches its brush between
    /// rounds, the same coarse "remove the brush" treatment a broken
    /// breakable gets.
    pub push_openable: bool,
}

/// Whether a `trigger_changelevel` volume was reached this round, and how
/// far it sits from spawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangeLevelStatus {
    /// At least one `trigger_changelevel` volume overlapped a reached cell.
    pub reachable: bool,
    /// The nearest such volume's straight-line distance from spawn,
    /// rounded to the nearest [`DISTANCE_ROUNDING`] units. `None` when the
    /// map declares no `trigger_changelevel` at all.
    pub distance_rounded: Option<f32>,
}

/// One round's aggregate results.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundReport {
    /// How many rounds of door-opening preceded this one (`0` is the
    /// walk from spawn with nothing yet opened).
    pub round: usize,
    /// How many distinct 16-unit grid cells this round's walk reached.
    pub reachable_cells: usize,
    /// Brush-entity classnames on this round's unreached frontier,
    /// ordered by classname.
    pub frontier_classes: Vec<FrontierClass>,
    /// Whether a `trigger_changelevel` was reached this round.
    pub changelevel: ChangeLevelStatus,
    /// How many of this round's [`Self::reachable_cells`] were first
    /// reached by a one-way fall (a plain step or a jump landing) deeper
    /// than the walk's old, conservative [`DROP`] bound — a route through
    /// one of these cells needs a real fall, not just a stair step or a
    /// short hop, to work.
    pub long_drop_cells: usize,
    /// How many of this round's [`Self::reachable_cells`] were reached
    /// only by the long-jump edge ([`ReachabilityConfig::assume_longjump`])
    /// — neither the plain step nor the ordinary running jump reached
    /// them, so a route through one of these cells needs the long jump
    /// module. Always `0` without `assume_longjump`.
    pub long_jump_cells: usize,
    /// How many doors this round found and opened for the *next* round
    /// (`0` on the last round, since nothing further needed opening).
    pub doors_opened: usize,
    /// How many breakables this round found openable-by-damage
    /// ([`FrontierClass::damage_openable`]) and broke for the *next*
    /// round (`0` when [`ReachabilityConfig::assume_armed`] was not set,
    /// or none was found).
    pub breakables_opened: usize,
    /// How many pushables this round found on the frontier
    /// ([`FrontierClass::push_openable`]) and pushed out of the way for
    /// the *next* round.
    pub pushables_opened: usize,
    /// Whether this round's walk stopped early because it hit
    /// [`ReachabilityConfig::cell_cap`] rather than exhausting every
    /// reachable cell — a sign the reported [`Self::reachable_cells`] and
    /// frontier are a lower bound, not the map's whole reachable area.
    pub capped: bool,
}

/// The whole bounded walk's results: one [`RoundReport`] per round
/// [`compute_reachability_report`] ran, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct ReachabilityReport {
    /// Per-round results, oldest first.
    pub rounds: Vec<RoundReport>,
}

/// One 16-unit grid cell, quantized from a world position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Cell(i32, i32, i32);

// A published GoldSrc map's coordinates fit comfortably within `i16`, so
// dividing by `CELL_SIZE` and rounding never approaches `i32`'s range; the
// truncation clippy warns about cannot occur for any position this walk
// ever visits.
#[allow(clippy::cast_possible_truncation)]
fn cell_of(position: Vec3) -> Cell {
    Cell(
        (position.x / CELL_SIZE).round() as i32,
        (position.y / CELL_SIZE).round() as i32,
        (position.z / CELL_SIZE).round() as i32,
    )
}

/// One round's raw walk output, before it is summarized into a
/// [`RoundReport`].
struct WalkResult {
    /// Every landing position the walk accepted, one per visited cell.
    visited_positions: Vec<Vec3>,
    /// Every attached brush the walk was stopped by on the frontier (a
    /// brush whose collision blocked a step that would otherwise have
    /// reached a new cell), excluding any already-detached ones.
    frontier_brushes: HashSet<BrushId>,
    /// Whether the walk stopped early because it hit [`ReachabilityConfig::cell_cap`].
    capped: bool,
    /// How many landings fell further than [`DROP`] below the cell they
    /// stepped or jumped from (see [`RoundReport::long_drop_cells`]).
    long_drop_cells: usize,
    /// How many landings were reached only by the long-jump edge (see
    /// [`RoundReport::long_jump_cells`]).
    long_jump_cells: usize,
}

/// The ascend/horizontal bounds a jump edge is allowed, derived once from
/// the live [`ohl_physics::MoveConfig`] a walk runs against rather than
/// restated as fixed numbers (see this module's own doc comment).
#[derive(Debug, Clone, Copy)]
struct JumpBounds {
    /// Tallest obstruction a jump edge may ascend over: the standing
    /// step-up plus the jump apex height (`v² / (2g)`).
    ascend: f32,
    /// Furthest horizontal distance a jump edge may cross in one hop: run
    /// speed times a full jump's airtime (`2v / g`). A coarse, documented
    /// approximation of a running jump's actual range, not a simulated arc.
    horizontal: f32,
}

impl JumpBounds {
    fn from_move_config(config: &ohl_physics::MoveConfig) -> Self {
        let apex_height = config.jump_velocity * config.jump_velocity / (2.0 * config.gravity);
        let airtime = 2.0 * config.jump_velocity / config.gravity;
        Self {
            ascend: config.step_size + apex_height,
            horizontal: config.max_speed * airtime,
        }
    }

    /// As [`Self::from_move_config`], for a long jump (`item_longjump`)
    /// instead of an ordinary running jump. Unlike an ordinary jump —
    /// which only boosts the *vertical* launch speed, so its horizontal
    /// reach is bounded by the player's own separately-configured run
    /// speed (`max_speed`) — `ohl_physics::movement`'s long-jump impulse
    /// sets *both* components directly
    /// (`velocity = forward * long_jump_forward_speed + Z *
    /// long_jump_up_speed`, see that module's `walk_or_air_move`), so both
    /// this bound's airtime and its horizontal reach are derived from the
    /// long jump's own two published constants alone, never from
    /// `max_speed`.
    fn from_long_jump_config(config: &ohl_physics::MoveConfig) -> Self {
        let apex_height =
            config.long_jump_up_speed * config.long_jump_up_speed / (2.0 * config.gravity);
        let airtime = 2.0 * config.long_jump_up_speed / config.gravity;
        Self {
            ascend: config.step_size + apex_height,
            horizontal: config.long_jump_forward_speed * airtime,
        }
    }
}

/// One edge attempt's outcome: either a new landing (with how far below
/// the starting cell it fell), or a reason it failed.
#[derive(Clone, Copy)]
enum EdgeOutcome {
    Landed { position: Vec3, drop: f32 },
    BlockedUp,
    BlockedAcross(Option<BrushId>),
    NoFloor,
}

/// Tries one ascend/move/drop edge from `position` in direction
/// `horizontal_dir`, ascending at most `ascend`, moving `horizontal_dist`
/// across, then descending at most [`MAX_FALL`] to find a new floor.
fn try_edge(
    collision: &CollisionModel,
    hull: Hull,
    position: Vec3,
    horizontal_dir: Vec3,
    ascend: f32,
    horizontal_dist: f32,
) -> EdgeOutcome {
    let up = collision.trace(hull, position, position + Vec3::Z * ascend);
    if up.start_solid {
        return EdgeOutcome::BlockedUp;
    }
    let top = up.end_pos;

    let across = collision.trace(hull, top, top + horizontal_dir * horizontal_dist);
    if across.blocked() {
        return EdgeOutcome::BlockedAcross(across.brush_index);
    }

    let down_target = across.end_pos - Vec3::Z * (ascend + MAX_FALL);
    let down = collision.trace(hull, across.end_pos, down_target);
    if down.start_solid || down.fraction >= 1.0 {
        // Either embedded in solid immediately (shouldn't happen after a
        // successful horizontal move, but skip rather than trust it) or no
        // floor within the fall bound: a void, not a new reachable cell.
        return EdgeOutcome::NoFloor;
    }

    let landing = down.end_pos;
    let drop = (position.z - landing.z).max(0.0);
    EdgeOutcome::Landed {
        position: landing,
        drop,
    }
}

/// Drops `start` straight down onto the first floor beneath it, the way
/// the spawning player themselves falls on the map's first ticks.
///
/// A map's `info_player_start` is a *point* entity: nothing requires it to
/// sit exactly on the floor, and mappers routinely place one well above
/// one (the engine spawns the player there and lets them fall). The walk
/// below, however, is a fixed-position grid step from wherever it is told
/// to start: from a point hanging in mid-air, every one of the eight
/// compass directions ends in a drop longer than [`DROP`] and is discarded,
/// so the whole map reports as exactly one reachable cell. Settling first
/// makes the walk start where the player actually ends up standing.
///
/// Returns `start` unchanged when the downward trace finds no floor at all
/// (a spawn over a pit, or a map with no usable geometry beneath it) or
/// when the start point is already embedded in solid: neither is this
/// function's to invent a position for.
fn settle_start(collision: &CollisionModel, start: Vec3) -> Vec3 {
    let down = collision.trace(Hull::Standing, start, start - Vec3::Z * MAX_FALL);
    if down.start_solid || down.fraction >= 1.0 {
        return start;
    }
    down.end_pos
}

/// The step-up/move/drop walk itself: from `start`, breadth-first over the
/// 16-unit grid, using [`Hull::Standing`] against `collision` exactly as
/// the walking player would. From every visited cell, in every direction,
/// a plain [`STEP_UP`]/[`CELL_SIZE`] edge is tried first; a jump edge
/// (bounded by `jump`) is tried only when that plain edge fails; a
/// long-jump edge (bounded by `long_jump`, only when given — see
/// [`ReachabilityConfig::assume_longjump`]) is tried only when that
/// ordinary jump edge also fails — see this module's own doc comment.
fn walk(
    collision: &CollisionModel,
    start: Vec3,
    cap: usize,
    jump: JumpBounds,
    long_jump: Option<JumpBounds>,
) -> WalkResult {
    let hull = Hull::Standing;
    let mut visited_cells: HashSet<Cell> = HashSet::new();
    let mut visited_positions = Vec::new();
    let mut frontier_brushes = HashSet::new();
    let mut queue = VecDeque::new();
    let mut long_drop_cells = 0usize;
    let mut long_jump_cells = 0usize;

    visited_cells.insert(cell_of(start));
    visited_positions.push(start);
    queue.push_back(start);

    let mut capped = false;
    while let Some(position) = queue.pop_front() {
        for (dx, dy) in DIRECTIONS {
            if visited_cells.len() >= cap {
                capped = true;
                break;
            }
            let horizontal_dir = Vec3::new(dx, dy, 0.0).normalize_or_zero();
            if horizontal_dir == Vec3::ZERO {
                continue;
            }

            let plain = try_edge(
                collision,
                hull,
                position,
                horizontal_dir,
                STEP_UP,
                CELL_SIZE,
            );
            let ordinary_jump = if matches!(plain, EdgeOutcome::Landed { .. }) {
                plain
            } else {
                try_edge(
                    collision,
                    hull,
                    position,
                    horizontal_dir,
                    jump.ascend,
                    jump.horizontal,
                )
            };
            let (outcome, via_long_jump) = if matches!(ordinary_jump, EdgeOutcome::Landed { .. }) {
                (ordinary_jump, false)
            } else if let Some(long_jump) = long_jump {
                (
                    try_edge(
                        collision,
                        hull,
                        position,
                        horizontal_dir,
                        long_jump.ascend,
                        long_jump.horizontal,
                    ),
                    true,
                )
            } else {
                (ordinary_jump, false)
            };

            match outcome {
                EdgeOutcome::Landed {
                    position: landing,
                    drop,
                } => {
                    let cell = cell_of(landing);
                    if visited_cells.insert(cell) {
                        visited_positions.push(landing);
                        queue.push_back(landing);
                        if drop > DROP {
                            long_drop_cells += 1;
                        }
                        if via_long_jump {
                            long_jump_cells += 1;
                        }
                    }
                }
                EdgeOutcome::BlockedAcross(_) | EdgeOutcome::BlockedUp | EdgeOutcome::NoFloor => {
                    // Neither the plain step, the jump, nor (if tried) the
                    // long jump found a new cell: record every blocking
                    // brush any attempt found, so the frontier reflects
                    // whatever actually stopped the walk in this direction.
                    for attempt in [plain, ordinary_jump, outcome] {
                        if let EdgeOutcome::BlockedAcross(Some(brush)) = attempt {
                            frontier_brushes.insert(brush);
                        }
                    }
                }
            }
        }
        if capped {
            break;
        }
    }

    WalkResult {
        visited_positions,
        frontier_brushes,
        capped: capped || visited_cells.len() >= cap,
        long_drop_cells,
        long_jump_cells,
    }
}

/// The classname of `entity`, or an empty string when it somehow has none
/// (never expected in practice — every spawned entity carries
/// [`ClassName`] — but this module never panics on map-derived data).
fn classname_of(game: &Game, entity: Entity) -> String {
    game.registry()
        .world
        .get::<&ClassName>(entity)
        .map(|name| name.0.clone())
        .unwrap_or_default()
}

/// The entity a brush hull belongs to, via [`Game::brush_collision`].
fn entity_for_brush(game: &Game, brush: BrushId) -> Option<Entity> {
    game.brush_collision()
        .iter()
        .find(|(_, id)| *id == brush)
        .map(|(entity, _)| *entity)
}

/// Whether any of `positions` sits within [`USE_RADIUS`] of `entity`'s
/// current placed brush centre — the same proximity test
/// [`ohl_game::find_usable_within`] runs, checked here against every
/// reached cell instead of one live player position.
fn use_openable_from(game: &Game, entity: Entity, positions: &[Vec3]) -> bool {
    let Some(center) = ohl_game::pose::brush_center(game.registry(), entity) else {
        return false;
    };
    positions
        .iter()
        .any(|position| position.distance(center) <= USE_RADIUS)
}

/// Rounds `value` to the nearest multiple of [`DISTANCE_ROUNDING`].
fn round_distance(value: f32) -> f32 {
    (value / DISTANCE_ROUNDING).round() * DISTANCE_ROUNDING
}

/// Whether `position` lies inside `bounds`, expanded by half a grid cell
/// in every direction so a floor-snapped walk landing exactly at a
/// trigger volume's own boundary still counts as reaching it.
fn bounds_contains_with_margin(bounds: &BrushBounds, position: Vec3) -> bool {
    let margin = Vec3::splat(CELL_SIZE / 2.0);
    let mins = bounds.mins - margin;
    let maxs = bounds.maxs + margin;
    position.cmpge(mins).all() && position.cmple(maxs).all()
}

/// This round's `trigger_changelevel` status: reachable if any reached
/// cell overlaps any `trigger_changelevel` volume's bounds, and (whether
/// or not it was reached) the nearest one's distance from `start`.
fn changelevel_status(game: &Game, start: Vec3, reached: &[Vec3]) -> ChangeLevelStatus {
    let mut nearest: Option<f32> = None;
    let mut reachable = false;
    for (_change_level, bounds) in &mut game
        .registry()
        .world
        .query::<(&ChangeLevel, &BrushBounds)>()
    {
        let center = Vec3::new(
            f32::midpoint(bounds.mins.x, bounds.maxs.x),
            f32::midpoint(bounds.mins.y, bounds.maxs.y),
            f32::midpoint(bounds.mins.z, bounds.maxs.z),
        );
        let distance = start.distance(center);
        nearest = Some(nearest.map_or(distance, |best: f32| best.min(distance)));
        if reached
            .iter()
            .any(|position| bounds_contains_with_margin(bounds, *position))
        {
            reachable = true;
        }
    }
    ChangeLevelStatus {
        reachable,
        distance_rounded: nearest.map(round_distance),
    }
}

/// One classname's aggregated frontier state for a round: how many
/// distinct entities, and whether at least one of them is openable each of
/// the three ways [`compute_reachability_report`] models (use, damage,
/// push).
#[derive(Default)]
struct FrontierAgg {
    count: usize,
    use_openable: bool,
    damage_openable: bool,
    push_openable: bool,
}

/// One round's frontier, classified into per-classname aggregates
/// ([`FrontierAgg`]) and the three separate brush lists
/// [`compute_reachability_report`] detaches between rounds: closed,
/// use-openable doors; breakables openable-by-damage
/// (`config.assume_armed` only); and pushables (always).
struct ClassifiedFrontier {
    classes: BTreeMap<String, FrontierAgg>,
    openable_doors: Vec<BrushId>,
    openable_breakables: Vec<BrushId>,
    openable_pushables: Vec<BrushId>,
}

/// Classifies `walk_result`'s frontier brushes by classname and by which
/// of the three round-advance edges (door/breakable/pushable) each one
/// offers, per this module's own doc comment.
fn classify_frontier(
    game: &Game,
    walk_result: &WalkResult,
    config: &ReachabilityConfig,
) -> ClassifiedFrontier {
    let mut classes: BTreeMap<String, FrontierAgg> = BTreeMap::new();
    let mut openable_doors: Vec<BrushId> = Vec::new();
    let mut openable_breakables: Vec<BrushId> = Vec::new();
    let mut openable_pushables: Vec<BrushId> = Vec::new();
    for brush in &walk_result.frontier_brushes {
        let Some(entity) = entity_for_brush(game, *brush) else {
            continue;
        };
        let classname = classname_of(game, entity);
        let is_closed_door = game
            .registry()
            .world
            .get::<&Door>(entity)
            .is_ok_and(|door| door.state == MoverState::Closed);
        let door_openable =
            is_closed_door && use_openable_from(game, entity, &walk_result.visited_positions);

        let breakable_state =
            game.registry()
                .world
                .get::<&Breakable>(entity)
                .ok()
                .map(|breakable| {
                    !breakable.broken && !breakable.trigger_only && breakable.health > 0.0
                });
        let damage_openable = config.assume_armed && breakable_state.unwrap_or(false);

        let push_openable = game.registry().world.get::<&Pushable>(entity).is_ok();

        let entry = classes.entry(classname).or_default();
        entry.count += 1;
        entry.use_openable |= door_openable;
        entry.damage_openable |= damage_openable;
        entry.push_openable |= push_openable;

        if door_openable {
            openable_doors.push(*brush);
        }
        if damage_openable {
            openable_breakables.push(*brush);
        }
        if push_openable {
            openable_pushables.push(*brush);
        }
    }
    ClassifiedFrontier {
        classes,
        openable_doors,
        openable_breakables,
        openable_pushables,
    }
}

/// Runs the bounded, iterative reachability walk described in this
/// module's own doc comment, starting from `game`'s current player
/// position.
///
/// Mutates `game`'s live collision model: doors this round's walk both
/// found on the frontier and found use-openable are detached
/// (`CollisionModel::detach_brush`) before the next round runs, so a
/// caller that wants the report without permanently altering a `Game` it
/// still needs afterward should call this on a `Game` it loaded solely for
/// this analysis (matching this project's other headless dev-tools
/// commands).
///
/// This walk (and so this detach) only ever reads/mutates
/// [`Game::collision`] — the *player's* collision model — through
/// [`Game::collision_mut`], never [`Game::monster_collision`] (M9.11,
/// `docs/FORMAT_SOURCES.md` item 33): the two models diverge for the
/// remainder of a `Game` this function was called on, but that is
/// harmless here specifically because nothing in this module ever traces
/// against, or otherwise reads, the monster model — the walk this module
/// runs is deliberately a player-reachability question ("can the walking
/// *player* get from spawn to the exit"), not a monster-pathing one, so
/// simulating an opened door only where this module actually looks is the
/// right scope, not an oversight. A caller that also needs the monster
/// model to reflect this walk's simulated door-opens (none do today)
/// would need its own detach against [`Game::monster_collision_mut`]. The
/// same applies to a breakable's or a pushable's brush, detached exactly
/// the same way (M9.12).
#[must_use]
pub fn compute_reachability_report(
    game: &mut Game,
    config: &ReachabilityConfig,
) -> ReachabilityReport {
    let mut rounds = Vec::new();

    if game.collision().is_none() {
        rounds.push(RoundReport {
            round: 0,
            reachable_cells: 0,
            frontier_classes: Vec::new(),
            changelevel: ChangeLevelStatus {
                reachable: false,
                distance_rounded: None,
            },
            long_drop_cells: 0,
            long_jump_cells: 0,
            doors_opened: 0,
            breakables_opened: 0,
            pushables_opened: 0,
            capped: false,
        });
        return ReachabilityReport { rounds };
    }

    let jump = JumpBounds::from_move_config(game.move_config());
    let start = match game.collision() {
        Some(collision) => settle_start(collision, Vec3::from_array(game.player_origin())),
        None => Vec3::from_array(game.player_origin()),
    };
    let long_jump = config
        .assume_longjump
        .then(|| JumpBounds::from_long_jump_config(game.move_config()));

    for round in 0..config.max_rounds.max(1) {
        let walk_result = {
            let Some(collision) = game.collision() else {
                break;
            };
            walk(collision, start, config.cell_cap, jump, long_jump)
        };

        let ClassifiedFrontier {
            classes,
            openable_doors,
            openable_breakables,
            openable_pushables,
        } = classify_frontier(game, &walk_result, config);

        let changelevel = changelevel_status(game, start, &walk_result.visited_positions);
        let doors_opened = openable_doors.len();
        let breakables_opened = openable_breakables.len();
        let pushables_opened = openable_pushables.len();

        rounds.push(RoundReport {
            round,
            reachable_cells: walk_result.visited_positions.len(),
            frontier_classes: classes
                .into_iter()
                .map(|(classname, agg)| FrontierClass {
                    classname,
                    instance_count: agg.count,
                    use_openable: agg.use_openable,
                    damage_openable: agg.damage_openable,
                    push_openable: agg.push_openable,
                })
                .collect(),
            changelevel,
            long_drop_cells: walk_result.long_drop_cells,
            long_jump_cells: walk_result.long_jump_cells,
            doors_opened,
            breakables_opened,
            pushables_opened,
            capped: walk_result.capped,
        });

        if openable_doors.is_empty()
            && openable_breakables.is_empty()
            && openable_pushables.is_empty()
        {
            break;
        }
        let Some(collision) = game.collision_mut() else {
            break;
        };
        for brush in openable_doors
            .into_iter()
            .chain(openable_breakables)
            .chain(openable_pushables)
        {
            collision.detach_brush(brush);
        }
    }

    ReachabilityReport { rounds }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        REACH_DOOR_MAP, reachability_changelevel_entities,
        reachability_changelevel_entities_at_height, reachability_door_bsp,
    };
    use crate::{AssetSource, MemoryAssets};

    /// See `a_gap_beyond_the_ordinary_jump_is_reached_only_with_assume_longjump`'s
    /// own comment for why this margin exists.
    const GAP_MARGIN: f32 = 20.0;

    fn game() -> Game {
        let bytes = reachability_door_bsp(&reachability_changelevel_entities("ohlreachnext"));
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_DOOR_MAP}.bsp"), bytes);
        Game::load(&assets as &dyn AssetSource, REACH_DOOR_MAP).expect("the fixture loads")
    }

    /// The same fixture with its `info_player_start` hanging `spawn_z`
    /// above the floor instead of just clear of it.
    fn game_spawned_at(spawn_z: f32) -> Game {
        let bytes = reachability_door_bsp(&reachability_changelevel_entities_at_height(
            "ohlreachnext",
            spawn_z,
        ));
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_DOOR_MAP}.bsp"), bytes);
        Game::load(&assets as &dyn AssetSource, REACH_DOOR_MAP).expect("the fixture loads")
    }

    /// A map's `info_player_start` is a point entity a mapper may hang well
    /// above the floor; the spawning player falls onto it. This walk is a
    /// fixed-position grid step, so from a start in mid-air every one of
    /// the eight directions ends in a drop longer than [`DROP`] and is
    /// discarded — the whole map reports as one cell, whatever is actually
    /// walkable under it. [`settle_start`] drops the start onto the floor
    /// first, which must make a high spawn report exactly what the same
    /// map reports from a floor-level one.
    #[test]
    fn a_spawn_hanging_above_the_floor_walks_the_same_map_as_one_on_it() {
        let config = ReachabilityConfig::default();
        let on_the_floor = compute_reachability_report(&mut game(), &config);
        // Far enough above the floor that an unsettled walk cannot step
        // anywhere at all (`DROP` is 72).
        let hanging = compute_reachability_report(&mut game_spawned_at(200.0), &config);

        assert!(
            on_the_floor.rounds[0].reachable_cells > 1,
            "the floor-level control walk should reach a real area"
        );
        assert_eq!(
            hanging.rounds[0].reachable_cells, on_the_floor.rounds[0].reachable_cells,
            "a spawn hanging above the floor must settle onto it and walk the same map"
        );
        assert_eq!(
            hanging.rounds.len(),
            on_the_floor.rounds.len(),
            "the settled walk must find (and open) the same doors, round for round"
        );
        assert_eq!(
            hanging
                .rounds
                .last()
                .map(|round| round.changelevel.reachable),
            on_the_floor
                .rounds
                .last()
                .map(|round| round.changelevel.reachable),
            "the settled walk must reach the same level-change trigger"
        );
    }

    /// The whole point of this module: a closed door hides a
    /// `trigger_changelevel` behind it on round 0, and opening the one
    /// door the frontier found (and could reach with `use`) makes it
    /// reachable on round 1.
    #[test]
    fn changelevel_becomes_reachable_only_after_the_door_opens() {
        let mut game = game();
        let report = compute_reachability_report(&mut game, &ReachabilityConfig::default());

        assert!(
            report.rounds.len() >= 2,
            "expected at least two rounds (the door blocks round 0), got {}",
            report.rounds.len()
        );

        let first = &report.rounds[0];
        assert!(
            !first.changelevel.reachable,
            "the closed door should keep the changelevel trigger out of round 0's reachable set"
        );
        assert_eq!(
            first.doors_opened, 1,
            "round 0's frontier should find exactly the one closed, use-openable door"
        );
        let door_class = first
            .frontier_classes
            .iter()
            .find(|class| class.classname == "func_door")
            .expect("the closed func_door should be on round 0's frontier");
        assert_eq!(door_class.instance_count, 1);
        assert!(door_class.use_openable);

        let second = &report.rounds[1];
        assert!(
            second.changelevel.reachable,
            "with the door detached, the walk should now reach the changelevel trigger"
        );
        assert!(
            second.reachable_cells > first.reachable_cells,
            "opening the door should reveal strictly more reachable cells: {} vs {}",
            second.reachable_cells,
            first.reachable_cells
        );
    }

    /// A map with no `trigger_changelevel` at all reports a `None`
    /// distance rather than a fabricated one, and the walk still finds the
    /// door on the frontier.
    #[test]
    fn no_changelevel_declared_reports_no_distance() {
        use crate::test_support::reachability_door_only_entities;

        let bytes = reachability_door_bsp(&reachability_door_only_entities());
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_DOOR_MAP}.bsp"), bytes);
        let mut game = Game::load(&assets as &dyn AssetSource, REACH_DOOR_MAP)
            .expect("the fixture loads even without a changelevel trigger");
        let report = compute_reachability_report(&mut game, &ReachabilityConfig::default());
        assert_eq!(report.rounds[0].changelevel.distance_rounded, None);
        assert!(!report.rounds[0].changelevel.reachable);
    }

    /// A ledge whose only route is a one-way fall taller than the walk's
    /// old, conservative [`DROP`] bound (72 units) is now reached — and
    /// counted separately as a long drop, not silently folded into the
    /// ordinary reachable-cell count.
    #[test]
    fn a_ledge_reachable_only_by_a_long_drop_is_reached() {
        use crate::test_support::{
            REACH_LEDGE_MAP, reachability_ledge_bsp, reachability_ledge_entities,
        };

        let bytes = reachability_ledge_bsp(&reachability_ledge_entities("ohlreachnext"));
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_LEDGE_MAP}.bsp"), bytes);
        let mut game =
            Game::load(&assets as &dyn AssetSource, REACH_LEDGE_MAP).expect("the fixture loads");

        let report = compute_reachability_report(&mut game, &ReachabilityConfig::default());
        let first = &report.rounds[0];

        assert!(
            first.changelevel.reachable,
            "the ledge beyond a >72-unit drop should be reached in round 0 (no door to open)"
        );
        assert!(
            first.long_drop_cells > 0,
            "at least the cell landed on right after the cliff edge should be counted as a long drop"
        );
    }

    /// A gap narrower than the walking player's own jump range (run speed
    /// times jump airtime, both read from [`ohl_physics::MoveConfig`]) is
    /// crossed; the same map widened past that range is not — the jump
    /// edge's horizontal bound is a real bound, not an unlimited hop.
    #[test]
    fn a_jumpable_gap_is_reached_but_a_wider_one_is_not() {
        use crate::test_support::{REACH_GAP_MAP, reachability_gap_bsp, reachability_gap_entities};

        let config = ohl_physics::MoveConfig::default();
        let jump = JumpBounds::from_move_config(&config);

        let narrow_width = jump.horizontal - 64.0;
        let wide_width = jump.horizontal + 64.0;
        assert!(
            narrow_width > 0.0,
            "the fixture's own default jump range should comfortably fit a 64-unit margin"
        );

        let entities = reachability_gap_entities("ohlreachnext");

        let narrow_bytes = reachability_gap_bsp(narrow_width, &entities);
        let mut narrow_assets = MemoryAssets::new();
        narrow_assets.insert(&format!("maps/{REACH_GAP_MAP}.bsp"), narrow_bytes);
        let mut narrow_game = Game::load(&narrow_assets as &dyn AssetSource, REACH_GAP_MAP)
            .expect("the narrow-gap fixture loads");
        let narrow_report =
            compute_reachability_report(&mut narrow_game, &ReachabilityConfig::default());
        assert!(
            narrow_report.rounds[0].changelevel.reachable,
            "a gap narrower than the jump range should be crossed"
        );

        let wide_bytes = reachability_gap_bsp(wide_width, &entities);
        let mut wide_assets = MemoryAssets::new();
        wide_assets.insert(&format!("maps/{REACH_GAP_MAP}.bsp"), wide_bytes);
        let mut wide_game = Game::load(&wide_assets as &dyn AssetSource, REACH_GAP_MAP)
            .expect("the wide-gap fixture loads");
        let wide_report =
            compute_reachability_report(&mut wide_game, &ReachabilityConfig::default());
        assert!(
            !wide_report.rounds[0].changelevel.reachable,
            "a gap wider than the jump range should not be crossed"
        );
    }

    /// A gap wider than the ordinary running-jump range but narrower than
    /// the long-jump range (both read live from
    /// [`ohl_physics::MoveConfig`], never restated) is reached only when
    /// [`ReachabilityConfig::assume_longjump`] is set: without it, neither
    /// the plain step nor the ordinary jump edge crosses it; with it, the
    /// long-jump edge does, and the landing is counted in
    /// [`RoundReport::long_jump_cells`].
    #[test]
    fn a_gap_beyond_the_ordinary_jump_is_reached_only_with_assume_longjump() {
        use crate::test_support::{REACH_GAP_MAP, reachability_gap_bsp, reachability_gap_entities};

        let config = ohl_physics::MoveConfig::default();
        let ordinary_jump = JumpBounds::from_move_config(&config);
        let long_jump = JumpBounds::from_long_jump_config(&config);
        assert!(
            long_jump.horizontal > ordinary_jump.horizontal,
            "this fixture assumes a long jump reaches further than an ordinary jump"
        );
        // The walk's real crossable width for either jump kind sits
        // noticeably above each kind's own raw `horizontal` bound alone
        // (once a jump lands anywhere on solid far-side ground, the walk
        // continues toward the trigger by ordinary plain steps from
        // there) — the same reason the existing ordinary-jump regression
        // test above uses a flat +/-64 unit margin rather than the raw
        // bound directly. This offset was found empirically against this
        // exact fixture family and is pinned here as a regression margin,
        // not restated as a claimed motion-model fact.
        let width = long_jump.horizontal + GAP_MARGIN;

        let entities = reachability_gap_entities("ohlreachnext");
        let bytes = reachability_gap_bsp(width, &entities);

        let mut without_assets = MemoryAssets::new();
        without_assets.insert(&format!("maps/{REACH_GAP_MAP}.bsp"), bytes.clone());
        let mut without_game = Game::load(&without_assets as &dyn AssetSource, REACH_GAP_MAP)
            .expect("the fixture loads");
        let without_report =
            compute_reachability_report(&mut without_game, &ReachabilityConfig::default());
        assert!(
            !without_report.rounds[0].changelevel.reachable,
            "beyond the ordinary jump range, the gap should not be crossed without assume_longjump"
        );

        let mut with_assets = MemoryAssets::new();
        with_assets.insert(&format!("maps/{REACH_GAP_MAP}.bsp"), bytes);
        let mut with_game =
            Game::load(&with_assets as &dyn AssetSource, REACH_GAP_MAP).expect("the fixture loads");
        let with_config = ReachabilityConfig {
            assume_longjump: true,
            ..ReachabilityConfig::default()
        };
        let with_report = compute_reachability_report(&mut with_game, &with_config);
        assert!(
            with_report.rounds[0].changelevel.reachable,
            "with assume_longjump, the long-jump edge should cross the gap"
        );
        assert!(
            with_report.rounds[0].long_jump_cells > 0,
            "the landing should be counted as reached only by the long-jump edge"
        );
    }

    /// A corridor blocked by a `func_breakable` (`health > 0`, not
    /// `trigger_only`) hides a `trigger_changelevel` on round 0 exactly
    /// like a closed door does — but, unlike a door, it stays blocked on
    /// every later round when the walk is run without
    /// [`ReachabilityConfig::assume_armed`]: this walk carries no weapon,
    /// so nothing here ever knocks it down. Setting the flag instead
    /// reports it openable-by-damage on round 0 and reaches the trigger on
    /// round 1, the same round-advance shape a door's own regression test
    /// (`changelevel_becomes_reachable_only_after_the_door_opens`) proves.
    #[test]
    fn a_breakable_is_openable_only_when_armed_is_assumed() {
        use crate::test_support::{REACH_BREAKABLE_MAP, reachability_breakable_bsp};

        let bytes = reachability_breakable_bsp("ohlreachnext");
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_BREAKABLE_MAP}.bsp"), bytes);

        let unarmed_config = ReachabilityConfig::default();
        assert!(!unarmed_config.assume_armed);
        let mut unarmed_game =
            Game::load(&assets as &dyn AssetSource, REACH_BREAKABLE_MAP).expect("fixture loads");
        let unarmed_report = compute_reachability_report(&mut unarmed_game, &unarmed_config);
        assert!(
            !unarmed_report.rounds[0].changelevel.reachable,
            "the breakable should keep the changelevel trigger unreachable without --reachability-assume-armed"
        );
        let unarmed_class = unarmed_report.rounds[0]
            .frontier_classes
            .iter()
            .find(|class| class.classname == "func_breakable")
            .expect("the func_breakable should be on the frontier");
        assert!(!unarmed_class.damage_openable);
        assert_eq!(unarmed_report.rounds[0].breakables_opened, 0);
        assert!(
            unarmed_report.rounds.len() == 1,
            "no round-advance edge exists without an assumed weapon, so the walk should stop after round 0"
        );

        let armed_config = ReachabilityConfig {
            assume_armed: true,
            ..ReachabilityConfig::default()
        };
        let mut armed_game =
            Game::load(&assets as &dyn AssetSource, REACH_BREAKABLE_MAP).expect("fixture loads");
        let armed_report = compute_reachability_report(&mut armed_game, &armed_config);
        assert!(
            armed_report.rounds.len() >= 2,
            "assuming a weapon should open a round-advance edge, expected at least two rounds, got {}",
            armed_report.rounds.len()
        );
        let armed_first = &armed_report.rounds[0];
        assert!(!armed_first.changelevel.reachable);
        assert_eq!(armed_first.breakables_opened, 1);
        let armed_class = armed_first
            .frontier_classes
            .iter()
            .find(|class| class.classname == "func_breakable")
            .expect("the func_breakable should be on the frontier");
        assert!(armed_class.damage_openable);
        assert!(
            armed_report.rounds[1].changelevel.reachable,
            "with the breakable's brush detached, the walk should now reach the changelevel trigger"
        );
    }

    /// A corridor blocked by a `func_pushable` hides a `trigger_changelevel`
    /// on round 0; unlike a breakable, no assumed weapon is needed — the
    /// pushable is reported openable-by-push on round 0 regardless, and
    /// the trigger is reached on round 1 once its brush is detached
    /// (approximating the crate being shoved out of the way).
    #[test]
    fn a_pushable_is_openable_by_push_without_any_assumed_weapon() {
        use crate::test_support::{REACH_PUSHABLE_MAP, reachability_pushable_bsp};

        let bytes = reachability_pushable_bsp("ohlreachnext");
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_PUSHABLE_MAP}.bsp"), bytes);
        let mut game =
            Game::load(&assets as &dyn AssetSource, REACH_PUSHABLE_MAP).expect("fixture loads");

        let config = ReachabilityConfig::default();
        assert!(!config.assume_armed);
        let report = compute_reachability_report(&mut game, &config);

        assert!(
            report.rounds.len() >= 2,
            "pushing needs no assumed weapon, so the pushable should open a round-advance edge on its own"
        );
        let first = &report.rounds[0];
        assert!(!first.changelevel.reachable);
        assert_eq!(first.pushables_opened, 1);
        let class = first
            .frontier_classes
            .iter()
            .find(|class| class.classname == "func_pushable")
            .expect("the func_pushable should be on the frontier");
        assert!(class.push_openable);
        assert!(
            !class.damage_openable,
            "no weapon was assumed for this walk"
        );

        assert!(
            report.rounds[1].changelevel.reachable,
            "with the pushable's brush detached, the walk should now reach the changelevel trigger"
        );
    }
}
