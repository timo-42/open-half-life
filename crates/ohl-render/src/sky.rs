//! The sky pass: uploads [`ohl_world::SkyboxAsset`] as a cubemap and draws
//! it as a camera-rotation-only, inside-out cube after world geometry.

use ohl_world::{SKY_FACE_SUFFIXES, SkyboxAsset};

use crate::camera::FreeFlyCamera;
use crate::error::{RenderError, Result};
use crate::gpu::GpuContext;
use crate::math;
use crate::renderer::DEPTH_FORMAT;

/// `mat4x4<f32>` plus one `vec4<f32>` of parameters; must match `sky.wgsl`'s
/// `Camera`.
const CAMERA_UNIFORM_BYTES: wgpu::BufferAddress = 64 + 16;

/// Maps [`SKY_FACE_SUFFIXES`] order (`bk`, `dn`, `ft`, `lf`, `rt`, `up`) to
/// wgpu's cubemap array-layer order (`+X`, `-X`, `+Y`, `-Y`, `+Z`, `-Z`).
///
/// This project defines its own mapping from GoldSrc's world axes (`+X`
/// forward, `+Y` left, `+Z` up) to the six documented face names, since
/// neither the face-name convention nor `wgpu`'s cubemap layer order says
/// anything about the other: `+X` (forward) is `ft` (front), `-X` is `bk`
/// (back), `+Y` (left) is `lf`, `-Y` (right) is `rt`, `+Z` (up) is `up`, and
/// `-Z` (down) is `dn`. `sky.wgsl` samples with a direction vector in this
/// same world-axis space, so the two mappings agree.
const WGPU_LAYER_ORDER: [usize; 6] = [
    2, // +X <- ft
    0, // -X <- bk
    3, // +Y <- lf
    4, // -Y <- rt
    5, // +Z <- up
    1, // -Z <- dn
];

/// Uploads a [`SkyboxAsset`] and draws it as the background.
pub struct SkyRenderer {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    srgb_output: bool,
}

impl SkyRenderer {
    /// Builds the cubemap texture and pipeline for a `color_format` target.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        context: &GpuContext,
        skybox: &SkyboxAsset,
        color_format: wgpu::TextureFormat,
    ) -> Result<Self> {
        debug_assert_eq!(SKY_FACE_SUFFIXES.len(), 6);
        let device = &context.device;
        let (width, height) = (skybox.faces[0].width(), skybox.faces[0].height());
        if width == 0 || height == 0 {
            return Err(RenderError::WorldTooLarge);
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ohl skybox cubemap"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (layer, &face_index) in WGPU_LAYER_ORDER.iter().enumerate() {
            let face = &skybox.faces[face_index];
            context.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: u32::try_from(layer).unwrap_or(0),
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                face.rgba(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(face.width() * 4),
                    rows_per_image: Some(face.height()),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ohl skybox sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl sky bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(CAMERA_UNIFORM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl sky camera uniform"),
            size: CAMERA_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ohl sky bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl sky shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sky.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ohl sky pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ohl sky pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Never writes depth, and only draws where the opaque
                // passes left the cleared far value: "world occludes sky".
                // This only holds because `sky.wgsl`'s vertex shader pins
                // every vertex's post-divide depth to exactly `1.0`
                // (`clip_position.z = clip_position.w`) regardless of the
                // sky cube's own arbitrary `HALF_EXTENT` size — without
                // that, opaque geometry farther from the camera than
                // `HALF_EXTENT` has a *larger* depth than the sky cube and
                // this `LessEqual` test would let the sky wrongly draw over
                // it (fidelity finding F1).
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            pipeline,
            camera_buffer,
            bind_group,
            srgb_output: color_format.is_srgb(),
        })
    }

    /// Draws the skybox into `target`/`depth_view`, which must already hold
    /// the opaque world (and studio) passes' output: this pass loads both
    /// rather than clearing them.
    pub fn render(
        &self,
        context: &GpuContext,
        camera: &FreeFlyCamera,
        target: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        #[allow(clippy::cast_precision_loss)]
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let mut rotation_only = camera.view();
        rotation_only[12] = 0.0;
        rotation_only[13] = 0.0;
        rotation_only[14] = 0.0;
        let view_projection = math::multiply(&camera.projection(aspect), &rotation_only);

        let mut uniform = Vec::with_capacity(usize::try_from(CAMERA_UNIFORM_BYTES).unwrap_or(80));
        for value in view_projection {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        let srgb = if self.srgb_output { 1.0f32 } else { 0.0f32 };
        for value in [srgb, 0.0, 0.0, 0.0] {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        context.queue.write_buffer(&self.camera_buffer, 0, &uniform);

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl sky encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl sky pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..36, 0..1);
        }
        context.queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::WGPU_LAYER_ORDER;

    #[test]
    fn every_wgpu_layer_maps_to_a_distinct_face() {
        let mut seen = [false; 6];
        for &face in &WGPU_LAYER_ORDER {
            assert!(face < 6);
            assert!(!seen[face], "face {face} mapped from more than one layer");
            seen[face] = true;
        }
    }
}
