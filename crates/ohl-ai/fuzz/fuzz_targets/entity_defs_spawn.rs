//! `cargo fuzz` target for the monster-spawn path over an
//! `Arbitrary`-derived list of map-entity keyvalue pairs.
//!
//! Builds an [`ohl_game::Registry`] from those entities, spawns monsters
//! from them through the public [`ohl_ai::attach_monsters`] path with a
//! dummy always-spawn rule, attaches an [`ohl_ai::AiWorld`], and ticks it
//! ten times with no collision model and no navigator configured. Must
//! never panic on any input, however malformed the keyvalues or however
//! many entities the input describes.

#![no_main]

use std::collections::BTreeMap;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ohl_ai::state::Classification;
use ohl_ai::{AiWorld, DefaultBrain, MonsterSpawn, SightContext, attach_monsters};
use ohl_game::keyvalues::{Limits, RenderProps};
use ohl_game::{EntityDef, Registry};

/// Most entities one fuzz input builds; bounded so one input cannot spend
/// its whole time budget only growing the registry.
const MAX_ENTITIES: usize = 64;
/// Most keyvalue pairs kept per entity.
const MAX_KEYS: usize = 16;
/// Most bytes kept from one keyvalue's arbitrary key or value string.
const MAX_STRING_BYTES: usize = 64;
/// Simulation ticks run after spawning.
const TICKS: usize = 10;
const DT: f32 = 0.03;

#[derive(Debug, Arbitrary)]
struct FuzzKeyvalue {
    key: String,
    value: String,
}

#[derive(Debug, Arbitrary)]
struct FuzzEntity {
    is_monster: bool,
    origin: [f32; 3],
    yaw: f32,
    spawnflags: u32,
    keyvalues: Vec<FuzzKeyvalue>,
}

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    entities: Vec<FuzzEntity>,
}

fn bounded_component(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1_000_000.0, 1_000_000.0)
    } else {
        0.0
    }
}

fn to_entity_def(entity: &FuzzEntity) -> EntityDef {
    let mut keyvalues = BTreeMap::new();
    for pair in entity.keyvalues.iter().take(MAX_KEYS) {
        let key: String = pair.key.chars().take(MAX_STRING_BYTES).collect();
        if key.is_empty() {
            continue;
        }
        let value: String = pair.value.chars().take(MAX_STRING_BYTES).collect();
        keyvalues.insert(key, value);
    }
    let classname = if entity.is_monster {
        "monster_human_grunt"
    } else {
        "worldspawn"
    };
    let origin = [
        bounded_component(entity.origin[0]),
        bounded_component(entity.origin[1]),
        bounded_component(entity.origin[2]),
    ];
    let yaw = bounded_component(entity.yaw);
    EntityDef {
        classname: classname.to_string(),
        keyvalues,
        origin,
        angles: [0.0, yaw, 0.0],
        targetname: None,
        target: None,
        spawnflags: entity.spawnflags,
        model: None,
        render: RenderProps::default(),
    }
}

fuzz_target!(|input: FuzzInput| {
    let defs: Vec<EntityDef> = input
        .entities
        .iter()
        .take(MAX_ENTITIES)
        .map(to_entity_def)
        .collect();
    if defs.is_empty() {
        return;
    }

    let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());

    let mut ai = AiWorld::new(0x5EED);
    let brain = ai.register_brain(Box::new(DefaultBrain::ranged(
        Classification::HumanMilitary,
    )));

    let _spawned = attach_monsters(&mut registry, &defs, &|def: &EntityDef| {
        (def.classname == "monster_human_grunt")
            .then(|| MonsterSpawn::new(Classification::HumanMilitary, brain))
    });

    for _ in 0..TICKS {
        let _ = ai.tick(&mut registry.world, &SightContext::empty(), DT);
    }
});
