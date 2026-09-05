//! The studio-model renderer: skinning pipeline, per-mesh bind groups and
//! the instance draw loop.
//!
//! This sits alongside [`crate::WorldRenderer`] and does not touch it: world
//! geometry is drawn first into a colour and depth target, and
//! [`StudioRenderer::render`] then draws models into the same target with a
//! loading (never clearing) pass. Given no external depth buffer it manages
//! and clears its own, which is what the model-only development viewer and
//! the headless test use.

use ohl_world::{
    MAX_BONES, STUDIO_VERTEX_BYTES, StudioModel, StudioPose, TextureImage, index_bytes,
    studio_vertex_bytes,
};

use crate::camera::FreeFlyCamera;
use crate::error::{RenderError, Result};
use crate::gpu::GpuContext;
use crate::math::{self, Mat4};
use crate::renderer::DEPTH_FORMAT;

/// Bytes per [`ohl_world::StudioVertex`].
const VERTEX_STRIDE: wgpu::BufferAddress = STUDIO_VERTEX_BYTES as wgpu::BufferAddress;

/// Three `mat4x4<f32>` plus three `vec4<f32>` of parameters, followed by the
/// bone matrix array. Must match `studio.wgsl`'s `Instance`.
const INSTANCE_HEADER_BYTES: usize = 3 * 64 + 3 * 16;
const INSTANCE_UNIFORM_BYTES: wgpu::BufferAddress =
    (INSTANCE_HEADER_BYTES + MAX_BONES * 64) as wgpu::BufferAddress;

/// One `vec4<f32>` of material flags. Must match `studio.wgsl`'s `Material`.
const MATERIAL_UNIFORM_BYTES: wgpu::BufferAddress = 16;

/// The colour a model-only frame is cleared to.
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.04,
    a: 1.0,
};

/// One placed, posed model to draw.
pub struct ModelInstance<'a> {
    /// The model's placement in world space, column-major.
    pub transform: Mat4,
    /// The sampled skeleton pose whose bone matrices are uploaded.
    pub pose: &'a StudioPose,
    /// Which sub-model each body part draws; a missing or out-of-range
    /// entry selects the body part's first sub-model.
    pub body: &'a [u32],
    /// Which skin family resolves each mesh's texture slot.
    pub skin: usize,
    /// Ambient colour, in `0..1`; callers using a map should pass
    /// [`ohl_world::WorldModel::ambient_at`] at the model's origin.
    pub ambient: [f32; 3],
    /// The unit direction the single directional light travels in.
    pub light_direction: [f32; 3],
    /// The directional light's colour, in `0..1`.
    pub light_color: [f32; 3],
}

impl ModelInstance<'_> {
    /// The default light this project uses when nothing better is known: a
    /// dim-ambient, over-the-shoulder key light.
    #[must_use]
    pub fn default_light_direction() -> [f32; 3] {
        math::normalize([-0.4, -0.6, -0.7])
    }
}

/// A pipeline and per-mesh resources for one [`StudioModel`].
pub struct StudioRenderer {
    opaque_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    instance_layout: wgpu::BindGroupLayout,
    /// One `(uniform buffer, bind group)` per instance drawn this frame,
    /// grown on demand so a steady-state frame allocates nothing.
    instance_slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// One bind group per texture, in [`StudioModel::textures`] order.
    texture_bind_groups: Vec<wgpu::BindGroup>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: usize,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    srgb_output: bool,
    last_triangle_count: usize,
}

impl StudioRenderer {
    /// Uploads `model` and builds the pipelines for a `color_format` target.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        context: &GpuContext,
        model: &StudioModel,
        color_format: wgpu::TextureFormat,
    ) -> Result<Self> {
        if model.vertices.is_empty() || model.indices.is_empty() {
            return Err(RenderError::WorldTooLarge);
        }
        let device = &context.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl studio shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("studio.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ohl studio sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let instance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl studio instance layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(INSTANCE_UNIFORM_BYTES),
                },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl studio material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(MATERIAL_UNIFORM_BYTES),
                    },
                    count: None,
                },
            ],
        });

        let mut texture_bind_groups = Vec::with_capacity(model.textures.len());
        for texture in &model.textures {
            let view = upload_texture(context, &texture.image);
            let mut flags = Vec::with_capacity(16);
            for value in [
                f32::from(u8::from(texture.is_chrome())),
                f32::from(u8::from(texture.is_fullbright())),
                f32::from(u8::from(texture.is_masked())),
                f32::from(u8::from(texture.is_additive())),
            ] {
                flags.extend_from_slice(&value.to_le_bytes());
            }
            let material = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ohl studio material uniform"),
                size: MATERIAL_UNIFORM_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context.queue.write_buffer(&material, 0, &flags);
            texture_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ohl studio material bind group"),
                layout: &material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: material.as_entire_binding(),
                    },
                ],
            }));
        }

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ohl studio pipeline layout"),
            bind_group_layouts: &[Some(&instance_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let vertex_attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 24,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 32,
                shader_location: 3,
            },
        ];
        let make_pipeline =
            |label: &'static str, blend: Option<wgpu::BlendState>, depth_write: bool| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vertex_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: VERTEX_STRIDE,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &vertex_attributes,
                        })],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fragment_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: color_format,
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        // Studio meshes mix strip and fan runs whose winding this
                        // project has not yet normalised, so both sides are drawn.
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(depth_write),
                        depth_compare: Some(wgpu::CompareFunction::Less),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let opaque_pipeline = make_pipeline("ohl studio pipeline", None, true);
        let additive_pipeline = make_pipeline(
            "ohl studio additive pipeline",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
            false,
        );

        let vertex_data = studio_vertex_bytes(&model.vertices);
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl studio vertices"),
            size: (vertex_data.len() as wgpu::BufferAddress).max(VERTEX_STRIDE),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&vertex_buffer, 0, &vertex_data);

        let index_data = index_bytes(&model.indices);
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl studio indices"),
            size: (index_data.len() as wgpu::BufferAddress).max(12),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&index_buffer, 0, &index_data);

        Ok(Self {
            opaque_pipeline,
            additive_pipeline,
            instance_layout,
            instance_slots: Vec::new(),
            texture_bind_groups,
            vertex_buffer,
            index_buffer,
            index_count: model.indices.len(),
            depth: None,
            srgb_output: color_format.is_srgb(),
            last_triangle_count: 0,
        })
    }

    /// The number of triangles the last [`Self::render`] call submitted.
    #[must_use]
    pub fn last_triangle_count(&self) -> usize {
        self.last_triangle_count
    }

    /// Recreates this renderer's own depth buffer when the target size
    /// changes. Only used when no external depth view is supplied.
    fn ensure_own_depth(&mut self, context: &GpuContext, width: u32, height: u32) {
        if !matches!(self.depth, Some((_, w, h)) if w == width && h == height) {
            let texture = context.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ohl studio depth buffer"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
                width,
                height,
            ));
        }
    }

    /// Grows the per-instance uniform slots to hold at least `count`.
    fn reserve_slots(&mut self, context: &GpuContext, count: usize) {
        while self.instance_slots.len() < count {
            let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ohl studio instance uniform"),
                size: INSTANCE_UNIFORM_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = context
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ohl studio instance bind group"),
                    layout: &self.instance_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
            self.instance_slots.push((buffer, bind_group));
        }
    }

    /// Draws `instances` of `model` from `camera` into `target`.
    ///
    /// Pass the world renderer's depth view as `external_depth` to draw on
    /// top of already-rendered world geometry (the colour target is then
    /// loaded, not cleared). Passing `None` clears both colour and depth and
    /// uses this renderer's own depth buffer, which is what a model-only
    /// view wants.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        context: &GpuContext,
        model: &StudioModel,
        camera: &FreeFlyCamera,
        instances: &[ModelInstance<'_>],
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        external_depth: Option<&wgpu::TextureView>,
    ) {
        let (width, height) = (width.max(1), height.max(1));
        #[allow(clippy::cast_precision_loss)]
        let aspect = width as f32 / height as f32;
        let view = camera.view();
        let view_projection = camera.view_projection(aspect);

        self.reserve_slots(context, instances.len());
        for (slot, instance) in instances.iter().enumerate() {
            let Some((buffer, _)) = self.instance_slots.get(slot) else {
                continue;
            };
            context.queue.write_buffer(
                buffer,
                0,
                &instance_uniform(instance, &view_projection, &view, self.srgb_output),
            );
        }

        if external_depth.is_none() {
            self.ensure_own_depth(context, width, height);
        }
        let Some(depth_view) =
            external_depth.or_else(|| self.depth.as_ref().map(|(view, _, _)| view))
        else {
            return;
        };
        let load_color = if external_depth.is_some() {
            wgpu::LoadOp::Load
        } else {
            wgpu::LoadOp::Clear(CLEAR_COLOR)
        };
        let load_depth = if external_depth.is_some() {
            wgpu::LoadOp::Load
        } else {
            wgpu::LoadOp::Clear(1.0)
        };

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl studio encoder"),
            });
        let triangles = Self::encode_pass_with(
            self,
            &mut encoder,
            model,
            instances,
            target,
            depth_view,
            load_color,
            load_depth,
        );
        context.queue.submit(Some(encoder.finish()));
        self.last_triangle_count = triangles;
    }

    /// Records the model pass and returns the triangle count it draws.
    #[allow(clippy::too_many_arguments)]
    fn encode_pass_with(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        model: &StudioModel,
        instances: &[ModelInstance<'_>],
        target: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        load_color: wgpu::LoadOp<wgpu::Color>,
        load_depth: wgpu::LoadOp<f32>,
    ) -> usize {
        let mut triangles = 0usize;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl studio pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_color,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: load_depth,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

            for (slot, instance) in instances.iter().enumerate() {
                let Some((_, bind_group)) = self.instance_slots.get(slot) else {
                    continue;
                };
                pass.set_bind_group(0, bind_group, &[]);
                // Opaque meshes first, then additive ones, so additive
                // surfaces blend against a complete depth buffer.
                for additive_pass in [false, true] {
                    pass.set_pipeline(if additive_pass {
                        &self.additive_pipeline
                    } else {
                        &self.opaque_pipeline
                    });
                    for mesh_index in model.visible_meshes(instance.body) {
                        let Some(mesh) = model.meshes.get(mesh_index) else {
                            continue;
                        };
                        let texture = model.resolve_skin(instance.skin, mesh.skin_slot);
                        let Some(material) = self.texture_bind_groups.get(texture) else {
                            continue;
                        };
                        let is_additive = model
                            .textures
                            .get(texture)
                            .is_some_and(ohl_world::StudioTexture::is_additive);
                        if is_additive != additive_pass {
                            continue;
                        }
                        let end = mesh.first_index + mesh.index_count;
                        if end as usize > self.index_count {
                            continue;
                        }
                        pass.set_bind_group(1, material, &[]);
                        pass.draw_indexed(mesh.first_index..end, 0, 0..1);
                        triangles += mesh.index_count as usize / 3;
                    }
                }
            }
        }
        triangles
    }
}

/// Serialises one instance's uniform block: the three matrices, the three
/// parameter vectors, and the bone matrix array padded to [`MAX_BONES`].
fn instance_uniform(
    instance: &ModelInstance<'_>,
    view_projection: &Mat4,
    view: &Mat4,
    srgb_output: bool,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(usize::try_from(INSTANCE_UNIFORM_BYTES).unwrap_or(8496));
    for matrix in [view_projection, view, &instance.transform] {
        for value in matrix {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    let srgb = if srgb_output { 1.0f32 } else { 0.0f32 };
    let vectors = [
        [
            instance.ambient[0],
            instance.ambient[1],
            instance.ambient[2],
            0.0,
        ],
        [
            instance.light_direction[0],
            instance.light_direction[1],
            instance.light_direction[2],
            srgb,
        ],
        [
            instance.light_color[0],
            instance.light_color[1],
            instance.light_color[2],
            0.0,
        ],
    ];
    for vector in vectors {
        for value in vector {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    let identity = math::identity();
    for bone in 0..MAX_BONES {
        let matrix = instance.pose.matrices.get(bone).unwrap_or(&identity);
        for value in matrix {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

fn upload_texture(context: &GpuContext, image: &TextureImage) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: image.width(),
        height: image.height(),
        depth_or_array_layers: 1,
    };
    let texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ohl studio texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // As with world textures: GoldSrc palettes are already gamma
        // encoded and are composited in that space.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    context.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        image.rgba(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width() * 4),
            rows_per_image: Some(image.height()),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Builds a column-major placement matrix from a world origin and a yaw in
/// degrees, which is how GoldSrc entities are placed.
#[must_use]
pub fn placement(origin: [f32; 3], yaw_degrees: f32) -> Mat4 {
    let (sin, cos) = yaw_degrees.to_radians().sin_cos();
    let mut m = math::identity();
    m[0] = cos;
    m[1] = sin;
    m[4] = -sin;
    m[5] = cos;
    m[12] = origin[0];
    m[13] = origin[1];
    m[14] = origin[2];
    m
}

#[cfg(test)]
mod tests {
    use super::{INSTANCE_UNIFORM_BYTES, MATERIAL_UNIFORM_BYTES, VERTEX_STRIDE, placement};
    use ohl_world::{MAX_BONES, STUDIO_VERTEX_BYTES};

    #[test]
    fn uniform_sizes_match_the_shader_layout() {
        assert_eq!(usize::try_from(VERTEX_STRIDE), Ok(STUDIO_VERTEX_BYTES));
        assert_eq!(MATERIAL_UNIFORM_BYTES, 16);
        assert_eq!(
            usize::try_from(INSTANCE_UNIFORM_BYTES),
            Ok(3 * 64 + 3 * 16 + MAX_BONES * 64)
        );
    }

    #[test]
    fn placement_rotates_about_the_up_axis() {
        let m = placement([1.0, 2.0, 3.0], 90.0);
        // +X maps to +Y after a 90-degree yaw.
        assert!(m[0].abs() < 1e-6 && (m[1] - 1.0).abs() < 1e-6);
        assert!((m[12] - 1.0).abs() < 1e-6 && (m[14] - 3.0).abs() < 1e-6);
    }
}
