//! Synthetic studio property/depth probes, opt-in GPU only. No payload data.

use ohl_render::{
    DEPTH_FORMAT, FreeFlyCamera, GpuContext, ModelInstance, OFFSCREEN_FORMAT, OffscreenTarget,
    RenderMode, RenderProps, RenderedModelInstance, StudioRenderPhase, StudioRenderer, math,
    placement, wgpu,
};
use ohl_world::{
    STUDIO_NF_ADDITIVE, STUDIO_NF_FULLBRIGHT, STUDIO_NF_MASKED, StudioLimits, StudioModel,
    StudioPose, TextureImage,
};

const EDGE: u32 = 64;
const BACKGROUND: [u8; 3] = [24, 48, 72];
const TEXTURE: [u8; 3] = [96, 64, 32];

fn synthetic_model(rgb: [u8; 3], alpha: u8, flags: u32) -> (StudioModel, StudioPose) {
    let (bytes, _) = ohl_formats::test_support::build_minimal_mdl10();
    let mut model = StudioModel::parse(&bytes, &StudioLimits::default()).unwrap();
    model.textures[0].image = TextureImage::new(1, 1, vec![rgb[0], rgb[1], rgb[2], alpha]).unwrap();
    model.textures[0].flags = STUDIO_NF_FULLBRIGHT | flags;
    model.bounds_min = [0.0; 3];
    model.bounds_max = [1.0, 1.0, 0.0];
    let mut pose = StudioPose::bind(&model);
    pose.matrices.fill(math::identity());
    (model, pose)
}

fn depth(context: &GpuContext) -> wgpu::TextureView {
    context
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("synthetic studio depth"),
            size: wgpu::Extent3d {
                width: EDGE,
                height: EDGE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn clear(context: &GpuContext, target: &OffscreenTarget, depth: &wgpu::TextureView, rgb: [f64; 3]) {
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("synthetic studio background"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: rgb[0],
                        g: rgb[1],
                        b: rgb[2],
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
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
    }
    context.queue.submit([encoder.finish()]);
}

fn camera() -> FreeFlyCamera {
    FreeFlyCamera {
        position: [0.5, 0.5, 4.0],
        pitch: 89.9,
        yaw: 0.0,
        near: 0.1,
        fov_y_degrees: 60.0,
        ..FreeFlyCamera::default()
    }
}

fn instance(pose: &StudioPose, z: f32, props: RenderProps) -> RenderedModelInstance<'_> {
    RenderedModelInstance {
        instance: ModelInstance {
            transform: placement([0.0, 0.0, z], 0.0),
            pose,
            body: &[],
            skin: 0,
            ambient: [1.0; 3],
            light_direction: [0.0; 3],
            light_color: [0.0; 3],
        },
        render_props: props,
    }
}

fn pixel(context: &GpuContext, target: &OffscreenTarget) -> [u8; 3] {
    let pixels = target.read_rgba(context).unwrap();
    let offset = usize::try_from((EDGE / 2 * EDGE + EDGE / 2) * 4).unwrap();
    pixels[offset..offset + 3].try_into().unwrap()
}

fn close(actual: [u8; 3], expected: [f32; 3]) {
    for channel in 0..3 {
        assert!(
            (f32::from(actual[channel]) - expected[channel]).abs() <= 2.0,
            "channel {channel}: got {} expected {}",
            actual[channel],
            expected[channel]
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn studio_properties_composite_and_own_depth_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let context = GpuContext::headless().expect("explicit GPU opt-in requires an adapter");
    let target = OffscreenTarget::new(&context, EDGE, EDGE).unwrap();
    let depth = depth(&context);
    let (model, pose) = synthetic_model(TEXTURE, 255, 0);
    let mut renderer = StudioRenderer::new(&context, &model, OFFSCREEN_FORMAT).unwrap();
    for mode in [
        RenderMode::Color,
        RenderMode::Texture,
        RenderMode::Additive,
        RenderMode::Glow,
    ] {
        for amount in [0, 128, 255] {
            let props = RenderProps {
                mode,
                amount,
                color: [240, 16, 8],
                fx: 0,
            };
            clear(
                &context,
                &target,
                &depth,
                BACKGROUND.map(|x| f64::from(x) / 255.0),
            );
            renderer.render_with_props(
                &context,
                &model,
                &camera(),
                &[instance(&pose, 0.0, props)],
                StudioRenderPhase::All,
                target.view(),
                EDGE,
                EDGE,
                &depth,
            );
            let alpha = f32::from(amount) / 255.0;
            let source = if mode == RenderMode::Color {
                props.color
            } else {
                TEXTURE
            };
            let additive = matches!(mode, RenderMode::Additive | RenderMode::Glow);
            close(
                pixel(&context, &target),
                std::array::from_fn(|i| {
                    (f32::from(source[i]) * alpha
                        + f32::from(BACKGROUND[i]) * if additive { 1.0 } else { 1.0 - alpha })
                    .min(255.0)
                }),
            );
        }
    }
    let (probe_model, probe_pose) = synthetic_model([0, 0, 200], 255, 0);
    let mut probe = StudioRenderer::new(&context, &probe_model, OFFSCREEN_FORMAT).unwrap();
    for mode in [
        RenderMode::Normal,
        RenderMode::Solid,
        RenderMode::Texture,
        RenderMode::Color,
        RenderMode::Additive,
    ] {
        for amount in [0, 128] {
            clear(
                &context,
                &target,
                &depth,
                BACKGROUND.map(|x| f64::from(x) / 255.0),
            );
            renderer.render_with_props(
                &context,
                &model,
                &camera(),
                &[instance(
                    &pose,
                    1.0,
                    RenderProps {
                        mode,
                        amount,
                        color: [240, 0, 0],
                        fx: 0,
                    },
                )],
                StudioRenderPhase::All,
                target.view(),
                EDGE,
                EDGE,
                &depth,
            );
            probe.render_with_props(
                &context,
                &probe_model,
                &camera(),
                &[instance(&probe_pose, 0.0, RenderProps::default())],
                StudioRenderPhase::All,
                target.view(),
                EDGE,
                EDGE,
                &depth,
            );
            close(
                pixel(&context, &target),
                if matches!(mode, RenderMode::Normal | RenderMode::Solid) {
                    TEXTURE.map(f32::from)
                } else {
                    [0.0, 0.0, 200.0]
                },
            );
        }
    }
}

#[test]
fn studio_material_masks_default_parity_and_translucent_order_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let context = GpuContext::headless().expect("explicit GPU opt-in requires an adapter");
    let target = OffscreenTarget::new(&context, EDGE, EDGE).unwrap();
    let depth = depth(&context);
    for (flags, alpha) in [(0, 255), (STUDIO_NF_ADDITIVE, 255), (STUDIO_NF_MASKED, 0)] {
        let (model, pose) = synthetic_model(TEXTURE, alpha, flags);
        let mut renderer = StudioRenderer::new(&context, &model, OFFSCREEN_FORMAT).unwrap();
        let entry = instance(&pose, 0.0, RenderProps::default());
        renderer.render(
            &context,
            &model,
            &camera(),
            &[entry.instance],
            target.view(),
            EDGE,
            EDGE,
            None,
        );
        let old = target.read_rgba(&context).unwrap();
        clear(&context, &target, &depth, [0.02, 0.02, 0.04]);
        renderer.render_with_props(
            &context,
            &model,
            &camera(),
            &[instance(&pose, 0.0, RenderProps::default())],
            StudioRenderPhase::All,
            target.view(),
            EDGE,
            EDGE,
            &depth,
        );
        assert_eq!(target.read_rgba(&context).unwrap(), old);
        clear(
            &context,
            &target,
            &depth,
            BACKGROUND.map(|x| f64::from(x) / 255.0),
        );
        let props = RenderProps::from_entity(1, 128, [200, 0, 0], 0);
        renderer.render_with_props(
            &context,
            &model,
            &camera(),
            &[instance(&pose, 0.0, props)],
            StudioRenderPhase::All,
            target.view(),
            EDGE,
            EDGE,
            &depth,
        );
        let a = 128.0 / 255.0;
        close(
            pixel(&context, &target),
            if flags == STUDIO_NF_MASKED {
                BACKGROUND.map(f32::from)
            } else {
                std::array::from_fn(|i| {
                    f32::from(props.color[i]) * a + f32::from(BACKGROUND[i]) * (1.0 - a)
                })
            },
        );
    }
    let (model, pose) = synthetic_model(TEXTURE, 255, 0);
    let mut renderer = StudioRenderer::new(&context, &model, OFFSCREEN_FORMAT).unwrap();
    for reverse in [false, true] {
        let mut entries = [
            instance(&pose, 1.0, RenderProps::from_entity(1, 128, [200, 0, 0], 0)),
            instance(&pose, 0.0, RenderProps::from_entity(1, 128, [0, 0, 200], 0)),
        ];
        if reverse {
            entries.reverse();
        }
        clear(
            &context,
            &target,
            &depth,
            BACKGROUND.map(|x| f64::from(x) / 255.0),
        );
        renderer.render_with_props(
            &context,
            &model,
            &camera(),
            &entries,
            StudioRenderPhase::All,
            target.view(),
            EDGE,
            EDGE,
            &depth,
        );
        let a = 128.0 / 255.0;
        close(
            pixel(&context, &target),
            std::array::from_fn(|i| {
                [200.0, 0.0, 0.0][i] * a
                    + ([0.0, 0.0, 200.0][i] * a + f32::from(BACKGROUND[i]) * (1.0 - a)) * (1.0 - a)
            }),
        );
    }
}

fn mixed_skin_model() -> (StudioModel, StudioPose) {
    let (mut bytes, layout) = ohl_formats::test_support::build_minimal_mdl10();
    let texture = bytes[layout.textures_offset..layout.textures_offset + 80].to_vec();
    let texture_offset = u32::try_from(bytes.len()).unwrap();
    bytes.extend_from_slice(&texture);
    bytes.extend_from_slice(&texture);
    let skin_offset = u32::try_from(bytes.len()).unwrap();
    for entry in [0_u16, 1, 1, 0] {
        bytes.extend_from_slice(&entry.to_le_bytes());
    }
    let length = u32::try_from(bytes.len()).unwrap();
    for (offset, value) in [
        (72, length),
        (180, 2),
        (184, texture_offset),
        (192, 2),
        (196, 2),
        (200, skin_offset),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut model = StudioModel::parse(&bytes, &StudioLimits::default()).unwrap();
    model.textures[0].image = TextureImage::new(1, 1, vec![100, 16, 8, 255]).unwrap();
    model.textures[0].flags = STUDIO_NF_FULLBRIGHT;
    model.textures[1].image = TextureImage::new(1, 1, vec![8, 40, 80, 255]).unwrap();
    model.textures[1].flags = STUDIO_NF_FULLBRIGHT | STUDIO_NF_ADDITIVE;
    for vertex in &mut model.vertices {
        vertex.position[0] *= 0.5;
    }
    let vertex_count = u32::try_from(model.vertices.len()).unwrap();
    let right: Vec<_> = model
        .vertices
        .iter()
        .map(|vertex| {
            let mut vertex = *vertex;
            vertex.position[0] += 0.5;
            vertex
        })
        .collect();
    let indices: Vec<_> = model
        .indices
        .iter()
        .map(|index| index + vertex_count)
        .collect();
    let mut mesh = model.meshes[0];
    mesh.first_index = u32::try_from(model.indices.len()).unwrap();
    mesh.skin_slot = 1;
    model.vertices.extend(right);
    model.indices.extend(indices);
    model.meshes.push(mesh);
    model.body_parts[0].models[0].meshes.push(1);
    model.bounds_min = [0.0; 3];
    model.bounds_max = [1.0, 1.0, 0.0];
    let mut pose = StudioPose::bind(&model);
    pose.matrices.fill(math::identity());
    (model, pose)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn pixel_at_model_point(
    context: &GpuContext,
    target: &OffscreenTarget,
    point: [f32; 3],
) -> [u8; 3] {
    let m = camera().view_projection(1.0);
    let clip: [f32; 4] = std::array::from_fn(|row| {
        m[row] * point[0] + m[4 + row] * point[1] + m[8 + row] * point[2] + m[12 + row]
    });
    let x = clip[0] / clip[3];
    let y = clip[1] / clip[3];
    assert!(x.is_finite() && y.is_finite() && x.abs() < 1.0 && y.abs() < 1.0);
    let x = ((x + 1.0) * 0.5 * EDGE as f32) as u32;
    let y = ((1.0 - y) * 0.5 * EDGE as f32) as u32;
    let image = target.read_rgba(context).unwrap();
    let offset = usize::try_from((y * EDGE + x) * 4).unwrap();
    image[offset..offset + 3].try_into().unwrap()
}

#[test]
fn normal_studio_resolves_skin_and_mixed_mesh_phase_and_depth_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let context = GpuContext::headless().expect("explicit GPU opt-in requires an adapter");
    let target = OffscreenTarget::new(&context, EDGE, EDGE).unwrap();
    let depth = depth(&context);
    let (model, pose) = mixed_skin_model();
    let mut renderer = StudioRenderer::new(&context, &model, OFFSCREEN_FORMAT).unwrap();
    let (probe_model, probe_pose) = synthetic_model([0, 0, 200], 255, 0);
    let mut probe = StudioRenderer::new(&context, &probe_model, OFFSCREEN_FORMAT).unwrap();
    let points = [[0.25, 0.5, 0.0], [0.75, 0.5, 0.0]];
    for skin in [0, 1] {
        let mut entry = instance(&pose, 0.0, RenderProps::default());
        entry.instance.skin = skin;
        let selected = [skin, 1 - skin];
        clear(
            &context,
            &target,
            &depth,
            BACKGROUND.map(|value| f64::from(value) / 255.0),
        );
        renderer.render_with_props(
            &context,
            &model,
            &camera(),
            &[entry],
            StudioRenderPhase::Opaque,
            target.view(),
            EDGE,
            EDGE,
            &depth,
        );
        assert_eq!(renderer.last_triangle_count(), 2);
        for (point, material) in points.into_iter().zip(selected) {
            close(
                pixel_at_model_point(&context, &target, point),
                if material == 0 {
                    [100.0, 16.0, 8.0]
                } else {
                    BACKGROUND.map(f32::from)
                },
            );
        }
        let mut entry = instance(&pose, 0.0, RenderProps::default());
        entry.instance.skin = skin;
        renderer.render_with_props(
            &context,
            &model,
            &camera(),
            &[entry],
            StudioRenderPhase::Translucent,
            target.view(),
            EDGE,
            EDGE,
            &depth,
        );
        assert_eq!(renderer.last_triangle_count(), 2);
        for (point, material) in points.into_iter().zip(selected) {
            close(
                pixel_at_model_point(&context, &target, point),
                if material == 0 {
                    [100.0, 16.0, 8.0]
                } else {
                    std::array::from_fn(|channel| {
                        f32::from(BACKGROUND[channel]) + [8.0, 40.0, 80.0][channel]
                    })
                },
            );
        }
        probe.render_with_props(
            &context,
            &probe_model,
            &camera(),
            &[instance(&probe_pose, -0.5, RenderProps::default())],
            StudioRenderPhase::All,
            target.view(),
            EDGE,
            EDGE,
            &depth,
        );
        for (point, material) in points.into_iter().zip(selected) {
            close(
                pixel_at_model_point(&context, &target, point),
                if material == 0 {
                    [100.0, 16.0, 8.0]
                } else {
                    [0.0, 0.0, 200.0]
                },
            );
        }
    }
}
