//! Project-authored visual primitives, independent of gameplay and assets.
//!
//! Beams and particles blend additively; surface marks alpha blend; small
//! cuboid debris writes depth. These are deliberately simple fallback shapes,
//! not reproductions of retail textures/models. Callers own emission, aging,
//! collision and decal clipping. See `docs/FORMAT_SOURCES.md`.

use std::ops::Range;

use crate::{DEPTH_FORMAT, FreeFlyCamera, GpuContext, math};

/// Maximum instances accepted by one draw. Bounds CPU and GPU allocations.
pub const MAX_EFFECT_INSTANCES: usize = 4096;

/// One asset-independent effect. Dimensions are in world units and colors
/// are RGBA in `0..=1`. Invalid geometry is skipped; colors are clamped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectInstance {
    /// Camera-facing ribbon between two world points, with a full width.
    Beam {
        start: [f32; 3],
        end: [f32; 3],
        width: f32,
        color: [f32; 4],
    },
    /// Camera-facing square with a full size, for points or spark fragments.
    Particle {
        origin: [f32; 3],
        size: f32,
        color: [f32; 4],
    },
    /// Procedural round bullet/blood mark on a plane. The caller must clip
    /// it to the hit surface if the mark lies near a boundary.
    Decal {
        origin: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        color: [f32; 4],
    },
    /// Cuboid gib/debris placeholder at a column-major affine transform.
    /// This draws geometry only; it creates no physics body.
    Gib {
        transform: math::Mat4,
        half_extents: [f32; 3],
        color: [f32; 4],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blend {
    Opaque,
    Alpha,
    Additive,
}

impl EffectInstance {
    fn center(self) -> [f32; 3] {
        match self {
            Self::Beam { start, end, .. } => std::array::from_fn(|i| start[i].midpoint(end[i])),
            Self::Particle { origin, .. } | Self::Decal { origin, .. } => origin,
            Self::Gib { transform, .. } => [transform[12], transform[13], transform[14]],
        }
    }

    fn blend(self) -> Blend {
        match self {
            Self::Beam { .. } | Self::Particle { .. } => Blend::Additive,
            Self::Decal { .. } => Blend::Alpha,
            Self::Gib { .. } => Blend::Opaque,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    position: [f32; 3],
    color: [f32; 4],
}

#[derive(Default)]
struct Mesh {
    vertices: Vec<Vertex>,
    draws: Vec<(Blend, Range<u32>)>,
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn scale(a: [f32; 3], amount: f32) -> [f32; 3] {
    a.map(|v| v * amount)
}

fn positive(value: f32) -> bool {
    value.is_finite() && value > 0.0 && value <= 4096.0
}

fn finite(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite())
}

// The homogeneous row is an affine encoding, not an approximate result.
#[allow(clippy::float_cmp)]
fn affine(transform: math::Mat4) -> bool {
    transform[3] == 0.0 && transform[7] == 0.0 && transform[11] == 0.0 && transform[15] == 1.0
}

fn quad(out: &mut Vec<Vertex>, corners: [[f32; 3]; 4], color: [f32; 4]) {
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex {
            position: corners[index],
            color,
        });
    }
}

/// A perpendicular basis even when the camera looks along a beam/normal.
fn perpendicular(direction: [f32; 3]) -> [f32; 3] {
    let axis = if direction[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    math::normalize(math::cross(direction, axis))
}

#[allow(clippy::too_many_lines)]
fn append_geometry(effect: EffectInstance, camera: &FreeFlyCamera, out: &mut Vec<Vertex>) {
    let color = match effect {
        EffectInstance::Beam { color, .. }
        | EffectInstance::Particle { color, .. }
        | EffectInstance::Decal { color, .. }
        | EffectInstance::Gib { color, .. } => color,
    };
    if !finite(&color) || color[3] <= 0.0 {
        return;
    }
    let color = color.map(|value| value.clamp(0.0, 1.0));
    match effect {
        EffectInstance::Beam {
            start, end, width, ..
        } => {
            let delta = sub(end, start);
            if !finite(&start)
                || !finite(&end)
                || !positive(width)
                || math::dot(delta, delta) < 1e-8
            {
                return;
            }
            let direction = math::normalize(delta);
            let cross = math::cross(direction, sub(camera.position, effect.center()));
            let right = if math::dot(cross, cross) > 1e-8 {
                math::normalize(cross)
            } else {
                perpendicular(direction)
            };
            let offset = scale(right, width * 0.5);
            quad(
                out,
                [
                    sub(start, offset),
                    add(start, offset),
                    add(end, offset),
                    sub(end, offset),
                ],
                color,
            );
        }
        EffectInstance::Particle { origin, size, .. } => {
            if !finite(&origin) || !positive(size) {
                return;
            }
            let right = scale(perpendicular(camera.direction()), size * 0.5);
            let up = scale(
                math::cross(math::normalize(right), camera.direction()),
                size * 0.5,
            );
            quad(
                out,
                [
                    sub(sub(origin, right), up),
                    add(sub(origin, up), right),
                    add(add(origin, right), up),
                    add(sub(origin, right), up),
                ],
                color,
            );
        }
        EffectInstance::Decal {
            origin,
            normal,
            radius,
            ..
        } => {
            if !finite(&origin)
                || !finite(&normal)
                || math::dot(normal, normal) < 1e-8
                || !positive(radius)
            {
                return;
            }
            let normal = math::normalize(normal);
            // Project-authored bias avoids coplanar depth fighting.
            let origin = add(origin, scale(normal, 0.05));
            let right = scale(perpendicular(normal), radius);
            let up = math::cross(normal, right);
            for edge in 0..12_u16 {
                let angle = f32::from(edge) * std::f32::consts::TAU / 12.0;
                let next = f32::from(edge + 1) * std::f32::consts::TAU / 12.0;
                for position in [
                    origin,
                    add(
                        origin,
                        add(scale(right, angle.cos()), scale(up, angle.sin())),
                    ),
                    add(origin, add(scale(right, next.cos()), scale(up, next.sin()))),
                ] {
                    out.push(Vertex { position, color });
                }
            }
        }
        EffectInstance::Gib {
            transform,
            half_extents,
            ..
        } => {
            if !finite(&transform) || !half_extents.into_iter().all(positive) || !affine(transform)
            {
                return;
            }
            let corners: [[f32; 3]; 8] = std::array::from_fn(|index| {
                let local: [f32; 3] = std::array::from_fn(|axis| {
                    if index & (1 << axis) == 0 {
                        -half_extents[axis]
                    } else {
                        half_extents[axis]
                    }
                });
                std::array::from_fn(|row| {
                    transform[row] * local[0]
                        + transform[4 + row] * local[1]
                        + transform[8 + row] * local[2]
                        + transform[12 + row]
                })
            });
            for (face, shade) in [
                ([0, 1, 3, 2], 0.6),
                ([4, 6, 7, 5], 1.0),
                ([0, 4, 5, 1], 0.75),
                ([2, 3, 7, 6], 0.75),
                ([0, 2, 6, 4], 0.85),
                ([1, 5, 7, 3], 0.85),
            ] {
                quad(
                    out,
                    face.map(|index| corners[index]),
                    [color[0] * shade, color[1] * shade, color[2] * shade, 1.0],
                );
            }
        }
    }
}

fn build_mesh(instances: &[EffectInstance], camera: &FreeFlyCamera) -> Mesh {
    let mut order: Vec<_> = instances
        .iter()
        .take(MAX_EFFECT_INSTANCES)
        .copied()
        .collect();
    order.sort_by(
        |a, b| match (a.blend() == Blend::Opaque, b.blend() == Blend::Opaque) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => math::dot(sub(b.center(), camera.position), camera.direction()).total_cmp(
                &math::dot(sub(a.center(), camera.position), camera.direction()),
            ),
        },
    );
    let mut mesh = Mesh::default();
    for effect in order {
        // At most 36 vertices per instance, so this conversion is bounded.
        let start = u32::try_from(mesh.vertices.len()).unwrap_or(0);
        append_geometry(effect, camera, &mut mesh.vertices);
        // Finite inputs can still overflow after transformation or addition.
        if mesh.vertices[usize::try_from(start).unwrap_or(0)..]
            .iter()
            .any(|vertex| !finite(&vertex.position))
        {
            mesh.vertices.truncate(usize::try_from(start).unwrap_or(0));
        }
        let end = u32::try_from(mesh.vertices.len()).unwrap_or(start);
        if end > start {
            mesh.draws.push((effect.blend(), start..end));
        }
    }
    mesh
}

/// Reusable GPU resources for [`EffectInstance`] geometry. Call after world
/// rendering with the world's existing depth view and matching dimensions.
pub struct EffectRenderer {
    pipelines: [wgpu::RenderPipeline; 3],
    camera: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    srgb: bool,
}

impl EffectRenderer {
    #[allow(clippy::too_many_lines)]
    pub fn new(context: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let device = &context.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl effects shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl effects layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(80),
                },
                count: None,
            }],
        });
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl effects camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ohl effects camera binding"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ohl effects pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let pipelines =
            [None, Some(wgpu::BlendState::ALPHA_BLENDING), Some(additive)].map(|blend| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("ohl effects pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vertex_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: 28,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                        })],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fragment_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(blend.is_none()),
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl effects vertices"),
            size: (MAX_EFFECT_INSTANCES * 36 * 28) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipelines,
            camera,
            bind_group,
            vertices,
            srgb: format.is_srgb(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        context: &GpuContext,
        instances: &[EffectInstance],
        camera: &FreeFlyCamera,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let mesh = build_mesh(instances, camera);
        if mesh.vertices.is_empty() {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let mut uniform: Vec<u8> = camera
            .view_projection(aspect)
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        uniform.extend(
            [if self.srgb { 1.0_f32 } else { 0.0 }, 0.0, 0.0, 0.0]
                .iter()
                .flat_map(|value| value.to_le_bytes()),
        );
        context.queue.write_buffer(&self.camera, 0, &uniform);
        let bytes: Vec<u8> = mesh
            .vertices
            .iter()
            .flat_map(|vertex| {
                vertex
                    .position
                    .iter()
                    .chain(&vertex.color)
                    .flat_map(|value| value.to_le_bytes())
            })
            .collect();
        context.queue.write_buffer(&self.vertices, 0, &bytes);
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl effects encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl effects pass"),
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
                    view: depth,
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            for (blend, range) in mesh.draws {
                let slot = match blend {
                    Blend::Opaque => 0,
                    Blend::Alpha => 1,
                    Blend::Additive => 2,
                };
                pass.set_pipeline(&self.pipelines[slot]);
                pass.draw(range, 0..1);
            }
        }
        context.queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0; 4];

    #[test]
    fn ribbon_reaches_both_endpoints_and_has_the_requested_width() {
        let effect = EffectInstance::Beam {
            start: [10.0, -8.0, 64.0],
            end: [10.0, 8.0, 64.0],
            width: 4.0,
            color: WHITE,
        };
        let mesh = build_mesh(&[effect], &FreeFlyCamera::default());
        assert_eq!(mesh.vertices.len(), 6);
        for vertex in mesh.vertices {
            assert_eq!(vertex.position[0], 10.0);
            assert_eq!(vertex.position[1].abs(), 8.0);
            assert_eq!((vertex.position[2] - 64.0).abs(), 2.0);
        }
        let parallel = EffectInstance::Beam {
            start: [10.0, 0.0, 64.0],
            end: [20.0, 0.0, 64.0],
            width: 2.0,
            color: WHITE,
        };
        let mesh = build_mesh(&[parallel], &FreeFlyCamera::default());
        assert!(mesh.vertices.iter().all(|vertex| finite(&vertex.position)));
        assert_ne!(mesh.vertices[0].position, mesh.vertices[1].position);
    }

    #[test]
    fn particle_faces_camera_and_surface_mark_follows_its_normal() {
        let camera = FreeFlyCamera::default();
        let particle = EffectInstance::Particle {
            origin: [10.0, 0.0, 64.0],
            size: 4.0,
            color: WHITE,
        };
        let mesh = build_mesh(&[particle], &camera);
        assert_eq!(mesh.vertices.len(), 6);
        assert!(mesh.vertices.iter().all(|vertex| vertex.position[0] == 10.0
            && vertex.position[1].abs() == 2.0
            && (vertex.position[2] - 64.0).abs() == 2.0));
        let decal = EffectInstance::Decal {
            origin: [10.0, 0.0, 64.0],
            normal: [-1.0, 0.0, 0.0],
            radius: 3.0,
            color: WHITE,
        };
        let mesh = build_mesh(&[decal], &camera);
        assert_eq!(mesh.vertices.len(), 36);
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| (vertex.position[0] - 9.95).abs() < 1e-5)
        );
        assert_eq!(mesh.draws[0].0, Blend::Alpha);
    }

    #[test]
    fn debris_uses_its_transform_and_draws_before_translucent_effects() {
        let mut transform = math::identity();
        // Rotate around Z by ninety degrees as well as translating.
        transform[0] = 0.0;
        transform[1] = 1.0;
        transform[4] = -1.0;
        transform[5] = 0.0;
        transform[12] = 10.0;
        let gib = EffectInstance::Gib {
            transform,
            half_extents: [1.0, 2.0, 3.0],
            color: WHITE,
        };
        let particle = EffectInstance::Particle {
            origin: [10.0, 0.0, 64.0],
            size: 1.0,
            color: WHITE,
        };
        let mesh = build_mesh(&[particle, gib], &FreeFlyCamera::default());
        assert_eq!(mesh.draws[0], (Blend::Opaque, 0..36));
        assert!(
            mesh.vertices[..36]
                .iter()
                .all(|vertex| (vertex.position[0] - 10.0).abs() == 2.0
                    && vertex.position[1].abs() == 1.0
                    && vertex.position[2].abs() == 3.0)
        );
    }

    #[test]
    fn translucent_effects_sort_by_view_depth_and_invalid_inputs_are_bounded() {
        let particle = |x| EffectInstance::Particle {
            origin: [x, 0.0, 64.0],
            size: 1.0,
            color: WHITE,
        };
        let mesh = build_mesh(&[particle(10.0), particle(20.0)], &FreeFlyCamera::default());
        assert_eq!(mesh.vertices[0].position[0], 20.0);
        let invalid = EffectInstance::Beam {
            start: [0.0; 3],
            end: [0.0; 3],
            width: 1.0,
            color: WHITE,
        };
        assert!(
            build_mesh(&[invalid, particle(f32::NAN)], &FreeFlyCamera::default())
                .vertices
                .is_empty()
        );
        let mesh = build_mesh(
            &vec![particle(10.0); MAX_EFFECT_INSTANCES + 1],
            &FreeFlyCamera::default(),
        );
        assert_eq!(mesh.vertices.len(), MAX_EFFECT_INSTANCES * 6);
    }

    #[test]
    fn invalid_geometry_is_skipped_and_color_channels_are_clamped() {
        let camera = FreeFlyCamera::default();
        let mut overflow = math::identity();
        overflow[0] = f32::MAX;
        let mut projective = math::identity();
        projective[3] = 0.2;
        let invalid = [
            EffectInstance::Particle {
                origin: [10.0; 3],
                size: 0.0,
                color: WHITE,
            },
            EffectInstance::Decal {
                origin: [10.0; 3],
                normal: [0.0; 3],
                radius: 1.0,
                color: WHITE,
            },
            EffectInstance::Gib {
                transform: overflow,
                half_extents: [4096.0; 3],
                color: WHITE,
            },
            EffectInstance::Gib {
                transform: projective,
                half_extents: [1.0; 3],
                color: WHITE,
            },
        ];
        assert!(build_mesh(&invalid, &camera).vertices.is_empty());
        let particle = EffectInstance::Particle {
            origin: [10.0; 3],
            size: 1.0,
            color: [2.0, -1.0, 0.5, 2.0],
        };
        let mesh = build_mesh(&[particle], &camera);
        assert_eq!(mesh.vertices.len(), 6);
        assert!(
            mesh.vertices
                .iter()
                .all(|vertex| vertex.color == [1.0, 0.0, 0.5, 1.0])
        );
    }
}
