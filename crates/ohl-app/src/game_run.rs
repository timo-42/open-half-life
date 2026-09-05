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

use ohl_engine::{AssetFsSource, Game, GameEvent, Input, RenderTarget};
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
}

/// Runs the playable loop, either headless (writing a PNG) or windowed.
///
/// Returns a fixed, sanitized message on failure; the caller prints it as
/// is.
pub fn run(args: &GameArgs<'_>) -> Result<(), &'static str> {
    let asset_fs = ohl_assets::AssetFs::mount_default(args.payload_files)
        .map_err(|_| "the payload directory could not be indexed")?;
    let source = AssetFsSource::new(asset_fs);
    let mut game = Game::load(&source, args.map).map_err(|_| {
        // The map name is media-derived, so the reason names the step, not
        // the asset.
        "the start map could not be loaded from the payload"
    })?;
    tracing::info!("Map loaded.");
    if game.missing_model_count() > 0 {
        // Deliberately no count: it is derived from the map's own contents.
        tracing::info!("Some referenced models are not published in this payload; skipped.");
    }
    if !game.has_collision() {
        tracing::warn!("The map has no usable collision hulls; the camera flies instead.");
    }

    match args.screenshot {
        Some(path) => capture(&mut game, args, path),
        None => windowed(game, &source),
    }
}

/// Renders `frames` frames offscreen and writes the last one as a PNG.
fn capture(game: &mut Game, args: &GameArgs<'_>, path: &Path) -> Result<(), &'static str> {
    let context = GpuContext::headless().map_err(|_| "no usable graphics adapter is available")?;
    let (width, height) = CAPTURE_SIZE;
    let target = OffscreenTarget::new(&context, width, height)
        .map_err(|_| "no offscreen target could be created")?;

    if let Some(viewpoint) = args.viewpoint {
        game.set_viewpoint(viewpoint.position, viewpoint.pitch, viewpoint.yaw);
    }

    for _ in 0..args.frames.max(1) {
        // The capture stands still: only the world's own animation (doors,
        // light styles, liquid turbulence, model sequences) advances.
        let events = game.tick(CAPTURE_STEP, &Input::default());
        for event in events {
            let GameEvent::LevelChange { .. } = event;
            tracing::info!("A level change fired during capture; it was not followed.");
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
fn windowed(game: Game, source: &AssetFsSource) -> Result<(), &'static str> {
    let event_loop = EventLoop::new().map_err(|_| "no window system is available")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        game,
        source,
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
            let GameEvent::LevelChange { map, landmark } = event;
            // Neither string is logged: both are map-derived.
            if self.game.change_level(self.source, &map, &landmark).is_ok() {
                tracing::info!("Level changed.");
            } else {
                tracing::warn!("The destination map is not published; staying here.");
            }
        }
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
                if code == KeyCode::KeyE {
                    if pressed && !self.key_use_down {
                        self.input.use_pressed = true;
                    }
                    self.key_use_down = pressed;
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
