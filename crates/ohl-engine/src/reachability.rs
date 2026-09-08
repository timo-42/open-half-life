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
use ohl_game::registry::{BrushBounds, ChangeLevel, ClassName, Door, MoverState};
use ohl_physics::{BrushId, CollisionModel, Hull};

use crate::{Game, USE_RADIUS};

/// The grid spacing the walk steps in, in world units.
pub const CELL_SIZE: f32 = 16.0;

/// The documented step-up height (`sv_stepsize`; see
/// `ohl_physics::movement`'s own module doc and `docs/FORMAT_SOURCES.md`).
pub const STEP_UP: f32 = 18.0;

/// How far below a stepped horizontal move the walk looks for a new floor.
/// A drop further than this (a pit, a ledge) is treated as leaving the
/// walkable graph rather than as a new reachable cell.
pub const DROP: f32 = 72.0;

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
}

impl Default for ReachabilityConfig {
    fn default() -> Self {
        Self {
            cell_cap: 40_000,
            max_rounds: 6,
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
    /// How many doors this round found and opened for the *next* round
    /// (`0` on the last round, since nothing further needed opening).
    pub doors_opened: usize,
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
}

/// The step-up/move/drop walk itself: from `start`, breadth-first over the
/// 16-unit grid, using [`Hull::Standing`] against `collision` exactly as
/// the walking player would.
fn walk(collision: &CollisionModel, start: Vec3, cap: usize) -> WalkResult {
    let hull = Hull::Standing;
    let mut visited_cells: HashSet<Cell> = HashSet::new();
    let mut visited_positions = Vec::new();
    let mut frontier_brushes = HashSet::new();
    let mut queue = VecDeque::new();

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
            let horizontal = Vec3::new(dx, dy, 0.0).normalize_or_zero() * CELL_SIZE;
            if horizontal == Vec3::ZERO {
                continue;
            }

            let up = collision.trace(hull, position, position + Vec3::Z * STEP_UP);
            if up.start_solid {
                continue;
            }
            let top = up.end_pos;

            let across = collision.trace(hull, top, top + horizontal);
            if across.blocked() {
                if let Some(brush) = across.brush_index {
                    frontier_brushes.insert(brush);
                }
                continue;
            }

            let down_target = across.end_pos - Vec3::Z * (STEP_UP + DROP);
            let down = collision.trace(hull, across.end_pos, down_target);
            if down.start_solid || down.fraction >= 1.0 {
                // Either embedded in solid immediately (shouldn't happen
                // after a successful horizontal move, but skip rather than
                // trust it) or no floor within the drop bound: a pit or a
                // ledge, not a new reachable cell.
                continue;
            }

            let landing = down.end_pos;
            let cell = cell_of(landing);
            if visited_cells.insert(cell) {
                visited_positions.push(landing);
                queue.push_back(landing);
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
#[must_use]
pub fn compute_reachability_report(
    game: &mut Game,
    config: &ReachabilityConfig,
) -> ReachabilityReport {
    let start = Vec3::from_array(game.player_origin());
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
            doors_opened: 0,
            capped: false,
        });
        return ReachabilityReport { rounds };
    }

    for round in 0..config.max_rounds.max(1) {
        let walk_result = {
            let Some(collision) = game.collision() else {
                break;
            };
            walk(collision, start, config.cell_cap)
        };

        let mut classes: BTreeMap<String, (usize, bool)> = BTreeMap::new();
        let mut openable_doors: Vec<BrushId> = Vec::new();
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
            let openable =
                is_closed_door && use_openable_from(game, entity, &walk_result.visited_positions);
            let entry = classes.entry(classname).or_insert((0, false));
            entry.0 += 1;
            entry.1 |= openable;
            if openable {
                openable_doors.push(*brush);
            }
        }

        let changelevel = changelevel_status(game, start, &walk_result.visited_positions);
        let doors_opened = openable_doors.len();

        rounds.push(RoundReport {
            round,
            reachable_cells: walk_result.visited_positions.len(),
            frontier_classes: classes
                .into_iter()
                .map(
                    |(classname, (instance_count, use_openable))| FrontierClass {
                        classname,
                        instance_count,
                        use_openable,
                    },
                )
                .collect(),
            changelevel,
            doors_opened,
            capped: walk_result.capped,
        });

        if openable_doors.is_empty() {
            break;
        }
        let Some(collision) = game.collision_mut() else {
            break;
        };
        for brush in openable_doors {
            collision.detach_brush(brush);
        }
    }

    ReachabilityReport { rounds }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        REACH_DOOR_MAP, reachability_changelevel_entities, reachability_door_bsp,
    };
    use crate::{AssetSource, MemoryAssets};

    fn game() -> Game {
        let bytes = reachability_door_bsp(&reachability_changelevel_entities("ohlreachnext"));
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_DOOR_MAP}.bsp"), bytes);
        Game::load(&assets as &dyn AssetSource, REACH_DOOR_MAP).expect("the fixture loads")
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
}
