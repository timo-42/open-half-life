//! `cargo fuzz` target for M7.9 P4b's restore path: `Game::load_bytes`
//! (`GameSave::from_bytes` followed by `Game::from_save`/`Game::restore`)
//! must never panic, even when `SECTION_INVENTORY`/`SECTION_ENTITY_COMBAT`/
//! `SECTION_AI`/`SECTION_PROJECTILES`/`SECTION_RNG` (tags 23-27) decode into
//! adversarial values `sections_fuzz` cannot reach: that target only checks
//! `GameSave::from_bytes` itself, so it never runs `restore()`'s own logic
//! (spawn-index lookups, `ohl_ai::ScheduleRunner::restore`,
//! `ohl_combat::FiringState::restore`, `ProjectileSet`/`DeployableSet::
//! restore_from_parts`, the entity-despawn path for a `None`
//! `SECTION_ENTITY_COMBAT` slot, ...) against anything.
//!
//! Tags 16-22 are always the real bytes a valid `Game::to_save` produced
//! over this crate's own synthetic map fixture, so decoding those never
//! fails and every input reaches `Game::from_save`; only tags 23-27 are
//! arbitrary (and bounded), so the fuzzer's coverage feedback concentrates
//! on the restore path this package added rather than rediscovering
//! `sections_fuzz`'s own truncation/corruption coverage of the pre-existing
//! sections. A successful load also ticks the reloaded `Game` a few steps,
//! cheaply exercising the simulation phases against whatever state
//! `restore()` actually built, not just the restore call itself.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ohl_engine::save::{
    SECTION_AI, SECTION_ENGINE_HEADER, SECTION_ENTITY_COMBAT, SECTION_ENTITY_REGISTRY,
    SECTION_GLOBAL_STATE, SECTION_INVENTORY, SECTION_LIGHT_STYLE_TIME, SECTION_PLAYER_CARRY,
    SECTION_PROJECTILES, SECTION_RNG, SECTION_SIMULATION, SECTION_VIEW,
};
use ohl_engine::{Game, MemoryAssets};
use ohl_save::{Header, Limits, SaveWriter};

/// Most bytes kept from one section's arbitrary payload; matches
/// `sections_fuzz`'s own bound.
const MAX_SECTION_BYTES: usize = 512;

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    inventory: Vec<u8>,
    entity_combat: Vec<u8>,
    ai: Vec<u8>,
    projectiles: Vec<u8>,
    rng: Vec<u8>,
}

fuzz_target!(|input: FuzzInput| {
    let bytes = ohl_engine::test_support::synthetic_map_bsp();
    let mut assets = MemoryAssets::new();
    assets.insert("maps/ohlsynth.bsp", bytes.clone());
    let Ok(game) = Game::from_map_bytes(&assets, "ohlsynth", &bytes) else {
        return;
    };
    let base = game.to_save(0);

    let header = Header {
        game_version: String::new(),
        created_at_unix_secs: base.created_at_unix_secs,
        map_identity: base.header.map.clone(),
        title: base.header.map.clone(),
        thumbnail: Vec::new(),
    };
    let mut writer = SaveWriter::begin(header);
    // The seven sections M7.9 P4b did not add, encoded from a real,
    // just-captured save so they always decode and `restore()` is always
    // reached.
    if writer.add_section_serde(SECTION_ENGINE_HEADER, &base.header).is_err() {
        return;
    }
    if writer.add_section_serde(SECTION_PLAYER_CARRY, &base.player).is_err() {
        return;
    }
    if writer
        .add_section_serde(SECTION_ENTITY_REGISTRY, &base.entities)
        .is_err()
    {
        return;
    }
    if writer
        .add_section_serde(SECTION_SIMULATION, &base.simulation)
        .is_err()
    {
        return;
    }
    if writer
        .add_section_serde(SECTION_GLOBAL_STATE, &base.globals)
        .is_err()
    {
        return;
    }
    if writer
        .add_section_serde(SECTION_LIGHT_STYLE_TIME, &base.light_style_time)
        .is_err()
    {
        return;
    }
    if writer.add_section_serde(SECTION_VIEW, &base.view).is_err() {
        return;
    }

    // The five sections this package added, with arbitrary (bounded) bytes.
    // Most inputs fail to decode as the documented DTO: `GameSave::
    // from_bytes` (via `crate::save::optional_section`) then reports the
    // whole read as `EngineError::SaveUnreadable` rather than substituting a
    // default — a section present but corrupt fails the load closed, the
    // same as every other section; only a section absent from the
    // container entirely reads back as `None`/a default, which none of
    // these five ever are here (each is always written, just with
    // arbitrary bytes). Coverage feedback still finds the inputs that *do*
    // decode as the documented DTO, which is what actually exercises
    // `restore()`'s own logic on adversarial-but-well-typed values.
    for (tag, raw) in [
        (SECTION_INVENTORY, &input.inventory),
        (SECTION_ENTITY_COMBAT, &input.entity_combat),
        (SECTION_AI, &input.ai),
        (SECTION_PROJECTILES, &input.projectiles),
        (SECTION_RNG, &input.rng),
    ] {
        let end = raw.len().min(MAX_SECTION_BYTES);
        let _ = writer.add_section(tag, &raw[..end]);
    }

    let Ok(container) = writer.finish(&Limits::default()) else {
        return;
    };

    let Ok(mut reloaded) = Game::load_bytes(&assets, &container) else {
        return;
    };
    // Cheaply exercises ticking a state `restore()` actually built (not
    // just the restore call itself): a handful of steps is enough to run
    // every phase at least once against whatever the arbitrary sections
    // left in the AI/projectile/inventory state, at negligible cost per
    // input.
    let tick_input = ohl_engine::Input::default();
    for _ in 0..4 {
        reloaded.tick(ohl_engine::TICK_SECONDS, &tick_input);
    }
});
