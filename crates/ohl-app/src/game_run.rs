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

use std::path::Path;
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

/// Everything the playable loop needs from the command line.
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
}

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
    if game.missing_model_count() > 0 {
        // Deliberately no count: it is derived from the map's own contents.
        tracing::info!("Some referenced models are not published in this payload; skipped.");
    }
    if !game.has_collision() {
        tracing::warn!("The map has no usable collision hulls; the camera flies instead.");
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
) {
    if follow && game.change_level(source, map, landmark).is_ok() {
        if script_log {
            tracing::info!("A level change was followed.");
        }
    } else {
        tracing::info!("A level change fired during capture; it was not followed.");
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

    if let Some(viewpoint) = args.viewpoint {
        game.set_viewpoint(viewpoint.position, viewpoint.pitch, viewpoint.yaw);
    } else if let Some(offset) = args.spawn_offset {
        let camera = game.camera();
        let position = [
            camera.position[0] + offset.position[0],
            camera.position[1] + offset.position[1],
            camera.position[2] + offset.position[2],
        ];
        let (pitch, yaw) = (camera.pitch + offset.pitch, camera.yaw + offset.yaw);
        game.set_viewpoint(position, pitch, yaw);
    }

    let mut log = crate::script_log::ScriptLog::new(game);
    for input in script.inputs() {
        for event in game.tick(CAPTURE_STEP, input) {
            if let GameEvent::LevelChange { map, landmark } = event {
                handle_level_change(
                    game,
                    source,
                    &map,
                    &landmark,
                    args.follow_level_change,
                    args.script_log,
                );
            }
        }
        if args.script_log {
            log.observe(game, CAPTURE_STEP);
        }
    }

    if args.script_log {
        tracing::info!("Scripted input finished.");
    }

    match args.screenshot {
        Some(path) => write_screenshot(game, path),
        None => Ok(()),
    }
}

/// Renders exactly one frame and writes it as a PNG. Shared by
/// [`run_scripted`]; [`capture`] renders once per advanced frame instead,
/// since a capture without a script advances the world's own animation one
/// tick at a time between renders.
fn write_screenshot(game: &mut Game, path: &Path) -> Result<(), &'static str> {
    let context = GpuContext::headless().map_err(|_| "no usable graphics adapter is available")?;
    let (width, height) = CAPTURE_SIZE;
    let target = OffscreenTarget::new(&context, width, height)
        .map_err(|_| "no offscreen target could be created")?;
    game.render(
        &context,
        RenderTarget {
            view: target.view(),
            width,
            height,
            format: OFFSCREEN_FORMAT,
        },
    )
    .map_err(|_| "the frame could not be rendered")?;
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

    if placed_at_monster {
        // Handled above; the ordinary viewpoint/spawn-offset chain below is
        // mutually exclusive with it (clap's own `requires` wiring already
        // keeps `--viewpoint`/`--spawn-offset` and
        // `--viewpoint-at-nearest-monster` from making sense together, so
        // this just documents that this branch takes priority).
    } else if let Some(viewpoint) = args.viewpoint {
        game.set_viewpoint(viewpoint.position, viewpoint.pitch, viewpoint.yaw);
    } else if let Some(offset) = args.spawn_offset {
        // Relative to wherever the map's own player start put the camera,
        // so a capture can be aimed without anyone having to know (or
        // record) a map's coordinates.
        let camera = game.camera();
        let position = [
            camera.position[0] + offset.position[0],
            camera.position[1] + offset.position[1],
            camera.position[2] + offset.position[2],
        ];
        let (pitch, yaw) = (camera.pitch + offset.pitch, camera.yaw + offset.yaw);
        game.set_viewpoint(position, pitch, yaw);
    }

    // `set_viewpoint` runs with noclip on (see its doc comment) so it
    // cannot push the camera clear of an accidental overlap the way
    // ordinary spawn placement does; surface that as a warning rather than
    // silently writing a meaningless frame. The message is a fixed string
    // with no map-derived data, per this module's logging policy.
    if (args.viewpoint.is_some() || args.spawn_offset.is_some() || placed_at_monster)
        && game.eye_is_in_solid()
    {
        tracing::warn!("Capture viewpoint starts inside solid geometry.");
    }

    for _ in 0..args.frames.max(1) {
        // The capture stands still: only the world's own animation (doors,
        // light styles, liquid turbulence, model sequences) advances.
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
        game.render(
            &context,
            RenderTarget {
                view: target.view(),
                width,
                height,
                format: OFFSCREEN_FORMAT,
            },
        )
        .map_err(|_| "the frame could not be rendered")?;
    }
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
