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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_render::{FreeFlyCamera, GpuContext, MoveInput, WindowSurface, WorldRenderer, wgpu};
use ohl_world::{WorldBuildOptions, WorldModel};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// How often the frame-rate line is logged.
const FPS_INTERVAL: Duration = Duration::from_secs(2);

/// The initial window size in physical pixels.
const INITIAL_SIZE: (u32, u32) = (1280, 720);

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

    let event_loop = EventLoop::new().map_err(|_| "no window system is available")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let camera = model
        .spawn
        .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
    let mut app = Viewer {
        model,
        camera,
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

    fn draw(&mut self) {
        let Some(active) = self.state.as_mut() else {
            return;
        };
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;
        self.camera.update(self.input, delta.as_secs_f32());

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
                self.set_axis(code, event.state == ElementState::Pressed);
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
