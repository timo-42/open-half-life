//! Drawing one frame of a [`crate::Game`].
//!
//! The pass order mirrors what each `ohl-render` entry point expects:
//! opaque world (which clears colour and depth), studio models over the
//! world's depth buffer, the skybox behind everything that has already been
//! written, then the translucent passes — brush-entity submodels and
//! liquids — which read depth without clearing it.

use glam::Vec3;
use ohl_game::registry::{Door, MoverState};
use ohl_game::{TrackTrain, TrackTrainState};
use ohl_render::{
    FreeFlyCamera, GpuContext, LightStyles, ModelInstance, RenderMode, RenderProps, SkyRenderer,
    SpriteInstance, StudioRenderer, SubmodelInstance, WorldRenderer, placement, wgpu,
};
use ohl_world::StudioPose;

use crate::error::{EngineError, Result};
use crate::level::Level;

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
        Ok(Self { world, sky, studio })
    }

    pub(crate) fn draw(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        light_styles: &LightStyles,
        elapsed: f32,
        target: RenderTarget<'_>,
    ) {
        let (width, height) = (target.width.max(1), target.height.max(1));

        // Light styles animate at a fixed 10 Hz; re-blending the atlas each
        // frame keeps the two in step without extra bookkeeping.
        self.world
            .update_light_styles(context, &level.world, light_styles, elapsed);
        self.world
            .render(context, &level.world, camera, target.view, width, height);

        let depth = self.world.depth_view().cloned();
        self.draw_props(context, level, camera, elapsed, depth.as_ref(), target);

        if let (Some(sky), Some(depth)) = (self.sky.as_ref(), depth.as_ref()) {
            sky.render(context, camera, target.view, depth, width, height);
        }

        self.draw_brush_entities(context, level, camera, target);

        self.world
            .render_liquid(context, camera, target.view, width, height, elapsed, 1.0);

        self.draw_sprites(context, level, camera, elapsed, target);
    }

    /// Draws every placed `env_sprite`/`env_glow`/`cycler_sprite` entity.
    fn draw_sprites(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        elapsed: f32,
        target: RenderTarget<'_>,
    ) {
        let instances: Vec<SpriteInstance<'_>> = level
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
        self.world.draw_sprites(
            context,
            &instances,
            camera,
            target.view,
            target.width.max(1),
            target.height.max(1),
        );
    }

    /// Draws every placed studio model at its sampled pose.
    fn draw_props(
        &mut self,
        context: &GpuContext,
        level: &Level,
        camera: &FreeFlyCamera,
        elapsed: f32,
        depth: Option<&wgpu::TextureView>,
        target: RenderTarget<'_>,
    ) {
        for (slot, renderer) in self.studio.iter_mut().enumerate() {
            let Some(model) = level.studio_models.get(slot) else {
                continue;
            };
            let mut poses = Vec::new();
            let mut placements = Vec::new();
            for prop in level.props.iter().filter(|prop| prop.model == slot) {
                let Ok(pose) = StudioPose::sample(model, prop.sequence, elapsed) else {
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
            let (train_offset, yaw_override) = track_train_transform(level, &instance);
            let offset = door_offset(level, &instance) + train_offset;
            let origin = instance.origin + offset;
            let yaw = yaw_override.unwrap_or(instance.angles.y);
            self.world.draw_world_submodel(
                context,
                SubmodelInstance {
                    model,
                    transform: placement(origin.to_array(), yaw),
                },
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
/// offset from the brush entity's own (conventionally `0 0 0`) origin, and,
/// for a `func_tracktrain` (which the public documentation says turns to
/// face the next `path_track`), the yaw to face instead of the entity's own
/// spawned `angles`. Returns `(Vec3::ZERO, None)` for any entity that is not
/// a train with a resolved path (falling back to the door/static placement
/// path above).
fn track_train_transform(level: &Level, instance: &ohl_game::ModelInstance) -> (Vec3, Option<f32>) {
    let Ok(state) = level
        .registry
        .world
        .get::<&TrackTrainState>(instance.entity)
    else {
        return (Vec3::ZERO, None);
    };
    let Ok(train) = level.registry.world.get::<&TrackTrain>(instance.entity) else {
        return (Vec3::ZERO, None);
    };
    (
        state.position() - instance.origin,
        state.yaw_degrees(&train),
    )
}

/// How far a door has slid along its move direction, from the state machine
/// `ohl-game` advances.
///
/// `ohl-game` models a door as a timed state machine rather than a moving
/// transform, so the visual offset is derived here: the timer counts the
/// remaining travel, which maps onto a `0..=1` fraction of the door's own
/// `travel_distance`.
fn door_offset(level: &Level, instance: &ohl_game::ModelInstance) -> Vec3 {
    let Ok(door) = level.registry.world.get::<&Door>(instance.entity) else {
        return Vec3::ZERO;
    };
    let travel_seconds = if door.speed > 0.0 {
        door.travel_distance / door.speed
    } else {
        0.0
    };
    if travel_seconds <= 0.0 {
        // An instantly-travelling door has no intermediate position to
        // show; it is either where it started or fully open.
        let fraction = f32::from(u8::from(door.state == MoverState::Open));
        return door.movedir * door.travel_distance * fraction;
    }
    let progress = (door.timer / travel_seconds).clamp(0.0, 1.0);
    let fraction = match door.state {
        MoverState::Closed => 0.0,
        MoverState::Open => 1.0,
        MoverState::Opening => 1.0 - progress,
        MoverState::Closing => progress,
    };
    door.movedir * door.travel_distance * fraction
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
