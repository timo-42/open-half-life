//! Visual-only map effect bridge, owned by rendering rather than gameplay.
//!
//! Supports named-endpoint straight beams, clipped lasers and recurring Toggle
//! sparks. Activation and render properties are read from authoritative game
//! components, including initially inactive declarations. No damage is inferred
//! here. Geometry, spark motion/timing and mark dimensions are project-authored
//! placeholders, TODO(black-box). See `docs/FORMAT_SOURCES.md`.

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::keyvalues::EntityDef;
use ohl_game::registry::{BrushModel, Transform};
use ohl_physics::Hull;
use ohl_render::{EffectInstance, MAX_EFFECT_INSTANCES};

use crate::level::Level;

const MAX_MAP_EFFECTS: usize = 512;
const SPARK_LIFETIME: f32 = 0.35;

enum MapEffect {
    Beam {
        emitter: Entity,
        start: Entity,
        end: Entity,
        width: f32,
        render: ohl_game::keyvalues::RenderProps,
        laser: bool,
        flags: u32,
        life: f32,
        strike_delay: f32,
    },
    Spark {
        emitter: Entity,
        delay: f32,
    },
}

/// Cached effect declarations. Sampling reads live transforms and liveness;
/// it never writes gameplay state and never depends on render frequency.
pub(super) struct MapEffects {
    entries: Vec<MapEffect>,
    pub(super) instances: Vec<EffectInstance>,
}

fn number(def: &EntityDef, key: &str, default: f32) -> f32 {
    def.keyvalues
        .get(key)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn named(level: &Level, def: &EntityDef, key: &str) -> Option<Entity> {
    let name = def.keyvalues.get(key)?;
    // First definition is a deterministic project choice. Documented
    // randomized selection among duplicate names remains TODO(black-box).
    let index = level
        .defs
        .iter()
        .position(|target| target.targetname.as_ref() == Some(name))?;
    level.registry.entities.get(index).copied()
}

fn origin(level: &Level, entity: Entity) -> Option<Vec3> {
    let authored = level.registry.world.get::<&Transform>(entity).ok()?.origin;
    // Brush transforms keep their authored origin while mover state carries
    // displacement/rotation. Use the same posed center as rendering/collision.
    let live = if level.registry.world.get::<&BrushModel>(entity).is_ok() {
        ohl_game::pose::brush_center(&level.registry, entity)
            .unwrap_or_else(|| authored + ohl_game::pose::brush_offset(&level.registry, entity))
    } else {
        authored
    };
    live.is_finite().then_some(live)
}

impl MapEffects {
    pub(super) fn new(level: &Level) -> Self {
        let mut entries = Vec::new();
        for (index, def) in level.defs.iter().enumerate() {
            if entries.len() >= MAX_MAP_EFFECTS {
                break;
            }
            let Some(&emitter) = level.registry.entities.get(index) else {
                continue;
            };
            match def.classname.as_str() {
                "env_beam" | "env_laser" => {
                    let laser = def.classname == "env_laser";
                    // Random-endpoint and ring beams have no straight fallback.
                    if !laser && def.spawnflags & 8 != 0 {
                        continue;
                    }
                    let start = if laser {
                        Some(emitter)
                    } else {
                        named(level, def, "LightningStart")
                    };
                    let end = named(
                        level,
                        def,
                        if laser { "LaserTarget" } else { "LightningEnd" },
                    );
                    let (Some(start), Some(end)) = (start, end) else {
                        continue;
                    };
                    let width = if laser {
                        number(def, "width", number(def, "BoltWidth", 4.0))
                    } else {
                        number(def, "BoltWidth", 4.0)
                    };
                    entries.push(MapEffect::Beam {
                        emitter,
                        start,
                        end,
                        // Public pages disagree on beam width units. A direct
                        // world-unit width is a project placeholder.
                        width: width.clamp(0.0, 4096.0),
                        render: def.render,
                        laser,
                        flags: def.spawnflags,
                        life: if laser {
                            0.0
                        } else {
                            number(def, "life", 0.0).max(0.0)
                        },
                        strike_delay: number(def, "StrikeTime", 1.0).max(0.0),
                    });
                }
                "env_spark" if def.spawnflags & 32 != 0 => {
                    entries.push(MapEffect::Spark {
                        emitter,
                        delay: number(def, "MaxDelay", 1.0).clamp(SPARK_LIFETIME, 60.0),
                    });
                }
                _ => {}
            }
        }
        Self {
            entries,
            instances: Vec::new(),
        }
    }

    /// Render actual gameplay debris records and independent blast channels.
    /// Shapes and motion are bounded project placeholders, TODO(black-box).
    pub(super) fn append_gameplay(
        &mut self,
        level: &Level,
        effects: &crate::map_effects::EffectPresentation<'_>,
        custom_ready: &std::collections::BTreeSet<u64>,
    ) {
        for debris in effects.debris {
            if custom_ready.contains(&debris.id) {
                continue;
            }
            self.instances.push(EffectInstance::Gib {
                transform: debris.transform(),
                half_extents: debris.half_extents.to_array(),
                color: debris.color(),
            });
        }
        for blast in effects.blasts {
            let progress = (blast.age / 0.75).clamp(0.0, 1.0);
            let size = (blast.radius * 0.2).clamp(2.0, 64.0);
            if blast.channels.fireball {
                self.instances.push(EffectInstance::Particle {
                    origin: blast.origin.to_array(),
                    size: size * (1.0 + progress),
                    color: [1.0, 0.3, 0.03, 1.0 - progress],
                });
            }
            if blast.channels.smoke {
                self.instances.push(EffectInstance::Particle {
                    origin: (blast.origin + Vec3::Z * (blast.age * 24.0)).to_array(),
                    size: size * (1.0 + progress * 2.0),
                    color: [0.16, 0.16, 0.16, (1.0 - progress) * 0.5],
                });
            }
            if blast.channels.sparks {
                sparks(&mut self.instances, blast.origin, blast.age, 1.0);
            }
            if blast.channels.decal
                && let Some(collision) = &level.collision
            {
                let trace = collision.trace(
                    Hull::Point,
                    blast.origin,
                    blast.origin - Vec3::Z * size * 2.0,
                );
                if trace.fraction < 1.0 && !trace.start_solid && !trace.all_solid {
                    self.instances.push(EffectInstance::Decal {
                        origin: trace.end_pos.to_array(),
                        normal: trace.plane_normal.to_array(),
                        radius: size,
                        color: [0.03, 0.02, 0.01, 1.0 - progress],
                    });
                }
            }
        }
        self.instances.truncate(MAX_EFFECT_INSTANCES);
    }

    /// Borrow captured, simulation-aged turret laser shots. No geometry query,
    /// time advancement, entity activation or damage is performed by drawing.
    pub(super) fn append_tank_pulses(&mut self, pulses: &[crate::tanks::LaserPulse]) {
        let available = MAX_EFFECT_INSTANCES.saturating_sub(self.instances.len());
        self.instances.extend(
            pulses
                .iter()
                .take(available)
                .map(|pulse| EffectInstance::Beam {
                    start: pulse.start.to_array(),
                    end: pulse.end.to_array(),
                    width: pulse.width,
                    color: pulse.color,
                }),
        );
    }

    pub(super) fn sample(&mut self, level: &Level, elapsed: f32) {
        self.instances.clear();
        if !elapsed.is_finite() || elapsed < 0.0 {
            return;
        }
        for entry in &self.entries {
            match *entry {
                MapEffect::Beam {
                    emitter,
                    start,
                    end,
                    width,
                    render,
                    laser,
                    flags,
                    life,
                    strike_delay,
                } => {
                    if !active(level, emitter) {
                        continue;
                    }
                    let (render, _fx) =
                        ohl_game::effects::effective_render_props(&level.registry, emitter, render);
                    // This bridge retains its supported additive style. Other
                    // render modes/fx are preserved state, not new animations.
                    let color = [
                        f32::from(render.color[0]) / 255.0,
                        f32::from(render.color[1]) / 255.0,
                        f32::from(render.color[2]) / 255.0,
                        f32::from(u8::try_from(render.amt.clamp(0, 255)).unwrap_or(0)) / 255.0,
                    ];
                    if life > 0.0 && elapsed.rem_euclid(life + strike_delay) >= life {
                        continue;
                    }
                    let (Some(start), Some(mut end)) = (origin(level, start), origin(level, end))
                    else {
                        continue;
                    };
                    if laser {
                        let Some(collision) = level.collision.as_ref() else {
                            continue;
                        };
                        let trace = collision.trace(Hull::Point, start, end);
                        if trace.start_solid || trace.all_solid {
                            continue;
                        }
                        end = trace.end_pos;
                        if flags & 64 != 0 && trace.fraction < 1.0 {
                            self.instances.push(EffectInstance::Decal {
                                origin: end.to_array(),
                                normal: trace.plane_normal.to_array(),
                                radius: 2.0,
                                color: [0.06, 0.04, 0.03, 0.85],
                            });
                        }
                    }
                    self.instances.push(EffectInstance::Beam {
                        start: start.to_array(),
                        end: end.to_array(),
                        width,
                        color,
                    });
                    if flags & 16 != 0 {
                        sparks(&mut self.instances, start, elapsed, 1.0);
                    }
                    if flags & 32 != 0 {
                        sparks(&mut self.instances, end, elapsed, 1.0);
                    }
                }
                MapEffect::Spark { emitter, delay } => {
                    if !active(level, emitter) {
                        continue;
                    }
                    if let Some(origin) = origin(level, emitter) {
                        sparks(&mut self.instances, origin, elapsed, delay);
                    }
                }
            }
        }
        self.instances.truncate(MAX_EFFECT_INSTANCES);
    }
}

fn active(level: &Level, entity: Entity) -> bool {
    level
        .registry
        .world
        .get::<&ohl_game::effects::EffectActive>(entity)
        .is_ok_and(|state| state.active)
}

fn sparks(out: &mut Vec<EffectInstance>, origin: Vec3, elapsed: f32, delay: f32) {
    let age = elapsed.rem_euclid(delay);
    if age >= SPARK_LIFETIME {
        return;
    }
    // Fixed rays and analytical ballistic motion avoid consuming simulation
    // randomness in a draw call. All constants here are project-authored.
    for index in 0..8_u16 {
        let angle = f32::from(index) * std::f32::consts::TAU / 8.0;
        let velocity = Vec3::new(
            angle.cos() * 28.0,
            angle.sin() * 28.0,
            20.0 + f32::from(index % 3) * 8.0,
        );
        let position = origin + velocity * age - Vec3::Z * (64.0 * age * age);
        out.push(EffectInstance::Particle {
            origin: position.to_array(),
            size: 1.25,
            color: [1.0, 0.55, 0.08, 1.0 - age / SPARK_LIFETIME],
        });
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::{MemoryAssets, test_support::synthetic_map_bsp_with_extra_entity};

    fn level(extra: &str) -> Level {
        Level::from_bytes(
            &MemoryAssets::new(),
            "ohlsynth",
            &synthetic_map_bsp_with_extra_entity("next", extra),
        )
        .unwrap()
    }

    const ENDPOINTS: &str = "{\"classname\" \"info_target\" \"targetname\" \"ohl_a\" \"origin\" \"20 -10 40\"}\n{\"classname\" \"info_target\" \"targetname\" \"ohl_b\" \"origin\" \"20 10 40\"}\n";

    #[test]
    fn initially_off_cache_and_live_render_properties_follow_real_use_and_save() {
        let extra = format!(
            "{ENDPOINTS}{{\"classname\" \"env_beam\" \"targetname\" \"beam\" \"spawnflags\" \"2\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\" \"rendercolor\" \"255 255 255\"}}\n{{\"classname\" \"env_render\" \"targetname\" \"color\" \"target\" \"beam\" \"rendercolor\" \"0 255 32\" \"renderamt\" \"64\" \"renderfx\" \"17\"}}"
        );
        let mut level = level(&extra);
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.1);
        assert!(effects.instances.is_empty());
        let beam = level.registry.find("beam")[0];
        let controller = level.registry.find("color")[0];
        level.simulation.use_entity(
            &mut level.registry,
            beam,
            Some(level.player),
            &mut Vec::new(),
        );
        effects.sample(&level, 0.1);
        assert_eq!(effects.instances.len(), 1);
        level.simulation.use_entity(
            &mut level.registry,
            controller,
            Some(level.player),
            &mut Vec::new(),
        );
        effects.sample(&level, 0.1);
        let EffectInstance::Beam { color, .. } = effects.instances[0] else {
            panic!("beam expected");
        };
        assert_eq!(color, [0.0, 1.0, 32.0 / 255.0, 64.0 / 255.0]);
        let runtime = crate::map_effects::MapEffectsRuntime::default();
        let saved = runtime.snapshot(&level.registry, level.player, &level.simulation);
        assert!(saved.active_overrides.iter().any(|&(_, active)| active));
        assert!(saved.render_fx.iter().any(|&(_, fx)| fx == 17));
        level.simulation.use_entity(
            &mut level.registry,
            beam,
            Some(level.player),
            &mut Vec::new(),
        );
        effects.sample(&level, 0.1);
        assert!(effects.instances.is_empty());
    }

    #[test]
    fn beam_samples_live_endpoints_and_disappears_when_emitter_is_removed() {
        let extra = format!(
            "{ENDPOINTS}{{\"classname\" \"env_beam\" \"spawnflags\" \"1\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\" \"BoltWidth\" \"3\" \"rendercolor\" \"255 64 0\" \"renderamt\" \"128\"}}"
        );
        let mut level = level(&extra);
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.0);
        assert_eq!(effects.instances.len(), 1);
        let EffectInstance::Beam {
            start,
            end,
            width,
            color,
        } = effects.instances[0]
        else {
            panic!("beam expected")
        };
        assert_eq!(start, [20.0, -10.0, 40.0]);
        assert_eq!(end, [20.0, 10.0, 40.0]);
        assert_eq!(width, 3.0);
        assert_eq!(color, [1.0, 64.0 / 255.0, 0.0, 128.0 / 255.0]);
        let MapEffect::Beam { emitter, end, .. } = effects.entries[0] else {
            panic!("beam expected")
        };
        level
            .registry
            .world
            .get::<&mut Transform>(end)
            .unwrap()
            .origin
            .y = 30.0;
        effects.sample(&level, 0.1);
        assert!(matches!(
            effects.instances[0],
            EffectInstance::Beam {
                end: [20.0, 30.0, 40.0],
                ..
            }
        ));
        level.registry.world.despawn(emitter).unwrap();
        effects.sample(&level, 0.2);
        assert!(effects.instances.is_empty());
    }

    #[test]
    fn beam_endpoint_follows_a_real_train_state_while_its_transform_stays_authored() {
        let mut level = level(
            "{\"classname\" \"func_train\" \"targetname\" \"ohl_train\" \"model\" \"*1\" \"target\" \"ohl_node1\" \"speed\" \"40\"}\n{\"classname\" \"path_corner\" \"targetname\" \"ohl_node1\" \"target\" \"ohl_node2\" \"origin\" \"100 0 0\"}\n{\"classname\" \"path_corner\" \"targetname\" \"ohl_node2\" \"origin\" \"200 0 0\"}\n{\"classname\" \"info_target\" \"targetname\" \"ohl_start\" \"origin\" \"-100 -8 48\"}\n{\"classname\" \"env_beam\" \"spawnflags\" \"1\" \"LightningStart\" \"ohl_start\" \"LightningEnd\" \"ohl_train\"}",
        );
        let train = level.registry.find("ohl_train")[0];
        let authored = level
            .registry
            .world
            .get::<&Transform>(train)
            .unwrap()
            .origin;
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.0);
        let EffectInstance::Beam { end: initial, .. } = effects.instances[0] else {
            panic!("beam expected")
        };
        assert_eq!(initial, [0.0, 24.0, 48.0]);
        level
            .simulation
            .use_entity(&mut level.registry, train, None, &mut Vec::new());
        for _ in 0..30 {
            level.simulation.tick(&mut level.registry, 1.0 / 60.0);
        }
        effects.sample(&level, 0.5);
        let EffectInstance::Beam { end, .. } = effects.instances[0] else {
            panic!("beam expected")
        };
        assert!((end[0] - initial[0] - 20.0).abs() < 1e-3);
        assert_eq!(end[1..], initial[1..]);
        assert_eq!(
            level
                .registry
                .world
                .get::<&Transform>(train)
                .unwrap()
                .origin,
            authored
        );
        assert!(
            Vec3::from_array(end)
                .distance(ohl_game::pose::brush_center(&level.registry, train).unwrap())
                < 1e-3
        );
    }

    #[test]
    fn inactive_unresolved_and_ring_effects_have_no_invented_fallback() {
        let extra = format!(
            "{ENDPOINTS}{{\"classname\" \"env_beam\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\"}}\n{{\"classname\" \"env_laser\" \"spawnflags\" \"1\" \"LaserTarget\" \"ohl_missing\"}}\n{{\"classname\" \"env_beam\" \"spawnflags\" \"9\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\"}}\n{{\"classname\" \"env_spark\" \"spawnflags\" \"64\"}}"
        );
        let level = level(&extra);
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.0);
        assert!(effects.instances.is_empty());
    }

    #[test]
    fn finite_beam_life_has_a_repeatable_strike_gap() {
        let extra = format!(
            "{ENDPOINTS}{{\"classname\" \"env_beam\" \"spawnflags\" \"3\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\" \"life\" \"0.5\" \"StrikeTime\" \"1\"}}"
        );
        let level = level(&extra);
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.4);
        assert_eq!(effects.instances.len(), 1);
        effects.sample(&level, 0.5);
        assert!(effects.instances.is_empty());
        effects.sample(&level, 1.5);
        assert_eq!(effects.instances.len(), 1);
    }

    #[test]
    fn sparks_move_fade_expire_and_repeat_without_render_side_effects() {
        let level = level(
            "{\"classname\" \"env_spark\" \"spawnflags\" \"96\" \"origin\" \"20 0 40\" \"MaxDelay\" \"1\"}",
        );
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.1);
        let first = effects.instances.clone();
        assert_eq!(first.len(), 8);
        assert!(
            matches!(first[0], EffectInstance::Particle { origin, color, .. } if origin != [20.0, 0.0, 40.0] && color[3] < 1.0)
        );
        effects.sample(&level, 0.1);
        assert_eq!(effects.instances, first);
        effects.sample(&level, 0.4);
        assert!(effects.instances.is_empty());
        effects.sample(&level, 1.0);
        assert_eq!(effects.instances.len(), 8);
        effects.sample(&level, f32::NAN);
        assert!(effects.instances.is_empty());
    }

    #[test]
    fn laser_stops_on_collision_and_draws_a_surface_mark() {
        let level = level(
            "{\"classname\" \"info_target\" \"targetname\" \"ohl_target\" \"origin\" \"0 0 -40\"}\n{\"classname\" \"env_laser\" \"spawnflags\" \"65\" \"origin\" \"0 0 40\" \"LaserTarget\" \"ohl_target\" \"width\" \"3\"}",
        );
        let mut effects = MapEffects::new(&level);
        effects.sample(&level, 0.0);
        assert_eq!(effects.instances.len(), 2);
        assert!(matches!(
            effects.instances[0],
            EffectInstance::Decal {
                normal: [0.0, 0.0, 1.0],
                ..
            }
        ));
        assert!(
            matches!(effects.instances[1], EffectInstance::Beam { end, width: 3.0, .. } if end[2] > -40.0)
        );
    }

    #[test]
    fn actual_turret_input_produces_a_captured_visible_beam_without_render_damage() {
        let (mut game, _) = crate::tanks::live_tests::fixture("func_tanklaser", &[], "");
        crate::tanks::live_tests::tick(&mut game, true, true);
        let pulses = game.systems_mut().tanks.laser_pulses().to_vec();
        assert_eq!(pulses.len(), 1);
        let victim = game.registry().find("victim")[0];
        let before = game
            .registry()
            .world
            .get::<&ohl_ai::Actor>(victim)
            .unwrap()
            .health;
        let (level, _) = game.level_and_systems_mut();
        let mut effects = MapEffects::new(level);
        effects.sample(level, 0.0);
        effects.append_tank_pulses(&pulses);
        let beam = effects
            .instances
            .iter()
            .find_map(|instance| match instance {
                EffectInstance::Beam {
                    start,
                    end,
                    width,
                    color,
                } => Some((*start, *end, *width, *color)),
                _ => None,
            })
            .expect("accepted turret laser must reach the existing beam renderer");
        assert!(Vec3::from_array(beam.0).abs_diff_eq(Vec3::new(0.0, 0.0, 64.0), 0.001));
        assert!(beam.1[0] > 100.0 && beam.1[0] < 170.0);
        assert!((beam.2 - 3.0).abs() < 0.001);
        assert!((beam.3[0] - 64.0 / 255.0).abs() < 0.001);
        assert!((beam.3[3] - 96.0 / 255.0).abs() < 0.001);
        effects.append_tank_pulses(&pulses);
        let after = level
            .registry
            .world
            .get::<&ohl_ai::Actor>(victim)
            .unwrap()
            .health;
        assert!((before - 977.0).abs() < 0.001);
        assert!((after - before).abs() < 0.001);
    }
}
