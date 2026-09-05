//! The development-only `--dev-bsp` viewer.
//!
//! This module exists behind the `dev-tools` cargo feature (off by default,
//! so it is absent from release builds) purely so a developer can look at a
//! map while the renderer is being built. It loads a `.bsp` straight off
//! disk and therefore **bypasses the media pipeline entirely**: no ISO
//! validation, no import, no cache, no VFS. It is not a supported way to run
//! the game and it will be removed once maps arrive through the real
//! pipeline.
//!
//! Logging policy is the same here as everywhere else in the project: no
//! media-derived string ever reaches a log line. That includes the paths the
//! developer passed on the command line, which are echoed nowhere.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_game::keyvalues::{self, Limits as KeyvalueLimits};
use ohl_game::registry::Registry;
use ohl_game::{Simulation, find_usable_within};
use ohl_physics::{CollisionModel, ControllerInput, PlayerController};
use ohl_render::{FreeFlyCamera, GpuContext, MoveInput, WindowSurface, WorldRenderer, wgpu};
use ohl_world::{WorldBuildOptions, WorldModel};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// How close (in GoldSrc units) the camera must be to a door or button for
/// `E` to use it, mirroring a comfortable interaction range.
const USE_RADIUS: f32 = 64.0;

/// How often the frame-rate line is logged.
const FPS_INTERVAL: Duration = Duration::from_secs(2);

/// The initial window size in physical pixels.
const INITIAL_SIZE: (u32, u32) = (1280, 720);

/// How far the mouse turns the walking player, in degrees per pixel. The
/// free-fly camera keeps its own copy of this.
const WALK_SENSITIVITY: f32 = 0.15;

/// Runs the viewer until the window closes or Escape is pressed.
///
/// Returns a fixed, sanitized message on failure; the caller prints it as
/// is.
pub fn run(bsp_path: &Path, wad_paths: &[PathBuf]) -> Result<(), &'static str> {
    let bsp_bytes = std::fs::read(bsp_path).map_err(|_| "the map file could not be read")?;
    let mut wad_bytes = Vec::with_capacity(wad_paths.len());
    for path in wad_paths {
        wad_bytes.push(std::fs::read(path).map_err(|_| "a texture package could not be read")?);
    }
    let wad_slices: Vec<&[u8]> = wad_bytes.iter().map(Vec::as_slice).collect();

    let limits = Limits::default();
    let bsp = Bsp::parse(&bsp_bytes, &limits).map_err(|_| "the map file is not a BSP v30 map")?;
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &wad_slices,
            limits,
        },
    )
    .map_err(|_| "the map could not be turned into a renderable world")?;

    // Deliberately no counts, names, sizes or paths: the project's logging
    // policy keeps every media-derived value out of diagnostics, and a map's
    // face/texture counts are media-derived.
    tracing::info!("development map loaded");

    // Collision hulls for the walking mode. A map whose hulls do not
    // validate is still worth looking at, so this failure only disables
    // walking.
    let collision = CollisionModel::from_bsp(&bsp, &limits).ok();
    if collision.is_none() {
        tracing::warn!("map has no usable collision hulls; walking mode is unavailable");
    }

    // Build the entity registry and its brush-model bounding boxes so
    // `func_door`/`func_button`/`func_plat` and friends can be driven by the
    // map logic simulation. Failure here is not fatal to the viewer: a map
    // with no (or malformed) entities still renders, it just has nothing to
    // `use`.
    let kv_limits = KeyvalueLimits::default();
    let mut model_bounds = BTreeMap::new();
    if let Ok(models) = bsp.models(&limits) {
        for (index, submodel) in models.iter().enumerate() {
            let Ok(index) = u32::try_from(index) else {
                continue;
            };
            model_bounds.insert(
                index,
                (
                    [
                        submodel.mins[0].get(),
                        submodel.mins[1].get(),
                        submodel.mins[2].get(),
                    ],
                    [
                        submodel.maxs[0].get(),
                        submodel.maxs[1].get(),
                        submodel.maxs[2].get(),
                    ],
                ),
            );
        }
    }
    let registry = bsp.entities(&limits).map_or_else(
        |_| Registry::build(&[], &BTreeMap::new(), &kv_limits),
        |entities| {
            let defs = keyvalues::parse_entities(&entities, &kv_limits);
            Registry::build(&defs, &model_bounds, &kv_limits)
        },
    );
    let simulation = Simulation::new();

    let event_loop = EventLoop::new().map_err(|_| "no window system is available")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let camera = model
        .spawn
        .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
    let controller = model.spawn.map_or_else(PlayerController::default, |spawn| {
        PlayerController::spawn_at(Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch)
    });
    let mut app = Viewer {
        model,
        camera,
        // Walking is the default when the map has hulls; V returns to the
        // free-fly camera, which stays available for looking at geometry.
        walking: collision.is_some(),
        collision,
        controller,
        walk_input: ControllerInput::default(),
        registry,
        simulation,
        use_pressed: false,
        key_e_down: false,
        state: None,
        input: MoveInput::default(),
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

/// The window, GPU context and renderer, created together once winit hands
/// the application a display connection.
struct Active {
    window: Arc<Window>,
    context: GpuContext,
    surface: WindowSurface<'static>,
    renderer: WorldRenderer,
}

struct Viewer {
    model: WorldModel,
    camera: FreeFlyCamera,
    /// The map's collision hulls, when it has usable ones.
    collision: Option<CollisionModel>,
    controller: PlayerController,
    walk_input: ControllerInput,
    /// Whether the walking player drives the camera (`V` toggles it).
    walking: bool,
    registry: Registry,
    simulation: Simulation,
    /// Set for one `draw` call when `E` was pressed since the last frame,
    /// so a held key does not fire "use" every frame.
    use_pressed: bool,
    /// Whether `E` was already down as of the last `KeyboardInput` event,
    /// so `use_pressed` only latches on the press edge.
    key_e_down: bool,
    state: Option<Active>,
    input: MoveInput,
    last_frame: Instant,
    fps_window_start: Instant,
    frames: u32,
    failure: Option<&'static str>,
}

impl Viewer {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: &'static str) {
        self.failure = Some(message);
        event_loop.exit();
    }

    fn set_axis(&mut self, key: KeyCode, pressed: bool) {
        let value = i8::from(pressed);
        match key {
            KeyCode::KeyW => {
                self.input.forward = value;
                self.walk_input.forward = value;
            }
            KeyCode::KeyS => {
                self.input.forward = -value;
                self.walk_input.forward = -value;
            }
            KeyCode::KeyD => {
                self.input.right = value;
                self.walk_input.right = value;
            }
            KeyCode::KeyA => {
                self.input.right = -value;
                self.walk_input.right = -value;
            }
            KeyCode::Space => {
                self.input.up = value;
                self.walk_input.up = value;
                self.walk_input.jump = pressed;
            }
            KeyCode::ControlLeft => {
                self.input.up = -value;
                self.walk_input.up = -value;
                self.walk_input.duck = pressed;
            }
            KeyCode::ShiftLeft => self.input.fast = pressed,
            _ => {}
        }
    }

    /// Handles the two mode keys: `N` toggles noclip for the walking player
    /// and `V` switches between walking and the free-fly camera.
    fn toggle_mode(&mut self, key: KeyCode) {
        match key {
            KeyCode::KeyN if self.collision.is_some() => {
                let noclip = self.controller.toggle_noclip();
                tracing::info!(noclip, "walking mode: noclip toggled");
            }
            KeyCode::KeyV if self.collision.is_some() => {
                self.walking = !self.walking;
                if self.walking {
                    // Resume walking from wherever the free-fly camera is
                    // looking, dropping the player to the floor from there.
                    self.controller.yaw = self.camera.yaw;
                    self.controller.pitch = self.camera.pitch;
                }
                tracing::info!(walking = self.walking, "camera mode changed");
            }
            _ => {}
        }
    }

    /// Advances whichever camera mode is active and returns the eye position
    /// the renderer should use.
    fn update_view(&mut self, seconds: f32) {
        let Some(collision) = self.collision.as_ref() else {
            self.camera.update(self.input, seconds);
            return;
        };
        if self.walking {
            self.controller.yaw = self.camera.yaw;
            self.controller.pitch = self.camera.pitch;
            self.controller
                .advance(collision, &self.walk_input, seconds);
            self.camera.position = self.controller.eye_position().to_array();
        } else {
            self.camera.update(self.input, seconds);
        }
    }

    fn draw(&mut self) {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        self.update_view(delta.as_secs_f32());

        if self.use_pressed {
            self.use_pressed = false;
            let position = Vec3::from_array(self.camera.position);
            if let Some(entity) = find_usable_within(&self.registry, position, USE_RADIUS) {
                let mut events = Vec::new();
                self.simulation
                    .use_entity(&mut self.registry, entity, None, &mut events);
                // Level-change destinations are map-derived strings, so
                // this deliberately drops them rather than logging; loading
                // the next map is not implemented in this development-only
                // viewer.
            }
        }
        // Deterministic, fixed-timestep map logic (doors, buttons,
        // platforms, multi_manager fan-out, triggers): advanced by the
        // real frame delta here since the viewer has no fixed-tick loop of
        // its own yet.
        let _ = self
            .simulation
            .tick(&mut self.registry, delta.as_secs_f32());

        let Some(active) = self.state.as_mut() else {
            return;
        };
        let Some(frame) = active.surface.acquire(&active.context) else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        active.renderer.render(
            &active.context,
            &self.model,
            &self.camera,
            &view,
            active.surface.width(),
            active.surface.height(),
        );
        active.context.queue.present(frame);

        self.frames += 1;
        let elapsed = now.saturating_duration_since(self.fps_window_start);
        if elapsed >= FPS_INTERVAL {
            let seconds = elapsed.as_secs_f32();
            #[allow(clippy::cast_precision_loss)]
            let fps = self.frames as f32 / seconds;
            // Frame rate is a property of this machine and this run, not of
            // the map, so it is safe to report; the triangle count would be
            // map-derived and is deliberately omitted.
            tracing::info!(
                frames = self.frames,
                fps = format_args!("{fps:.1}"),
                "frame rate"
            );
            self.frames = 0;
            self.fps_window_start = now;
        }
        active.window.request_redraw();
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Open Half-Life (development map viewer)")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                INITIAL_SIZE.0,
                INITIAL_SIZE.1,
            ));
        let Ok(window) = event_loop.create_window(attributes) else {
            self.fail(event_loop, "a window could not be created");
            return;
        };
        let window = Arc::new(window);
        // Locked grabbing is unsupported on some platforms; confined is the
        // documented fallback, and a failure to grab at all is not fatal.
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
        let Ok(renderer) = WorldRenderer::new(&context, &self.model, surface.format()) else {
            self.fail(event_loop, "the renderer could not be created");
            return;
        };

        self.last_frame = Instant::now();
        self.fps_window_start = self.last_frame;
        self.frames = 0;
        self.state = Some(Active {
            window,
            context,
            surface,
            renderer,
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(active) = self.state.as_mut() {
                    active
                        .surface
                        .resize(&active.context, size.width, size.height);
                    active
                        .renderer
                        .resize(&active.context, size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                if code == KeyCode::Escape {
                    event_loop.exit();
                    return;
                }
                let pressed = event.state == ElementState::Pressed;
                if pressed && !event.repeat {
                    self.toggle_mode(code);
                }
                if code == KeyCode::KeyE {
                    if pressed && !self.key_e_down {
                        self.use_pressed = true;
                    }
                    self.key_e_down = pressed;
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
        if let DeviceEvent::MouseMotion { delta } = event {
            #[allow(clippy::cast_possible_truncation)]
            let (delta_x, delta_y) = (delta.0 as f32, delta.1 as f32);
            self.camera.apply_mouse_delta(delta_x, delta_y);
            self.controller
                .apply_mouse_delta(delta_x, delta_y, WALK_SENSITIVITY);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(active) = self.state.as_ref() {
            active.window.request_redraw();
        }
    }
}
