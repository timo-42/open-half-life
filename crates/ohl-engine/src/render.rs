//! Drawing one frame of a [`crate::Game`].
//!
//! The pass order mirrors what each `ohl-render` entry point expects:
//! opaque world (which clears colour and depth), studio models over the
//! world's depth buffer, the skybox behind everything that has already been
//! written, then the translucent passes — brush-entity submodels and
//! liquids — which read depth without clearing it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use glam::{Mat4, Quat, Vec3};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_render::{
    BlendKind, EffectInstance, FreeFlyCamera, GpuContext, LightStyles, ModelInstance, RenderProps,
    RenderedModelInstance, SkyRenderer, SpriteInstance, StudioRenderPhase, StudioRenderer,
    SubmodelInstance, WorldRenderer, math, placement, wgpu,
};
use ohl_world::{Aabb, Frustum, StudioPose};

use crate::components::StudioAnim;
use crate::error::{EngineError, Result};
use crate::level::{Level, PropPlacement};
use crate::sprites::TransientSprite;
use crate::viewmodel::{self, ViewModelFrame};

#[path = "visual_effects.rs"]
mod visual_effects;

#[derive(Clone, Copy)]
enum VisualDraw<'a> {
    Sprite(SpriteInstance<'a>),
    Effect(EffectInstance),
}

impl VisualDraw<'_> {
    fn writes_depth(self) -> bool {
        match self {
            Self::Sprite(sprite) => sprite.render_props.blend_kind() == BlendKind::Opaque,
            Self::Effect(effect) => effect.writes_depth(),
        }
    }

    fn view_depth(self, camera: &FreeFlyCamera) -> f32 {
        let origin = match self {
            Self::Sprite(sprite) => sprite.origin,
            Self::Effect(effect) => effect.center(),
        };
        math::dot(
            std::array::from_fn(|axis| origin[axis] - camera.position[axis]),
            camera.direction(),
        )
    }

    fn is_sprite(self) -> bool {
        matches!(self, Self::Sprite(_))
    }
}

/// The colour target one [`crate::Game::render`] call draws into.
#[derive(Clone, Copy)]
pub struct RenderTarget<'a> {
    /// The view to draw into.
    pub view: &'a wgpu::TextureView,
    /// Its width in physical pixels.
    pub width: u32,
    /// Its height in physical pixels.
    pub height: u32,
    /// Its colour format, which the pipelines are built for.
    pub format: wgpu::TextureFormat,
}

/// The ambient level used when a model's origin samples no lighting.
const FALLBACK_AMBIENT: [f32; 3] = [0.35, 0.35, 0.35];

/// The directional key light's colour.
const KEY_LIGHT: [f32; 3] = [0.75, 0.75, 0.75];

/// Cumulative uploads for the current level, useful for frame profiling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderResourceStats {
    /// Static brush geometry and texture preparation.
    pub submodels: ohl_render::SubmodelResourceStats,
    /// World lightmap updates after initial resource creation.
    pub lightmap_uploads: u64,
}

/// CPU time spent preparing and submitting each pass, not GPU timestamp
/// queries. Enabled only by `OHL_PROFILE_RENDER_STAGES=1`; all output fields
/// are fixed labels and numeric timings, never map-derived strings.
#[derive(Default)]
struct RenderStageTimings {
    totals: [Duration; 8],
    frames: u32,
}

#[derive(Clone, Copy)]
enum RenderStage {
    Lightmap,
    World,
    Studio,
    Sky,
    Brush,
    Liquid,
    Sprite,
    Viewmodel,
}

impl RenderStageTimings {
    fn finish_frame(&mut self) {
        self.frames += 1;
        if self.frames < 120 {
            return;
        }
        let milliseconds = self
            .totals
            .map(|total| total.as_secs_f64() * 1000.0 / f64::from(self.frames));
        eprintln!(
            "[info] render stage profile frames={} lightmap_ms={:.3} world_ms={:.3} studio_ms={:.3} sky_ms={:.3} brush_ms={:.3} liquid_ms={:.3} sprite_ms={:.3} viewmodel_ms={:.3}",
            self.frames,
            milliseconds[RenderStage::Lightmap as usize],
            milliseconds[RenderStage::World as usize],
            milliseconds[RenderStage::Studio as usize],
            milliseconds[RenderStage::Sky as usize],
            milliseconds[RenderStage::Brush as usize],
            milliseconds[RenderStage::Liquid as usize],
            milliseconds[RenderStage::Sprite as usize],
            milliseconds[RenderStage::Viewmodel as usize],
        );
        *self = Self::default();
    }
}

/// The GPU-side resources for one loaded level.
pub(crate) struct Renderers {
    world: WorldRenderer,
    effects: ohl_render::EffectRenderer,
    fade: FadeRenderer,
    map_effects: visual_effects::MapEffects,
    submodels: BTreeMap<u32, ohl_render::PreparedSubmodel>,
    lightmap_uploads: u64,
    stage_timings: Option<RenderStageTimings>,
    sky: Option<SkyRenderer>,
    studio: Vec<StudioRenderer>,
    debris_studio: Vec<Option<StudioRenderer>>,
}

impl Renderers {
    pub(crate) fn resource_stats(&self) -> RenderResourceStats {
        RenderResourceStats {
            submodels: self.world.submodel_resource_stats(),
            lightmap_uploads: self.lightmap_uploads,
        }
    }

    pub(crate) fn new(
        context: &GpuContext,
        level: &Level,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let world =
            WorldRenderer::new(context, &level.world, format).map_err(|_| EngineError::Renderer)?;
        let submodels = level
            .submodels
            .iter()
            .map(|(&index, model)| (index, world.prepare_map_submodel(context, model)))
            .collect();
        // A skybox that will not upload is not worth failing the level for:
        // the world still draws, just against the clear colour.
        let sky = level
            .skybox
            .as_ref()
            .and_then(|skybox| SkyRenderer::new(context, skybox, format).ok());
        let mut studio = Vec::with_capacity(level.studio_models.len());
        for model in &level.studio_models {
            let Ok(renderer) = StudioRenderer::new(context, model, format) else {
                // Keep the slots aligned with `Level::studio_models` by
                // stopping here: a model this device cannot upload means the
                // remaining ones are not addressable by index any more.
                break;
            };
            studio.push(renderer);
        }
        Ok(Self {
            world,
            effects: ohl_render::EffectRenderer::new(context, format),
            fade: FadeRenderer::new(context, format),
            map_effects: visual_effects::MapEffects::new(level),
            submodels,
            lightmap_uploads: 0,
            stage_timings: (std::env::var_os("OHL_PROFILE_RENDER_STAGES").as_deref()
                == Some(std::ffi::OsStr::new("1")))
            .then(RenderStageTimings::default),
            sky,
            studio,
            debris_studio: level
                .debris_models
                .models
                .iter()
                .map(|model| StudioRenderer::new(context, model, format).ok())
                .collect(),
        })
    }

    fn record_stage(&mut self, stage: RenderStage, checkpoint: &mut Option<Instant>) {
        if let (Some(timings), Some(previous)) = (&mut self.stage_timings, checkpoint) {
            let now = Instant::now();
            timings.totals[stage as usize] += now.duration_since(*previous);
            *previous = now;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        light_styles: &LightStyles,
        elapsed: f32,
        target: RenderTarget<'_>,
        view_model: Option<&ViewModelFrame>,
        transient_sprites: &[TransientSprite],
        gameplay_effects: &crate::map_effects::EffectPresentation<'_>,
    ) {
        let (width, height) = (target.width.max(1), target.height.max(1));
        let mut checkpoint = self.stage_timings.as_ref().map(|_| Instant::now());

        // Reblend only when the intensities used by this map change.
        if self
            .world
            .update_light_styles(context, &level.world, light_styles, elapsed)
        {
            self.lightmap_uploads += 1;
        }
        self.record_stage(RenderStage::Lightmap, &mut checkpoint);
        self.world
            .render(context, &level.world, camera, target.view, width, height);
        self.record_stage(RenderStage::World, &mut checkpoint);

        let depth = self.world.depth_view().cloned();
        let (studio_frame, custom_ready) =
            self.collect_studio_instances(level, camera, gameplay_effects);
        if let Some(depth) = depth.as_ref() {
            self.draw_props(
                context,
                level,
                camera,
                depth,
                target,
                &studio_frame,
                StudioRenderPhase::Opaque,
            );
        }
        self.record_stage(RenderStage::Studio, &mut checkpoint);

        if let (Some(sky), Some(depth)) = (self.sky.as_ref(), depth.as_ref()) {
            sky.render(context, camera, target.view, depth, width, height);
        }
        self.record_stage(RenderStage::Sky, &mut checkpoint);

        self.draw_brush_entities(context, level, camera, target);
        self.record_stage(RenderStage::Brush, &mut checkpoint);

        self.world
            .render_liquid(context, camera, target.view, width, height, elapsed, 1.0);
        self.record_stage(RenderStage::Liquid, &mut checkpoint);

        self.draw_visuals(
            context,
            level,
            camera,
            elapsed,
            target,
            transient_sprites,
            gameplay_effects,
            &custom_ready,
        );
        self.record_stage(RenderStage::Sprite, &mut checkpoint);
        if let Some(depth) = depth.as_ref() {
            self.draw_props(
                context,
                level,
                camera,
                depth,
                target,
                &studio_frame,
                StudioRenderPhase::Translucent,
            );
        }
        self.record_stage(RenderStage::Studio, &mut checkpoint);

        // M7.9 P3: the view model, drawn last, after everything else. Its
        // depth is reset first (a manual depth-only clear, since
        // `StudioRenderer::render`'s two modes are "load both" or "clear
        // both" and clearing colour here would erase the whole frame just
        // drawn), so a weapon pressed against a wall is never clipped by
        // world geometry — but it can still occlude nothing after it, since
        // nothing else draws this frame.
        if let (Some(frame), Some(depth)) = (view_model, depth.as_ref()) {
            self.draw_view_model(context, level, frame, depth, target);
        }
        if let Some(fade) = gameplay_effects.fade {
            self.fade.draw(context, target.view, fade);
        }
        self.record_stage(RenderStage::Viewmodel, &mut checkpoint);
        if let Some(timings) = &mut self.stage_timings {
            timings.finish_frame();
        }
    }

    /// Resets `depth` to the far plane without touching whatever colour is
    /// already in `target`, then draws `frame`'s model into both.
    fn draw_view_model(
        &mut self,
        context: &GpuContext,
        level: &Level,
        frame: &ViewModelFrame,
        depth: &wgpu::TextureView,
        target: RenderTarget<'_>,
    ) {
        let Some(renderer) = self.studio.get_mut(frame.model_slot) else {
            return;
        };
        let Some(model) = level.studio_models.get(frame.model_slot) else {
            return;
        };
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl viewmodel depth reset"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl viewmodel depth reset pass"),
                color_attachments: &[],
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
        context.queue.submit(std::iter::once(encoder.finish()));

        let instances = [viewmodel::instance(frame.transform, &frame.pose, KEY_LIGHT)];
        renderer.render(
            context,
            model,
            &frame.camera,
            &instances,
            target.view,
            target.width.max(1),
            target.height.max(1),
            Some(depth),
        );
    }

    /// Draws placed/transient sprites and effect primitives in one order.
    /// Opaque geometry writes depth first; alpha/additive draws interleave
    /// back-to-front. Consecutive items of one renderer share a submission.
    /// Brush/liquid/studio transparency and intersecting primitives remain
    /// outside this bounded compositor; see the milestone limits.
    #[allow(clippy::too_many_arguments)]
    fn draw_visuals(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        elapsed: f32,
        target: RenderTarget<'_>,
        transient_sprites: &[TransientSprite],
        gameplay_effects: &crate::map_effects::EffectPresentation<'_>,
        custom_ready: &BTreeSet<u64>,
    ) {
        let mut instances: Vec<SpriteInstance<'_>> = level
            .sprites
            .iter()
            .filter_map(|sprite| {
                if !sprite.initially_visible {
                    return None;
                }
                let entity = *level.registry.entities.get(sprite.entity_index)?;
                let transform = level.registry.world.get::<&Transform>(entity).ok()?;
                let asset = level.sprite_assets.get(sprite.sprite)?;
                Some(SpriteInstance {
                    asset,
                    origin: transform.origin.to_array(),
                    scale: sprite.scale,
                    render_props: live_render_props(level, entity, sprite.render),
                    frame_time: elapsed * sprite.framerate / ohl_world::MAX_SPRITE_FRAMERATE,
                })
            })
            .collect();
        instances.extend(transient_sprites.iter().filter_map(|sprite| {
            let asset = level.sprite_assets.get(sprite.asset)?;
            Some(SpriteInstance {
                asset,
                origin: sprite.origin,
                scale: sprite.scale,
                render_props: sprite.render,
                frame_time: sprite.age,
            })
        }));
        self.map_effects.sample(level, elapsed);
        self.map_effects
            .append_gameplay(level, gameplay_effects, custom_ready);
        let Some(depth) = self.world.depth_view().cloned() else {
            return;
        };
        let mut draws: Vec<_> = instances
            .iter()
            .copied()
            .map(VisualDraw::Sprite)
            .chain(
                self.map_effects
                    .instances
                    .iter()
                    .copied()
                    .map(VisualDraw::Effect),
            )
            .collect();
        draws.sort_by(|a, b| match (a.writes_depth(), b.writes_depth()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => b.view_depth(camera).total_cmp(&a.view_depth(camera)),
        });
        let mut index = 0;
        while index < draws.len() {
            let is_sprite = draws[index].is_sprite();
            let mut end = index + 1;
            while end < draws.len() && draws[end].is_sprite() == is_sprite {
                end += 1;
            }
            self.draw_visual_batch(context, &draws[index..end], camera, target, &depth);
            index = end;
        }
    }

    fn draw_visual_batch(
        &mut self,
        context: &GpuContext,
        draws: &[VisualDraw<'_>],
        camera: &FreeFlyCamera,
        target: RenderTarget<'_>,
        depth: &wgpu::TextureView,
    ) {
        if draws[0].is_sprite() {
            let sprites: Vec<_> = draws
                .iter()
                .filter_map(|draw| match *draw {
                    VisualDraw::Sprite(sprite) => Some(sprite),
                    VisualDraw::Effect(_) => None,
                })
                .collect();
            self.world.draw_sprites(
                context,
                &sprites,
                camera,
                target.view,
                target.width.max(1),
                target.height.max(1),
            );
        } else {
            let effects: Vec<_> = draws
                .iter()
                .filter_map(|draw| match *draw {
                    VisualDraw::Effect(effect) => Some(effect),
                    VisualDraw::Sprite(_) => None,
                })
                .collect();
            self.effects.draw(
                context,
                &effects,
                camera,
                target.view,
                depth,
                target.width.max(1),
                target.height.max(1),
            );
        }
    }

    /// Pair identity, live properties and sampled pose before any filtering or
    /// model-slot grouping. Both phases borrow this one frame preparation.
    fn collect_studio_instances(
        &self,
        level: &Level,
        camera: &FreeFlyCamera,
        effects: &crate::map_effects::EffectPresentation<'_>,
    ) -> (Vec<StudioFrame>, BTreeSet<u64>) {
        let mut frame = Vec::new();
        for (entity, anim, transform) in &mut level
            .registry
            .world
            .query::<(Entity, &StudioAnim, &Transform)>()
        {
            let prop = PropPlacement {
                model: anim.model,
                origin: transform.origin.to_array(),
                yaw: transform.angles.y,
                sequence: anim.sequence,
                body: anim.body,
                skin: anim.skin,
                cycle: anim.cycle,
            };
            let Some(model) = level.studio_models.get(prop.model) else {
                continue;
            };
            if self.studio.get(prop.model).is_none() {
                continue;
            }
            let Ok(pose) = StudioPose::sample(model, prop.sequence, prop.cycle) else {
                continue;
            };
            let mut entry = StudioFrame {
                source: StudioSource::Entity(prop.model),
                id: entity.to_bits().get(),
                transform: placement(prop.origin, prop.yaw),
                pose: Some(pose),
                body: vec![prop.body],
                skin: prop.skin,
                ambient: ambient_at(level, prop.origin),
                props: live_render_props(
                    level,
                    entity,
                    ohl_game::keyvalues::RenderProps::default(),
                ),
                depth: 0.0,
            };
            entry.depth = entry
                .instance(level)
                .expect("prepared pose")
                .view_depth(model, camera);
            frame.push(entry);
        }
        let mut ready = BTreeSet::new();
        for record in effects.debris {
            let Some((slot, transform)) = level.debris_models.placement(record) else {
                continue;
            };
            if self.debris_studio.get(slot).is_none_or(Option::is_none) {
                continue;
            }
            let Some(model) = level.debris_models.models.get(slot) else {
                continue;
            };
            let mut entry = StudioFrame {
                source: StudioSource::Debris(slot),
                id: record.id,
                transform,
                pose: None,
                body: Vec::new(),
                skin: 0,
                ambient: ambient_at(level, record.position.to_array()),
                props: RenderProps::default(),
                depth: 0.0,
            };
            let Some(instance) = entry.instance(level) else {
                continue;
            };
            entry.depth = instance.view_depth(model, camera);
            ready.insert(record.id);
            frame.push(entry);
        }
        frame.sort_by_key(|entry| (entry.source.kind(), entry.id));
        (frame, ready)
    }

    /// Opaque geometry goes early; translucent meshes follow the completed
    /// opaque sprite/effect depth. Global cross-family transparency remains cut.
    #[allow(clippy::too_many_arguments)]
    fn draw_props(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        depth: &wgpu::TextureView,
        target: RenderTarget<'_>,
        frame: &[StudioFrame],
        phase: StudioRenderPhase,
    ) {
        let mut order: Vec<_> = frame.iter().collect();
        if phase == StudioRenderPhase::Translucent {
            order.sort_by(|a, b| {
                b.depth
                    .total_cmp(&a.depth)
                    .then((a.source.kind(), a.id).cmp(&(b.source.kind(), b.id)))
            });
        }
        for entry in order {
            let (model, renderer) = match entry.source {
                StudioSource::Entity(slot) => {
                    (level.studio_models.get(slot), self.studio.get_mut(slot))
                }
                StudioSource::Debris(slot) => (
                    level.debris_models.models.get(slot),
                    self.debris_studio.get_mut(slot).and_then(Option::as_mut),
                ),
            };
            let (Some(model), Some(renderer), Some(instance)) =
                (model, renderer, entry.instance(level))
            else {
                continue;
            };
            renderer.render_with_props(
                context,
                model,
                camera,
                &[instance],
                phase,
                target.view,
                target.width.max(1),
                target.height.max(1),
                depth,
            );
        }
    }

    /// Draws every brush entity's submodel with its own render mode, offset
    /// by whatever the map-logic simulation has moved it to.
    fn draw_brush_entities(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        target: RenderTarget<'_>,
    ) {
        #[allow(clippy::cast_precision_loss)]
        let aspect = target.width.max(1) as f32 / target.height.max(1) as f32;
        let frustum = camera.frustum(aspect);
        let mut draws = Vec::new();
        for instance in ohl_game::brush::model_instances(&level.registry) {
            let Some(model) = level.submodels.get(&instance.model_index) else {
                continue;
            };
            let transform = brush_placement(&level.registry, instance.entity, instance.origin);
            if !brush_in_frustum(&model.bounds, &transform, &frustum) {
                continue;
            }
            let Some(model) = self.submodels.get(&instance.model_index) else {
                continue;
            };
            draws.push((
                SubmodelInstance { model, transform },
                live_render_props(level, instance.entity, instance.render),
            ));
        }
        self.world.draw_world_submodels(
            context,
            &draws,
            camera,
            target.view,
            target.width.max(1),
            target.height.max(1),
        );
    }
}

#[derive(Clone, Copy)]
enum StudioSource {
    Entity(usize),
    Debris(usize),
}

impl StudioSource {
    fn kind(self) -> u8 {
        match self {
            Self::Entity(_) => 0,
            Self::Debris(_) => 1,
        }
    }
}

struct StudioFrame {
    source: StudioSource,
    id: u64,
    transform: math::Mat4,
    pose: Option<StudioPose>,
    body: Vec<u32>,
    skin: usize,
    ambient: [f32; 3],
    props: RenderProps,
    depth: f32,
}

impl StudioFrame {
    fn instance<'a>(&'a self, level: &'a Level) -> Option<RenderedModelInstance<'a>> {
        let pose = match self.source {
            StudioSource::Entity(_) => self.pose.as_ref()?,
            StudioSource::Debris(slot) => level.debris_models.poses.get(slot)?,
        };
        Some(RenderedModelInstance {
            instance: ModelInstance {
                transform: self.transform,
                pose,
                body: &self.body,
                skin: self.skin,
                ambient: self.ambient,
                light_direction: ModelInstance::default_light_direction(),
                light_color: KEY_LIGHT,
            },
            render_props: self.props,
        })
    }
}

// Full-screen presentation uses blend hardware, with no scene readback or
// mutable camera state. Host HUD/UI draws after Game::render returns.
struct FadeRenderer {
    pipelines: [wgpu::RenderPipeline; 2],
    uniform: wgpu::Buffer,
    binding: wgpu::BindGroup,
    srgb: bool,
}

impl FadeRenderer {
    fn new(context: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let device = &context.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ohl map fade shader"),
            source: wgpu::ShaderSource::Wgsl(r"
                @group(0) @binding(0) var<uniform> color: vec4<f32>;
                @vertex fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
                    let points = array<vec2<f32>, 3>(vec2<f32>(-1.0,-1.0), vec2<f32>(3.0,-1.0), vec2<f32>(-1.0,3.0));
                    return vec4<f32>(points[index], 0.0, 1.0);
                }
                @fragment fn fragment_main() -> @location(0) vec4<f32> { return color; }
            ".into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ohl map fade layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ohl map fade color"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ohl map fade binding"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ohl map fade pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let modulation = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipelines = [wgpu::BlendState::ALPHA_BLENDING, modulation].map(|blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ohl map fade pipeline"),
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
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        });
        Self {
            pipelines,
            uniform,
            binding,
            srgb: format.is_srgb(),
        }
    }

    fn draw(
        &self,
        context: &GpuContext,
        target: &wgpu::TextureView,
        mut fade: crate::map_effects::FadeOverlay,
    ) {
        if fade.amount <= 0.0 {
            return;
        }
        if self.srgb {
            fade.color = fade.color.map(|value| {
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            });
        }
        let color = if fade.modulate {
            fade.composite([1.0; 3])
        } else {
            fade.color
        };
        let rgba = [
            color[0],
            color[1],
            color[2],
            if fade.modulate { 1.0 } else { fade.amount },
        ];
        let bytes: Vec<u8> = rgba.into_iter().flat_map(f32::to_le_bytes).collect();
        context.queue.write_buffer(&self.uniform, 0, &bytes);
        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ohl map fade encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ohl map fade pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            pass.set_pipeline(&self.pipelines[usize::from(fade.modulate)]);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.draw(0..3, 0..1);
        }
        context.queue.submit([encoder.finish()]);
    }
}

/// The world placement matrix a brush entity's compiled submodel is drawn
/// with: `instance_origin` (its `origin` keyvalue) plus however far its own
/// state machine has moved it, and, for a rotating mover or a turning
/// `func_tracktrain`, the live rotation `brush_pose_rotation` reports.
///
/// The same inputs `Level::sync_brush_collision` poses the collision hull
/// from, read from the same helpers, so a brush entity is drawn exactly
/// where it collides.
///
/// A translating brush entity is placed by *translation only*. Its authored
/// `angles`/`angle` keyvalue is deliberately not applied as a rotation: for
/// `func_door`, `func_button` and `func_plat` that keyvalue names the
/// direction the entity moves in (the `-1`/`-2` sentinels mean up/down; see
/// `ohl_game::registry::movedir_from_angles`), and a brush entity without an
/// origin brush has its geometry compiled in absolute world space, so any
/// yaw would swing it about the *map* origin. The previous
/// `placement(origin, angles.y)` did exactly that: a door authored `angle
/// -1` drew rotated one degree about `(0, 0, 0)`, a purely horizontal shift
/// of roughly 1.7% of its distance from the map origin — a few pixels
/// sideways from its own frame — while its collision hull sat in the right
/// place.
pub(crate) fn brush_placement(
    registry: &ohl_game::registry::Registry,
    entity: Entity,
    instance_origin: Vec3,
) -> math::Mat4 {
    let (rotation_axis, rotation_degrees, rotation_pivot) = brush_pose_rotation(registry, entity);
    let origin = instance_origin + brush_offset(registry, entity);
    if rotation_axis == Vec3::ZERO {
        placement(origin.to_array(), 0.0)
    } else {
        // A rotating mover's compiled geometry is stored relative to its
        // own origin keyvalue (see `rotated_placement`'s doc comment) and
        // never also carries a translating `brush_offset` (`Door::movedir`
        // is left `Vec3::ZERO` for a `func_door_rotating`, and
        // `func_rotating` has no offset source at all — see
        // `mover_rotation`'s doc comment), so adding `brush_offset` above
        // is a no-op for one. Only the live simulation-driven rotation
        // state matters, which for a `func_tracktrain` is the yaw it faces
        // its segment at.
        rotated_placement(origin, rotation_pivot, rotation_axis, rotation_degrees)
    }
}

/// Test the same placement used for drawing, including live mover rotation.
/// Unknown bounds must stay visible; `Frustum::intersects` itself rejects
/// empty bounds, which is appropriate for world faces but not an entity
/// whose geometry may still be drawable.
fn brush_in_frustum(bounds: &Aabb, transform: &math::Mat4, frustum: &Frustum) -> bool {
    if !bounds.is_valid()
        || !bounds.min.iter().chain(&bounds.max).all(|v| v.is_finite())
        || !transform.iter().all(|v| v.is_finite())
    {
        return true;
    }
    let transform = Mat4::from_cols_array(transform);
    let mut placed = Aabb::empty();
    // Rotating only the min/max corners can shrink the box incorrectly.
    // All eight corners enclose the entire transformed model instead.
    for x in [bounds.min[0], bounds.max[0]] {
        for y in [bounds.min[1], bounds.max[1]] {
            for z in [bounds.min[2], bounds.max[2]] {
                let point = transform.transform_point3(Vec3::new(x, y, z));
                if !point.is_finite() {
                    return true;
                }
                placed.extend(point.to_array());
            }
        }
    }
    // Keep geometry touching a clip plane despite f32 rounding in either
    // the placement or the plane extraction. This is far below one map unit.
    for (min, max) in placed.min.iter_mut().zip(&mut placed.max) {
        *min -= 0.01;
        *max += 0.01;
    }
    frustum.intersects(&placed)
}

/// The lighting a model standing at `origin` picks up, falling back to a
/// dim ambient where the map samples nothing.
fn ambient_at(level: &Level, origin: [f32; 3]) -> [f32; 3] {
    let sampled = level.world.ambient_at(origin);
    if sampled.iter().all(|channel| *channel <= f32::EPSILON) {
        FALLBACK_AMBIENT
    } else {
        sampled
    }
}

/// The brush-mover pose helpers, re-exported from `ohl-game` so the
/// renderer, the collision attachment in `crate::level`, and the `use`
/// proximity check in `ohl_game::find_usable_within` all read one
/// implementation of "where is this brush entity right now" rather than
/// three. See `ohl_game::pose` for the placement rule they share.
pub(crate) use ohl_game::pose::{brush_offset, brush_pose_rotation};

/// A world-space placement matrix for a rotating brush entity: rotate the
/// submodel's own compiled vertices by `angle_degrees` about `axis`, then
/// translate by `origin`.
///
/// Unlike an ordinary brush entity's geometry (baked in world space, so
/// [`placement`] only ever has to add a translation on top of it), a
/// `func_door_rotating`/`func_rotating` *requires* an origin brush (TWHL
/// wiki `func_door_rotating`: it "gives it the axis to rotate on";
/// `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic"), and the
/// compiler responds by storing that submodel's geometry relative to the
/// origin brush's own position rather than in absolute world space — the
/// same convention `track_train_transform`'s doc comment already records
/// for `func_train`'s origin keyvalue ("the compiler writes that origin
/// brush's position into the entity's `origin` keyvalue and stores the
/// submodel's geometry relative to it"). So for such a mover the pivot to
/// rotate about is simply the submodel's own local origin (`Vec3::ZERO`),
/// and `origin` is added *after* rotating, exactly the
/// same "rotate, then translate" order [`placement`] already uses for a
/// yaw-only rotation. A zero angle still needs the `origin` translation —
/// `Quat::from_axis_angle` with a zero angle is already the identity
/// quaternion, so the composed matrix is a pure translation by `origin`,
/// exactly matching a closed door's or idle rotator's resting pose — so
/// only a zero `axis` (whose `normalize()` would be NaN) short-circuits to
/// the identity matrix; every caller still branches on [`mover_rotation`]
/// first rather than relying on that, since a rotating mover never also
/// carries a translating [`brush_offset`] to add in.
///
/// `pivot` is that rotation centre, in the submodel's own *compiled*
/// frame, so a brush entity whose geometry is not centred on the point it
/// turns about can say so: a world-baked `func_tracktrain` has no origin
/// brush and turns about its chain's first node instead
/// (`ohl_game::pose::track_train_pivot`). The composed matrix is
/// "translate to the pivot, rotate, translate back, then translate by
/// `origin`", which for the `Vec3::ZERO` pivot every rotating mover passes
/// is bit-identical to the plain rotate-then-translate above. The one
/// transform this builds is exactly what `Level::sync_brush_collision`
/// hands `ohl_physics::CollisionModel::set_brush_pose`, so the drawn car
/// and the colliding car are the same car.
pub(crate) fn rotated_placement(
    origin: Vec3,
    pivot: Vec3,
    axis: Vec3,
    angle_degrees: f32,
) -> math::Mat4 {
    if axis == Vec3::ZERO {
        return math::identity();
    }
    let rotation = Quat::from_axis_angle(axis.normalize(), angle_degrees.to_radians());
    let matrix = Mat4::from_translation(origin + pivot)
        * Mat4::from_quat(rotation)
        * Mat4::from_translation(-pivot);
    matrix.to_cols_array()
}

/// Maps `ohl-game`'s raw `rendermode`/`renderamt`/`rendercolor` keyvalues
/// onto the renderer's typed render properties.
fn live_render_props(
    level: &Level,
    entity: Entity,
    fallback: ohl_game::keyvalues::RenderProps,
) -> RenderProps {
    let (props, fx) = ohl_game::effects::effective_render_props(&level.registry, entity, fallback);
    // `renderamt` defaults to 0 when the key is absent, which for either of
    // the documented opaque modes means "fully opaque", not "invisible";
    // `RenderProps::from_entity` applies that rule (and the unknown-mode
    // fallback) for both `Normal` and `Solid`.
    RenderProps::from_entity(props.mode, props.amt, props.color, fx.0)
}

#[cfg(test)]
// Every comparison below is against an exact analytic value (a plain
// translation composed with an identity or 90-degree rotation), matching
// `save_sections.rs`'s own precedent for exact-comparison tests.
#[allow(clippy::float_cmp)]
mod tests {
    use std::collections::BTreeMap;

    use super::{brush_in_frustum, brush_placement, rotated_placement};
    use glam::{Mat4, Vec3};
    use ohl_formats::bsp30::Entity as RawEntity;
    use ohl_game::keyvalues::{Limits, parse_entities};
    use ohl_game::registry::Registry;
    use ohl_render::FreeFlyCamera;
    use ohl_world::{Aabb, Frustum};

    /// A registry holding one brush entity (`*1`, targetname `mover`) with
    /// the given keyvalues. This project's own synthetic fixture; no bytes
    /// here come from any game installation (see `docs/CLEAN_ROOM.md`).
    fn registry_with_brush(keys: &[(&str, &str)]) -> Registry {
        let mut pairs = vec![("targetname", "mover"), ("model", "*1")];
        pairs.extend_from_slice(keys);
        let raw: RawEntity = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let defs = parse_entities(&[raw], &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([600.0, -300.0, 0.0], [664.0, -292.0, 96.0]));
        Registry::build(&defs, &bounds, &Limits::default())
    }

    /// A translating brush entity's authored `angles`/`angle` is a move
    /// direction, not a rotation, so at rest it must be placed by the
    /// identity: its geometry is compiled in world space and any yaw would
    /// swing it about the map origin. The regression this pins: a
    /// `func_door` with `angle -1` ("up") drew rotated one degree about
    /// `(0, 0, 0)`, a horizontal shift of several units, while a `func_door`
    /// with a sideways yaw would have drawn a quarter turn away from its
    /// frame.
    #[test]
    fn translating_brush_entities_ignore_their_authored_yaw() {
        for (classname, angle_key, angle) in [
            ("func_door", "angle", "-1"),
            ("func_door", "angle", "-2"),
            ("func_door", "angles", "0 90 0"),
            ("func_button", "angle", "180"),
            ("func_plat", "angles", "0 45 0"),
            ("func_wall", "angles", "0 30 0"),
        ] {
            let registry = registry_with_brush(&[("classname", classname), (angle_key, angle)]);
            let entity = registry.find("mover")[0];
            let matrix = brush_placement(&registry, entity, Vec3::ZERO);
            assert_eq!(
                matrix,
                Mat4::IDENTITY.to_cols_array(),
                "{classname} {angle_key} {angle} must be placed by the identity at rest"
            );
        }
    }

    /// The live travel offset still applies on top: a door caught mid-slide
    /// is translated by that offset and nothing else.
    #[test]
    fn translating_brush_placement_is_the_live_offset_alone() {
        let registry = registry_with_brush(&[
            ("classname", "func_door"),
            ("angle", "-1"),
            ("speed", "100"),
            ("lip", "0"),
        ]);
        let entity = registry.find("mover")[0];
        {
            let mut door = registry
                .world
                .get::<&mut ohl_game::registry::Door>(entity)
                .expect("the fixture entity is a door");
            door.state = ohl_game::registry::MoverState::Open;
        }
        let expected = ohl_game::pose::brush_offset(&registry, entity);
        assert!(expected.z > 0.0, "an open up-door has travelled upward");
        let matrix = brush_placement(&registry, entity, Vec3::ZERO);
        assert_eq!(matrix, Mat4::from_translation(expected).to_cols_array());
    }

    #[test]
    fn brush_culling_uses_live_translation_and_camera_direction() {
        let camera = FreeFlyCamera {
            position: [0.0; 3],
            ..FreeFlyCamera::default()
        };
        let bounds = Aabb {
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        let ahead = Mat4::from_translation(Vec3::X * 100.0).to_cols_array();
        let behind = Mat4::from_translation(Vec3::NEG_X * 100.0).to_cols_array();
        assert!(brush_in_frustum(&bounds, &ahead, &camera.frustum(1.0)));
        assert!(!brush_in_frustum(&bounds, &behind, &camera.frustum(1.0)));
        let turned = FreeFlyCamera {
            yaw: 180.0,
            ..camera
        };
        assert!(!brush_in_frustum(&bounds, &ahead, &turned.frustum(1.0)));
        assert!(brush_in_frustum(&bounds, &behind, &turned.frustum(1.0)));
    }

    #[test]
    fn brush_culling_keeps_rotated_corners_and_rejects_rotated_outside_bounds() {
        let frustum = Frustum::from_view_projection(&Mat4::IDENTITY.to_cols_array());
        let bounds = Aabb {
            min: [-1.0, -1.0, 0.25],
            max: [1.0, 1.0, 0.75],
        };
        // The min and max corners both rotate to x=2. The other corners
        // extend to x=2-sqrt(2), inside the frustum's right plane at x=1.
        let rotated = rotated_placement(Vec3::new(2.0, 0.0, 0.0), Vec3::ZERO, Vec3::Z, 45.0);
        assert!(brush_in_frustum(&bounds, &rotated, &frustum));
        let outside = rotated_placement(Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, Vec3::Z, 45.0);
        assert!(!brush_in_frustum(&bounds, &outside, &frustum));
    }

    #[test]
    fn brush_culling_keeps_boxes_touching_each_clip_plane() {
        let identity = Mat4::IDENTITY.to_cols_array();
        let frustum = Frustum::from_view_projection(&identity);
        let centers = [
            Vec3::new(-1.5, 0.0, 0.5),
            Vec3::new(1.5, 0.0, 0.5),
            Vec3::new(0.0, -1.5, 0.5),
            Vec3::new(0.0, 1.5, 0.5),
            Vec3::new(0.0, 0.0, -0.5),
            Vec3::new(0.0, 0.0, 1.5),
        ];
        for center in centers {
            let bounds = Aabb {
                min: (center - Vec3::splat(0.5)).to_array(),
                max: (center + Vec3::splat(0.5)).to_array(),
            };
            assert!(brush_in_frustum(&bounds, &identity, &frustum));
        }
    }

    #[test]
    fn brush_culling_retains_unknown_bounds_and_nonfinite_placements() {
        let identity = Mat4::IDENTITY.to_cols_array();
        let frustum = Frustum::from_view_projection(&identity);
        for bounds in [
            Aabb::empty(),
            Aabb {
                min: [f32::NAN; 3],
                max: [1.0; 3],
            },
            Aabb {
                min: [f32::NEG_INFINITY; 3],
                max: [f32::INFINITY; 3],
            },
        ] {
            assert!(brush_in_frustum(&bounds, &identity, &frustum));
        }
        let bounds = Aabb {
            min: [2.0; 3],
            max: [3.0; 3],
        };
        for value in [f32::NAN, f32::INFINITY, f32::MAX] {
            let transform = Mat4::from_scale(Vec3::splat(value)).to_cols_array();
            assert!(brush_in_frustum(&bounds, &transform, &frustum));
        }
    }

    /// A closed rotating door or an idle `func_rotating` (angle exactly
    /// zero, the spawn state for every one of them that does not start
    /// open/spinning) must still place its submodel at `origin`: the
    /// blocking bug this test pins down returned the identity matrix in
    /// that case, silently discarding the translation and drawing the
    /// brush at world `(0, 0, 0)` instead (see the PR #107 review at
    /// `rotated_placement`, `crates/ohl-engine/src/render.rs`).
    #[test]
    fn rotated_placement_keeps_the_origin_translation_at_zero_angle() {
        let origin = Vec3::new(100.0, 200.0, 300.0);
        let matrix = rotated_placement(origin, Vec3::ZERO, Vec3::Z, 0.0);
        let translation = [matrix[12], matrix[13], matrix[14]];
        assert_eq!(translation, [100.0, 200.0, 300.0]);
        // At angle zero the rotation itself must also be the identity, so
        // a point at the local origin maps straight to `origin`.
        assert_eq!(matrix[0], 1.0);
        assert_eq!(matrix[5], 1.0);
        assert_eq!(matrix[10], 1.0);
    }

    /// A quarter turn keeps the same `origin` translation and rotates the
    /// basis vectors perpendicular to the axis, so this is not simply
    /// pinning the zero-angle case at the cost of the general one.
    #[test]
    fn rotated_placement_still_rotates_at_ninety_degrees() {
        let origin = Vec3::new(100.0, 200.0, 300.0);
        let matrix = rotated_placement(origin, Vec3::ZERO, Vec3::Z, 90.0);
        let translation = [matrix[12], matrix[13], matrix[14]];
        assert_eq!(translation, [100.0, 200.0, 300.0]);
        // Rotating +X by 90 degrees about +Z lands on +Y.
        assert!((matrix[0]).abs() < 1e-5);
        assert!((matrix[1] - 1.0).abs() < 1e-5);
    }

    /// A zero axis (no rotation configured at all) still has to fall back
    /// to the identity matrix rather than attempt `Vec3::ZERO.normalize()`,
    /// which is NaN.
    #[test]
    fn rotated_placement_zero_axis_is_identity() {
        let matrix = rotated_placement(Vec3::new(5.0, 6.0, 7.0), Vec3::ZERO, Vec3::ZERO, 45.0);
        assert_eq!(matrix, glam::Mat4::IDENTITY.to_cols_array());
    }
}

#[cfg(test)]
mod gameplay_frame_tests {
    use super::*;
    use crate::test_support::{entity_block, synthetic_map_bsp_with_entities};
    use crate::{Game, Input, MemoryAssets};
    use ohl_render::{OFFSCREEN_FORMAT, OffscreenTarget};

    const EDGE: u32 = 96;
    const VIEW: [f32; 3] = [42.0, 32.0, 128.0];

    fn gpu() -> GpuContext {
        let context = GpuContext::headless().expect("explicit GPU opt-in requires an adapter");
        let info = context.adapter.get_info();
        eprintln!(
            "synthetic gameplay GPU adapter: {} ({:?})",
            info.name, info.backend
        );
        context
    }

    fn capture(name: &str, rgba: &[u8]) {
        let Some(directory) = std::env::var_os("OHL_P6A_CAPTURE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut ppm = format!("P6\n{EDGE} {EDGE}\n255\n").into_bytes();
        for pixel in rgba.as_chunks::<4>().0 {
            ppm.extend_from_slice(&pixel[..3]);
        }
        std::fs::write(directory.join(format!("{name}.ppm")), ppm).unwrap();
    }

    fn assets(extra: &str) -> MemoryAssets {
        assets_with_model_scale(extra, 64.0)
    }

    fn assets_with_model_scale(extra: &str, model_scale: f32) -> MemoryAssets {
        let text = entity_block("worldspawn", [0.0; 3], 0.0, &[])
            + &entity_block("info_player_start", [0.0, 0.0, 40.0], 0.0, &[])
            + extra;
        let mut assets = MemoryAssets::new();
        assets.insert(
            "maps/ohl_frame_effects.bsp",
            synthetic_map_bsp_with_entities(&text),
        );
        let (mut bytes, layout) = ohl_formats::test_support::build_minimal_mdl10();
        // These four authored positions run around the square perimeter.
        // A triangle strip needs alternating sides to cover the entire square;
        // perimeter order leaves a triangular hole through its bounds center.
        for (corner, vertex) in [0_u16, 1, 3, 2].into_iter().enumerate() {
            let offset = layout.tricommands_offset + 2 + corner * 8;
            bytes[offset..offset + 2].copy_from_slice(&vertex.to_le_bytes());
        }
        for vertex in 0..4 {
            for axis in 0..2 {
                let offset = layout.verts_offset + vertex * 12 + axis * 4;
                let value =
                    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) * model_scale;
                bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        let palette = layout.texture_data_offset + 256;
        bytes[palette..palette + 768].fill(255);
        bytes[layout.textures_offset + 64..layout.textures_offset + 68]
            .copy_from_slice(&ohl_world::STUDIO_NF_FULLBRIGHT.to_le_bytes());
        for (offset, value) in [
            (112, 10.0_f32),
            (116, 0.0),
            (120, 0.0),
            (124, 10.0 + model_scale),
            (128, model_scale),
            (132, 0.0),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        assets.insert("models/ohl_frame_a.mdl", bytes.clone());
        assets.insert("models/ohl_frame_b.mdl", bytes);
        let mut sprite = ohl_formats::test_support::build_minimal_spr();
        sprite[8..12].copy_from_slice(&2_i32.to_le_bytes());
        for color in sprite[42..810].as_chunks_mut::<3>().0 {
            color.copy_from_slice(&[16, 160, 32]);
        }
        assets.insert("sprites/ohl_frame.spr", sprite);
        assets
    }

    struct Counted {
        assets: MemoryAssets,
        reads: std::cell::Cell<usize>,
    }

    impl crate::AssetSource for Counted {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            crate::AssetSource::read(&self.assets, path)
        }
    }

    fn game(assets: &MemoryAssets) -> Game {
        let mut game = Game::load(assets, "ohl_frame_effects").unwrap();
        game.set_viewpoint(VIEW, 89.9, 0.0);
        game
    }

    fn press(game: &mut Game) {
        game.tick(
            crate::tick::TICK_SECONDS,
            &Input {
                use_pressed: true,
                ..Input::default()
            },
        );
        for _ in 0..8 {
            game.tick(crate::tick::TICK_SECONDS, &Input::default());
        }
    }

    fn pixels(context: &GpuContext, game: &mut Game) -> Vec<u8> {
        let target = OffscreenTarget::new(context, EDGE, EDGE).unwrap();
        game.render(
            context,
            RenderTarget {
                view: target.view(),
                width: EDGE,
                height: EDGE,
                format: OFFSCREEN_FORMAT,
            },
        )
        .unwrap();
        target.read_rgba(context).unwrap()
    }

    fn center(pixels: &[u8]) -> [u8; 3] {
        let offset = usize::try_from((EDGE / 2 * EDGE + EDGE / 2) * 4).unwrap();
        pixels[offset..offset + 3].try_into().unwrap()
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn at_world(image: &[u8], camera: &FreeFlyCamera, point: Vec3) -> [u8; 3] {
        let clip = glam::Mat4::from_cols_array(&camera.view_projection(1.0)) * point.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.is_finite() && ndc.x.abs() < 1.0 && ndc.y.abs() < 1.0);
        let x = (ndc.x.midpoint(1.0) * EDGE as f32) as u32;
        let y = ((1.0 - ndc.y) * 0.5 * EDGE as f32) as u32;
        let offset = usize::try_from((y * EDGE + x) * 4).unwrap();
        image[offset..offset + 3].try_into().unwrap()
    }

    fn switch(target: &str) -> String {
        entity_block(
            "func_button",
            VIEW,
            0.0,
            &[("target", target), ("wait", "-1")],
        )
    }

    #[test]
    fn real_use_live_studio_and_fade_reach_pixels_and_restore_when_opted_in() {
        if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let context = gpu();
        let target = entity_block(
            "cycler",
            [0.0, 0.0, 32.0],
            0.0,
            &[
                ("targetname", "studio"),
                ("model", "models/ohl_frame_a.mdl"),
            ],
        );
        let scene = target
            + &switch("color")
            + &entity_block(
                "env_render",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "color"),
                    ("target", "studio"),
                    ("rendermode", "1"),
                    ("renderamt", "128"),
                    ("rendercolor", "255 0 0"),
                    ("renderfx", "17"),
                ],
            );
        let source = assets(&scene);
        let mut live = game(&source);
        let before_image = pixels(&context, &mut live);
        capture("live_studio_before", &before_image);
        let before = center(&before_image);
        assert!(before.iter().all(|&value| value > 240));
        press(&mut live);
        let after_image = pixels(&context, &mut live);
        capture("live_studio_after", &after_image);
        let after = center(&after_image);
        assert!(
            u16::from(after[0]) > u16::from(after[1]) + 80
                && u16::from(after[0]) > u16::from(after[2]) + 80,
            "before={before:?}, after={after:?}"
        );
        let saved = live.save_bytes(123).unwrap();
        let mut loaded = Game::load_bytes(&source, &saved).unwrap();
        loaded.set_viewpoint(VIEW, 89.9, 0.0);
        assert_eq!(center(&pixels(&context, &mut loaded)), after);
        assert!(
            loaded
                .to_save(123)
                .map_effects
                .unwrap()
                .render_fx
                .iter()
                .any(|&(_, fx)| fx == 17)
        );
        for modulate in [false, true] {
            assert_fade_pixels(&context, modulate);
        }
    }

    fn assert_fade_pixels(context: &GpuContext, modulate: bool) {
        let flags = if modulate { "3" } else { "1" };
        let scene = switch("fade")
            + &entity_block(
                "env_fade",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "fade"),
                    ("duration", "3"),
                    ("holdtime", "2"),
                    ("spawnflags", flags),
                    ("renderamt", "128"),
                    ("rendercolor", "64 128 192"),
                ],
            );
        let source = assets(&scene);
        let mut game = game(&source);
        let before = center(&pixels(context, &mut game));
        press(&mut game);
        let overlay = game.systems_mut().map_effects.presentation().fade.unwrap();
        let expected = overlay.composite(before.map(|x| f32::from(x) / 255.0));
        let image = pixels(context, &mut game);
        capture(
            if modulate {
                "fade_modulate"
            } else {
                "fade_normal"
            },
            &image,
        );
        let after = center(&image);
        for channel in 0..3 {
            assert!((f32::from(after[channel]) - expected[channel] * 255.0).abs() <= 2.0);
        }
        let mut loaded = Game::load_bytes(&source, &game.save_bytes(123).unwrap()).unwrap();
        loaded.set_viewpoint(VIEW, 89.9, 0.0);
        let cleared = pixels(context, &mut loaded);
        capture(&format!("fade_cleared_{modulate}"), &cleared);
        assert_eq!(center(&cleared), before);
    }

    fn custom_gib_scene() -> String {
        switch("break")
            + &entity_block(
                "func_breakable",
                [42.0, 32.0, 64.0],
                0.0,
                &[
                    ("targetname", "break"),
                    ("gibmodel", "models/ohl_frame_a.mdl"),
                ],
            )
            + &entity_block(
                "func_breakable",
                [100.0, 100.0, 32.0],
                0.0,
                &[
                    ("targetname", "reserve"),
                    ("gibmodel", "models/ohl_frame_b.mdl"),
                ],
            )
    }

    fn effect_pixels(
        context: &GpuContext,
        renderers: &mut Renderers,
        level: &Level,
        camera: &FreeFlyCamera,
        elapsed: f32,
        effects: &crate::map_effects::EffectPresentation<'_>,
    ) -> Vec<u8> {
        let target = OffscreenTarget::new(context, EDGE, EDGE).unwrap();
        renderers.draw(
            context,
            level,
            camera,
            &LightStyles::new(),
            elapsed,
            RenderTarget {
                view: target.view(),
                width: EDGE,
                height: EDGE,
                format: OFFSCREEN_FORMAT,
            },
            None,
            &[],
            effects,
        );
        target.read_rgba(context).unwrap()
    }

    fn assert_later_debris_slot_survives(
        context: &GpuContext,
        source: &Counted,
        camera: &FreeFlyCamera,
        mut switched: crate::save::GameSave,
    ) {
        // A missing first resource must not disable a later prepared model slot.
        for record in &mut switched.map_effects.as_mut().unwrap().debris {
            record.gib_model = Some("models/ohl_frame_b.mdl".to_owned());
        }
        let mut other = Game::from_save(source, &switched).unwrap();
        let (level, systems) = other.level_and_systems_mut();
        let mut renderers = Renderers::new(context, level, OFFSCREEN_FORMAT).unwrap();
        renderers.debris_studio[0] = None;
        assert_eq!(
            renderers
                .collect_studio_instances(level, camera, &systems.map_effects.presentation())
                .1
                .len(),
            6
        );
    }

    #[test]
    fn real_break_custom_gibs_draw_once_and_keep_fallback_on_resource_failure_when_opted_in() {
        if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let context = gpu();
        let extra = custom_gib_scene();
        let source = Counted {
            assets: assets(&extra),
            reads: std::cell::Cell::new(0),
        };
        let mut live = Game::load(&source, "ohl_frame_effects").unwrap();
        live.set_viewpoint(VIEW, 89.9, 0.0);
        let asset_reads = source.reads.get();
        press(&mut live);
        let saved = live.to_save(123);
        assert_eq!(saved.map_effects.as_ref().unwrap().debris.len(), 6);
        assert_eq!(
            source.reads.get(),
            asset_reads,
            "simulation never reads model assets"
        );
        let camera = *live.camera();
        let elapsed = live.elapsed();
        let (level, systems) = live.level_and_systems_mut();
        level.preload_debris_models(&source);
        assert_eq!(
            source.reads.get(),
            asset_reads,
            "cached success/failure paths are never retried"
        );
        let before = systems
            .map_effects
            .snapshot(&level.registry, level.player, &level.simulation);
        let effects = systems.map_effects.presentation();
        let mut renderers = Renderers::new(&context, level, OFFSCREEN_FORMAT).unwrap();
        let (frame, ready) = renderers.collect_studio_instances(level, &camera, &effects);
        assert_eq!(ready.len(), 6);
        assert_eq!(
            frame
                .iter()
                .filter(|entry| matches!(entry.source, StudioSource::Debris(_)))
                .count(),
            6
        );
        let empty = crate::map_effects::EffectPresentation {
            shake: effects.shake,
            fade: effects.fade,
            debris: &[],
            blasts: effects.blasts,
        };
        let background = effect_pixels(&context, &mut renderers, level, &camera, elapsed, &empty);
        let custom = effect_pixels(&context, &mut renderers, level, &camera, elapsed, &effects);
        capture("custom_background", &background);
        capture("custom_geometry", &custom);
        assert!(
            custom
                .as_chunks::<4>()
                .0
                .iter()
                .zip(background.as_chunks::<4>().0)
                .filter(|(drawn, empty)| drawn[..3] != empty[..3]
                    && drawn[..3].iter().all(|&channel| channel > 240))
                .count()
                >= 2,
            "custom meshes must contribute visible fullbright white pixels"
        );
        assert!(
            !renderers
                .map_effects
                .instances
                .iter()
                .any(|effect| matches!(effect, EffectInstance::Gib { .. }))
        );
        renderers.debris_studio[0] = None;
        let (_, ready) = renderers.collect_studio_instances(level, &camera, &effects);
        assert!(ready.is_empty());
        let fallback = effect_pixels(&context, &mut renderers, level, &camera, elapsed, &effects);
        capture("custom_fallback", &fallback);
        assert_eq!(
            renderers
                .map_effects
                .instances
                .iter()
                .filter(|effect| matches!(effect, EffectInstance::Gib { .. }))
                .count(),
            6
        );
        assert_ne!(
            custom, fallback,
            "the actual custom silhouette differs from material cuboids"
        );
        assert_eq!(
            systems
                .map_effects
                .snapshot(&level.registry, level.player, &level.simulation),
            before
        );
        assert_eq!(source.reads.get(), asset_reads);
        assert_later_debris_slot_survives(&context, &source, &camera, saved);
    }

    #[test]
    fn custom_gib_pixels_follow_saved_record_position_and_rotation_when_opted_in() {
        if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let context = gpu();
        let extra = switch("break")
            + &entity_block(
                "func_breakable",
                [42.0, 32.0, 64.0],
                0.0,
                &[
                    ("targetname", "break"),
                    ("gibmodel", "models/ohl_frame_a.mdl"),
                ],
            );
        // Keep the sampled sequence-zero translation large relative to this
        // unit model: substituting bind pose displaces the fitted mesh entirely
        // away from the independently asserted simulated record position.
        let source = assets_with_model_scale(&extra, 1.0);
        let mut live = game(&source);
        press(&mut live);
        let mut saved = live.to_save(123);
        saved.map_effects.as_mut().unwrap().debris.truncate(1);
        let first = Vec3::new(42.0, 24.0, 88.0);
        let second = Vec3::new(42.0, 40.0, 88.0);
        let mut images = Vec::new();
        for (position, angle) in [
            (first, 0.0),
            (first, std::f32::consts::FRAC_PI_4),
            (second, std::f32::consts::FRAC_PI_4),
        ] {
            let record = &mut saved.map_effects.as_mut().unwrap().debris[0];
            record.position = position;
            record.angles = Vec3::new(0.0, 0.0, angle);
            record.half_extents = Vec3::splat(3.0);
            let mut loaded = Game::from_save(&source, &saved).unwrap();
            loaded.set_viewpoint(VIEW, 89.9, 0.0);
            let state = loaded.to_save(123).map_effects.unwrap();
            let image = pixels(&context, &mut loaded);
            capture(&format!("custom_pose_{}", images.len()), &image);
            assert!(
                at_world(&image, loaded.camera(), position)
                    .iter()
                    .all(|&value| value > 240)
            );
            assert_eq!(loaded.to_save(123).map_effects.unwrap(), state);
            images.push(image);
        }
        assert_ne!(
            images[0], images[1],
            "actual record rotation changes the custom silhouette"
        );
        assert_ne!(
            images[1], images[2],
            "actual record position moves the custom mesh"
        );
        saved.map_effects.as_mut().unwrap().debris.clear();
        let mut empty = Game::from_save(&source, &saved).unwrap();
        empty.set_viewpoint(VIEW, 89.9, 0.0);
        let background = pixels(&context, &mut empty);
        assert_ne!(
            at_world(&images[0], empty.camera(), first),
            at_world(&background, empty.camera(), first)
        );
        assert_eq!(
            at_world(&images[2], empty.camera(), first),
            at_world(&background, empty.camera(), first)
        );
        assert_ne!(
            at_world(&images[2], empty.camera(), second),
            at_world(&background, empty.camera(), second)
        );
    }

    #[test]
    fn blank_selected_custom_gib_retains_exactly_one_cuboid_when_opted_in() {
        if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let context = gpu();
        let extra = switch("break")
            + &entity_block(
                "func_breakable",
                [42.0, 32.0, 64.0],
                0.0,
                &[
                    ("targetname", "break"),
                    ("gibmodel", "models/ohl_frame_a.mdl"),
                ],
            );
        let mut source = assets(&extra);
        source.insert(
            "models/ohl_frame_a.mdl",
            crate::debris::synthetic_blank_first_gib_model(),
        );
        let mut live = game(&source);
        press(&mut live);
        let camera = *live.camera();
        let elapsed = live.elapsed();
        let (level, systems) = live.level_and_systems_mut();
        let effects = systems.map_effects.presentation();
        assert_eq!(effects.debris.len(), 6);
        let mut renderers = Renderers::new(&context, level, OFFSCREEN_FORMAT).unwrap();
        assert!(
            renderers
                .collect_studio_instances(level, &camera, &effects)
                .1
                .is_empty()
        );
        let target = OffscreenTarget::new(&context, EDGE, EDGE).unwrap();
        renderers.draw(
            &context,
            level,
            &camera,
            &LightStyles::new(),
            elapsed,
            RenderTarget {
                view: target.view(),
                width: EDGE,
                height: EDGE,
                format: OFFSCREEN_FORMAT,
            },
            None,
            &[],
            &effects,
        );
        assert_eq!(
            renderers
                .map_effects
                .instances
                .iter()
                .filter(|effect| matches!(effect, EffectInstance::Gib { .. }))
                .count(),
            6
        );
    }

    #[test]
    fn studio_alpha_follows_opaque_sprite_background_and_depth_when_opted_in() {
        if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let context = gpu();
        let studio = entity_block(
            "cycler",
            [0.0, 0.0, 64.0],
            0.0,
            &[
                ("model", "models/ohl_frame_a.mdl"),
                ("rendermode", "1"),
                ("renderamt", "128"),
                ("rendercolor", "200 0 0"),
            ],
        );
        for (height, behind) in [(32.0, true), (96.0, false)] {
            let sprite = entity_block(
                "env_sprite",
                [42.0, 32.0, height],
                0.0,
                &[("model", "sprites/ohl_frame.spr"), ("scale", "8")],
            );
            let background = center(&pixels(&context, &mut game(&assets(&sprite))));
            assert!(background[1] > background[0] + 80);
            let image = pixels(&context, &mut game(&assets(&(sprite + &studio))));
            capture(&format!("studio_sprite_behind_{behind}"), &image);
            let combined = center(&image);
            if behind {
                let alpha = 128.0 / 255.0;
                for channel in 0..3 {
                    let expected = [200.0, 0.0, 0.0][channel] * alpha
                        + f32::from(background[channel]) * (1.0 - alpha);
                    assert!((f32::from(combined[channel]) - expected).abs() <= 2.0);
                }
            } else {
                assert_eq!(combined, background, "front opaque sprite owns depth");
            }
        }
    }

    #[test]
    fn studio_translucency_sorts_across_model_slots_when_opted_in() {
        if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let context = gpu();
        let far = entity_block(
            "cycler",
            [0.0, 0.0, 32.0],
            0.0,
            &[
                ("model", "models/ohl_frame_a.mdl"),
                ("rendermode", "1"),
                ("renderamt", "128"),
                ("rendercolor", "200 0 0"),
            ],
        );
        let near = entity_block(
            "cycler",
            [0.0, 0.0, 64.0],
            0.0,
            &[
                ("model", "models/ohl_frame_b.mdl"),
                ("rendermode", "1"),
                ("renderamt", "128"),
                ("rendercolor", "0 0 200"),
            ],
        );
        let background = center(&pixels(&context, &mut game(&assets(""))));
        let mut results = Vec::new();
        for scene in [far.clone() + &near, near + &far] {
            let image = pixels(&context, &mut game(&assets(&scene)));
            capture(&format!("studio_slots_order_{}", results.len()), &image);
            results.push(center(&image));
        }
        assert_eq!(results[0], results[1]);
        let a = 128.0 / 255.0;
        let expected: [f32; 3] = std::array::from_fn(|i| {
            [0.0, 0.0, 200.0][i] * a
                + ([200.0, 0.0, 0.0][i] * a + f32::from(background[i]) * (1.0 - a)) * (1.0 - a)
        });
        for (actual, expected) in results[0].iter().zip(expected) {
            assert!((f32::from(*actual) - expected).abs() <= 2.0);
        }
    }
}
