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
use ohl_engine::{
    AssetFsSource, AssetSource, ChannelClass, Game, GameConfig, GameEvent, Input, RenderTarget,
};
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, WindowSurface, wgpu};
use ohl_ui::{
    UiLayer,
    bindings::{Action, Binding},
    console::Console,
    debug::GraphicsDebugInfo,
    hud::HudState,
    menu::{
        BotSkill as MenuBotSkill, ChapterEntry, Difficulty as MenuDifficulty, DisplayMode,
        DisplaySettings, LevelEntry, MenuAction, MenuData, MenuPane, MenuState, OptionsState,
        Resolution, SaveEntry, Screen,
    },
};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::audio::AudioRuntime;
use crate::frame_profile::{FrameProfile, FrameSample, LiveFrameProfile};

/// The offscreen capture size, in pixels.
const CAPTURE_SIZE: (u32, u32) = (1280, 720);

/// The initial window size in physical pixels.
const INITIAL_SIZE: (u32, u32) = (1280, 720);

/// The fixed step headless capture advances the simulation by, so a capture
/// is reproducible regardless of how fast the host renders it.
pub(crate) const CAPTURE_STEP: f32 = 1.0 / 60.0;

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
    // `ohl_engine::EngineError::message` is already a fixed, sanitized
    // reason (never media-derived), so this reports the specific step that
    // failed (e.g. "the renderer could not be created") instead of the one
    // generic "the frame could not be rendered" every failure used to
    // collapse into, which made a degenerate/unbuildable world
    // indistinguishable from a missing adapter or a lost device.
    .map_err(|error| error.message())
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
    /// Runs completed-frame offscreen measurements for this many seconds.
    pub benchmark_seconds: Option<u32>,
    /// Reports CPU frame stages in the interactive window.
    pub profile_frames: bool,
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
    /// Opens the player-facing main menu before simulation begins.
    pub start_in_menu: bool,
    /// Starts a local skirmish on `map` (`--skirmish`); an empty `map`
    /// picks the [`Self::arena`]th deathmatch map the payload publishes.
    pub skirmish: Option<ohl_engine::SkirmishConfig>,
    /// Which deathmatch map an empty-`map` skirmish plays, counting from 1
    /// in map-name order (`--arena`); `None` is the first.
    pub arena: Option<usize>,
    /// Places the capture viewpoint this many units from the nearest
    /// spawned monster instead of at the map's player start or a caller
    /// chosen viewpoint (`--viewpoint-at-nearest-monster`, `dev-tools`
    /// only). Ignored without `headless_screenshot`.
    #[cfg(feature = "dev-tools")]
    pub viewpoint_at_nearest_monster: Option<f32>,
    /// Runs the bounded reachability/route-triage walk
    /// (`--reachability-report`, `dev-tools` only) instead of the
    /// interactive window, a capture, or a script, and prints its report.
    ///
    /// Combined with `chain_script` it runs *after* the chain instead, so
    /// the walk starts from wherever the chain's last route left the
    /// player — the arrival point of a level change, which is the one
    /// place a cold `--map <name>` load can never put the walk.
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
    /// Where `--plan-route` (`dev-tools` only) writes the route file it
    /// planned and validated. `None` leaves the planner unrun.
    #[cfg(feature = "dev-tools")]
    pub plan_route: Option<&'a Path>,
    /// The classname `--plan-goal` plans a route to instead of
    /// `trigger_changelevel` (`dev-tools` only). Ignored without
    /// `plan_route`.
    #[cfg(feature = "dev-tools")]
    pub plan_goal: Option<&'a str>,
    /// How many plan/replay attempts `--plan-attempts` allows
    /// (`dev-tools` only). `None` keeps
    /// `crate::route_planner::DEFAULT_ATTEMPTS`. Ignored without
    /// `plan_route`.
    #[cfg(feature = "dev-tools")]
    pub plan_attempts: Option<usize>,

    /// Development only: how many of one plan's travelling segments each
    /// attempt commits (`--plan-segments`). Ignored without
    /// `plan_route`.
    #[cfg(feature = "dev-tools")]
    pub plan_segments: Option<usize>,
}

/// The fixed failure `--arena N` reports when the payload publishes fewer
/// than `N` deathmatch maps; `cargo xtask skirmish-smoke` stops at it.
pub const NO_SUCH_ARENA: &str = "the payload publishes fewer deathmatch maps than --arena asks for";

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
fn load_initial_game(
    source: &AssetFsSource,
    args: &GameArgs<'_>,
    map: &str,
    config: GameConfig,
) -> Result<Option<Game>, &'static str> {
    if let Some(skirmish) = args.skirmish.as_ref() {
        // Neither the map name nor the reason is media-derived text: the
        // name stays out of the line, and the reason names the step.
        return Game::load_skirmish(source, map, &config, skirmish)
            .map(Some)
            .map_err(|error| match error {
                ohl_engine::EngineError::NoSpawnPoints => {
                    "the map has no player spawn point to start a skirmish on"
                }
                _ => "the skirmish map could not be loaded from the payload",
            });
    }
    if let Some(name) = args.load_slot {
        let slot = save_slot_dir().ok_or("no per-user save directory is available")?;
        // Neither the slot name nor the saved map name is logged: one is
        // user-supplied, the other media-derived.
        return Game::load_slot_with(source, &slot, name, &config)
            .map(Some)
            .map_err(|_| "the save slot could not be loaded");
    }

    match Game::load_with(source, map, &config) {
        Ok(game) => Ok(Some(game)),
        // ISO-only launch doubles as the import command. A valid medium can
        // publish a payload that is not a playable Half-Life installation
        // (the installed-worker integration fixture is one such payload).
        // Preserve the successful import result in that case; only a payload
        // with a loadable campaign start proceeds to the player-facing menu.
        Err(_) if args.start_in_menu => Ok(None),
        // The map name is media-derived, so the reason names the step, not
        // the asset.
        Err(_) => Err("the start map could not be loaded from the payload"),
    }
}

/// Logs whether the loaded map has a real entity world to play in (see
/// [`ENTITY_WORLD_OK_LINE`]), or, for a skirmish, that the match started.
fn log_entity_world(game: &Game, args: &GameArgs<'_>) {
    // A map reached only through a `trigger_changelevel`/`info_landmark`
    // pair legitimately declares no `info_player_start` of its own (the
    // player arrives relative to the landmark), so a landmark counts as
    // evidence of a real, loaded entity world just as a player start does.
    // What is never legitimate is a map with neither — nor one with no
    // entity definitions at all. A deathmatch arena spawns from its own
    // `info_player_deathmatch` points instead, which the skirmish start
    // has already checked for.
    if let Some(skirmish) = args.skirmish.as_ref() {
        tracing::info!(bots = skirmish.bots, "Skirmish started.");
    } else if game.entity_def_count() > 0 && (game.has_player_start() || game.has_landmark()) {
        tracing::info!("{ENTITY_WORLD_OK_LINE}");
    } else {
        // A map that loads with no entity definitions, or with none the
        // spawn resolver recognised as a player start, is an empty room:
        // the player is placed at the world origin, which is normally
        // inside solid geometry. Before the entities lump's own failure was
        // surfaced as a load error this was completely silent.
        tracing::warn!("{ENTITY_WORLD_EMPTY_LINE}");
    }
}

/// The map a skirmish with no `--map` plays: the `arena`th (by default the
/// first) deathmatch map the payload publishes, in map-name order. The
/// chosen name is never logged.
fn arena_map(source: &AssetFsSource, arena: Option<usize>) -> Result<String, &'static str> {
    let names = crate::skirmish::published_map_names(source.asset_fs());
    let arenas = crate::skirmish::deathmatch_maps(source, &names);
    if arenas.is_empty() {
        return Err("the payload publishes no deathmatch map to start a skirmish on");
    }
    arenas
        .into_iter()
        .nth(arena.unwrap_or(1).saturating_sub(1))
        .ok_or(NO_SUCH_ARENA)
}

pub fn run(args: &GameArgs<'_>) -> Result<(), &'static str> {
    let root = game_root(args.payload_files);
    let asset_fs = ohl_assets::AssetFs::mount_default(&root)
        .map_err(|_| "the payload directory could not be indexed")?;
    let source = AssetFsSource::new(asset_fs);
    let config = GameConfig {
        difficulty: args.difficulty,
        overbright: args.overbright,
    };
    let discovered;
    let map = if args.skirmish.is_some() && args.map.is_empty() {
        discovered = arena_map(&source, args.arena)?;
        discovered.as_str()
    } else {
        args.map
    };
    let Some(mut game) = load_initial_game(&source, args, map, config)? else {
        return Ok(());
    };
    tracing::info!("Map loaded.");
    log_entity_world(&game, args);
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
    if let Some(path) = args.plan_route
        && args.chain_script.is_empty()
    {
        return run_route_planner(&mut game, &source, args, path, &[]);
    }

    #[cfg(feature = "dev-tools")]
    if args.reachability_report && args.chain_script.is_empty() {
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
        let (visited, arrived_cleanly) = run_chained(&mut game, &source, args, args.chain_script)?;
        // `compute_reachability_report` walks out from the player's
        // *current* origin, so running it here reports the map the chain
        // ended in, from the point the chain left the player standing —
        // the arrival-point triage a cold `--map <name>` load cannot do.
        // The route planner starts from that same point, for the same
        // reason: a route for the map the chain ended in can only be
        // authored from where the chain left the player.
        #[cfg(feature = "dev-tools")]
        {
            if args.reachability_report {
                run_reachability_report(
                    &mut game,
                    args.reachability_assume_armed,
                    args.reachability_assume_longjump,
                    args.reachability_assume_pendulum_wait,
                    args.reachability_cell_cap,
                    args.reachability_round_cap,
                );
            }
            if let Some(path) = args.plan_route {
                if !arrived_cleanly {
                    return Err(PLAN_REFUSED_INCOMPLETE_CHAIN);
                }
                return run_route_planner(&mut game, &source, args, path, &visited);
            }
        }
        #[cfg(not(feature = "dev-tools"))]
        drop((visited, arrived_cleanly));
        return Ok(());
    }

    if let Some(script_path) = args.script {
        return run_scripted(&mut game, &source, args, script_path);
    }

    if let Some(seconds) = args.benchmark_seconds {
        return benchmark(&mut game, &source, seconds);
    }

    match args.screenshot {
        Some(path) => capture(&mut game, &source, args, path),
        None => windowed(game, &source, args),
    }
}

fn log_profile_device(context: &GpuContext, width: u32, height: u32) {
    let info = context.adapter.get_info();
    tracing::info!(
        adapter = info.name,
        backend = ?info.backend,
        width,
        height,
        "frame profile device"
    );
}

/// Measures fully completed frames without presentation/vsync or readback.
/// The ordinary simulation advances by one fixed tick per rendered frame;
/// level changes and death end the run instead of changing its workload.
///
/// Sound is part of that workload: every cue is resolved, decoded and
/// mixed into a silent sink exactly as the windowed loop does it, and the
/// time it takes is counted as simulation, which is where
/// [`App::tick_game`] spends it too.
fn benchmark(game: &mut Game, source: &dyn AssetSource, seconds: u32) -> Result<(), &'static str> {
    let context = GpuContext::headless().map_err(|_| "no usable graphics adapter is available")?;
    let (width, height) = CAPTURE_SIZE;
    let target = OffscreenTarget::new(&context, width, height)
        .map_err(|_| "no offscreen target could be created")?;
    log_profile_device(&context, width, height);
    tracing::info!("Benchmark warming up for five seconds.");
    // Silent on every platform: a benchmark is a measurement, like a
    // scripted run (see `run_scripted`).
    let mut audio = AudioRuntime::silent();
    benchmark_frames(
        game,
        source,
        &mut audio,
        &BenchmarkWindow {
            warmup: Duration::from_secs(5),
            seconds: u64::from(seconds),
        },
        |game| {
            let render_start = Instant::now();
            render_capture(
                game,
                &context,
                RenderTarget {
                    view: target.view(),
                    width,
                    height,
                    format: OFFSCREEN_FORMAT,
                },
                &CapturePose::None,
            )?;
            let render = render_start.elapsed();
            let wait_start = Instant::now();
            context.wait();
            Ok((render, wait_start.elapsed()))
        },
    )
}

/// How long [`benchmark_frames`] warms up for, and then measures.
struct BenchmarkWindow {
    /// Frames run, and not measured, before measurement starts.
    warmup: Duration,
    /// How many whole seconds are measured.
    seconds: u64,
}

/// The benchmark's own frame loop: one simulation tick (sound included,
/// through [`route_benchmark_events`]) and one call to `render` per frame,
/// until `window` has been measured or a level change or a death ends the
/// run.
///
/// `render` is the GPU half — draw one frame, wait for it — returning how
/// long the drawing and the wait took. [`benchmark`] hands in the real
/// one; a test hands in one that draws nothing, so the loop that routes
/// every cue is exercised without a GPU.
fn benchmark_frames(
    game: &mut Game,
    source: &dyn AssetSource,
    audio: &mut AudioRuntime,
    window: &BenchmarkWindow,
    mut render: impl FnMut(&mut Game) -> Result<(Duration, Duration), &'static str>,
) -> Result<(), &'static str> {
    let warmup_start = Instant::now();
    let mut measurement_start = None;
    let mut warmup_resources = ohl_engine::RenderResourceStats::default();
    let mut profile = FrameProfile::default();
    loop {
        let frame_start = Instant::now();
        audio.set_listener(game.eye_position(), game.camera().yaw);
        let events = game.tick(CAPTURE_STEP, &Input::default());
        if route_benchmark_events(audio, source, events) {
            tracing::warn!(
                "Benchmark stopped because the level changed, the player died or the section ended."
            );
            return Ok(());
        }
        audio.frame(CAPTURE_STEP);
        let simulation = frame_start.elapsed();
        let (render, gpu_wait) = render(game)?;
        let completed = Instant::now();
        if let Some(start) = measurement_start {
            profile.record(FrameSample {
                frame: completed.duration_since(frame_start),
                simulation,
                render,
                gpu_wait,
                ..FrameSample::default()
            });
            let elapsed = completed.duration_since(start);
            if elapsed >= Duration::from_secs(window.seconds) {
                if let Some(summary) = profile.finish(elapsed) {
                    summary.log("headless_completed");
                }
                log_resource_uploads(game, warmup_resources);
                return Ok(());
            }
        } else if completed.duration_since(warmup_start) >= window.warmup {
            let seconds = window.seconds;
            tracing::info!(seconds, "Benchmark measurement started.");
            warmup_resources = game.render_resource_stats();
            measurement_start = Some(Instant::now());
        }
    }
}

/// Plays every sound one benchmark tick produced, and reports whether the
/// tick also ended the benchmark (a level change, the player's death, or a
/// `trigger_endsection` ending the game: each would change the workload
/// being measured, so none is followed). Sounds listed after the event
/// that ended the run are dropped with it.
fn route_benchmark_events(
    audio: &mut AudioRuntime,
    source: &dyn AssetSource,
    events: Vec<GameEvent>,
) -> bool {
    let mut ends_the_run = false;
    for event in events {
        match event {
            GameEvent::Sound(cue) => {
                if !ends_the_run {
                    audio.play(source, &cue);
                }
            }
            GameEvent::LevelChange { .. } | GameEvent::PlayerDied => ends_the_run = true,
            GameEvent::EndSection => {
                tracing::info!("{SECTION_ENDED}");
                ends_the_run = true;
            }
            GameEvent::ChapterTitle(_)
            | GameEvent::Message { .. }
            | GameEvent::Suit(_)
            | GameEvent::ViewModel(_)
            | GameEvent::Frag { .. }
            | GameEvent::MatchOver { .. }
            | GameEvent::PlayerRespawned => {}
        }
    }
    ends_the_run
}

/// Resource work since the supplied checkpoint; only enabled by profiling.
fn log_resource_uploads(game: &Game, previous: ohl_engine::RenderResourceStats) {
    let current = game.render_resource_stats();
    tracing::info!(
        brush_preparations = current
            .submodels
            .preparations
            .saturating_sub(previous.submodels.preparations),
        brush_static_upload_bytes = current
            .submodels
            .static_upload_bytes
            .saturating_sub(previous.submodels.static_upload_bytes),
        brush_texture_uploads = current
            .submodels
            .texture_uploads
            .saturating_sub(previous.submodels.texture_uploads),
        world_lightmap_uploads = current
            .lightmap_uploads
            .saturating_sub(previous.lightmap_uploads),
        "profile resource uploads"
    );
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
    source: &dyn AssetSource,
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
    // Silent on every platform, not merely on the ones with no device: a
    // scripted run is a reproducible measurement, and it must not make a
    // noise on the machine it runs on. The mixer still runs, so a cue that
    // cannot be resolved is still a cue that cannot be resolved here.
    let mut audio = AudioRuntime::silent();
    run_script_ticks(
        game,
        source,
        &mut audio,
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
    if let Some(status) = game.skirmish_status() {
        crate::skirmish::log_summary(&status);
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
    /// A `trigger_endsection` fired, so the route stopped where it stood
    /// rather than running its remaining ticks. See [`SECTION_ENDED`].
    ended_section: bool,
    /// How many simulation ticks actually ran. Fewer than the script
    /// scheduled when [`TickOptions::stop_on_level_change`] cut the route
    /// short.
    ticks: u64,
}

/// Ticks one parsed script through [`Game::tick`] at [`CAPTURE_STEP`],
/// handling the events it produces through [`route_headless_events`].
/// Shared by [`run_scripted`] (one script, run to its end) and
/// [`run_chained`] (one script per map, each ending at the level change
/// that carries the player into the next one). `audio` is the caller's, so
/// a chain walk's sounds outlive one route the way its game does.
fn run_script_ticks(
    game: &mut Game,
    source: &dyn AssetSource,
    audio: &mut AudioRuntime,
    script: &crate::script::Script,
    log: &mut crate::script_log::ScriptLog,
    options: &TickOptions,
) -> TickOutcome {
    let mut outcome = TickOutcome {
        followed_level_change: false,
        ended_section: false,
        ticks: 0,
    };
    let mut input_state = crate::script::ScriptInputState::default();
    for step in script.steps() {
        let input = input_state.resolve(game, step);
        audio.set_listener(game.eye_position(), game.camera().yaw);
        let events = game.tick(CAPTURE_STEP, &input);
        let routed = route_headless_events(
            game,
            source,
            audio,
            events,
            &HeadlessEventOptions {
                follow_level_change: options.follow_level_change,
                script_log: options.script_log,
                // The same fixed line the interactive window logs
                // (`App::handle_game_events`, below): a scripted/headless
                // run is otherwise silent about the player's death, even
                // though `ohl_engine::Systems::step` has already stopped
                // simulating the player's movement from this point on.
                player_died_line: "The player died.",
            },
        );
        if routed.followed_level_change {
            input_state.followed_level_change();
        }
        outcome.followed_level_change |= routed.followed_level_change;
        outcome.ended_section |= routed.ended_section;
        audio.frame(CAPTURE_STEP);
        outcome.ticks += 1;
        if options.script_log {
            log.observe(game, CAPTURE_STEP);
        }
        if options.stop_on_level_change && outcome.followed_level_change {
            break;
        }
        // A section that has ended is a run that is over: nothing after
        // this tick belongs to it, whichever caller asked for the ticks.
        if outcome.ended_section {
            break;
        }
    }
    outcome
}

/// How [`route_headless_events`] treats the events it does not simply
/// play.
struct HeadlessEventOptions {
    /// Passed straight to [`handle_level_change`].
    follow_level_change: bool,
    /// Passed straight to [`handle_level_change`].
    script_log: bool,
    /// The fixed line logged when the player dies. A capture and a
    /// scripted run have always logged different ones, and the smokes
    /// read them.
    player_died_line: &'static str,
}

/// What [`route_headless_events`] did with one tick's events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HeadlessOutcome {
    /// A level change was followed onto its destination map.
    followed_level_change: bool,
    /// A `trigger_endsection` ended the run (see [`SECTION_ENDED`]).
    ended_section: bool,
}

/// Handles one tick's events for a run nobody is listening to: a scripted
/// run, one leg of a chain walk, or a still capture.
///
/// This is the one place those run paths turn a `GameEvent::Sound` into a
/// play request; [`App::handle_game_events`] does the same for the window,
/// and [`route_benchmark_events`] for a benchmark. A followed level change
/// stops everything `audio` is playing, and drops the tick's remaining
/// sounds, since the map being left must not keep humming under the one
/// arriving, which announces its own soundscape from its first tick.
///
/// A `trigger_endsection` ends the run: the fixed [`SECTION_ENDED`] line is
/// logged, everything `audio` is playing stops, and the rest of the tick's
/// events are dropped — a level change listed after it must not load a
/// map behind a game that is over. The caller stops ticking.
fn route_headless_events(
    game: &mut Game,
    source: &dyn AssetSource,
    audio: &mut AudioRuntime,
    events: Vec<GameEvent>,
    options: &HeadlessEventOptions,
) -> HeadlessOutcome {
    let mut followed_level_change = false;
    for event in events {
        match event {
            // Once a level change has been followed, the rest of this
            // tick's sounds were produced on the map that was left (see
            // `App::handle_game_events`) and are dropped with it.
            GameEvent::Sound(cue) => {
                if !followed_level_change {
                    audio.play(source, &cue);
                }
            }
            GameEvent::LevelChange { map, landmark } => {
                let followed = handle_level_change(
                    game,
                    source,
                    &map,
                    &landmark,
                    options.follow_level_change,
                    options.script_log,
                );
                if followed {
                    audio.stop_all();
                }
                followed_level_change |= followed;
            }
            GameEvent::PlayerDied => {
                tracing::info!("{}", options.player_died_line);
            }
            GameEvent::EndSection => {
                tracing::info!("{SECTION_ENDED}");
                audio.stop_all();
                return HeadlessOutcome {
                    followed_level_change,
                    ended_section: true,
                };
            }
            GameEvent::MatchOver { .. } => tracing::info!("The skirmish match ended."),
            GameEvent::PlayerRespawned => tracing::info!("The player respawned."),
            // Map-authored text and presentation events with nothing to
            // act on in a run nobody watches (M7.9 P1): none of these are
            // logged. A skirmish's frags are summed up once, at the end
            // of the run (`crate::skirmish::log_summary`).
            GameEvent::ChapterTitle(_)
            | GameEvent::Message { .. }
            | GameEvent::Suit(_)
            | GameEvent::ViewModel(_)
            | GameEvent::Frag { .. } => {}
        }
    }
    HeadlessOutcome {
        followed_level_change,
        ended_section: false,
    }
}

/// The fixed line every run logs when a `trigger_endsection` fires.
///
/// TWHL's `trigger_endsection` page documents the entity as one that "ends the
/// current game and returns the player to the game's main menu"
/// (`docs/FORMAT_SOURCES.md`, "Map entities the registry used to drop"). The
/// interactive window does exactly that: it stops ticking the game and shows
/// [`Screen::MainMenu`]. A scripted or headless run has no menu to return to,
/// so there the section ending means the run is over: it stops where it stands,
/// and none of the script's remaining ticks run.
///
/// Name-free like every other line in this module: the entity's own
/// `section` keyvalue never leaves `ohl-engine` (see
/// `ohl_engine::GameEvent::EndSection`), so there is nothing map-derived
/// to leak here.
pub const SECTION_ENDED: &str = "The section ended.";

/// The fixed line a chain walk logs when one of its routes ran out of
/// scripted ticks without reaching a `trigger_changelevel`: the chain got
/// no further than the map that route ran on.
const CHAIN_STOPPED: &str = "The chain walk stopped.";

/// The fixed line a chain walk logs when a `trigger_endsection` ended the
/// game partway through a route (see [`SECTION_ENDED`]): the walk stops
/// there, and says so in its own words rather than as a route that merely
/// ran out of ticks ([`CHAIN_STOPPED`]). Not a failure in itself — a game
/// that ends where its section ends is the documented behaviour — but not
/// a clean arrival either, so `--plan-route` refuses to plan from it.
const CHAIN_SECTION_ENDED: &str = "The chain walk ended its section.";

/// The fixed line a chain walk logs when every route it was given did
/// reach a level change, so the walk ended only because no route was
/// authored for the map it last arrived in. Not a failure.
const CHAIN_NO_FURTHER_ROUTE: &str = "The chain walk has no further route.";

/// The fixed line a chain walk logs when a route's level change landed it
/// back in a map the chain had already entered — most often by walking
/// straight back into the boundary it just arrived through, which is a
/// route-authoring mistake rather than progress. A re-entry ends the chain
/// as a failure: a walk that may revisit maps could satisfy any depth
/// requirement by ping-ponging across a single boundary, which would make
/// the depth aggregate worthless as a progress metric.
///
/// Name-free like every other line here: it reports *that* a map repeated,
/// never which one.
const CHAIN_RE_ENTERED: &str = "The chain walk re-entered a map it had already visited.";

/// The fixed line a chain walk logs when a route's level change was
/// followed with the player already dead.
///
/// A map may fire its own `trigger_changelevel` by name rather than wait
/// for the player to walk into it (`ohl_engine::route_plan`'s scripted
/// goals), and a chain of `multi_manager`s does that whether or not the
/// player who set it going survived to see it. Counting such a hop as
/// progress would let the depth aggregate grow off a corpse being carried
/// across a boundary, so a dead arrival ends the chain as a failure — the
/// same shape a re-entry ([`CHAIN_RE_ENTERED`]) already has, and for the
/// same reason: the aggregate has to stay worth something.
///
/// Name-free like every other line here: it reports *that* the walk
/// arrived dead, never where.
const CHAIN_ARRIVED_DEAD: &str = "The chain walk arrived dead.";

/// The fixed line `--plan-route` logs, and the fixed error [`run`] returns,
/// instead of planning after a chain that stopped short
/// ([`CHAIN_STOPPED`]) or re-entered a map ([`CHAIN_RE_ENTERED`]).
///
/// Either outcome leaves the player wherever the interrupted route
/// happened to stall or double back, not at the destination map's own
/// clean arrival point — the state [`run_route_planner`] is documented to
/// require. Planning from it anyway would silently hand back a route
/// keyed to the wrong hop; refusing outright, with a file never written,
/// is cheap to check against the chain's own arrival state
/// ([`run_chained`]'s second return value).
#[cfg(feature = "dev-tools")]
const PLAN_REFUSED_INCOMPLETE_CHAIN: &str =
    "Route plan refused: the chain did not arrive cleanly, so nothing was planned.";

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
/// The reported depth counts *distinct* maps entered, and a level change
/// back into a map the chain has already been in ends the walk as a
/// failure ([`CHAIN_RE_ENTERED`]). Counting entries instead would let a
/// route that simply walks back into the boundary it arrived through
/// report unbounded "progress"; the visited set below is kept in memory
/// only and never reaches a log line, the same as every other map name in
/// this module.
///
/// Logs, beyond the per-hop "A level change was followed." line
/// [`handle_level_change`] already emits: exactly one of the three fixed
/// terminal lines above, plus the walk's own bounded aggregates (how many
/// distinct maps deep it got and how many simulated seconds that took). No
/// map name, entity name or position is logged, here or anywhere below.
fn run_chained(
    game: &mut Game,
    source: &AssetFsSource,
    args: &GameArgs<'_>,
    routes: &[PathBuf],
) -> Result<(Vec<String>, bool), &'static str> {
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

    // Every map this chain has entered, lowercased for comparison. Held in
    // memory to detect a re-entry and to count distinct maps; never
    // logged, never written anywhere.
    let mut visited: Vec<String> = vec![game.map().to_ascii_lowercase()];
    let mut ticks: u64 = 0;
    let mut stopped = false;
    let mut section_ended = false;
    let mut re_entered = false;
    let mut arrived_dead = false;
    // One silent runtime for the whole walk, as `run_scripted` has one for
    // its one route: the level change between two routes is what stops
    // the sounds of the map being left, not the end of a route.
    let mut audio = AudioRuntime::silent();
    for script in &scripts {
        // A fresh log per route, so every milestone line is observed from
        // this map's own arrival point rather than from the chain's start.
        let mut log = crate::script_log::ScriptLog::new(game);
        let outcome = run_script_ticks(
            game,
            source,
            &mut audio,
            script,
            &mut log,
            &TickOptions {
                script_log: args.script_log,
                follow_level_change: true,
                stop_on_level_change: true,
            },
        );
        ticks += outcome.ticks;
        if outcome.ended_section {
            section_ended = true;
            break;
        }
        if !outcome.followed_level_change {
            stopped = true;
            break;
        }
        if game.player_health() <= 0.0 {
            arrived_dead = true;
            break;
        }
        let arrived = game.map().to_ascii_lowercase();
        if visited.contains(&arrived) {
            re_entered = true;
            break;
        }
        visited.push(arrived);
        log_arrival_inventory(game, visited.len());
    }

    if args.script_log {
        tracing::info!("Scripted input finished.");
    }
    if arrived_dead {
        tracing::info!("{CHAIN_ARRIVED_DEAD}");
    } else if re_entered {
        tracing::info!("{CHAIN_RE_ENTERED}");
    } else if section_ended {
        tracing::info!("{CHAIN_SECTION_ENDED}");
    } else if stopped {
        tracing::info!("{CHAIN_STOPPED}");
    } else {
        tracing::info!("{CHAIN_NO_FURTHER_ROUTE}");
    }
    // Two bounded aggregates over project-authored routes (how many
    // distinct maps the chain entered, and how much simulated time the
    // routes it ran took), in the fixed shapes `xtask/src/chain_walk.rs`
    // parses. Neither is a media-derived name, path or content figure.
    tracing::info!("Chain walk depth: {}.", visited.len());
    #[allow(clippy::cast_precision_loss, reason = "a tick count for a report line")]
    let seconds = ticks as f32 * CAPTURE_STEP;
    tracing::info!("Chain walk simulated seconds: {seconds:.1}.");
    // The visited list itself is returned, never logged: `--plan-route`
    // uses it to avoid planning a route straight back through the
    // boundary the chain just arrived through (see
    // `ohl_engine::PlanConfig::avoid_goal_maps`). The second value is
    // whether the chain arrived cleanly (neither stopped short nor
    // re-entered a map): `--plan-route` refuses to plan at all when it
    // did not, rather than plan from an interrupted route's stall point
    // (see `PLAN_REFUSED_INCOMPLETE_CHAIN`).
    Ok((
        visited,
        !stopped && !section_ended && !re_entered && !arrived_dead,
    ))
}

/// The fixed prefix a chain walk's per-arrival inventory line carries.
/// `xtask/src/chain_walk.rs` parses these into one summary row per
/// arrival, with its own copy of this prefix; a test there reads this
/// definition and the line [`log_arrival_inventory`] formats, so the two
/// cannot drift apart unnoticed.
const CHAIN_ARRIVAL_PREFIX: &str = "Chain walk arrival ";

/// Logs what the player is carrying on arriving in the `index`-th map of
/// a chain walk, as two counts and nothing else: how many weapons are
/// owned, and how many rounds of every kind are carried altogether, in
/// reserve and loaded in a clip ([`Game::inventory_totals`]) — so a
/// reload between two arrivals does not read as rounds spent.
///
/// This is the measurement the chain's own routes are judged by — a walk
/// that never stops for anything arrives everywhere empty-handed — and it
/// is an aggregate over project-authored routes, not a media-derived
/// name, path or content figure. No weapon or ammo *name* is logged: a
/// count cannot say which map handed out what.
fn log_arrival_inventory(game: &Game, index: usize) {
    let (weapons, ammo) = game.inventory_totals();
    tracing::info!("{CHAIN_ARRIVAL_PREFIX}{index}: weapons {weapons}, ammo {ammo}.");
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

/// Development only: plans, validates and writes a route file
/// (`--plan-route`), and reports what it took.
///
/// Every line printed here is a fixed string or a bounded aggregate — how
/// many grid cells the search reached, how many walk-forward segments and
/// door presses the route holds, how many plan/replay attempts it took,
/// and how many simulated seconds it runs for. Never a map name, a
/// coordinate or a targetname (`docs/CLEAN_ROOM.md`); the route file
/// itself holds script commands and project-authored comment words only.
#[cfg(feature = "dev-tools")]
fn run_route_planner(
    game: &mut Game,
    source: &AssetFsSource,
    args: &GameArgs<'_>,
    path: &Path,
    visited: &[String],
) -> Result<(), &'static str> {
    let default_config = ohl_engine::ReachabilityConfig::default();
    let options = crate::route_planner::PlanOptions {
        attempts: args
            .plan_attempts
            .unwrap_or(crate::route_planner::DEFAULT_ATTEMPTS),
        settle_rounds: crate::route_planner::DEFAULT_SETTLE_ROUNDS,
        segments_per_attempt: args
            .plan_segments
            .unwrap_or(crate::route_planner::DEFAULT_SEGMENTS_PER_ATTEMPT),
        plan: ohl_engine::PlanConfig {
            cell_cap: args
                .reachability_cell_cap
                .unwrap_or(default_config.cell_cap),
            max_rounds: args
                .reachability_round_cap
                .unwrap_or(default_config.max_rounds),
            goal_classname: args
                .plan_goal
                .unwrap_or(ohl_engine::route_plan::DEFAULT_GOAL_CLASSNAME)
                .to_string(),
            // A map the chain has already been in is not somewhere a new
            // route should lead: walking back through the boundary just
            // arrived through is how a chain walk fails, not how it gets
            // deeper (`xtask/src/chain_walk.rs`). Held in memory only,
            // never logged, exactly like `run_chained`'s own copy.
            avoid_goal_maps: visited.to_vec(),
            assume_longjump: args.reachability_assume_longjump,
            // The default: never plan a fall the player does not walk
            // away from unhurt (`ohl_engine::route_plan::safe_drop_height`).
            max_drop: None,
            // The defaults: a route steps aside for a handful of the
            // pickups the map left beside it, so the chain arrives in the
            // next map carrying what this one offered rather than
            // whatever the harness was told to hand out
            // (`ohl_engine::route_plan`'s pickup detours).
            max_pickup_detours: ohl_engine::route_plan::DEFAULT_MAX_PICKUP_DETOURS,
            pickup_detour_budget: ohl_engine::route_plan::DEFAULT_PICKUP_DETOUR_BUDGET,
        },
    };

    tracing::info!("Route planner: planning.");
    let route = match crate::route_planner::plan(game, source, &options) {
        Ok(route) => route,
        Err(failure) => {
            // A fixed, self-describing reason; none of them carries
            // anything map-derived.
            tracing::error!("Route planner: {failure}.");
            return Err(ROUTE_PLAN_FAILED);
        }
    };
    tracing::info!("Route plan cells: {}.", route.cells);
    tracing::info!("Route plan segments: {}.", route.segments);
    tracing::info!("Route plan ladder climbs: {}.", route.climbs);
    tracing::info!("Route plan pickup detours: {}.", route.pickups);
    tracing::info!("Route plan door presses: {}.", route.doors);
    tracing::info!("Route plan replay attempts: {}.", route.attempts);
    tracing::info!("Route plan simulated seconds: {:.1}.", route.seconds());
    if crate::route_planner::write_route(path, &route).is_err() {
        return Err("the planned route file could not be written");
    }
    tracing::info!("{ROUTE_PLAN_WRITTEN}");
    Ok(())
}

/// The fixed error a failed plan ends the run with. The reason itself is
/// logged separately (and is equally fixed); this is what the process
/// exits on.
#[cfg(feature = "dev-tools")]
const ROUTE_PLAN_FAILED: &str = "no route to the goal could be planned and validated";

/// The fixed line a written route ends the planner with, in the shape
/// `xtask/src/plan_chain_hop.rs` looks for.
#[cfg(feature = "dev-tools")]
pub const ROUTE_PLAN_WRITTEN: &str = "Route plan written.";

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

    // A capture writes a picture, never a sound: silent on every platform
    // (see `run_scripted`'s own note). The mixer still runs so a
    // capture exercises exactly the same cue-resolution path a windowed
    // run does.
    let mut audio = AudioRuntime::silent();

    for _ in 0..args.frames.max(1) {
        // The capture stands still (aside from a rider pose following the
        // player): only the world's own animation (doors, light styles,
        // liquid turbulence, model sequences) advances.
        audio.set_listener(game.eye_position(), game.camera().yaw);
        let events = game.tick(CAPTURE_STEP, &Input::default());
        let routed = route_headless_events(
            game,
            source,
            &mut audio,
            events,
            &HeadlessEventOptions {
                follow_level_change: args.follow_level_change,
                script_log: args.script_log,
                player_died_line: "The player died during capture.",
            },
        );
        audio.frame(CAPTURE_STEP);
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
        // The section ended this tick: this frame is the last one the run
        // has, and the capture keeps it rather than ticking a game that is
        // over (see [`SECTION_ENDED`]).
        if routed.ended_section {
            break;
        }
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

/// The level-select pane's chapters: the Hazard Course, then every chapter
/// `ohl-campaign` lists, each level marked by whether `published` (the
/// payload's sorted bare map names) holds it.
fn menu_chapters(published: &[String]) -> Vec<ChapterEntry> {
    let entry = |title: &'static str, maps: &'static [&'static str]| ChapterEntry {
        title,
        levels: maps
            .iter()
            .map(|&map| LevelEntry {
                map,
                available: map_published(published, map),
            })
            .collect(),
    };
    std::iter::once(entry(
        "Hazard Course",
        ohl_campaign::chapters::HAZARD_COURSE_MAPS,
    ))
    .chain(
        ohl_campaign::CHAPTERS
            .iter()
            .map(|chapter| entry(chapter.title, chapter.maps)),
    )
    .collect()
}

/// Whether `map` is among `published` (sorted, lower-case bare names, as
/// `crate::skirmish::published_map_names` lists them). An empty list means
/// the host does not know, so every map counts as published.
fn map_published(published: &[String], map: &str) -> bool {
    let map = map.to_ascii_lowercase();
    published.is_empty() || published.binary_search(&map).is_ok()
}

/// How long ago something happened, for the load pane.
fn age_text(seconds: u64) -> String {
    match seconds {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        86_400..172_800 => "yesterday".to_owned(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// The load pane's list: every save in `slot`, newest first. The chapter
/// and map names come from the user's own save files and are only shown,
/// never logged.
fn save_entries(slot: &ohl_save::SaveSlot, now: u64) -> Vec<SaveEntry> {
    let mut listed = slot.list(&ohl_save::Limits::default()).unwrap_or_default();
    listed.sort_by(|a, b| {
        b.header
            .created_at_unix_secs
            .cmp(&a.header.created_at_unix_secs)
            .then_with(|| a.name.cmp(&b.name))
    });
    listed
        .into_iter()
        .map(|listing| {
            let kind = match listing.name.as_str() {
                ohl_save::QUICKSAVE_SLOT_NAME => "Quick save",
                ohl_save::AUTOSAVE_SLOT_NAME => "Autosave",
                other => other,
            };
            let age = age_text(now.saturating_sub(listing.header.created_at_unix_secs));
            SaveEntry {
                title: listing.header.title,
                detail: format!("{kind}  ·  {}  ·  {age}", listing.header.map_identity),
                slot: listing.name,
            }
        })
        .collect()
}

/// Window sizes the video options offer beside whatever the monitor
/// itself reports.
const COMMON_RESOLUTIONS: [(u32, u32); 9] = [
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1366, 768),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];

/// The resolutions the video options offer for `window`'s monitor,
/// largest first: its own video modes plus the common sizes that fit it.
fn display_resolutions(window: &Window) -> Vec<Resolution> {
    let monitor = window.current_monitor();
    let largest = monitor
        .as_ref()
        .map(winit::monitor::MonitorHandle::size)
        .filter(|size| size.width > 0 && size.height > 0);
    let mut list: Vec<Resolution> = monitor
        .iter()
        .flat_map(winit::monitor::MonitorHandle::video_modes)
        .map(|mode| Resolution::new(mode.size().width, mode.size().height))
        .chain(
            COMMON_RESOLUTIONS
                .iter()
                .map(|&(width, height)| Resolution::new(width, height))
                .filter(|resolution| {
                    largest.is_none_or(|max| {
                        resolution.width <= max.width && resolution.height <= max.height
                    })
                }),
        )
        .filter(|resolution| {
            (Resolution::MIN..=Resolution::MAX).contains(&resolution.width)
                && (Resolution::MIN..=Resolution::MAX).contains(&resolution.height)
        })
        .collect();
    list.sort_by(|a, b| b.cmp(a));
    list.dedup();
    list
}

/// Puts `window` in `display`'s mode. Exclusive fullscreen uses the
/// monitor's best video mode of the chosen size and falls back to
/// borderless when the monitor has none (or reports no modes at all).
fn apply_display(window: &Window, display: DisplaySettings) {
    let resolution = display.resolution;
    match display.mode {
        DisplayMode::Windowed => {
            window.set_fullscreen(None);
            let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
                resolution.width,
                resolution.height,
            ));
        }
        DisplayMode::Borderless => {
            window.set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
        }
        DisplayMode::Fullscreen => {
            let mode = window.current_monitor().and_then(|monitor| {
                monitor
                    .video_modes()
                    .filter(|mode| {
                        mode.size()
                            == winit::dpi::PhysicalSize::new(resolution.width, resolution.height)
                    })
                    .max_by_key(|mode| (mode.refresh_rate_millihertz(), mode.bit_depth()))
            });
            window.set_fullscreen(Some(match mode {
                Some(mode) => Fullscreen::Exclusive(mode),
                None => Fullscreen::Borderless(window.current_monitor()),
            }));
        }
    }
}

/// The engine's own mouse turn rate corresponds to this sensitivity, so
/// the default setting leaves mouselook exactly as it was.
const BASE_SENSITIVITY: f32 = 3.0;

/// Opens a window and runs the menu/game loop until the player quits.
#[allow(clippy::needless_pass_by_value)]
fn windowed(game: Game, source: &AssetFsSource, args: &GameArgs<'_>) -> Result<(), &'static str> {
    let event_loop = EventLoop::new().map_err(|_| "no window system is available")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let settings_path = crate::settings::default_path();
    let options = settings_path
        .as_deref()
        .map_or_else(OptionsState::default, crate::settings::load);
    let all_maps = crate::skirmish::published_map_names(source.asset_fs());
    let mut app = App {
        saves: save_slot_dir(),
        profile: args.profile_frames.then(FrameProfile::default),
        chapters: menu_chapters(&all_maps),
        all_maps,
        menu: MenuState::with_options(options),
        settings_path,
        ..App::new(
            game,
            source,
            // The one run path that actually wants to be heard. On Linux
            // this is still a `NullSink` (the recorded no-FFI decision); on
            // macOS/Windows it reaches CoreAudio/WASAPI, and on a machine
            // with no output device at all it falls back to silence rather
            // than failing the run.
            AudioRuntime::open(),
            if args.start_in_menu {
                Screen::MainMenu
            } else {
                Screen::InGame
            },
            GameConfig {
                difficulty: args.difficulty,
                overbright: args.overbright,
            },
        )
    };
    app.apply_options();
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
    adapter_name: String,
    backend_name: String,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "each is an independent window or key state, not related state a caller could \
confuse for one another"
)]
struct App<'a> {
    game: Game,
    /// Where every map, save and sound this window loads is read from: the
    /// payload's own files in a real run, an in-memory table in a test.
    source: &'a dyn AssetSource,
    /// The save directory quicksave/quickload and the level-change autosave
    /// use, when the platform publishes one.
    saves: Option<ohl_save::SaveSlot>,
    /// The output device, mixer and sound-asset cache. See `crate::audio`.
    audio: AudioRuntime,
    input: Input,
    /// Which bound actions are held down, by [`Action::index`], so the
    /// movement axes and the "use" press edge follow the keys themselves.
    held: [bool; Action::ALL.len()],
    console: Console,
    screen: Screen,
    /// The menu's navigation and every option the player has set.
    menu: MenuState,
    /// The pane the menu showed last frame, to notice it changing.
    last_pane: MenuPane,
    /// Where the options are saved, when the platform has a configuration
    /// directory; `None` in a test, which never touches the disk.
    settings_path: Option<PathBuf>,
    /// Whether the options changed since they were last saved.
    settings_dirty: bool,
    /// The level-select pane's chapters.
    chapters: Vec<ChapterEntry>,
    /// The load pane's saves, listed when it opens.
    save_entries: Vec<SaveEntry>,
    /// The resolutions the video options offer for the window's monitor.
    resolutions: Vec<Resolution>,
    /// Every map the payload publishes, by bare name; the skirmish pane's
    /// candidates. Empty in a test window.
    all_maps: Vec<String>,
    /// The deathmatch maps among [`Self::all_maps`], found the first time
    /// the skirmish pane opens (it reads every candidate's entity lump).
    skirmish_maps: Option<Vec<String>>,
    /// Whether the scoreboard key is held.
    scoreboard_held: bool,
    config: GameConfig,
    quit_requested: bool,
    live_profile: LiveFrameProfile,
    hud: HudState,
    state: Option<Active>,
    last_frame: Instant,
    /// When the next frame is due under the frame-rate cap. It advances by
    /// whole frame intervals, so the time it takes to wake up for a frame
    /// is not added to every frame's length.
    next_frame: Instant,
    fps_window_start: Instant,
    frames: u32,
    profile: Option<FrameProfile>,
    failure: Option<&'static str>,
}

/// Draws the HUD and, in a skirmish, the scoreboard: while `held` (its key,
/// in game), and always once the match is over.
fn draw_play_overlay(context: &ohl_ui::egui::Context, hud: &HudState, game: &Game, held: bool) {
    ohl_ui::hud::draw(context, hud);
    if let Some(status) = game.skirmish_status()
        && (held || status.winner.is_some())
    {
        ohl_ui::scoreboard::draw(context, &crate::skirmish::scoreboard(&status));
    }
}

fn draw_graphics_debug(
    active: &Active,
    profile: &LiveFrameProfile,
    now: Instant,
    game: &Game,
    width: u32,
    height: u32,
) {
    let timing = profile.summary(now);
    let resources = game.render_resource_stats();
    ohl_ui::debug::draw(
        active.ui.context(),
        &GraphicsDebugInfo {
            fps: timing.fps,
            one_percent_low_fps: timing.one_percent_low_fps,
            frame_ms: timing.frame_ms,
            simulation_ms: timing.simulation_ms,
            acquire_ms: timing.acquire_ms,
            render_ms: timing.render_ms,
            ui_present_ms: timing.ui_present_ms,
            width,
            height,
            adapter: &active.adapter_name,
            backend: &active.backend_name,
            static_upload_bytes: resources.submodels.static_upload_bytes,
            texture_uploads: resources.submodels.texture_uploads,
            lightmap_uploads: resources.lightmap_uploads,
        },
    );
}

impl<'a> App<'a> {
    /// A window-less app over `game`: no save directory, no frame
    /// profile, default options, no settings file, and no window until
    /// winit hands it one. `audio` is the caller's choice, so a test can
    /// drive the same loop silently.
    fn new(
        game: Game,
        source: &'a dyn AssetSource,
        audio: AudioRuntime,
        screen: Screen,
        config: GameConfig,
    ) -> Self {
        Self {
            game,
            source,
            saves: None,
            audio,
            input: Input::default(),
            held: [false; Action::ALL.len()],
            console: Console::new(),
            screen,
            menu: MenuState::new(),
            last_pane: MenuPane::Root,
            settings_path: None,
            settings_dirty: false,
            chapters: menu_chapters(&[]),
            save_entries: Vec::new(),
            resolutions: Vec::new(),
            all_maps: Vec::new(),
            skirmish_maps: None,
            scoreboard_held: false,
            config,
            quit_requested: false,
            live_profile: LiveFrameProfile::default(),
            hud: HudState::default(),
            state: None,
            last_frame: Instant::now(),
            next_frame: Instant::now(),
            fps_window_start: Instant::now(),
            frames: 0,
            profile: None,
            failure: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, message: &'static str) {
        self.failure = Some(message);
        event_loop.exit();
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.release_movement();
        let Some(active) = self.state.as_ref() else {
            return;
        };
        let capture = screen.input_capture();
        if capture.release_cursor {
            let _ = active.window.set_cursor_grab(CursorGrabMode::None);
        } else if active
            .window
            .set_cursor_grab(CursorGrabMode::Locked)
            .is_err()
        {
            let _ = active.window.set_cursor_grab(CursorGrabMode::Confined);
        }
        active.window.set_cursor_visible(capture.release_cursor);
    }

    /// Leaves the menu for a game that just started (or was loaded): the
    /// game being left stops sounding and its HUD is cleared.
    fn enter_new_game(&mut self, game: Game) {
        self.game = game;
        // As at a level change: the new game announces its own ambience
        // from its first tick, and nothing would ever stop a loop the old
        // one had started.
        self.audio.stop_all();
        self.hud = HudState::default();
        self.menu.reset();
        self.set_screen(Screen::InGame);
    }

    fn handle_menu_actions(&mut self, actions: Vec<MenuAction>) {
        for action in actions {
            match action {
                MenuAction::StartSinglePlayer { map, difficulty } => {
                    self.config.difficulty = match difficulty {
                        MenuDifficulty::Easy => ohl_campaign::Difficulty::Easy,
                        MenuDifficulty::Medium => ohl_campaign::Difficulty::Medium,
                        MenuDifficulty::Hard => ohl_campaign::Difficulty::Hard,
                    };
                    if let Ok(game) = Game::load_with(self.source, map, &self.config) {
                        self.enter_new_game(game);
                        tracing::info!("Single-player mission started.");
                    } else {
                        tracing::warn!("The selected mission could not be loaded.");
                        self.menu.notify("That level could not be loaded.");
                    }
                }
                MenuAction::LoadSave(slot) => self.load_save(&slot),
                MenuAction::Resume => self.set_screen(Screen::InGame),
                MenuAction::ReturnToMainMenu => self.return_to_main_menu(),
                MenuAction::Quit => self.quit_requested = true,
                MenuAction::SaveGame => {
                    if self.quicksave() {
                        self.menu.notify("Game saved.");
                    } else {
                        self.menu.notify("The game could not be saved.");
                    }
                }
                MenuAction::StartSkirmish {
                    map,
                    bots,
                    skill,
                    frag_limit,
                    time_limit_minutes,
                } => self.start_skirmish(&map, bots, skill, frag_limit, time_limit_minutes),
                // LAN play does not exist yet; the panes are previews. The
                // address and server name are the user's own and are never
                // logged.
                MenuAction::HostLanServer(_) => {
                    tracing::info!("LAN hosting is not implemented yet.");
                    self.menu
                        .notify("Hosting LAN games is not available yet. Try a skirmish!");
                }
                MenuAction::JoinLanServer { .. } => {
                    tracing::info!("Joining LAN games is not implemented yet.");
                    self.menu.notify("Joining LAN games is not available yet.");
                }
                MenuAction::OptionsChanged => {
                    self.apply_options();
                    self.settings_dirty = true;
                }
                MenuAction::ApplyDisplay(display) => {
                    if let Some(active) = self.state.as_ref() {
                        apply_display(&active.window, display);
                    }
                    self.settings_dirty = true;
                }
            }
        }
    }

    /// Applies every live option: the volumes, vertical sync, the
    /// crosshair and whether the console may stay open. Mouse settings,
    /// the field of view and the frame-rate cap are read where they are
    /// used, and the display mode only on APPLY or at startup.
    fn apply_options(&mut self) {
        let options = &self.menu.options;
        let audio = options.audio;
        self.audio.set_volume(audio.master);
        for class in ChannelClass::ALL {
            let volume = match class {
                ChannelClass::Voice => audio.voice,
                ChannelClass::Static | ChannelClass::Stream => audio.ambience,
                ChannelClass::Auto
                | ChannelClass::Weapon
                | ChannelClass::Item
                | ChannelClass::Body => audio.effects,
            };
            self.audio.set_class_volume(class, volume);
        }
        if let Some(active) = self.state.as_mut() {
            active
                .surface
                .set_vsync(&active.context, options.video.vsync);
        }
        self.hud.show_crosshair = options.gameplay.crosshair;
        if !options.gameplay.console && self.console.is_open() {
            self.console.set_open(false);
        }
    }

    /// Writes the options to the settings file if they changed.
    fn save_settings(&mut self) {
        if !self.settings_dirty {
            return;
        }
        self.settings_dirty = false;
        if let Some(path) = self.settings_path.as_deref() {
            crate::settings::save(path, &self.menu.options);
        }
    }

    /// Starts a skirmish the menu's skirmish pane set up. The map name is
    /// the user's own data and is never logged.
    fn start_skirmish(
        &mut self,
        map: &str,
        bots: u8,
        skill: MenuBotSkill,
        frag_limit: u32,
        time_limit_minutes: u32,
    ) {
        let skirmish = ohl_engine::SkirmishConfig {
            bots,
            bot_skill: match skill {
                MenuBotSkill::Easy => ohl_engine::BotSkill::Easy,
                MenuBotSkill::Normal => ohl_engine::BotSkill::Normal,
                MenuBotSkill::Hard => ohl_engine::BotSkill::Hard,
            },
            frag_limit,
            time_limit_seconds: f32::from(u16::try_from(time_limit_minutes).unwrap_or(u16::MAX))
                * 60.0,
            ..ohl_engine::SkirmishConfig::default()
        };
        if let Ok(game) = Game::load_skirmish(self.source, map, &self.config, &skirmish) {
            self.enter_new_game(game);
            tracing::info!("Skirmish started.");
        } else {
            tracing::warn!("The selected skirmish map could not be started.");
            self.menu.notify("That map could not be started.");
        }
    }

    /// Refreshes what a pane lists when the player opens it: the saves for
    /// the load pane, and (once, since it reads every published map's
    /// entity lump) the deathmatch maps for the skirmish and LAN server
    /// panes.
    fn refresh_menu_data(&mut self) {
        let pane = self.menu.pane;
        if pane != self.last_pane {
            self.last_pane = pane;
            if pane == MenuPane::LoadGame {
                self.save_entries = self
                    .saves
                    .as_ref()
                    .map(|slot| save_entries(slot, now_unix_secs()))
                    .unwrap_or_default();
                self.menu.selected_save = None;
            }
        }
        if matches!(pane, MenuPane::Skirmish | MenuPane::CreateServer)
            && self.skirmish_maps.is_none()
        {
            self.skirmish_maps = Some(crate::skirmish::deathmatch_maps(
                self.source,
                &self.all_maps,
            ));
        }
    }

    /// Leaves the game for the main menu: it stops ticking (`Self::draw`
    /// only ticks it in game) and sounding, and its HUD is cleared.
    fn return_to_main_menu(&mut self) {
        self.audio.stop_all();
        self.hud = HudState::default();
        self.menu.reset();
        self.set_screen(Screen::MainMenu);
    }

    /// Whether `action` is held down.
    fn is_held(&self, action: Action) -> bool {
        self.held[action.index()]
    }

    /// Acts on a bound key or mouse button going down (`pressed`) or up.
    /// `repeat` marks a held key's auto-repeat, which is never a new press.
    fn handle_action(&mut self, action: Action, pressed: bool, repeat: bool) {
        let edge = pressed && !repeat && !self.is_held(action);
        self.held[action.index()] = pressed;
        let slot = match action {
            Action::Weapon1 => Some(1),
            Action::Weapon2 => Some(2),
            Action::Weapon3 => Some(3),
            Action::Weapon4 => Some(4),
            Action::Weapon5 => Some(5),
            _ => None,
        };
        if let Some(slot) = slot {
            if edge {
                self.input.select_slot = Some(slot);
            }
            return;
        }
        match action {
            Action::MoveForward
            | Action::MoveBack
            | Action::MoveLeft
            | Action::MoveRight
            | Action::Jump
            | Action::Duck => self.update_movement(),
            Action::Use => {
                self.input.use_pressed |= edge;
                self.input.use_held = pressed;
            }
            // A press reaches the next tick even when the button is let go
            // before it (`Self::tick_game` then puts the held state back).
            Action::Attack => self.input.attack |= pressed,
            Action::Attack2 => self.input.attack2 |= pressed,
            Action::Reload => self.input.reload |= edge,
            Action::Flashlight => self.input.flashlight_pressed |= edge,
            Action::Scoreboard => self.scoreboard_held = pressed,
            Action::QuickSave if edge => {
                self.quicksave();
            }
            Action::QuickLoad if edge => self.quickload(),
            Action::PerformanceOverlay if edge => {
                let overlay = &mut self.menu.options.video.performance_overlay;
                *overlay = !*overlay;
                self.settings_dirty = true;
            }
            _ => {}
        }
    }

    /// Recomputes the movement axes from the held movement actions, so
    /// releasing one of two opposite keys leaves the other in effect.
    fn update_movement(&mut self) {
        let axis = |app: &Self, positive: Action, negative: Action| {
            i8::from(app.is_held(positive)) - i8::from(app.is_held(negative))
        };
        self.input.forward = axis(self, Action::MoveForward, Action::MoveBack);
        self.input.right = axis(self, Action::MoveRight, Action::MoveLeft);
        self.input.up = axis(self, Action::Jump, Action::Duck);
        self.input.jump = self.is_held(Action::Jump);
        self.input.duck = self.is_held(Action::Duck);
    }

    /// Clears every held axis, so releasing the pointer into the console
    /// does not leave the player walking.
    fn release_movement(&mut self) {
        self.input = Input {
            mouse_delta: self.input.mouse_delta,
            ..Input::default()
        };
        self.held = [false; Action::ALL.len()];
        self.scoreboard_held = false;
    }

    /// Turns raw mouse motion into look input, scaled by the sensitivity
    /// setting and inverted vertically when the player asked for it.
    fn add_mouse_motion(&mut self, delta_x: f32, delta_y: f32) {
        let controls = &self.menu.options.controls;
        let scale = controls.sensitivity / BASE_SENSITIVITY;
        let vertical = if controls.invert_mouse { -scale } else { scale };
        self.input.mouse_delta.0 += delta_x * scale;
        self.input.mouse_delta.1 += delta_y * vertical;
    }

    /// `Escape`: closes the console, pauses the game, or steps back through
    /// the menu (resuming from the pause menu's root).
    fn escape(&mut self) {
        if self.console.is_open() {
            self.console.set_open(false);
            return;
        }
        // An open dropdown takes the key for itself (egui closes it).
        if matches!(self.screen, Screen::MainMenu | Screen::Pause) && self.menu.popup_open() {
            return;
        }
        match self.screen {
            Screen::InGame => {
                self.menu.reset();
                self.set_screen(Screen::Pause);
            }
            Screen::Pause => {
                if !self.menu.back() {
                    self.set_screen(Screen::InGame);
                }
            }
            Screen::Console => self.set_screen(Screen::InGame),
            // The main menu's root has nowhere to go back to; QUIT is a
            // button, not a stray key press.
            Screen::MainMenu => {
                self.menu.back();
            }
        }
    }

    /// Routes a key or mouse press to the binding being captured in the
    /// controls tab. Returns whether the event was consumed by it.
    fn capture_binding(&mut self, event: &WindowEvent) -> bool {
        if self.menu.capturing_binding().is_none() {
            return false;
        }
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && let PhysicalKey::Code(code) = event.physical_key
                {
                    if code == KeyCode::Escape {
                        self.menu.cancel_capture();
                    } else {
                        self.menu.capture_binding(Binding::from_key(code));
                    }
                }
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                self.menu.capture_binding(Binding::from_mouse(*button));
                true
            }
            _ => false,
        }
    }

    /// Writes the autosave slot after a level change, if a save directory
    /// exists and the player has not switched autosaving off. A failure is
    /// reported once and never retried in a loop.
    fn autosave(&mut self) {
        if self.menu.options.gameplay.autosave && self.write_slot(ohl_save::AUTOSAVE_SLOT_NAME) {
            tracing::info!("Autosaved.");
        }
    }

    fn quicksave(&mut self) -> bool {
        let saved = self.write_slot(ohl_save::QUICKSAVE_SLOT_NAME);
        if saved {
            tracing::info!("Quicksaved.");
        }
        saved
    }

    /// Writes one save slot, reporting failure as a fixed line. The slot
    /// name is never logged: it is either a constant or user-supplied.
    fn write_slot(&mut self, name: &str) -> bool {
        if self.game.is_skirmish() {
            tracing::info!("A skirmish is not saved.");
            return false;
        }
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
            // A loaded game restarts its ambience from the map's entity
            // defaults (`ohl_game::AmbientState` is not saved), so a loop
            // the abandoned game had switched on, and the load spawns
            // silent, would otherwise hum on with no entity left to stop
            // it.
            self.audio.stop_all();
            tracing::info!("Quickload complete.");
        } else {
            tracing::warn!("The quicksave could not be loaded.");
        }
    }

    /// Loads the save in `name` from the load pane and starts playing it.
    /// The slot name is never logged.
    fn load_save(&mut self, name: &str) {
        let loaded = self
            .saves
            .as_ref()
            .and_then(|slot| Game::load_slot(self.source, slot, name).ok());
        if let Some(game) = loaded {
            self.enter_new_game(game);
            tracing::info!("Saved game loaded.");
        } else {
            tracing::warn!("The saved game could not be loaded.");
            self.menu.notify("That saved game could not be loaded.");
        }
    }

    /// Acts on one frame's [`GameEvent`]s, in the order the game produced
    /// them.
    /// Returns whether a `trigger_endsection` ended the game this frame,
    /// in which case the window is already on the main menu.
    fn handle_game_events(&mut self, events: Vec<GameEvent>) -> bool {
        // Every event in one frame's list was produced on the map the frame
        // started on, and a level change is listed before the sounds of the
        // same frame (`Game::tick`). Once the change has been followed,
        // those sounds belong to a map that is gone: playing them would
        // start a sound in the new one that nothing there will ever stop.
        let mut left_the_map = false;
        for event in events {
            match event {
                GameEvent::LevelChange { map, landmark } => {
                    // Neither string is logged: both are map-derived.
                    if self.game.change_level(self.source, &map, &landmark).is_ok() {
                        tracing::info!("Level changed.");
                        // The map being left stops sounding; the arriving
                        // one re-announces its own ambience.
                        self.audio.stop_all();
                        left_the_map = true;
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
                // The map's own soundscape and the payload's own speech
                // are played here; a cue whose asset this project has no
                // reviewed path for is dropped inside `AudioRuntime::play`
                // (see `crate::audio` and `ohl_gameplay::sounds`).
                GameEvent::Sound(cue) => {
                    if !left_the_map {
                        self.audio.play(self.source, &cue);
                    }
                }
                // Suit metadata remains informational; supported suit audio
                // already arrives as Sound, so handling it again would restart.
                GameEvent::Suit(_) | GameEvent::ViewModel(_) | GameEvent::PlayerRespawned => {}
                GameEvent::PlayerDied => {
                    tracing::info!("The player died.");
                }
                // Every name is project-authored (the player's or a bot's),
                // so the line goes to the HUD; nothing is logged per frag.
                GameEvent::Frag {
                    killer,
                    victim,
                    weapon,
                    involves_player,
                } => {
                    self.hud.push_kill_feed(
                        crate::skirmish::kill_feed_text(killer.as_deref(), &victim, weapon),
                        involves_player,
                    );
                }
                GameEvent::MatchOver { .. } => tracing::info!("The skirmish match ended."),
                GameEvent::EndSection => {
                    // "Returns the player to the game's main menu": the
                    // game stops ticking (`Self::draw` only ticks it
                    // in-game), its sounds stop and its HUD is cleared, and
                    // starting a mission from the menu loads a fresh one,
                    // exactly as from a cold start. The rest of this
                    // frame's events belong to a game that is over and are
                    // dropped: a level change listed after this one must
                    // not load (and autosave) a map behind the menu.
                    tracing::info!("{SECTION_ENDED}");
                    self.return_to_main_menu();
                    return true;
                }
            }
        }
        false
    }

    /// Advances simulation and refreshes the HUD for one display frame.
    fn tick_game(&mut self, delta_seconds: f32) {
        // The held axes persist across frames; the edge-triggered fields
        // (mouse motion, the presses, a click already let go) are consumed
        // here.
        let frame_input = self.input;
        self.input.attack = self.is_held(Action::Attack);
        self.input.attack2 = self.is_held(Action::Attack2);
        self.input.mouse_delta = (0.0, 0.0);
        self.input.use_pressed = false;
        self.input.reload = false;
        self.input.select_slot = None;
        self.input.flashlight_pressed = false;
        // Where the player's ears are for the cues this frame produces.
        self.audio
            .set_listener(self.game.eye_position(), self.game.camera().yaw);
        let events = self.game.tick(delta_seconds, &frame_input);
        if self.handle_game_events(events) {
            // The section ended: the menu is up and the HUD was cleared,
            // so nothing from the game that just ended is copied back in.
            return;
        }
        self.audio.frame(delta_seconds);
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
        self.hud.show_crosshair = self.menu.options.gameplay.crosshair;

        self.hud.decay_damage_flash(2.0, delta_seconds);
        self.hud.tick_message(delta_seconds);
        self.hud.tick_kill_feed(delta_seconds);
        if let Some(status) = self.game.skirmish_status() {
            self.hud.frags = Some(status.human_frags);
            self.hud.match_clock = status.seconds_left;
            self.hud.center_notice = crate::skirmish::center_notice(&status);
            // The scoreboard has been up for the whole intermission
            // (`mp_chattime`); the match is over, so back to the menu.
            if status.intermission_left.is_some_and(|left| left <= 0.0) {
                tracing::info!("The skirmish intermission ended.");
                self.return_to_main_menu();
            }
        } else {
            self.hud.frags = None;
            self.hud.match_clock = None;
            self.hud.center_notice = None;
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one frame's tick, render and UI sequence; splitting it would only scatter \
                  the borrow of the active window across helpers"
    )]
    fn draw(&mut self) {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        if let Some(interval) = self.frame_interval() {
            // From when this frame was due, but never so far behind that a
            // stall is made up with a burst of frames.
            self.next_frame = (self.next_frame + interval).max(now);
        }

        if self.screen == Screen::InGame && !self.console.is_open() {
            self.tick_game(delta.as_secs_f32());
        }
        self.refresh_menu_data();
        self.game.set_fov_y_degrees(self.menu.options.video.fov);
        let simulation = now.elapsed();

        let Some(active) = self.state.as_mut() else {
            return;
        };
        let acquire_start = Instant::now();
        let Some(frame) = active.surface.acquire(&active.context) else {
            return;
        };
        let acquire = acquire_start.elapsed();
        let render_start = Instant::now();
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (width, height) = (active.surface.width(), active.surface.height());
        if let Err(error) = self.game.render(
            &active.context,
            RenderTarget {
                view: &view,
                width,
                height,
                format: active.surface.format(),
            },
        ) {
            // As `render_capture`'s own `map_err`: the specific, sanitized
            // reason, not one generic string every render failure used to
            // collapse into.
            self.failure = Some(error.message());
        }

        let render = render_start.elapsed();
        let ui_start = Instant::now();
        active.ui.begin_frame();
        // The HUD belongs to the game being played; the menus cover it.
        if !matches!(self.screen, Screen::MainMenu | Screen::Pause) {
            let held = self.scoreboard_held && self.screen == Screen::InGame;
            draw_play_overlay(active.ui.context(), &self.hud, &self.game, held);
        }
        if self.menu.options.video.performance_overlay {
            draw_graphics_debug(active, &self.live_profile, now, &self.game, width, height);
        }
        if self.console.is_open() {
            let mut root = ohl_ui::root_ui(active.ui.context());
            let _ = ohl_ui::console::draw_console(&mut root, &mut self.console);
        }
        let menu_actions = if matches!(self.screen, Screen::MainMenu | Screen::Pause) {
            let published = |map: &'static str| map_published(&self.all_maps, map).then_some(map);
            let data = MenuData {
                in_game: self.screen == Screen::Pause,
                can_save: self.saves.is_some() && !self.game.is_skirmish(),
                new_game_map: published(ohl_campaign::STARTMAP),
                training_map: published(ohl_campaign::TRAINMAP),
                chapters: &self.chapters,
                skirmish_maps: self.skirmish_maps.as_deref().unwrap_or(&[]),
                saves: &self.save_entries,
                resolutions: &self.resolutions,
                audio_output: self.audio.is_audible(),
            };
            let mut root = ohl_ui::root_ui(active.ui.context());
            ohl_ui::menu::draw(&mut root, &mut self.menu, &data)
        } else {
            Vec::new()
        };
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

        let sample = FrameSample {
            frame: delta,
            simulation,
            acquire,
            render,
            ui_present: ui_start.elapsed(),
            ..FrameSample::default()
        };
        self.live_profile.record(now, sample);
        if let Some(profile) = self.profile.as_mut() {
            profile.record(sample);
        }

        self.frames += 1;
        let elapsed = now.saturating_duration_since(self.fps_window_start);
        if elapsed >= FPS_INTERVAL {
            #[allow(clippy::cast_precision_loss)]
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            // A property of this machine and this run, not of the map.
            tracing::info!(fps = format_args!("{fps:.1}"), "frame rate");
            if let Some(profile) = self.profile.as_mut()
                && let Some(summary) = profile.finish(elapsed)
            {
                summary.log("window_cpu");
                log_resource_uploads(&self.game, ohl_engine::RenderResourceStats::default());
            }
            self.frames = 0;
            self.fps_window_start = now;
        }
        self.handle_menu_actions(menu_actions);
        // Sliders are saved once the player leaves the options, not on
        // every frame of a drag.
        if self.menu.pane != MenuPane::Options {
            self.save_settings();
        }
    }

    /// The time between frames under the frame-rate cap, or `None` when it
    /// is uncapped.
    fn frame_interval(&self) -> Option<Duration> {
        let limit = self.menu.options.video.fps_limit;
        (limit > 0).then(|| Duration::from_secs_f64(1.0 / f64::from(limit)))
    }
}

impl ApplicationHandler for App<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let display = self.menu.options.video.display;
        let size = if display.mode == DisplayMode::Windowed {
            (display.resolution.width, display.resolution.height)
        } else {
            INITIAL_SIZE
        };
        let attributes = Window::default_attributes()
            .with_title("Open Half-Life")
            .with_active(self.profile.is_none())
            .with_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1));
        let Ok(window) = event_loop.create_window(attributes) else {
            self.fail(event_loop, "a window could not be created");
            return;
        };
        let window = Arc::new(window);
        if display.mode != DisplayMode::Windowed {
            apply_display(&window, display);
        }
        if self.profile.is_none() && self.screen == Screen::InGame {
            if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                let _ = window.set_cursor_grab(CursorGrabMode::Confined);
            }
            window.set_cursor_visible(false);
        }
        self.resolutions = display_resolutions(&window);

        let Ok((context, wgpu_surface)) = GpuContext::for_surface(Arc::clone(&window)) else {
            self.fail(event_loop, "no usable graphics adapter is available");
            return;
        };
        let size = window.inner_size();
        if self.profile.is_some() {
            log_profile_device(&context, size.width, size.height);
        }
        let Ok(surface) = WindowSurface::new(&context, wgpu_surface, size.width, size.height)
        else {
            self.fail(event_loop, "the window surface could not be configured");
            return;
        };
        let ui = UiLayer::new_windowed(&context.device, Arc::clone(&window), surface.format());
        let adapter_info = context.adapter.get_info();

        self.last_frame = Instant::now();
        self.next_frame = self.last_frame;
        self.fps_window_start = self.last_frame;
        self.frames = 0;
        self.state = Some(Active {
            window,
            context,
            surface,
            ui,
            adapter_name: adapter_info.name,
            backend_name: format!("{:?}", adapter_info.backend),
        });
        self.apply_options();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // A key or button the controls tab is waiting for goes to it, not
        // to the menu widgets or the game.
        if self.capture_binding(&event) {
            return;
        }
        if let Some(active) = self.state.as_mut() {
            let consumed = active.ui.handle_window_event(&event);
            if consumed
                && (self.console.is_open()
                    || matches!(self.screen, Screen::MainMenu | Screen::Pause))
                && !matches!(event, WindowEvent::KeyboardInput { .. })
            {
                return;
            }
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
                    if pressed && !event.repeat && self.menu.options.gameplay.console {
                        self.console.toggle();
                        self.release_movement();
                    }
                    return;
                }
                if code == KeyCode::Escape {
                    if pressed && !event.repeat {
                        self.escape();
                    }
                    return;
                }
                if self.console.is_open() || matches!(self.screen, Screen::MainMenu | Screen::Pause)
                {
                    return;
                }
                if let Some(action) = self.menu.options.controls.bindings.action_for_key(code) {
                    self.handle_action(action, pressed, event.repeat);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.console.is_open() || self.screen != Screen::InGame {
                    return;
                }
                if let Some(action) = self.menu.options.controls.bindings.action_for_mouse(button) {
                    self.handle_action(action, state == ElementState::Pressed, false);
                }
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
        if self.console.is_open() || self.screen != Screen::InGame || self.profile.is_some() {
            return;
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            #[allow(clippy::cast_possible_truncation)]
            let (delta_x, delta_y) = (delta.0 as f32, delta.1 as f32);
            self.add_mouse_motion(delta_x, delta_y);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit_requested {
            event_loop.exit();
            return;
        }
        let Some(active) = self.state.as_ref() else {
            return;
        };
        match self.frame_interval() {
            Some(_) if Instant::now() < self.next_frame => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
            }
            _ => {
                event_loop.set_control_flow(ControlFlow::Poll);
                active.window.request_redraw();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.save_settings();
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

/// Every run path in this module hands its `GameEvent::Sound` cues to the
/// audio runtime, and every way of leaving a game behind silences it.
///
/// Each test drives the run path's own code ([`run_script_ticks`] and
/// [`route_headless_events`], [`route_benchmark_events`], and
/// [`App::tick_game`]/[`App::handle_game_events`]/
/// [`App::handle_menu_actions`]/[`App::quickload`])
/// over a synthetic room and a silent runtime, so none of them opens an
/// output device or needs a GPU. Every map, entity block, sound name,
/// `sentences.txt` line and WAV here is project-authored; nothing comes
/// from any game installation.
#[cfg(test)]
mod sound_routing_tests {
    use super::*;
    use crate::audio::fixtures::{sampled_synthetic_wav, synthetic_wav};
    use ohl_engine::test_support::{
        LANDMARK, NEXT_MAP, SCRIPT_MAP, SYNTHETIC_MAP, entity_block, entity_of_classname,
        script_room_bsp, script_room_entities, synthetic_map_bsp_named,
        synthetic_map_bsp_with_extra_entity,
    };
    use ohl_engine::{ChannelClass, MemoryAssets, SoundAsset, SoundCue};

    /// Two seconds of 22.05 kHz audio: longer than any test below pumps
    /// the mixer for, so a channel that is gone was stopped, not run out.
    const LONG_SOUND_FRAMES: usize = 44_100;

    /// One `ambient_generic` that sounds from the map's first tick.
    fn humming_ambient(origin: [f32; 3]) -> String {
        entity_block(
            "ambient_generic",
            origin,
            0.0,
            &[("targetname", "ohl_hum"), ("message", "ohl/hum.wav")],
        )
    }

    /// The payload side: the ambient's sound, a one-line `sentences.txt`
    /// and the two word samples its one sentence names.
    fn sound_assets() -> MemoryAssets {
        let mut assets = MemoryAssets::new();
        assets.insert("sound/ohl/hum.wav", synthetic_wav(LONG_SOUND_FRAMES));
        assets.insert(
            "sound/sentences.txt",
            b"OHL_GREETING ohl/hello ohl/there\n".to_vec(),
        );
        assets.insert("sound/ohl/hello.wav", synthetic_wav(LONG_SOUND_FRAMES));
        assets.insert("sound/ohl/there.wav", synthetic_wav(LONG_SOUND_FRAMES));
        assets
    }

    /// The script room with the humming ambient, plus a scientist told by
    /// a `scripted_sentence` to speak the one sentence `sound_assets`
    /// publishes, as soon as the map loads. Use the published `!` marker
    /// so the host tests cover explicit references all the way to the mixer.
    fn speaking_room() -> String {
        script_room_entities(
            [-192.0, -192.0, 36.0],
            &format!(
                "{}{}{}{}",
                humming_ambient([64.0, 0.0, 48.0]),
                entity_block(
                    "monster_scientist",
                    [0.0, 96.0, 36.0],
                    0.0,
                    &[("targetname", "ohl_speaker")],
                ),
                entity_block(
                    "scripted_sentence",
                    [0.0, 0.0, 36.0],
                    0.0,
                    &[
                        ("targetname", "ohl_line"),
                        ("sentence", "!OHL_GREETING"),
                        ("entity", "ohl_speaker"),
                        ("spawnflags", "1"),
                    ],
                ),
                entity_block(
                    "trigger_auto",
                    [0.0, 0.0, 0.0],
                    0.0,
                    &[("target", "ohl_line")],
                ),
            ),
        )
    }

    /// `assets` plus the script room built from `entities`, loaded.
    fn script_room_game(assets: &mut MemoryAssets, entities: &str) -> Game {
        assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), script_room_bsp(entities));
        Game::load(&*assets, SCRIPT_MAP).expect("the synthetic room loads")
    }

    /// The low 32 bits of `classname`'s first entity: the key both an
    /// ambient's and a sentence's cue name their channel by.
    fn channel_key(game: &Game, classname: &str) -> u32 {
        entity_of_classname(game, classname)
            .expect("the fixture spawns it")
            .id()
    }

    fn is_playing(audio: &AudioRuntime, entity: u32, class: ChannelClass) -> bool {
        audio
            .mixer()
            .lock()
            .expect("lock mixer")
            .is_playing(entity, class)
    }

    fn channel_count(audio: &AudioRuntime) -> usize {
        audio
            .mixer()
            .lock()
            .expect("lock mixer")
            .active_channel_count()
    }

    /// Runs `ticks` idle ticks of a scripted run over `game`.
    fn run_idle(
        game: &mut Game,
        source: &dyn AssetSource,
        audio: &mut AudioRuntime,
        ticks: u32,
        follow_level_change: bool,
    ) -> TickOutcome {
        let script = crate::script::Script::parse(format!("{ticks} wait\n").as_bytes())
            .expect("a project-authored script parses");
        let mut log = crate::script_log::ScriptLog::new(game);
        run_script_ticks(
            game,
            source,
            audio,
            &script,
            &mut log,
            &TickOptions {
                script_log: false,
                follow_level_change,
                stop_on_level_change: false,
            },
        )
    }

    /// What `--script` and `--chain-script` do with a map's ambience and a
    /// monster's speech: both reach the mixer, the ambient on its own
    /// static channel and the sentence on the speaker's voice channel.
    #[test]
    fn a_scripted_run_hands_the_maps_ambience_and_its_speech_to_the_mixer() {
        let mut assets = sound_assets();
        let mut game = script_room_game(&mut assets, &speaking_room());
        let ambient = channel_key(&game, "ambient_generic");
        let speaker = channel_key(&game, "monster_scientist");
        let mut audio = AudioRuntime::silent();

        run_idle(&mut game, &assets, &mut audio, 30, false);

        assert!(
            is_playing(&audio, ambient, ChannelClass::Static),
            "the map's ambient_generic is sounding"
        );
        assert!(
            is_playing(&audio, speaker, ChannelClass::Voice),
            "the scientist is speaking its sentence"
        );
    }

    /// A followed level change stops everything the map being left was
    /// playing; the arriving map (which has no ambience of its own here)
    /// leaves the mixer empty.
    #[test]
    fn a_followed_level_change_silences_the_map_being_left() {
        let mut assets = sound_assets();
        let leaving = synthetic_map_bsp_with_extra_entity(
            NEXT_MAP,
            &format!(
                "{}{}",
                humming_ambient([64.0, 0.0, 48.0]),
                // Fires the fixture's own named `trigger_changelevel`
                // half a second in.
                entity_block(
                    "trigger_auto",
                    [0.0, 0.0, 0.0],
                    0.0,
                    &[("target", "ohl_exit"), ("delay", "0.5")],
                ),
            ),
        );
        assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), leaving);
        assets.insert(
            &format!("maps/{NEXT_MAP}.bsp"),
            synthetic_map_bsp_named(SYNTHETIC_MAP),
        );
        let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the synthetic map loads");
        let ambient = channel_key(&game, "ambient_generic");
        let mut audio = AudioRuntime::silent();

        let before = run_idle(&mut game, &assets, &mut audio, 10, true);
        assert!(!before.followed_level_change);
        assert!(is_playing(&audio, ambient, ChannelClass::Static));

        let after = run_idle(&mut game, &assets, &mut audio, 60, true);
        assert!(after.followed_level_change, "the named change fired");
        assert_eq!(
            channel_count(&audio),
            0,
            "nothing from the map that was left is still playing"
        );
    }

    /// The two-map fixture: the synthetic room, whose named
    /// `trigger_changelevel` leads to [`NEXT_MAP`], and that map, loaded
    /// on the first.
    fn two_map_game(assets: &mut MemoryAssets) -> Game {
        assets.insert(
            &format!("maps/{SYNTHETIC_MAP}.bsp"),
            synthetic_map_bsp_named(NEXT_MAP),
        );
        assets.insert(
            &format!("maps/{NEXT_MAP}.bsp"),
            synthetic_map_bsp_named(SYNTHETIC_MAP),
        );
        Game::load(&*assets, SYNTHETIC_MAP).expect("the synthetic map loads")
    }

    /// One tick's events as `Game::tick` orders them: a level change, then
    /// a sound produced on the map that tick started on.
    fn change_then_sound() -> Vec<GameEvent> {
        vec![
            GameEvent::LevelChange {
                map: String::from(NEXT_MAP),
                landmark: String::from(LANDMARK),
            },
            GameEvent::Sound(SoundCue::new(
                7,
                ChannelClass::Static,
                SoundAsset::file("sound/ohl/hum.wav"),
            )),
        ]
    }

    /// A sound listed after a followed level change in the same tick was
    /// produced on the map that was left; a run nobody listens to drops
    /// it rather than starting it in the new one.
    #[test]
    fn a_headless_run_drops_the_sounds_of_the_tick_that_left_the_map() {
        let mut assets = sound_assets();
        let mut game = two_map_game(&mut assets);
        let mut audio = AudioRuntime::silent();

        let followed = route_headless_events(
            &mut game,
            &assets,
            &mut audio,
            change_then_sound(),
            &HeadlessEventOptions {
                follow_level_change: true,
                script_log: false,
                player_died_line: "The player died.",
            },
        )
        .followed_level_change;
        assert!(followed);
        assert_eq!(game.map(), NEXT_MAP);
        assert_eq!(channel_count(&audio), 0);

        // Not followed, the same sound is the current map's own, and plays.
        let mut game = two_map_game(&mut assets);
        let followed = route_headless_events(
            &mut game,
            &assets,
            &mut audio,
            change_then_sound(),
            &HeadlessEventOptions {
                follow_level_change: false,
                script_log: false,
                player_died_line: "The player died.",
            },
        )
        .followed_level_change;
        assert!(!followed);
        assert_eq!(channel_count(&audio), 1);
    }

    /// The window's own level change silences the map being left, and
    /// drops the sounds of the tick that left it.
    #[test]
    fn the_windows_level_change_silences_the_map_it_left() {
        let mut assets = sound_assets();
        let game = two_map_game(&mut assets);
        let mut app = window(game, &assets);
        // Something the map being left was already playing.
        app.handle_game_events(vec![GameEvent::Sound(SoundCue::new(
            9,
            ChannelClass::Static,
            SoundAsset::file("sound/ohl/hum.wav"),
        ))]);
        assert_eq!(channel_count(&app.audio), 1);

        app.handle_game_events(change_then_sound());
        assert_eq!(app.game.map(), NEXT_MAP, "the level change was followed");
        assert_eq!(channel_count(&app.audio), 0);
    }

    /// `--benchmark` plays what it hears, and ends only on what would
    /// change its workload.
    #[test]
    fn the_benchmark_plays_its_sounds_and_stops_on_a_level_change_or_a_death() {
        let assets = sound_assets();
        let mut audio = AudioRuntime::silent();
        let hum = || {
            GameEvent::Sound(SoundCue::new(
                7,
                ChannelClass::Static,
                SoundAsset::file("sound/ohl/hum.wav"),
            ))
        };

        assert!(!route_benchmark_events(&mut audio, &assets, vec![hum()]));
        assert!(is_playing(&audio, 7, ChannelClass::Static));

        assert!(route_benchmark_events(
            &mut audio,
            &assets,
            vec![GameEvent::PlayerDied]
        ));
        assert!(route_benchmark_events(
            &mut audio,
            &assets,
            vec![GameEvent::LevelChange {
                map: String::from(NEXT_MAP),
                landmark: String::from(LANDMARK),
            }],
        ));
        assert!(
            route_benchmark_events(&mut audio, &assets, vec![GameEvent::EndSection]),
            "a section that ended ends the benchmark too"
        );
    }

    /// A section ending, then a level change and a sound in the same tick,
    /// as one tick could list them: a run nobody watches stops on the
    /// first, silences what was playing, and neither follows the change nor
    /// starts the sound.
    #[test]
    fn a_headless_run_stops_at_the_section_end_and_drops_the_rest_of_the_tick() {
        let mut assets = sound_assets();
        let mut game = two_map_game(&mut assets);
        let mut audio = AudioRuntime::silent();
        audio.play(
            &assets,
            &SoundCue::new(
                9,
                ChannelClass::Static,
                SoundAsset::file("sound/ohl/hum.wav"),
            ),
        );
        assert_eq!(channel_count(&audio), 1);

        let mut events = vec![GameEvent::EndSection];
        events.extend(change_then_sound());
        let outcome = route_headless_events(
            &mut game,
            &assets,
            &mut audio,
            events,
            &HeadlessEventOptions {
                follow_level_change: true,
                script_log: false,
                player_died_line: "The player died.",
            },
        );
        assert!(outcome.ended_section);
        assert!(!outcome.followed_level_change);
        assert_eq!(
            game.map(),
            SYNTHETIC_MAP,
            "no map loads behind an ended run"
        );
        assert_eq!(channel_count(&audio), 0, "and nothing keeps playing");
    }

    /// The window's own section end: back to the main menu with the game's
    /// sounds stopped and its HUD cleared, and the same tick's later level
    /// change never followed.
    #[test]
    fn the_windows_section_end_returns_to_a_silent_main_menu() {
        let mut assets = sound_assets();
        let game = two_map_game(&mut assets);
        let mut app = window(game, &assets);
        app.handle_game_events(vec![GameEvent::Sound(SoundCue::new(
            9,
            ChannelClass::Static,
            SoundAsset::file("sound/ohl/hum.wav"),
        ))]);
        app.hud.clip_ammo = Some(6);
        app.hud.reserve_ammo = Some(12);
        assert_eq!(channel_count(&app.audio), 1);

        let mut events = vec![GameEvent::EndSection];
        events.extend(change_then_sound());
        assert!(app.handle_game_events(events));
        assert_eq!(app.screen, Screen::MainMenu);
        assert_eq!(
            app.game.map(),
            SYNTHETIC_MAP,
            "the later level change is dropped"
        );
        assert_eq!(channel_count(&app.audio), 0, "nothing plays over the menu");
        assert_eq!(app.hud.clip_ammo, None);
        assert_eq!(app.hud.reserve_ammo, None);
    }

    /// The window's own frame: once the section ends, the HUD is left
    /// cleared rather than refilled from the game that just ended — the
    /// gun the player was holding is not still on the menu's HUD.
    #[test]
    fn the_windows_frame_leaves_the_hud_cleared_once_the_section_ends() {
        let mut assets = MemoryAssets::new();
        assets.insert(
            "maps/ohlendsectionwindowsynth.bsp",
            ohl_engine::test_support::killable_brush_floor_bsp(
                "{\n\"classname\" \"worldspawn\"\n}\n\
                 {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n}\n\
                 {\n\"classname\" \"func_wall\"\n\"model\" \"*1\"\n}\n\
                 {\n\"classname\" \"trigger_endsection\"\n\"targetname\" \"ohl_end\"\n\
                 \"section\" \"ohl_test_section\"\n\"spawnflags\" \"1\"\n}\n\
                 {\n\"classname\" \"trigger_once\"\n\"target\" \"ohl_end\"\n\"delay\" \"1\"\n\
                 \"model\" \"*1\"\n\"origin\" \"0 0 40\"\n}\n",
            ),
        );
        let mut game = Game::load(&assets, "ohlendsectionwindowsynth").expect("the fixture loads");
        game.give_start_inventory(
            &ohl_engine::parse_start_inventory("weapon_357").expect("a cited classname"),
        );
        let mut app = window(game, &assets);
        app.input.select_slot = Some(2);

        let mut armed = false;
        for _ in 0..240 {
            app.tick_game(1.0 / 60.0);
            armed |= app.hud.reserve_ammo.is_some();
            if app.screen == Screen::MainMenu {
                break;
            }
        }
        assert!(armed, "the gun's reserve was on the HUD before the end");
        assert_eq!(app.screen, Screen::MainMenu, "the section ended");
        assert_eq!(app.hud.clip_ammo, None);
        assert_eq!(app.hud.reserve_ammo, None);
    }

    /// The benchmark's own frame loop, with a renderer that draws nothing:
    /// the map's ambience reaches the mixer through it, as it does in every
    /// other run path.
    #[test]
    fn the_benchmarks_own_frame_loop_plays_the_maps_sounds() {
        let mut assets = sound_assets();
        let mut game = script_room_game(
            &mut assets,
            &script_room_entities([-192.0, -192.0, 36.0], &humming_ambient([64.0, 0.0, 48.0])),
        );
        let ambient = channel_key(&game, "ambient_generic");
        let mut audio = AudioRuntime::silent();
        let mut frames = 0;
        benchmark_frames(
            &mut game,
            &assets,
            &mut audio,
            &BenchmarkWindow {
                warmup: Duration::ZERO,
                seconds: 0,
            },
            |_| {
                frames += 1;
                Ok((Duration::ZERO, Duration::ZERO))
            },
        )
        .expect("a benchmark that draws nothing still runs");
        assert_eq!(frames, 2, "one warm-up frame and one measured frame");
        assert!(is_playing(&audio, ambient, ChannelClass::Static));
    }

    /// Whether the mixer's listener is at `eye`, facing along `yaw`'s right
    /// vector (see `AudioRuntime::set_listener`). The position is a copy of
    /// the eye, so exact equality is the point.
    #[allow(clippy::float_cmp)]
    fn listener_is_at(audio: &AudioRuntime, eye: [f32; 3], yaw: f32) -> bool {
        let listener = audio.mixer().lock().expect("lock mixer").listener();
        let (sin, cos) = yaw.to_radians().sin_cos();
        listener.position == eye
            && (listener.right[0] - sin).abs() < 1e-6
            && (listener.right[1] + cos).abs() < 1e-6
    }

    /// Every run path puts the listener where the player's eye is before
    /// the tick whose cues it then plays, so a sound is panned and
    /// attenuated from where the player stands, not from the world origin.
    #[test]
    fn every_run_path_puts_the_listener_at_the_players_eye() {
        let entities = script_room_entities([-192.0, -192.0, 36.0], "");
        let mut assets = sound_assets();

        // A scripted run.
        let mut game = script_room_game(&mut assets, &entities);
        let (eye, yaw) = (game.eye_position(), game.camera().yaw);
        assert!(
            eye.iter().any(|axis| axis.abs() > 1.0),
            "the fixture's player is away from the origin"
        );
        let mut audio = AudioRuntime::silent();
        run_idle(&mut game, &assets, &mut audio, 1, false);
        assert!(listener_is_at(&audio, eye, yaw), "scripted run");

        // The benchmark's loop.
        let mut game = script_room_game(&mut assets, &entities);
        let (eye, yaw) = (game.eye_position(), game.camera().yaw);
        let mut audio = AudioRuntime::silent();
        benchmark_frames(
            &mut game,
            &assets,
            &mut audio,
            &BenchmarkWindow {
                warmup: Duration::from_secs(3600),
                seconds: 0,
            },
            |_| Err("stop after one frame"),
        )
        .expect_err("the renderer ends the run");
        assert!(listener_is_at(&audio, eye, yaw), "benchmark");

        // The window.
        let game = script_room_game(&mut assets, &entities);
        let (eye, yaw) = (game.eye_position(), game.camera().yaw);
        let mut app = window(game, &assets);
        app.tick_game(CAPTURE_STEP);
        assert!(listener_is_at(&app.audio, eye, yaw), "window");
    }

    /// A window over `game`, silent, with `assets` behind it.
    fn window(game: Game, assets: &MemoryAssets) -> App<'_> {
        App::new(
            game,
            assets,
            AudioRuntime::silent(),
            Screen::InGame,
            GameConfig::default(),
        )
    }

    fn hev_assets(sentence: Option<&[u8]>) -> MemoryAssets {
        let mut assets = MemoryAssets::new();
        let extra = format!(
            "{}{}",
            entity_block("item_suit", [-160.0, -160.0, 36.0], 0.0, &[]),
            entity_block(
                "trigger_hurt",
                [64.0, 64.0, 36.0],
                0.0,
                &[("dmg", "2"), ("damagetype", "8")],
            ),
        );
        assets.insert(
            &format!("maps/{SCRIPT_MAP}.bsp"),
            script_room_bsp(&script_room_entities([-160.0, -160.0, 36.0], &extra)),
        );
        if let Some(sentence) = sentence {
            assets.insert("sound/sentences.txt", sentence.to_vec());
        }
        let ramp: Vec<i16> = (0..128).map(|frame| 1024 + frame * 32).collect();
        assets.insert("sound/ohl/hev_first.wav", sampled_synthetic_wav(&ramp));
        assets.insert(
            "sound/ohl/hev_second.wav",
            sampled_synthetic_wav(&[-8192; 128]),
        );
        assets.insert("sound/ohl/hev_decoy.wav", synthetic_wav(128));
        assets.insert("sound/ohl/hev_invalid.wav", vec![0; 48]);
        assets
    }

    /// Capture the actual engine event pair after the ordinary suit pickup;
    /// no direct suit injection or new public damage-testing API is involved.
    fn hev_burn_pair(assets: &MemoryAssets) -> (Game, SoundCue, GameEvent) {
        let mut game = Game::load(assets, SCRIPT_MAP).expect("synthetic HEV room loads");
        game.tick(ohl_engine::TICK_SECONDS, &Input::default());
        assert!(game.player_suit_equipped());
        assert!((game.player_health() - 100.0).abs() < f32::EPSILON);
        game.set_viewpoint([64.0, 64.0, 36.0], 0.0, 0.0);
        let mut events = Vec::new();
        // Pickup's first tick started the producer's hurt interval. Capture
        // the first actual damage frame without waiting for the new Sound,
        // so adapter/callsite mutations still reach exact cue assertions.
        for _ in 0..100 {
            events = game.tick(ohl_engine::TICK_SECONDS, &Input::default());
            if game.player_health() < 100.0 {
                break;
            }
        }
        assert!(
            game.player_health() < 100.0,
            "burn setup must actually damage"
        );
        let sounds: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                GameEvent::Sound(cue) if cue.class == ChannelClass::Voice => Some(cue.clone()),
                _ => None,
            })
            .collect();
        let suits: Vec<_> = events
            .into_iter()
            .filter(|event| matches!(event, GameEvent::Suit(_)))
            .collect();
        assert_eq!(sounds.len(), 1, "one sound per actual burn occasion");
        assert_eq!(suits.len(), 1, "one existing producer event");
        assert_eq!(sounds[0].entity, game.player_entity().id());
        (game, sounds[0].clone(), suits[0].clone())
    }

    fn hev_pcm(audio: &AudioRuntime) -> Vec<f32> {
        let mut samples = vec![0.0; 32];
        audio
            .mixer()
            .lock()
            .expect("lock mixer")
            .render(&mut samples);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        samples
    }

    fn hev_assert_metadata_cursor(
        mut route: impl FnMut(Vec<GameEvent>) -> Vec<f32>,
        assets: &MemoryAssets,
        cue: SoundCue,
        suit: GameEvent,
    ) {
        assert!(
            route(vec![suit.clone()])
                .iter()
                .all(|sample| sample.abs() < f32::EPSILON),
            "metadata alone is silent"
        );
        let mut control = AudioRuntime::silent();
        control.play(assets, &cue);
        let prefix = hev_pcm(&control);
        let next = hev_pcm(&control);
        assert_ne!(prefix, next, "ramp makes a cursor restart distinguishable");
        assert_eq!(route(vec![GameEvent::Sound(cue)]), prefix);
        assert_eq!(
            route(vec![suit]),
            next,
            "metadata must not restart the existing voice"
        );
    }

    #[test]
    fn hev_audio_all_three_routes_preserve_cursor_after_actual_suit_metadata() {
        let assets = hev_assets(Some(b"HEV_FIRE ohl/hev_first ohl/hev_second\n"));
        for route in 0..3 {
            let (mut game, cue, suit) = hev_burn_pair(&assets);
            assert_eq!(
                cue.asset,
                SoundAsset::sentence(vec![
                    "sound/ohl/hev_first.wav".into(),
                    "sound/ohl/hev_second.wav".into()
                ])
            );
            match route {
                0 => {
                    let mut app = window(game, &assets);
                    hev_assert_metadata_cursor(
                        |events| {
                            assert!(!app.handle_game_events(events));
                            hev_pcm(&app.audio)
                        },
                        &assets,
                        cue,
                        suit,
                    );
                }
                1 => {
                    let mut audio = AudioRuntime::silent();
                    hev_assert_metadata_cursor(
                        |events| {
                            let outcome = route_headless_events(
                                &mut game,
                                &assets,
                                &mut audio,
                                events,
                                &HeadlessEventOptions {
                                    follow_level_change: false,
                                    script_log: false,
                                    player_died_line: "The player died.",
                                },
                            );
                            assert!(!outcome.followed_level_change && !outcome.ended_section);
                            hev_pcm(&audio)
                        },
                        &assets,
                        cue,
                        suit,
                    );
                }
                _ => {
                    let mut audio = AudioRuntime::silent();
                    hev_assert_metadata_cursor(
                        |events| {
                            assert!(!route_benchmark_events(&mut audio, &assets, events));
                            hev_pcm(&audio)
                        },
                        &assets,
                        cue,
                        suit,
                    );
                }
            }
        }
    }

    #[test]
    fn hev_audio_all_three_routes_silence_missing_sentence_or_word_assets() {
        for sentence in [
            None,
            Some(b"HEV_FIRE ohl/hev_absent\n".as_slice()),
            Some(b"HEV_FIRE ohl/hev_invalid\n".as_slice()),
        ] {
            let assets = hev_assets(sentence);
            for route in 0..3 {
                let (mut game, cue, suit) = hev_burn_pair(&assets);
                let events = vec![GameEvent::Sound(cue), suit];
                let pcm = match route {
                    0 => {
                        let mut app = window(game, &assets);
                        assert!(!app.handle_game_events(events));
                        assert_eq!(channel_count(&app.audio), 0);
                        hev_pcm(&app.audio)
                    }
                    1 => {
                        let mut audio = AudioRuntime::silent();
                        route_headless_events(
                            &mut game,
                            &assets,
                            &mut audio,
                            events,
                            &HeadlessEventOptions {
                                follow_level_change: false,
                                script_log: false,
                                player_died_line: "The player died.",
                            },
                        );
                        assert_eq!(channel_count(&audio), 0);
                        hev_pcm(&audio)
                    }
                    _ => {
                        let mut audio = AudioRuntime::silent();
                        assert!(!route_benchmark_events(&mut audio, &assets, events));
                        assert_eq!(channel_count(&audio), 0);
                        hev_pcm(&audio)
                    }
                };
                assert!(
                    pcm.iter().all(|sample| sample.abs() < f32::EPSILON),
                    "missing assets do not use the playable decoy"
                );
            }
        }
    }

    /// The window's own loop plays the map's ambience, and the options
    /// screen's volume slider reaches the mixer.
    #[test]
    fn the_window_plays_the_maps_sounds_and_the_menus_volume_reaches_the_mixer() {
        let mut assets = sound_assets();
        let game = script_room_game(&mut assets, &speaking_room());
        let ambient = channel_key(&game, "ambient_generic");
        let speaker = channel_key(&game, "monster_scientist");
        let mut app = window(game, &assets);

        for _ in 0..30 {
            app.tick_game(CAPTURE_STEP);
        }
        assert!(is_playing(&app.audio, ambient, ChannelClass::Static));
        assert!(is_playing(&app.audio, speaker, ChannelClass::Voice));

        app.menu.options.audio.master = 0.25;
        app.menu.options.audio.voice = 0.5;
        app.handle_menu_actions(vec![MenuAction::OptionsChanged]);
        let mixer = app.audio.mixer().lock().expect("lock mixer");
        let volume = mixer.master_volume();
        assert!((volume - 0.25).abs() < 1e-6, "{volume}");
        let voice = mixer.class_volume(ChannelClass::Voice);
        assert!((voice - 0.5).abs() < 1e-6, "{voice}");
        let ambience = mixer.class_volume(ChannelClass::Static);
        assert!((ambience - 1.0).abs() < 1e-6, "{ambience}");
    }

    /// Starting a mission from the menu replaces the game; whatever the
    /// one being left was playing stops with it.
    #[test]
    fn starting_a_mission_from_the_menu_silences_the_game_being_left() {
        let mut assets = sound_assets();
        assets.insert(
            &format!("maps/{SYNTHETIC_MAP}.bsp"),
            synthetic_map_bsp_named(NEXT_MAP),
        );
        let game = script_room_game(
            &mut assets,
            &script_room_entities([-192.0, -192.0, 36.0], &humming_ambient([64.0, 0.0, 48.0])),
        );
        let mut app = window(game, &assets);
        app.tick_game(CAPTURE_STEP);
        assert_eq!(channel_count(&app.audio), 1);

        app.handle_menu_actions(vec![MenuAction::StartSinglePlayer {
            map: SYNTHETIC_MAP,
            difficulty: MenuDifficulty::Medium,
        }]);
        assert_eq!(app.game.map(), SYNTHETIC_MAP, "the mission started");
        assert_eq!(channel_count(&app.audio), 0);
    }

    /// A quickload replaces the game too, and must silence it the same way.
    #[test]
    fn a_quickload_silences_what_the_abandoned_game_was_playing() {
        let saves = tempfile::tempdir().expect("a temporary save directory");
        let mut assets = sound_assets();
        let game = script_room_game(
            &mut assets,
            &script_room_entities([-192.0, -192.0, 36.0], &humming_ambient([64.0, 0.0, 48.0])),
        );
        let mut app = window(game, &assets);
        app.saves = Some(ohl_save::SaveSlot::new(saves.path()));
        app.quicksave();
        app.tick_game(CAPTURE_STEP);
        assert_eq!(channel_count(&app.audio), 1);

        app.quickload();
        assert_eq!(channel_count(&app.audio), 0);
    }
}

#[cfg(test)]
mod skirmish_window_tests {
    use std::fmt::Write as _;

    use super::*;
    use ohl_engine::MemoryAssets;
    use ohl_engine::test_support::{AI_MAP, deathmatch_room_bsp, queue_engine_damage_from};

    const CORNERS: [(f32, f32); 4] = [
        (-192.0, -192.0),
        (192.0, -192.0),
        (-192.0, 192.0),
        (192.0, 192.0),
    ];

    fn arena_assets() -> MemoryAssets {
        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{AI_MAP}.bsp"),
            deathmatch_room_bsp(&CORNERS, false, ""),
        );
        assets
    }

    fn window(game: Game, assets: &MemoryAssets) -> App<'_> {
        App::new(
            game,
            assets,
            AudioRuntime::silent(),
            Screen::MainMenu,
            GameConfig::default(),
        )
    }

    fn started(assets: &MemoryAssets, frag_limit: u32) -> App<'_> {
        let game = Game::load(assets, AI_MAP).expect("the arena loads as a plain map too");
        let mut app = window(game, assets);
        app.handle_menu_actions(vec![MenuAction::StartSkirmish {
            map: AI_MAP.to_string(),
            bots: 2,
            skill: MenuBotSkill::Easy,
            frag_limit,
            time_limit_minutes: 0,
        }]);
        app
    }

    #[test]
    fn the_skirmish_pane_starts_a_match_against_bots() {
        let assets = arena_assets();
        let app = started(&assets, 10);
        assert!(app.game.is_skirmish());
        assert_eq!(app.game.skirmish_bots().len(), 2);
        assert_eq!(app.screen, Screen::InGame);
        assert_eq!(app.menu.pane, MenuPane::Root);
    }

    fn armed_arena_assets() -> MemoryAssets {
        let mut assets = arena_assets();
        assets.insert(
            "sound/weapons/pl_gun3.wav",
            crate::audio::fixtures::synthetic_wav(44_100),
        );
        assets
    }

    #[test]
    fn skirmish_player_and_bot_shots_reach_the_mixer_as_nonzero_pcm() {
        for human_fires in [true, false] {
            let assets = armed_arena_assets();
            let mut app = started(&assets, 0);
            let human = app.game.player_entity().id();
            let bots = app.game.skirmish_bots();
            let mut heard = false;
            for _ in 0..500 {
                app.input.attack = human_fires;
                app.tick_game(ohl_engine::TICK_SECONDS);
                let mut mixer = app.audio.mixer().lock().expect("lock mixer");
                let owner_is_playing = if human_fires {
                    mixer.is_playing(human, ChannelClass::Weapon)
                } else {
                    bots.iter()
                        .any(|bot| mixer.is_playing(bot.id(), ChannelClass::Weapon))
                };
                if owner_is_playing {
                    let mut pcm = [0.0; 512];
                    mixer.render(&mut pcm);
                    assert!(pcm.iter().any(|sample| sample.abs() > 0.0001));
                    heard = true;
                    break;
                }
            }
            assert!(heard, "skirmish gunfire reaches the window's audio runtime");
        }
    }

    #[test]
    fn missing_weapon_samples_do_not_interrupt_a_skirmish() {
        let assets = arena_assets();
        let mut app = started(&assets, 0);
        for _ in 0..100 {
            app.input.attack = true;
            app.tick_game(ohl_engine::TICK_SECONDS);
        }
        assert!(app.game.is_skirmish());
        assert_eq!(
            app.audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            0
        );
    }

    #[test]
    fn skirmish_player_and_bot_pickups_reach_the_mixer() {
        let mut assets = arena_assets();
        let mut extra = String::new();
        for (x, y) in CORNERS {
            let _ = write!(
                extra,
                "{{\n\"classname\" \"weapon_shotgun\"\n\"origin\" \"{x} {y} 36\"\n}}\n"
            );
        }
        assets.insert(
            &format!("maps/{AI_MAP}.bsp"),
            deathmatch_room_bsp(&CORNERS, false, &extra),
        );
        assets.insert(
            "sound/items/gunpickup2.wav",
            crate::audio::fixtures::synthetic_wav(44_100),
        );
        let mut app = started(&assets, 0);
        app.tick_game(ohl_engine::TICK_SECONDS);
        let mut mixer = app.audio.mixer().lock().expect("lock mixer");
        assert!(mixer.is_playing(app.game.player_entity().id(), ChannelClass::Item));
        for bot in app.game.skirmish_bots() {
            assert!(mixer.is_playing(bot.id(), ChannelClass::Item));
        }
        let mut pcm = [0.0; 512];
        mixer.render(&mut pcm);
        assert!(pcm.iter().any(|sample| sample.abs() > 0.0001));
    }

    #[test]
    fn the_deathmatch_maps_are_found_among_the_published_ones() {
        let mut assets = arena_assets();
        assets.insert(
            "maps/ohltest_single.bsp",
            ohl_engine::test_support::synthetic_map_bsp(),
        );
        let found = crate::skirmish::deathmatch_maps(
            &assets,
            &[
                "ohltest_single".to_string(),
                AI_MAP.to_string(),
                "ohltest_missing".to_string(),
            ],
        );
        assert_eq!(found, [AI_MAP]);
    }

    #[test]
    fn a_frag_reaches_the_kill_feed_and_the_frag_counter() {
        let assets = arena_assets();
        let mut app = started(&assets, 0);
        let bot = app.game.skirmish_bots()[0];
        let player = app.game.player_entity();
        queue_engine_damage_from(&mut app.game, bot, player, 500.0);
        app.tick_game(CAPTURE_STEP);
        assert_eq!(app.hud.frags, Some(1));
        assert_eq!(app.hud.kill_feed.len(), 1);
        assert!(app.hud.kill_feed[0].local_involved);
        assert!(
            app.hud.kill_feed[0]
                .text
                .starts_with(ohl_engine::skirmish::HUMAN_NAME)
        );
    }

    #[test]
    fn dying_shows_how_to_respawn() {
        let assets = arena_assets();
        let mut app = started(&assets, 0);
        let bot = app.game.skirmish_bots()[0];
        let player = app.game.player_entity();
        queue_engine_damage_from(&mut app.game, player, bot, 500.0);
        app.tick_game(CAPTURE_STEP);
        assert_eq!(app.hud.center_notice.as_deref(), Some("You were fragged"));
        for _ in 0..90 {
            app.tick_game(CAPTURE_STEP);
        }
        assert_eq!(
            app.hud.center_notice.as_deref(),
            Some("Press fire to respawn")
        );
    }

    #[test]
    fn the_window_returns_to_the_menu_after_the_intermission() {
        let assets = arena_assets();
        let mut app = started(&assets, 1);
        let bot = app.game.skirmish_bots()[0];
        let player = app.game.player_entity();
        queue_engine_damage_from(&mut app.game, bot, player, 500.0);
        app.tick_game(CAPTURE_STEP);
        assert_eq!(app.screen, Screen::InGame, "the scoreboard holds first");
        assert!(
            app.hud
                .center_notice
                .as_deref()
                .is_some_and(|notice| notice.contains("wins"))
        );
        let mut waited = 0.0;
        while app.screen == Screen::InGame
            && waited < ohl_engine::skirmish::INTERMISSION_SECONDS * 2.0
        {
            app.tick_game(0.1);
            waited += 0.1;
        }
        assert_eq!(app.screen, Screen::MainMenu);
        assert!(waited >= ohl_engine::skirmish::INTERMISSION_SECONDS - 0.2);
    }

    #[test]
    fn a_skirmish_is_never_written_to_a_save_slot() {
        let assets = arena_assets();
        let saves = tempfile::tempdir().expect("a temporary save directory");
        let mut app = started(&assets, 0);
        app.saves = Some(ohl_save::SaveSlot::new(saves.path().to_path_buf()));
        assert!(!app.write_slot(ohl_save::QUICKSAVE_SLOT_NAME));
    }

    /// Presses or releases `code` through the player's bindings, as the
    /// window does. Returns whether the key is bound at all.
    fn key(app: &mut App<'_>, code: KeyCode, pressed: bool, repeat: bool) -> bool {
        let Some(action) = app.menu.options.controls.bindings.action_for_key(code) else {
            return false;
        };
        app.handle_action(action, pressed, repeat);
        true
    }

    #[test]
    fn the_weapon_keys_reach_the_input_and_clear_after_one_frame() {
        let assets = arena_assets();
        let mut app = started(&assets, 0);
        assert!(key(&mut app, KeyCode::Digit3, true, false));
        assert!(key(&mut app, KeyCode::KeyR, true, false));
        assert!(key(&mut app, KeyCode::KeyF, true, false));
        assert!(key(&mut app, KeyCode::Tab, true, false));
        assert!(!key(&mut app, KeyCode::KeyZ, true, false), "Z is unbound");
        assert_eq!(app.input.select_slot, Some(3));
        assert!(app.input.reload && app.input.flashlight_pressed);
        assert!(app.scoreboard_held);
        // A key repeat is not a second press.
        app.input.reload = false;
        assert!(key(&mut app, KeyCode::KeyR, true, true));
        assert!(!app.input.reload);
        app.input.reload = true;
        app.tick_game(CAPTURE_STEP);
        assert_eq!(app.input.select_slot, None);
        assert!(!app.input.reload && !app.input.flashlight_pressed);
        assert!(key(&mut app, KeyCode::Tab, false, false));
        assert!(!app.scoreboard_held);
    }

    /// A click pressed and released inside one frame (a quick tap, or any
    /// synthetic click) still fires one shot, and then fire is up again.
    #[test]
    fn a_click_shorter_than_a_frame_still_fires_once() {
        let assets = arena_assets();
        let mut app = started(&assets, 0);
        app.tick_game(CAPTURE_STEP);
        let full = app.hud.clip_ammo;
        app.handle_action(Action::Attack, true, false);
        app.handle_action(Action::Attack, false, false);
        app.tick_game(CAPTURE_STEP);
        assert_eq!(app.hud.clip_ammo, full.map(|clip| clip - 1), "one shot");
        assert!(!app.input.attack, "the released button is up again");
        for _ in 0..60 {
            app.tick_game(CAPTURE_STEP);
        }
        assert_eq!(app.hud.clip_ammo, full.map(|clip| clip - 1), "and only one");

        app.handle_action(Action::Attack2, true, false);
        app.tick_game(CAPTURE_STEP);
        assert!(app.input.attack2, "a held button stays down");
        app.release_movement();
        assert!(!app.input.attack2 && !app.is_held(Action::Attack2));
    }
}

/// The window's side of the menu: the options reaching input, audio and
/// the HUD, the load pane, `Escape` and the panes that only say "not yet".
/// No window is opened; every test drives [`App`] directly.
#[cfg(test)]
mod menu_window_tests {
    use super::*;
    use ohl_engine::MemoryAssets;
    use ohl_engine::test_support::{SYNTHETIC_MAP, synthetic_map_bsp};

    fn assets() -> MemoryAssets {
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), synthetic_map_bsp());
        assets
    }

    fn window(assets: &MemoryAssets, screen: Screen) -> App<'_> {
        let game =
            Game::load(assets as &dyn AssetSource, SYNTHETIC_MAP).expect("the synthetic map loads");
        App::new(
            game,
            assets,
            AudioRuntime::silent(),
            screen,
            GameConfig::default(),
        )
    }

    /// Presses or releases `code` through the player's bindings, as the
    /// window does.
    fn key(app: &mut App<'_>, code: KeyCode, pressed: bool) {
        if let Some(action) = app.menu.options.controls.bindings.action_for_key(code) {
            app.handle_action(action, pressed, false);
        }
    }

    #[test]
    fn the_menus_default_field_of_view_is_the_cameras_own() {
        let camera = ohl_render::FreeFlyCamera::default();
        assert!((ohl_ui::menu::DEFAULT_FOV - camera.fov_y_degrees).abs() < f32::EPSILON);
        assert!(ohl_engine::FOV_RANGE.contains(ohl_ui::menu::FOV_RANGE.start()));
        assert!(ohl_engine::FOV_RANGE.contains(ohl_ui::menu::FOV_RANGE.end()));
    }

    #[test]
    fn a_rebound_key_drives_its_new_action_and_the_old_key_nothing() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        app.menu
            .options
            .controls
            .bindings
            .bind(Action::Jump, Binding::Key(KeyCode::KeyJ));
        key(&mut app, KeyCode::Space, true);
        assert!(!app.input.jump);
        key(&mut app, KeyCode::KeyJ, true);
        assert!(app.input.jump);
        assert_eq!(app.input.up, 1);
        key(&mut app, KeyCode::KeyJ, false);
        assert!(!app.input.jump);
        assert_eq!(app.input.up, 0);
    }

    #[test]
    fn opposite_movement_keys_cancel_and_releasing_one_leaves_the_other() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        key(&mut app, KeyCode::KeyW, true);
        assert_eq!(app.input.forward, 1);
        key(&mut app, KeyCode::KeyS, true);
        assert_eq!(app.input.forward, 0);
        key(&mut app, KeyCode::KeyW, false);
        assert_eq!(app.input.forward, -1);
        key(&mut app, KeyCode::KeyA, true);
        assert_eq!(app.input.right, -1);
        app.release_movement();
        assert_eq!((app.input.forward, app.input.right), (0, 0));
        key(&mut app, KeyCode::KeyD, true);
        assert_eq!(app.input.right, 1, "nothing stays held after a release");
    }

    #[test]
    fn use_is_one_press_per_key_press() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        key(&mut app, KeyCode::KeyE, true);
        assert!(app.input.use_pressed && app.input.use_held);
        app.tick_game(CAPTURE_STEP);
        key(&mut app, KeyCode::KeyE, true);
        assert!(!app.input.use_pressed, "still held is not a new press");
        key(&mut app, KeyCode::KeyE, false);
        assert!(!app.input.use_held);
    }

    #[test]
    fn mouse_motion_follows_the_sensitivity_and_inversion_settings() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        app.add_mouse_motion(10.0, 10.0);
        assert_eq!(
            app.input.mouse_delta,
            (10.0, 10.0),
            "the default is unscaled"
        );
        app.input.mouse_delta = (0.0, 0.0);
        app.menu.options.controls.sensitivity = 6.0;
        app.menu.options.controls.invert_mouse = true;
        app.add_mouse_motion(10.0, 10.0);
        assert_eq!(app.input.mouse_delta, (20.0, -20.0));
    }

    #[test]
    fn escape_pauses_steps_back_through_the_menu_and_resumes() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        app.escape();
        assert_eq!(app.screen, Screen::Pause);
        assert_eq!(app.menu.pane, MenuPane::Root);
        app.menu.open(MenuPane::Options);
        app.escape();
        assert_eq!(app.screen, Screen::Pause);
        assert_eq!(app.menu.pane, MenuPane::Root);
        app.escape();
        assert_eq!(app.screen, Screen::InGame);
    }

    #[test]
    fn escape_on_the_main_menus_root_does_not_quit() {
        let assets = assets();
        let mut app = window(&assets, Screen::MainMenu);
        app.menu.open(MenuPane::SinglePlayer);
        app.menu.open(MenuPane::NewGame);
        app.escape();
        assert_eq!(app.menu.pane, MenuPane::SinglePlayer);
        app.escape();
        app.escape();
        assert_eq!(app.screen, Screen::MainMenu);
        assert_eq!(app.menu.pane, MenuPane::Root);
        assert!(!app.quit_requested);
    }

    #[test]
    fn the_load_pane_lists_saves_and_loading_one_starts_playing() {
        let saves = tempfile::tempdir().expect("a temporary save directory");
        let assets = assets();
        let mut app = window(&assets, Screen::MainMenu);
        app.saves = Some(ohl_save::SaveSlot::new(saves.path()));
        assert!(app.quicksave());
        app.menu.open(MenuPane::SinglePlayer);
        app.menu.open(MenuPane::LoadGame);
        app.refresh_menu_data();
        assert_eq!(app.save_entries.len(), 1);
        let entry = app.save_entries[0].clone();
        assert_eq!(entry.slot, ohl_save::QUICKSAVE_SLOT_NAME);
        assert!(entry.detail.starts_with("Quick save"), "{}", entry.detail);
        assert!(entry.detail.ends_with("just now"), "{}", entry.detail);

        app.handle_menu_actions(vec![MenuAction::LoadSave(entry.slot)]);
        assert_eq!(app.screen, Screen::InGame);
        assert_eq!(app.menu.pane, MenuPane::Root);
    }

    #[test]
    fn a_save_that_cannot_be_loaded_leaves_the_menu_up_with_a_notice() {
        let saves = tempfile::tempdir().expect("a temporary save directory");
        let assets = assets();
        let mut app = window(&assets, Screen::MainMenu);
        app.saves = Some(ohl_save::SaveSlot::new(saves.path()));
        app.handle_menu_actions(vec![MenuAction::LoadSave("missing".to_owned())]);
        assert_eq!(app.screen, Screen::MainMenu);
        assert!(app.menu.notice().is_some());
    }

    #[test]
    fn saving_from_the_pause_menu_says_so() {
        let saves = tempfile::tempdir().expect("a temporary save directory");
        let assets = assets();
        let mut app = window(&assets, Screen::Pause);
        app.handle_menu_actions(vec![MenuAction::SaveGame]);
        assert_eq!(app.menu.notice(), Some("The game could not be saved."));
        app.saves = Some(ohl_save::SaveSlot::new(saves.path()));
        app.handle_menu_actions(vec![MenuAction::SaveGame]);
        assert_eq!(app.menu.notice(), Some("Game saved."));
    }

    #[test]
    fn switching_autosave_off_skips_the_level_change_save() {
        let saves = tempfile::tempdir().expect("a temporary save directory");
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        let slot = ohl_save::SaveSlot::new(saves.path());
        app.saves = Some(slot.clone());
        app.menu.options.gameplay.autosave = false;
        app.autosave();
        let listed = |slot: &ohl_save::SaveSlot| {
            slot.list(&ohl_save::Limits::default())
                .expect("the save directory lists")
                .len()
        };
        assert_eq!(listed(&slot), 0);
        app.menu.options.gameplay.autosave = true;
        app.autosave();
        assert_eq!(listed(&slot), 1);
    }

    #[test]
    fn the_crosshair_option_reaches_the_hud() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        app.menu.options.gameplay.crosshair = false;
        app.handle_menu_actions(vec![MenuAction::OptionsChanged]);
        assert!(!app.hud.show_crosshair);
        app.hud = HudState::default();
        app.tick_game(CAPTURE_STEP);
        assert!(!app.hud.show_crosshair, "a fresh HUD takes the option too");
    }

    #[test]
    fn closing_the_console_option_closes_an_open_console() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        app.console.set_open(true);
        app.menu.options.gameplay.console = false;
        app.handle_menu_actions(vec![MenuAction::OptionsChanged]);
        assert!(!app.console.is_open());
    }

    #[test]
    fn returning_to_the_main_menu_resets_the_menu() {
        let assets = assets();
        let mut app = window(&assets, Screen::Pause);
        app.menu.open(MenuPane::Options);
        app.handle_menu_actions(vec![MenuAction::ReturnToMainMenu]);
        assert_eq!(app.screen, Screen::MainMenu);
        assert_eq!(app.menu.pane, MenuPane::Root);
    }

    #[test]
    fn lan_play_says_it_is_not_available_yet() {
        let assets = assets();
        let mut app = window(&assets, Screen::MainMenu);
        app.handle_menu_actions(vec![MenuAction::HostLanServer(
            ohl_ui::menu::LanServerSettings::default(),
        )]);
        assert!(
            app.menu
                .notice()
                .is_some_and(|text| text.contains("not available"))
        );
        app.handle_menu_actions(vec![MenuAction::JoinLanServer {
            address: "192.0.2.1".to_owned(),
        }]);
        assert!(
            app.menu
                .notice()
                .is_some_and(|text| text.contains("not available"))
        );
        assert_eq!(app.screen, Screen::MainMenu);
    }

    #[test]
    fn the_performance_overlay_key_toggles_the_saved_option() {
        let assets = assets();
        let mut app = window(&assets, Screen::InGame);
        key(&mut app, KeyCode::KeyP, true);
        assert!(app.menu.options.video.performance_overlay);
        assert!(app.settings_dirty);
        key(&mut app, KeyCode::KeyP, false);
        key(&mut app, KeyCode::KeyP, true);
        assert!(!app.menu.options.video.performance_overlay);
    }

    #[test]
    fn changed_settings_are_written_to_the_settings_file() {
        let directory = tempfile::tempdir().expect("a temporary settings directory");
        let path = directory.path().join("settings.cfg");
        let assets = assets();
        let mut app = window(&assets, Screen::MainMenu);
        app.settings_path = Some(path.clone());
        app.save_settings();
        assert!(!path.exists(), "nothing changed, so nothing is written");
        app.menu.options.audio.effects = 0.5;
        app.handle_menu_actions(vec![MenuAction::OptionsChanged]);
        app.save_settings();
        assert_eq!(crate::settings::load(&path), app.menu.options);
        assert!(!app.settings_dirty);
    }

    #[test]
    fn the_level_select_marks_levels_missing_from_the_payload() {
        let mut published = vec![
            ohl_campaign::STARTMAP.to_owned(),
            ohl_campaign::TRAINMAP.to_owned(),
        ];
        published.sort();
        let chapters = menu_chapters(&published);
        assert_eq!(chapters.len(), ohl_campaign::CHAPTERS.len() + 1);
        let training = &chapters[0];
        assert_eq!(training.title, "Hazard Course");
        assert_eq!(training.levels[0].map, ohl_campaign::TRAINMAP);
        assert!(training.levels[0].available);
        assert!(!training.levels[1].available);
        assert_eq!(chapters[1].levels[0].map, ohl_campaign::STARTMAP);
        assert!(chapters[1].levels[0].available);

        let unknown = menu_chapters(&[]);
        assert!(
            unknown
                .iter()
                .flat_map(|chapter| &chapter.levels)
                .all(|level| level.available),
            "with no listing every level is offered"
        );
    }

    #[test]
    fn save_ages_read_naturally() {
        assert_eq!(age_text(5), "just now");
        assert_eq!(age_text(150), "2 min ago");
        assert_eq!(age_text(7_200), "2 h ago");
        assert_eq!(age_text(90_000), "yesterday");
        assert_eq!(age_text(3 * 86_400), "3 days ago");
    }
}

#[cfg(test)]
mod guard_entry_view_tests {
    use super::*;
    use crate::route_planner::guard_entry_view_fixture;
    use crate::script::Script;
    use ohl_engine::MemoryAssets;

    fn run(game: &mut Game, assets: &MemoryAssets, script: &Script) {
        let mut audio = AudioRuntime::silent();
        let mut log = crate::script_log::ScriptLog::new(game);
        let outcome = run_script_ticks(
            game,
            assets,
            &mut audio,
            script,
            &mut log,
            &TickOptions {
                script_log: false,
                follow_level_change: false,
                stop_on_level_change: false,
            },
        );
        assert_eq!(outcome.ticks, u64::try_from(script.len()).unwrap());
        assert!(!outcome.followed_level_change && !outcome.ended_section);
    }

    #[test]
    fn guard_entry_view_headless_exported_route() {
        guard_entry_view_fixture::exported_route(run);
    }

    #[test]
    fn guard_entry_view_headless_same_tick_and_boundaries() {
        guard_entry_view_fixture::boundaries(run);
    }
    #[test]
    fn guard_entry_view_followed_map_starts_a_new_interval() {
        use ohl_engine::test_support::{
            PLAN_SCRIPTED_MONSTER_MODEL, ai_room_bsp, entity_block, script_room_entities,
        };
        const NEXT: &str = "ohlguardviewnext";
        let extra = format!(
            "{}{}",
            entity_block(
                "trigger_auto",
                [0.0; 3],
                0.0,
                &[("target", "exit"), ("delay", "3.5")]
            ),
            entity_block(
                "trigger_changelevel",
                [220.0, 220.0, 40.0],
                0.0,
                &[("targetname", "exit"), ("map", NEXT)]
            )
        );
        let build = || {
            let (mut assets, game) = guard_entry_view_fixture::fixture_with_exit(0.0, 0.0, &extra);
            let enemy = entity_block(
                "monster_houndeye",
                [160.0, 0.0, 0.0],
                0.0,
                &[("model", PLAN_SCRIPTED_MONSTER_MODEL)],
            );
            let entities = script_room_entities([0.0, 0.0, 40.0], &enemy).replacen(
                "\"angle\" \"0\"",
                "\"angle\" \"135\"",
                1,
            );
            assets.insert(&format!("maps/{NEXT}.bsp"), ai_room_bsp(&entities, false));
            (assets, game)
        };
        let drive = |game: &mut Game, assets: &MemoryAssets, text: &str, stop| {
            let mut audio = AudioRuntime::silent();
            let mut log = crate::script_log::ScriptLog::new(game);
            run_script_ticks(
                game,
                assets,
                &mut audio,
                &Script::parse(text.as_bytes()).unwrap(),
                &mut log,
                &TickOptions {
                    script_log: false,
                    follow_level_change: true,
                    stop_on_level_change: stop,
                },
            )
        };
        let (assets, mut arrival) = build();
        let outcome = drive(&mut arrival, &assets, "90 guard\n", true);
        assert!(outcome.followed_level_change && outcome.ticks < 90);
        assert_eq!(arrival.map(), NEXT);
        assert!(
            (arrival.camera().yaw - 135.0).abs() < 0.01,
            "actual destination spawn differs from old entry"
        );

        let (assets, mut game) = build();
        let outcome = drive(
            &mut game,
            &assets,
            "90 guard\n1 look 0 30\n1 look 0 15\n",
            false,
        );
        assert!(outcome.followed_level_change && !outcome.ended_section);
        assert_eq!(outcome.ticks, 92);
        assert_eq!(game.map(), NEXT);
        assert!(
            game.player_health() > 0.0 && game.weapon_fired_count() > 0,
            "new-map Guard actually shoots after the real transition"
        );
        assert!(
            (game.camera().yaw - 180.0).abs() < 0.01,
            "new Guard captures destination entry once even while accumulated followed remains true"
        );
        let outcome = drive(&mut game, &assets, "1 look 0 15\n", false);
        assert_eq!(outcome.ticks, 1);
        assert!(
            (game.camera().yaw - 195.0).abs() < 0.01,
            "next invocation has no old interval"
        );
    }
}
