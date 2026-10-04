//! Typed map-effect declarations and immutable requests for the host.
//!
//! Map logic never deals damage or advances debris. Published contracts:
//! <https://wiki.svencoop.com/Env_explosion>,
//! <https://twhl.info/wiki/page/env_shake>,
//! <https://twhl.info/wiki/page/env_fade>,
//! <https://wiki.svencoop.com/Env_render>, and
//! <https://wiki.svencoop.com/Func_breakable>. Sven documentation is not
//! evidence of exact original-build formulas; the policies below are
//! project-authored and retain their TODO(black-box) boundaries.
//! Key/default mappings additionally use the published mapping FGD:
//! <https://wiki.svencoop.com/Mapping/Sven_Co-op_FGD>.

use glam::Vec3;
use hecs::Entity;

use crate::keyvalues::{EntityDef, RenderProps};
use crate::registry::{Registry, Target, TargetName, TriggerUse};

/// Project resource cap, shared by live queues and restored commands.
pub const MAX_PENDING_EFFECTS: usize = 4096;
/// Project magnitude bound; TODO(black-box): original magnitude/radius limits.
pub const MAX_BLAST_MAGNITUDE: f32 = 4096.0;
/// Project timer bound; TODO(black-box): original presentation timer limits.
pub const MAX_EFFECT_SECONDS: f32 = 3600.0;
/// Project shake radius bound; radius only controls eligibility.
pub const MAX_SHAKE_RADIUS: f32 = 65536.0;
/// Project shake amplitude bound; TODO(black-box): original accepted range.
pub const MAX_SHAKE_AMPLITUDE: f32 = 64.0;
/// Project shake frequency bound; TODO(black-box): original accepted range.
pub const MAX_SHAKE_FREQUENCY: f32 = 256.0;
/// Project fallback for absent brush geometry; TODO(black-box): debris sizing.
pub const FALLBACK_BREAK_HALF_EXTENT: f32 = 8.0;
/// Project geometry bound; prevents malformed bounds overflowing debris motion.
pub const MAX_BREAK_HALF_EXTENT: f32 = 32768.0;

/// A localized, project-authored magnitude conversion used by map blasts.
/// TODO(black-box): no inspected source establishes a historic radius formula.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MapBlastProfile {
    /// Maximum damage at the origin, before the existing combat falloff.
    pub damage: f32,
    /// Radius in world units. Zero magnitude produces a zero-radius no-op.
    pub radius: f32,
}

impl MapBlastProfile {
    /// Project policy: damage equals magnitude, radius equals twice magnitude.
    #[must_use]
    pub fn from_magnitude(magnitude: f32) -> Self {
        let magnitude = finite_clamped(magnitude, MAX_BLAST_MAGNITUDE);
        Self {
            damage: magnitude,
            radius: 2.0 * magnitude,
        }
    }
}

/// The explosion's four independently enabled presentation channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(clippy::struct_excessive_bools)]
pub struct ExplosionVisuals {
    /// Fireball sprite.
    pub fireball: bool,
    /// Smoke particles.
    pub smoke: bool,
    /// Surface scorch mark.
    pub decal: bool,
    /// Spark particles.
    pub sparks: bool,
}

impl Default for ExplosionVisuals {
    fn default() -> Self {
        Self {
            fireball: true,
            smoke: true,
            decal: true,
            sparks: true,
        }
    }
}

/// `env_explosion`'s immutable trigger parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExplosionDef {
    /// Damage/radius profile; independent from presentation suppression.
    pub blast: MapBlastProfile,
    /// No Damage suppresses combat only.
    pub no_damage: bool,
    /// Repeatable permits another trigger after this one.
    pub repeatable: bool,
    /// Independently enabled visual channels.
    pub visuals: ExplosionVisuals,
}

impl ExplosionDef {
    pub(crate) fn from_entity(def: &EntityDef) -> Self {
        Self {
            blast: MapBlastProfile::from_magnitude(number(def, "iMagnitude", 100.0)),
            no_damage: def.spawnflags & 1 != 0,
            repeatable: def.spawnflags & 2 != 0,
            visuals: ExplosionVisuals {
                fireball: def.spawnflags & 4 == 0,
                smoke: def.spawnflags & 8 == 0,
                decal: def.spawnflags & 16 == 0,
                sparks: def.spawnflags & 32 == 0,
            },
        }
    }
}

/// Separate continuation component; never appended to an old entity snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExplosionState {
    /// A nonrepeatable source consumes itself before enqueueing its request.
    pub consumed: bool,
}

/// `env_shake` parameters. TWHL specifies binary radius and grounded gating.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ShakeDef {
    /// Full eligible amplitude; never distance-attenuated.
    pub amplitude: f32,
    /// Simulation seconds before expiry.
    pub duration: f32,
    /// Authored oscillation frequency.
    pub frequency: f32,
    /// Maximum trigger-time player distance.
    pub radius: f32,
    /// The Everybody flag bypasses the radius check, but not grounding.
    pub global: bool,
}

impl ShakeDef {
    pub(crate) fn from_entity(def: &EntityDef) -> Self {
        Self {
            amplitude: finite_clamped(number(def, "amplitude", 0.0), MAX_SHAKE_AMPLITUDE),
            duration: finite_clamped(number(def, "duration", 0.0), MAX_EFFECT_SECONDS),
            frequency: finite_clamped(number(def, "frequency", 0.0), MAX_SHAKE_FREQUENCY),
            radius: finite_clamped(number(def, "radius", 0.0), MAX_SHAKE_RADIUS),
            global: def.spawnflags & 1 != 0,
        }
    }

    /// Evaluated at the trigger edge, before any next-step player movement.
    #[must_use]
    pub fn affects(self, origin: Vec3, player: EffectPlayer) -> bool {
        player.grounded
            && origin.is_finite()
            && player.origin.is_finite()
            && self.duration > 0.0
            && self.amplitude > 0.0
            && (self.global || origin.distance(player.origin) <= self.radius)
    }
}

/// `env_fade` parameters; active fade timers are cleared on save restore.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FadeDef {
    /// Ramp duration. A zero duration does nothing.
    pub duration: f32,
    /// Full-strength hold duration.
    pub hold: f32,
    /// Maximum blend strength, in `0..=255`.
    pub amount: u8,
    /// Overlay or multiplication color.
    pub color: [u8; 3],
    /// Hold at full strength before fading away (project timing policy).
    pub reverse: bool,
    /// Interpolate toward componentwise multiplication instead of overlay.
    pub modulate: bool,
    /// Only a real triggering player receives this fade.
    pub activator_only: bool,
}

impl FadeDef {
    pub(crate) fn from_entity(def: &EntityDef) -> Self {
        Self {
            duration: finite_clamped(number(def, "duration", 0.0), MAX_EFFECT_SECONDS),
            hold: finite_clamped(number(def, "holdtime", 0.0), MAX_EFFECT_SECONDS),
            amount: u8::try_from(def.render.amt.clamp(0, 255)).unwrap_or(0),
            color: def.render.color,
            reverse: def.spawnflags & 1 != 0,
            modulate: def.spawnflags & 2 != 0,
            activator_only: def.spawnflags & 4 != 0,
        }
    }
}

/// Separate render-fx field: the existing serialized `RenderProps` is frozen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RenderFx(pub i32);

impl RenderFx {
    pub(crate) fn from_entity(def: &EntityDef) -> Self {
        Self(
            def.keyvalues
                .get("renderfx")
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0),
        )
    }
}

/// `env_render` preserves fields selected by the four documented mask bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderControl {
    /// Mask bits: fx 1, amount 2, mode 4, color 8. Other bits are ignored.
    pub preserve: u32,
}

/// Authoritative activation for the visual bridge's supported declarations.
///
/// Published mappings: beam Start On 1 / Toggle 2, laser Start On 1 with
/// inherent toggling, spark Toggle 32 / Start On 64. See
/// <https://wiki.svencoop.com/Env_beam>,
/// <https://wiki.svencoop.com/Env_laser>, and
/// <https://wiki.svencoop.com/Env_spark>. Only `active` is a save override;
/// `toggleable` is rebuilt from authored keys. Both stay outside old snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectActive {
    /// Whether the declared visual is enabled, including initially-off entries.
    pub active: bool,
    /// Whether Off/Toggle may switch this declaration back off.
    pub toggleable: bool,
}

impl EffectActive {
    pub(crate) fn from_entity(def: &EntityDef) -> Option<Self> {
        let named = |key: &str| {
            def.keyvalues
                .get(key)
                .is_some_and(|value| !value.trim().is_empty())
        };
        match def.classname.as_str() {
            "env_beam"
                if def.spawnflags & 8 == 0 && named("LightningStart") && named("LightningEnd") =>
            {
                Some(Self {
                    active: def.spawnflags & 1 != 0,
                    toggleable: def.spawnflags & 2 != 0,
                })
            }
            "env_laser" if named("LaserTarget") => Some(Self {
                active: def.spawnflags & 1 != 0,
                toggleable: true,
            }),
            "env_spark" if def.spawnflags & 32 != 0 => Some(Self {
                active: def.spawnflags & 64 != 0,
                toggleable: true,
            }),
            _ => None,
        }
    }

    /// On and Off are idempotent; plain Use toggles eligible declarations.
    /// A non-toggleable beam can be enabled but cannot be disabled. Finite-life
    /// beam timing remains the existing visual bridge's approximation, not a
    /// new gameplay timer or original-build fidelity claim. TODO(black-box).
    pub(crate) fn activate(&mut self, use_type: TriggerUse) {
        match use_type {
            TriggerUse::On => self.active = true,
            TriggerUse::Off if self.toggleable => self.active = false,
            TriggerUse::Toggle => self.active = !self.toggleable || !self.active,
            TriggerUse::Off => {}
        }
    }
}

/// Mirrors the existing visual bridge's project defaults in authoritative
/// state, so live reads do not confuse an untouched default with an explicit
/// env_render copy of zero or black. No serialized shape changes.
pub(crate) fn initial_render_props(def: &EntityDef) -> RenderProps {
    let mut props = def.render;
    if matches!(
        def.classname.as_str(),
        "env_sprite" | "env_glow" | "cycler_sprite"
    ) && !def.keyvalues.contains_key("rendercolor")
    {
        props.color = [255; 3];
    }
    if matches!(def.classname.as_str(), "env_beam" | "env_laser")
        && !def.keyvalues.contains_key("renderamt")
    {
        props.amt = 255;
    }
    props
}

/// Compatibility for a save predating live visual properties. The old renderer
/// supplied display defaults outside EntitySnapshot, whose zero values are
/// therefore ambiguous. Only absent authored keys and unchanged old field values
/// receive that prior display default; explicit authored zero/black is preserved.
pub fn restore_legacy_visual_defaults(registry: &mut Registry, defs: &[EntityDef]) {
    for (&entity, def) in registry.entities.iter().zip(defs) {
        let initial = initial_render_props(def);
        if let Ok(mut live) = registry.world.get::<&mut RenderProps>(entity) {
            if !def.keyvalues.contains_key("rendercolor") && live.color == def.render.color {
                live.color = initial.color;
            }
            if !def.keyvalues.contains_key("renderamt") && live.amt == def.render.amt {
                live.amt = initial.amt;
            }
        }
    }
}

/// Resolves current registry properties, using cached values only if absent.
/// Renderer callers retain their own documented support for each mode/fx.
#[must_use]
pub fn effective_render_props(
    registry: &Registry,
    entity: Entity,
    fallback: RenderProps,
) -> (RenderProps, RenderFx) {
    (
        registry
            .world
            .get::<&RenderProps>(entity)
            .map_or(fallback, |props| *props),
        registry
            .world
            .get::<&RenderFx>(entity)
            .map_or(RenderFx::default(), |fx| *fx),
    )
}

/// Applies to every live named target in stable order, including entities
/// omitted from the registry's deliberately bounded generic name index.
pub(crate) fn apply_render_control(
    registry: &mut Registry,
    source: Entity,
    control: RenderControl,
) {
    let Ok(target) = registry.world.get::<&Target>(source) else {
        return;
    };
    let name = target.0.clone();
    drop(target);
    let (source_props, source_fx) =
        effective_render_props(registry, source, RenderProps::default());
    let mut targets: Vec<Entity> = registry
        .world
        .query::<(Entity, &TargetName)>()
        .iter()
        .filter(|(_, target)| target.0 == name)
        .map(|(entity, _)| entity)
        .collect();
    targets.sort_unstable_by_key(|entity| entity.to_bits());
    for entity in targets {
        let (mut props, _) = effective_render_props(registry, entity, RenderProps::default());
        if control.preserve & 2 == 0 {
            props.amt = source_props.amt;
        }
        if control.preserve & 4 == 0 {
            props.mode = source_props.mode;
        }
        if control.preserve & 8 == 0 {
            props.color = source_props.color;
        }
        registry.world.insert_one(entity, props).ok();
        if control.preserve & 1 == 0 {
            registry.world.insert_one(entity, source_fx).ok();
        }
    }
}

/// Additional break parameters, kept out of the frozen `Breakable` encoding.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BreakEffects {
    /// Optional secondary explosion; zero magnitude means no blast.
    pub blast: MapBlastProfile,
    /// `explosion = 1` permits attack-relative debris when a hit supplies it.
    pub attack_relative: bool,
    /// Sanitized relative model reference; missing models use primitive debris.
    #[cfg_attr(feature = "serde", serde(deserialize_with = "bounded_gib_model"))]
    pub gib_model: Option<String>,
}

impl BreakEffects {
    pub(crate) fn from_entity(def: &EntityDef) -> Self {
        Self {
            blast: MapBlastProfile::from_magnitude(number(def, "explodemagnitude", 0.0)),
            attack_relative: def
                .keyvalues
                .get("explosion")
                .is_some_and(|value| value.trim() == "1"),
            gib_model: def
                .keyvalues
                .get("gibmodel")
                .and_then(|value| sanitized_model(value)),
        }
    }
}

/// Host-provided player facts sampled before map use and touch dispatch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectPlayer {
    /// Real player identity, distinct from a relay or other activating entity.
    pub entity: Entity,
    /// Current unshaken world position.
    pub origin: Vec3,
    /// Ground contact at the trigger edge.
    pub grounded: bool,
}

/// Optional provenance supplied by the combat host to the single break edge.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BreakContext {
    /// Triggering/damaging actor; the source remains a separate inflictor.
    pub activator: Option<Entity>,
    /// World-space incoming attack direction; absent for trigger/pressure.
    pub attack_direction: Option<Vec3>,
}

/// Immutable material, geometry and parameters captured at the break edge.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BreakCommand {
    /// Placed brush half-extents; the command origin is its live center.
    pub half_extents: Vec3,
    /// Existing material identifier, including unknown values.
    pub material: u8,
    /// Normalized attack direction, only for attack-relative debris.
    pub attack_direction: Option<Vec3>,
    /// Blast and appearance parameters.
    pub effects: BreakEffects,
}

/// A resolved operation for the host's simulation-owned effect runtime.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MapEffect {
    /// Radial combat and independently masked presentation.
    Explosion(ExplosionDef),
    /// Trigger-time eligible shake; the host only handles waveform/timing.
    Shake(ShakeDef),
    /// Trigger-time eligible fade; the host only handles compositing/timing.
    Fade(FadeDef),
    /// The unique false-to-true brush break edge, including physical debris.
    Break(BreakCommand),
}

/// Host-bound command. `E` permits stable save references without serializing
/// raw runtime handles; the host maps source/activator at its save boundary.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MapEffectCommand<E = Entity> {
    /// Inflictor/source identity; may already be despawned when consumed.
    pub source: E,
    /// Triggering or damaging actor, distinct from the source.
    pub activator: Option<E>,
    /// Captured world origin, independent from the source's later lifetime.
    pub origin: Vec3,
    /// Captured parameters; no retail keyvalue lookup is needed at runtime.
    pub effect: MapEffect,
}

impl<E> MapEffectCommand<E> {
    /// Rejects malformed restored commands before they enter a live queue.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        if !self.origin.is_finite() {
            return false;
        }
        let bounded =
            |value: f32, maximum: f32| value.is_finite() && (0.0..=maximum).contains(&value);
        let profile = |blast: MapBlastProfile| {
            bounded(blast.damage, MAX_BLAST_MAGNITUDE)
                && bounded(blast.radius, 2.0 * MAX_BLAST_MAGNITUDE)
        };
        match &self.effect {
            MapEffect::Explosion(definition) => profile(definition.blast),
            MapEffect::Shake(definition) => {
                bounded(definition.amplitude, MAX_SHAKE_AMPLITUDE)
                    && bounded(definition.frequency, MAX_SHAKE_FREQUENCY)
                    && bounded(definition.duration, MAX_EFFECT_SECONDS)
                    && bounded(definition.radius, MAX_SHAKE_RADIUS)
            }
            MapEffect::Fade(definition) => {
                bounded(definition.duration, MAX_EFFECT_SECONDS)
                    && bounded(definition.hold, MAX_EFFECT_SECONDS)
            }
            MapEffect::Break(command) => {
                profile(command.effects.blast)
                    && command.half_extents.is_finite()
                    && command.half_extents.min_element() >= 0.0
                    && command.half_extents.max_element() <= MAX_BREAK_HALF_EXTENT
                    && command
                        .attack_direction
                        .is_none_or(|direction| direction.is_finite() && direction.is_normalized())
                    && command
                        .effects
                        .gib_model
                        .as_ref()
                        .is_none_or(|model| sanitized_model(model).as_ref() == Some(model))
            }
        }
    }

    /// Re-encodes identities while keeping the immutable operation intact.
    #[must_use]
    pub fn map_entities<T>(self, mut map: impl FnMut(E) -> T) -> MapEffectCommand<T> {
        MapEffectCommand {
            source: map(self.source),
            activator: self.activator.map(map),
            origin: self.origin,
            effect: self.effect,
        }
    }
}

fn number(def: &EntityDef, key: &str, default: f32) -> f32 {
    def.keyvalues.get(key).map_or(default, |value| {
        value
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
    })
}

fn finite_clamped(value: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, maximum)
    } else {
        0.0
    }
}

fn sanitized_model(value: &str) -> Option<String> {
    let value = value.trim().replace('\\', "/");
    (!value.is_empty()
        && value.len() <= 260
        && !value.contains(':')
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."))
    .then_some(value)
}

#[cfg(feature = "serde")]
fn bounded_gib_model<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    struct OptionalModel;
    struct Model;
    impl serde::de::Visitor<'_> for Model {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded relative gib model")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            if value.len() > 260 {
                return Err(E::custom("gib model capacity exceeded"));
            }
            match sanitized_model(value) {
                Some(model) if model == value => Ok(model),
                _ => Err(E::custom("invalid gib model reference")),
            }
        }
    }
    impl<'de> serde::de::Visitor<'de> for OptionalModel {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("optional bounded gib model")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_str(Model).map(Some)
        }
    }
    deserializer.deserialize_option(OptionalModel)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::Simulation;
    use crate::keyvalues::{Limits, parse_entities};
    use crate::registry::{Breakable, Transform};
    use ohl_formats::bsp30::Entity as RawEntity;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn fixture(entities: &[RawEntity]) -> (Registry, Simulation) {
        let bounds = BTreeMap::from([(1, ([10.0, -4.0, -2.0], [30.0, 4.0, 2.0]))]);
        let limits = Limits::default();
        (
            Registry::build(&parse_entities(entities, &limits), &bounds, &limits),
            Simulation::new(),
        )
    }

    fn activate(
        registry: &mut Registry,
        simulation: &mut Simulation,
        name: &str,
        activator: Option<Entity>,
    ) {
        let entity = registry.find(name)[0];
        simulation.use_entity(registry, entity, activator, &mut Vec::new());
    }

    #[test]
    fn relay_explosion_captures_origin_and_actor_and_consumes_before_retrigger() {
        let (mut registry, mut simulation) = fixture(&[
            raw(&[
                ("classname", "trigger_relay"),
                ("targetname", "switch"),
                ("target", "blast"),
            ]),
            raw(&[
                ("classname", "env_explosion"),
                ("targetname", "blast"),
                ("origin", "80 20 6"),
                ("iMagnitude", "45"),
            ]),
            raw(&[("classname", "info_target"), ("targetname", "actor")]),
        ]);
        let actor = registry.find("actor")[0];
        let blast = registry.find("blast")[0];
        activate(&mut registry, &mut simulation, "switch", Some(actor));
        simulation.tick(&mut registry, 0.01);
        activate(&mut registry, &mut simulation, "blast", Some(actor));
        let commands: Vec<_> = simulation.drain_effect_commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].source, blast);
        assert_eq!(commands[0].activator, Some(actor));
        assert_eq!(commands[0].origin, Vec3::new(80.0, 20.0, 6.0));
        let MapEffect::Explosion(definition) = commands[0].effect else {
            panic!("expected blast");
        };
        assert_eq!(
            definition.blast,
            MapBlastProfile {
                damage: 45.0,
                radius: 90.0
            }
        );
        assert!(!definition.no_damage);
        assert_eq!(definition.visuals, ExplosionVisuals::default());
        assert!(
            registry
                .world
                .get::<&ExplosionState>(blast)
                .unwrap()
                .consumed
        );
        registry.world.despawn(blast).unwrap();
        simulation.restore_effect_commands(commands.clone());
        assert_eq!(
            simulation.drain_effect_commands().collect::<Vec<_>>(),
            commands
        );
    }

    #[test]
    fn explosion_damage_repeatability_and_each_visual_mask_are_independent() {
        for flags in [0, 1, 2, 4, 8, 16, 32, 63] {
            let (mut registry, mut simulation) = fixture(&[raw(&[
                ("classname", "env_explosion"),
                ("targetname", "blast"),
                ("spawnflags", &flags.to_string()),
            ])]);
            activate(&mut registry, &mut simulation, "blast", None);
            activate(&mut registry, &mut simulation, "blast", None);
            let commands: Vec<_> = simulation.drain_effect_commands().collect();
            assert_eq!(commands.len(), if flags & 2 == 0 { 1 } else { 2 });
            let MapEffect::Explosion(definition) = commands[0].effect else {
                panic!("expected blast");
            };
            assert_eq!(definition.no_damage, flags & 1 != 0);
            assert_eq!(definition.visuals.fireball, flags & 4 == 0);
            assert_eq!(definition.visuals.smoke, flags & 8 == 0);
            assert_eq!(definition.visuals.decal, flags & 16 == 0);
            assert_eq!(definition.visuals.sparks, flags & 32 == 0);
            assert_eq!(definition.blast.damage, 100.0);
        }
    }

    #[test]
    fn shake_uses_trigger_time_ground_contact_and_binary_radius() {
        let (mut registry, mut simulation) = fixture(&[
            raw(&[
                ("classname", "env_shake"),
                ("targetname", "local"),
                ("amplitude", "7"),
                ("duration", "2"),
                ("frequency", "8"),
                ("radius", "100"),
            ]),
            raw(&[
                ("classname", "env_shake"),
                ("targetname", "global"),
                ("amplitude", "7"),
                ("duration", "2"),
                ("frequency", "8"),
                ("spawnflags", "1"),
            ]),
            raw(&[("classname", "info_target"), ("targetname", "actor")]),
        ]);
        let actor = registry.find("actor")[0];
        for distance in [1.0, 90.0, 101.0] {
            simulation.set_effect_player(Some(EffectPlayer {
                entity: actor,
                origin: Vec3::X * distance,
                grounded: true,
            }));
            activate(&mut registry, &mut simulation, "local", Some(actor));
        }
        let commands: Vec<_> = simulation.drain_effect_commands().collect();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].effect, commands[1].effect);
        for name in ["local", "global"] {
            simulation.set_effect_player(Some(EffectPlayer {
                entity: actor,
                origin: Vec3::ZERO,
                grounded: false,
            }));
            activate(&mut registry, &mut simulation, name, Some(actor));
        }
        assert!(simulation.effect_commands().is_empty());
        simulation.set_effect_player(Some(EffectPlayer {
            entity: actor,
            origin: Vec3::X * 1000.0,
            grounded: true,
        }));
        activate(&mut registry, &mut simulation, "global", None);
        assert_eq!(simulation.effect_commands().len(), 1);
        // A later airborne state cannot revoke already captured eligibility.
        simulation.set_effect_player(None);
        assert_eq!(simulation.effect_commands().len(), 1);
    }

    #[test]
    fn delayed_fade_requires_actual_player_activator_and_zero_duration_is_noop() {
        let (mut registry, mut simulation) = fixture(&[
            raw(&[
                ("classname", "trigger_relay"),
                ("targetname", "switch"),
                ("target", "fade"),
                ("delay", "0.2"),
            ]),
            raw(&[
                ("classname", "env_fade"),
                ("targetname", "fade"),
                ("duration", "2"),
                ("holdtime", "3"),
                ("renderamt", "170"),
                ("rendercolor", "20 40 60"),
                ("spawnflags", "7"),
            ]),
            raw(&[
                ("classname", "env_fade"),
                ("targetname", "zero"),
                ("duration", "0"),
                ("renderamt", "255"),
            ]),
            raw(&[("classname", "info_target"), ("targetname", "actor")]),
        ]);
        let actor = registry.find("actor")[0];
        let relay = registry.find("switch")[0];
        simulation.set_effect_player(Some(EffectPlayer {
            entity: actor,
            origin: Vec3::ZERO,
            grounded: true,
        }));
        for activator in [None, Some(relay), Some(actor)] {
            activate(&mut registry, &mut simulation, "switch", activator);
            simulation.tick(&mut registry, 0.25);
        }
        activate(&mut registry, &mut simulation, "zero", Some(actor));
        let commands: Vec<_> = simulation.drain_effect_commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].activator, Some(actor));
        let MapEffect::Fade(definition) = commands[0].effect else {
            panic!("expected fade");
        };
        assert_eq!(definition.duration, 2.0);
        assert_eq!(definition.hold, 3.0);
        assert_eq!(definition.amount, 170);
        assert_eq!(definition.color, [20, 40, 60]);
        assert!(definition.reverse && definition.modulate && definition.activator_only);
        simulation.restore_effect_commands(commands);
        assert!(
            simulation.effect_commands().is_empty(),
            "restoring cannot replay a fade"
        );
    }

    #[test]
    fn render_masks_update_every_live_named_target_and_preserve_unrelated_state() {
        for flags in [0, 1, 2, 4, 8, 15] {
            let mut entities = vec![raw(&[
                ("classname", "env_render"),
                ("targetname", "controller"),
                ("target", "surface"),
                ("renderfx", "17"),
                ("rendermode", "5"),
                ("renderamt", "120"),
                ("rendercolor", "15 30 45"),
                ("spawnflags", &flags.to_string()),
            ])];
            for _ in 0..70 {
                entities.push(raw(&[
                    ("classname", "env_sprite"),
                    ("targetname", "surface"),
                    ("renderfx", "987"),
                    ("rendermode", "2"),
                    ("renderamt", "70"),
                    ("rendercolor", "60 80 100"),
                    ("origin", "12 24 48"),
                ]));
            }
            let (mut registry, mut simulation) = fixture(&entities);
            let removed = registry.entities[20];
            registry.world.despawn(removed).unwrap();
            activate(&mut registry, &mut simulation, "controller", None);
            for &entity in registry
                .entities
                .iter()
                .skip(1)
                .filter(|&&entity| entity != removed)
            {
                let (props, fx) = effective_render_props(&registry, entity, RenderProps::default());
                assert_eq!(props.mode, if flags & 4 == 0 { 5 } else { 2 });
                assert_eq!(props.amt, if flags & 2 == 0 { 120 } else { 70 });
                assert_eq!(
                    props.color,
                    if flags & 8 == 0 {
                        [15, 30, 45]
                    } else {
                        [60, 80, 100]
                    }
                );
                assert_eq!(fx.0, if flags & 1 == 0 { 17 } else { 987 });
                assert_eq!(
                    registry.world.get::<&Transform>(entity).unwrap().origin,
                    Vec3::new(12.0, 24.0, 48.0)
                );
            }
            assert!(simulation.effect_commands().is_empty());
        }
    }

    #[test]
    fn break_emits_once_from_placed_brush_center_with_direction_only_on_damage() {
        for triggered in [false, true] {
            let (mut registry, mut simulation) = fixture(&[
                raw(&[
                    ("classname", "func_breakable"),
                    ("targetname", "crate"),
                    ("target", "fade"),
                    ("delay", "0.3"),
                    ("model", "*1"),
                    ("origin", "100 200 300"),
                    ("health", "20"),
                    ("explosion", "1"),
                    ("explodemagnitude", "50"),
                    ("material", "2"),
                    ("gibmodel", "models/synthetic.mdl"),
                ]),
                raw(&[
                    ("classname", "env_fade"),
                    ("targetname", "fade"),
                    ("duration", "1"),
                    ("spawnflags", "4"),
                ]),
                raw(&[("classname", "info_target"), ("targetname", "actor")]),
            ]);
            let entity = registry.find("crate")[0];
            let actor = registry.find("actor")[0];
            simulation.set_effect_player(Some(EffectPlayer {
                entity: actor,
                origin: Vec3::ZERO,
                grounded: true,
            }));
            if triggered {
                activate(&mut registry, &mut simulation, "crate", Some(actor));
            } else {
                assert!(simulation.damage_breakable_with_context(
                    &mut registry,
                    entity,
                    20.0,
                    false,
                    BreakContext {
                        activator: Some(actor),
                        attack_direction: Some(Vec3::new(2.0, 0.0, 0.0))
                    }
                ));
            }
            assert!(!simulation.break_entity(&mut registry, entity));
            assert!(registry.world.get::<&Breakable>(entity).unwrap().broken);
            let commands: Vec<_> = simulation.drain_effect_commands().collect();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].origin, Vec3::new(120.0, 200.0, 300.0));
            assert_eq!(commands[0].activator, Some(actor));
            let MapEffect::Break(command) = &commands[0].effect else {
                panic!("expected break");
            };
            assert_eq!(command.half_extents, Vec3::new(10.0, 4.0, 2.0));
            assert_eq!(command.material, 2);
            assert_eq!(
                command.effects.blast,
                MapBlastProfile {
                    damage: 50.0,
                    radius: 100.0
                }
            );
            assert_eq!(
                command.effects.gib_model.as_deref(),
                Some("models/synthetic.mdl")
            );
            assert_eq!(
                command.attack_direction,
                if triggered { None } else { Some(Vec3::X) }
            );
            simulation.tick(&mut registry, 0.1);
            assert!(simulation.effect_commands().is_empty());
            simulation.tick(&mut registry, 0.3);
            assert!(
                matches!(simulation.effect_commands(), [MapEffectCommand { activator: Some(found), effect: MapEffect::Fade(_), .. }] if *found == actor)
            );
        }
    }

    #[test]
    fn touch_crowbar_and_trigger_only_keep_existing_break_rules() {
        let (mut registry, mut simulation) = fixture(&[
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "touch"),
                ("model", "*1"),
                ("spawnflags", "2"),
            ]),
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "crowbar"),
                ("health", "200"),
                ("spawnflags", "256"),
            ]),
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "only"),
                ("health", "10"),
                ("spawnflags", "1"),
            ]),
        ]);
        let crowbar = registry.find("crowbar")[0];
        let only = registry.find("only")[0];
        simulation.touch_breakables(
            &mut registry,
            Vec3::new(15.0, -1.0, -1.0),
            Vec3::new(16.0, 1.0, 1.0),
        );
        assert!(simulation.damage_breakable(&mut registry, crowbar, 1.0, true));
        assert!(!simulation.damage_breakable(&mut registry, only, 100.0, true));
        assert_eq!(simulation.effect_commands().len(), 2);
        activate(&mut registry, &mut simulation, "only", None);
        simulation.touch_breakables(
            &mut registry,
            Vec3::new(15.0, -1.0, -1.0),
            Vec3::new(16.0, 1.0, 1.0),
        );
        assert_eq!(simulation.effect_commands().len(), 3);
    }

    #[test]
    fn malformed_parameters_and_untrusted_restore_are_bounded() {
        for magnitude in ["-7", "NaN", "inf", "not-a-number"] {
            let (mut registry, mut simulation) = fixture(&[raw(&[
                ("classname", "env_explosion"),
                ("targetname", "blast"),
                ("iMagnitude", magnitude),
            ])]);
            activate(&mut registry, &mut simulation, "blast", None);
            let MapEffect::Explosion(definition) = simulation.effect_commands()[0].effect else {
                panic!("expected blast");
            };
            assert_eq!(definition.blast, MapBlastProfile::default());
        }
        let (mut registry, mut simulation) = fixture(&[raw(&[
            ("classname", "env_explosion"),
            ("targetname", "blast"),
            ("iMagnitude", "1e30"),
            ("spawnflags", "2"),
        ])]);
        for _ in 0..=MAX_PENDING_EFFECTS {
            activate(&mut registry, &mut simulation, "blast", None);
        }
        assert_eq!(simulation.effect_commands().len(), MAX_PENDING_EFFECTS);
        let command = simulation.effect_commands()[0].clone();
        let MapEffect::Explosion(definition) = command.effect else {
            panic!("expected blast");
        };
        assert_eq!(definition.blast.damage, MAX_BLAST_MAGNITUDE);
        simulation.restore_effect_commands(std::iter::repeat_n(
            command.clone(),
            MAX_PENDING_EFFECTS + 1,
        ));
        assert_eq!(simulation.effect_commands().len(), MAX_PENDING_EFFECTS);
        let mut invalid = command;
        invalid.origin.x = f32::NAN;
        simulation.restore_effect_commands([invalid]);
        assert!(simulation.effect_commands().is_empty());
        for path in [
            "../escape.mdl",
            "/absolute.mdl",
            "C:\\absolute.mdl",
            "models/../escape.mdl",
        ] {
            let (registry, _) =
                fixture(&[raw(&[("classname", "func_breakable"), ("gibmodel", path)])]);
            assert!(
                registry
                    .world
                    .get::<&BreakEffects>(registry.entities[0])
                    .unwrap()
                    .gib_model
                    .is_none()
            );
        }
    }

    #[test]
    fn supported_visuals_follow_real_use_and_idempotent_relay_on_off() {
        for (classname, off_flags, on_flags) in [
            ("env_beam", "2", "3"),
            ("env_laser", "0", "1"),
            ("env_spark", "32", "96"),
        ] {
            for initially_on in [false, true] {
                let (mut registry, mut simulation) = fixture(&[
                    raw(&[
                        ("classname", classname),
                        ("targetname", "effect"),
                        (
                            "spawnflags",
                            if initially_on { on_flags } else { off_flags },
                        ),
                        ("LightningStart", "start"),
                        ("LightningEnd", "end"),
                        ("LaserTarget", "end"),
                    ]),
                    raw(&[("classname", "info_target"), ("targetname", "start")]),
                    raw(&[("classname", "info_target"), ("targetname", "end")]),
                    raw(&[
                        ("classname", "trigger_relay"),
                        ("targetname", "on"),
                        ("target", "effect"),
                        ("triggerstate", "1"),
                        ("wait", "0"),
                    ]),
                    raw(&[
                        ("classname", "trigger_relay"),
                        ("targetname", "off"),
                        ("target", "effect"),
                        ("triggerstate", "0"),
                        ("wait", "0"),
                    ]),
                ]);
                let effect = registry.find("effect")[0];
                let enabled = |registry: &Registry| {
                    registry.world.get::<&EffectActive>(effect).unwrap().active
                };
                assert_eq!(enabled(&registry), initially_on);
                activate(&mut registry, &mut simulation, "effect", None);
                assert_eq!(enabled(&registry), !initially_on);
                activate(&mut registry, &mut simulation, "effect", None);
                assert_eq!(enabled(&registry), initially_on);
                for _ in 0..2 {
                    activate(&mut registry, &mut simulation, "on", None);
                    simulation.tick(&mut registry, 0.01);
                    assert!(enabled(&registry));
                }
                for _ in 0..2 {
                    activate(&mut registry, &mut simulation, "off", None);
                    simulation.tick(&mut registry, 0.01);
                    assert!(!enabled(&registry));
                }
                assert!(
                    simulation.effect_commands().is_empty(),
                    "visual activation must not request damage"
                );
            }
        }
    }

    #[test]
    fn beam_without_toggle_latches_and_unsupported_visuals_remain_outside_bridge() {
        let (mut registry, mut simulation) = fixture(&[
            raw(&[
                ("classname", "env_beam"),
                ("targetname", "beam"),
                ("LightningStart", "start"),
                ("LightningEnd", "end"),
            ]),
            raw(&[
                ("classname", "trigger_relay"),
                ("targetname", "off"),
                ("target", "beam"),
                ("triggerstate", "0"),
                ("wait", "0"),
            ]),
            raw(&[("classname", "info_target"), ("targetname", "start")]),
            raw(&[("classname", "info_target"), ("targetname", "end")]),
            raw(&[
                ("classname", "env_beam"),
                ("targetname", "ring"),
                ("LightningStart", "start"),
                ("LightningEnd", "end"),
                ("spawnflags", "11"),
            ]),
            raw(&[
                ("classname", "env_beam"),
                ("targetname", "random"),
                ("spawnflags", "3"),
            ]),
            raw(&[
                ("classname", "env_spark"),
                ("targetname", "single_spark"),
                ("spawnflags", "64"),
            ]),
            raw(&[
                ("classname", "env_sprite"),
                ("targetname", "sprite"),
                ("spawnflags", "1"),
            ]),
        ]);
        let beam = registry.find("beam")[0];
        let enabled =
            |registry: &Registry| registry.world.get::<&EffectActive>(beam).unwrap().active;
        activate(&mut registry, &mut simulation, "off", None);
        simulation.tick(&mut registry, 0.01);
        assert!(!enabled(&registry));
        for _ in 0..2 {
            activate(&mut registry, &mut simulation, "beam", None);
            assert!(enabled(&registry));
        }
        activate(&mut registry, &mut simulation, "off", None);
        simulation.tick(&mut registry, 0.01);
        assert!(enabled(&registry));
        for name in ["ring", "random", "single_spark", "sprite"] {
            let entity = registry.find(name)[0];
            activate(&mut registry, &mut simulation, name, None);
            assert!(registry.world.get::<&EffectActive>(entity).is_err());
        }
        assert!(simulation.effect_commands().is_empty());
    }
}
