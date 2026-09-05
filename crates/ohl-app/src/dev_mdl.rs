//! The development-only `--dev-mdl` studio-model viewer.
//!
//! Like [`crate::dev_bsp`], this module exists behind the `dev-tools` cargo
//! feature (off by default, so it is absent from release builds) purely so a
//! developer can look at a model while the renderer is being built. It loads
//! an `.mdl` straight off disk and therefore **bypasses the media pipeline
//! entirely**: no ISO validation, no import, no cache, no VFS.
//!
//! The model orbits in front of the camera, its animation plays at the
//! sequence's own frame rate, and `[` / `]` step through the sequence list.
//! Passing `--dev-bsp` as well loads the map too and places the model at the
//! map's player start, so model lighting can be compared against the world
//! it stands in.
//!
//! Logging policy is the same here as everywhere else in the project: no
//! media-derived string ever reaches a log line, including the paths the
//! developer passed on the command line.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ohl_formats::bsp30::{Bsp, Limits as BspParseLimits};
use ohl_render::{
    FreeFlyCamera, GpuContext, ModelInstance, MoveInput, StudioRenderer, WindowSurface,
    WorldRenderer, placement, wgpu,
};
use ohl_world::{StudioLimits, StudioModel, StudioPose, WorldBuildOptions, WorldModel};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// How often the frame-rate line is logged.
const FPS_INTERVAL: Duration = Duration::from_secs(2);

/// The initial window size in physical pixels.
const INITIAL_SIZE: (u32, u32) = (1280, 720);

/// How far in front of the camera a model with no map sits, in GoldSrc
/// units, and how fast it turns.
const ORBIT_DISTANCE: f32 = 80.0;
const ORBIT_DEGREES_PER_SECOND: f32 = 45.0;

/// The ambient level used when there is no map to sample lighting from.
const DEFAULT_AMBIENT: [f32; 3] = [0.45, 0.45, 0.45];

/// The directional key light's colour.
const KEY_LIGHT: [f32; 3] = [0.75, 0.75, 0.75];

/// Runs the viewer until the window closes or Escape is pressed.
///
/// Returns a fixed, sanitized message on failure; the caller prints it as
/// is.
pub fn run(
    mdl_path: &Path,
    bsp_path: Option<&Path>,
    wad_paths: &[PathBuf],
) -> Result<(), &'static str> {
    let mdl_bytes = std::fs::read(mdl_path).map_err(|_| "the model file could not be read")?;
    let model = StudioModel::parse(&mdl_bytes, &StudioLimits::default())
        .map_err(|_| "the model file is not a studio model this build can render")?;
    if model.sequences.is_empty() {
        return Err("the model declares no animation sequences");
    }

    let world = match bsp_path {
        Some(path) => Some(load_world(path, wad_paths)?),
        None => None,
    };

    // Deliberately no counts, names, sizes or paths: the project's logging
    // policy keeps every media-derived value out of diagnostics.
    tracing::info!("development model viewer loaded");

    let event_loop = EventLoop::new().map_err(|_| "no window system is available")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    // With a map, stand at its player start and put the model there too;
    // without one, orbit the model in front of a default camera.
    let (camera, origin) = match world.as_ref() {
        Some(world) => {
            let camera = world
                .spawn
                .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
            let origin = world.spawn.map_or([0.0, 0.0, 0.0], |spawn| spawn.origin);
            (camera, origin)
        }
        None => (FreeFlyCamera::default(), [ORBIT_DISTANCE, 0.0, 0.0]),
    };

    let mut app = Viewer {
        model,
        world,
        origin,
        camera,
        state: None,
        input: MoveInput::default(),
        sequence: 0,
        sequence_time: 0.0,
        yaw: 0.0,
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

/// Loads the optional map the model is placed in.
fn load_world(bsp_path: &Path, wad_paths: &[PathBuf]) -> Result<WorldModel, &'static str> {
    let bsp_bytes = std::fs::read(bsp_path).map_err(|_| "the map file could not be read")?;
    let mut wad_bytes = Vec::with_capacity(wad_paths.len());
    for path in wad_paths {
        wad_bytes.push(std::fs::read(path).map_err(|_| "a texture package could not be read")?);
    }
    let wad_slices: Vec<&[u8]> = wad_bytes.iter().map(Vec::as_slice).collect();
    let limits = BspParseLimits::default();
    let bsp = Bsp::parse(&bsp_bytes, &limits).map_err(|_| "the map file is not a BSP v30 map")?;
    WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &wad_slices,
            limits,
            ..WorldBuildOptions::default()
        },
    )
    .map_err(|_| "the map could not be turned into a renderable world")
}

/// The window, GPU context and renderers, created together once winit hands
/// the application a display connection.
struct Active {
    window: Arc<Window>,
    context: GpuContext,
    surface: WindowSurface<'static>,
    world_renderer: Option<WorldRenderer>,
    studio_renderer: StudioRenderer,
}

struct Viewer {
    model: StudioModel,
    world: Option<WorldModel>,
    origin: [f32; 3],
    camera: FreeFlyCamera,
    state: Option<Active>,
    input: MoveInput,
    sequence: usize,
    sequence_time: f32,
    yaw: f32,
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
            KeyCode::KeyW => self.input.forward = value,
            KeyCode::KeyS => self.input.forward = -value,
            KeyCode::KeyD => self.input.right = value,
            KeyCode::KeyA => self.input.right = -value,
            KeyCode::Space => self.input.up = value,
            KeyCode::ControlLeft => self.input.up = -value,
            KeyCode::ShiftLeft => self.input.fast = pressed,
            _ => {}
        }
    }

    /// Steps the sequence selection by `delta`, wrapping at both ends and
    /// restarting playback.
    fn cycle_sequence(&mut self, delta: isize) {
        let count = self.model.sequences.len();
        if count == 0 {
            return;
        }
        let current = isize::try_from(self.sequence).unwrap_or(0);
        let count_signed = isize::try_from(count).unwrap_or(1);
        let next = (current + delta).rem_euclid(count_signed);
        self.sequence = usize::try_from(next).unwrap_or(0);
        self.sequence_time = 0.0;
        // The index is a property of the model file, so it stays out of the
        // log; only the fact that the selection changed is reported.
        tracing::info!("animation sequence changed");
    }

    #[allow(clippy::too_many_lines)]
    fn draw(&mut self) {
        let Some(active) = self.state.as_mut() else {
            return;
        };
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        let seconds = delta.as_secs_f32();
        self.camera.update(self.input, seconds);
        self.sequence_time += seconds;
        if self.world.is_none() {
            self.yaw = (self.yaw + ORBIT_DEGREES_PER_SECOND * seconds).rem_euclid(360.0);
        }

        let Some(frame) = active.surface.acquire(&active.context) else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // World first, then models on top of the same depth buffer.
        let mut external_depth = None;
        if let (Some(world), Some(renderer)) = (self.world.as_ref(), active.world_renderer.as_mut())
        {
            renderer.render(
                &active.context,
                world,
                &self.camera,
                &view,
                active.surface.width(),
                active.surface.height(),
            );
            external_depth = renderer.depth_view();
        }

        let ambient = self
            .world
            .as_ref()
            .map_or(DEFAULT_AMBIENT, |world| world.ambient_at(self.origin));
        let pose = StudioPose::sample(&self.model, self.sequence, self.sequence_time)
            .unwrap_or_else(|_| StudioPose::bind(&self.model));
        let instance = ModelInstance {
            transform: placement(self.origin, self.yaw),
            pose: &pose,
            body: &[],
            skin: 0,
            ambient,
            light_direction: ModelInstance::default_light_direction(),
            light_color: KEY_LIGHT,
        };
        active.studio_renderer.render(
            &active.context,
            &self.model,
            &self.camera,
            std::slice::from_ref(&instance),
            &view,
            active.surface.width(),
            active.surface.height(),
            external_depth,
        );
        active.context.queue.present(frame);

        // Restart a non-looping sequence once it has played out, so the
        // viewer keeps showing motion.
        if let Some(sequence) = self.model.sequences.get(self.sequence) {
            let duration = sequence.duration();
            if duration > 0.0 && !sequence.is_looping() && self.sequence_time > duration {
                self.sequence_time = 0.0;
            }
        }

        self.frames += 1;
        let elapsed = now.saturating_duration_since(self.fps_window_start);
        if elapsed >= FPS_INTERVAL {
            let seconds = elapsed.as_secs_f32();
            #[allow(clippy::cast_precision_loss)]
            let fps = self.frames as f32 / seconds;
            // Frame rate is a property of this machine and this run, not of
            // the model, so it is safe to report.
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
            .with_title("Open Half-Life (development model viewer)")
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
        let mut world_renderer = None;
        if let Some(world) = self.world.as_ref() {
            let Ok(renderer) = WorldRenderer::new(&context, world, surface.format()) else {
                self.fail(event_loop, "the renderer could not be created");
                return;
            };
            world_renderer = Some(renderer);
        }
        let Ok(studio_renderer) = StudioRenderer::new(&context, &self.model, surface.format())
        else {
            self.fail(event_loop, "the model renderer could not be created");
            return;
        };

        self.last_frame = Instant::now();
        self.fps_window_start = self.last_frame;
        self.frames = 0;
        self.state = Some(Active {
            window,
            context,
            surface,
            world_renderer,
            studio_renderer,
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
                    if let Some(renderer) = active.world_renderer.as_mut() {
                        renderer.resize(&active.context, size.width, size.height);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;
                match code {
                    KeyCode::Escape => event_loop.exit(),
                    KeyCode::BracketLeft if pressed => self.cycle_sequence(-1),
                    KeyCode::BracketRight if pressed => self.cycle_sequence(1),
                    _ => self.set_axis(code, pressed),
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
        if let DeviceEvent::MouseMotion { delta } = event {
            #[allow(clippy::cast_possible_truncation)]
            self.camera
                .apply_mouse_delta(delta.0 as f32, delta.1 as f32);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(active) = self.state.as_ref() {
            active.window.request_redraw();
        }
    }
}
