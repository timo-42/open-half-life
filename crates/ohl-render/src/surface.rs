//! Windowed presentation: surface configuration and resize handling.

use crate::error::{RenderError, Result};
use crate::gpu::GpuContext;

/// A configured swap chain for a window surface.
pub struct WindowSurface<'window> {
    surface: wgpu::Surface<'window>,
    configuration: wgpu::SurfaceConfiguration,
}

impl<'window> WindowSurface<'window> {
    /// Configures `surface` for `width` x `height`, choosing the first
    /// supported non-sRGB format when one exists so the GoldSrc-style
    /// gamma-space composite needs no conversion, and otherwise falling back
    /// to the surface's preferred format.
    pub fn new(
        context: &GpuContext,
        surface: wgpu::Surface<'window>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let capabilities = surface.get_capabilities(&context.adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RenderError::UnsupportedSurface)?;
        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            *capabilities
                .present_modes
                .first()
                .ok_or(RenderError::UnsupportedSurface)?
        };
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .ok_or(RenderError::UnsupportedSurface)?;
        let configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: Vec::new(),
        };
        surface.configure(&context.device, &configuration);
        Ok(Self {
            surface,
            configuration,
        })
    }

    /// The configured colour format, which the pipeline must match.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.configuration.format
    }

    /// The configured width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.configuration.width
    }

    /// The configured height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.configuration.height
    }

    /// Reconfigures the swap chain after the window changed size.
    pub fn resize(&mut self, context: &GpuContext, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if self.configuration.width == width && self.configuration.height == height {
            return;
        }
        self.configuration.width = width;
        self.configuration.height = height;
        self.surface.configure(&context.device, &self.configuration);
    }

    /// Acquires the next frame, reconfiguring once and retrying if the swap
    /// chain went stale (a resize or display change wgpu noticed first).
    pub fn acquire(&mut self, context: &GpuContext) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture as Current;
        match self.surface.get_current_texture() {
            Current::Success(frame) | Current::Suboptimal(frame) => Some(frame),
            Current::Outdated | Current::Lost => {
                self.surface.configure(&context.device, &self.configuration);
                match self.surface.get_current_texture() {
                    Current::Success(frame) | Current::Suboptimal(frame) => Some(frame),
                    _ => None,
                }
            }
            Current::Timeout | Current::Occluded | Current::Validation => None,
        }
    }
}
