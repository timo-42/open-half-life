//! The world renderer: pipeline, buffers, bind groups and the draw loop.

use ohl_world::{DrawList, TextureImage, WorldModel, index_bytes, vertex_bytes};

use crate::camera::FreeFlyCamera;
use crate::error::{RenderError, Result};
use crate::gpu::GpuContext;
use crate::light_styles::LightStyles;
use crate::math::{self, Mat4};
use crate::render_props::{BlendKind, RenderProps};

/// The depth format the renderer uses. `Depth32Float` is available on every
/// wgpu backend, including downlevel and software adapters.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Bytes per [`ohl_world::WorldVertex`].
const VERTEX_STRIDE: wgpu::BufferAddress = 7 * 4;

/// `mat4x4<f32>` plus one `vec4<f32>` of parameters.
const CAMERA_UNIFORM_BYTES: wgpu::BufferAddress = 64 + 16;

/// `mat4x4<f32>` plus two `vec4<f32>`s (parameters, render colour); must
/// match `world_submodel.wgsl`'s `Camera`.
const SUBMODEL_UNIFORM_BYTES: wgpu::BufferAddress = 64 + 16 + 16;

/// The colour a frame is cleared to before any geometry is drawn.
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.02,
    b: 0.04,
    a: 1.0,
};

fn upload_texture(
    context: &GpuContext,
    image: &TextureImage,
    label: &'static str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: image.width(),
        height: image.height(),
        depth_or_array_layers: 1,
    };
    let texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Deliberately not an sRGB format: GoldSrc's palettes and lightmaps
        // are already gamma-encoded and are composited in that space.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_texture(context, &texture, size, image.rgba(), image.width());
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn write_texture(
    context: &GpuContext,
    texture: &wgpu::Texture,
    size: wgpu::Extent3d,
    rgba: &[u8],
    width: u32,
) {
    context.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(size.height),
        },
        size,
    );
}

/// Everything needed to draw one [`WorldModel`] into a colour target.
pub struct WorldRenderer {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    global_bind_group: wgpu::BindGroup,
    texture_bind_groups: Vec<wgpu::BindGroup>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_capacity: usize,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    draw_list: DrawList,
    srgb_output: bool,
    /// Kept (rather than just its view) so [`Self::update_light_styles`] can
    /// re-upload a re-blended atlas.
    lightmap_texture: wgpu::Texture,
    lightmap_size: (u32, u32),
    /// The liquid ("water") pass: same bind group layouts as the opaque
    /// pass (so it can reuse [`Self::texture_bind_groups`]), a dedicated
    /// camera-like uniform carrying the turbulence phase and alpha, and its
    /// own index buffer (liquid batches are a range within
    /// [`ohl_world::DrawList::liquid_indices`], not [`Self::index_buffer`]).
    liquid_pipeline: wgpu::RenderPipeline,
    liquid_camera_buffer: wgpu::Buffer,
    liquid_global_bind_group: wgpu::BindGroup,
    liquid_index_buffer: wgpu::Buffer,
    liquid_index_capacity: usize,
    /// Resources [`Self::draw_world_submodel`] reuses to build each brush
    /// entity's own (transient) buffers and bind groups: its own global
    /// bind group layout (a wider uniform than the opaque pass's, to carry
    /// the entity transform and render-mode parameters), `texture_layout`
    /// shared with the opaque pass, both samplers, and one precompiled
    /// pipeline per [`crate::render_props::BlendKind`].
    submodel_global_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    world_sampler: wgpu::Sampler,
    lightmap_sampler: wgpu::Sampler,
    submodel_opaque_pipeline: wgpu::RenderPipeline,
    submodel_alpha_pipeline: wgpu::RenderPipeline,
    submodel_additive_pipeline: wgpu::RenderPipeline,
    submodel_draw_list: DrawList,
}

/// One placed brush-entity submodel to draw with
/// [`WorldRenderer::draw_world_submodel`].
#[derive(Clone, Copy)]
pub struct SubmodelInstance<'a> {
    /// The submodel's own [`WorldModel`], built by
    /// [`ohl_world::WorldModel::build_submodel`].
    pub model: &'a WorldModel,
    /// The entity's placement in world space, column-major (for example
    /// [`crate::placement`]).
    pub transform: Mat4,
}

impl WorldRenderer {
    /// Uploads `model` and builds the pipeline for a `color_format` target.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        context: &GpuContext,
        model: &WorldModel,
        color_format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let device = &context.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl world shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ohl world sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ohl lightmap sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl global bind group layout"),
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
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl texture bind group layout"),
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
            ],
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl camera uniform"),
            size: CAMERA_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (lightmap_texture, lightmap_view) =
            upload_texture(context, &model.lightmap_atlas, "ohl lightmap atlas");
        let lightmap_size = (model.lightmap_atlas.width(), model.lightmap_atlas.height());
        let global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ohl global bind group"),
            layout: &global_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&lightmap_sampler),
                },
            ],
        });

        // One bind group per texture; a texture array would need a
        // per-device maximum-binding negotiation that buys nothing at this
        // milestone's batch counts.
        let mut texture_bind_groups = Vec::with_capacity(model.textures.len());
        for image in &model.textures {
            let (_texture, view) = upload_texture(context, image, "ohl world texture");
            texture_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ohl texture bind group"),
                layout: &texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }));
        }

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ohl world pipeline layout"),
            bind_group_layouts: &[Some(&global_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ohl world pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 20,
                            shader_location: 2,
                        },
                    ],
                })],
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
                // Face winding is not yet normalised across surfedge
                // direction and `plane_side`, so both sides are drawn.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // The liquid ("water") pass reuses `global_layout`/`texture_layout`
        // (identical bind-group structure) and `texture_bind_groups`, so it
        // only needs its own uniform buffer, bind group and pipeline.
        let liquid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl world water shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world_water.wgsl").into()),
        });
        let liquid_camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl liquid camera uniform"),
            size: CAMERA_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let liquid_global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ohl liquid global bind group"),
            layout: &global_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: liquid_camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&lightmap_sampler),
                },
            ],
        });
        let liquid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ohl liquid pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &liquid_shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 20,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &liquid_shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
                // Sorted after the opaque pass and never itself
                // depth-written, per the documented liquid-surface
                // convention (see `docs/FORMAT_SOURCES.md`).
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // A frame's liquid indices are a subset of `model.indices`, so the
        // same upper bound the opaque buffer uses is always enough.
        let liquid_index_capacity = model.indices.len().max(3);
        let liquid_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl liquid indices"),
            size: (liquid_index_capacity * 4) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let vertex_data = vertex_bytes(&model.vertices);
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl world vertices"),
            size: (vertex_data.len() as wgpu::BufferAddress).max(VERTEX_STRIDE),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&vertex_buffer, 0, &vertex_data);

        let index_capacity = model.indices.len().max(3);
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl world indices"),
            size: (index_capacity * 4) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        if model.vertices.is_empty() {
            return Err(RenderError::WorldTooLarge);
        }

        // The submodel pass: one pipeline per `BlendKind`, reusing
        // `texture_layout` with the opaque world pass (each submodel still
        // gets its own bind groups built at draw time, since a submodel is
        // a separate `WorldModel` with its own textures and lightmap
        // atlas), but with its own global bind group layout: its uniform
        // (`world_submodel.wgsl`'s `Camera`) is 96 bytes, not `world.wgsl`'s
        // 80 (`CAMERA_UNIFORM_BYTES`), so it cannot reuse `global_layout`
        // (whose binding declares that smaller `min_binding_size`).
        let submodel_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl submodel shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world_submodel.wgsl").into()),
        });
        let submodel_global_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("ohl submodel global bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(SUBMODEL_UNIFORM_BYTES),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
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
        let submodel_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ohl submodel pipeline layout"),
                bind_group_layouts: &[Some(&submodel_global_layout), Some(&texture_layout)],
                immediate_size: 0,
            });
        let make_submodel_pipeline =
            |label: &'static str, blend: Option<wgpu::BlendState>, depth_write: bool| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&submodel_pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &submodel_shader,
                        entry_point: Some("vertex_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: VERTEX_STRIDE,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &[
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x3,
                                    offset: 0,
                                    shader_location: 0,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x2,
                                    offset: 12,
                                    shader_location: 1,
                                },
                                wgpu::VertexAttribute {
                                    format: wgpu::VertexFormat::Float32x2,
                                    offset: 20,
                                    shader_location: 2,
                                },
                            ],
                        })],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &submodel_shader,
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
        // `RenderMode::Normal`/`Solid`: opaque, depth-written.
        let submodel_opaque_pipeline =
            make_submodel_pipeline("ohl submodel opaque pipeline", None, true);
        // `RenderMode::Color`/`Texture`: standard "over" alpha blending, not
        // depth-written.
        let submodel_alpha_pipeline = make_submodel_pipeline(
            "ohl submodel alpha pipeline",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
        );
        // `RenderMode::Glow`/`Additive`: additive, not depth-written.
        let submodel_additive_pipeline = make_submodel_pipeline(
            "ohl submodel additive pipeline",
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

        Ok(Self {
            pipeline,
            camera_buffer,
            global_bind_group,
            texture_bind_groups,
            vertex_buffer,
            index_buffer,
            index_capacity,
            depth: None,
            draw_list: DrawList::new(),
            srgb_output: color_format.is_srgb(),
            lightmap_texture,
            lightmap_size,
            liquid_pipeline,
            liquid_camera_buffer,
            liquid_global_bind_group,
            liquid_index_buffer,
            liquid_index_capacity,
            submodel_global_layout,
            texture_layout,
            world_sampler: sampler,
            lightmap_sampler,
            submodel_opaque_pipeline,
            submodel_alpha_pipeline,
            submodel_additive_pipeline,
            submodel_draw_list: DrawList::new(),
        })
    }

    /// Re-blends [`ohl_world::WorldModel::lightmap_atlas`] at `styles`'
    /// intensities for `time_seconds` and re-uploads it, animating light
    /// styles. Cheap enough to call once per rendered frame (or, since
    /// styles only change at [`crate::STYLE_HZ`], only when that step has
    /// advanced); a no-op if `model` is not the model this renderer was
    /// built from.
    pub fn update_light_styles(
        &self,
        context: &GpuContext,
        model: &WorldModel,
        styles: &LightStyles,
        time_seconds: f32,
    ) {
        let blended = model.blend_lightmap(|style| styles.intensity(style, time_seconds));
        if (blended.width(), blended.height()) != self.lightmap_size {
            return;
        }
        let size = wgpu::Extent3d {
            width: self.lightmap_size.0,
            height: self.lightmap_size.1,
            depth_or_array_layers: 1,
        };
        write_texture(
            context,
            &self.lightmap_texture,
            size,
            blended.rgba(),
            size.width,
        );
    }

    /// Recreates the depth buffer when the target size changes.
    pub fn resize(&mut self, context: &GpuContext, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if matches!(self.depth, Some((_, w, h)) if w == width && h == height) {
            return;
        }
        let texture = context.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ohl depth buffer"),
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

    /// The depth buffer this renderer last rendered with, so a second pass
    /// (the studio-model pass, for instance) can draw into the same depth
    /// state instead of allocating its own. `None` before the first
    /// [`Self::render`] or [`Self::resize`] call.
    #[must_use]
    pub fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.depth.as_ref().map(|(view, _, _)| view)
    }

    /// The number of triangles the last [`Self::render`] call submitted.
    #[must_use]
    pub fn last_triangle_count(&self) -> usize {
        self.draw_list.triangle_count()
    }

    /// Draws `model` from `camera` into `target`.
    ///
    /// Culling happens on the CPU in `ohl-world`: the PVS of the leaf the
    /// camera stands in selects candidate faces and the frustum rejects the
    /// rest, so only the surviving indices are uploaded each frame.
    pub fn render(
        &mut self,
        context: &GpuContext,
        model: &WorldModel,
        camera: &FreeFlyCamera,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        self.resize(context, width, height);
        #[allow(clippy::cast_precision_loss)]
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let view_projection = camera.view_projection(aspect);
        let frustum = camera.frustum(aspect);
        model.build_draw_list(camera.position, Some(&frustum), &mut self.draw_list);

        let mut uniform = Vec::with_capacity(usize::try_from(CAMERA_UNIFORM_BYTES).unwrap_or(80));
        for value in view_projection {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        let srgb = if self.srgb_output { 1.0f32 } else { 0.0f32 };
        for value in [srgb, 0.0, 0.0, 0.0] {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        context.queue.write_buffer(&self.camera_buffer, 0, &uniform);

        let indices =
            &self.draw_list.indices[..self.draw_list.indices.len().min(self.index_capacity)];
        if !indices.is_empty() {
            context
                .queue
                .write_buffer(&self.index_buffer, 0, &index_bytes(indices));
        }

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl world encoder"),
            });
        {
            let depth_view = self
                .depth
                .as_ref()
                .map(|(view, _, _)| view)
                .expect("resize created the depth buffer above");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl world pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.global_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for batch in &self.draw_list.batches {
                let Some(bind_group) = self.texture_bind_groups.get(batch.texture) else {
                    continue;
                };
                let end = batch.first_index + batch.index_count;
                if end as usize > indices.len() {
                    continue;
                }
                pass.set_bind_group(1, bind_group, &[]);
                pass.draw_indexed(batch.first_index..end, 0, 0..1);
            }
        }
        context.queue.submit(Some(encoder.finish()));
    }

    /// Draws this frame's liquid ("water") faces, translucent and without
    /// writing depth, into the same `target`/depth as the just-completed
    /// [`Self::render`] call (loaded, not cleared).
    ///
    /// Must be called after [`Self::render`] in the same frame: it draws
    /// from the liquid batches that call's [`ohl_world::WorldModel::build_draw_list`]
    /// already computed, rather than recomputing culling itself.
    ///
    /// `time_seconds` drives the UV turbulence phase; `alpha` (`0.0..=1.0`)
    /// is the pass's overall translucency (a worldspawn or entity's
    /// `renderamt`/255, defaulting to `1.0`).
    #[allow(clippy::too_many_arguments)]
    pub fn render_liquid(
        &mut self,
        context: &GpuContext,
        camera: &FreeFlyCamera,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        time_seconds: f32,
        alpha: f32,
    ) {
        if self.draw_list.liquid_indices.is_empty() {
            return;
        }
        let Some((depth_view, _, _)) = &self.depth else {
            return;
        };
        #[allow(clippy::cast_precision_loss)]
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let view_projection = camera.view_projection(aspect);

        let mut uniform = Vec::with_capacity(usize::try_from(CAMERA_UNIFORM_BYTES).unwrap_or(80));
        for value in view_projection {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        let srgb = if self.srgb_output { 1.0f32 } else { 0.0f32 };
        for value in [
            srgb,
            if time_seconds.is_finite() {
                time_seconds
            } else {
                0.0
            },
            alpha.clamp(0.0, 1.0),
            0.0,
        ] {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        context
            .queue
            .write_buffer(&self.liquid_camera_buffer, 0, &uniform);

        let indices = &self.draw_list.liquid_indices[..self
            .draw_list
            .liquid_indices
            .len()
            .min(self.liquid_index_capacity)];
        context
            .queue
            .write_buffer(&self.liquid_index_buffer, 0, &index_bytes(indices));

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl liquid encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl liquid pass"),
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
            pass.set_pipeline(&self.liquid_pipeline);
            pass.set_bind_group(0, &self.liquid_global_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(
                self.liquid_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for batch in &self.draw_list.liquid_batches {
                let Some(bind_group) = self.texture_bind_groups.get(batch.texture) else {
                    continue;
                };
                let end = batch.first_index + batch.index_count;
                if end as usize > indices.len() {
                    continue;
                }
                pass.set_bind_group(1, bind_group, &[]);
                pass.draw_indexed(batch.first_index..end, 0, 0..1);
            }
        }
        context.queue.submit(Some(encoder.finish()));
    }

    /// Draws one brush-entity submodel (see
    /// [`ohl_world::WorldModel::build_submodel`]) with `props`' render-mode
    /// blend state, into the same `target`/depth as the just-completed
    /// [`Self::render`] call (loaded, not cleared).
    ///
    /// Must be called after [`Self::render`] in the same frame, so a depth
    /// buffer already exists to test (and, for
    /// [`crate::render_props::RenderMode::Normal`]/[`crate::render_props::RenderMode::Solid`],
    /// write) against.
    ///
    /// Builds this submodel's vertex/index buffers and texture/lightmap
    /// bind groups fresh on every call rather than caching them per model:
    /// brush entities are typically small, and this keeps the first-light
    /// implementation simple (see `docs/MILESTONES.md`, M3.4); a future
    /// milestone can cache per-`WorldModel` resources if profiling shows
    /// this matters. Ignores this submodel's own liquid faces (only
    /// [`ohl_world::DrawList::batches`] is drawn, from
    /// [`ohl_world::WorldModel::build_draw_list_for_model`]) under whichever
    /// pipeline `props.blend_kind()` selects.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn draw_world_submodel(
        &mut self,
        context: &GpuContext,
        instance: SubmodelInstance<'_>,
        props: RenderProps,
        camera: &FreeFlyCamera,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let model = instance.model;
        if model.vertices.is_empty() || model.indices.is_empty() {
            return;
        }
        let Some((depth_view, _, _)) = &self.depth else {
            return;
        };
        let device = &context.device;

        model.build_draw_list_for_model(&mut self.submodel_draw_list);
        if self.submodel_draw_list.indices.is_empty() {
            return;
        }

        let vertex_data = vertex_bytes(&model.vertices);
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl submodel vertices"),
            size: (vertex_data.len() as wgpu::BufferAddress).max(VERTEX_STRIDE),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&vertex_buffer, 0, &vertex_data);

        let index_data = index_bytes(&self.submodel_draw_list.indices);
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl submodel indices"),
            size: (index_data.len() as wgpu::BufferAddress).max(12),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue.write_buffer(&index_buffer, 0, &index_data);

        let (_lightmap_texture, lightmap_view) = upload_texture(
            context,
            &model.lightmap_atlas,
            "ohl submodel lightmap atlas",
        );

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl submodel camera uniform"),
            size: SUBMODEL_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ohl submodel global bind group"),
            layout: &self.submodel_global_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.lightmap_sampler),
                },
            ],
        });

        let mut texture_bind_groups = Vec::with_capacity(model.textures.len());
        for image in &model.textures {
            let (_texture, view) = upload_texture(context, image, "ohl submodel texture");
            texture_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ohl submodel texture bind group"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.world_sampler),
                    },
                ],
            }));
        }

        #[allow(clippy::cast_precision_loss)]
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let combined = math::multiply(&camera.view_projection(aspect), &instance.transform);

        let mut uniform = Vec::with_capacity(usize::try_from(SUBMODEL_UNIFORM_BYTES).unwrap_or(96));
        for value in combined {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        let srgb = if self.srgb_output { 1.0f32 } else { 0.0f32 };
        let use_render_color = if props.uses_render_color() {
            1.0f32
        } else {
            0.0f32
        };
        for value in [srgb, props.alpha(), use_render_color, 0.0] {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            f32::from(props.color[0]) / 255.0,
            f32::from(props.color[1]) / 255.0,
            f32::from(props.color[2]) / 255.0,
            0.0,
        ] {
            uniform.extend_from_slice(&value.to_le_bytes());
        }
        context.queue.write_buffer(&camera_buffer, 0, &uniform);

        let pipeline = match props.blend_kind() {
            BlendKind::Opaque => &self.submodel_opaque_pipeline,
            BlendKind::AlphaBlend => &self.submodel_alpha_pipeline,
            BlendKind::Additive => &self.submodel_additive_pipeline,
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ohl submodel encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl submodel pass"),
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &global_bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for batch in &self.submodel_draw_list.batches {
                let Some(bind_group) = texture_bind_groups.get(batch.texture) else {
                    continue;
                };
                let end = batch.first_index + batch.index_count;
                if end as usize > self.submodel_draw_list.indices.len() {
                    continue;
                }
                pass.set_bind_group(1, bind_group, &[]);
                pass.draw_indexed(batch.first_index..end, 0, 0..1);
            }
        }
        context.queue.submit(Some(encoder.finish()));
    }
}
