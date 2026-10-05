//! Default sky selection through the real skirmish load path. All map and
//! image bytes are project-authored. The default identifier's provenance is
//! recorded in docs/FORMAT_SOURCES.md, "Skirmish sky and combat sound compatibility".

use ohl_engine::test_support::{AI_MAP, ai_room_bsp};
use ohl_engine::{Game, GameConfig, MemoryAssets, SkirmishConfig};
use ohl_world::SKY_FACE_SUFFIXES;

fn assets(sky_key: &str) -> MemoryAssets {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n{sky_key}\n}}\n\
         {{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"0 0 36\"\n}}\n"
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), ai_room_bsp(&entities, false));
    assets
}

fn sky_faces(assets: &mut MemoryAssets, name: &str) {
    // Independently authored 1x1 true-color TGA, with one visible pixel.
    let mut tga = vec![0; 18];
    tga[2] = 2;
    tga[12] = 1;
    tga[14] = 1;
    tga[16] = 24;
    tga.extend_from_slice(&[180, 120, 80]);
    for suffix in SKY_FACE_SUFFIXES {
        assets.insert(&format!("gfx/env/{name}{suffix}.tga"), tga.clone());
    }
}

fn load(assets: &MemoryAssets) -> Game {
    Game::load_skirmish(
        assets,
        AI_MAP,
        &GameConfig::default(),
        &SkirmishConfig {
            bots: 0,
            ..SkirmishConfig::default()
        },
    )
    .expect("the synthetic arena loads")
}

#[test]
fn an_absent_empty_or_unavailable_map_sky_uses_the_default() {
    for key in ["", "\"skyname\" \"\"", "\"skyname\" \"ohl_missing\""] {
        let mut assets = assets(key);
        sky_faces(&mut assets, "desert");
        assert!(load(&assets).has_skybox());
    }
}

#[test]
fn an_explicit_map_sky_loads_without_default_assets() {
    let mut assets = assets("\"skyname\" \"ohl_sky\"");
    sky_faces(&mut assets, "ohl_sky");
    assert!(load(&assets).has_skybox());
}

#[test]
fn a_corrupt_map_sky_falls_back_to_the_default() {
    let mut assets = assets("\"skyname\" \"ohl_sky\"");
    sky_faces(&mut assets, "ohl_sky");
    assets.insert("gfx/env/ohl_skyup.tga", vec![0; 4]);
    sky_faces(&mut assets, "desert");
    assert!(load(&assets).has_skybox());
}

#[test]
fn missing_or_incomplete_default_assets_do_not_prevent_starting_a_match() {
    let mut assets = assets("");
    assert!(!load(&assets).has_skybox());
    sky_faces(&mut assets, "desert");
    assets.insert("gfx/env/desertup.tga", vec![0; 4]);
    let game = load(&assets);
    assert!(game.is_skirmish());
    assert!(!game.has_skybox());
}
