//! Simulation-side map effects and the optional tag-43 payload.
//!
//! This module consumes typed game requests and produces blast requests for
//! the shared combat dispatcher. It never duplicates `radius_damage` or calls
//! the projectile system's private blast handler. The frame owner must detach
//! every `EffectBatch::broken_sources` brush from both collision models before
//! resolving that batch's blasts, and route resulting damage through phase 9.
//!
//! Scheduling, waveform, overlap, finite presentation lifetime and fallback
//! visuals are project-authored policies. TODO(black-box): original timings.
//! Fade follows the accepted TWHL contract and clears on restore; shake also
//! clears, as a project transient policy. Existing save encodings are untouched.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use glam::Vec3;
use ohl_combat::DamageType;
use ohl_game::Simulation;
use ohl_game::effects::{
    EffectActive, ExplosionState, ExplosionVisuals, FadeDef, MAX_PENDING_EFFECTS, MapBlastProfile,
    MapEffect, MapEffectCommand, RenderFx, ShakeDef,
};
use ohl_game::hecs::Entity;
use ohl_game::registry::{Registry, TriggerUse};
use ohl_render::FreeFlyCamera;
use serde::{Deserialize, Serialize};

use crate::debris::{DebrisRecord, DebrisSystem, DebrisWorld, MAX_DEBRIS};

const MAX_BLAST_VISUALS: usize = 128;
const BLAST_VISUAL_SECONDS: f32 = 0.75;
const MAX_REGISTRY_OVERRIDES: usize = 8192;
const MAX_STEP_SECONDS: f32 = 0.25;

/// The shared engine blast seam. `attacker` and `inflictor` deliberately
/// differ; the dispatcher retains P1's owner/self rule and replaces only the
/// queued `DamageInfo.inflictor` with the actual map source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlastRequest {
    pub origin: Vec3,
    pub profile: MapBlastProfile,
    pub kind: DamageType,
    pub attacker: Entity,
    pub inflictor: Entity,
}

#[derive(Debug, Default)]
pub(crate) struct EffectBatch {
    /// Detach these unique sources before any blast occlusion queries.
    pub broken_sources: Vec<Entity>,
    pub blasts: Vec<BlastRequest>,
}

/// Simulation-aged presentation. Visual suppression never changes combat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlastVisual {
    pub origin: Vec3,
    pub radius: f32,
    pub age: f32,
    pub channels: ExplosionVisuals,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ViewShake {
    pub offset: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for ViewShake {
    fn default() -> Self {
        Self {
            offset: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

/// Normal color blending and modulation have distinct, testable compositing.
/// The renderer applies this after scene/viewmodel and before readable UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FadeOverlay {
    pub color: [f32; 3],
    pub amount: f32,
    pub modulate: bool,
}

impl FadeOverlay {
    pub(crate) fn composite(self, scene: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|channel| {
            let target = if self.modulate {
                scene[channel] * self.color[channel]
            } else {
                self.color[channel]
            };
            scene[channel] * (1.0 - self.amount) + target * self.amount
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ActiveShake {
    definition: ShakeDef,
    age: f32,
    phase: f32,
}

#[derive(Debug, Clone, Copy)]
struct ActiveFade {
    definition: FadeDef,
    age: f32,
}

/// Immutable view of actual simulated records, never a gib counter.
pub(crate) struct EffectPresentation<'a> {
    pub shake: ViewShake,
    pub fade: Option<FadeOverlay>,
    pub debris: &'a [DebrisRecord],
    pub blasts: &'a [BlastVisual],
}

#[derive(Debug)]
pub(crate) struct MapEffectsRuntime {
    pending: VecDeque<MapEffectCommand>,
    debris: DebrisSystem,
    shake: Option<ActiveShake>,
    fade: Option<ActiveFade>,
    blast_visuals: Vec<BlastVisual>,
    /// Per-fixed-step cascade work bound, refreshed only by `begin_step`.
    commands_left: usize,
    broken_this_step: BTreeSet<Entity>,
}

impl Default for MapEffectsRuntime {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            debris: DebrisSystem::default(),
            shake: None,
            fade: None,
            blast_visuals: Vec::new(),
            commands_left: MAX_PENDING_EFFECTS,
            broken_this_step: BTreeSet::new(),
        }
    }
}

impl MapEffectsRuntime {
    /// Called once per fixed step, before any same-step damage cascade.
    pub(crate) fn begin_step(&mut self) {
        self.commands_left = MAX_PENDING_EFFECTS;
        self.broken_this_step.clear();
    }

    /// Late use effects keep their captured trigger-time eligibility. Shake and
    /// fade start immediately; blast/break operations await the next combat
    /// opportunity, or the current phase-9 cascade when called from that phase.
    pub(crate) fn capture_commands(
        &mut self,
        commands: impl IntoIterator<Item = MapEffectCommand>,
    ) {
        for command in commands.into_iter().take(MAX_PENDING_EFFECTS) {
            if !command.is_valid() {
                continue;
            }
            match command.effect {
                MapEffect::Shake(definition) => {
                    if definition.duration > 0.0 {
                        let phase =
                            f32::from(u16::try_from(command.source.id() % 1024).unwrap_or(0))
                                * std::f32::consts::TAU
                                / 1024.0
                                + 1.173;
                        self.shake = Some(ActiveShake {
                            definition,
                            age: 0.0,
                            phase,
                        });
                    }
                }
                MapEffect::Fade(definition) => {
                    if definition.duration > 0.0 {
                        self.fade = Some(ActiveFade {
                            definition,
                            age: 0.0,
                        });
                    }
                }
                MapEffect::Explosion(_) | MapEffect::Break(_) => {
                    if self.pending.len() < MAX_PENDING_EFFECTS {
                        self.pending.push_back(command);
                    }
                }
            }
        }
    }

    /// Returns combat work without performing traces or applying damage. The
    /// frame owner supplies actor validity using its current actor/player state.
    pub(crate) fn resolve_pending(
        &mut self,
        mut valid_actor: impl FnMut(Entity) -> bool,
    ) -> EffectBatch {
        let mut batch = EffectBatch::default();
        while self.commands_left > 0 {
            let Some(command) = self.pending.pop_front() else {
                break;
            };
            self.commands_left -= 1;
            let attacker = command
                .activator
                .filter(|entity| valid_actor(*entity))
                .unwrap_or(command.source);
            match command.effect {
                MapEffect::Explosion(definition) => {
                    self.add_blast_visual(command.origin, definition.blast, definition.visuals);
                    if !definition.no_damage
                        && definition.blast.damage > 0.0
                        && definition.blast.radius > 0.0
                    {
                        batch.blasts.push(BlastRequest {
                            origin: command.origin,
                            profile: definition.blast,
                            kind: DamageType::BLAST,
                            attacker,
                            inflictor: command.source,
                        });
                    }
                }
                MapEffect::Break(breakage) => {
                    if !self.broken_this_step.insert(command.source) {
                        continue;
                    }
                    batch.broken_sources.push(command.source);
                    self.debris
                        .spawn_break(command.source, command.origin, &breakage);
                    if breakage.effects.blast.damage > 0.0 && breakage.effects.blast.radius > 0.0 {
                        self.add_blast_visual(
                            command.origin,
                            breakage.effects.blast,
                            ExplosionVisuals::default(),
                        );
                        batch.blasts.push(BlastRequest {
                            origin: command.origin,
                            profile: breakage.effects.blast,
                            kind: DamageType::BLAST,
                            attacker,
                            inflictor: command.source,
                        });
                    }
                }
                MapEffect::Shake(_) | MapEffect::Fade(_) => {}
            }
        }
        batch
    }

    fn add_blast_visual(
        &mut self,
        origin: Vec3,
        profile: MapBlastProfile,
        channels: ExplosionVisuals,
    ) {
        if self.blast_visuals.len() < MAX_BLAST_VISUALS {
            self.blast_visuals.push(BlastVisual {
                origin,
                radius: profile.radius,
                age: 0.0,
                channels,
            });
        }
    }

    /// Advance once per simulation tick. No sampling/render entry advances time.
    pub(crate) fn advance(&mut self, dt: f32, gravity: f32, world: &impl DebrisWorld) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let dt = dt.min(MAX_STEP_SECONDS);
        self.debris.advance(dt, gravity, world);
        if let Some(shake) = &mut self.shake {
            shake.age += dt;
            if shake.age >= shake.definition.duration {
                self.shake = None;
            }
        }
        if let Some(fade) = &mut self.fade {
            fade.age += dt;
            if fade.age >= fade.definition.duration + fade.definition.hold {
                self.fade = None;
            }
        }
        for visual in &mut self.blast_visuals {
            visual.age += dt;
        }
        self.blast_visuals
            .retain(|visual| visual.age < BLAST_VISUAL_SECONDS);
    }

    pub(crate) fn presentation(&self) -> EffectPresentation<'_> {
        EffectPresentation {
            shake: self.shake_sample(),
            fade: self.fade_sample(),
            debris: self.debris.records(),
            blasts: &self.blast_visuals,
        }
    }

    /// Always starts from the unshaken camera copy, so repeated draws cannot
    /// alter controller position, aim, or accumulate shake into the next frame.
    pub(crate) fn view_camera(&self, base: &FreeFlyCamera) -> FreeFlyCamera {
        let shake = self.presentation().shake;
        let mut view = *base;
        view.position = (Vec3::from_array(base.position) + shake.offset).to_array();
        view.yaw += shake.yaw;
        view.pitch += shake.pitch;
        view
    }

    fn shake_sample(&self) -> ViewShake {
        let Some(shake) = self.shake else {
            return ViewShake::default();
        };
        let envelope = (1.0 - shake.age / shake.definition.duration).clamp(0.0, 1.0);
        let amplitude = shake.definition.amplitude * envelope;
        let phase = shake.phase + shake.age * shake.definition.frequency * std::f32::consts::TAU;
        ViewShake {
            offset: Vec3::new(phase.sin(), (phase * 1.31).sin(), (phase * 0.83).cos()) * amplitude,
            yaw: (phase * 0.71).sin() * amplitude * 0.1,
            pitch: (phase * 1.13).cos() * amplitude * 0.15,
        }
    }

    fn fade_sample(&self) -> Option<FadeOverlay> {
        let fade = self.fade?;
        let definition = fade.definition;
        let ramp = if definition.reverse {
            1.0 - ((fade.age - definition.hold) / definition.duration).clamp(0.0, 1.0)
        } else {
            (fade.age / definition.duration).clamp(0.0, 1.0)
        };
        Some(FadeOverlay {
            color: definition
                .color
                .map(|component| f32::from(component) / 255.0),
            amount: ramp * f32::from(definition.amount) / 255.0,
            modulate: definition.modulate,
        })
    }

    /// Captures both runtime-held and still-undrained game commands at the public
    /// save boundary. No source needs to remain alive after its request capture.
    pub(crate) fn snapshot(
        &self,
        registry: &Registry,
        player: Entity,
        simulation: &Simulation,
    ) -> MapEffectsSnapshot {
        let references: BTreeMap<_, _> = registry
            .entities
            .iter()
            .enumerate()
            .filter_map(|(index, &entity)| {
                u32::try_from(index)
                    .ok()
                    .map(|index| (entity, EffectEntityRef::Registry(index)))
            })
            .chain(std::iter::once((player, EffectEntityRef::Player)))
            .collect();
        let encode = |entity: Entity| references.get(&entity).copied();
        let pending = self
            .pending
            .iter()
            .chain(simulation.effect_commands())
            .take(MAX_PENDING_EFFECTS)
            .filter(|command| {
                matches!(
                    command.effect,
                    MapEffect::Explosion(_) | MapEffect::Break(_)
                )
            })
            .filter_map(|command| {
                Some(MapEffectCommand {
                    source: encode(command.source)?,
                    activator: command.activator.and_then(encode),
                    origin: command.origin,
                    effect: command.effect.clone(),
                })
            })
            .collect();
        let debris = self
            .debris
            .records()
            .iter()
            .filter_map(|record| Some(record.clone().map_source(encode(record.source)?)))
            .collect();
        let mut snapshot = MapEffectsSnapshot {
            pending,
            debris,
            next_debris_id: self.debris.next_id(),
            debris_rng: self.debris.rng(),
            button_activators: simulation
                .button_activators()
                .take(MAX_PENDING_EFFECTS)
                .filter_map(|(button, actor)| Some((encode(button)?, encode(actor)?)))
                .collect(),
            pending_uses: simulation
                .pending_use_contexts()
                .take(MAX_PENDING_EFFECTS)
                .map(|(activator, use_type)| PendingUseSnapshot {
                    activator: activator.and_then(encode),
                    use_type: use_type.into(),
                })
                .collect(),
            ..MapEffectsSnapshot::default()
        };
        for (index, &entity) in registry
            .entities
            .iter()
            .take(MAX_REGISTRY_OVERRIDES)
            .enumerate()
        {
            let Ok(index) = u32::try_from(index) else {
                continue;
            };
            if registry
                .world
                .get::<&ExplosionState>(entity)
                .is_ok_and(|state| state.consumed)
            {
                snapshot.consumed_explosions.push(index);
            }
            if let Ok(fx) = registry.world.get::<&RenderFx>(entity) {
                snapshot.render_fx.push((index, fx.0));
            }
            if let Ok(active) = registry.world.get::<&EffectActive>(entity) {
                snapshot.active_overrides.push((index, active.active));
            }
        }
        snapshot
    }

    /// Called after registry/player recreation and all frozen section overlays.
    /// Absence initializes authored state and never scans old broken brushes for
    /// effects. Presentation is always cleared, including undrained game fades.
    pub(crate) fn restore(
        &mut self,
        snapshot: Option<&MapEffectsSnapshot>,
        registry: &mut Registry,
        player: Entity,
        simulation: &mut Simulation,
    ) {
        *self = Self::default();
        simulation.restore_effect_commands(std::iter::empty());
        simulation.restore_button_activators(std::iter::empty());
        let Some(snapshot) = snapshot else {
            return;
        };
        simulation.restore_pending_use_contexts(std::iter::empty());
        let decode = |reference: EffectEntityRef| match reference {
            EffectEntityRef::Player => Some(player),
            EffectEntityRef::Registry(index) => usize::try_from(index)
                .ok()
                .and_then(|index| registry.entities.get(index))
                .copied(),
        };
        self.pending = snapshot
            .pending
            .iter()
            .take(MAX_PENDING_EFFECTS)
            .filter(|command| {
                command.is_valid()
                    && matches!(
                        command.effect,
                        MapEffect::Explosion(_) | MapEffect::Break(_)
                    )
            })
            .filter_map(|command| {
                Some(MapEffectCommand {
                    source: decode(command.source)?,
                    activator: command.activator.and_then(decode),
                    origin: command.origin,
                    effect: command.effect.clone(),
                })
            })
            .collect();
        self.debris.restore(
            snapshot
                .debris
                .iter()
                .take(MAX_DEBRIS)
                .filter_map(|record| Some(record.clone().map_source(decode(record.source)?))),
            snapshot.next_debris_id,
            snapshot.debris_rng,
        );
        simulation.restore_pending_use_contexts(
            snapshot
                .pending_uses
                .iter()
                .take(MAX_PENDING_EFFECTS)
                .map(|context| (context.activator.and_then(decode), context.use_type.into())),
        );
        simulation.restore_button_activators(
            snapshot
                .button_activators
                .iter()
                .take(MAX_PENDING_EFFECTS)
                .filter_map(|&(button, actor)| Some((decode(button)?, decode(actor)?))),
        );
        for &index in snapshot
            .consumed_explosions
            .iter()
            .take(MAX_REGISTRY_OVERRIDES)
        {
            if let Some(entity) = decode(EffectEntityRef::Registry(index))
                && let Ok(mut state) = registry.world.get::<&mut ExplosionState>(entity)
            {
                state.consumed = true;
            }
        }
        for &(index, value) in snapshot.render_fx.iter().take(MAX_REGISTRY_OVERRIDES) {
            if let Some(entity) = decode(EffectEntityRef::Registry(index))
                && let Ok(mut fx) = registry.world.get::<&mut RenderFx>(entity)
            {
                fx.0 = value;
            }
        }
        for &(index, value) in snapshot
            .active_overrides
            .iter()
            .take(MAX_REGISTRY_OVERRIDES)
        {
            if let Some(entity) = decode(EffectEntityRef::Registry(index))
                && let Ok(mut active) = registry.world.get::<&mut EffectActive>(entity)
            {
                active.active = value;
            }
        }
    }
}

/// Independent tag-43 vocabulary; tag 42's existing reference enum is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectEntityRef {
    Player,
    Registry(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedUseType {
    Off,
    On,
    Toggle,
}

impl From<TriggerUse> for SavedUseType {
    fn from(value: TriggerUse) -> Self {
        match value {
            TriggerUse::Off => Self::Off,
            TriggerUse::On => Self::On,
            TriggerUse::Toggle => Self::Toggle,
        }
    }
}

impl From<SavedUseType> for TriggerUse {
    fn from(value: SavedUseType) -> Self {
        match value {
            SavedUseType::Off => Self::Off,
            SavedUseType::On => Self::On,
            SavedUseType::Toggle => Self::Toggle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUseSnapshot {
    pub activator: Option<EffectEntityRef>,
    pub use_type: SavedUseType,
}

/// Optional section 43, wholly separate from every existing encoding.
/// Its vectors reject excess entries during decoding, before allocating by an
/// untrusted size hint. Runtime restore also rejects malformed numeric records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapEffectsSnapshot {
    #[serde(deserialize_with = "bounded_pending")]
    pub pending: Vec<MapEffectCommand<EffectEntityRef>>,
    #[serde(deserialize_with = "bounded_debris")]
    pub debris: Vec<DebrisRecord<EffectEntityRef>>,
    pub next_debris_id: u64,
    pub debris_rng: u32,
    #[serde(deserialize_with = "bounded_overrides")]
    pub consumed_explosions: Vec<u32>,
    #[serde(deserialize_with = "bounded_overrides")]
    pub render_fx: Vec<(u32, i32)>,
    #[serde(deserialize_with = "bounded_overrides")]
    pub active_overrides: Vec<(u32, bool)>,
    #[serde(deserialize_with = "bounded_pending")]
    pub pending_uses: Vec<PendingUseSnapshot>,
    #[serde(deserialize_with = "bounded_pending")]
    pub button_activators: Vec<(EffectEntityRef, EffectEntityRef)>,
}

impl MapEffectsSnapshot {
    pub(crate) fn within_limits(&self) -> bool {
        self.pending.len() <= MAX_PENDING_EFFECTS
            && self.debris.len() <= MAX_DEBRIS
            && self.consumed_explosions.len() <= MAX_REGISTRY_OVERRIDES
            && self.render_fx.len() <= MAX_REGISTRY_OVERRIDES
            && self.active_overrides.len() <= MAX_REGISTRY_OVERRIDES
            && self.pending_uses.len() <= MAX_PENDING_EFFECTS
            && self.button_activators.len() <= MAX_PENDING_EFFECTS
            && self.pending.iter().all(MapEffectCommand::is_valid)
            && self.debris.iter().all(DebrisRecord::is_valid)
    }
}

fn bounded_pending<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    bounded_vec(d, MAX_PENDING_EFFECTS)
}
fn bounded_debris<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<DebrisRecord<EffectEntityRef>>, D::Error> {
    bounded_vec(d, MAX_DEBRIS)
}
fn bounded_overrides<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    bounded_vec(d, MAX_REGISTRY_OVERRIDES)
}

fn bounded_vec<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
    max: usize,
) -> Result<Vec<T>, D::Error> {
    struct Bounded<T>(usize, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded map-effect state")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|count| count > self.0) {
                return Err(serde::de::Error::custom("map-effect capacity exceeded"));
            }
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == self.0 {
                    return Err(serde::de::Error::custom("map-effect capacity exceeded"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Bounded(max, std::marker::PhantomData))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::debris::{DebrisTrace, DebrisWorld};
    use ohl_formats::bsp30::Entity as RawEntity;
    use ohl_game::effects::EffectPlayer;
    use ohl_game::keyvalues::{Limits, parse_entities};

    struct EmptyWorld;
    impl DebrisWorld for EmptyWorld {
        fn sweep(&self, _start: Vec3, end: Vec3, _half_extents: Vec3) -> DebrisTrace {
            DebrisTrace::miss(end)
        }
    }

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn fixture(raw: &[RawEntity], padding: usize) -> (Registry, Simulation, Entity) {
        let bounds = BTreeMap::from([(1, ([-4.0; 3], [4.0; 3]))]);
        let limits = Limits::default();
        let mut registry = Registry::build(&parse_entities(raw, &limits), &bounds, &limits);
        for _ in 0..padding {
            registry.world.spawn(());
        }
        let player = registry.world.spawn(());
        let mut simulation = Simulation::new();
        simulation.set_effect_player(Some(EffectPlayer {
            entity: player,
            origin: Vec3::ZERO,
            grounded: true,
        }));
        (registry, simulation, player)
    }

    fn use_effect(
        runtime: &mut MapEffectsRuntime,
        registry: &mut Registry,
        simulation: &mut Simulation,
        name: &str,
        player: Entity,
    ) {
        let entity = registry.find(name)[0];
        simulation.use_entity(registry, entity, Some(player), &mut Vec::new());
        runtime.capture_commands(simulation.drain_effect_commands());
    }

    fn advance(runtime: &mut MapEffectsRuntime, steps: usize, world: &impl DebrisWorld) {
        for _ in 0..steps {
            runtime.advance(0.025, 800.0, world);
        }
    }

    #[test]
    fn explosion_requests_separate_combat_channels_and_actor_from_inflictor() {
        for flags in [0, 1, 4, 8, 16, 32, 61] {
            let (mut registry, mut simulation, player) = fixture(
                &[raw(&[
                    ("classname", "env_explosion"),
                    ("targetname", "blast"),
                    ("origin", "30 40 50"),
                    ("iMagnitude", "30"),
                    ("spawnflags", &flags.to_string()),
                ])],
                0,
            );
            let source = registry.find("blast")[0];
            let mut runtime = MapEffectsRuntime::default();
            use_effect(
                &mut runtime,
                &mut registry,
                &mut simulation,
                "blast",
                player,
            );
            registry.world.despawn(source).unwrap();
            let batch = runtime.resolve_pending(|entity| entity == player);
            assert_eq!(batch.blasts.len(), usize::from(flags & 1 == 0));
            if let Some(blast) = batch.blasts.first() {
                assert_eq!(blast.attacker, player);
                assert_eq!(blast.inflictor, source);
                assert_eq!(blast.origin, Vec3::new(30.0, 40.0, 50.0));
                assert_eq!(
                    blast.profile,
                    MapBlastProfile {
                        damage: 30.0,
                        radius: 60.0
                    }
                );
            }
            let visual = runtime.presentation().blasts[0];
            assert_eq!(visual.channels.fireball, flags & 4 == 0);
            assert_eq!(visual.channels.smoke, flags & 8 == 0);
            assert_eq!(visual.channels.decal, flags & 16 == 0);
            assert_eq!(visual.channels.sparks, flags & 32 == 0);
            assert!(runtime.resolve_pending(|_| true).blasts.is_empty());
        }
    }

    #[test]
    fn debris_moves_bounces_rests_and_expires_without_any_renderer() {
        let (mut registry, mut simulation, player) = fixture(
            &[raw(&[
                ("classname", "func_breakable"),
                ("targetname", "crate"),
                ("model", "*1"),
                ("origin", "0 0 40"),
                ("explodemagnitude", "20"),
                ("gibmodel", "models/synthetic.mdl"),
            ])],
            0,
        );
        let world = ohl_physics::test_support::collision_model_from(
            &ohl_physics::test_support::build_flat_floor_bsp(),
        );
        let mut runtime = MapEffectsRuntime::default();
        use_effect(
            &mut runtime,
            &mut registry,
            &mut simulation,
            "crate",
            player,
        );
        let batch = runtime.resolve_pending(|entity| entity == player);
        assert_eq!(batch.broken_sources, vec![registry.find("crate")[0]]);
        assert_eq!(batch.blasts.len(), 1);
        let initial = runtime.presentation().debris.to_vec();
        assert!(initial.len() > 1);
        let mut bounced = false;
        for _ in 0..100 {
            let before = runtime.presentation().debris.to_vec();
            advance(&mut runtime, 1, &world);
            for (old, record) in before.iter().zip(runtime.presentation().debris) {
                bounced |= old.velocity.z < 0.0 && record.velocity.z > 0.0;
                assert!(record.position.z >= record.half_extents.length() - 0.05);
            }
        }
        assert!(bounced, "actual velocity must reverse on a floor collision");
        assert!(
            runtime
                .presentation()
                .debris
                .iter()
                .any(|record| record.resting)
        );
        assert_ne!(
            initial[0].position,
            runtime.presentation().debris[0].position
        );
        assert_eq!(
            runtime.presentation().debris[0].gib_model.as_deref(),
            Some("models/synthetic.mdl")
        );
        let record = &runtime.presentation().debris[0];
        assert_eq!(
            &record.transform()[12..15],
            record.position.to_array().as_slice()
        );
        advance(&mut runtime, 240, &world);
        assert!(runtime.presentation().debris.is_empty());
        assert!(runtime.presentation().blasts.is_empty());
    }

    #[test]
    fn debris_and_rng_resume_identically_and_new_emissions_keep_unique_ids() {
        let raw = [
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "first"),
                ("model", "*1"),
                ("origin", "0 0 60"),
            ]),
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "second"),
                ("model", "*1"),
                ("origin", "20 0 60"),
            ]),
        ];
        let world = ohl_physics::test_support::collision_model_from(
            &ohl_physics::test_support::build_flat_floor_bsp(),
        );
        let (mut registry, mut simulation, player) = fixture(&raw, 0);
        let mut baseline = MapEffectsRuntime::default();
        use_effect(
            &mut baseline,
            &mut registry,
            &mut simulation,
            "first",
            player,
        );
        baseline.resolve_pending(|_| true);
        advance(&mut baseline, 12, &world);
        let saved = baseline.snapshot(&registry, player, &simulation);
        let bytes = postcard::to_allocvec(&saved).unwrap();
        let decoded: MapEffectsSnapshot = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(saved, decoded);
        let (mut loaded_registry, mut loaded_simulation, loaded_player) = fixture(&raw, 3);
        let mut loaded = MapEffectsRuntime::default();
        loaded.restore(
            Some(&decoded),
            &mut loaded_registry,
            loaded_player,
            &mut loaded_simulation,
        );
        for (runtime, registry, simulation, player) in [
            (&mut baseline, &mut registry, &mut simulation, player),
            (
                &mut loaded,
                &mut loaded_registry,
                &mut loaded_simulation,
                loaded_player,
            ),
        ] {
            runtime.begin_step();
            use_effect(runtime, registry, simulation, "second", player);
            runtime.resolve_pending(|_| true);
            advance(runtime, 80, &world);
        }
        assert_eq!(
            baseline.snapshot(&registry, player, &simulation).debris,
            loaded
                .snapshot(&loaded_registry, loaded_player, &loaded_simulation)
                .debris
        );
        let records = loaded.presentation().debris;
        assert_eq!(
            records
                .iter()
                .map(|record| record.id)
                .collect::<BTreeSet<_>>()
                .len(),
            records.len()
        );
    }

    #[test]
    fn shake_only_changes_a_fresh_view_copy_and_decays_in_simulation_time() {
        let (mut registry, mut simulation, player) = fixture(
            &[raw(&[
                ("classname", "env_shake"),
                ("targetname", "shake"),
                ("amplitude", "8"),
                ("frequency", "4"),
                ("duration", "1"),
                ("spawnflags", "1"),
            ])],
            0,
        );
        let mut runtime = MapEffectsRuntime::default();
        use_effect(
            &mut runtime,
            &mut registry,
            &mut simulation,
            "shake",
            player,
        );
        let base = FreeFlyCamera {
            position: [4.0, 5.0, 6.0],
            yaw: 17.0,
            pitch: -9.0,
            ..FreeFlyCamera::default()
        };
        let first = runtime.view_camera(&base);
        assert_ne!(first.position, base.position);
        for _ in 0..100 {
            let again = runtime.view_camera(&base);
            assert_eq!(again.position, first.position);
            assert_eq!(again.yaw, first.yaw);
        }
        assert_eq!(base.position, [4.0, 5.0, 6.0]);
        assert_eq!((base.yaw, base.pitch), (17.0, -9.0));
        advance(&mut runtime, 36, &EmptyWorld);
        assert!(runtime.presentation().shake.offset.abs().max_element() < 1.0);
        advance(&mut runtime, 5, &EmptyWorld);
        assert_eq!(runtime.presentation().shake, ViewShake::default());
        assert_eq!(runtime.view_camera(&base).position, base.position);
    }

    #[test]
    fn forward_reverse_and_modulate_fades_have_distinct_timing_and_compositing() {
        for (flags, reverse, modulate) in [(0, false, false), (1, true, false), (2, false, true)] {
            let (mut registry, mut simulation, player) = fixture(
                &[raw(&[
                    ("classname", "env_fade"),
                    ("targetname", "fade"),
                    ("duration", "1"),
                    ("holdtime", "0.5"),
                    ("renderamt", "128"),
                    ("rendercolor", "40 80 120"),
                    ("spawnflags", &flags.to_string()),
                ])],
                0,
            );
            let mut runtime = MapEffectsRuntime::default();
            use_effect(&mut runtime, &mut registry, &mut simulation, "fade", player);
            let start = runtime.presentation().fade.unwrap();
            assert_eq!(start.amount, if reverse { 128.0 / 255.0 } else { 0.0 });
            advance(&mut runtime, 20, &EmptyWorld);
            let middle = runtime.presentation().fade.unwrap();
            let expected = if reverse { 128.0 / 255.0 } else { 64.0 / 255.0 };
            assert!((middle.amount - expected).abs() < 1e-5);
            let scene = [0.6, 0.5, 0.4];
            let rendered = middle.composite(scene);
            for channel in 0..3 {
                let target = if modulate {
                    scene[channel] * middle.color[channel]
                } else {
                    middle.color[channel]
                };
                assert!(
                    (rendered[channel]
                        - (scene[channel] * (1.0 - middle.amount) + target * middle.amount))
                        .abs()
                        < 1e-6
                );
            }
            let mut overlay = middle;
            overlay.modulate = !modulate;
            assert_ne!(middle.composite(scene), overlay.composite(scene));
            advance(&mut runtime, 41, &EmptyWorld);
            assert!(runtime.presentation().fade.is_none());
        }
    }

    #[test]
    fn optional_restore_remaps_delayed_player_and_preserves_pending_one_shot() {
        let raw = [
            raw(&[
                ("classname", "trigger_relay"),
                ("targetname", "switch"),
                ("target", "fade"),
                ("delay", "0.25"),
                ("triggerstate", "1"),
            ]),
            raw(&[
                ("classname", "env_fade"),
                ("targetname", "fade"),
                ("duration", "1"),
                ("spawnflags", "4"),
            ]),
            raw(&[
                ("classname", "env_explosion"),
                ("targetname", "blast"),
                ("iMagnitude", "20"),
            ]),
        ];
        let (mut registry, mut simulation, player) = fixture(&raw, 0);
        let mut runtime = MapEffectsRuntime::default();
        use_effect(
            &mut runtime,
            &mut registry,
            &mut simulation,
            "switch",
            player,
        );
        use_effect(
            &mut runtime,
            &mut registry,
            &mut simulation,
            "blast",
            player,
        );
        let saved = runtime.snapshot(&registry, player, &simulation);
        let legacy = simulation.snapshot();
        assert_eq!(
            saved.pending_uses[0].activator,
            Some(EffectEntityRef::Player)
        );
        assert_eq!(saved.pending_uses[0].use_type, SavedUseType::On);
        let (mut restored_registry, mut restored_simulation, restored_player) = fixture(&raw, 4);
        assert_ne!(restored_player, player);
        restored_simulation.restore(&legacy);
        runtime.restore(
            Some(&saved),
            &mut restored_registry,
            restored_player,
            &mut restored_simulation,
        );
        restored_simulation.set_effect_player(Some(EffectPlayer {
            entity: restored_player,
            origin: Vec3::ZERO,
            grounded: true,
        }));
        use_effect(
            &mut runtime,
            &mut restored_registry,
            &mut restored_simulation,
            "blast",
            restored_player,
        );
        let batch = runtime.resolve_pending(|entity| entity == restored_player);
        assert_eq!(batch.blasts.len(), 1);
        assert_eq!(batch.blasts[0].attacker, restored_player);
        restored_simulation.tick(&mut restored_registry, 0.3);
        runtime.capture_commands(restored_simulation.drain_effect_commands());
        assert!(runtime.presentation().fade.is_some());
        runtime.restore(
            None,
            &mut restored_registry,
            restored_player,
            &mut restored_simulation,
        );
        assert!(runtime.presentation().fade.is_none());
        assert!(runtime.presentation().debris.is_empty());
        assert!(runtime.resolve_pending(|_| true).blasts.is_empty());
    }

    #[test]
    fn section_dto_has_golden_bytes_and_rejects_oversized_vectors() {
        let snapshot = MapEffectsSnapshot {
            next_debris_id: 7,
            debris_rng: 11,
            consumed_explosions: vec![2],
            render_fx: vec![(3, 5)],
            active_overrides: vec![(4, true)],
            pending_uses: vec![
                PendingUseSnapshot {
                    activator: Some(EffectEntityRef::Player),
                    use_type: SavedUseType::On,
                },
                PendingUseSnapshot {
                    activator: Some(EffectEntityRef::Registry(6)),
                    use_type: SavedUseType::Off,
                },
                PendingUseSnapshot {
                    activator: None,
                    use_type: SavedUseType::Toggle,
                },
            ],
            ..MapEffectsSnapshot::default()
        };
        let bytes = postcard::to_allocvec(&snapshot).unwrap();
        assert_eq!(
            bytes,
            [
                0, 0, 7, 11, 1, 2, 1, 3, 10, 1, 4, 1, 3, 1, 0, 1, 1, 1, 6, 0, 0, 2, 0
            ]
        );
        assert_eq!(
            postcard::from_bytes::<MapEffectsSnapshot>(&bytes).unwrap(),
            snapshot
        );
        let oversized = MapEffectsSnapshot {
            active_overrides: vec![(0, true); MAX_REGISTRY_OVERRIDES + 1],
            ..MapEffectsSnapshot::default()
        };
        assert!(
            postcard::from_bytes::<MapEffectsSnapshot>(&postcard::to_allocvec(&oversized).unwrap())
                .is_err()
        );
        let oversized = MapEffectsSnapshot {
            pending_uses: vec![
                PendingUseSnapshot {
                    activator: None,
                    use_type: SavedUseType::Toggle
                };
                MAX_PENDING_EFFECTS + 1
            ],
            ..MapEffectsSnapshot::default()
        };
        assert!(
            postcard::from_bytes::<MapEffectsSnapshot>(&postcard::to_allocvec(&oversized).unwrap())
                .is_err()
        );
    }

    #[test]
    fn live_fx_and_active_overrides_restore_while_fade_and_shake_clear() {
        let raw = [
            raw(&[
                ("classname", "env_render"),
                ("targetname", "controller"),
                ("target", "beam"),
                ("renderfx", "17"),
            ]),
            raw(&[
                ("classname", "env_beam"),
                ("targetname", "beam"),
                ("spawnflags", "2"),
                ("LightningStart", "a"),
                ("LightningEnd", "b"),
                ("renderfx", "4"),
            ]),
            raw(&[("classname", "info_target"), ("targetname", "a")]),
            raw(&[("classname", "info_target"), ("targetname", "b")]),
            raw(&[
                ("classname", "env_fade"),
                ("targetname", "fade"),
                ("duration", "2"),
                ("renderamt", "255"),
            ]),
            raw(&[
                ("classname", "env_shake"),
                ("targetname", "shake"),
                ("duration", "2"),
                ("amplitude", "5"),
                ("spawnflags", "1"),
            ]),
        ];
        let (mut registry, mut simulation, player) = fixture(&raw, 0);
        let mut runtime = MapEffectsRuntime::default();
        for name in ["controller", "beam", "fade", "shake"] {
            use_effect(&mut runtime, &mut registry, &mut simulation, name, player);
        }
        assert!(runtime.presentation().fade.is_some());
        assert_ne!(runtime.presentation().shake, ViewShake::default());
        let saved = runtime.snapshot(&registry, player, &simulation);
        let (mut loaded_registry, mut loaded_simulation, loaded_player) = fixture(&raw, 2);
        runtime.restore(
            Some(&saved),
            &mut loaded_registry,
            loaded_player,
            &mut loaded_simulation,
        );
        let beam = loaded_registry.find("beam")[0];
        assert_eq!(loaded_registry.world.get::<&RenderFx>(beam).unwrap().0, 17);
        assert!(
            loaded_registry
                .world
                .get::<&EffectActive>(beam)
                .unwrap()
                .active
        );
        assert!(runtime.presentation().fade.is_none());
        assert_eq!(runtime.presentation().shake, ViewShake::default());
    }

    #[test]
    fn debris_decode_restore_and_admission_enforce_caps_and_validity() {
        let entities: Vec<_> = (0..130)
            .map(|index| {
                raw(&[
                    ("classname", "func_breakable"),
                    ("targetname", &format!("crate{index}")),
                    ("model", "*1"),
                    ("origin", "0 0 40"),
                ])
            })
            .collect();
        let (mut registry, mut simulation, player) = fixture(&entities, 0);
        let mut runtime = MapEffectsRuntime::default();
        for index in 0..130 {
            use_effect(
                &mut runtime,
                &mut registry,
                &mut simulation,
                &format!("crate{index}"),
                player,
            );
        }
        let batch = runtime.resolve_pending(|_| true);
        assert_eq!(batch.broken_sources.len(), 130);
        assert_eq!(runtime.presentation().debris.len(), MAX_DEBRIS);
        let mut saved = runtime.snapshot(&registry, player, &simulation);
        saved.debris.push(saved.debris[0].clone());
        assert!(
            postcard::from_bytes::<MapEffectsSnapshot>(&postcard::to_allocvec(&saved).unwrap())
                .is_err()
        );
        saved.debris.truncate(4);
        saved.debris[1] = saved.debris[0].clone();
        saved.debris[2].velocity.x = f32::NAN;
        saved.debris[3].age = saved.debris[3].lifetime;
        runtime.restore(Some(&saved), &mut registry, player, &mut simulation);
        assert_eq!(runtime.presentation().debris.len(), 1);
        let mut invalid_model = saved;
        invalid_model.debris[0].gib_model = Some("x".repeat(261));
        assert!(
            postcard::from_bytes::<MapEffectsSnapshot>(
                &postcard::to_allocvec(&invalid_model).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn combat_command_budget_defers_overflow_until_the_next_fixed_step() {
        let (mut registry, mut simulation, player) = fixture(
            &[raw(&[
                ("classname", "env_explosion"),
                ("targetname", "blast"),
                ("spawnflags", "2"),
            ])],
            0,
        );
        let mut runtime = MapEffectsRuntime::default();
        for _ in 0..=MAX_PENDING_EFFECTS {
            use_effect(
                &mut runtime,
                &mut registry,
                &mut simulation,
                "blast",
                player,
            );
        }
        assert_eq!(
            runtime.resolve_pending(|_| true).blasts.len(),
            MAX_PENDING_EFFECTS
        );
        use_effect(
            &mut runtime,
            &mut registry,
            &mut simulation,
            "blast",
            player,
        );
        assert!(runtime.resolve_pending(|_| true).blasts.is_empty());
        runtime.begin_step();
        assert_eq!(runtime.resolve_pending(|_| true).blasts.len(), 1);
        assert!(runtime.resolve_pending(|_| true).blasts.is_empty());
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod engine_tests {
    use super::*;
    use crate::test_support::{
        BREAKABLE_OBSTACLE_MAXS, entity_block, obstacle_corridor_bsp,
        synthetic_map_bsp_with_entities,
    };
    use crate::{Game, Input, MemoryAssets};

    struct CountedAssets<'a> {
        assets: &'a MemoryAssets,
        reads: std::cell::Cell<usize>,
    }

    impl crate::AssetSource for CountedAssets<'_> {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            crate::AssetSource::read(self.assets, path)
        }
    }

    fn fixture(extra: &str, obstacle: bool) -> (Game, MemoryAssets) {
        let entities = format!(
            "{}{}{}",
            entity_block("worldspawn", [0.0; 3], 0.0, &[]),
            entity_block("info_player_start", [0.0, 0.0, 40.0], 0.0, &[]),
            extra
        );
        let bytes = if obstacle {
            obstacle_corridor_bsp(&entities, BREAKABLE_OBSTACLE_MAXS, false, false)
        } else {
            synthetic_map_bsp_with_entities(&entities)
        };
        let mut assets = MemoryAssets::new();
        assets.insert("maps/ohl_effect_tests.bsp", bytes);
        (Game::load(&assets, "ohl_effect_tests").unwrap(), assets)
    }

    fn tick(game: &mut Game, input: &Input) {
        game.tick(crate::tick::TICK_SECONDS, input);
    }
    fn idle(game: &mut Game, steps: usize) {
        for _ in 0..steps {
            tick(game, &Input::default());
        }
    }
    fn press(game: &mut Game) {
        tick(
            game,
            &Input {
                use_pressed: true,
                ..Input::default()
            },
        );
    }
    fn button(target: &str, delay: &str) -> String {
        entity_block(
            "func_button",
            [0.0, 0.0, 40.0],
            0.0,
            &[
                ("targetname", "switch"),
                ("target", target),
                ("delay", delay),
                ("wait", "-1"),
            ],
        )
    }
    fn state(game: &Game) -> MapEffectsSnapshot {
        game.to_save(123).map_effects.unwrap()
    }
    fn faded(game: &mut Game) -> bool {
        game.systems_mut().map_effects.presentation().fade.is_some()
    }

    #[test]
    fn section43_uses_real_container_golden_roundtrip_absence_and_bounds() {
        let (game, assets) = fixture("", false);
        let mut save = game.to_save(123);
        let expected = MapEffectsSnapshot {
            next_debris_id: 7,
            debris_rng: 11,
            consumed_explosions: vec![2],
            render_fx: vec![(3, 5)],
            active_overrides: vec![(4, true)],
            pending_uses: vec![
                PendingUseSnapshot {
                    activator: Some(EffectEntityRef::Player),
                    use_type: SavedUseType::On,
                },
                PendingUseSnapshot {
                    activator: Some(EffectEntityRef::Registry(6)),
                    use_type: SavedUseType::Off,
                },
                PendingUseSnapshot {
                    activator: None,
                    use_type: SavedUseType::Toggle,
                },
            ],
            ..MapEffectsSnapshot::default()
        };
        const GOLDEN: &[u8] = &[
            0, 0, 7, 11, 1, 2, 1, 3, 10, 1, 4, 1, 3, 1, 0, 1, 1, 1, 6, 0, 0, 2, 0,
        ];
        save.map_effects = Some(expected.clone());
        let bytes = save.to_bytes().unwrap();
        let reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default()).unwrap();
        assert_eq!(
            reader.section(crate::save::SECTION_MAP_EFFECTS).unwrap(),
            GOLDEN
        );
        assert_eq!(
            crate::save::GameSave::from_bytes(&bytes)
                .unwrap()
                .map_effects,
            Some(expected)
        );
        for bad in [
            [GOLDEN, &[0]].concat(),
            postcard::to_allocvec(&MapEffectsSnapshot {
                active_overrides: vec![(0, true); MAX_REGISTRY_OVERRIDES + 1],
                ..MapEffectsSnapshot::default()
            })
            .unwrap(),
            postcard::to_allocvec(&MapEffectsSnapshot {
                button_activators: vec![
                    (EffectEntityRef::Player, EffectEntityRef::Player);
                    MAX_PENDING_EFFECTS + 1
                ],
                ..MapEffectsSnapshot::default()
            })
            .unwrap(),
        ] {
            let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
            for entry in reader.sections() {
                writer
                    .add_section(
                        entry.tag,
                        if entry.tag == crate::save::SECTION_MAP_EFFECTS {
                            &bad
                        } else {
                            reader.section(entry.tag).unwrap()
                        },
                    )
                    .unwrap();
            }
            assert!(
                crate::save::GameSave::from_bytes(
                    &writer.finish(&ohl_save::Limits::default()).unwrap()
                )
                .is_err()
            );
        }
        save.map_effects = None;
        let absent = save.to_bytes().unwrap();
        assert!(
            crate::save::GameSave::from_bytes(&absent)
                .unwrap()
                .map_effects
                .is_none()
        );
        let restored = Game::load_bytes(&assets, &absent).unwrap();
        let fresh = state(&restored);
        assert!(
            fresh.pending.is_empty()
                && fresh.debris.is_empty()
                && fresh.consumed_explosions.is_empty()
        );
        save.map_effects = Some(MapEffectsSnapshot {
            pending_uses: vec![
                PendingUseSnapshot {
                    activator: None,
                    use_type: SavedUseType::Toggle
                };
                MAX_PENDING_EFFECTS + 1
            ],
            ..MapEffectsSnapshot::default()
        });
        assert!(save.to_bytes().is_err());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn section43_nonempty_nested_schema_has_independent_literal_container_golden() {
        use ohl_game::effects::{BreakCommand, BreakEffects, ExplosionDef};
        // Field-by-field protocol transcription, independent of the Rust codec:
        // float bytes are little-endian IEEE754; integer/tag bytes are postcard
        // unsigned LEB128 or signed zigzag. Neither encoder computes this literal.
        const GOLDEN: &[u8] = &[
            // Two commands; explosion source/activator/origin/variant
            2, 1, 3, 1, 0, // Explosion origin and profile
            0, 0, 128, 63, 0, 0, 0, 64, 0, 0, 64, 64, 0, 0, 0, 128, 64, 0, 0, 160, 64,
            // Explosion damage/repeat and four visual flags
            0, 1, 1, 0, 1, 0, // Break source/actor, origin, variant and extents
            1, 6, 1, 1, 7, 0, 0, 0, 65, 0, 0, 16, 65, 0, 0, 32, 65, 3, 0, 0, 48, 65, 0, 0, 64, 65,
            0, 0, 80, 65, // Break material, optional direction, profile and model
            2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 96, 65, 0, 0, 112, 65, 1, 1, 12,
            109, 111, 100, 101, 108, 115, 47, 98, 46, 109, 100, 108,
            // One debris record: id/source/material
            1, 16, 1, 17, 3, // Debris position/velocity/angles/angular velocity
            0, 0, 144, 65, 0, 0, 152, 65, 0, 0, 160, 65, 0, 0, 168, 65, 0, 0, 176, 65, 0, 0, 184,
            65, 0, 0, 192, 65, 0, 0, 200, 65, 0, 0, 208, 65, 0, 0, 216, 65, 0, 0, 224, 65, 0, 0,
            232, 65, // Debris bounds/age/lifetime/resting/model
            0, 0, 160, 63, 0, 0, 32, 64, 0, 0, 112, 64, 0, 0, 0, 63, 0, 0, 208, 64, 0, 1, 12, 109,
            111, 100, 101, 108, 115, 47, 100, 46, 109, 100, 108,
            // Next id/RNG, consumed sources, signed FX rows
            172, 2, 173, 2, 2, 31, 32, 2, 33, 67, 35, 72,
            // Active rows and pending use rows
            2, 37, 1, 38, 0, 3, 1, 0, 1, 1, 1, 39, 0, 0, 2, // Two button/activator pairs
            2, 1, 40, 0, 1, 41, 1, 42,
        ];
        let expected = MapEffectsSnapshot {
            pending: vec![
                MapEffectCommand {
                    source: EffectEntityRef::Registry(3),
                    activator: Some(EffectEntityRef::Player),
                    origin: Vec3::new(1.0, 2.0, 3.0),
                    effect: MapEffect::Explosion(ExplosionDef {
                        blast: MapBlastProfile {
                            damage: 4.0,
                            radius: 5.0,
                        },
                        no_damage: false,
                        repeatable: true,
                        visuals: ExplosionVisuals {
                            fireball: true,
                            smoke: false,
                            decal: true,
                            sparks: false,
                        },
                    }),
                },
                MapEffectCommand {
                    source: EffectEntityRef::Registry(6),
                    activator: Some(EffectEntityRef::Registry(7)),
                    origin: Vec3::new(8.0, 9.0, 10.0),
                    effect: MapEffect::Break(BreakCommand {
                        half_extents: Vec3::new(11.0, 12.0, 13.0),
                        material: 2,
                        attack_direction: Some(Vec3::Z),
                        effects: BreakEffects {
                            blast: MapBlastProfile {
                                damage: 14.0,
                                radius: 15.0,
                            },
                            attack_relative: true,
                            gib_model: Some("models/b.mdl".to_owned()),
                        },
                    }),
                },
            ],
            debris: vec![DebrisRecord {
                id: 16,
                source: EffectEntityRef::Registry(17),
                material: 3,
                position: Vec3::new(18.0, 19.0, 20.0),
                velocity: Vec3::new(21.0, 22.0, 23.0),
                angles: Vec3::new(24.0, 25.0, 26.0),
                angular_velocity: Vec3::new(27.0, 28.0, 29.0),
                half_extents: Vec3::new(1.25, 2.5, 3.75),
                age: 0.5,
                lifetime: 6.5,
                resting: false,
                gib_model: Some("models/d.mdl".to_owned()),
            }],
            next_debris_id: 300,
            debris_rng: 301,
            consumed_explosions: vec![31, 32],
            render_fx: vec![(33, -34), (35, 36)],
            active_overrides: vec![(37, true), (38, false)],
            pending_uses: vec![
                PendingUseSnapshot {
                    activator: Some(EffectEntityRef::Player),
                    use_type: SavedUseType::On,
                },
                PendingUseSnapshot {
                    activator: Some(EffectEntityRef::Registry(39)),
                    use_type: SavedUseType::Off,
                },
                PendingUseSnapshot {
                    activator: None,
                    use_type: SavedUseType::Toggle,
                },
            ],
            button_activators: vec![
                (EffectEntityRef::Registry(40), EffectEntityRef::Player),
                (EffectEntityRef::Registry(41), EffectEntityRef::Registry(42)),
            ],
        };
        let (game, _) = fixture("", false);
        let mut save = game.to_save(123);
        save.map_effects = Some(expected.clone());
        let encoded = save.to_bytes().unwrap();
        let limits = ohl_save::Limits::default();
        let reader = ohl_save::SaveReader::open(&encoded, &limits).unwrap();
        assert_eq!(
            reader.section(crate::save::SECTION_MAP_EFFECTS).unwrap(),
            GOLDEN
        );
        let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
        for entry in reader.sections() {
            writer
                .add_section(
                    entry.tag,
                    if entry.tag == crate::save::SECTION_MAP_EFFECTS {
                        GOLDEN
                    } else {
                        reader.section(entry.tag).unwrap()
                    },
                )
                .unwrap();
        }
        let fixed = writer.finish(&limits).unwrap();
        let decoded = crate::save::GameSave::from_bytes(&fixed).unwrap();
        assert_eq!(decoded.map_effects, Some(expected));
        let reencoded = decoded.to_bytes().unwrap();
        assert_eq!(
            ohl_save::SaveReader::open(&reencoded, &limits)
                .unwrap()
                .section(crate::save::SECTION_MAP_EFFECTS)
                .unwrap(),
            GOLDEN
        );
    }

    #[test]
    fn missing43_preserves_legacy_display_defaults_and_explicit_black_zero() {
        let extra = entity_block("env_beam", [0.0; 3], 0.0, &[("targetname", "default_beam")])
            + &entity_block(
                "env_beam",
                [0.0; 3],
                0.0,
                &[("targetname", "zero_beam"), ("renderamt", "0")],
            )
            + &entity_block(
                "env_sprite",
                [0.0; 3],
                0.0,
                &[("targetname", "default_sprite")],
            )
            + &entity_block(
                "env_sprite",
                [0.0; 3],
                0.0,
                &[("targetname", "black_sprite"), ("rendercolor", "0 0 0")],
            );
        let (game, assets) = fixture(&extra, false);
        let mut old = game.to_save(123);
        old.map_effects = None;
        for (snapshot, def) in old.entities.iter_mut().zip(game.entity_defs()) {
            snapshot.render = Some(def.render);
        }
        let loaded = Game::load_bytes(&assets, &old.to_bytes().unwrap()).unwrap();
        let props = |name: &str| {
            let entity = loaded.registry().find(name)[0];
            *loaded
                .registry()
                .world
                .get::<&ohl_game::registry::RenderPropsComponent>(entity)
                .unwrap()
        };
        assert_eq!(props("default_beam").amt, 255);
        assert_eq!(props("zero_beam").amt, 0);
        assert_eq!(props("default_sprite").color, [255; 3]);
        assert_eq!(props("black_sprite").color, [0; 3]);
    }

    #[test]
    fn real_use_button_completion_relay_and_restore_keep_player_fade_provenance() {
        let extra = button("relay", "0.2")
            + &entity_block(
                "trigger_relay",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "relay"),
                    ("target", "fade"),
                    ("delay", "0.3"),
                    ("triggerstate", "1"),
                ],
            )
            + &entity_block(
                "env_fade",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "fade"),
                    ("spawnflags", "4"),
                    ("duration", "2"),
                    ("renderamt", "255"),
                ],
            );
        let (mut game, assets) = fixture(&extra, false);
        press(&mut game);
        let save = game.to_save(123);
        assert_eq!(
            save.map_effects.as_ref().unwrap().button_activators.len(),
            1
        );
        assert_eq!(
            save.map_effects.as_ref().unwrap().button_activators[0].1,
            EffectEntityRef::Player
        );
        let mut loaded = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
        let mut legacy = save.clone();
        legacy.map_effects = None;
        let mut old = Game::load_bytes(&assets, &legacy.to_bytes().unwrap()).unwrap();
        idle(&mut game, 80);
        idle(&mut loaded, 80);
        idle(&mut old, 80);
        assert!(faded(&mut game) && faded(&mut loaded));
        assert!(
            !faded(&mut old),
            "absence keeps prior button-source activation, never guesses player"
        );
        let saved_active_fade = game.save_bytes(123).unwrap();
        assert!(!faded(
            &mut Game::load_bytes(&assets, &saved_active_fade).unwrap()
        ));
        let (mut nonplayer, _) = fixture(&extra, false);
        let (level, _) = nonplayer.level_and_systems_mut();
        let switch = level.registry.find("switch")[0];
        level
            .simulation
            .use_entity(&mut level.registry, switch, Some(switch), &mut Vec::new());
        idle(&mut nonplayer, 80);
        assert!(!faded(&mut nonplayer));
    }

    fn shot_button_fixture(classname: &str) -> (Game, MemoryAssets) {
        let entities = entity_block("worldspawn", [0.0; 3], 0.0, &[])
            + &entity_block("info_player_start", [0.0, -24.0, 40.0], 90.0, &[])
            + &entity_block("weapon_357", [0.0, -24.0, 40.0], 0.0, &[])
            + &entity_block(
                classname,
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "shot"),
                    ("target", "relay"),
                    ("model", "*1"),
                    ("health", "30"),
                    ("delay", "0.3"),
                    ("wait", "-1"),
                    ("distance", "90"),
                    ("speed", "360"),
                ],
            )
            + &entity_block(
                "trigger_relay",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "relay"),
                    ("target", "effect"),
                    ("delay", "0.2"),
                    ("triggerstate", "1"),
                ],
            )
            + &entity_block(
                "env_fade",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "effect"),
                    ("spawnflags", "4"),
                    ("duration", "3"),
                    ("renderamt", "255"),
                ],
            )
            + &entity_block(
                "env_explosion",
                [0.0, -24.0, 36.0],
                0.0,
                &[("targetname", "effect"), ("iMagnitude", "5")],
            )
            + &entity_block(
                "monster_scientist",
                [128.0, 128.0, 40.0],
                0.0,
                &[("targetname", "npc")],
            );
        let mut assets = MemoryAssets::new();
        assets.insert(
            "maps/ohl_shot_effects.bsp",
            crate::test_support::rot_button_bsp(&entities),
        );
        (Game::load(&assets, "ohl_shot_effects").unwrap(), assets)
    }

    fn queued_button_hit(game: &mut Game, attacker: Option<Entity>, amount: f32) {
        let button = game.registry().find("shot")[0];
        game.systems_mut()
            .damage_queue
            .push(crate::systems::QueuedDamage {
                target: button,
                info: ohl_combat::DamageInfo {
                    attacker: attacker.map(crate::ids::entity_id),
                    inflictor: attacker.map(crate::ids::entity_id),
                    amount,
                    kind: DamageType::BULLET,
                    origin: Vec3::ZERO,
                    direction: Vec3::Y,
                },
            });
        tick(game, &Input::default());
    }

    fn finish_shot_effect(game: &mut Game) -> (bool, Option<EffectEntityRef>) {
        for _ in 0..120 {
            if let Some(command) = state(game)
                .pending
                .iter()
                .find(|command| matches!(command.effect, MapEffect::Explosion(_)))
            {
                return (faded(game), command.activator);
            }
            tick(game, &Input::default());
        }
        panic!("delayed shot button must eventually queue its target explosion");
    }

    #[test]
    fn real_fire_button_and_rot_button_keep_damage_activator_through_save_and_ignored_hits() {
        for classname in ["func_button", "func_rot_button"] {
            let (mut live, assets) = shot_button_fixture(classname);
            tick(&mut live, &Input::default());
            assert!(live.inventory().has_weapon(ohl_combat::WeaponId::Python));
            tick(
                &mut live,
                &Input {
                    select_slot: Some(2),
                    ..Input::default()
                },
            );
            tick(
                &mut live,
                &Input {
                    reload: true,
                    ..Input::default()
                },
            );
            // Match the existing shot-button fixture's three-second reload wait.
            idle(&mut live, 300);
            assert!(live.inventory().clip(ohl_combat::WeaponId::Python) > 0);
            tick(
                &mut live,
                &Input {
                    attack: true,
                    ..Input::default()
                },
            );
            let saved = live.to_save(123);
            assert_eq!(
                saved.map_effects.as_ref().unwrap().button_activators.len(),
                1
            );
            assert_eq!(
                saved.map_effects.as_ref().unwrap().button_activators[0].1,
                EffectEntityRef::Player
            );
            let mut loaded = Game::load_bytes(&assets, &saved.to_bytes().unwrap()).unwrap();
            let mut old_save = saved;
            old_save.map_effects = None;
            let mut old = Game::from_save(&assets, &old_save).unwrap();
            let npc = live.registry().find("npc")[0];
            queued_button_hit(&mut live, Some(npc), 40.0);
            queued_button_hit(&mut live, None, 1.0);
            assert_eq!(
                state(&live).button_activators[0].1,
                EffectEntityRef::Player,
                "nonactivating damage cannot replace the original press identity"
            );
            assert_eq!(
                finish_shot_effect(&mut live),
                (true, Some(EffectEntityRef::Player))
            );
            assert_eq!(
                finish_shot_effect(&mut loaded),
                (true, Some(EffectEntityRef::Player))
            );
            let (old_fade, old_actor) = finish_shot_effect(&mut old);
            assert!(!old_fade && old_actor != Some(EffectEntityRef::Player));
        }
    }

    #[test]
    fn nonplayer_and_unknown_button_damage_never_fabricate_player_activation() {
        for classname in ["func_button", "func_rot_button"] {
            for npc_damage in [false, true] {
                let (mut live, assets) = shot_button_fixture(classname);
                let npc = live.registry().find("npc")[0];
                queued_button_hit(&mut live, npc_damage.then_some(npc), 40.0);
                let saved = live.save_bytes(123).unwrap();
                let mut loaded = Game::load_bytes(&assets, &saved).unwrap();
                let expected = finish_shot_effect(&mut live);
                assert!(!expected.0 && expected.1 != Some(EffectEntityRef::Player));
                assert_eq!(finish_shot_effect(&mut loaded), expected);
                if npc_damage {
                    let index = live
                        .registry()
                        .entities
                        .iter()
                        .position(|entity| *entity == npc)
                        .unwrap();
                    assert_eq!(
                        expected.1,
                        Some(EffectEntityRef::Registry(u32::try_from(index).unwrap()))
                    );
                }
            }
        }
    }

    #[test]
    fn touch_trigger_supplies_real_player_to_activator_only_fade() {
        let extra = entity_block(
            "trigger_once",
            [0.0; 3],
            0.0,
            &[("targetname", "touch"), ("target", "fade"), ("model", "*1")],
        ) + &entity_block(
            "env_fade",
            [0.0; 3],
            0.0,
            &[
                ("targetname", "fade"),
                ("spawnflags", "4"),
                ("duration", "2"),
                ("renderamt", "255"),
            ],
        );
        let (mut game, _) = fixture(&extra, true);
        // Ordinary movement, not a direct use/queue hook, enters the authored volume.
        for _ in 0..60 {
            tick(
                &mut game,
                &Input {
                    forward: 1,
                    ..Input::default()
                },
            );
            if faded(&mut game) {
                break;
            }
        }
        assert!(game.touch_trigger_count() > 0);
        assert!(faded(&mut game));
    }

    #[test]
    fn shared_blast_preserves_attacker_self_rule_and_distinct_inflictor() {
        let (mut game, _) = fixture(
            &entity_block("env_explosion", [0.0; 3], 0.0, &[("targetname", "source")]),
            false,
        );
        let (level, _) = game.level_and_systems_mut();
        let source = level.registry.find("source")[0];
        let player = level.player;
        let center = level
            .registry
            .world
            .get::<&ohl_game::registry::Transform>(player)
            .unwrap()
            .origin;
        let mut rule = ohl_combat::ExplosionRule::default();
        rule.self_damage_scale.value = 0.25;
        let mut damage = Vec::new();
        crate::projectiles::dispatch_blast(
            level,
            center,
            100.0,
            40.0,
            DamageType::BLAST,
            Some(crate::ids::entity_id(player)),
            Some(crate::ids::entity_id(source)),
            &rule,
            &BTreeMap::new(),
            &mut damage,
        );
        let hit = damage.iter().find(|hit| hit.target == player).unwrap();
        assert_eq!(hit.info.amount, 10.0);
        assert_eq!(hit.info.attacker, Some(crate::ids::entity_id(player)));
        assert_eq!(hit.info.inflictor, Some(crate::ids::entity_id(source)));
    }

    #[test]
    fn real_use_shake_reads_grounding_and_only_changes_a_fresh_view_copy() {
        let extra = button("shake", "0")
            + &entity_block(
                "env_shake",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "shake"),
                    ("spawnflags", "1"),
                    ("amplitude", "6"),
                    ("duration", "2"),
                    ("frequency", "4"),
                ],
            );
        for jumping in [false, true] {
            let (mut game, _) = fixture(&extra, false);
            idle(&mut game, 100);
            assert!(game.player_on_ground());
            tick(
                &mut game,
                &Input {
                    use_pressed: true,
                    jump: jumping,
                    ..Input::default()
                },
            );
            idle(&mut game, 4);
            let camera = *game.camera();
            let origin = game.player_origin();
            let view = game.systems_mut().map_effects.view_camera(&camera);
            assert_eq!(view.position != camera.position, !jumping);
            assert_eq!(
                view.position,
                game.systems_mut().map_effects.view_camera(&camera).position
            );
            assert_eq!(game.camera().position, camera.position);
            assert_eq!(game.player_origin(), origin);
        }
    }

    #[test]
    fn real_use_blast_break_cascade_preserves_damage_actor_and_direction() {
        let extra = button("blast", "0")
            + &entity_block(
                "env_explosion",
                [40.0, 0.0, 40.0],
                0.0,
                &[("targetname", "blast"), ("iMagnitude", "40")],
            )
            + &entity_block(
                "func_breakable",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "first"),
                    ("target", "relay"),
                    ("model", "*1"),
                    ("health", "1"),
                    ("explodemagnitude", "80"),
                    ("explosion", "1"),
                ],
            )
            + &entity_block(
                "func_breakable",
                [48.0, 0.0, 0.0],
                0.0,
                &[
                    ("targetname", "second"),
                    ("model", "*1"),
                    ("health", "15"),
                    ("explodemagnitude", "80"),
                    ("explosion", "1"),
                ],
            )
            + &entity_block(
                "trigger_relay",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "relay"),
                    ("target", "fade"),
                    ("triggerstate", "1"),
                ],
            )
            + &entity_block(
                "env_fade",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "fade"),
                    ("spawnflags", "4"),
                    ("duration", "2"),
                    ("renderamt", "255"),
                ],
            );
        let (mut game, _) = fixture(&extra, true);
        press(&mut game);
        let first = game.registry().find("first")[0];
        for _ in 0..8 {
            if game
                .registry()
                .world
                .get::<&ohl_game::registry::Breakable>(first)
                .unwrap()
                .broken
            {
                break;
            }
            tick(&mut game, &Input::default());
        }
        for name in ["first", "second"] {
            let entity = game.registry().find(name)[0];
            assert!(
                game.registry()
                    .world
                    .get::<&ohl_game::registry::Breakable>(entity)
                    .unwrap()
                    .broken
            );
            assert!(
                !game
                    .brush_collision()
                    .iter()
                    .any(|(source, _)| *source == entity),
                "player collision detaches in the same cascade step"
            );
            assert!(
                !game
                    .monster_brush_collision()
                    .iter()
                    .any(|(source, _)| *source == entity),
                "monster collision detaches in the same cascade step"
            );
        }
        let snapshot = state(&game);
        assert_eq!(snapshot.debris.len(), 12);
        assert!(snapshot.debris.iter().all(|record| record.velocity.x > 0.0));
        assert!(
            faded(&mut game),
            "damage credit survives a break target and relay"
        );
        assert!(
            game.player_damage_event_count() >= 2,
            "secondary blasts resolve before lifecycle drains the queue"
        );
    }

    #[test]
    fn real_button_break_detaches_both_models_and_debris_continues_across_save() {
        let extra = button("break", "0")
            + &entity_block(
                "func_breakable",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "break"),
                    ("model", "*1"),
                    ("health", "1"),
                    ("material", "1"),
                    ("explodemagnitude", "80"),
                ],
            );
        let (mut game, assets) = fixture(&extra, true);
        let source = game.registry().find("break")[0];
        assert!(
            game.brush_collision()
                .iter()
                .any(|(entity, _)| *entity == source)
        );
        assert!(
            game.monster_brush_collision()
                .iter()
                .any(|(entity, _)| *entity == source)
        );
        press(&mut game);
        idle(&mut game, 8);
        assert!(
            game.registry()
                .world
                .get::<&ohl_game::registry::Breakable>(source)
                .unwrap()
                .broken
        );
        assert!(
            !game
                .brush_collision()
                .iter()
                .any(|(entity, _)| *entity == source)
        );
        assert!(
            !game
                .monster_brush_collision()
                .iter()
                .any(|(entity, _)| *entity == source)
        );
        assert!(
            game.player_damage_event_count() > 0,
            "broken source cannot occlude its own blast"
        );
        let first = state(&game);
        assert_eq!(first.debris.len(), 6);
        assert!(
            first
                .debris
                .iter()
                .any(|record| record.velocity != Vec3::ZERO)
        );
        let mut restored = Game::load_bytes(&assets, &game.save_bytes(123).unwrap()).unwrap();
        assert_eq!(state(&restored), first);
        for _ in 0..40 {
            tick(&mut game, &Input::default());
            tick(&mut restored, &Input::default());
        }
        assert_eq!(state(&restored), state(&game));
        assert_ne!(state(&game).debris[0].position, first.debris[0].position);
        let count = state(&game).next_debris_id;
        {
            let (level, _) = game.level_and_systems_mut();
            level.simulation.use_entity(
                &mut level.registry,
                source,
                Some(level.player),
                &mut Vec::new(),
            );
        }
        idle(&mut game, 4);
        assert_eq!(state(&game).next_debris_id, count);
        idle(&mut game, 1200);
        assert!(state(&game).debris.is_empty());
    }

    #[test]
    fn custom_gib_asset_preloads_once_and_missing_or_degenerate_assets_fall_back() {
        for asset_case in 0..4 {
            let present = asset_case == 1;
            let extra = button("break", "0")
                + &entity_block(
                    "func_breakable",
                    [0.0; 3],
                    0.0,
                    &[
                        ("targetname", "break"),
                        ("model", "*1"),
                        ("health", "1"),
                        ("gibmodel", "models/ohl_test_gib.mdl"),
                    ],
                );
            let (_, mut assets) = fixture(&extra, true);
            if present {
                let (mut model, _) = ohl_formats::test_support::build_minimal_mdl10();
                // Synthetic header bounds: a planar model is still drawable.
                for (offset, value) in [
                    (112, -8.0_f32),
                    (116, -4.0),
                    (120, 2.0),
                    (124, 8.0),
                    (128, 4.0),
                    (132, 2.0),
                ] {
                    model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                }
                assets.insert("models/ohl_test_gib.mdl", model);
            } else if asset_case == 2 {
                assets.insert("models/ohl_test_gib.mdl", vec![0; 32]);
            } else if asset_case == 3 {
                let bytes = crate::debris::synthetic_blank_first_gib_model();
                let model =
                    ohl_world::StudioModel::parse(&bytes, &ohl_world::StudioLimits::default())
                        .unwrap();
                assert!(!model.vertices.is_empty() && !model.indices.is_empty());
                assert!(model.visible_meshes(&[]).is_empty());
                assert!(!model.visible_meshes(&[1]).is_empty());
                assert!(ohl_world::StudioPose::sample(&model, 0, 0.0).is_ok());
                assets.insert("models/ohl_test_gib.mdl", bytes);
            }
            let source = CountedAssets {
                assets: &assets,
                reads: std::cell::Cell::new(0),
            };
            let mut game = Game::load(&source, "ohl_effect_tests").unwrap();
            let initial_reads = source.reads.get();
            press(&mut game);
            idle(&mut game, 8);
            let (level, systems) = game.level_and_systems_mut();
            level.preload_debris_models(&source);
            assert_eq!(
                source.reads.get(),
                initial_reads,
                "no tick reads or preload retries"
            );
            let record = &systems.map_effects.presentation().debris[0];
            assert_eq!(level.debris_models.models.len(), usize::from(present));
            assert_eq!(level.debris_models.placement(record).is_some(), present);
            if present {
                let (_, transform) = level.debris_models.placement(record).unwrap();
                let matrix = glam::Mat4::from_cols_array(&transform);
                let lengths = [
                    matrix.x_axis.truncate().length(),
                    matrix.y_axis.truncate().length(),
                    matrix.z_axis.truncate().length(),
                ];
                assert!(
                    (lengths[0] - lengths[1]).abs() < 1e-5
                        && (lengths[0] - lengths[2]).abs() < 1e-5
                );
                let expected_scale = (record.half_extents.x / 8.0).min(record.half_extents.y / 4.0);
                assert!((lengths[0] - expected_scale).abs() < 1e-5);
                assert!(
                    matrix
                        .transform_point3(Vec3::new(0.0, 0.0, 2.0))
                        .abs_diff_eq(record.position, 1e-5)
                );
                let expected_orientation = glam::Mat4::from_cols_array(&record.transform());
                assert!(
                    matrix
                        .x_axis
                        .truncate()
                        .normalize()
                        .abs_diff_eq(expected_orientation.x_axis.truncate(), 1e-5)
                );
                level.debris_models.models[0].bounds_max = level.debris_models.models[0].bounds_min;
                assert!(level.debris_models.placement(record).is_none());
            }
        }
    }

    #[test]
    fn real_button_explosion_is_pending_saved_once_and_no_damage_keeps_visuals() {
        for flags in [0, 1] {
            let extra = button("blast", "0")
                + &entity_block(
                    "env_explosion",
                    [12.0, 0.0, 40.0],
                    0.0,
                    &[
                        ("targetname", "blast"),
                        ("iMagnitude", "20"),
                        ("spawnflags", &flags.to_string()),
                    ],
                );
            let (mut game, assets) = fixture(&extra, false);
            press(&mut game);
            for _ in 0..8 {
                if !state(&game).pending.is_empty() {
                    break;
                }
                idle(&mut game, 1);
            }
            assert_eq!(state(&game).pending.len(), 1);
            let pending = game.save_bytes(123).unwrap();
            let mut loaded = Game::load_bytes(&assets, &pending).unwrap();
            idle(&mut game, 2);
            idle(&mut loaded, 2);
            assert_eq!(game.player_health(), loaded.player_health());
            assert_eq!(state(&game).consumed_explosions.len(), 1);
            assert_eq!(game.player_damage_event_count() > 0, flags == 0);
            let presentation = loaded.systems_mut().map_effects.presentation();
            assert_eq!(presentation.blasts.len(), 1);
            assert!(presentation.blasts[0].channels.fireball);
            let after = state(&game);
            let mut after_load = Game::load_bytes(&assets, &game.save_bytes(123).unwrap()).unwrap();
            idle(&mut after_load, 2);
            assert_eq!(after_load.player_damage_event_count(), 0);
            assert_eq!(
                state(&after_load).consumed_explosions,
                after.consumed_explosions
            );
        }
    }
}
