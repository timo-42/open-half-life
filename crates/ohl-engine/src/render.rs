//! Drawing one frame of a [`crate::Game`].
//!
//! The pass order mirrors what each `ohl-render` entry point expects:
//! opaque world (which clears colour and depth), studio models over the
//! world's depth buffer, the skybox behind everything that has already been
//! written, then the translucent passes — brush-entity submodels and
//! liquids — which read depth without clearing it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use glam::{Mat4, Quat, Vec3};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_render::{
    FreeFlyCamera, GpuContext, LightStyles, ModelInstance, RenderProps, SkyRenderer,
    SpriteInstance, StudioRenderer, SubmodelInstance, WorldRenderer, math, placement, wgpu,
};
use ohl_world::{Aabb, Frustum, StudioPose};

use crate::components::StudioAnim;
use crate::error::{EngineError, Result};
use crate::level::{Level, PropPlacement};
use crate::sprites::TransientSprite;
use crate::viewmodel::{self, ViewModelFrame};

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
    submodels: BTreeMap<u32, ohl_render::PreparedSubmodel>,
    lightmap_uploads: u64,
    stage_timings: Option<RenderStageTimings>,
    sky: Option<SkyRenderer>,
    studio: Vec<StudioRenderer>,
    /// This frame's studio instances, kept across frames so a per-frame
    /// rebuild reuses the allocation instead of asking the allocator for
    /// one every frame.
    props: Vec<PropPlacement>,
    /// Scratch for sorting `props` into a stable order; pooled for the same
    /// reason.
    prop_order: Vec<(u64, PropPlacement)>,
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
            submodels,
            lightmap_uploads: 0,
            stage_timings: (std::env::var_os("OHL_PROFILE_RENDER_STAGES").as_deref()
                == Some(std::ffi::OsStr::new("1")))
            .then(RenderStageTimings::default),
            sky,
            studio,
            props: Vec::new(),
            prop_order: Vec::new(),
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
        self.collect_studio_instances(level);
        self.draw_props(context, level, camera, depth.as_ref(), target);
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

        self.draw_sprites(context, level, camera, elapsed, target, transient_sprites);
        self.record_stage(RenderStage::Sprite, &mut checkpoint);

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

    /// Draws every placed `env_sprite`/`env_glow`/`cycler_sprite` entity,
    /// plus this frame's transient sprites (muzzle flashes, impacts,
    /// explosions — `crate::sprites`), appended to the same instance list.
    fn draw_sprites(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        elapsed: f32,
        target: RenderTarget<'_>,
        transient_sprites: &[TransientSprite],
    ) {
        let mut instances: Vec<SpriteInstance<'_>> = level
            .sprites
            .iter()
            .filter_map(|sprite| {
                let asset = level.sprite_assets.get(sprite.sprite)?;
                Some(SpriteInstance {
                    asset,
                    origin: sprite.origin,
                    scale: sprite.scale,
                    render_props: render_props(sprite.render),
                    frame_time: elapsed,
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
        self.world.draw_sprites(
            context,
            &instances,
            camera,
            target.view,
            target.width.max(1),
            target.height.max(1),
        );
    }

    /// Rebuilds [`Self::props`], this frame's studio instance list, from the
    /// entities carrying a [`StudioAnim`] rather than from the level's static
    /// placement list.
    ///
    /// The result is sorted by entity, so the order a frame draws in does not
    /// depend on how the world happens to have laid out its archetypes. Both
    /// buffers are reused across frames.
    fn collect_studio_instances(&mut self, level: &Level) {
        self.prop_order.clear();
        for (entity, anim, transform) in &mut level
            .registry
            .world
            .query::<(Entity, &StudioAnim, &Transform)>()
        {
            self.prop_order.push((
                entity.to_bits().get(),
                PropPlacement {
                    model: anim.model,
                    origin: transform.origin.to_array(),
                    yaw: transform.angles.y,
                    sequence: anim.sequence,
                    body: anim.body,
                    skin: anim.skin,
                    cycle: anim.cycle,
                },
            ));
        }
        self.prop_order.sort_unstable_by_key(|(bits, _)| *bits);
        self.props.clear();
        self.props
            .extend(self.prop_order.iter().map(|(_, prop)| *prop));
    }

    /// Draws every studio instance at its sampled pose.
    ///
    /// Each instance carries its own animation cursor, so a monster the AI
    /// moved and a static prop the map placed take the same path.
    fn draw_props(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        depth: Option<&wgpu::TextureView>,
        target: RenderTarget<'_>,
    ) {
        for (slot, renderer) in self.studio.iter_mut().enumerate() {
            let Some(model) = level.studio_models.get(slot) else {
                continue;
            };
            let mut poses = Vec::new();
            let mut placements = Vec::new();
            for prop in self.props.iter().filter(|prop| prop.model == slot) {
                let Ok(pose) = StudioPose::sample(model, prop.sequence, prop.cycle) else {
                    continue;
                };
                poses.push(pose);
                placements.push(*prop);
            }
            if poses.is_empty() {
                continue;
            }
            let bodies: Vec<[u32; 1]> = placements.iter().map(|prop| [prop.body]).collect();
            let instances: Vec<ModelInstance<'_>> = poses
                .iter()
                .zip(&placements)
                .zip(&bodies)
                .map(|((pose, prop), body)| ModelInstance {
                    transform: placement(prop.origin, prop.yaw),
                    pose,
                    body,
                    skin: prop.skin,
                    ambient: ambient_at(level, prop.origin),
                    light_direction: ModelInstance::default_light_direction(),
                    light_color: KEY_LIGHT,
                })
                .collect();
            renderer.render(
                context,
                model,
                camera,
                &instances,
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
            let (rotation_axis, rotation_degrees, rotation_pivot) =
                brush_pose_rotation(&level.registry, instance.entity);
            // The same three inputs `Level::sync_brush_collision` poses the
            // collision hull from, read from the same one helper: a brush
            // entity is drawn exactly where it collides, including a
            // `func_tracktrain` turning through a bend.
            let origin = instance.origin + brush_offset(&level.registry, instance.entity);
            let transform = if rotation_axis == Vec3::ZERO {
                placement(origin.to_array(), instance.angles.y)
            } else {
                // A rotating mover's compiled geometry is stored relative
                // to its own origin keyvalue (see `rotated_placement`'s
                // doc comment) and never also carries a translating
                // `brush_offset` (`Door::movedir` is left `Vec3::ZERO` for
                // a `func_door_rotating`, and `func_rotating` has no
                // offset source at all — see `mover_rotation`'s doc
                // comment), so adding `brush_offset` above is a no-op for
                // one. The entity's own authored `angles` yaw is not
                // reapplied here either: only the live simulation-driven
                // rotation state matters, which for a `func_tracktrain` is
                // the yaw it faces its segment at.
                rotated_placement(origin, rotation_pivot, rotation_axis, rotation_degrees)
            };
            if !brush_in_frustum(&model.bounds, &transform, &frustum) {
                continue;
            }
            let Some(model) = self.submodels.get(&instance.model_index) else {
                continue;
            };
            draws.push((
                SubmodelInstance { model, transform },
                render_props(instance.render),
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
fn render_props(props: ohl_game::keyvalues::RenderProps) -> RenderProps {
    // `renderamt` defaults to 0 when the key is absent, which for either of
    // the documented opaque modes means "fully opaque", not "invisible";
    // `RenderProps::from_entity` applies that rule (and the unknown-mode
    // fallback) for both `Normal` and `Solid`.
    RenderProps::from_entity(props.mode, props.amt, props.color, 0)
}

#[cfg(test)]
// Every comparison below is against an exact analytic value (a plain
// translation composed with an identity or 90-degree rotation), matching
// `save_sections.rs`'s own precedent for exact-comparison tests.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{brush_in_frustum, rotated_placement};
    use glam::{Mat4, Vec3};
    use ohl_render::FreeFlyCamera;
    use ohl_world::{Aabb, Frustum};

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
