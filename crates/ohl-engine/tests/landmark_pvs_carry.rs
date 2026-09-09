//! Which entities a level change carries: the documented "inside the
//! `trigger_transition` volume, or otherwise in the landmark's PVS"
//! eligibility rule (`docs/FORMAT_SOURCES.md`, "Campaign flow").
//!
//! Neither map here declares a transition volume, which is the shape the
//! rule's PVS half is for. The fixture room is 2,048 units across — wider
//! than `DEFAULT_CARRY_RADIUS`, which is only this project's stand-in for
//! the cases a leaf query cannot answer — and split into two leaves by the
//! `x = 0` plane, with a hand-written visibility lump in which each leaf
//! sees only itself. So the three entities below separate the two readings
//! completely:
//!
//! * beyond the radius, in the landmark's own leaf — carried by the PVS
//!   rule, dropped by a radius-only one;
//! * beyond the radius, in the other leaf — dropped by both;
//! * *inside* the radius, in the other leaf — dropped by the PVS rule, and
//!   carried by a rule that merely `or`s the radius over the top of it.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{PVS_MAP, PVS_NEXT_MAP, pvs_room_bsp};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::registry::TargetName;

/// The landmark both maps share. Project-authored.
const LANDMARK: &str = "ohl_pvs_landmark";

/// Beyond the carry radius, in the landmark's own leaf.
const FAR_VISIBLE: &str = "ohl_far_visible";

/// Beyond the carry radius, in the leaf the landmark's row does not set.
const FAR_HIDDEN: &str = "ohl_far_hidden";

/// Inside the carry radius, in the leaf the landmark's row does not set.
const NEAR_HIDDEN: &str = "ohl_near_hidden";

/// The source map: a landmark at `x = 16` (leaf 1), a `trigger_changelevel`
/// naming the destination, and the three props above.
fn source_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"16 0 32\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"16 0 32\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"targetname\" \"ohl_pvs_exit\"\n\
         \"map\" \"{PVS_NEXT_MAP}\"\n\"landmark\" \"{LANDMARK}\"\n}}\n\
         {{\n\"classname\" \"monster_scientist\"\n\"targetname\" \"{FAR_VISIBLE}\"\n\
         \"origin\" \"800 0 32\"\n}}\n\
         {{\n\"classname\" \"monster_scientist\"\n\"targetname\" \"{FAR_HIDDEN}\"\n\
         \"origin\" \"-800 0 32\"\n}}\n\
         {{\n\"classname\" \"monster_scientist\"\n\"targetname\" \"{NEAR_HIDDEN}\"\n\
         \"origin\" \"-100 0 32\"\n}}\n"
    )
}

/// The destination declares the same landmark and nothing else: every
/// entity that arrives here arrived by being carried.
fn destination_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"16 0 32\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"16 0 32\"\n}}\n"
    )
}

fn assets(visible_everywhere: bool) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PVS_MAP}.bsp"),
        pvs_room_bsp(&source_entities(), visible_everywhere),
    );
    assets.insert(
        &format!("maps/{PVS_NEXT_MAP}.bsp"),
        pvs_room_bsp(&destination_entities(), visible_everywhere),
    );
    assets
}

/// Every `targetname` the destination map holds, sorted.
fn arrived(game: &Game) -> Vec<String> {
    let mut names: Vec<String> = game
        .registry()
        .world
        .query::<(ohl_game::hecs::Entity, &TargetName)>()
        .iter()
        .map(|(_, name)| name.0.clone())
        .filter(|name| name.starts_with("ohl_far") || name.starts_with("ohl_near"))
        .collect();
    names.sort();
    names
}

fn crossed(visible_everywhere: bool) -> Vec<String> {
    let assets = assets(visible_everywhere);
    let mut game = Game::load(&assets, PVS_MAP).expect("the fixture map loads");
    game.tick(TICK_SECONDS, &Input::default());
    game.change_level(&assets, PVS_NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    assert_eq!(game.map(), PVS_NEXT_MAP);
    arrived(&game)
}

/// The rule, both directions at once: an entity 784 units from the landmark
/// travels because it is in the landmark's PVS, and neither entity in the
/// other leaf travels — including the one only 116 units away, which a
/// radius-only rule (or a rule that keeps the radius as an `or`) would have
/// carried.
#[test]
fn the_landmark_s_pvs_decides_which_entities_travel() {
    assert_eq!(crossed(false), vec![FAR_VISIBLE.to_string()]);
}

/// The same fixture with a lump in which every leaf sees every leaf: now
/// all three travel, so the fixture's geometry and distances are not what
/// produced the result above — the visibility bits are.
#[test]
fn a_lump_that_sees_everywhere_carries_all_three() {
    assert_eq!(
        crossed(true),
        vec![
            FAR_HIDDEN.to_string(),
            FAR_VISIBLE.to_string(),
            NEAR_HIDDEN.to_string()
        ]
    );
}

/// A brush entity the destination declares no counterpart for is *not*
/// materialised there, however eligible it is: the cited pages say a brush
/// entity needs "a unique global name to be able to be carried over", and
/// that name is how the destination's own copy of the brush is found. There
/// is no copy to find here, and a brush entity separated from the submodel
/// its own map compiled is nothing at all.
#[test]
fn an_eligible_brush_entity_is_never_materialised_in_the_destination() {
    let mut assets = MemoryAssets::new();
    // One `func_wall` on submodel `*0`, in the landmark's own leaf, with a
    // `targetname` the destination never declares.
    let source = format!(
        "{}{{\n\"classname\" \"func_wall\"\n\"targetname\" \"ohl_carried_wall\"\n\
         \"model\" \"*0\"\n\"origin\" \"800 0 0\"\n}}\n",
        source_entities()
    );
    assets.insert(&format!("maps/{PVS_MAP}.bsp"), pvs_room_bsp(&source, false));
    assets.insert(
        &format!("maps/{PVS_NEXT_MAP}.bsp"),
        pvs_room_bsp(&destination_entities(), false),
    );

    let mut game = Game::load(&assets, PVS_MAP).expect("the fixture map loads");
    game.tick(TICK_SECONDS, &Input::default());
    game.change_level(&assets, PVS_NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    assert!(
        game.registry().find("ohl_carried_wall").is_empty(),
        "a brush entity must not be re-created in a map that never compiled its submodel"
    );
    // The point entity beside it still travels, so the assertion above is
    // about the brush and not about eligibility.
    assert_eq!(arrived(&game), vec![FAR_VISIBLE.to_string()]);
}
