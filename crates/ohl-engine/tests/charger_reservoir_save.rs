//! Project-authored charger persistence through actual held-use input.
//! All geometry, entity blocks and corrupt wire fixtures are synthetic.

use std::cell::Cell;

use ohl_engine::components::Charger;
use ohl_engine::save::SECTION_CHARGER_RESERVOIRS;
use ohl_engine::save_state::{
    CHARGER_RESERVOIRS_VERSION, ChargerReservoirEntry, ChargerReservoirKind,
    ChargerReservoirsSnapshot,
};
use ohl_engine::test_support::{entity_block, synthetic_map_bsp_with_extra_entity};
use ohl_engine::{AssetSource, EngineError, Game, GameSave, Input, MemoryAssets, TICK_SECONDS};

const HEALTH: &str = "ohl_health_charger";
const SUIT: &str = "ohl_suit_charger";
// Over 600 ticks, 0.02 covers cumulative f32 additions/subtractions while
// remaining far below the extra charge a reset-to-full mutation dispenses.
const TOLERANCE: f32 = 0.02;

fn assets(extra: &str) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(
        "maps/ohlsynth.bsp",
        synthetic_map_bsp_with_extra_entity("ohl_charger_next", extra),
    );
    assets
}

fn charger(classname: &str, name: &str, origin: [f32; 3]) -> String {
    entity_block(classname, origin, 0.0, &[("targetname", name)])
}

fn health_assets(extra: &str) -> MemoryAssets {
    assets(&format!(
        "{}{}{}",
        entity_block("trigger_hurt", [0.0, 0.0, 32.0], 0.0, &[("dmg", "10")]),
        charger("func_healthcharger", HEALTH, [5.0, 0.0, 32.0]),
        extra,
    ))
}

fn suit_assets() -> MemoryAssets {
    assets(&format!(
        "{}{}{}",
        entity_block("item_suit", [0.0, 0.0, 32.0], 0.0, &[]),
        charger("func_recharge", SUIT, [5.0, 0.0, 32.0]),
        // Outside the engine's current 128-unit hurt radius from the charger.
        entity_block("trigger_hurt", [160.0, 0.0, 40.0], 0.0, &[("dmg", "1")]),
    ))
}

fn load(assets: &dyn AssetSource) -> Game {
    Game::load(assets, "ohlsynth").expect("synthetic room loads")
}

fn tick(game: &mut Game, count: usize, held: bool) {
    for _ in 0..count {
        game.tick(
            TICK_SECONDS,
            &Input {
                use_held: held,
                ..Input::default()
            },
        );
    }
}

fn named(game: &Game, name: &str) -> ohl_game::hecs::Entity {
    *game
        .registry()
        .find(name)
        .first()
        .expect("named synthetic entity")
}

fn index(game: &Game, name: &str) -> u32 {
    u32::try_from(
        game.registry()
            .entities
            .iter()
            .position(|e| *e == named(game, name))
            .unwrap(),
    )
    .expect("small synthetic registry")
}

fn remaining(game: &Game, name: &str) -> f32 {
    game.registry()
        .world
        .get::<&Charger>(named(game, name))
        .unwrap()
        .0
        .remaining()
}

fn close(actual: f32, expected: f32, message: &str) {
    assert!(
        (actual - expected).abs() < TOLERANCE,
        "{message}: {actual} vs {expected}"
    );
}

fn checkpoint(assets: &dyn AssetSource, depleted: bool) -> (GameSave, f32) {
    let mut game = load(assets);
    // Real damage first creates headroom; charging never saturates health.
    tick(&mut game, 200, false);
    tick(&mut game, if depleted { 600 } else { 200 }, true);
    let value = remaining(&game, HEALTH);
    if depleted {
        assert_eq!(value.to_bits(), 0.0_f32.to_bits());
    } else {
        assert!(value > 0.0 && value < ohl_combat::ChargerState::health().remaining());
    }
    assert!(game.player_health() > 0.0 && game.player_health() < game.player_max_health());
    (game.to_save(0), value)
}

fn health_payout(assets: &dyn AssetSource, save: &GameSave, expected: f32) {
    let bytes = save.to_bytes().expect("checkpoint encodes");
    let mut held = Game::load_bytes(assets, &bytes).expect("held continuation loads");
    let mut released = Game::load_bytes(assets, &bytes).expect("released continuation loads");
    for _ in 0..600 {
        tick(&mut held, 1, true);
        tick(&mut released, 1, false);
        for game in [&held, &released] {
            assert!(
                game.player_health() > 0.0,
                "paired continuations must stay alive"
            );
            assert!(
                game.player_health() < game.player_max_health(),
                "continuation must have headroom"
            );
        }
    }
    close(
        held.player_health() - released.player_health(),
        expected,
        "saved health payout",
    );
    assert_eq!(remaining(&held, HEALTH).to_bits(), 0.0_f32.to_bits());
}

#[test]
fn partially_used_health_charger_pays_only_saved_remainder_after_load() {
    let assets = health_assets("");
    let (save, value) = checkpoint(&assets, false);
    assert!(save.charger_reservoirs.is_some());
    health_payout(&assets, &save, value);
}

#[test]
fn partially_used_suit_charger_pays_only_saved_remainder_after_load() {
    let assets = suit_assets();
    let mut game = load(&assets);
    tick(&mut game, 1, false);
    assert!(game.player_suit_equipped());
    tick(&mut game, 200, true);
    let value = remaining(&game, SUIT);
    let armor = game.player_armor();
    assert!(value > 0.0 && value < 50.0);
    assert!(
        game.player_max_armor() - armor > 50.0,
        "even reset-to-full has headroom"
    );
    let mut restored = Game::load_bytes(&assets, &game.save_bytes(0).unwrap()).unwrap();
    tick(&mut restored, 600, true);
    close(restored.player_armor() - armor, value, "saved suit payout");
    assert!(restored.player_armor() < restored.player_max_armor());
    close(
        remaining(&restored, SUIT),
        0.0,
        "partial suit remainder is spent",
    );
}

#[test]
fn depleted_health_charger_stays_depleted_under_real_use_after_load() {
    let assets = health_assets("");
    let (save, value) = checkpoint(&assets, true);
    assert!(
        save.charger_reservoirs
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.remaining <= 0.0)
    );
    health_payout(&assets, &save, value);
}

#[test]
fn depleted_suit_charger_stays_depleted_under_real_use_after_load() {
    let assets = suit_assets();
    let mut game = load(&assets);
    tick(&mut game, 1, false);
    tick(&mut game, 600, true);
    close(
        remaining(&game, SUIT),
        0.0,
        "suit is exhausted within f32 resolution",
    );
    // Armor::recharge reports the rounded armor delta. From zero armor, the
    // last offer can be half an armor ULP and round to no gain. One real hurt
    // hit away from the charger changes that rounding boundary and creates
    // headroom for an entire reset-to-full mutation. Held use then spends the
    // final offer exactly; no live component or player scalar is assigned.
    let before_hurt = game.player_armor();
    game.set_viewpoint([160.0, 0.0, 40.0], 0.0, 0.0);
    for _ in 0..60 {
        tick(&mut game, 1, false);
        if game.player_armor() < before_hurt {
            break;
        }
    }
    assert!(
        game.player_armor() < before_hurt,
        "real hurt hit must create headroom"
    );
    game.set_viewpoint([0.0, 0.0, 40.0], 0.0, 0.0);
    tick(&mut game, 1, true);
    assert_eq!(remaining(&game, SUIT).to_bits(), 0.0_f32.to_bits());
    let armor = game.player_armor();
    assert!(game.player_max_armor() - armor >= 50.0 - TOLERANCE);
    let save = game.to_save(0);
    assert_eq!(save.charger_reservoirs.as_ref().unwrap().entries.len(), 1);
    let mut restored = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
    tick(&mut restored, 600, true);
    close(restored.player_armor(), armor, "depleted suit pays nothing");
}

#[test]
fn reservoir_restore_initializes_components_without_tag_39() {
    let assets = health_assets("");
    let (mut save, value) = checkpoint(&assets, false);
    save.switches = None;
    assert!(save.charger_reservoirs.is_some());
    health_payout(&assets, &save, value);
}

#[test]
fn old_save_without_reservoir_section_uses_full_spawn_default() {
    let assets = health_assets("");
    let (mut save, _) = checkpoint(&assets, false);
    save.charger_reservoirs = None;
    let bytes = save.to_bytes().unwrap();
    let reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default()).unwrap();
    assert!(matches!(
        reader.section(SECTION_CHARGER_RESERVOIRS),
        Err(ohl_save::SaveError::SectionNotFound)
    ));
    health_payout(
        &assets,
        &save,
        ohl_combat::ChargerState::health().remaining(),
    );
}

#[test]
fn cold_full_and_present_empty_captures_have_no_reservoir_overlay() {
    let assets = suit_assets();
    let mut game = load(&assets);
    assert!(game.to_save(0).charger_reservoirs.is_none());
    assert!(
        game.registry()
            .world
            .get::<&Charger>(named(&game, SUIT))
            .is_err(),
        "cold capture must stay lazy"
    );
    tick(&mut game, 1, false);
    assert!(game.to_save(0).charger_reservoirs.is_none());
    let mut save = game.to_save(0);
    save.charger_reservoirs = Some(snapshot(Vec::new()));
    let restored = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
    close(
        remaining(&restored, SUIT),
        50.0,
        "empty overlay keeps full capacity",
    );
    assert!(restored.to_save(0).charger_reservoirs.is_none());
    let cold = load(&assets).save_bytes(0).unwrap();
    let restored = Game::load_bytes(&assets, &cold).unwrap();
    assert!(restored.to_save(0).charger_reservoirs.is_none());
}

#[test]
fn sparse_entries_preserve_separated_charger_identity_and_unused_full_state() {
    let assets = assets(&format!(
        "{}{}{}{}",
        entity_block("item_suit", [0.0, 0.0, 32.0], 0.0, &[]),
        charger("func_recharge", "ohl_first", [0.0, 0.0, 40.0]),
        charger("func_recharge", "ohl_second", [100.0, 0.0, 40.0]),
        charger("func_recharge", "ohl_unused", [-100.0, 0.0, 40.0]),
    ));
    let mut game = load(&assets);
    tick(&mut game, 1, false);
    game.set_viewpoint([0.0, 0.0, 40.0], 0.0, 0.0);
    tick(&mut game, 100, true);
    game.set_viewpoint([100.0, 0.0, 40.0], 0.0, 0.0);
    tick(&mut game, 200, true);
    let first = remaining(&game, "ohl_first");
    let second = remaining(&game, "ohl_second");
    assert!((first - second).abs() > 5.0);
    let save = game.to_save(0);
    assert_eq!(save.charger_reservoirs.as_ref().unwrap().entries.len(), 2);
    let mut restored = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
    close(remaining(&restored, "ohl_first"), first, "first identity");
    close(
        remaining(&restored, "ohl_second"),
        second,
        "second identity",
    );
    close(
        remaining(&restored, "ohl_unused"),
        50.0,
        "sparse omitted full charger",
    );
    restored.set_viewpoint([0.0, 0.0, 40.0], 0.0, 0.0);
    tick(&mut restored, 100, true);
    close(
        remaining(&restored, "ohl_first"),
        first - 10.0,
        "first real-use continuation",
    );
    close(
        remaining(&restored, "ohl_second"),
        second,
        "second stays separate",
    );
    restored.set_viewpoint([100.0, 0.0, 40.0], 0.0, 0.0);
    tick(&mut restored, 100, true);
    close(
        remaining(&restored, "ohl_second"),
        second - 10.0,
        "second real-use continuation",
    );
}

fn snapshot(entries: Vec<ChargerReservoirEntry>) -> ChargerReservoirsSnapshot {
    ChargerReservoirsSnapshot {
        version: CHARGER_RESERVOIRS_VERSION,
        entries,
    }
}

fn entry(registry_index: u32, kind: ChargerReservoirKind, remaining: f32) -> ChargerReservoirEntry {
    ChargerReservoirEntry {
        registry_index,
        kind,
        remaining,
    }
}

fn with_raw_section(save: &GameSave, raw: &[u8]) -> Vec<u8> {
    let bytes = save.to_bytes().unwrap();
    let reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default()).unwrap();
    let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
    for section in reader
        .sections()
        .iter()
        .filter(|section| section.tag != SECTION_CHARGER_RESERVOIRS)
    {
        writer
            .add_section(section.tag, reader.section(section.tag).unwrap())
            .unwrap();
    }
    writer.add_section(SECTION_CHARGER_RESERVOIRS, raw).unwrap();
    writer.finish(&ohl_save::Limits::default()).unwrap()
}

#[test]
fn exact_256_entry_boundary_and_whole_container_roundtrip() {
    let mut save = load(&suit_assets()).to_save(0);
    let value = snapshot(
        (0..256)
            .map(|i| entry(i, ChargerReservoirKind::Health, 12.5))
            .collect(),
    );
    save.charger_reservoirs = Some(value.clone());
    let decoded = GameSave::from_bytes(&save.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded.charger_reservoirs, Some(value));
    save.charger_reservoirs
        .as_mut()
        .unwrap()
        .entries
        .push(entry(256, ChargerReservoirKind::Health, 1.0));
    assert!(matches!(save.to_bytes(), Err(EngineError::SaveUnwritable)));
    let oversized = postcard::to_allocvec(save.charger_reservoirs.as_ref().unwrap()).unwrap();
    save.charger_reservoirs = None;
    assert!(matches!(
        GameSave::from_bytes(&with_raw_section(&save, &oversized)),
        Err(EngineError::SaveUnreadable)
    ));
}

struct ReadCounter(Cell<usize>);
impl AssetSource for ReadCounter {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        self.0.set(self.0.get() + 1);
        None
    }
    fn resolve_wads(&self, _: &str) -> Vec<Vec<u8>> {
        self.0.set(self.0.get() + 1);
        Vec::new()
    }
}

#[test]
fn invalid_dtos_fail_writer_decoder_and_direct_load_before_any_asset_read() {
    let base = load(&suit_assets()).to_save(0);
    let mut invalid = Vec::new();
    for kind in [ChargerReservoirKind::Health, ChargerReservoirKind::Suit] {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 50.1] {
            invalid.push(snapshot(vec![entry(0, kind, value)]));
        }
    }
    invalid.push(ChargerReservoirsSnapshot {
        version: 2,
        entries: Vec::new(),
    });
    invalid.push(snapshot(vec![entry(
        65_536,
        ChargerReservoirKind::Health,
        1.0,
    )]));
    invalid.push(snapshot(vec![
        entry(1, ChargerReservoirKind::Health, 1.0),
        entry(1, ChargerReservoirKind::Health, 2.0),
    ]));
    invalid.push(snapshot(vec![
        entry(2, ChargerReservoirKind::Health, 1.0),
        entry(1, ChargerReservoirKind::Health, 2.0),
    ]));
    invalid.push(snapshot(
        (0..257)
            .map(|i| entry(i, ChargerReservoirKind::Health, 0.0))
            .collect(),
    ));
    for value in invalid {
        let mut save = base.clone();
        save.charger_reservoirs = Some(value.clone());
        assert!(matches!(save.to_bytes(), Err(EngineError::SaveUnwritable)));
        let counter = ReadCounter(Cell::new(0));
        assert!(matches!(
            Game::from_save(&counter, &save),
            Err(EngineError::SaveUnreadable)
        ));
        assert_eq!(
            counter.0.get(),
            0,
            "invalid DTO must reject before any asset read"
        );
        let raw = postcard::to_allocvec(&value).unwrap();
        assert!(matches!(
            GameSave::from_bytes(&with_raw_section(&base, &raw)),
            Err(EngineError::SaveUnreadable)
        ));
    }
}

#[test]
fn malformed_unknown_kind_truncated_trailing_and_excessive_length_sections_fail_closed() {
    let base = load(&suit_assets()).to_save(0);
    let valid = postcard::to_allocvec(&snapshot(vec![entry(1, ChargerReservoirKind::Health, 0.0)]))
        .unwrap();
    let mut trailing = valid.clone();
    trailing.push(0);
    let mut unknown_kind = valid.clone();
    unknown_kind[3] = 2;
    for raw in [
        Vec::new(),
        vec![1],
        valid[..valid.len() - 1].to_vec(),
        trailing,
        unknown_kind,
        vec![1, 0x81, 0x02],
        vec![1, 0xff, 0xff, 0xff, 0xff, 0x0f],
    ] {
        assert!(matches!(
            GameSave::from_bytes(&with_raw_section(&base, &raw)),
            Err(EngineError::SaveUnreadable)
        ));
    }
}

#[test]
fn valid_orphan_noncharger_and_wrong_kind_slots_do_not_shift_to_an_extant_charger() {
    let assets = suit_assets();
    let mut game = load(&assets);
    tick(&mut game, 1, false);
    let charger_index = index(&game, SUIT);
    let noncharger_index = game
        .registry()
        .entities
        .iter()
        .position(|e| *e != named(&game, SUIT))
        .unwrap();
    let mut entries = vec![
        entry(
            u32::try_from(noncharger_index).unwrap(),
            ChargerReservoirKind::Suit,
            0.0,
        ),
        entry(charger_index, ChargerReservoirKind::Health, 0.0),
        entry(65_535, ChargerReservoirKind::Suit, 0.0),
    ];
    entries.sort_unstable_by_key(|e| e.registry_index);
    let mut save = game.to_save(0);
    save.charger_reservoirs = Some(snapshot(entries));
    let mut restored = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
    close(
        remaining(&restored, SUIT),
        50.0,
        "ignored references leave correct charger full",
    );
    tick(&mut restored, 100, true);
    close(
        restored.player_armor(),
        10.0,
        "extant charger still pays through real use",
    );
}

#[test]
fn an_ordinarily_deleted_charger_is_not_resurrected_by_a_valid_reservoir_reference() {
    let extra = format!(
        "{}{}{}",
        entity_block(
            "monster_barney",
            [64.0, 64.0, 40.0],
            0.0,
            &[("targetname", "ohl_guard")]
        ),
        entity_block(
            "scripted_sequence",
            [64.0, 64.0, 40.0],
            0.0,
            &[
                ("targetname", "ohl_remove"),
                ("m_iszEntity", "ohl_guard"),
                ("m_fMoveTo", "0"),
                ("killtarget", HEALTH)
            ]
        ),
        entity_block(
            "trigger_auto",
            [0.0, 0.0, 0.0],
            0.0,
            &[("target", "ohl_remove"), ("delay", "0.3")]
        ),
    );
    let assets = health_assets(&extra);
    let mut game = load(&assets);
    let charger_index = index(&game, HEALTH);
    tick(&mut game, 60, false);
    assert_eq!(game.script_completion_count(), 1);
    assert!(!game.registry().world.contains(named(&game, HEALTH)));
    let mut save = game.to_save(0);
    assert!(
        save.entity_combat.as_ref().unwrap()[usize::try_from(charger_index).unwrap()].is_none()
    );
    save.charger_reservoirs = Some(snapshot(vec![entry(
        charger_index,
        ChargerReservoirKind::Health,
        12.5,
    )]));
    let restored = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
    assert!(!restored.registry().world.contains(named(&restored, HEALTH)));
    // Old/hand-constructed saves without tag 24 cannot infer deletion from a sparse reservoir.
    save.entity_combat = None;
    let restored = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
    assert!(restored.registry().world.contains(named(&restored, HEALTH)));
    close(
        remaining(&restored, HEALTH),
        12.5,
        "absence of tag 24 has no deletion claim",
    );
}
