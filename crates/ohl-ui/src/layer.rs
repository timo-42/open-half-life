//! Owns the egui context, the egui-wgpu renderer and (in windowed mode) the
//! egui-winit input translator, and ties the three together into a
//! begin/end-frame pair the host application drives once per frame.

use std::sync::Arc;

use egui_wgpu::ScreenDescriptor;
use winit::window::Window;

/// Where a [`UiLayer`] gets its input and presents its output.
enum Backend {
    /// Driven by real winit window events; input comes from
    /// [`UiLayer::handle_window_event`] and [`UiLayer::begin_frame`].
    Windowed {
        window: Arc<Window>,
        state: Box<egui_winit::State>,
    },
    /// Driven entirely by [`UiLayer::begin_frame_headless`]; used by the
    /// offscreen render test and any other host that has no winit window.
    Headless,
}

/// Converts winit's `f64` scale factor to the `f32` egui expects. HiDPI
/// scale factors are always small (well under `2^24`), so the narrowing
/// carries no meaningful precision loss.
#[allow(clippy::cast_possible_truncation)]
fn scale_factor_to_pixels_per_point(scale_factor: f64) -> f32 {
    scale_factor as f32
}

/// Converts a pixel extent (up to a few tens of thousands for any real
/// display) to egui points at `pixels_per_point`. `f32`'s 23-bit mantissa
/// covers this range exactly.
#[allow(clippy::cast_precision_loss)]
fn pixels_to_points(pixels: u32, pixels_per_point: f32) -> f32 {
    pixels as f32 / pixels_per_point
}

/// Owns everything needed to turn egui calls into pixels: the context, the
/// wgpu-backed renderer, and (in windowed mode) the winit input bridge.
pub struct UiLayer {
    context: egui::Context,
    renderer: egui_wgpu::Renderer,
    backend: Backend,
    pixels_per_point: f32,
}

impl UiLayer {
    /// Creates a layer bound to a real window. `color_format` must match the
    /// surface the caller will present into; `end_frame_and_render` draws
    /// straight onto whatever view it is given, so this only needs to match
    /// the target's format for egui's blending to look right.
    #[must_use]
    pub fn new_windowed(
        device: &wgpu::Device,
        window: Arc<Window>,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let context = egui::Context::default();
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(scale_factor_to_pixels_per_point(window.scale_factor())),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer =
            egui_wgpu::Renderer::new(device, color_format, egui_wgpu::RendererOptions::default());
        Self {
            context,
            renderer,
            backend: Backend::Windowed {
                window,
                state: Box::new(state),
            },
            pixels_per_point: 1.0,
        }
    }

    /// Creates a layer with no window, for offscreen rendering in tests.
    /// The caller drives frames with [`Self::begin_frame_headless`] instead
    /// of [`Self::begin_frame`].
    #[must_use]
    pub fn new_headless(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let context = egui::Context::default();
        let renderer =
            egui_wgpu::Renderer::new(device, color_format, egui_wgpu::RendererOptions::default());
        Self {
            context,
            renderer,
            backend: Backend::Headless,
            pixels_per_point: 1.0,
        }
    }

    /// The egui context, for drawing UI (console, HUD, menus) between
    /// [`Self::begin_frame`]/[`Self::begin_frame_headless`] and
    /// [`Self::end_frame_and_render`].
    #[must_use]
    pub fn context(&self) -> &egui::Context {
        &self.context
    }

    /// Feeds a winit window event to egui and reports whether egui consumed
    /// it (in which case the host should not also treat it as gameplay
    /// input). A no-op that returns `false` in headless mode.
    pub fn handle_window_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        let Backend::Windowed { window, state } = &mut self.backend else {
            return false;
        };
        state.on_window_event(window, event).consumed
    }

    /// Starts a new frame in windowed mode: pulls accumulated input from the
    /// winit bridge and begins the egui pass. Draw calls against
    /// [`Self::context`] follow; finish with [`Self::end_frame_and_render`].
    pub fn begin_frame(&mut self) {
        let Backend::Windowed { window, state } = &mut self.backend else {
            return;
        };
        let input = state.take_egui_input(window);
        self.pixels_per_point = scale_factor_to_pixels_per_point(window.scale_factor());
        self.context.begin_pass(input);
    }

    /// Starts a new frame with no window, for tests and other headless
    /// hosts. `size_in_pixels` and `pixels_per_point` stand in for what
    /// [`Self::begin_frame`] would otherwise read off the window.
    pub fn begin_frame_headless(&mut self, size_in_pixels: [u32; 2], pixels_per_point: f32) {
        self.pixels_per_point = pixels_per_point;
        let screen_rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                pixels_to_points(size_in_pixels[0], pixels_per_point),
                pixels_to_points(size_in_pixels[1], pixels_per_point),
            ),
        );
        self.context.begin_pass(egui::RawInput {
            screen_rect: Some(screen_rect),
            ..Default::default()
        });
    }

    /// Ends the current pass, tessellates, uploads buffers and records the
    /// draw calls for this frame's UI into `encoder`, targeting `view`. In
    /// windowed mode this also applies egui's requested platform output
    /// (cursor icon, clipboard, ...) to the window.
    pub fn end_frame_and_render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_in_pixels: [u32; 2],
    ) {
        let mut output = self.context.end_pass();

        if let Backend::Windowed { window, state } = &mut self.backend {
            state.handle_platform_output(window, output.platform_output);
        }

        // `TexturesDelta` asserts on drop that every entry was handled;
        // taking it up front and clearing it once we are done (rather than
        // leaving it inside `output`) satisfies that regardless of which
        // branch below returns early.
        let textures_delta = std::mem::take(&mut output.textures_delta);
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }

        let screen_descriptor = ScreenDescriptor {
            size_in_pixels,
            pixels_per_point: self.pixels_per_point,
        };
        let paint_jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let command_buffers =
            self.renderer
                .update_buffers(device, queue, encoder, &paint_jobs, &screen_descriptor);
        if !command_buffers.is_empty() {
            queue.submit(command_buffers);
        }

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl-ui egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer
                .render(&mut pass, &paint_jobs, &screen_descriptor);
        }

        for id in &textures_delta.free {
            self.renderer.free_texture(id);
        }
        // Every entry has now been applied to the renderer; satisfy
        // `TexturesDelta`'s must-be-handled invariant explicitly.
        let mut textures_delta = textures_delta;
        textures_delta.clear();
    }
}
