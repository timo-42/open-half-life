//! A route *planner* over the same bounded walk [`crate::reachability`]
//! triages with: it answers "how does the player get there", not only
//! "can they".
//!
//! `--reachability-report` (see [`crate::reachability`]) already walks a
//! map's live collision model breadth-first and reports whether a
//! `trigger_changelevel` is reachable, round by round, opening the closed
//! doors it found on the way. What it never produced is a *route*: the
//! aggregate "reachable at round 2" left every chain-walk route
//! (`xtask/chain-routes/`) to be hand-authored, and hand-authored greedy
//! walks repeatedly failed to walk a route that report says exists
//! (`.plan/chain-hop6.md`: a greedy heading walk grinds along the first
//! wall the straight line meets; a coarse waypoint chase stalls short of
//! the trigger).
//!
//! This module records what that walk already knows and throws away. It
//! runs the same edge model — [`crate::reachability::CELL_SIZE`] grid,
//! [`ohl_physics::Hull::Standing`], the same ascend/move/drop step, the
//! same jump bounds derived from the live [`ohl_physics::MoveConfig`], the
//! same "detach a closed, use-openable door and walk again" round advance
//! — but keeps a parent link and an [`EdgeKind`] for every cell it
//! reaches, so the cell the goal was found in can be walked back to the
//! start. The resulting cell path is then:
//!
//! 1. **split at door presses.** A door this walk opened between rounds
//!    is attributed to the path point the path last stands on within
//!    [`crate::USE_RADIUS`] of that door's own brush centre (measured from
//!    the player's eye, the way [`ohl_game::find_usable_within`] measures
//!    it), before the path crosses the leaf's closed volume. A door the
//!    path crosses with no such point truncates the route there instead
//!    of being walked through while shut: the route walks up to the leaf,
//!    and the plan made from *there* is the one that presses it.
//! 2. **string-pulled.** A breadth-first grid path zigzags between the
//!    eight compass directions; a walker following it turns every few
//!    units. Each chunk between door presses is greedily shortened to the
//!    furthest later point a standing-hull trace reaches in a straight
//!    line with continuous floor beneath it ([`STRAIGHT_LINE_SAMPLE`]),
//!    which is what turns "220 grid cells" into a handful of straight
//!    runs.
//! 3. **merged into segments** ([`merge_collinear`]): consecutive points
//!    sharing a heading become one [`PlanAction::Move`].
//!
//! The output is a [`RoutePlan`]: a list of [`PlanAction`]s in world
//! terms (a heading in degrees, a distance in units, a door's own
//! documented open time in seconds). Turning that into scripted input is
//! the caller's job — the tick rate and the scripted-input grammar both
//! belong to the composition root, not to the engine.
//!
//! # What this module never does
//!
//! It never *reports* anything. Every coordinate here stays in memory:
//! [`RoutePlan`] is handed back to the caller, which writes it out as
//! script commands and prints only aggregates (see `docs/CLEAN_ROOM.md`,
//! and [`crate::reachability`]'s own identical note). Nothing in this
//! module logs.
//!
//! It also never plans an edge the caller cannot express. A long-jump
//! edge ([`PlanConfig::assume_longjump`]) needs an item and an input
//! combination no route file has, so a path that depends on one fails as
//! [`PlanError::UnsupportedEdge`] instead of producing a script that
//! silently falls short. Breakables, pushables and pendulums are never
//! opened at all here (unlike [`crate::reachability`], which reports them
//! as round-advance edges): a closed, use-openable door is the only
//! obstacle a script can honestly clear, so anything else simply leaves
//! the goal [`PlanError::GoalUnreachable`].

use std::collections::{HashMap, HashSet, VecDeque};

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::registry::{BrushBounds, ChangeLevel, ClassName, Door, MoverState};
use ohl_physics::{BrushId, CollisionModel, Hull};

use crate::reachability::{
    CELL_SIZE, Cell, DIRECTIONS, DROP, EdgeOutcome, JumpBounds, STEP_UP,
    bounds_contains_with_margin, cell_of, entity_for_brush, settle_start, try_edge,
};
use crate::{Game, USE_RADIUS};

/// The classname [`PlanConfig::default`] plans a route to.
pub const DEFAULT_GOAL_CLASSNAME: &str = "trigger_changelevel";

/// How far apart the floor beneath a candidate straight-line shortcut is
/// sampled, in world units. A shortcut is only taken when every sample
/// along it has floor within a step of the line: a clear hull trace alone
/// would happily cut a corner across a pit.
pub const STRAIGHT_LINE_SAMPLE: f32 = CELL_SIZE;

/// How which edge one path step was reached by: what a script has to do
/// to cross it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A plain step onto a floor at (roughly) the same height.
    Walk,
    /// A plain step that ascended, within the walk's own step-up bound.
    Step,
    /// A plain step whose landing fell further than
    /// [`crate::reachability::DROP`] below it: a one-way fall.
    Drop,
    /// An ordinary running jump (the walk's jump edge).
    Jump,
    /// The long-jump edge, which needs an item and an input combination
    /// no route file expresses — see this module's own doc comment.
    LongJump,
}

impl EdgeKind {
    /// Whether a straight-line shortcut may absorb this edge. A jump, a
    /// long jump and a one-way fall are all committed motions whose exact
    /// take-off point matters; only ordinary ground movement may be
    /// straightened.
    #[must_use]
    pub fn is_ground_movement(self) -> bool {
        matches!(self, Self::Walk | Self::Step)
    }
}

/// One point on a planned path: where it stands, and which edge got it
/// there from the previous point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    /// The landing position, in world units.
    pub position: Vec3,
    /// The edge the walk crossed to reach [`Self::position`].
    pub kind: EdgeKind,
}

/// One step of a planned route, in world terms: the caller converts these
/// into whatever scripted-input grammar it owns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanAction {
    /// Face `yaw` degrees (the `atan2(dy, dx)` convention
    /// [`ohl_physics::PlayerController::wish_move`] walks along) and move
    /// `distance` world units forward, holding jump throughout when
    /// `jump` is set.
    Move {
        /// The heading to face, in degrees.
        yaw: f32,
        /// How far to travel along it, in world units.
        distance: f32,
        /// Whether this is the walk's jump edge rather than ground
        /// movement.
        jump: bool,
    },
    /// Face `yaw` degrees, press use once, and wait `open_seconds` for the
    /// door to finish opening (its own `delay` plus its travel time, both
    /// read from the door's live [`ohl_game::registry::Door`]).
    UseDoor {
        /// The heading to face while pressing use, in degrees.
        yaw: f32,
        /// How long the door takes to open, in seconds.
        open_seconds: f32,
    },
}

/// A planned route, plus the bounded aggregates a caller may report.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePlan {
    /// The route itself, in order.
    pub actions: Vec<PlanAction>,
    /// How many grid cells the final round's walk reached.
    pub cells: usize,
    /// How many rounds the walk ran (one more than the number of rounds
    /// that opened a door).
    pub rounds: usize,
    /// How many path points the walk's own parent links produced, before
    /// straightening and merging.
    pub path_points: usize,
    /// How many door presses the route needs.
    pub doors: usize,
    /// Whether this route actually ends inside a goal volume.
    ///
    /// `false` for a *partial* plan: the goal was not reachable at all,
    /// so the route walks to whichever reached cell sits closest to it
    /// instead. A partial plan is progress, not an answer — its caller is
    /// expected to walk it, look again from there, and only accept a
    /// route once the goal has actually been reached (a map that opens
    /// its own way on a schedule, or another character who opens a door
    /// once the player is standing at it, both need exactly that).
    pub reaches_goal: bool,
    /// How far the route's own end point still is from the nearest goal
    /// volume's centre, rounded to
    /// [`crate::reachability::DISTANCE_ROUNDING`] units exactly as that
    /// module's own report rounds a distance. Zero for a route that
    /// reaches the goal. This is what tells one partial plan from
    /// another: a caller walking partial plans one after another has to
    /// know whether it is still getting closer or has stopped.
    pub goal_distance_rounded: f32,
    /// How far the route's own *start* is from the nearest goal volume's
    /// centre, rounded the same way — the same "~N units from spawn"
    /// aggregate [`crate::reachability`]'s report prints, for the point
    /// this plan was made from. A caller walking one plan after another
    /// watches this shrink; when it stops shrinking, the walk is stuck.
    pub start_distance_rounded: f32,
}

impl RoutePlan {
    /// How many [`PlanAction::Move`] segments the route holds.
    #[must_use]
    pub fn segments(&self) -> usize {
        self.actions
            .iter()
            .filter(|action| matches!(action, PlanAction::Move { .. }))
            .count()
    }
}

/// What to plan, and how much work the search may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanConfig {
    /// The per-round cell cap, exactly as
    /// [`crate::reachability::ReachabilityConfig::cell_cap`].
    pub cell_cap: usize,
    /// The round cap, exactly as
    /// [`crate::reachability::ReachabilityConfig::max_rounds`].
    pub max_rounds: usize,
    /// The classname to plan a route to; [`DEFAULT_GOAL_CLASSNAME`] by
    /// default. Only an entity with a brush volume
    /// ([`ohl_game::registry::BrushBounds`]) can be a goal: the plan ends
    /// on a cell inside that volume.
    pub goal_classname: String,
    /// Destination map names a goal must *not* name, lowercased by the
    /// caller or not — matched case-insensitively.
    ///
    /// A map's own level-change triggers include the one the player just
    /// arrived through, and it is usually the closest one to them. A
    /// route planned to that trigger walks straight back where it came
    /// from, which is a route-authoring mistake rather than progress (see
    /// `xtask/src/chain_walk.rs`'s own re-entry rule). A caller that
    /// knows which maps it has already been in says so here; the names
    /// stay in memory and are never printed, exactly as `run_chained`'s
    /// own visited list is.
    ///
    /// Only a goal that declares a destination map
    /// ([`ohl_game::registry::ChangeLevel`]) can be excluded this way; a
    /// `--plan-goal` classname without one is unaffected.
    pub avoid_goal_maps: Vec<String>,
    /// Whether the walk may use the long-jump edge. Off by default, and
    /// planning fails with [`PlanError::UnsupportedEdge`] when the route
    /// found actually depends on one — see this module's own doc comment.
    pub assume_longjump: bool,
}

impl Default for PlanConfig {
    fn default() -> Self {
        let reachability = crate::reachability::ReachabilityConfig::default();
        Self {
            cell_cap: reachability.cell_cap,
            max_rounds: reachability.max_rounds,
            goal_classname: DEFAULT_GOAL_CLASSNAME.to_string(),
            avoid_goal_maps: Vec::new(),
            assume_longjump: false,
        }
    }
}

/// Why a route could not be planned. Every variant has one fixed
/// message and carries nothing map-derived, so a caller may print it
/// unconditionally (`docs/CLEAN_ROOM.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    /// The map has no usable collision hulls to walk.
    NoCollision,
    /// No entity of the goal classname declares a brush volume.
    NoGoalEntity,
    /// The bounded walk never reached the goal, even after opening every
    /// closed, use-openable door it found.
    GoalUnreachable,
    /// The route depends on an edge no scripted-input grammar expresses
    /// (today: the long-jump edge).
    UnsupportedEdge,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoCollision => "the map has no usable collision hulls",
            Self::NoGoalEntity => "no entity of the goal classname declares a brush volume",
            Self::GoalUnreachable => {
                "the bounded walk did not reach the goal, even with every reachable door opened"
            }
            Self::UnsupportedEdge => {
                "the route depends on an edge the scripted-input grammar cannot express"
            }
        })
    }
}

impl std::error::Error for PlanError {}

/// A refused plan: why, and the bounded aggregates the search got to
/// before it gave up — the same triage numbers
/// [`crate::reachability`]'s own report prints, so a caller can tell "the
/// walk barely left the room" from "the walk flooded the map and still
/// found nothing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanRejection {
    /// Why the plan was refused.
    pub error: PlanError,
    /// How many grid cells the last search reached.
    pub cells: usize,
    /// How many rounds it ran.
    pub rounds: usize,
}

impl PlanRejection {
    /// A refusal carrying the aggregates the search got to.
    #[must_use]
    pub fn new(error: PlanError, cells: usize, rounds: usize) -> Self {
        Self {
            error,
            cells,
            rounds,
        }
    }
}

impl std::fmt::Display for PlanRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} (the search reached {} cell(s) in {} round(s))",
            self.error, self.cells, self.rounds
        )
    }
}

impl std::error::Error for PlanRejection {}

/// One goal volume: its bounds, and its centre (the point a reached cell
/// is scored against).
struct GoalVolume {
    bounds: BrushBounds,
    center: Vec3,
}

/// A door this walk opened between rounds, with everything the route
/// needs to press it: where to stand relative to it, and how long it
/// takes to open.
#[derive(Debug, Clone, Copy)]
struct OpenedDoor {
    /// The leaf's placed brush centre, the point
    /// [`ohl_game::find_usable_within`] measures against.
    center: Vec3,
    /// The leaf's closed volume, captured before the brush was detached.
    bounds: BrushBounds,
    /// The door's own `delay` plus its travel time, in seconds.
    open_seconds: f32,
}

/// One round's walk, with the parent links [`crate::reachability`]'s own
/// walk discards.
struct Trace {
    /// The accepted landing position for every visited cell.
    landing: HashMap<Cell, Vec3>,
    /// For every cell but the start, the cell it was reached from, the
    /// edge that reached it, and (for a [`sidestep`] edge) the point the
    /// walk stepped aside to first.
    parent: HashMap<Cell, ParentLink>,
    /// Every landing, in visit order — what a door's use-proximity test
    /// is checked against.
    order: Vec<Vec3>,
    /// Every attached brush that blocked a step which would otherwise
    /// have reached a new cell.
    frontier: HashSet<BrushId>,
}

/// How one cell was reached: from which cell, by which edge, and — when
/// the edge had to step aside to thread a gap narrower than the grid —
/// through which intermediate point.
#[derive(Debug, Clone, Copy)]
struct ParentLink {
    /// The cell this edge started from.
    from: Cell,
    /// The edge that crossed.
    kind: EdgeKind,
    /// The point the walk stepped aside to before crossing, for a
    /// [`sidestep`] edge; the route has to visit it too, or it walks into
    /// the frame the sidestep exists to clear.
    via: Option<Vec3>,
}

/// How far to either side a blocked plain step is retried from, in world
/// units: half a grid cell.
///
/// A doorway the player fits through by a hand's width is not a doorway
/// the [`CELL_SIZE`] grid fits through: whether a grid-aligned step
/// threads it is a matter of where the walk's own cells happen to have
/// landed. A published map is full of such frames (`.plan/chain-hop6.md`
/// reports a live probe stalling at exactly one), and a player simply
/// steps aside a little. So does this walk: when the plain step in a
/// direction is blocked, it is retried from half a cell to either side,
/// and the point it stepped aside to is kept on the path.
///
/// [`crate::reachability`]'s own walk deliberately does *not* do this: a
/// triage report answers "is anything reachable at all", where a coarser,
/// cheaper edge set is the right trade. A route has to be walkable by a
/// body, which is a stricter question.
const SIDESTEP: f32 = CELL_SIZE / 2.0;

/// Retries a blocked plain step from half a cell to either side of
/// `position`, returning the point stepped aside to and the outcome.
fn sidestep(
    collision: &CollisionModel,
    hull: Hull,
    position: Vec3,
    direction: Vec3,
) -> Option<(Vec3, Vec3, f32)> {
    let perpendicular = Vec3::new(-direction.y, direction.x, 0.0);
    for offset in [SIDESTEP, -SIDESTEP] {
        let aside = position + perpendicular * offset;
        let across = collision.trace(hull, position, aside);
        if across.start_solid || across.blocked() {
            continue;
        }
        if let EdgeOutcome::Landed {
            position: landing,
            drop,
        } = try_edge(
            collision,
            hull,
            across.end_pos,
            direction,
            STEP_UP,
            CELL_SIZE,
        ) {
            return Some((across.end_pos, landing, drop));
        }
    }
    None
}

/// Which [`EdgeKind`] an accepted plain-step outcome represents.
fn plain_edge_kind(from: Vec3, to: Vec3, drop: f32) -> EdgeKind {
    if drop > DROP {
        EdgeKind::Drop
    } else if to.z > from.z + 0.5 {
        EdgeKind::Step
    } else {
        EdgeKind::Walk
    }
}

/// One direction's outcome from one cell.
enum Crossing {
    /// A new landing, by `kind`, optionally through a [`sidestep`] point.
    Landed {
        landing: Vec3,
        kind: EdgeKind,
        via: Option<Vec3>,
    },
    /// Nothing crossed; the brush that stopped the plain step, if it was
    /// a brush that stopped it.
    Blocked(Option<BrushId>),
}

/// Tries every edge this walk knows in one direction, in the order
/// [`crate::reachability`]'s own walk tries them — plain step, then (new
/// here) the same step from half a cell aside, then an ordinary jump,
/// then a long jump when one is allowed.
fn cross(
    collision: &CollisionModel,
    position: Vec3,
    direction: Vec3,
    jump: JumpBounds,
    long_jump: Option<JumpBounds>,
) -> Crossing {
    let hull = Hull::Standing;
    let plain = try_edge(collision, hull, position, direction, STEP_UP, CELL_SIZE);
    if let EdgeOutcome::Landed {
        position: landing,
        drop,
    } = plain
    {
        return Crossing::Landed {
            landing,
            kind: plain_edge_kind(position, landing, drop),
            via: None,
        };
    }
    if let Some((aside, landing, drop)) = sidestep(collision, hull, position, direction) {
        return Crossing::Landed {
            landing,
            kind: plain_edge_kind(position, landing, drop),
            via: Some(aside),
        };
    }
    if let EdgeOutcome::Landed {
        position: landing, ..
    } = try_edge(
        collision,
        hull,
        position,
        direction,
        jump.ascend,
        jump.horizontal,
    ) {
        return Crossing::Landed {
            landing,
            kind: EdgeKind::Jump,
            via: None,
        };
    }
    if let Some(long) = long_jump
        && let EdgeOutcome::Landed {
            position: landing, ..
        } = try_edge(
            collision,
            hull,
            position,
            direction,
            long.ascend,
            long.horizontal,
        )
    {
        return Crossing::Landed {
            landing,
            kind: EdgeKind::LongJump,
            via: None,
        };
    }
    Crossing::Blocked(match plain {
        EdgeOutcome::BlockedAcross(brush) => brush,
        EdgeOutcome::BlockedUp | EdgeOutcome::NoFloor | EdgeOutcome::Landed { .. } => None,
    })
}

/// Records one newly reached cell, ignoring a cell already visited (the
/// first way a cell is reached is the shortest, breadth-first).
fn record(trace: &mut Trace, queue: &mut VecDeque<Vec3>, landing: Vec3, link: ParentLink) {
    if let std::collections::hash_map::Entry::Vacant(entry) = trace.landing.entry(cell_of(landing))
    {
        entry.insert(landing);
        trace.parent.insert(cell_of(landing), link);
        trace.order.push(landing);
        queue.push_back(landing);
    }
}

/// The walk itself: [`cross`]'s edge ladder from every visited cell in
/// all eight compass directions, recording a parent link and an
/// [`EdgeKind`] per cell.
fn walk_with_parents(
    collision: &CollisionModel,
    start: Vec3,
    cap: usize,
    jump: JumpBounds,
    long_jump: Option<JumpBounds>,
) -> Trace {
    let mut trace = Trace {
        landing: HashMap::new(),
        parent: HashMap::new(),
        order: Vec::new(),
        frontier: HashSet::new(),
    };
    let mut queue = VecDeque::new();
    trace.landing.insert(cell_of(start), start);
    trace.order.push(start);
    queue.push_back(start);

    while let Some(position) = queue.pop_front() {
        let from = cell_of(position);
        for (dx, dy) in DIRECTIONS {
            if trace.landing.len() >= cap {
                return trace;
            }
            let direction = Vec3::new(dx, dy, 0.0).normalize_or_zero();
            if direction == Vec3::ZERO {
                continue;
            }
            match cross(collision, position, direction, jump, long_jump) {
                Crossing::Landed { landing, kind, via } => {
                    record(
                        &mut trace,
                        &mut queue,
                        landing,
                        ParentLink { from, kind, via },
                    );
                }
                Crossing::Blocked(Some(brush)) => {
                    trace.frontier.insert(brush);
                }
                Crossing::Blocked(None) => {}
            }
        }
    }
    trace
}

/// Every entity of `classname` that declares a brush volume, minus any
/// whose own destination map is one the caller asked to avoid (see
/// [`PlanConfig::avoid_goal_maps`]).
fn goal_volumes(game: &Game, config: &PlanConfig) -> Vec<GoalVolume> {
    let mut volumes = Vec::new();
    for (entity, name, bounds) in &mut game
        .registry()
        .world
        .query::<(Entity, &ClassName, &BrushBounds)>()
    {
        if !name.0.eq_ignore_ascii_case(&config.goal_classname) {
            continue;
        }
        let avoided = game
            .registry()
            .world
            .get::<&ChangeLevel>(entity)
            .is_ok_and(|change| {
                config
                    .avoid_goal_maps
                    .iter()
                    .any(|avoid| avoid.eq_ignore_ascii_case(&change.map))
            });
        if avoided {
            continue;
        }
        volumes.push(GoalVolume {
            bounds: *bounds,
            center: Vec3::new(
                f32::midpoint(bounds.mins.x, bounds.maxs.x),
                f32::midpoint(bounds.mins.y, bounds.maxs.y),
                f32::midpoint(bounds.mins.z, bounds.maxs.z),
            ),
        });
    }
    volumes
}

/// The reached cell closest to any goal volume's centre, for a partial
/// plan: where to stand and look again when nothing reaches the goal
/// itself. `None` when the walk reached nothing but its own start.
fn pick_nearest_cell(trace: &Trace, goals: &[GoalVolume], start: Cell) -> Option<Cell> {
    let mut best: Option<(Cell, f32)> = None;
    for (cell, position) in &trace.landing {
        if *cell == start {
            continue;
        }
        let Some(score) = goals
            .iter()
            .map(|goal| position.distance(goal.center))
            .min_by(f32::total_cmp)
        else {
            continue;
        };
        if best.is_none_or(|(_, best_score)| score < best_score) {
            best = Some((*cell, score));
        }
    }
    best.map(|(cell, _)| cell)
}

/// The reached cell to walk back from: whichever cell inside a goal
/// volume sits closest to that volume's own centre, so the player ends up
/// well inside the trigger rather than clipping its outermost boundary
/// cell (which is where a margin-expanded containment test would
/// otherwise be happiest).
fn pick_goal_cell(trace: &Trace, goals: &[GoalVolume]) -> Option<Cell> {
    let mut best: Option<(Cell, f32)> = None;
    for (cell, position) in &trace.landing {
        for goal in goals {
            if !bounds_contains_with_margin(&goal.bounds, *position) {
                continue;
            }
            let score = position.distance(goal.center);
            if best.is_none_or(|(_, best_score)| score < best_score) {
                best = Some((*cell, score));
            }
        }
    }
    best.map(|(cell, _)| cell)
}

/// Walks `trace`'s parent links back from `goal` and returns the path
/// from the start cell (inclusive, as the first point) to `goal`.
fn path_to(trace: &Trace, start: Vec3, goal: Cell) -> Vec<PathPoint> {
    let mut reversed = Vec::new();
    let mut cell = goal;
    let start_cell = cell_of(start);
    while cell != start_cell {
        let Some(position) = trace.landing.get(&cell) else {
            break;
        };
        let Some(link) = trace.parent.get(&cell).copied() else {
            break;
        };
        reversed.push(PathPoint {
            position: *position,
            kind: link.kind,
        });
        if let Some(via) = link.via {
            reversed.push(PathPoint {
                position: via,
                kind: EdgeKind::Walk,
            });
        }
        cell = link.from;
    }
    reversed.push(PathPoint {
        position: start,
        kind: EdgeKind::Walk,
    });
    reversed.reverse();
    reversed
}

/// Every closed, use-openable door on this round's frontier, in the shape
/// the route planner needs them.
///
/// "Use-openable" is the same proximity test
/// [`crate::reachability`] runs, measured from the player's *eye* (their
/// origin plus the standing view height) rather than their feet, because
/// that is the position the engine's own use press is dispatched from
/// (`ohl_engine::Systems`).
fn openable_doors(game: &Game, trace: &Trace) -> Vec<(BrushId, OpenedDoor)> {
    let eye = Vec3::Z * game.move_config().view_height_standing;
    let mut doors = Vec::new();
    for brush in &trace.frontier {
        let Some(entity) = entity_for_brush(game, *brush) else {
            continue;
        };
        let Ok(door) = game.registry().world.get::<&Door>(entity) else {
            continue;
        };
        if door.state != MoverState::Closed {
            continue;
        }
        let Some(center) = ohl_game::pose::brush_center(game.registry(), entity) else {
            continue;
        };
        let Ok(bounds) = game.registry().world.get::<&BrushBounds>(entity) else {
            continue;
        };
        if !trace
            .order
            .iter()
            .any(|position| (*position + eye).distance(center) <= USE_RADIUS)
        {
            continue;
        }
        let travel = if door.speed > 0.0 {
            door.travel_distance.abs() / door.speed
        } else {
            0.0
        };
        doors.push((
            *brush,
            OpenedDoor {
                center,
                bounds: *bounds,
                open_seconds: door.delay.max(0.0) + travel,
            },
        ));
    }
    doors
}

/// Where a route presses the doors it crosses, and how far of it can be
/// walked at all.
struct DoorMarks {
    /// The door to press at each path index.
    presses: HashMap<usize, OpenedDoor>,
    /// How much of the path is usable: shorter than the path itself when
    /// it crosses a leaf no point on it stands in reach of, in which case
    /// the route stops at the last point before that leaf.
    usable_len: usize,
}

/// Attributes each opened door to the path point the route should press
/// it from: the last point standing within [`USE_RADIUS`] of the leaf's
/// centre before the path first crosses the leaf's own closed volume.
///
/// A leaf the path crosses without ever standing in reach of truncates
/// the route there rather than failing it: walking *up to* a door is
/// progress, and the next plan — made from a point that is now beside the
/// leaf — is the one that presses it. A path truncated to nothing is how
/// a caller learns that this door is the thing to wait at.
fn attribute_doors(path: &[PathPoint], doors: &[OpenedDoor], eye: Vec3) -> DoorMarks {
    let mut presses: HashMap<usize, OpenedDoor> = HashMap::new();
    let mut usable_len = path.len();
    for door in doors {
        let Some(crossing) = path
            .iter()
            .position(|point| bounds_contains_with_margin(&door.bounds, point.position))
        else {
            continue;
        };
        match path[..crossing]
            .iter()
            .rposition(|point| (point.position + eye).distance(door.center) <= USE_RADIUS)
        {
            Some(press) => {
                presses.insert(press, *door);
            }
            None => usable_len = usable_len.min(crossing),
        }
    }
    presses.retain(|index, _| *index < usable_len);
    DoorMarks {
        presses,
        usable_len,
    }
}

/// Whether a straight standing-hull line from `from` to `to` is walkable:
/// clear of solids at step height, no more than a step of height change,
/// and with continuous floor beneath every [`STRAIGHT_LINE_SAMPLE`] of it.
fn straight_line_is_walkable(collision: &CollisionModel, from: Vec3, to: Vec3) -> bool {
    if (to.z - from.z).abs() > STEP_UP {
        return false;
    }
    let lifted_from = from + Vec3::Z * STEP_UP;
    let lifted_to = to + Vec3::Z * STEP_UP;
    let across = collision.trace(Hull::Standing, lifted_from, lifted_to);
    if across.start_solid || across.blocked() {
        return false;
    }
    let span = lifted_from.distance(lifted_to);
    if span <= f32::EPSILON {
        return true;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a sample count over one path segment of a published map's own scale"
    )]
    let samples = (span / STRAIGHT_LINE_SAMPLE).ceil() as usize;
    for index in 1..=samples {
        #[allow(clippy::cast_precision_loss, reason = "a small sample index")]
        let fraction = index as f32 / samples as f32;
        let point = lifted_from.lerp(lifted_to, fraction);
        let down = collision.trace(Hull::Standing, point, point - Vec3::Z * (STEP_UP + DROP));
        if down.start_solid || down.fraction >= 1.0 {
            return false;
        }
        let expected = from.z + (to.z - from.z) * fraction;
        if (down.end_pos.z - expected).abs() > STEP_UP {
            return false;
        }
    }
    true
}

/// Greedily straightens one chunk of ground movement: from each point,
/// jump ahead to the furthest later point a
/// [`straight_line_is_walkable`] line reaches. Points reached by a jump,
/// a long jump or a one-way fall are never absorbed (see
/// [`EdgeKind::is_ground_movement`]).
fn string_pull(collision: &CollisionModel, chunk: &[PathPoint]) -> Vec<PathPoint> {
    let mut pulled = Vec::new();
    if chunk.is_empty() {
        return pulled;
    }
    pulled.push(chunk[0]);
    let mut index = 0usize;
    while index + 1 < chunk.len() {
        let mut best = index + 1;
        let mut candidate = index + 1;
        while candidate < chunk.len() && chunk[candidate].kind.is_ground_movement() {
            if straight_line_is_walkable(
                collision,
                chunk[index].position,
                chunk[candidate].position,
            ) {
                best = candidate;
            }
            candidate += 1;
        }
        pulled.push(chunk[best]);
        index = best;
    }
    pulled
}

/// The heading, in degrees, from `from` to `to`, or `None` when the two
/// are (horizontally) the same point.
fn heading(from: Vec3, to: Vec3) -> Option<(f32, f32)> {
    let delta = to - from;
    let distance = delta.truncate().length();
    if distance < f32::EPSILON {
        return None;
    }
    Some((delta.y.atan2(delta.x).to_degrees(), distance))
}

/// How close two headings must be, in degrees, to merge into one segment.
const HEADING_EPSILON: f32 = 0.5;

/// Merges a path's consecutive points into one [`PlanAction::Move`] per
/// run of collinear ground movement.
///
/// A jump edge is never merged with anything: its take-off point and its
/// exact distance are what make it a jump rather than a walk. A point at
/// (horizontally) the same place as the previous one contributes nothing
/// and is dropped: there is no heading to face along it.
#[must_use]
pub fn merge_collinear(path: &[PathPoint]) -> Vec<PlanAction> {
    let mut actions: Vec<PlanAction> = Vec::new();
    for window in path.windows(2) {
        let (from, to) = (window[0], window[1]);
        let Some((yaw, distance)) = heading(from.position, to.position) else {
            continue;
        };
        let jump = matches!(to.kind, EdgeKind::Jump | EdgeKind::LongJump);
        if let Some(PlanAction::Move {
            yaw: last_yaw,
            distance: last_distance,
            jump: false,
        }) = actions.last_mut()
            && !jump
            && (*last_yaw - yaw).abs() <= HEADING_EPSILON
        {
            *last_distance += distance;
            continue;
        }
        actions.push(PlanAction::Move {
            yaw,
            distance,
            jump,
        });
    }
    actions
}

/// Splits `path` at its door presses, straightens each chunk, and emits
/// the merged actions with a [`PlanAction::UseDoor`] between them.
fn actions_for(
    collision: &CollisionModel,
    path: &[PathPoint],
    marks: &HashMap<usize, OpenedDoor>,
    eye: Vec3,
) -> Vec<PlanAction> {
    let mut actions = Vec::new();
    let mut chunk_start = 0usize;
    for index in 0..path.len() {
        let Some(door) = marks.get(&index) else {
            continue;
        };
        actions.extend(merge_collinear(&string_pull(
            collision,
            &path[chunk_start..=index],
        )));
        let yaw = heading(path[index].position + eye, door.center).map_or(0.0, |(yaw, _)| yaw);
        actions.push(PlanAction::UseDoor {
            yaw,
            open_seconds: door.open_seconds,
        });
        chunk_start = index;
    }
    actions.extend(merge_collinear(&string_pull(
        collision,
        &path[chunk_start..],
    )));
    actions
}

/// Plans a route from `game`'s current player position to the nearest
/// reachable cell inside a [`PlanConfig::goal_classname`] volume.
///
/// Like [`crate::reachability::compute_reachability_report`], this
/// mutates `game`'s player collision model (it detaches the doors it
/// decides to open), so call it on a `Game` loaded — or restored from a
/// snapshot — solely for this analysis.
///
/// # Errors
/// One of [`PlanError`]'s fixed reasons; none of them carries anything
/// map-derived.
pub fn plan_route(game: &mut Game, config: &PlanConfig) -> Result<RoutePlan, PlanRejection> {
    if game.collision().is_none() {
        return Err(PlanRejection::new(PlanError::NoCollision, 0, 0));
    }
    let goals = goal_volumes(game, config);
    if goals.is_empty() {
        return Err(PlanRejection::new(PlanError::NoGoalEntity, 0, 0));
    }

    let jump = JumpBounds::from_move_config(game.move_config());
    let long_jump = config
        .assume_longjump
        .then(|| JumpBounds::from_long_jump_config(game.move_config()));
    let start = match game.collision() {
        Some(collision) => settle_start(collision, Vec3::from_array(game.player_origin())),
        None => return Err(PlanRejection::new(PlanError::NoCollision, 0, 0)),
    };

    let mut doors: Vec<OpenedDoor> = Vec::new();
    let mut cells = 0usize;
    for round in 0..config.max_rounds.max(1) {
        let trace = {
            let Some(collision) = game.collision() else {
                return Err(PlanRejection::new(PlanError::NoCollision, cells, round));
            };
            walk_with_parents(collision, start, config.cell_cap, jump, long_jump)
        };
        cells = trace.landing.len();
        let rounds = round + 1;
        if let Some(goal) = pick_goal_cell(&trace, &goals) {
            let search = Search {
                start,
                goals: &goals,
                doors: &doors,
            };
            return build_plan(game, &search, &trace, goal, (cells, rounds), true);
        }

        let openable = openable_doors(game, &trace);
        if openable.is_empty() {
            // Nothing reaches the goal and no door left to open: walk as
            // close to it as this map lets us and let the caller look
            // again from there (see `RoutePlan::reaches_goal`).
            let Some(nearest) = pick_nearest_cell(&trace, &goals, cell_of(start)) else {
                return Err(PlanRejection::new(
                    PlanError::GoalUnreachable,
                    cells,
                    rounds,
                ));
            };
            let search = Search {
                start,
                goals: &goals,
                doors: &doors,
            };
            return build_plan(game, &search, &trace, nearest, (cells, rounds), false);
        }
        let Some(collision) = game.collision_mut() else {
            return Err(PlanRejection::new(PlanError::NoCollision, cells, rounds));
        };
        for (brush, door) in openable {
            collision.detach_brush(brush);
            doors.push(door);
        }
    }
    Err(PlanRejection::new(
        PlanError::GoalUnreachable,
        cells,
        config.max_rounds.max(1),
    ))
}

/// Everything a plan is built against that does not change between the
/// search's own rounds: where it started, what it is looking for, and
/// which doors it has opened so far.
#[derive(Clone, Copy)]
struct Search<'a> {
    start: Vec3,
    goals: &'a [GoalVolume],
    doors: &'a [OpenedDoor],
}

/// Rounds a distance to [`crate::reachability::DISTANCE_ROUNDING`], the
/// way that module's own report rounds one.
fn round_distance(value: f32) -> f32 {
    (value / crate::reachability::DISTANCE_ROUNDING).round()
        * crate::reachability::DISTANCE_ROUNDING
}

/// Walks `trace`'s parent links back from `target`, splits the path at
/// its door presses, straightens it and merges it into a [`RoutePlan`].
fn build_plan(
    game: &Game,
    search: &Search<'_>,
    trace: &Trace,
    target: Cell,
    aggregates: (usize, usize),
    reaches_goal: bool,
) -> Result<RoutePlan, PlanRejection> {
    let (cells, rounds) = aggregates;
    let Search {
        start,
        goals,
        doors,
    } = *search;
    let eye = Vec3::Z * game.move_config().view_height_standing;
    let path = path_to(trace, start, target);
    if path.iter().any(|point| point.kind == EdgeKind::LongJump) {
        return Err(PlanRejection::new(
            PlanError::UnsupportedEdge,
            cells,
            rounds,
        ));
    }
    let DoorMarks {
        presses,
        usable_len,
    } = attribute_doors(&path, doors, eye);
    let reaches_goal = reaches_goal && usable_len == path.len();
    let path = &path[..usable_len];
    let Some(collision) = game.collision() else {
        return Err(PlanRejection::new(PlanError::NoCollision, cells, rounds));
    };
    let start_distance = goals
        .iter()
        .map(|goal| start.distance(goal.center))
        .min_by(f32::total_cmp)
        .unwrap_or(f32::MAX);
    let goal_distance = if reaches_goal {
        0.0
    } else {
        path.last().map_or(f32::MAX, |point| {
            goals
                .iter()
                .map(|goal| point.position.distance(goal.center))
                .min_by(f32::total_cmp)
                .unwrap_or(f32::MAX)
        })
    };
    Ok(RoutePlan {
        actions: actions_for(collision, path, &presses, eye),
        cells,
        rounds,
        path_points: path.len(),
        doors: presses.len(),
        reaches_goal,
        goal_distance_rounded: round_distance(goal_distance),
        start_distance_rounded: round_distance(start_distance),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        PLAN_TURN_MAP, REACH_GAP_MAP, plan_turn_bsp, reachability_gap_bsp,
        reachability_gap_entities,
    };
    use crate::{AssetSource, MemoryAssets};

    fn turn_game() -> Game {
        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{PLAN_TURN_MAP}.bsp"),
            plan_turn_bsp("ohlplannext"),
        );
        Game::load(&assets as &dyn AssetSource, PLAN_TURN_MAP).expect("the fixture loads")
    }

    fn point(x: f32, y: f32, kind: EdgeKind) -> PathPoint {
        PathPoint {
            position: Vec3::new(x, y, 0.0),
            kind,
        }
    }

    /// The merge is the step that turns a breadth-first grid path into a
    /// script: a straight run of points sharing one heading has to become
    /// exactly one segment, and a turn has to end it.
    #[test]
    fn merge_collapses_a_straight_run_and_breaks_at_a_turn() {
        let path = [
            point(0.0, 0.0, EdgeKind::Walk),
            point(16.0, 0.0, EdgeKind::Walk),
            point(32.0, 0.0, EdgeKind::Step),
            point(48.0, 0.0, EdgeKind::Walk),
            point(48.0, 16.0, EdgeKind::Walk),
            point(48.0, 48.0, EdgeKind::Walk),
        ];
        let actions = merge_collinear(&path);
        assert_eq!(actions.len(), 2, "one run each side of the turn");
        match actions[0] {
            PlanAction::Move {
                yaw,
                distance,
                jump,
            } => {
                assert!(yaw.abs() < 1e-3, "the first run heads along +x");
                assert!((distance - 48.0).abs() < 1e-3, "three cells merged");
                assert!(!jump);
            }
            PlanAction::UseDoor { .. } => panic!("no door in this path"),
        }
        match actions[1] {
            PlanAction::Move { yaw, distance, .. } => {
                assert!((yaw - 90.0).abs() < 1e-3, "the second run heads along +y");
                assert!((distance - 48.0).abs() < 1e-3);
            }
            PlanAction::UseDoor { .. } => panic!("no door in this path"),
        }
    }

    /// A jump is a committed motion: it never merges into the run before
    /// or after it, however collinear it looks.
    #[test]
    fn merge_never_absorbs_a_jump_edge() {
        let path = [
            point(0.0, 0.0, EdgeKind::Walk),
            point(16.0, 0.0, EdgeKind::Walk),
            point(120.0, 0.0, EdgeKind::Jump),
            point(136.0, 0.0, EdgeKind::Walk),
        ];
        let actions = merge_collinear(&path);
        assert_eq!(actions.len(), 3, "walk, jump, walk");
        assert!(matches!(actions[1], PlanAction::Move { jump: true, .. }));
        assert!(matches!(actions[0], PlanAction::Move { jump: false, .. }));
        assert!(matches!(actions[2], PlanAction::Move { jump: false, .. }));
    }

    /// A point at the same place as the previous one has no heading and
    /// contributes no segment.
    #[test]
    fn merge_drops_a_point_that_goes_nowhere() {
        let path = [
            point(0.0, 0.0, EdgeKind::Walk),
            point(0.0, 0.0, EdgeKind::Walk),
            point(16.0, 0.0, EdgeKind::Walk),
        ];
        assert_eq!(merge_collinear(&path).len(), 1);
    }

    /// The end-to-end shape this module exists for: a corridor with a
    /// turn and a closed door in it plans a route that turns, presses the
    /// door, and carries on to the level-change trigger.
    #[test]
    fn plans_a_route_through_a_turn_and_a_door() {
        let plan = plan_route(&mut turn_game(), &PlanConfig::default()).expect("a route is found");
        assert_eq!(plan.doors, 1, "the corridor's one door has to be pressed");
        assert_eq!(
            plan.rounds, 2,
            "round 0 opens the door, round 1 gets through"
        );
        assert!(plan.cells > 1);
        let door_at = plan
            .actions
            .iter()
            .position(|action| matches!(action, PlanAction::UseDoor { .. }))
            .expect("the plan presses the door");
        assert!(
            door_at > 0,
            "the route walks to the door before pressing it"
        );
        assert!(
            door_at + 1 < plan.actions.len(),
            "the route carries on past the door"
        );
        if let PlanAction::UseDoor { open_seconds, .. } = plan.actions[door_at] {
            assert!(open_seconds > 0.0, "the door takes time to open");
        }
        let headings: Vec<f32> = plan
            .actions
            .iter()
            .filter_map(|action| match action {
                PlanAction::Move { yaw, .. } => Some(*yaw),
                PlanAction::UseDoor { .. } => None,
            })
            .collect();
        assert!(
            headings.iter().any(|yaw| yaw.abs() < 45.0)
                && headings.iter().any(|yaw| (*yaw - 90.0).abs() < 45.0),
            "the route both runs along the first leg and turns into the second"
        );
    }

    /// Straightening is what makes the plan a script rather than a
    /// zigzag: a corridor whose grid path is dozens of cells long has to
    /// come out as a handful of segments.
    #[test]
    fn the_planned_route_has_far_fewer_segments_than_path_points() {
        let plan = plan_route(&mut turn_game(), &PlanConfig::default()).expect("a route is found");
        assert!(
            plan.path_points > 8,
            "the fixture's grid path should be many cells long, got {}",
            plan.path_points
        );
        assert!(
            plan.segments() * 4 <= plan.path_points,
            "straightening should collapse the path: {} segment(s) from {} point(s)",
            plan.segments(),
            plan.path_points
        );
    }

    /// A goal whose own destination is a map the caller has already been
    /// in is not a goal: a route to it walks back where it came from.
    #[test]
    fn a_goal_leading_back_to_a_visited_map_is_not_a_goal() {
        let config = PlanConfig {
            avoid_goal_maps: vec!["OhlPlanNext".to_string()],
            ..PlanConfig::default()
        };
        assert_eq!(
            plan_route(&mut turn_game(), &config)
                .map(|_| ())
                .map_err(|rejection| rejection.error),
            Err(PlanError::NoGoalEntity),
            "the fixture's only changelevel names the avoided map"
        );
        assert!(
            plan_route(&mut turn_game(), &PlanConfig::default()).is_ok(),
            "and without that exclusion it is planned as before"
        );
    }

    /// A map with no goal entity fails with the fixed reason rather than
    /// planning something arbitrary.
    #[test]
    fn a_missing_goal_classname_is_a_fixed_failure() {
        let config = PlanConfig {
            goal_classname: "func_button".to_string(),
            ..PlanConfig::default()
        };
        assert_eq!(
            plan_route(&mut turn_game(), &config).map(|_| ()),
            Err(PlanRejection::new(PlanError::NoGoalEntity, 0, 0))
        );
    }

    /// A route that only exists across the long-jump edge is refused: no
    /// route file can express one, so the planner says so instead of
    /// writing a script that falls in the gap.
    #[test]
    fn a_route_needing_the_long_jump_edge_is_refused() {
        let long_jump = JumpBounds::from_long_jump_config(&ohl_physics::MoveConfig::default());
        let bytes = reachability_gap_bsp(
            long_jump.horizontal + 20.0,
            &reachability_gap_entities("ohlplannext"),
        );
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{REACH_GAP_MAP}.bsp"), bytes);
        let mut game =
            Game::load(&assets as &dyn AssetSource, REACH_GAP_MAP).expect("the fixture loads");

        let without = PlanConfig::default();
        let partial = plan_route(&mut game, &without).expect("a partial route is still planned");
        assert!(
            !partial.reaches_goal,
            "without the long-jump edge the gap is simply not crossed"
        );
        assert!(
            partial.goal_distance_rounded > 0.0,
            "a partial route ends short of the goal"
        );

        let mut game =
            Game::load(&assets as &dyn AssetSource, REACH_GAP_MAP).expect("the fixture loads");
        let with = PlanConfig {
            assume_longjump: true,
            ..PlanConfig::default()
        };
        assert_eq!(
            plan_route(&mut game, &with)
                .map(|_| ())
                .map_err(|rejection| rejection.error),
            Err(PlanError::UnsupportedEdge),
            "the walk crosses it, but no script can"
        );
    }
}
