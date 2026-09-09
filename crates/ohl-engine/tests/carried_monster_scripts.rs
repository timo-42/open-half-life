//! A monster the destination map does not declare, but whose own
//! `scripted_sequence`s name it, arrives as a *monster*.
//!
//! The documented eligibility rule for a level change is that an entity
//! must be inside a `trigger_transition` volume "or otherwise in the
//! landmark's PVS" (`docs/FORMAT_SOURCES.md`, "Campaign flow"), and an
//! entity the destination declares no counterpart for is re-created there.
//! Re-creating it from a classname and a position alone was not enough: the
//! destination's own build pipeline (`ohl_ai::spawn::attach_monsters`,
//! `AiState::{register_brains, attach_scripts}`) reads
//! `ohl_engine::level::Level::defs`, so a carried monster with no entity
//! *definition* got no brain and no `Actor`, and a destination map whose
//! arrival sequence is a chain of `scripted_sequence`s written around that
//! monster waited for an actor that could never exist.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    DOOR_NAME, LANDMARK, NEXT_MAP, SYNTHETIC_MAP, synthetic_map_bsp_with_extra_entity,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets, StudioAnim, TICK_SECONDS};
use ohl_game::registry::{ClassName, Door, MoverState, TargetName};

/// `monster_scientist`'s own documented default model path
/// (`ohl_ai::monsters::table::MonsterKind::default_model_path`), published
/// here so the fixture's carried guard — which names no explicit `model`
/// keyvalue, the ordinary case for a `monster_*` entity — has one to load.
const GUARD_MODEL: &str = "models/scientist.mdl";

/// The `targetname` the source map's monster carries and the destination
/// map's script names. Project-authored, like every other literal here.
const GUARD: &str = "ohl_guard";

/// The destination's script, fired by its own `trigger_auto`.
const SCRIPT: &str = "ohl_script";

fn assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    // The source map: the usual fixture plus one named monster, standing
    // well inside the landmark's carry radius.
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_extra_entity(
            NEXT_MAP,
            &format!(
                "{{\n\"classname\" \"monster_scientist\"\n\"targetname\" \"{GUARD}\"\n\
                 \"origin\" \"48 0 32\"\n}}\n"
            ),
        ),
    );
    // The destination declares no monster at all — only a script that names
    // one by `targetname`, and a `trigger_auto` to start it. The script
    // neither moves its monster (`m_fMoveTo` "0") nor plays an animation,
    // so it completes as soon as it has one and fires the fixture's door.
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        synthetic_map_bsp_with_extra_entity(
            SYNTHETIC_MAP,
            &format!(
                "{{\n\"classname\" \"scripted_sequence\"\n\"targetname\" \"{SCRIPT}\"\n\
                 \"m_iszEntity\" \"{GUARD}\"\n\"m_fMoveTo\" \"0\"\n\
                 \"target\" \"{DOOR_NAME}\"\n\"origin\" \"48 0 32\"\n}}\n\
                 {{\n\"classname\" \"trigger_auto\"\n\"target\" \"{SCRIPT}\"\n\
                 \"origin\" \"0 0 32\"\n}}\n"
            ),
        ),
    );
    // The carried guard's own default studio model (see `GUARD_MODEL`'s doc
    // comment): a minimal, valid, synthetic MDL10 fixture — not any
    // payload-derived model — so the carry can be asserted past `Actor`
    // attachment through to `StudioAnim` attachment.
    let (mdl_bytes, _layout) = ohl_formats::test_support::build_minimal_mdl10();
    assets.insert(GUARD_MODEL, mdl_bytes);
    assets
}

/// How many entities in `game`'s current level carry a [`StudioAnim`] — the
/// same gate `render.rs`'s `collect_studio_instances` queries to decide
/// what to draw a model for (see that function's own doc comment). Not a
/// call into the (GPU-backed, not unit-testable) renderer itself, but the
/// same ECS predicate it runs.
fn studio_anim_count(game: &Game) -> usize {
    game.registry().world.query::<&StudioAnim>().iter().count()
}

fn tick_n(game: &mut Game, n: u32) {
    for _ in 0..n {
        game.tick(TICK_SECONDS, &Input::default());
    }
}

/// Whether the destination declares an entity of `classname`.
fn count_of(game: &Game, classname: &str) -> usize {
    game.registry()
        .world
        .query::<(ohl_game::hecs::Entity, &ClassName)>()
        .iter()
        .filter(|(_, name)| name.0 == classname)
        .count()
}

fn door_state(game: &Game) -> MoverState {
    game.registry()
        .world
        .query::<(ohl_game::hecs::Entity, &Door, &TargetName)>()
        .iter()
        .find(|(_, _, name)| name.0 == DOOR_NAME)
        .map(|(_, door, _)| door.state)
        .expect("the fixture declares its door")
}

#[test]
fn a_carried_monster_arrives_as_a_monster_the_destination_can_script() {
    let assets = assets();
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the synthetic map loads");
    tick_n(&mut game, 5);
    assert_eq!(count_of(&game, "monster_scientist"), 1);
    assert_eq!(
        studio_anim_count(&game),
        1,
        "the source map's own guard is drawable before any carry happens"
    );

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    assert_eq!(game.map(), NEXT_MAP);
    assert_eq!(
        count_of(&game, "monster_scientist"),
        1,
        "the carried monster must exist in the destination"
    );
    // The carried guard is simulated (an `Actor`, a brain, a running
    // script) well before this fix; without it, it was never also drawable
    // — `Level::attach_studio_models` is what a level change now runs for
    // whatever `crate::transition::materialise_carried` just appended.
    assert_eq!(
        studio_anim_count(&game),
        1,
        "the carried guard must get a StudioAnim in the destination too"
    );
    assert_eq!(
        game.prop_count(),
        1,
        "and count as one of this level's drawable studio placements"
    );

    // The script needs a monster to possess before it can complete, and it
    // only ever gets one if the carried entity was built as a monster.
    tick_n(&mut game, 240);
    assert!(
        game.script_completion_count() > 0,
        "the destination's script must have run its carried actor"
    );
    assert_ne!(
        door_state(&game),
        MoverState::Closed,
        "the script's own `target` must have fired the destination's door"
    );
}

/// A materialised carried entity has to survive a save/load, and this is
/// not a nicety: `Game::restore` zips every index-keyed section against the
/// *freshly built* level's `Registry::entities`, which a reload rebuilds
/// from the map's own entity lump alone — and a carried entity is appended
/// past the end of it. Without `SECTION_CARRIED_ENTITIES` (36) the monster
/// is simply gone after a load, and with it the destination's whole arrival
/// sequence, which is written around that monster. The real map's own
/// opening chain fires a `trigger_autosave`, so this is the ordinary case,
/// not a corner.
#[test]
fn a_materialised_carried_entity_survives_a_save_and_load() {
    let assets = assets();
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the synthetic map loads");
    tick_n(&mut game, 5);
    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    assert_eq!(count_of(&game, "monster_scientist"), 1);

    let save = game.to_save(1_700_000_000);
    assert_eq!(
        save.carried_entities.as_ref().map(Vec::len),
        Some(1),
        "the transition materialised exactly one entity, so the save writes it"
    );
    let bytes = save.to_bytes().expect("the save encodes");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");

    assert_eq!(
        count_of(&reloaded, "monster_scientist"),
        1,
        "the carried monster must come back out of tag 36"
    );
    // And it must come back as a *monster*, not a husk: the destination's
    // script still has to find an actor to possess...
    assert_eq!(
        studio_anim_count(&reloaded),
        1,
        "...and it must come back drawable too: `Game::from_save_with` runs \
         `Level::attach_studio_models` for whatever tag 36's own \
         `restore_carried_entities` just re-materialised, the same pass a \
         live level change runs"
    );
    tick_n(&mut reloaded, 240);
    assert!(
        reloaded.script_completion_count() > 0,
        "the reloaded map's script must still run its carried actor"
    );
    assert_ne!(
        door_state(&reloaded),
        MoverState::Closed,
        "and still fire the door it names"
    );
}

/// A save from a build before tag 36 existed — or from any map nothing was
/// carried into — still loads, with the section reading as its documented
/// default: the map's own entities and nothing else.
#[test]
fn a_save_missing_tag_36_still_loads() {
    let assets = assets();
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the synthetic map loads");
    tick_n(&mut game, 5);
    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    let mut save = game.to_save(1_700_000_000);
    save.carried_entities = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing tag 36 still encodes");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("an old-shaped save still loads");
    assert_eq!(
        count_of(&reloaded, "monster_scientist"),
        0,
        "no tag 36: the reload is the map's own entities, exactly as before M9.26"
    );
}

/// A cold-loaded map carries nothing, so its save writes no section at all
/// rather than an empty one.
#[test]
fn a_cold_loaded_map_writes_no_carried_entity_section() {
    let assets = assets();
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the synthetic map loads");
    tick_n(&mut game, 5);
    assert_eq!(game.to_save(1_700_000_000).carried_entities, None);
    let _: &dyn AssetSource = &assets;
}
