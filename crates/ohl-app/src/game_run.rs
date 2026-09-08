//! The production playable loop over an imported payload.
//!
//! This is the real path, not a development aid: every asset is resolved
//! through [`ohl_assets::AssetFs`] over a published payload tree, the start
//! map comes from `ohl-campaign`'s sourced table, and the whole frame is
//! composed by [`ohl_engine::Game`]. This module only wires input, a window
//! or an offscreen target, and the UI shell onto it.
//!
//! Logging policy is the project's usual one: no media-derived string,
//! count or size ever reaches a log line, which includes map names, model
//! paths, entity counts and the user's own command-line paths.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "dev-tools")]
use glam::Vec3;
use ohl_engine::{AssetFsSource, Game, GameConfig, GameEvent, Input, RenderTarget};
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, WindowSurface, wgpu};
use ohl_ui::{UiLayer, console::Console, hud::HudState};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// The offscreen capture size, in pixels.
const CAPTURE_SIZE: (u32, u32) = (1280, 720);

/// The initial window size in physical pixels.
const INITIAL_SIZE: (u32, u32) = (1280, 720);

/// The fixed step headless capture advances the simulation by, so a capture
/// is reproducible regardless of how fast the host renders it.
const CAPTURE_STEP: f32 = 1.0 / 60.0;

/// How often the frame-rate line is logged.
const FPS_INTERVAL: Duration = Duration::from_secs(2);

/// How long a chapter title stays on the HUD, in seconds.
const CHAPTER_TITLE_SECONDS: f32 = 5.0;

/// A caller-chosen camera placement for a headless capture.
#[derive(Debug, Clone, Copy)]
pub struct Viewpoint {
    /// World-space position.
    pub position: [f32; 3],
    /// Pitch in degrees, positive looking down.
    pub pitch: f32,
    /// Yaw in degrees.
    pub yaw: f32,
}

impl std::str::FromStr for Viewpoint {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split(',');
        let mut next = || -> Result<f32, String> {
            parts
                .next()
                .and_then(|part| part.trim().parse::<f32>().ok())
                .filter(|number| number.is_finite())
                .ok_or_else(|| "expected x,y,z,pitch,yaw as five finite numbers".to_string())
        };
        let position = [next()?, next()?, next()?];
        let pitch = next()?;
        let yaw = next()?;
        if parts.next().is_some() {
            return Err("expected x,y,z,pitch,yaw as five finite numbers".to_string());
        }
        Ok(Self {
            position,
            pitch,
            yaw,
        })
    }
}

/// How a headless/scripted capture's camera is placed for each frame it
/// renders.
///
/// `--viewpoint` (and `--viewpoint-at-nearest-monster`) freeze the camera
/// in world space, in noclip: [`Game::set_viewpoint`] is called once, and
/// every subsequent frame renders from wherever that landed, regardless of
/// what the rest of the level does around it. `--spawn-offset` instead
/// rides with the player: no noclip is ever enabled, so the walking
/// player keeps colliding and moving normally (including riding a mover),
/// and each frame's render eye is recomputed as that player's *current*
/// eye position plus the caller's fixed offset (see [`rider_pose`]).
enum CapturePose {
    /// Neither flag was given: render from the map's own player start,
    /// unmodified.
    None,
    /// `--viewpoint`/`--viewpoint-at-nearest-monster`: already applied to
    /// the game's tracked camera via [`Game::set_viewpoint`]; every frame
    /// renders from there as-is.
    Frozen,
    /// `--spawn-offset`: recomputed fresh from the player's current eye
    /// position every time a frame is rendered or a solid-geometry check
    /// is made.
    Rider(Viewpoint),
}

/// The camera pose a `--spawn-offset` rider capture uses right now: the
/// player's current physics eye position plus the caller's offset, so the
/// camera keeps riding along with the player (a moving `func_train`, the
/// tram) instead of freezing in world space the way `--viewpoint` does.
fn rider_pose(game: &Game, offset: Viewpoint) -> Viewpoint {
    let eye = game.eye_position();
    let camera = game.camera();
    Viewpoint {
        position: [
            eye[0] + offset.position[0],
            eye[1] + offset.position[1],
            eye[2] + offset.position[2],
        ],
        pitch: camera.pitch + offset.pitch,
        yaw: camera.yaw + offset.yaw,
    }
}

/// Whether `pose`'s current position sits inside solid collision geometry
/// right now, checked once per call (a rider pose is recomputed each time,
/// since the player may have moved since the last check).
fn pose_is_in_solid(game: &Game, pose: &CapturePose) -> bool {
    match pose {
        CapturePose::None => false,
        CapturePose::Frozen => game.eye_is_in_solid(),
        CapturePose::Rider(offset) => game.position_is_in_solid(rider_pose(game, *offset).position),
    }
}

/// Renders one frame per `pose`: [`CapturePose::None`]/[`CapturePose::Frozen`]
/// draw from the game's own tracked camera (already set to the frozen
/// world-space pose, if any, by the caller); [`CapturePose::Rider`] draws
/// from the player's current eye position plus the offset via
/// [`Game::render_from`], leaving the tracked camera untouched so the next
/// [`Game::tick`] keeps simulating the walking player normally.
fn render_capture(
    game: &mut Game,
    context: &GpuContext,
    target: RenderTarget<'_>,
    pose: &CapturePose,
) -> Result<(), &'static str> {
    match pose {
        CapturePose::Rider(offset) => {
            let rider = rider_pose(game, *offset);
            game.render_from(context, target, rider.position, rider.pitch, rider.yaw)
        }
        CapturePose::Frozen | CapturePose::None => game.render(context, target),
    }
    .map_err(|_| "the frame could not be rendered")
}

/// Everything the playable loop needs from the command line.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is an independent command-line switch, not related state a caller could \
confuse for one another"
)]
pub struct GameArgs<'a> {
    /// The published payload's `files/` directory.
    pub payload_files: &'a Path,
    /// The map to load.
    pub map: &'a str,
    /// Where to write a PNG capture instead of opening a window.
    pub screenshot: Option<&'a Path>,
    /// How many frames a headless capture advances before writing.
    pub frames: u32,
    /// Where to stand for a headless capture.
    pub viewpoint: Option<Viewpoint>,
    /// Where to stand for a headless capture, relative to the map's player
    /// start. Ignored when `viewpoint` is given.
    pub spawn_offset: Option<Viewpoint>,
    /// A save slot to resume from instead of loading `map` fresh.
    pub load_slot: Option<&'a str>,
    /// The campaign difficulty.
    pub difficulty: ohl_campaign::Difficulty,
    /// A deterministic scripted-input file (`crate::script`), run instead
    /// of the interactive window or the frame-count capture loop.
    pub script: Option<&'a Path>,
    /// A chain of scripted-input route files (`--chain-script`, given once
    /// per route, in chain order), run instead of a single `script`: see
    /// [`run_chained`]. Empty when no chain was asked for.
    pub chain_script: &'a [PathBuf],
    /// Enables the scripted-input milestone log lines. Ignored without
    /// `script`.
    pub script_log: bool,
    /// The lightmap ramp's overbright multiplier (`--overbright`); see
    /// `ohl_engine::GameConfig::overbright`.
    pub overbright: f32,
    /// Follows a `trigger_changelevel` during a headless/scripted run
    /// (`--follow-level-change`) instead of logging that it was not
    /// followed and staying on the original map.
    pub follow_level_change: bool,
    /// Places the capture viewpoint this many units from the nearest
    /// spawned monster instead of at the map's player start or a caller
    /// chosen viewpoint (`--viewpoint-at-nearest-monster`, `dev-tools`
    /// only). Ignored without `headless_screenshot`.
    #[cfg(feature = "dev-tools")]
    pub viewpoint_at_nearest_monster: Option<f32>,
    /// Runs the bounded reachability/route-triage walk
    /// (`--reachability-report`, `dev-tools` only) instead of the
    /// interactive window, a capture, or a script, and prints its report.
    #[cfg(feature = "dev-tools")]
    pub reachability_report: bool,
    /// Treats a `func_breakable` on the reachability walk's frontier as
    /// openable-by-damage between rounds (`--reachability-assume-armed`,
    /// `dev-tools` only). Ignored without `reachability_report`.
    #[cfg(feature = "dev-tools")]
    pub reachability_assume_armed: bool,
    /// Adds a long-jump edge to the reachability walk
    /// (`--reachability-assume-longjump`, `dev-tools` only). Ignored
    /// without `reachability_report`.
    #[cfg(feature = "dev-tools")]
    pub reachability_assume_longjump: bool,
    /// Treats a `func_pendulum` on the reachability walk's frontier as
    /// passable between rounds (`--reachability-assume-pendulum-wait`,
    /// `dev-tools` only). Ignored without `reachability_report`.
    #[cfg(feature = "dev-tools")]
    pub reachability_assume_pendulum_wait: bool,
    /// Overrides the reachability walk's per-round cell cap
    /// (`--reachability-cell-cap`, `dev-tools` only), bounded by
    /// `ohl_engine::reachability::MAX_CELL_CAP`. `None` keeps
    /// `ohl_engine::reachability::ReachabilityConfig::default`'s own cap.
    /// Ignored without `reachability_report`.
    #[cfg(feature = "dev-tools")]
    pub reachability_cell_cap: Option<usize>,
    /// Overrides the reachability walk's door-opening round cap
    /// (`--reachability-round-cap`, `dev-tools` only), bounded by
    /// `ohl_engine::reachability::MAX_ROUND_CAP`. `None` keeps
    /// `ohl_engine::reachability::ReachabilityConfig::default`'s own cap.
    /// Ignored without `reachability_report`.
    #[cfg(feature = "dev-tools")]
    pub reachability_round_cap: Option<usize>,
    /// A `--start-inventory` list (`dev-tools` only): comma-separated
    /// `weapon_*`/`ammo_*` classnames given to the player right after the
    /// map loads. See `ohl_engine::parse_start_inventory`.
    #[cfg(feature = "dev-tools")]
    pub start_inventory: Option<&'a str>,
}

/// The fixed line a run prints once, right after a successful load, when
/// the loaded map really does have an entity world: at least one entity
/// definition, *and* either a resolved player start or an `info_landmark`
/// to arrive at. `cargo xtask campaign-smoke`
/// requires this exact line before it scores a map as loaded, so a map that
/// renders a room but has no entities in it can never pass again. Carries
/// nothing media-derived: no map name, no counts.
pub const ENTITY_WORLD_OK_LINE: &str =
    "Entity world loaded: the map declares entities and a spawn or landmark to arrive at.";

/// The fixed line printed instead of [`ENTITY_WORLD_OK_LINE`] when the map
/// loaded with no entities at all, or with neither a player start nor a
/// landmark.
pub const ENTITY_WORLD_EMPTY_LINE: &str =
    "Entity world empty: the map declares no entities, or neither a player start nor a landmark.";

/// The save directory this run reads and writes slots in, or `None` when the
/// platform publishes no per-user data directory.
fn save_slot_dir() -> Option<ohl_save::SaveSlot> {
    ohl_save::SaveSlot::default_dir().map(ohl_save::SaveSlot::new)
}

/// A save file's creation timestamp. Wall-clock time is host state, not
/// game state, so the engine takes it as an argument.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Runs the playable loop, either headless (writing a PNG) or windowed.
///
/// Returns a fixed, sanitized message on failure; the caller prints it as
/// is.
pub fn run(args: &GameArgs<'_>) -> Result<(), &'static str> {
    let root = game_root(args.payload_files);
    let asset_fs = ohl_assets::AssetFs::mount_default(&root)
        .map_err(|_| "the payload directory could not be indexed")?;
    let source = AssetFsSource::new(asset_fs);
    let config = GameConfig {
        difficulty: args.difficulty,
        overbright: args.overbright,
    };
    let mut game = match args.load_slot {
        Some(name) => {
            let slot = save_slot_dir().ok_or("no per-user save directory is available")?;
            // Neither the slot name nor the saved map name is logged: one is
            // user-supplied, the other media-derived.
            Game::load_slot_with(&source, &slot, name, &config)
                .map_err(|_| "the save slot could not be loaded")?
        }
        None => Game::load_with(&source, args.map, &config).map_err(|_| {
            // The map name is media-derived, so the reason names the step,
            // not the asset.
            "the start map could not be loaded from the payload"
        })?,
    };
    tracing::info!("Map loaded.");
    // A map reached only through a `trigger_changelevel`/`info_landmark`
    // pair legitimately declares no `info_player_start` of its own (the
    // player arrives relative to the landmark), so a landmark counts as
    // evidence of a real, loaded entity world just as a player start does.
    // What is never legitimate is a map with neither — nor one with no
    // entity definitions at all.
    if game.entity_def_count() > 0 && (game.has_player_start() || game.has_landmark()) {
        tracing::info!("{ENTITY_WORLD_OK_LINE}");
    } else {
        // A map that loads with no entity definitions, or with none the
        // spawn resolver recognised as a player start, is an empty room:
        // the player is placed at the world origin, which is normally
        // inside solid geometry. Before the entities lump's own failure was
        // surfaced as a load error this was completely silent.
        tracing::warn!("{ENTITY_WORLD_EMPTY_LINE}");
    }
    if game.entity_lump_relaxed_strings() > 0 {
        // Deliberately no count and no text: only the fact that the
        // relaxed decode path was taken at all.
        tracing::info!(
            "The map's entity lump holds at least one non-UTF-8 byte inside a quoted \
             string; it was decoded byte-per-byte."
        );
    }
    if game.missing_model_count() > 0 {
        // Deliberately no count: it is derived from the map's own contents.
        tracing::info!("Some referenced models are not published in this payload; skipped.");
    }
    if !game.has_collision() {
        tracing::warn!("The map has no usable collision hulls; the camera flies instead.");
    }

    #[cfg(feature = "dev-tools")]
    if let Some(spec) = args.start_inventory {
        let items = ohl_engine::parse_start_inventory(spec).map_err(|_| {
            "the --start-inventory list named an item this project does not \
recognise (expected a comma-separated list of weapon_*/ammo_* classnames)"
        })?;
        game.give_start_inventory(&items);
    }

    #[cfg(feature = "dev-tools")]
    if args.reachability_report {
        run_reachability_report(
            &mut game,
            args.reachability_assume_armed,
            args.reachability_assume_longjump,
            args.reachability_assume_pendulum_wait,
            args.reachability_cell_cap,
            args.reachability_round_cap,
        );
        return Ok(());
    }

    if !args.chain_script.is_empty() {
        return run_chained(&mut game, &source, args, args.chain_script);
    }

    if let Some(script_path) = args.script {
        return run_scripted(&mut game, &source, args, script_path);
    }

    match args.screenshot {
        Some(path) => capture(&mut game, &source, args, path),
        None => windowed(game, &source),
    }
}

/// Logs the outcome of a `GameEvent::LevelChange` a headless/scripted run
/// just received: with `--follow-level-change`, calls the same
/// [`Game::change_level`] the windowed loop uses and keeps ticking on the
/// destination map, logging the fixed "A level change was followed." line
/// (gated on `script_log`, matching this module's other milestone lines);
/// without the flag, or when the destination could not be loaded, the
/// original map keeps running and the existing "not followed" line is
/// logged unconditionally, exactly as before this flag existed.
fn handle_level_change(
    game: &mut Game,
    source: &AssetFsSource,
    map: &str,
    landmark: &str,
    follow: bool,
    script_log: bool,
) -> bool {
    if follow && game.change_level(source, map, landmark).is_ok() {
        if script_log {
            tracing::info!("A level change was followed.");
        }
        true
    } else {
        tracing::info!("A level change fired during capture; it was not followed.");
        false
    }
}

/// Runs a deterministic scripted-input file: `script.len()` ticks at
/// [`CAPTURE_STEP`], with no GPU context created unless `args.screenshot`
/// is also given. See `crate::script` for the grammar and
/// `crate::script_log` for the milestone log lines.
fn run_scripted(
    game: &mut Game,
    source: &AssetFsSource,
    args: &GameArgs<'_>,
    script_path: &Path,
) -> Result<(), &'static str> {
    let bytes = std::fs::read(script_path).map_err(|_| "the script file could not be read")?;
    let script =
        crate::script::Script::parse(&bytes).map_err(|_| "the script file could not be parsed")?;

    if args.script_log {
        tracing::info!("Scripted input loaded.");
    }

    // `--viewpoint-at-nearest-monster` is applied once, right here, before
    // the script's own ticks run — the same "place once, then let the
    // world go" shape `--viewpoint` and `--spawn-offset` already have. A
    // scripted capture is exactly where a motion capture from a chosen
    // vantage point is most useful, so this flag is wired in rather than
    // silently ignored under `--script` (J4).
    #[cfg(feature = "dev-tools")]
    let placed_at_monster = if let Some(distance) = args.viewpoint_at_nearest_monster {
        place_viewpoint_near_nearest_monster(game, distance);
        true
    } else {
        false
    };
    #[cfg(not(feature = "dev-tools"))]
    let placed_at_monster = false;

    let pose = if placed_at_monster {
        CapturePose::Frozen
    } else if let Some(viewpoint) = args.viewpoint {
        game.set_viewpoint(viewpoint.position, viewpoint.pitch, viewpoint.yaw);
        CapturePose::Frozen
    } else if let Some(offset) = args.spawn_offset {
        CapturePose::Rider(offset)
    } else {
        CapturePose::None
    };

    // See `capture`'s own identical check: a frozen (`--viewpoint`/
    // `--viewpoint-at-nearest-monster`) pose can start inside solid
    // geometry, and a rider (`--spawn-offset`) pose can end up there once
    // the player has moved (see J1). Checked again after the script's own
    // ticks run, below.
    if !matches!(pose, CapturePose::None) && pose_is_in_solid(game, &pose) {
        tracing::warn!("Capture viewpoint starts inside solid geometry.");
    }

    let mut log = crate::script_log::ScriptLog::new(game);
    run_script_ticks(
        game,
        source,
        &script,
        &mut log,
        &TickOptions {
            script_log: args.script_log,
            follow_level_change: args.follow_level_change,
            stop_on_level_change: false,
        },
    );

    if args.script_log {
        tracing::info!("Scripted input finished.");
    }

    if !matches!(pose, CapturePose::None) && pose_is_in_solid(game, &pose) {
        tracing::warn!("Capture viewpoint ends inside solid geometry.");
    }

    match args.screenshot {
        Some(path) => write_screenshot(game, path, &pose),
        None => Ok(()),
    }
}

/// How [`run_script_ticks`] treats the events one route's ticks produce.
struct TickOptions {
    /// Emits the `crate::script_log` milestone lines.
    script_log: bool,
    /// Passed straight to [`handle_level_change`].
    follow_level_change: bool,
    /// Ends the route as soon as a level change has actually been
    /// followed, leaving the rest of this script's ticks unrun. `false`
    /// for a plain `--script` run, whose remaining ticks keep running on
    /// the destination map exactly as they did before a chain walk
    /// existed; `true` for one leg of a `--chain-script` walk, where the
    /// next leg's own route takes over at the arrival point.
    stop_on_level_change: bool,
}

/// What one route's ticks did, as data: the caller decides what to log.
struct TickOutcome {
    /// A `trigger_changelevel` fired and was followed onto its
    /// destination map.
    followed_level_change: bool,
    /// How many simulation ticks actually ran. Fewer than the script
    /// scheduled when [`TickOptions::stop_on_level_change`] cut the route
    /// short.
    ticks: u64,
}

/// Ticks one parsed script through [`Game::tick`] at [`CAPTURE_STEP`],
/// handling the events it produces. Shared by [`run_scripted`] (one
/// script, run to its end) and [`run_chained`] (one script per map, each
/// ending at the level change that carries the player into the next one).
fn run_script_ticks(
    game: &mut Game,
    source: &AssetFsSource,
    script: &crate::script::Script,
    log: &mut crate::script_log::ScriptLog,
    options: &TickOptions,
) -> TickOutcome {
    let mut outcome = TickOutcome {
        followed_level_change: false,
        ticks: 0,
    };
    for input in script.inputs() {
        for event in game.tick(CAPTURE_STEP, input) {
            match event {
                GameEvent::LevelChange { map, landmark } => {
                    let followed = handle_level_change(
                        game,
                        source,
                        &map,
                        &landmark,
                        options.follow_level_change,
                        options.script_log,
                    );
                    outcome.followed_level_change |= followed;
                }
                // The same fixed line the interactive window logs
                // (`GameRun::draw`, below): a scripted/headless run is
                // otherwise silent about the player's death, even though
                // `ohl_engine::Systems::step` has already stopped
                // simulating the player's movement from this point on.
                GameEvent::PlayerDied => {
                    tracing::info!("The player died.");
                }
                GameEvent::ChapterTitle(_)
                | GameEvent::Message { .. }
                | GameEvent::Sound(_)
                | GameEvent::Suit(_)
                | GameEvent::ViewModel(_) => {}
            }
        }
        outcome.ticks += 1;
        if options.script_log {
            log.observe(game, CAPTURE_STEP);
        }
        if options.stop_on_level_change && outcome.followed_level_change {
            break;
        }
    }
    outcome
}

/// The fixed line a chain walk logs when one of its routes ran out of
/// scripted ticks without reaching a `trigger_changelevel`: the chain got
/// no further than the map that route ran on.
const CHAIN_STOPPED: &str = "The chain walk stopped.";

/// The fixed line a chain walk logs when every route it was given did
/// reach a level change, so the walk ended only because no route was
/// authored for the map it last arrived in. Not a failure.
const CHAIN_NO_FURTHER_ROUTE: &str = "The chain walk has no further route.";

/// Runs a *sequence* of scripted-input routes across level changes in one
/// process: `routes[0]` from the start map's own player start, and every
/// later route from the point the preceding route's followed level change
/// put the player down in the destination map — carrying health, armor,
/// weapons and ammo through `ohl_engine::transition`'s own machinery,
/// which is exactly what a cold `--map <name>` load of a mid-campaign map
/// cannot reproduce.
///
/// A route ends at the first level change it follows (the next route takes
/// over there) or when its own ticks run out (the chain stops). Level
/// changes are always followed here: a chain walk that did not follow them
/// would be a plain `--script` run.
///
/// Logs, beyond the per-hop "A level change was followed." line
/// [`handle_level_change`] already emits: one of the two fixed terminal
/// lines above, plus the walk's own bounded aggregates (how many maps deep
/// it got and how many simulated seconds that took). No map name, entity
/// name or position is logged, here or anywhere below.
fn run_chained(
    game: &mut Game,
    source: &AssetFsSource,
    args: &GameArgs<'_>,
    routes: &[PathBuf],
) -> Result<(), &'static str> {
    let mut scripts = Vec::with_capacity(routes.len());
    for path in routes {
        let bytes = std::fs::read(path).map_err(|_| "the script file could not be read")?;
        scripts.push(
            crate::script::Script::parse(&bytes)
                .map_err(|_| "the script file could not be parsed")?,
        );
    }

    if args.script_log {
        tracing::info!("Scripted input loaded.");
    }

    let mut ticks: u64 = 0;
    let mut depth: usize = 1;
    let mut stopped = false;
    for script in &scripts {
        // A fresh log per route, so every milestone line is observed from
        // this map's own arrival point rather than from the chain's start.
        let mut log = crate::script_log::ScriptLog::new(game);
        let outcome = run_script_ticks(
            game,
            source,
            script,
            &mut log,
            &TickOptions {
                script_log: args.script_log,
                follow_level_change: true,
                stop_on_level_change: true,
            },
        );
        ticks += outcome.ticks;
        if outcome.followed_level_change {
            depth += 1;
        } else {
            stopped = true;
            break;
        }
    }

    if args.script_log {
        tracing::info!("Scripted input finished.");
    }
    if stopped {
        tracing::info!("{CHAIN_STOPPED}");
    } else {
        tracing::info!("{CHAIN_NO_FURTHER_ROUTE}");
    }
    // Two bounded aggregates over project-authored routes (how many maps
    // the chain entered, and how much simulated time the routes it ran
    // took), in the fixed shapes `xtask/src/chain_walk.rs` parses. Neither
    // is a media-derived name, path or content figure.
    tracing::info!("Chain walk depth: {depth}.");
    #[allow(clippy::cast_precision_loss, reason = "a tick count for a report line")]
    let seconds = ticks as f32 * CAPTURE_STEP;
    tracing::info!("Chain walk simulated seconds: {seconds:.1}.");
    Ok(())
}

/// Renders exactly one frame and writes it as a PNG. Shared by
/// [`run_scripted`]; [`capture`] renders once per advanced frame instead,
/// since a capture without a script advances the world's own animation one
/// tick at a time between renders.
fn write_screenshot(game: &mut Game, path: &Path, pose: &CapturePose) -> Result<(), &'static str> {
    let context = GpuContext::headless().map_err(|_| "no usable graphics adapter is available")?;
    let (width, height) = CAPTURE_SIZE;
    let target = OffscreenTarget::new(&context, width, height)
        .map_err(|_| "no offscreen target could be created")?;
    render_capture(
        game,
        &context,
        RenderTarget {
            view: target.view(),
            width,
            height,
            format: OFFSCREEN_FORMAT,
        },
        pose,
    )?;
    context.wait();

    let pixels = target
        .read_rgba(&context)
        .map_err(|_| "the frame could not be read back")?;
    let image = image::RgbaImage::from_raw(width, height, pixels)
        .ok_or("the frame did not fill the capture buffer")?;
    image
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|_| "the capture could not be written")?;
    tracing::info!("Screenshot written.");
    Ok(())
}

/// Development only: runs `ohl_engine::reachability`'s bounded walk from
/// the map's player start and prints its report.
///
/// Every printed line is either a fixed string, a classname (this
/// project's own documented entity vocabulary), an aggregate count, or a
/// distance already rounded to the nearest ten units by
/// `ohl_engine::reachability` itself — never a map name, a coordinate, or
/// a targetname (`docs/CLEAN_ROOM.md`; the caller already knows which map
/// it asked for).
#[cfg(feature = "dev-tools")]
fn run_reachability_report(
    game: &mut Game,
    assume_armed: bool,
    assume_longjump: bool,
    assume_pendulum_wait: bool,
    cell_cap: Option<usize>,
    round_cap: Option<usize>,
) {
    if !game.has_collision() {
        tracing::info!("Reachability report: the map has no usable collision hulls.");
        return;
    }

    let default_config = ohl_engine::ReachabilityConfig::default();
    let config = ohl_engine::ReachabilityConfig {
        assume_armed,
        assume_longjump,
        assume_pendulum_wait,
        cell_cap: cell_cap.unwrap_or(default_config.cell_cap),
        max_rounds: round_cap.unwrap_or(default_config.max_rounds),
    };
    let report = ohl_engine::compute_reachability_report(game, &config);

    tracing::info!("Reachability report:");
    for round in &report.rounds {
        print_reachability_round(round);
    }
}

/// Prints one [`ohl_engine::RoundReport`]'s fixed lines, per
/// [`run_reachability_report`]'s own logging policy (split out of that
/// function solely to keep it under this project's own line-count lint).
#[cfg(feature = "dev-tools")]
fn print_reachability_round(round: &ohl_engine::RoundReport) {
    tracing::info!(
        "Round {}: {} cell(s) reachable{}{}.",
        round.round,
        round.reachable_cells,
        if round.capped { " (capped)" } else { "" },
        if round.pendulum_wait {
            " (pendulum wait assumed)"
        } else {
            ""
        }
    );
    if round.frontier_classes.is_empty() {
        tracing::info!("  Frontier: nothing blocking (or the walk found open space only).");
    }
    for class in &round.frontier_classes {
        tracing::info!(
            "  Frontier: {} x{}{}{}{}{}.",
            class.classname,
            class.instance_count,
            if class.use_openable {
                ", use-openable from a reached cell"
            } else {
                ", not use-openable from a reached cell"
            },
            if class.damage_openable {
                ", damage-openable (armed assumed)"
            } else {
                ""
            },
            if class.push_openable {
                ", push-openable"
            } else {
                ""
            },
            if class.pendulum_openable {
                ", pendulum-openable (pendulum wait assumed)"
            } else {
                ""
            }
        );
    }
    match (
        round.changelevel.reachable,
        round.changelevel.distance_rounded,
    ) {
        (true, Some(distance)) => {
            tracing::info!("  trigger_changelevel: reachable, ~{distance:.0} units from spawn.");
        }
        (true, None) => {
            // Not expected (a reachable trigger always has a distance),
            // but never fabricate one.
            tracing::info!("  trigger_changelevel: reachable.");
        }
        (false, Some(distance)) => tracing::info!(
            "  trigger_changelevel: not reachable this round, ~{distance:.0} units from spawn."
        ),
        (false, None) => tracing::info!("  trigger_changelevel: none declared."),
    }
    if round.long_drop_cells > 0 {
        tracing::info!(
            "  {} cell(s) reached only by a one-way fall taller than the walk's old {:.0}-unit bound.",
            round.long_drop_cells,
            ohl_engine::reachability::DROP,
        );
    }
    if round.long_jump_cells > 0 {
        tracing::info!(
            "  {} cell(s) reached only by the long-jump edge (armed with the long jump module assumed).",
            round.long_jump_cells,
        );
    }
    if round.doors_opened > 0 {
        tracing::info!(
            "  Opening {} door(s) for the next round.",
            round.doors_opened
        );
    }
    if round.breakables_opened > 0 {
        tracing::info!(
            "  Breaking {} breakable(s) for the next round.",
            round.breakables_opened
        );
    }
    if round.pushables_opened > 0 {
        tracing::info!(
            "  Pushing {} pushable(s) for the next round.",
            round.pushables_opened
        );
    }
    if round.pendulums_opened > 0 {
        tracing::info!(
            "  Treating {} pendulum(s) as passable for the next round (wait assumed).",
            round.pendulums_opened
        );
    }
}

/// Development only: places the capture eye `distance` units from whichever
/// spawned monster sits closest to the map's own player start, at the
/// monster's eye height, facing it, in noclip (see [`Game::set_viewpoint`]).
///
/// Logs only the fixed "placed"/"not found" lines documented on
/// `--viewpoint-at-nearest-monster`: never a position, a distance or a
/// classname, matching this module's usual logging policy.
#[cfg(feature = "dev-tools")]
fn place_viewpoint_near_nearest_monster(game: &mut Game, distance: f32) {
    let from = Vec3::from_array(game.camera().position);
    let Some(monster_eye) = game.nearest_monster_position(from) else {
        tracing::info!("No monster found for the capture viewpoint.");
        return;
    };
    let distance = if distance.is_finite() {
        distance.max(1.0)
    } else {
        1.0
    };

    // Try a handful of horizontal directions and keep the first one that
    // does not land the eye inside solid geometry: whichever side the
    // map's own player start sits on first (the direction a player would
    // actually have approached the monster from, so it is the likeliest
    // to be open space), then its opposite, then the four horizontal
    // axes, so the placement is deterministic (never a caller-chosen
    // angle) while still trying to land somewhere a frame is worth
    // capturing. Falls back to the first candidate if every one of them
    // is solid, since some placement is still owed to the caller and
    // `--headless-screenshot`'s own solid-geometry warning already covers
    // that case.
    let mut spawnward = from - monster_eye;
    spawnward.z = 0.0;
    let spawnward = if spawnward.length_squared() > 1e-6 {
        spawnward.normalize()
    } else {
        Vec3::X
    };
    let candidates = [
        spawnward,
        -spawnward,
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
    ];

    let mut fallback = None;
    let mut placed = false;
    for direction in candidates {
        let eye = Vec3::new(
            monster_eye.x + direction.x * distance,
            monster_eye.y + direction.y * distance,
            monster_eye.z,
        );
        let facing = -direction;
        let yaw = facing.y.atan2(facing.x).to_degrees();
        game.set_viewpoint(eye.to_array(), 0.0, yaw);
        fallback.get_or_insert((eye, yaw));
        if !game.eye_is_in_solid() {
            placed = true;
            break;
        }
    }
    if !placed {
        // Every candidate was solid; `candidates` is non-empty, so
        // `fallback` is always set by the loop above. Re-apply it: the
        // last iteration already left the *last* candidate active, not
        // necessarily the first.
        if let Some((eye, yaw)) = fallback {
            game.set_viewpoint(eye.to_array(), 0.0, yaw);
        }
    }
    tracing::info!("Capture viewpoint placed near a monster.");
}

/// The directory inside a published payload that holds the mod directories.
///
/// An installer stages its files under its own destination variable rather
/// than at the tree root, so the mod directories can sit one level in. This
/// looks for the base mod directory at the root first and then, in sorted
/// order for determinism, one level down. Nothing here is logged: every
/// name involved comes from the medium.
fn game_root(files: &Path) -> std::path::PathBuf {
    const BASE_MOD: &str = ohl_assets::DEFAULT_SEARCH_PATHS[0];

    if files.join(BASE_MOD).is_dir() {
        return files.to_path_buf();
    }
    let Ok(entries) = std::fs::read_dir(files) else {
        return files.to_path_buf();
    };
    let mut candidates: Vec<std::path::PathBuf> =
        entries.flatten().map(|entry| entry.path()).collect();
    candidates.sort();
    candidates
        .into_iter()
        .find(|candidate| candidate.join(BASE_MOD).is_dir())
        .unwrap_or_else(|| files.to_path_buf())
}

/// Renders `frames` frames offscreen and writes the last one as a PNG.
fn capture(
    game: &mut Game,
    source: &AssetFsSource,
    args: &GameArgs<'_>,
    path: &Path,
) -> Result<(), &'static str> {
    let context = GpuContext::headless().map_err(|_| "no usable graphics adapter is available")?;
    let (width, height) = CAPTURE_SIZE;
    let target = OffscreenTarget::new(&context, width, height)
        .map_err(|_| "no offscreen target could be created")?;

    #[cfg(feature = "dev-tools")]
    let placed_at_monster = if let Some(distance) = args.viewpoint_at_nearest_monster {
        place_viewpoint_near_nearest_monster(game, distance);
        true
    } else {
        false
    };
    #[cfg(not(feature = "dev-tools"))]
    let placed_at_monster = false;

    let pose = if placed_at_monster {
        // Handled above; the ordinary viewpoint/spawn-offset chain below is
        // mutually exclusive with it (clap's own `requires` wiring already
        // keeps `--viewpoint`/`--spawn-offset` and
        // `--viewpoint-at-nearest-monster` from making sense together, so
        // this just documents that this branch takes priority).
        CapturePose::Frozen
    } else if let Some(viewpoint) = args.viewpoint {
        game.set_viewpoint(viewpoint.position, viewpoint.pitch, viewpoint.yaw);
        CapturePose::Frozen
    } else if let Some(offset) = args.spawn_offset {
        // Relative to wherever the map's own player start put the camera,
        // so a capture can be aimed without anyone having to know (or
        // record) a map's coordinates. Unlike `--viewpoint` this never
        // enables noclip: the player spawns and moves normally (falling,
        // colliding, riding a mover), and `render_capture` recomputes the
        // render eye from the player's *current* position plus this
        // offset every frame, rather than freezing it in world space at
        // whatever the map's player start happened to be at tick 0 (J1).
        CapturePose::Rider(offset)
    } else {
        CapturePose::None
    };

    // A frozen (`--viewpoint`/`--viewpoint-at-nearest-monster`) pose runs
    // in noclip and so cannot push the camera clear of an accidental
    // overlap the way ordinary spawn placement does; a rider
    // (`--spawn-offset`) pose can start clear but end up inside geometry
    // the player has since moved through or a mover has vacated. Surface
    // both as a warning rather than silently writing a meaningless frame.
    // The message is a fixed string with no map-derived data, per this
    // module's logging policy. Checked again after the last frame, below.
    if !matches!(pose, CapturePose::None) && pose_is_in_solid(game, &pose) {
        tracing::warn!("Capture viewpoint starts inside solid geometry.");
    }

    for _ in 0..args.frames.max(1) {
        // The capture stands still (aside from a rider pose following the
        // player): only the world's own animation (doors, light styles,
        // liquid turbulence, model sequences) advances.
        let events = game.tick(CAPTURE_STEP, &Input::default());
        for event in events {
            match event {
                GameEvent::LevelChange { map, landmark } => {
                    handle_level_change(
                        game,
                        source,
                        &map,
                        &landmark,
                        args.follow_level_change,
                        args.script_log,
                    );
                }
                // Map-authored text, presentation events with nothing to
                // act on during a still capture (M7.9 P1): none of these
                // are logged.
                GameEvent::ChapterTitle(_)
                | GameEvent::Message { .. }
                | GameEvent::Sound(_)
                | GameEvent::Suit(_)
                | GameEvent::ViewModel(_) => {}
                GameEvent::PlayerDied => {
                    tracing::info!("The player died during capture.");
                }
            }
        }
        render_capture(
            game,
            &context,
            RenderTarget {
                view: target.view(),
                width,
                height,
                format: OFFSCREEN_FORMAT,
            },
            &pose,
        )?;
    }
    context.wait();

    if !matches!(pose, CapturePose::None) && pose_is_in_solid(game, &pose) {
        tracing::warn!("Capture viewpoint ends inside solid geometry.");
    }

    let pixels = target
        .read_rgba(&context)
        .map_err(|_| "the frame could not be read back")?;
    let image = image::RgbaImage::from_raw(width, height, pixels)
        .ok_or("the frame did not fill the capture buffer")?;
    image
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|_| "the capture could not be written")?;
    tracing::info!("Screenshot written.");
    Ok(())
}

/// Opens a window and runs the loop until it closes or Escape is pressed.
#[allow(clippy::needless_pass_by_value)]
fn windowed(game: Game, source: &AssetFsSource) -> Result<(), &'static str> {
    let event_loop = EventLoop::new().map_err(|_| "no window system is available")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        game,
        source,
        saves: save_slot_dir(),
        input: Input::default(),
        key_use_down: false,
        console: Console::new(),
        hud: HudState::default(),
        state: None,
        last_frame: Instant::now(),
        fps_window_start: Instant::now(),
        frames: 0,
        failure: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|_| "the window event loop stopped unexpectedly")?;
    app.failure.map_or(Ok(()), Err)
}

/// The window, GPU context, surface and UI layer, created together once
/// winit hands the application a display connection.
struct Active {
    window: Arc<Window>,
    context: GpuContext,
    surface: WindowSurface<'static>,
    ui: UiLayer,
}

struct App<'a> {
    game: Game,
    source: &'a AssetFsSource,
    /// The save directory quicksave/quickload and the level-change autosave
    /// use, when the platform publishes one.
    saves: Option<ohl_save::SaveSlot>,
    input: Input,
    /// Whether "use" was already down as of the last keyboard event, so the
    /// one-frame press edge is not re-latched by key repeat.
    key_use_down: bool,
    console: Console,
    hud: HudState,
    state: Option<Active>,
    last_frame: Instant,
    fps_window_start: Instant,
    frames: u32,
    failure: Option<&'static str>,
}

impl App<'_> {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: &'static str) {
        self.failure = Some(message);
        event_loop.exit();
    }

    fn set_axis(&mut self, key: KeyCode, pressed: bool) {
        let value = i8::from(pressed);
        match key {
            KeyCode::KeyW => self.input.forward = value,
            KeyCode::KeyS => self.input.forward = -value,
            KeyCode::KeyD => self.input.right = value,
            KeyCode::KeyA => self.input.right = -value,
            KeyCode::Space => {
                self.input.up = value;
                self.input.jump = pressed;
            }
            KeyCode::ControlLeft => {
                self.input.up = -value;
                self.input.duck = pressed;
            }
            _ => {}
        }
    }

    /// Clears every held axis, so releasing the pointer into the console
    /// does not leave the player walking.
    fn release_movement(&mut self) {
        self.input = Input {
            mouse_delta: self.input.mouse_delta,
            ..Input::default()
        };
    }

    /// Writes the autosave slot after a level change, if a save directory
    /// exists. A failure is reported once and never retried in a loop.
    fn autosave(&mut self) {
        if self.write_slot(ohl_save::AUTOSAVE_SLOT_NAME) {
            tracing::info!("Autosaved.");
        }
    }

    fn quicksave(&mut self) {
        if self.write_slot(ohl_save::QUICKSAVE_SLOT_NAME) {
            tracing::info!("Quicksaved.");
        }
    }

    /// Writes one save slot, reporting failure as a fixed line. The slot
    /// name is never logged: it is either a constant or user-supplied.
    fn write_slot(&mut self, name: &str) -> bool {
        let Some(slot) = self.saves.as_ref() else {
            tracing::warn!("No per-user save directory is available; not saving.");
            return false;
        };
        if self.game.save_slot(slot, name, now_unix_secs()).is_ok() {
            true
        } else {
            tracing::warn!("The save could not be written.");
            false
        }
    }

    /// Reloads the quicksave slot in place.
    fn quickload(&mut self) {
        let Some(slot) = self.saves.as_ref() else {
            tracing::warn!("No per-user save directory is available; not loading.");
            return;
        };
        if let Ok(game) = Game::load_slot(self.source, slot, ohl_save::QUICKSAVE_SLOT_NAME) {
            self.game = game;
            tracing::info!("Quickload complete.");
        } else {
            tracing::warn!("The quicksave could not be loaded.");
        }
    }

    fn draw(&mut self) {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;

        // The held axes persist across frames; the two edge-triggered
        // fields (mouse motion and the "use" press) are consumed here.
        let frame_input = self.input;
        self.input.mouse_delta = (0.0, 0.0);
        self.input.use_pressed = false;
        for event in self.game.tick(delta.as_secs_f32(), &frame_input) {
            match event {
                GameEvent::LevelChange { map, landmark } => {
                    // Neither string is logged: both are map-derived.
                    if self.game.change_level(self.source, &map, &landmark).is_ok() {
                        tracing::info!("Level changed.");
                        self.autosave();
                    } else {
                        tracing::warn!("The destination map is not published; staying here.");
                    }
                }
                GameEvent::ChapterTitle(title) => {
                    // The title itself is map-derived, so it goes to the HUD
                    // and never to a log line.
                    self.hud.show_message(title, CHAPTER_TITLE_SECONDS);
                }
                GameEvent::Message { block } => {
                    let seconds = block.total_seconds();
                    self.hud.show_message(block.text, seconds);
                }
                // M7.9 P1 presentation events. `ohl_gameplay::SoundCue::path`
                // is always `None` until a clean-room provenance review
                // admits a sound asset path, and viewmodel/suit-voice
                // rendering are later work, so there is nothing to act on
                // here yet beyond the fixed line below.
                GameEvent::Sound(_) | GameEvent::Suit(_) | GameEvent::ViewModel(_) => {}
                GameEvent::PlayerDied => {
                    tracing::info!("The player died.");
                }
            }
        }
        // Health, armor, ammo and the damage flash are `Game::hud()`'s own
        // state (M7.9 P1), written every step from the player's inventory
        // and combat events; the title/message fields above are this
        // struct's own (`env_message`/chapter titles arrive as `GameEvent`s,
        // not through `HudState`), so this copies the former without
        // clobbering the latter.
        let engine_hud = self.game.hud();
        self.hud.health = engine_hud.health;
        self.hud.armor = engine_hud.armor;
        self.hud.clip_ammo = engine_hud.clip_ammo;
        self.hud.reserve_ammo = engine_hud.reserve_ammo;
        self.hud.damage_flash = self.hud.damage_flash.max(engine_hud.damage_flash);

        self.hud.decay_damage_flash(2.0, delta.as_secs_f32());
        self.hud.tick_message(delta.as_secs_f32());

        let Some(active) = self.state.as_mut() else {
            return;
        };
        let Some(frame) = active.surface.acquire(&active.context) else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (width, height) = (active.surface.width(), active.surface.height());
        if self
            .game
            .render(
                &active.context,
                RenderTarget {
                    view: &view,
                    width,
                    height,
                    format: active.surface.format(),
                },
            )
            .is_err()
        {
            self.failure = Some("the frame could not be rendered");
        }

        active.ui.begin_frame();
        ohl_ui::hud::draw(active.ui.context(), &self.hud);
        if self.console.is_open() {
            let mut root = ohl_ui::root_ui(active.ui.context());
            let _ = ohl_ui::console::draw_console(&mut root, &mut self.console);
        }
        let mut encoder =
            active
                .context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ohl ui encoder"),
                });
        active.ui.end_frame_and_render(
            &active.context.device,
            &active.context.queue,
            &mut encoder,
            &view,
            [width, height],
        );
        active.context.queue.submit([encoder.finish()]);
        active.context.queue.present(frame);

        self.frames += 1;
        let elapsed = now.saturating_duration_since(self.fps_window_start);
        if elapsed >= FPS_INTERVAL {
            #[allow(clippy::cast_precision_loss)]
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            // A property of this machine and this run, not of the map.
            tracing::info!(fps = format_args!("{fps:.1}"), "frame rate");
            self.frames = 0;
            self.fps_window_start = now;
        }
        active.window.request_redraw();
    }
}

impl ApplicationHandler for App<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Open Half-Life")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                INITIAL_SIZE.0,
                INITIAL_SIZE.1,
            ));
        let Ok(window) = event_loop.create_window(attributes) else {
            self.fail(event_loop, "a window could not be created");
            return;
        };
        let window = Arc::new(window);
        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
        window.set_cursor_visible(false);

        let Ok((context, wgpu_surface)) = GpuContext::for_surface(Arc::clone(&window)) else {
            self.fail(event_loop, "no usable graphics adapter is available");
            return;
        };
        let size = window.inner_size();
        let Ok(surface) = WindowSurface::new(&context, wgpu_surface, size.width, size.height)
        else {
            self.fail(event_loop, "the window surface could not be configured");
            return;
        };
        let ui = UiLayer::new_windowed(&context.device, Arc::clone(&window), surface.format());

        self.last_frame = Instant::now();
        self.fps_window_start = self.last_frame;
        self.frames = 0;
        self.state = Some(Active {
            window,
            context,
            surface,
            ui,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(active) = self.state.as_mut()
            && active.ui.handle_window_event(&event)
            && self.console.is_open()
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(active) = self.state.as_mut() {
                    active
                        .surface
                        .resize(&active.context, size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                if code == KeyCode::Backquote {
                    if pressed && !event.repeat {
                        self.console.toggle();
                        self.release_movement();
                    }
                    return;
                }
                if code == KeyCode::Escape {
                    if self.console.is_open() {
                        self.console.set_open(false);
                        return;
                    }
                    event_loop.exit();
                    return;
                }
                if self.console.is_open() {
                    return;
                }
                if code == KeyCode::F6 {
                    if pressed && !event.repeat {
                        self.quicksave();
                    }
                    return;
                }
                if code == KeyCode::F7 {
                    if pressed && !event.repeat {
                        self.quickload();
                    }
                    return;
                }
                if code == KeyCode::KeyE {
                    if pressed && !self.key_use_down {
                        self.input.use_pressed = true;
                    }
                    self.key_use_down = pressed;
                    self.input.use_held = pressed;
                    return;
                }
                self.set_axis(code, pressed);
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if self.console.is_open() {
            return;
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            #[allow(clippy::cast_possible_truncation)]
            let (delta_x, delta_y) = (delta.0 as f32, delta.1 as f32);
            self.input.mouse_delta.0 += delta_x;
            self.input.mouse_delta.1 += delta_y;
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(active) = self.state.as_ref() {
            active.window.request_redraw();
        }
    }
}

/// Unit tests for the rider-offset math (`--spawn-offset`) and the
/// solid-geometry checks that gate its warnings, against `ohl-engine`'s
/// own synthetic fixtures (`test-support`, a dev-dependency). No GPU is
/// needed: none of these call `Game::render`/`render_from`, only `tick`
/// and the pure position math.
#[cfg(test)]
mod tests {
    use super::*;
    use ohl_engine::test_support::{
        MOVER_MAP, MOVER_SEGMENT_LENGTH, MOVER_SPEED, SYNTHETIC_MAP, mover_train_bsp,
        synthetic_map_bsp,
    };
    use ohl_engine::{AssetSource, MemoryAssets};

    fn load(map: &str, bsp: Vec<u8>) -> Game {
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{map}.bsp"), bsp);
        Game::load(&assets as &dyn AssetSource, map).expect("the synthetic map loads")
    }

    fn settle(game: &mut Game, ticks: u32) {
        for _ in 0..ticks {
            game.tick(CAPTURE_STEP, &Input::default());
        }
    }

    /// Ticks for the mover fixture to finish its one-segment ride, plus a
    /// little slack.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn ride_ticks() -> u32 {
        ((MOVER_SEGMENT_LENGTH / MOVER_SPEED) / CAPTURE_STEP) as u32
    }

    fn assert_positions_close(actual: [f32; 3], expected: [f32; 3]) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() < 1e-3,
                "expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The core J1 fix: the rider pose is not a snapshot taken once at
    /// spawn, it is recomputed from wherever the player's eye actually is
    /// right now — so once the player has ridden a mover well away from
    /// their spawn point, the rider pose moves with them.
    #[test]
    fn rider_pose_tracks_the_players_current_eye_position_plus_the_offset() {
        let mut game = load(MOVER_MAP, mover_train_bsp());
        // Let the player settle onto the train before it has covered any
        // ground, then ride along for most of its one-segment travel.
        settle(&mut game, 3);
        settle(&mut game, ride_ticks());

        let offset = Viewpoint {
            position: [0.0, 0.0, 32.0],
            pitch: -5.0,
            yaw: 10.0,
        };
        let pose = rider_pose(&game, offset);
        let eye = game.eye_position();
        let camera = game.camera();
        assert_positions_close(
            pose.position,
            [
                eye[0] + offset.position[0],
                eye[1] + offset.position[1],
                eye[2] + offset.position[2],
            ],
        );
        assert!((pose.pitch - (camera.pitch + offset.pitch)).abs() < 1e-6);
        assert!((pose.yaw - (camera.yaw + offset.yaw)).abs() < 1e-6);

        // And the player must actually have moved with the train by now
        // (not frozen at spawn) — otherwise this test would trivially
        // pass by never exercising the rider behaviour at all.
        assert!(
            eye[0] > MOVER_SEGMENT_LENGTH * 0.5,
            "the player did not ride the train: eye={eye:?}"
        );
    }

    /// The scenario J1 found broken on the real tram map: freezing the
    /// camera in world space at spawn ends the capture embedded in
    /// geometry the mover has since vacated. A rider pose, sampled
    /// throughout the whole ride, must never do that.
    #[test]
    fn rider_pose_never_ends_up_in_solid_geometry_the_train_has_left() {
        let mut game = load(MOVER_MAP, mover_train_bsp());
        settle(&mut game, 3);
        // An offset that keeps the eye at ordinary player height above the
        // train's own deck, matching how a real capture would frame it.
        let offset = Viewpoint {
            position: [0.0, 0.0, 0.0],
            pitch: 0.0,
            yaw: 0.0,
        };
        for tick in 0..ride_ticks() + 60 {
            game.tick(CAPTURE_STEP, &Input::default());
            if tick % 10 == 0 {
                assert!(
                    !game.position_is_in_solid(rider_pose(&game, offset).position),
                    "rider pose landed in solid geometry at tick {tick}"
                );
            }
        }
    }

    /// On a map with no mover, the player's eye stops changing once
    /// gravity settles them onto the floor, so a rider pose sampled at any
    /// later frame is the same value the old frozen-at-spawn behaviour
    /// would already have produced. This is the "identical on static
    /// maps" property the fix must keep for the seven standard viewpoints.
    #[test]
    fn a_static_maps_rider_pose_does_not_drift_once_the_player_has_settled() {
        let mut game = load(SYNTHETIC_MAP, synthetic_map_bsp());
        settle(&mut game, 30);
        let offset = Viewpoint {
            position: [10.0, -5.0, 2.0],
            pitch: 3.0,
            yaw: -8.0,
        };
        let first = rider_pose(&game, offset);
        settle(&mut game, 60);
        let later = rider_pose(&game, offset);
        assert_positions_close(first.position, later.position);
        assert!((first.pitch - later.pitch).abs() < 1e-6);
        assert!((first.yaw - later.yaw).abs() < 1e-6);
    }

    #[test]
    fn pose_is_in_solid_matches_the_capture_pose() {
        let game = load(SYNTHETIC_MAP, synthetic_map_bsp());
        assert!(!pose_is_in_solid(&game, &CapturePose::None));
        assert_eq!(
            pose_is_in_solid(&game, &CapturePose::Frozen),
            game.eye_is_in_solid()
        );
    }
}
