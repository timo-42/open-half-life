//! `open-half-life`: the Rust composition-root binary.
//!
//! This is the M1-rs milestone: it parses arguments (or prompts on stdin, as
//! the C++ build did), acquires the media path exactly once into a pinned
//! [`ohl_platform::MediaSource`], classifies it with the ISO 9660 preflight
//! and then the UDF preflight, fingerprints and binds a
//! [`ohl_media::ValidatedMedia`] proof, mounts it read-only through
//! [`ohl_vfs::Mount`], and publishes or reuses a metadata-only provenance
//! cache entry. It then composes the R4.7a payload import: it locates one
//! container in the mounted tree, hands a confined parser worker a bounded
//! window over it, and stages whatever the worker enumerates into the
//! payload store. On Linux x86-64 that import is real end to end: the
//! confined worker recognises the container, enumerates it and streams its
//! entries, and this binary publishes the payload tree. See
//! `docs/MEDIA_IMPORT.md` and `docs/MILESTONES.md`.
//!
//! Every failure is logged as a single sanitized line and maps to the same
//! exit codes the C++ `src/app/main.cpp` used: `2` for a command-line usage
//! error, `1` for a media, mount, or cache failure, `0` on success.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia};
use ohl_payload::SelectionRecipe;
use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, MediaSourceBlockReader, Mount};

#[cfg(feature = "dev-tools")]
mod dev_bsp;
#[cfg(feature = "dev-tools")]
mod dev_mdl;
mod frame_profile;
mod game_run;
// The route planner's script conversion, closed-loop validation and file
// writer. Only the `--plan-route` flag that drives it is `dev-tools`; the
// module itself is compiled (and tested) unconditionally, exactly like
// `ohl_engine::route_plan`, the search it converts.
#[cfg_attr(
    not(feature = "dev-tools"),
    allow(
        dead_code,
        reason = "only --plan-route calls it, and that flag is dev-tools only"
    )
)]
mod route_planner;
mod script;
mod script_log;

/// The integration tests' synthetic fixtures, shared rather than duplicated:
/// a binary crate's unit tests cannot `use` its own `tests/` modules, so they
/// are included by path instead. Compiled only for `cargo test`.
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

const APP_NAME: &str = "Open Half-Life";
const VERSION: &str = env!("OHL_APP_VERSION");

/// Command-line usage error: `--iso` and a positional path both given, or
/// neither given and stdin had nothing to offer.
const EXIT_USAGE: u8 = 2;
/// A media, mount, or cache failure.
const EXIT_FAILURE: u8 = 1;

/// How to install the worker beside a binary built in this Cargo profile.
///
/// These are compile-fixed strings: launch diagnostics never interpolate an
/// image path, an OS error, or any media-derived data.
#[cfg(debug_assertions)]
const WORKER_IMAGE_INSTALL_HINT: &str =
    "Install the parser worker for this debug build with `cargo xtask worker-image`, then retry.";
#[cfg(not(debug_assertions))]
const WORKER_IMAGE_INSTALL_HINT: &str = "Install the parser worker for this release build with \
    `cargo run --release -p xtask -- worker-image`, then retry.";

/// The `--difficulty` choices, mapped onto `ohl-campaign`'s documented
/// `skill` cvar values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum DifficultyArg {
    /// `skill 1`.
    Easy,
    /// `skill 2`.
    Medium,
    /// `skill 3`.
    Hard,
}

impl From<DifficultyArg> for ohl_campaign::Difficulty {
    fn from(value: DifficultyArg) -> Self {
        match value {
            DifficultyArg::Easy => Self::Easy,
            DifficultyArg::Medium => Self::Medium,
            DifficultyArg::Hard => Self::Hard,
        }
    }
}

/// The largest `--overbright` multiplier accepted: a generous bound above
/// the documented 2x Quake-family overbright convention this project found
/// (see `docs/FORMAT_SOURCES.md`, "Rendering conventions"), high enough for
/// experimentation but low enough to reject an obvious typo (`--overbright
/// 800` rather than `8.0`) with a fixed, actionable error instead of
/// silently producing a blown-out capture.
const MAX_OVERBRIGHT: f32 = 8.0;

/// Parses `--overbright`: rejects anything that is not finite, not
/// strictly positive, or above [`MAX_OVERBRIGHT`], with one fixed message
/// regardless of which rule failed, so a caller sees the same actionable
/// text for `nan`, a negative value, or an oversized one.
fn parse_overbright(value: &str) -> Result<f32, String> {
    const MESSAGE: &str = "expected a finite number greater than 0 and at most 8.0";
    let parsed: f32 = value.parse().map_err(|_| MESSAGE.to_string())?;
    if parsed.is_finite() && parsed > 0.0 && parsed <= MAX_OVERBRIGHT {
        Ok(parsed)
    } else {
        Err(MESSAGE.to_string())
    }
}

/// Validates `--reachability-cell-cap`: a whole number greater than 0 and no
/// larger than [`ohl_engine::reachability::MAX_CELL_CAP`] (that constant's
/// own doc comment explains why an unbounded override would be unsafe).
#[cfg(feature = "dev-tools")]
fn parse_reachability_cell_cap(value: &str) -> Result<usize, String> {
    let message = format!(
        "expected a whole number greater than 0 and at most {}",
        ohl_engine::reachability::MAX_CELL_CAP
    );
    let parsed: usize = value.parse().map_err(|_| message.clone())?;
    if parsed > 0 && parsed <= ohl_engine::reachability::MAX_CELL_CAP {
        Ok(parsed)
    } else {
        Err(message)
    }
}

/// Validates `--reachability-round-cap`, the same way
/// [`parse_reachability_cell_cap`] validates its own flag, bounded instead
/// by [`ohl_engine::reachability::MAX_ROUND_CAP`].
#[cfg(feature = "dev-tools")]
fn parse_reachability_round_cap(value: &str) -> Result<usize, String> {
    let message = format!(
        "expected a whole number greater than 0 and at most {}",
        ohl_engine::reachability::MAX_ROUND_CAP
    );
    let parsed: usize = value.parse().map_err(|_| message.clone())?;
    if parsed > 0 && parsed <= ohl_engine::reachability::MAX_ROUND_CAP {
        Ok(parsed)
    } else {
        Err(message)
    }
}

/// Validates `--plan-attempts`: a whole number greater than 0 and no
/// larger than [`crate::route_planner::MAX_ATTEMPTS`] (that constant's own
/// doc comment explains why the closed loop is bounded).
#[cfg(feature = "dev-tools")]
fn parse_plan_attempts(value: &str) -> Result<usize, String> {
    let message = format!(
        "expected a whole number greater than 0 and at most {}",
        crate::route_planner::MAX_ATTEMPTS
    );
    let parsed: usize = value.parse().map_err(|_| message.clone())?;
    if parsed > 0 && parsed <= crate::route_planner::MAX_ATTEMPTS {
        Ok(parsed)
    } else {
        Err(message)
    }
}

/// Parses `--plan-segments`, bounded the way the planner itself bounds
/// it: at least one travelling segment per attempt, and no more than one
/// plan's worth.
#[cfg(feature = "dev-tools")]
fn parse_plan_segments(value: &str) -> Result<usize, String> {
    let message = format!(
        "expected a whole number at most {} (0 commits a whole plan)",
        crate::route_planner::MAX_SEGMENTS_PER_ATTEMPT
    );
    let parsed: usize = value.parse().map_err(|_| message.clone())?;
    if parsed <= crate::route_planner::MAX_SEGMENTS_PER_ATTEMPT {
        Ok(parsed)
    } else {
        Err(message)
    }
}

/// `Open Half-Life <version>` command line.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is an independent command-line switch, not related state a caller could \
confuse for one another"
)]
#[derive(Debug, Parser)]
#[command(name = "Open Half-Life", version = VERSION, about = None, long_about = None)]
#[command(group(clap::ArgGroup::new("scripted").args(["script", "chain_script"])))]
// Both dev-tools walks over the live collision model share the cell and
// round caps below, so those flags require *one of* them rather than the
// report specifically.
#[cfg_attr(
    feature = "dev-tools",
    command(group(
        clap::ArgGroup::new("collision_walk")
            .args(["reachability_report", "plan_route"])
            .multiple(true)
    ))
)]
struct Cli {
    /// Path to a Half-Life installation ISO.
    #[arg(long, conflicts_with = "path")]
    iso: Option<PathBuf>,

    /// Development only: load a BSP v30 map straight off disk and open a
    /// renderer window (press Escape to quit).
    ///
    /// This bypasses the media pipeline (no ISO validation, import, cache or
    /// VFS) and exists only while the renderer is being built. It is
    /// compiled in solely by the non-default `dev-tools` cargo feature and
    /// is therefore absent from release builds.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH", conflicts_with_all = ["benchmark_seconds", "profile_frames"])]
    dev_bsp: Option<PathBuf>,

    /// Development only: WAD3 texture packages consulted for the map's
    /// external textures. May be repeated. Without them, externally stored
    /// textures render as a checkerboard placeholder. Ignored when
    /// `--dev-payload` is given: its texture packages are resolved
    /// automatically instead.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH", requires = "dev_bsp")]
    dev_wad: Vec<PathBuf>,

    /// Development only: an imported payload's `files/` directory. When
    /// given together with `--dev-bsp`, the map path is resolved as a
    /// game-relative asset path (for example `maps/crossfire.bsp`) through
    /// `ohl_assets::AssetFs` instead of straight off disk, and its texture
    /// packages are resolved automatically from the map's worldspawn `wad`
    /// key. Without `--dev-payload`, `--dev-bsp` keeps its original
    /// absolute-path behaviour.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH", requires = "dev_bsp")]
    dev_payload: Option<PathBuf>,

    /// Directory for the metadata-only provenance cache.
    ///
    /// Defaults to the platform's per-user cache directory
    /// (`ohl_media::CacheLayout::user_default`).
    #[arg(long)]
    cache: Option<PathBuf>,

    /// Directory the imported payload trees are published under.
    ///
    /// Defaults to a `payload` directory in the platform's per-user data
    /// directory.
    #[arg(long)]
    payload_root: Option<PathBuf>,

    /// A runtime-only, user-local TOML selection recipe.
    ///
    /// Without one, every enumerated component is included. A recipe is
    /// never shipped with the engine and is never logged; see
    /// `docs/MEDIA_IMPORT.md`.
    #[arg(long)]
    recipe: Option<PathBuf>,

    /// The map to load, by its bare name. Without one the campaign's
    /// documented start map is used (or the hazard-course start map with
    /// `--training`).
    #[arg(long, value_name = "NAME")]
    map: Option<String>,

    /// Start on the hazard course rather than the campaign's start map.
    #[arg(long, conflicts_with = "map")]
    training: bool,

    /// Resume from a save slot instead of starting a map fresh.
    #[arg(long, value_name = "SLOT", conflicts_with_all = ["map", "training"])]
    load: Option<String>,

    /// The campaign difficulty, selecting which `skill.cfg` values the game
    /// reads.
    #[arg(long, value_name = "LEVEL", default_value = "medium")]
    difficulty: DifficultyArg,

    /// The lightmap ramp's overbright multiplier.
    ///
    /// `ohl_world::lightmap::LightRamp::default()`'s `overbright` (`1.0`,
    /// i.e. no multiplier) remains the engine's documented raw default;
    /// this application-level default is calibrated separately. A fidelity
    /// investigation (round 4, finding E5) measured this project's mean
    /// scene luma at roughly 1.7x below public reference screenshots
    /// across six clean viewpoints, and found that GoldSrc's OpenGL
    /// renderer does implement a real, documented lightmap "overbright"
    /// convention (inherited from the wider Quake engine family) that
    /// doubles brightness beyond the ordinary 0..=1 range — but that
    /// convention ships *disabled* by default in stock Half-Life
    /// (`gl_overbright 0`), so no public source pins a specific non-default
    /// multiplier as an engine fact. A follow-up fidelity investigation
    /// (round 5) measured `--overbright 1.7` bringing this project's
    /// captures to roughly 1.01x the public-reference mean luma with no
    /// added clipping, against roughly 1.72x under at `1.0`. This project
    /// therefore adopts `1.7` as the application's own calibrated display
    /// default — a project display choice, not a claimed engine fact — while
    /// `1.0` remains available for the raw, unmultiplied lightmap; see
    /// `docs/FORMAT_SOURCES.md`, "Rendering conventions". Must be a finite
    /// number greater than `0` and no more than `8.0` (see
    /// [`parse_overbright`]).
    #[arg(
        long,
        value_name = "MULTIPLIER",
        default_value_t = 1.7,
        value_parser = parse_overbright,
        long_help = "The lightmap ramp's overbright multiplier: project default \
calibrated against public reference screenshots (round 5, ~1.01x the \
public-reference mean luma with no added clipping); 1.0 = raw lightmap \
(ohl_world::lightmap::LightRamp::default()'s engine value, unmultiplied). \
See docs/FORMAT_SOURCES.md, \"Rendering conventions\". Must be a finite \
number greater than 0 and no more than 8.0."
    )]
    overbright: f32,

    /// Render offscreen and write a PNG here instead of opening a window.
    #[arg(long, value_name = "PATH")]
    headless_screenshot: Option<PathBuf>,

    /// Benchmark completed offscreen frames after a five-second warmup.
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u32).range(1..=3600),
        conflicts_with_all = ["headless_screenshot", "script", "chain_script", "follow_level_change", "profile_frames"]
    )]
    benchmark_seconds: Option<u32>,

    /// Log window frame-time percentiles and CPU stages every two seconds.
    #[arg(long, conflicts_with_all = ["headless_screenshot", "script", "chain_script"])]
    profile_frames: bool,

    /// How many frames a headless capture advances before it is written.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1,
        requires = "headless_screenshot"
    )]
    frames: u32,

    /// Stand at `x,y,z,pitch,yaw` for a headless capture instead of at the
    /// map's player start: an absolute world pose, applied once with
    /// noclip and left frozen there in world space for every frame the
    /// capture renders, regardless of what the level does around it (a
    /// mover keeps moving without the camera).
    #[arg(long, value_name = "X,Y,Z,PITCH,YAW", requires = "headless_screenshot")]
    viewpoint: Option<game_run::Viewpoint>,

    /// Stand `dx,dy,dz,dpitch,dyaw` away from the map's player start for a
    /// headless capture. Ignored when `--viewpoint` is given. Unlike
    /// `--viewpoint` this never enables noclip: the player spawns and
    /// moves normally (falling, colliding, riding a mover), and the
    /// offset rides along with them — every rendered frame is the
    /// player's *current* eye position plus this fixed offset, not a
    /// pose frozen at tick 0.
    #[arg(
        long,
        value_name = "DX,DY,DZ,DPITCH,DYAW",
        requires = "headless_screenshot"
    )]
    spawn_offset: Option<game_run::Viewpoint>,

    /// Open the playable window over an already-imported payload.
    #[arg(long)]
    play: bool,

    /// Path to a Half-Life installation ISO (positional form).
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Runs deterministic scripted input instead of the interactive window
    /// or the frame-count headless capture loop. Usable with or without
    /// `--headless-screenshot`: without one, the scripted ticks still run
    /// (headlessly, with no GPU needed), just with no PNG written at the
    /// end. See `crate::script` for the grammar.
    #[arg(long, value_name = "PATH")]
    script: Option<PathBuf>,

    /// Runs a *chain* of deterministic scripted-input routes across level
    /// changes in one process: give the flag once per route, in chain
    /// order. The first route runs from the start map's own player start;
    /// every later one runs from wherever the preceding route's level
    /// change put the player down in the destination map, with the health,
    /// armor, weapons and ammo `ohl_engine::transition` carries across —
    /// the campaign state a cold `--map <name>` load of a mid-campaign map
    /// cannot reproduce. Level changes are always followed here, so
    /// `--follow-level-change` is neither needed nor consulted. A route
    /// ends at the first level change it reaches; when a route's ticks run
    /// out first the chain stops there. See `game_run::run_chained` for
    /// the fixed report lines, and `cargo xtask chain-walk` for the
    /// harness that assembles a chain and reports how deep it got.
    ///
    /// The routes are ordinal rather than keyed by destination map name on
    /// purpose: which map a level change lands in is a fact about the
    /// user's own payload, and only `ohl_campaign`'s own publicly sourced
    /// table of names may be written down in this repository (see
    /// `docs/CLEAN_ROOM.md` rule 7).
    ///
    /// A chain walk is headless and writes no PNG, so it takes no capture
    /// pose: `--headless-screenshot`, `--viewpoint` and `--spawn-offset`
    /// are rejected outright rather than silently ignored. A frozen or
    /// rider pose is defined against one map's geometry, and a chain
    /// deliberately leaves that map partway through.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["script", "headless_screenshot", "viewpoint", "spawn_offset"]
    )]
    chain_script: Vec<PathBuf>,

    /// Enables the scripted-input milestone log lines documented in
    /// `docs/m79-design.md` §7. Ignored without `--script`/`--chain-script`.
    #[arg(long, requires = "scripted")]
    script_log: bool,

    /// Follows a `trigger_changelevel` during a headless (`--headless-
    /// screenshot`) or scripted (`--script`) run, calling the same
    /// [`ohl_engine::Game::change_level`] path the interactive window
    /// uses instead of staying on the original map. Without this flag the
    /// run logs a fixed "not followed" line and keeps rendering the map it
    /// started on, which remains the default so an ordinary capture never
    /// silently jumps to a different map.
    #[arg(long)]
    follow_level_change: bool,

    /// Development only: places the headless capture eye `DISTANCE` units
    /// from the nearest spawned monster, at the monster's eye height,
    /// facing it, in noclip, instead of at the map's player start or a
    /// caller-chosen `--viewpoint`/`--spawn-offset`. Never logs a position
    /// or classname (see `ohl_engine::Game::nearest_monster_position`).
    /// Combines with `--script`: the placement is applied once, right
    /// after the map loads, before the scripted input runs — so a motion
    /// capture can start already facing the nearest monster. Like
    /// `--dev-mdl` this is compiled in solely by the non-default
    /// `dev-tools` cargo feature.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "DISTANCE", requires = "headless_screenshot")]
    viewpoint_at_nearest_monster: Option<f32>,

    /// Development only: load a studio model (MDL v10) straight off disk and
    /// open a renderer window showing it animating (`[` and `]` cycle the
    /// sequence, Escape quits).
    ///
    /// Combined with `--dev-bsp` the map is loaded too and the model is
    /// placed at its player start; on its own the model simply orbits in
    /// front of the camera. Like `--dev-bsp` this bypasses the media
    /// pipeline and is compiled in solely by the non-default `dev-tools`
    /// cargo feature.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH", conflicts_with_all = ["benchmark_seconds", "profile_frames"])]
    dev_mdl: Option<PathBuf>,

    /// Development only: runs a bounded, deterministic breadth-first
    /// reachability walk over the map's live collision model, from the
    /// player start, and reports (as fixed lines, one per round): how many
    /// 16-unit grid cells were reached, which brush-entity classnames sit
    /// on the unreached frontier (a count of distinct entities and
    /// whether the engine's own use-proximity path could open one from a
    /// reached cell), and whether a `trigger_changelevel` was reached (and
    /// its straight-line distance from spawn, rounded to the nearest ten
    /// units). Closed doors the walk found and could open are then
    /// simulated open for up to six rounds by default (see
    /// `--reachability-round-cap`), so a route needing several doors opened
    /// in sequence is triaged one round at a time. See
    /// `ohl_engine::reachability`.
    ///
    /// Loads through the normal `--map`/payload path exactly like
    /// `--script`, headlessly: no window opens and no GPU is used. Prints
    /// only classnames (this project's own documented entity vocabulary),
    /// aggregate counts and rounded distances — never a map name,
    /// coordinate, or targetname (`docs/CLEAN_ROOM.md`). Compiled in
    /// solely by the non-default `dev-tools` cargo feature.
    ///
    /// Combined with `--chain-script` the walk runs *after* the chain,
    /// from wherever its last route left the player standing, rather than
    /// from a player start: that is the arrival point of a level change,
    /// which is exactly the state a cold `--map <name>` load cannot
    /// reproduce and where a chain route has to be authored from.
    #[cfg(feature = "dev-tools")]
    #[arg(long, conflicts_with_all = ["benchmark_seconds", "profile_frames"])]
    reachability_report: bool,

    /// Development only: with `--reachability-report`, treats a
    /// `func_breakable` on the walk's frontier (`health > 0`, not the
    /// documented "Only Trigger" flag, and not already broken) as
    /// openable-by-damage between rounds, the same way a closed,
    /// use-openable door is opened — tagged in the report as
    /// "damage-openable" and counted separately from doors.
    ///
    /// This assumes a weapon capable of dealing damage is available; it
    /// never checks or grants an actual inventory (a cold map load starts
    /// with none — see `--start-inventory` for that). Without this flag a
    /// breakable stays on the frontier forever, which is this project's
    /// own default: a fresh, unarmed spawn cannot break anything.
    #[cfg(feature = "dev-tools")]
    #[arg(long, requires = "reachability_report")]
    reachability_assume_armed: bool,

    /// Development only: with `--reachability-report`, adds a third,
    /// longer-reaching edge attempt (tried only when both the plain step
    /// and the ordinary running-jump edge fail): a long jump
    /// (`item_longjump`), bounded by the same live
    /// `ohl_physics::MoveConfig::long_jump_forward_speed`/
    /// `long_jump_up_speed`/`gravity` this build's engine already uses for
    /// the real long-jump impulse, never a restated literal. A cell
    /// reached only this way is counted separately in the printed report.
    ///
    /// This assumes the long jump module is owned; it never checks or
    /// grants actual ownership (a cold map load owns none — see
    /// `--start-inventory` for actually giving weapons/ammo, though the
    /// long jump module itself is not a `weapon_*`/`ammo_*` pickup this
    /// flag can grant). Without this flag the walk's jump edge stays
    /// bounded by the ordinary running jump only, which this project's
    /// own default: a fresh spawn does not own the long jump module.
    #[cfg(feature = "dev-tools")]
    #[arg(long, requires = "reachability_report")]
    reachability_assume_longjump: bool,

    /// Development only: with `--reachability-report`, treats a
    /// `func_pendulum` on the walk's frontier as passable between rounds —
    /// tagged in the report as "pendulum-openable" on the round it is
    /// found, and the following round marked "(pendulum wait)" — the same
    /// round-advance shape a closed door or a breakable gets.
    ///
    /// This walk has no notion of a swing's timing: a `func_pendulum` is
    /// either permanently blocking (without this flag) or permanently
    /// passable for a whole round (with it), never "blocking except during
    /// a clear moment." Setting this flag is a caller-supplied assumption
    /// ("assume the player can time the swing and walk through during a
    /// gap"), not a claim that the corridor is actually open — see
    /// `ohl_engine::reachability::ReachabilityConfig::assume_pendulum_wait`.
    /// Without this flag a `func_pendulum` stays on the frontier forever,
    /// this project's own long-standing default (a follow-up
    /// investigation's `c1a2` finding, recorded in local notes and not
    /// part of the repository).
    #[cfg(feature = "dev-tools")]
    #[arg(long, requires = "reachability_report")]
    reachability_assume_pendulum_wait: bool,

    /// Development only: with `--reachability-report`, overrides the
    /// walk's own per-round cell cap (40,000 by default —
    /// `ohl_engine::reachability::ReachabilityConfig::default`). A map
    /// whose own reachable area is larger than the default hits that cap
    /// before a single round-advance edge (door/breakable/pushable/
    /// pendulum) runs at all, hiding whatever those edges would otherwise
    /// reveal (that same investigation's `c4a2` finding, the gap this
    /// flag closes). Bounded by
    /// `ohl_engine::reachability::MAX_CELL_CAP` — a hard sanity maximum,
    /// not a per-map tuned value — so even the most permissive override
    /// keeps the walk's work bounded.
    #[cfg(feature = "dev-tools")]
    #[arg(
        long,
        requires = "collision_walk",
        value_name = "N",
        value_parser = parse_reachability_cell_cap
    )]
    reachability_cell_cap: Option<usize>,

    /// Development only: with `--reachability-report`, overrides the
    /// walk's own door-opening round cap (6 by default —
    /// `ohl_engine::reachability::ReachabilityConfig::default`), the
    /// analogous override to `--reachability-cell-cap` for the number of
    /// rounds run rather than the cells visited per round. Bounded by
    /// `ohl_engine::reachability::MAX_ROUND_CAP`.
    #[cfg(feature = "dev-tools")]
    #[arg(
        long,
        requires = "collision_walk",
        value_name = "N",
        value_parser = parse_reachability_round_cap
    )]
    reachability_round_cap: Option<usize>,

    /// Development only: gives the player named weapons and ammo right
    /// after the map loads, so a single-map probe or scenario can model
    /// the inventory a real campaign run would have carried in from an
    /// earlier map via `changelevel`, instead of always starting from the
    /// empty inventory a cold load otherwise gets
    /// (`ohl_combat::Inventory::new` grants nothing, not even the
    /// crowbar).
    ///
    /// A comma-separated list of `weapon_*`/`ammo_*` classnames from this
    /// project's own documented pickup vocabulary
    /// (`ohl_combat::classify_classname`, `docs/FORMAT_SOURCES.md`,
    /// "Pickups and chargers") — the exact same classnames a `weapon_*`/
    /// `ammo_*` pickup entity already uses, applied through the same
    /// grant path a touch pickup uses (`ohl_engine::Game::
    /// give_start_inventory`). A weapon entry grants its bundled ammo the
    /// same way picking it up would; repeating a classname stacks it (for
    /// example `ammo_buckshot,ammo_buckshot` grants two boxes' worth). An
    /// unrecognised classname, or one that names something other than a
    /// weapon or ammo (`item_suit`, `func_healthcharger`, ...), is a
    /// usage error. This never changes the save format: inventory is save
    /// tag 23, and giving items at load uses the normal runtime inventory
    /// API, not a new one.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "LIST")]
    start_inventory: Option<String>,

    /// Development only: plans a route from the player's current position
    /// to the nearest reachable `trigger_changelevel` (or a
    /// `--plan-goal` classname), converts it into a scripted-input route
    /// file, validates it by replaying it in-process until the level
    /// change actually fires, and writes it to PATH.
    ///
    /// The search is the same bounded, deterministic walk over the live
    /// collision model `--reachability-report` triages with
    /// (`ohl_engine::route_plan`), but it records how the walk got
    /// somewhere rather than only that it could: the cell path is
    /// straightened, merged into runs, and turned into `look`/`forward`
    /// lines, with a `use` press and the door's own open time wherever a
    /// closed door has to be opened. Nothing is written unless a replay
    /// from the very state the plan started at reached the level change;
    /// when a replay drifts, the planner re-plans from the drift point
    /// and appends the continuation (up to `--plan-attempts` times).
    ///
    /// Combined with `--chain-script` the plan starts from wherever the
    /// chain's last route left the player standing — the arrival point a
    /// cold `--map <name>` load cannot reproduce, and the only place the
    /// next chain-walk route can honestly be authored from.
    ///
    /// Prints aggregates only (cells, segments, replay attempts,
    /// seconds): never a map name, a coordinate or a targetname
    /// (`docs/CLEAN_ROOM.md`), and the written file holds script commands
    /// and project-authored comment words only. Compiled in solely by the
    /// non-default `dev-tools` cargo feature.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH", conflicts_with_all = ["benchmark_seconds", "profile_frames", "script", "headless_screenshot"])]
    plan_route: Option<PathBuf>,

    /// Development only: with `--plan-route`, plans a route to the
    /// nearest reachable brush entity of this classname instead of to a
    /// `trigger_changelevel`. Only an entity with a brush volume can be a
    /// goal.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "CLASSNAME", requires = "plan_route")]
    plan_goal: Option<String>,

    /// Development only: with `--plan-route`, how many plan/replay
    /// attempts the closed loop may take before giving up. Each attempt
    /// re-plans from wherever the previous attempt's replay drifted to.
    #[cfg(feature = "dev-tools")]
    #[arg(
        long,
        requires = "plan_route",
        value_name = "N",
        value_parser = parse_plan_attempts
    )]
    plan_attempts: Option<usize>,

    /// Development only: with `--plan-route`, how many of one plan's own
    /// travelling segments (a walk-forward run or a ladder climb) each
    /// attempt commits to the script before it replays and plans again.
    ///
    /// One — the default — is the tightest closed loop there is: every
    /// segment is walked and the next is planned from wherever the
    /// player actually ended up. A larger number spends fewer replays on
    /// a long route, at the cost of letting a segment's own drift carry
    /// into the segments planned after it. Zero commits a whole plan at
    /// a time, which is the right trade on a long route whose middle
    /// runs through places the search sees less of than its start does.
    #[cfg(feature = "dev-tools")]
    #[arg(
        long,
        requires = "plan_route",
        value_name = "N",
        value_parser = parse_plan_segments
    )]
    plan_segments: Option<usize>,
}

/// Formats an event as `[level] message`, mirroring the C++ `ohl::core::log`
/// style (`src/core/src/log.cpp`) so the two builds are easy to compare.
struct CompactEventFormat;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for CompactEventFormat
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = match *event.metadata().level() {
            tracing::Level::TRACE => "trace",
            tracing::Level::DEBUG => "debug",
            tracing::Level::INFO => "info",
            tracing::Level::WARN => "warning",
            tracing::Level::ERROR => "error",
        };
        write!(writer, "[{level}] ")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .event_format(CompactEventFormat)
        .init();
}

/// Returns a platform line such as `Platform: Linux x86_64`, mirroring
/// `ohl::platform::to_string` in the C++ build.
fn platform_line() -> String {
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("Platform: {os} {arch}")
}

/// Prompts on stdin for an ISO path, as the C++ build did when no path was
/// supplied on the command line.
///
/// Returns `None` when stdin is closed or the line is empty, matching the
/// C++ `prompt_for_iso`'s `std::nullopt` result.
fn prompt_for_iso() -> Option<PathBuf> {
    print!("Path to a legally obtained Half-Life ISO: ");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    let bytes_read = std::io::stdin().read_line(&mut line).ok()?;
    if bytes_read == 0 {
        return None;
    }
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Logs a sanitized "Media preflight failed" line, mirroring the C++
/// application's fixed-prefix error reporting.
fn log_preflight_failure(message: impl std::fmt::Display) {
    tracing::error!("Media preflight failed: {message}");
}

/// The one-line mapping from a preflight crate's result to the
/// `ohl-media` description, plus the `ohl-vfs` class needed to mount without
/// re-running the preflight a third time.
struct Classification {
    vfs_class: ohl_vfs::MediaClass,
    description: MediaDescription,
}

/// Runs the ISO 9660 preflight, then the UDF preflight, over `probe`,
/// exactly as `ohl_vfs::Mount::open` does internally; this copy lets the
/// application map the result onto `ohl_media::MediaDescription` before
/// mounting via `Mount::open_as`, which skips a third redundant probe.
fn classify(
    probe: &mut MediaSourceBlockReader,
) -> Result<Classification, ohl_core::SanitizedError> {
    match ohl_iso9660::preflight(probe) {
        Ok(preflight) => {
            return Ok(map_preflight(
                ohl_vfs::MediaClass::Iso9660,
                &preflight.media,
            ));
        }
        Err(ohl_core::SanitizedError::Unsupported) => {}
        Err(error) => return Err(error),
    }

    let preflight = ohl_udf::preflight(probe)?;
    Ok(map_preflight(ohl_vfs::MediaClass::Udf, &preflight))
}

fn map_preflight(
    vfs_class: ohl_vfs::MediaClass,
    preflight: &ohl_media_archive::MediaPreflight,
) -> Classification {
    let class = match preflight.media_class {
        ohl_media_archive::MediaClass::Udf => MediaClass::Udf,
        ohl_media_archive::MediaClass::Iso9660 => MediaClass::Iso9660,
    };
    let description = MediaDescription::new(
        class,
        preflight.filesystem.as_str(),
        ohl_media::VolumeLabel::sanitized(preflight.volume_label.as_str()),
    );
    Classification {
        vfs_class,
        description,
    }
}

/// Runs the `--dev-bsp` development map viewer, resolving the map (and,
/// when `--dev-payload` was given, its texture packages) either straight
/// off disk or through `ohl_assets::AssetFs`. Neither path is ever logged:
/// the project's logging policy is uniform, and a user-supplied path is
/// still untrusted input.
#[cfg(feature = "dev-tools")]
fn run_dev_bsp(cli: &Cli) -> ExitCode {
    tracing::warn!("development map viewer: media pipeline is bypassed");
    let path = cli.dev_bsp.as_deref().expect("checked by the caller");
    let outcome = if let Some(files_dir) = cli.dev_payload.as_deref() {
        match path.to_str() {
            Some(relative) => dev_bsp::run_payload(files_dir, relative),
            None => Err("the map path must be valid UTF-8 when used with --dev-payload"),
        }
    } else {
        dev_bsp::run(path, &cli.dev_wad)
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            std::process::ExitCode::from(3)
        }
    }
}

/// Opens the pinned source, classifies it, validates it, and mounts its
/// read-only root.
///
/// Every failure has already been logged through `log_preflight_failure` when
/// this returns `None`; the caller only has to choose the exit status.
fn preflight(iso_path: PathBuf) -> Option<(ValidatedMedia, Mount)> {
    // The path is acquired exactly once into the pinned capability, then
    // discarded: nothing past this point ever sees it again.
    let source = match MediaSource::open(&iso_path) {
        Ok(source) => Arc::new(source),
        Err(error) => {
            log_preflight_failure(error);
            return None;
        }
    };
    drop(iso_path);

    let mut probe = match MediaSourceBlockReader::new(Arc::clone(&source)) {
        Ok(probe) => probe,
        Err(error) => {
            log_preflight_failure(error);
            return None;
        }
    };

    let classification = match classify(&mut probe) {
        Ok(classification) => classification,
        Err(error) => {
            log_preflight_failure(error);
            return None;
        }
    };
    drop(probe);

    let validated =
        match ValidatedMedia::fingerprinting(Arc::clone(&source), classification.description) {
            Ok(validated) => validated,
            Err(error) => {
                log_preflight_failure(error);
                return None;
            }
        };

    let mount = match Mount::open_as(
        classification.vfs_class,
        Arc::clone(&source),
        DirectoryLimits::default(),
    ) {
        Ok(mount) => mount,
        Err(error) => {
            log_preflight_failure(error);
            return None;
        }
    };

    if let Err(error) = mount.list_page("/") {
        log_preflight_failure(error);
        return None;
    }

    tracing::info!("Mounted read-only media image.");

    Some((validated, mount))
}

fn run(cli: Cli) -> ExitCode {
    tracing::info!("{APP_NAME} {VERSION}");
    tracing::info!("{}", platform_line());
    tracing::debug!(core_version = ohl_core::VERSION, "loaded ohl-core");

    #[cfg(feature = "dev-tools")]
    if let Some(code) = run_dev_tools(&cli) {
        return code;
    }

    #[cfg(feature = "dev-tools")]
    let reachability_report = cli.reachability_report;
    #[cfg(not(feature = "dev-tools"))]
    let reachability_report = false;

    #[cfg(feature = "dev-tools")]
    let plan_route = cli.plan_route.is_some();
    #[cfg(not(feature = "dev-tools"))]
    let plan_route = false;

    if cli.play
        || cli.training
        || cli.map.is_some()
        || cli.load.is_some()
        || cli.headless_screenshot.is_some()
        || cli.benchmark_seconds.is_some()
        || cli.profile_frames
        || cli.script.is_some()
        || !cli.chain_script.is_empty()
        || reachability_report
        || plan_route
    {
        return run_game_flow(&cli);
    }

    run_media_flow(cli)
}

/// Resolves the payload store root the game reads from, and the map to
/// start on.
///
/// Neither is logged: the payload root is a user-supplied path and the map
/// name, though it comes from `ohl-campaign`'s own sourced table, names
/// game content.
fn run_game_flow(cli: &Cli) -> ExitCode {
    let Ok(root) = payload_root(cli.payload_root.clone()) else {
        tracing::error!("Payload location failed: no per-user data directory is available");
        return ExitCode::from(EXIT_FAILURE);
    };

    let files = match locate_payload_files(cli, &root) {
        Ok(files) => files,
        Err(code) => return code,
    };

    let map = cli.map.clone().unwrap_or_else(|| {
        if cli.training {
            ohl_campaign::TRAINMAP.to_string()
        } else {
            ohl_campaign::STARTMAP.to_string()
        }
    });

    match game_run::run(&game_run::GameArgs {
        payload_files: &files,
        map: &map,
        load_slot: cli.load.as_deref(),
        difficulty: cli.difficulty.into(),
        screenshot: cli.headless_screenshot.as_deref(),
        benchmark_seconds: cli.benchmark_seconds,
        profile_frames: cli.profile_frames,
        frames: cli.frames,
        viewpoint: cli.viewpoint,
        spawn_offset: cli.spawn_offset,
        script: cli.script.as_deref(),
        chain_script: &cli.chain_script,
        script_log: cli.script_log,
        overbright: cli.overbright,
        follow_level_change: cli.follow_level_change,
        #[cfg(feature = "dev-tools")]
        viewpoint_at_nearest_monster: cli.viewpoint_at_nearest_monster,
        #[cfg(feature = "dev-tools")]
        reachability_report: cli.reachability_report,
        #[cfg(feature = "dev-tools")]
        reachability_assume_armed: cli.reachability_assume_armed,
        #[cfg(feature = "dev-tools")]
        reachability_assume_longjump: cli.reachability_assume_longjump,
        #[cfg(feature = "dev-tools")]
        reachability_assume_pendulum_wait: cli.reachability_assume_pendulum_wait,
        #[cfg(feature = "dev-tools")]
        reachability_cell_cap: cli.reachability_cell_cap,
        #[cfg(feature = "dev-tools")]
        reachability_round_cap: cli.reachability_round_cap,
        #[cfg(feature = "dev-tools")]
        start_inventory: cli.start_inventory.as_deref(),
        #[cfg(feature = "dev-tools")]
        plan_route: cli.plan_route.as_deref(),
        #[cfg(feature = "dev-tools")]
        plan_goal: cli.plan_goal.as_deref(),
        #[cfg(feature = "dev-tools")]
        plan_attempts: cli.plan_attempts,
        #[cfg(feature = "dev-tools")]
        plan_segments: cli.plan_segments,
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            tracing::error!("{message}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Finds the published payload's `files/` directory: through the medium's
/// own provenance entry when an ISO was given (importing first if it has
/// not been imported yet), and otherwise by resolving the single published
/// tree under the payload root.
fn locate_payload_files(cli: &Cli, root: &Path) -> Result<PathBuf, ExitCode> {
    if let Some(iso_path) = cli.iso.clone().or_else(|| cli.path.clone()) {
        let Some((validated, mount)) = preflight(iso_path) else {
            return Err(ExitCode::from(EXIT_FAILURE));
        };
        let layout = cache_layout(cli.cache.clone())?;
        if let Some(tree) = ohl_import::find_published_payload(&layout, &validated, root) {
            return Ok(tree.files_directory().to_path_buf());
        }
        match ohl_media::prepare_import_cache(&validated, &layout) {
            Ok(report) => report.log(),
            Err(error) => {
                tracing::error!("Media cache preparation failed: {error}");
                return Err(ExitCode::from(EXIT_FAILURE));
            }
        }
        let code = import_payload(
            &validated,
            &mount,
            &layout,
            cli.recipe.as_deref(),
            Some(root.to_path_buf()),
        );
        if code != ExitCode::SUCCESS {
            return Err(code);
        }
        return ohl_import::find_published_payload(&layout, &validated, root)
            .map(|tree| tree.files_directory().to_path_buf())
            .ok_or_else(|| {
                tracing::error!("No payload is published for this medium.");
                ExitCode::from(EXIT_FAILURE)
            });
    }

    if let Some(files) = sole_published_tree(root) {
        return Ok(files);
    }
    tracing::error!("No imported payload was found. Import one first by passing --iso PATH.");
    Err(ExitCode::from(EXIT_FAILURE))
}

/// The one published tree under `root`, when there is exactly one.
///
/// A published payload is a directory holding a `files` directory (see
/// `ohl_payload::published_files_directory`); with no medium to identify
/// which one this run belongs to, an unambiguous single tree is the only
/// safe answer.
fn sole_published_tree(root: &Path) -> Option<PathBuf> {
    let mut found = None;
    for entry in std::fs::read_dir(root).ok()? {
        let files = entry.ok()?.path().join("files");
        if !std::fs::symlink_metadata(&files).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(files);
    }
    found
}

/// The metadata-only provenance cache layout, defaulting per user.
fn cache_layout(cache_root: Option<PathBuf>) -> Result<CacheLayout, ExitCode> {
    let layout = match cache_root {
        Some(root) => CacheLayout::with_root(root),
        None => CacheLayout::user_default(),
    };
    layout.map_err(|error| {
        tracing::error!("Media cache preparation failed: {error}");
        ExitCode::from(EXIT_FAILURE)
    })
}

/// The development-only viewers, which bypass the media pipeline entirely.
///
/// Returns `Some(exit_code)` when one of them handled the invocation, and
/// `None` when the normal media flow should run. Compiled in solely by the
/// non-default `dev-tools` feature, so a release build has no such arm.
#[cfg(feature = "dev-tools")]
fn run_dev_tools(cli: &Cli) -> Option<ExitCode> {
    // Neither path is ever logged: the project's logging policy is uniform,
    // and a user-supplied path is still untrusted input.
    if let Some(path) = cli.dev_mdl.as_deref() {
        tracing::warn!("development model viewer: media pipeline is bypassed");
        return Some(
            match dev_mdl::run(path, cli.dev_bsp.as_deref(), &cli.dev_wad) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::from(3)
                }
            },
        );
    }

    if cli.dev_bsp.is_some() {
        return Some(run_dev_bsp(cli));
    }

    None
}

/// Acquires the user's medium exactly once, validates it, and mounts it
/// read-only, then hands the proof and the mount to [`run_import_flow`].
fn run_media_flow(cli: Cli) -> ExitCode {
    let Some(iso_path) = cli.iso.or(cli.path).or_else(prompt_for_iso) else {
        tracing::error!("No ISO path provided. Use --iso PATH.");
        return ExitCode::from(EXIT_USAGE);
    };

    let Some((validated, mount)) = preflight(iso_path) else {
        return ExitCode::from(EXIT_FAILURE);
    };

    run_import_flow(
        &validated,
        &mount,
        cli.cache,
        cli.recipe.as_deref(),
        cli.payload_root,
    )
}

/// Publishes or reuses the metadata-only provenance entry, then runs the
/// payload import against it.
fn run_import_flow(
    validated: &ValidatedMedia,
    mount: &Mount,
    cache_root: Option<PathBuf>,
    recipe_path: Option<&Path>,
    payload_root: Option<PathBuf>,
) -> ExitCode {
    let layout = match cache_layout(cache_root) {
        Ok(layout) => layout,
        Err(code) => return code,
    };

    match ohl_media::prepare_import_cache(validated, &layout) {
        Ok(report) => report.log(),
        Err(error) => {
            tracing::error!("Media cache preparation failed: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    }

    import_payload(validated, mount, &layout, recipe_path, payload_root)
}

/// The fixed line for a medium whose container the worker cannot decode.
///
/// The worker recognises Wise overlays, Microsoft cabinets and InstallShield
/// 3 Z archives; anything else is refused, as is a container whose bytes do
/// not decode. The line names no format, because naming one would leak which
/// container the medium carries.
const UNSUPPORTED_LINE: &str = "Payload import is not supported for this medium's container format; no media executable was run.";

/// Coarse import progress, logged as fixed strings at the quarter marks.
///
/// The sink receives a fraction of the planned byte total and nothing else —
/// no name, no path, no count — so the lines it writes are fixed strings by
/// construction.
#[derive(Debug, Default)]
struct QuarterProgress {
    reported: u8,
}

impl ohl_import::ProgressSink for QuarterProgress {
    fn report(&mut self, fraction: f32) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the fraction is clamped to 0..=1 by the pipeline"
        )]
        let quarters = (fraction.clamp(0.0, 1.0) * 4.0) as u8;
        while self.reported < quarters.min(4) {
            self.reported += 1;
            match self.reported {
                1 => tracing::info!("Payload import 25% complete."),
                2 => tracing::info!("Payload import 50% complete."),
                3 => tracing::info!("Payload import 75% complete."),
                _ => tracing::info!("Payload import 100% complete."),
            }
        }
    }
}

/// The include-everything recipe used when the user supplies none.
const DEFAULT_RECIPE: &str = "version = 1\ndefault_decision = \"include\"\n";

/// Resolves the payload store root, defaulting under the per-user data
/// directory.
fn payload_root(explicit: Option<PathBuf>) -> Result<PathBuf, &'static str> {
    if let Some(root) = explicit {
        return Ok(root);
    }
    directories::ProjectDirs::from("", "", "open-half-life")
        .map(|dirs| dirs.data_dir().join("payload"))
        .ok_or("no per-user data directory is available")
}

/// Loads the recipe, or the include-everything default.
fn load_recipe(path: Option<&Path>) -> Result<SelectionRecipe, ohl_payload::SelectionRecipeError> {
    match path {
        Some(path) => SelectionRecipe::read_from_file(path),
        // Not a shipped recipe: the neutral policy that selects whatever the
        // worker enumerated, which a user recipe then narrows.
        None => SelectionRecipe::parse(DEFAULT_RECIPE),
    }
}

/// Runs the payload import against a freshly launched confined worker.
fn import_payload(
    validated: &ValidatedMedia,
    mount: &Mount,
    layout: &CacheLayout,
    recipe_path: Option<&Path>,
    explicit_payload_root: Option<PathBuf>,
) -> ExitCode {
    if ohl_import::pipeline::recorded_payload_identity(layout, validated).is_some() {
        tracing::info!("Payload already imported.");
        return ExitCode::SUCCESS;
    }

    let root = match payload_root(explicit_payload_root) {
        Ok(root) => root,
        Err(message) => {
            tracing::error!("Payload import failed: {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    // The recipe's *contents* are never logged, only that one was rejected.
    let recipe = match load_recipe(recipe_path) {
        Ok(recipe) => recipe,
        Err(error) => {
            tracing::error!("Payload import failed: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let transport = ohl_import::CancellationToken::default();
    let staging = ohl_payload::CancellationToken::default();
    let cancellation = ohl_import::ImportCancellation {
        transport: &transport,
        staging: &staging,
    };
    let mut progress = QuarterProgress::default();
    let outcome = ohl_import::run_import(
        validated,
        mount,
        &recipe,
        &root,
        layout,
        cancellation,
        &mut progress,
    );
    report_import(outcome)
}

/// The same composition against a caller-supplied worker.
///
/// This exists only for the `#[cfg(test)]` seam below: it is compiled out of
/// every non-test build, so no release binary can be pointed at anything but
/// the confined worker `run_import` launches. `WorkerProcess` is sealed by
/// `ohl-import`, so even the test can only use that crate's own doubles.
#[cfg(test)]
fn import_payload_with_worker<W: ohl_import::WorkerProcess>(
    validated: &ValidatedMedia,
    mount: &Mount,
    layout: &CacheLayout,
    payload_root: &Path,
    worker: W,
) -> ExitCode {
    let recipe = load_recipe(None).expect("the built-in default recipe parses");
    let transport = ohl_import::CancellationToken::default();
    let staging = ohl_payload::CancellationToken::default();
    let allocation = ohl_import::SessionIdAllocator::new()
        .allocate()
        .expect("a fresh session identity");
    report_import(ohl_import::run_import_with_worker(
        validated,
        mount,
        &recipe,
        payload_root,
        layout,
        &ohl_import::ImportConfig::default(),
        worker,
        allocation,
        ohl_import::ImportCancellation {
            transport: &transport,
            staging: &staging,
        },
        &mut ohl_import::DiscardProgress,
    ))
}

/// Maps one import outcome onto a fixed log line and an exit code.
///
/// The report's counts are media-derived and are deliberately not logged.
fn report_import(outcome: Result<ohl_import::ImportReport, ohl_import::ImportError>) -> ExitCode {
    match outcome {
        Ok(report) => {
            if report.outcome == ohl_import::ImportOutcome::Published {
                tracing::info!("Payload imported.");
            } else {
                tracing::info!("Payload already imported.");
            }
            tracing::info!("Payload import complete.");
            ExitCode::SUCCESS
        }
        // The worker decoded nothing it recognises in this medium. That is
        // a property of the medium, not a user error, so it is not a failure
        // exit.
        Err(ohl_import::ImportError::Unsupported) => {
            tracing::info!("{UNSUPPORTED_LINE}");
            ExitCode::SUCCESS
        }
        // Media this build recognises no container in is equally not a user
        // error: nothing was attempted and nothing was written.
        Err(ohl_import::ImportError::NoContainer) => {
            tracing::info!(
                "No supported payload container was found in the media; nothing was imported."
            );
            ExitCode::SUCCESS
        }
        Err(
            error @ ohl_import::ImportError::WorkerUnavailable(
                ohl_platform::IsolatedWorkerError::ServiceUnavailable,
            ),
        ) => {
            tracing::error!("Payload import failed: {error}. {WORKER_IMAGE_INSTALL_HINT}");
            ExitCode::from(EXIT_FAILURE)
        }
        Err(error) => {
            tracing::error!("Payload import failed: {error}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    run(cli)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::ExitCode;
    use std::sync::Arc;

    use ohl_import::testing::{FakeWorker, SyntheticTransport};
    use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia, VolumeLabel};
    use ohl_platform::MediaSource;
    use ohl_vfs::{DirectoryLimits, Mount};

    use clap::Parser as _;

    use super::{Cli, DEFAULT_RECIPE, load_recipe, platform_line, report_import};

    #[test]
    fn benchmark_accepts_bounded_duration_without_a_screenshot() {
        let cli =
            Cli::try_parse_from(["open-half-life", "--training", "--benchmark-seconds", "30"])
                .expect("benchmark needs no screenshot path");
        assert_eq!(cli.benchmark_seconds, Some(30));
        for invalid in ["0", "3601", "-1"] {
            assert!(
                Cli::try_parse_from(["open-half-life", "--benchmark-seconds", invalid]).is_err()
            );
        }
        assert!(
            Cli::try_parse_from([
                "open-half-life",
                "--benchmark-seconds",
                "30",
                "--profile-frames"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "open-half-life",
                "--benchmark-seconds",
                "30",
                "--headless-screenshot",
                "capture.png"
            ])
            .is_err()
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn development_modes_cannot_silently_override_frame_profiling() {
        for mode in ["--dev-bsp", "--dev-mdl", "--reachability-report"] {
            let mut args = vec!["open-half-life", mode];
            if mode != "--reachability-report" {
                args.push("fixture");
            }
            let mut benchmark = args.clone();
            benchmark.extend(["--benchmark-seconds", "1"]);
            assert!(Cli::try_parse_from(benchmark).is_err());
            args.push("--profile-frames");
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    /// The app's own `--overbright` default is the round 5 calibrated
    /// `1.7`, not the engine's raw `1.0` (see
    /// `ohl_world::lightmap::LightRamp::default()` and
    /// `ohl_engine::GameConfig::default()`, both left unchanged at `1.0`).
    /// This is a project display default, not a claimed engine fact; see
    /// `docs/FORMAT_SOURCES.md`, "Rendering conventions".
    #[test]
    fn the_overbright_default_is_the_calibrated_1_7() {
        let cli = Cli::parse_from(["open-half-life", "--play"]);
        assert!(
            (cli.overbright - 1.7).abs() < f32::EPSILON,
            "expected the calibrated 1.7 default, got {}",
            cli.overbright
        );
    }

    /// A caller can still opt back into the engine's raw, unmultiplied
    /// lightmap ramp with an explicit `--overbright 1.0`.
    #[test]
    fn an_explicit_overbright_of_1_0_still_yields_the_raw_ramp() {
        let cli = Cli::parse_from(["open-half-life", "--overbright", "1.0", "--play"]);
        assert!(
            (cli.overbright - 1.0).abs() < f32::EPSILON,
            "expected the raw 1.0 ramp, got {}",
            cli.overbright
        );
    }

    #[test]
    fn platform_line_has_expected_shape() {
        let line = platform_line();
        assert!(line.starts_with("Platform: "));
        assert!(line.split_whitespace().count() >= 3);
    }

    #[test]
    fn the_built_in_recipe_includes_every_offered_component() {
        let recipe = load_recipe(None).expect("the built-in recipe parses");
        assert_eq!(
            recipe.default_decision(),
            ohl_payload::SelectionDecision::Include
        );
        assert_eq!(recipe.rule_count(), 0);
        assert!(DEFAULT_RECIPE.contains("version = 1"));
    }

    #[test]
    fn a_worker_that_refuses_the_enumeration_is_reported_as_unsupported_and_succeeds() {
        assert_eq!(
            report_import(Err(ohl_import::ImportError::Unsupported)),
            ExitCode::SUCCESS
        );
    }

    #[test]
    fn a_missing_worker_image_remains_a_failure_exit() {
        assert_eq!(
            report_import(Err(ohl_import::ImportError::WorkerUnavailable(
                ohl_platform::IsolatedWorkerError::ServiceUnavailable,
            ))),
            ExitCode::from(super::EXIT_FAILURE)
        );
    }

    /// Drives the composition root's import step with a worker that refuses
    /// to answer, which is exactly what the shipped parser worker does.
    #[test]
    fn the_composition_root_maps_a_refusing_worker_onto_a_successful_exit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = std::fs::canonicalize(directory.path()).expect("resolved directory");
        let image = crate::test_support::synthetic_container_iso();
        let iso = root.join("synthetic.iso");
        std::fs::write(&iso, &image).expect("synthetic iso fixture");

        let source = Arc::new(MediaSource::open(&iso).expect("pinned source"));
        let validated = ValidatedMedia::fingerprinting(
            Arc::clone(&source),
            MediaDescription::new(
                MediaClass::Iso9660,
                "iso9660",
                VolumeLabel::sanitized("SYNTHETIC"),
            ),
        )
        .expect("stable synthetic source");
        let mount = Mount::open(source, DirectoryLimits::default()).expect("mounted image");
        let layout = CacheLayout::with_root(root.join("cache")).expect("cache layout");

        // The transport answers the handshake and then closes, which is how
        // the shipped worker's `unsupported` dispatcher behaves.
        let transport = Arc::new(SyntheticTransport::new());
        let worker = FakeWorker::new(Arc::clone(&transport));
        let allocation = ohl_import::SessionIdAllocator::new()
            .allocate()
            .expect("identity");
        transport.push_frame(
            &ohl_parser_protocol::FrameHeader::new(
                ohl_parser_protocol::MessageType::Ready,
                allocation.session_id.get(),
                0,
                0,
            ),
            &[],
        );

        let code = super::import_payload_with_worker(
            &validated,
            &mount,
            &layout,
            Path::new(&root.join("payload")),
            worker.clone(),
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(worker.terminate_calls(), 1, "the worker is reaped once");
        assert!(
            ohl_import::pipeline::recorded_payload_identity(&layout, &validated).is_none(),
            "a refused import records nothing"
        );
    }
}
