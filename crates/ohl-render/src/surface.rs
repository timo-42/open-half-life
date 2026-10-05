//! Windowed presentation: surface configuration and resize handling.

use crate::error::{RenderError, Result};
use crate::gpu::GpuContext;

/// A configured swap chain for a window surface.
pub struct WindowSurface<'window> {
    surface: wgpu::Surface<'window>,
    configuration: wgpu::SurfaceConfiguration,
    /// The present modes the adapter offers for this surface, for
    /// [`Self::set_vsync`].
    present_modes: Vec<wgpu::PresentMode>,
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
            present_modes: capabilities.present_modes,
        })
    }

    /// Switches vertical sync on (`Fifo`, the default) or off (`Immediate`,
    /// else `Mailbox`, whichever the surface offers first) and reconfigures
    /// the swap chain when that changes the present mode. A surface that
    /// offers neither keeps presenting in its current mode.
    pub fn set_vsync(&mut self, context: &GpuContext, vsync: bool) {
        let Some(mode) = vsync_present_mode(&self.present_modes, vsync) else {
            return;
        };
        if self.configuration.present_mode != mode {
            self.configuration.present_mode = mode;
            self.surface.configure(&context.device, &self.configuration);
        }
    }

    /// Whether the swap chain currently waits for vertical blank.
    #[must_use]
    pub fn vsync(&self) -> bool {
        matches!(
            self.configuration.present_mode,
            wgpu::PresentMode::Fifo | wgpu::PresentMode::FifoRelaxed
        )
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

/// The present mode [`WindowSurface::set_vsync`] picks from `offered`, or
/// `None` when the surface offers none that matches the request.
fn vsync_present_mode(offered: &[wgpu::PresentMode], vsync: bool) -> Option<wgpu::PresentMode> {
    let preference: &[wgpu::PresentMode] = if vsync {
        &[wgpu::PresentMode::Fifo]
    } else {
        &[wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox]
    };
    preference
        .iter()
        .copied()
        .find(|mode| offered.contains(mode))
}

#[cfg(test)]
mod tests {
    use super::vsync_present_mode;
    use wgpu::PresentMode;

    #[test]
    fn vsync_off_prefers_immediate_then_mailbox() {
        let all = [
            PresentMode::Fifo,
            PresentMode::Mailbox,
            PresentMode::Immediate,
        ];
        assert_eq!(vsync_present_mode(&all, true), Some(PresentMode::Fifo));
        assert_eq!(
            vsync_present_mode(&all, false),
            Some(PresentMode::Immediate)
        );
        assert_eq!(
            vsync_present_mode(&[PresentMode::Fifo, PresentMode::Mailbox], false),
            Some(PresentMode::Mailbox)
        );
    }

    #[test]
    fn a_surface_offering_only_fifo_cannot_turn_vsync_off() {
        assert_eq!(vsync_present_mode(&[PresentMode::Fifo], false), None);
        assert_eq!(vsync_present_mode(&[PresentMode::Immediate], true), None);
    }
}
