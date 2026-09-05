//! Instance, adapter, device and queue setup.
//!
//! Backend selection follows `PROMPT.md`: Vulkan on Linux and Windows, Metal
//! on macOS. If the preferred backend has no adapter the search widens to
//! `wgpu::Backends::PRIMARY` so a machine with only a secondary backend
//! (or a software Vulkan implementation exposed through a different slot)
//! still works. Every entry point returns [`RenderError::NoAdapter`] rather
//! than panicking when there is no GPU at all, which is the normal case on a
//! headless CI runner.

use crate::error::{RenderError, Result};

/// The backend this build prefers on the current platform.
#[must_use]
pub fn preferred_backends() -> wgpu::Backends {
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        wgpu::Backends::METAL
    } else {
        wgpu::Backends::VULKAN
    }
}

/// A logical device plus the queue that feeds it.
pub struct GpuContext {
    /// The wgpu instance the adapter came from.
    pub instance: wgpu::Instance,
    /// The selected adapter.
    pub adapter: wgpu::Adapter,
    /// The logical device.
    pub device: wgpu::Device,
    /// The device's command queue.
    pub queue: wgpu::Queue,
}

fn instance(backends: wgpu::Backends) -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
    })
}

fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ohl-render device"),
        required_features: wgpu::Features::empty(),
        // Downlevel defaults keep the renderer usable on software adapters
        // and older hardware; nothing here needs more.
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|_| RenderError::NoDevice)
}

impl GpuContext {
    /// Creates a context with no surface, for offscreen rendering.
    ///
    /// Returns [`RenderError::NoAdapter`] when the host has no usable
    /// adapter; callers on CI are expected to treat that as "skip", not as a
    /// failure.
    pub fn headless() -> Result<Self> {
        for backends in [preferred_backends(), wgpu::Backends::PRIMARY] {
            let instance = instance(backends);
            let Ok(adapter) =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::None,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                }))
            else {
                continue;
            };
            let (device, queue) = request_device(&adapter)?;
            return Ok(Self {
                instance,
                adapter,
                device,
                queue,
            });
        }
        Err(RenderError::NoAdapter)
    }

    /// Creates a context and a surface for `target`, which is anything wgpu
    /// accepts as a surface target (typically an `Arc<winit::window::Window>`).
    #[allow(clippy::needless_pass_by_value)]
    pub fn for_surface<'window>(
        target: impl Into<wgpu::SurfaceTarget<'window>> + Clone,
    ) -> Result<(Self, wgpu::Surface<'window>)> {
        let mut last = RenderError::NoAdapter;
        for backends in [preferred_backends(), wgpu::Backends::PRIMARY] {
            let instance = instance(backends);
            let Ok(surface) = instance.create_surface(target.clone()) else {
                last = RenderError::NoSurface;
                continue;
            };
            let Ok(adapter) =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: Some(&surface),
                    apply_limit_buckets: false,
                }))
            else {
                last = RenderError::NoAdapter;
                continue;
            };
            let (device, queue) = request_device(&adapter)?;
            return Ok((
                Self {
                    instance,
                    adapter,
                    device,
                    queue,
                },
                surface,
            ));
        }
        Err(last)
    }

    /// Blocks until every submitted command has completed.
    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

#[cfg(test)]
mod tests {
    use super::preferred_backends;

    #[test]
    fn preferred_backend_matches_the_platform_policy() {
        let backends = preferred_backends();
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            assert_eq!(backends, wgpu::Backends::METAL);
        } else {
            assert_eq!(backends, wgpu::Backends::VULKAN);
        }
        assert!(wgpu::Backends::PRIMARY.contains(backends));
    }
}
