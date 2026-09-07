//! Drawing one frame of a [`crate::Game`].
//!
//! The pass order mirrors what each `ohl-render` entry point expects:
//! opaque world (which clears colour and depth), studio models over the
//! world's depth buffer, the skybox behind everything that has already been
//! written, then the translucent passes — brush-entity submodels and
//! liquids — which read depth without clearing it.

use glam::{Mat4, Quat, Vec3};
use ohl_game::hecs::Entity;
use ohl_game::registry::{Door, MoverState, Platform, Rotator, Transform};
use ohl_game::{TrackTrain, TrackTrainState};
use ohl_render::{
    FreeFlyCamera, GpuContext, LightStyles, ModelInstance, RenderProps, SkyRenderer,
    SpriteInstance, StudioRenderer, SubmodelInstance, WorldRenderer, math, placement, wgpu,
};
use ohl_world::StudioPose;

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

/// The GPU-side resources for one loaded level.
pub(crate) struct Renderers {
    world: WorldRenderer,
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
    pub(crate) fn new(
        context: &GpuContext,
        level: &Level,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let world =
            WorldRenderer::new(context, &level.world, format).map_err(|_| EngineError::Renderer)?;
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
            sky,
            studio,
            props: Vec::new(),
            prop_order: Vec::new(),
        })
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

        // Light styles animate at a fixed 10 Hz; re-blending the atlas each
        // frame keeps the two in step without extra bookkeeping.
        self.world
            .update_light_styles(context, &level.world, light_styles, elapsed);
        self.world
            .render(context, &level.world, camera, target.view, width, height);

        let depth = self.world.depth_view().cloned();
        self.collect_studio_instances(level);
        self.draw_props(context, level, camera, depth.as_ref(), target);

        if let (Some(sky), Some(depth)) = (self.sky.as_ref(), depth.as_ref()) {
            sky.render(context, camera, target.view, depth, width, height);
        }

        self.draw_brush_entities(context, level, camera, target);

        self.world
            .render_liquid(context, camera, target.view, width, height, elapsed, 1.0);

        self.draw_sprites(context, level, camera, elapsed, target, transient_sprites);

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
        for instance in ohl_game::brush::model_instances(&level.registry) {
            let Some(model) = level.submodels.get(&instance.model_index) else {
                continue;
            };
            let (rotation_axis, rotation_degrees) =
                mover_rotation(&level.registry, instance.entity);
            let transform = if rotation_axis == Vec3::ZERO {
                let (_, yaw_override) = track_train_transform(&level.registry, instance.entity);
                let offset = brush_offset(&level.registry, instance.entity);
                let origin = instance.origin + offset;
                let yaw = yaw_override.unwrap_or(instance.angles.y);
                placement(origin.to_array(), yaw)
            } else {
                // A rotating mover's compiled geometry is stored relative
                // to its own origin keyvalue (see `rotated_placement`'s
                // doc comment); it never also carries a translating
                // `brush_offset` (`Door::movedir` is left `Vec3::ZERO` for
                // a `func_door_rotating`, and `func_rotating` has no
                // offset source at all — see `mover_rotation`'s doc
                // comment), so the entity's own authored `angles` yaw is
                // not reapplied here either: only the live
                // simulation-driven rotation state matters.
                rotated_placement(instance.origin, rotation_axis, rotation_degrees)
            };
            self.world.draw_world_submodel(
                context,
                SubmodelInstance { model, transform },
                render_props(instance.render),
                camera,
                target.view,
                target.width.max(1),
                target.height.max(1),
            );
        }
    }
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

/// A `func_train`/`func_tracktrain`'s current placement, read from the
/// `ohl-game`-side [`TrackTrainState`] the map logic simulation advances
/// each tick (see `crates/ohl-game/src/track_train.rs`): a world-space
/// *delta* offset from the entity's own `origin` keyvalue, and, for a
/// `func_tracktrain` (which the public documentation says turns to face
/// the next `path_track`), the yaw to face instead of the entity's own
/// spawned `angles`. Returns `(Vec3::ZERO, None)` for any entity that is
/// not a train with a resolved path (falling back to the door/static
/// placement path above).
///
/// Every caller adds this to the entity's `origin` keyvalue (the renderer
/// through `ModelInstance::origin`, the collision model through
/// `Level::sync_brush_collision`), so returning `position() - origin`
/// places the train *at* [`TrackTrainState::position`] — which is what the
/// public documentation describes: a train rides the path with its origin
/// brush on it, the compiler writes that origin brush's position into the
/// entity's `origin` keyvalue and stores the submodel's geometry relative
/// to it, and `height` is documented as the offset "above the path_track
/// that the train will ride, **based on the location of the train's origin
/// brush**" (`docs/FORMAT_SOURCES.md`, "Track trains and paths"). A train
/// is therefore drawn and collided wherever its path currently puts it,
/// not wherever its brushes happened to be built — a map may author the
/// brush anywhere and let the first `path_track` place it at spawn.
///
/// The `docs/CLEAN_ROOM.md`-governed `.plan/fidelity-round-2.md` finding
/// E1 (returning the raw polyline coordinate, which the caller then adds
/// the `origin` keyvalue to and so double-applies it) stays fixed: the
/// `origin` keyvalue is subtracted here precisely so the sum cancels to
/// the absolute position exactly once. Subtracting the chain's first node
/// instead — the previous behaviour — cancelled to a zero offset at spawn
/// and so left the train frozen at wherever it was compiled, however far
/// from its own track that is.
///
/// The cancellation is exact only for a train that has an origin brush,
/// which is the only shape the documentation describes (`height` is
/// defined against that brush) and the shape a compiler leaves the
/// geometry in: vertices stored relative to the brush, its world position
/// in the `origin` keyvalue. A train authored *without* one has a `0 0 0`
/// keyvalue and world-baked vertices, so nothing cancels and it is placed
/// at the absolute polyline coordinate — the same thing an engine that
/// simply assigns the entity's origin from the path does, and a map shape
/// the documentation gives no other meaning to.
pub(crate) fn track_train_transform(
    registry: &ohl_game::Registry,
    entity: Entity,
) -> (Vec3, Option<f32>) {
    let Ok(state) = registry.world.get::<&TrackTrainState>(entity) else {
        return (Vec3::ZERO, None);
    };
    let Ok(train) = registry.world.get::<&TrackTrain>(entity) else {
        return (Vec3::ZERO, None);
    };
    let authored = registry
        .world
        .get::<&Transform>(entity)
        .map_or(Vec3::ZERO, |transform| transform.origin);
    (state.position() - authored, state.yaw_degrees(&train))
}

/// How far a door has slid along its move direction, from the state machine
/// `ohl-game` advances.
///
/// `ohl-game` models a door as a timed state machine rather than a moving
/// transform, so the visual offset is derived here: the timer counts the
/// remaining travel, which maps onto a `0..=1` fraction of the door's own
/// `travel_distance`.
/// How far a translating brush mover has slid along its move direction,
/// from `speed`/`travel_distance`/`movedir` and its own current
/// `state`/`timer` (the shape [`Door`] and [`Platform`] both carry
/// identically; see `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
/// logic"). Shared by [`door_offset`] and [`platform_offset`] so the two
/// staying in lock-step is a compile-time fact, not a doc comment claiming
/// they mirror each other.
fn mover_offset(
    speed: f32,
    travel_distance: f32,
    movedir: Vec3,
    state: MoverState,
    timer: f32,
) -> Vec3 {
    movedir * travel_distance * mover_fraction(speed, travel_distance, state, timer)
}

/// The `0.0..=1.0` progress fraction [`mover_offset`] (a translating
/// door/platform) and [`door_rotation_degrees`] (a rotating one) both scale
/// their travel distance by, factored out so the two stay in lock-step by
/// construction rather than by two doc comments claiming they mirror each
/// other.
fn mover_fraction(speed: f32, travel_distance: f32, state: MoverState, timer: f32) -> f32 {
    let travel_seconds = if speed > 0.0 {
        travel_distance / speed
    } else {
        0.0
    };
    if travel_seconds <= 0.0 {
        // An instantly-travelling mover has no intermediate position to
        // show; it is either where it started or fully open.
        return f32::from(u8::from(state == MoverState::Open));
    }
    let progress = (timer / travel_seconds).clamp(0.0, 1.0);
    match state {
        MoverState::Closed => 0.0,
        MoverState::Open => 1.0,
        MoverState::Opening => 1.0 - progress,
        MoverState::Closing => progress,
    }
}

/// How far a `func_door_rotating` has swung, in degrees, from the same
/// shared [`Door`] `state`/`timer` [`mover_offset`] reads for a translating
/// door — see [`mover_fraction`]. `Vec3::ZERO`/`0.0` for a `Door` with no
/// [`Door::rotation_axis`] (an ordinary translating `func_door`).
pub(crate) fn door_rotation_degrees(registry: &ohl_game::Registry, entity: Entity) -> (Vec3, f32) {
    let Ok(door) = registry.world.get::<&Door>(entity) else {
        return (Vec3::ZERO, 0.0);
    };
    let Some(axis) = door.rotation_axis else {
        return (Vec3::ZERO, 0.0);
    };
    let fraction = mover_fraction(door.speed, door.travel_distance, door.state, door.timer);
    (axis, door.travel_distance * fraction)
}

/// How far a `func_rotating` has spun, in degrees, from its own
/// continuously-accumulated [`Rotator::angle_deg`] (no open/close timer to
/// derive a fraction from; see `ohl_game::logic::Simulation::
/// advance_rotators`). `Vec3::ZERO`/`0.0` for any entity without a
/// [`Rotator`].
pub(crate) fn rotator_degrees(registry: &ohl_game::Registry, entity: Entity) -> (Vec3, f32) {
    registry
        .world
        .get::<&Rotator>(entity)
        .map_or((Vec3::ZERO, 0.0), |rotator| {
            (rotator.axis, rotator.angle_deg)
        })
}

/// The signed rotation axis and current angle (degrees) a rotating brush
/// mover — `func_door_rotating` or `func_rotating`, mutually exclusive
/// components on any one entity — is currently posed at. `Vec3::ZERO`/`0.0`
/// (no rotation) for every other brush entity, so a caller can branch on
/// `axis != Vec3::ZERO` to tell a rotating mover from a translating one.
pub(crate) fn mover_rotation(registry: &ohl_game::Registry, entity: Entity) -> (Vec3, f32) {
    let (axis, degrees) = door_rotation_degrees(registry, entity);
    if axis != Vec3::ZERO {
        return (axis, degrees);
    }
    rotator_degrees(registry, entity)
}

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
/// submodel's geometry relative to it"). So the pivot to rotate about is
/// simply the submodel's own local origin (`Vec3::ZERO`, no separate pivot
/// parameter needed), and `origin` is added *after* rotating, exactly the
/// same "rotate, then translate" order [`placement`] already uses for a
/// yaw-only rotation. A zero angle still needs the `origin` translation —
/// `Quat::from_axis_angle` with a zero angle is already the identity
/// quaternion, so the composed matrix is a pure translation by `origin`,
/// exactly matching a closed door's or idle rotator's resting pose — so
/// only a zero `axis` (whose `normalize()` would be NaN) short-circuits to
/// the identity matrix; every caller still branches on [`mover_rotation`]
/// first rather than relying on that, since a rotating mover never also
/// carries a translating [`brush_offset`] to add in.
pub(crate) fn rotated_placement(origin: Vec3, axis: Vec3, angle_degrees: f32) -> math::Mat4 {
    if axis == Vec3::ZERO {
        return math::identity();
    }
    let rotation = Quat::from_axis_angle(axis.normalize(), angle_degrees.to_radians());
    let matrix = Mat4::from_translation(origin) * Mat4::from_quat(rotation);
    matrix.to_cols_array()
}

pub(crate) fn door_offset(registry: &ohl_game::Registry, entity: Entity) -> Vec3 {
    let Ok(door) = registry.world.get::<&Door>(entity) else {
        return Vec3::ZERO;
    };
    mover_offset(
        door.speed,
        door.travel_distance,
        door.movedir,
        door.state,
        door.timer,
    )
}

/// How far a `func_plat`/`func_platform` has slid along its move direction,
/// from the state machine `ohl-game` advances.
///
/// Without this a `func_plat`'s `Platform` component advances its own
/// state machine but the brush never visibly (or collidably) moves at all.
pub(crate) fn platform_offset(registry: &ohl_game::Registry, entity: Entity) -> Vec3 {
    let Ok(platform) = registry.world.get::<&Platform>(entity) else {
        return Vec3::ZERO;
    };
    mover_offset(
        platform.speed,
        platform.travel_distance,
        platform.movedir,
        platform.state,
        platform.timer,
    )
}

/// How far a brush entity has moved from where its geometry was compiled.
///
/// The same value the renderer offsets the submodel by, so the brush the
/// player collides with is exactly the brush that is drawn: a door or
/// platform caught mid-slide blocks where it looks like it is, not where it
/// was authored.
pub(crate) fn brush_offset(registry: &ohl_game::Registry, entity: Entity) -> Vec3 {
    door_offset(registry, entity)
        + platform_offset(registry, entity)
        + track_train_transform(registry, entity).0
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
    use super::rotated_placement;
    use glam::Vec3;

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
        let matrix = rotated_placement(origin, Vec3::Z, 0.0);
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
        let matrix = rotated_placement(origin, Vec3::Z, 90.0);
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
        let matrix = rotated_placement(Vec3::new(5.0, 6.0, 7.0), Vec3::ZERO, 45.0);
        assert_eq!(matrix, glam::Mat4::IDENTITY.to_cols_array());
    }
}
