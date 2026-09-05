//! Rendering into a texture and reading it back, with no window involved.
//!
//! This is the path the headless test uses, and it is also how future
//! screenshot or reference-image tooling will capture frames.

use crate::error::{RenderError, Result};
use crate::gpu::GpuContext;

/// wgpu requires buffer copy rows to be a multiple of this many bytes.
const COPY_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// The colour format offscreen frames are rendered and read back in.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// An offscreen colour target plus the staging buffer used to read it back.
pub struct OffscreenTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl OffscreenTarget {
    /// Creates a `width` x `height` RGBA8 render target.
    pub fn new(context: &GpuContext, width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(RenderError::UnsupportedSurface);
        }
        let texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ohl offscreen colour"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            texture,
            view,
            width,
            height,
        })
    }

    /// The colour attachment to render into.
    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Target width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Target height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Copies the rendered frame back to the CPU as tightly packed RGBA8.
    pub fn read_rgba(&self, context: &GpuContext) -> Result<Vec<u8>> {
        let unpadded_row = self.width * 4;
        let padded_row = unpadded_row.div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT;
        let buffer_size = u64::from(padded_row) * u64::from(self.height);
        let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl offscreen readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        context.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result.is_ok());
        });
        if context
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
        {
            return Err(RenderError::Readback);
        }
        if receiver.recv() != Ok(true) {
            return Err(RenderError::Readback);
        }

        let mapped = slice
            .get_mapped_range()
            .map_err(|_| RenderError::Readback)?;
        let mut pixels = Vec::with_capacity((unpadded_row * self.height) as usize);
        for row in 0..self.height as usize {
            let start = row * padded_row as usize;
            let end = start + unpadded_row as usize;
            let Some(chunk) = mapped.get(start..end) else {
                return Err(RenderError::Readback);
            };
            pixels.extend_from_slice(chunk);
        }
        drop(mapped);
        buffer.unmap();
        Ok(pixels)
    }
}
