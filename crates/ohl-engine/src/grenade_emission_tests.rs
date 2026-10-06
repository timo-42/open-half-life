//! Authored frozen-world throw policy controls, not private-scene attribution.
use super::*;
use crate::components::StudioAnim;
use crate::projectiles::ProjectileSystem;
use ohl_combat::{HitboxIndex, HitboxLimits, ProjectileKind};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush, build_minimal_mdl10_with_hitbox};

pub(super) fn context_parts(level: &Level) -> (ProjectileSystem, HitboxIndex) {
    let mut hitboxes = HitboxIndex::new(HitboxLimits::default());
    crate::combat::rebuild_hitbox_index(&mut hitboxes, level);
    assert_eq!(hitboxes.rejected(), 0);
    let mut projectiles = ProjectileSystem::new(0);
    projectiles.update_blast_bounds(&hitboxes);
    (projectiles, hitboxes)
}

pub(super) fn place(game: &crate::Game, entity: Entity, origin: Vec3) {
    game.registry()
        .world
        .get::<&mut Actor>(entity)
        .unwrap()
        .origin = origin;
    game.registry()
        .world
        .get::<&mut Transform>(entity)
        .unwrap()
        .origin = origin;
}

/// Explicit forced launch for physics prerequisites; never a production veto bypass.
pub(super) fn forced_request(
    level: &Level,
    owner: Entity,
    difficulty: AiDifficulty,
) -> ProjectileRequest {
    let origin = level.registry.world.get::<&Actor>(owner).unwrap().eye();
    let aim = level
        .registry
        .world
        .get::<&Actor>(level.player)
        .unwrap()
        .eye();
    let kind = ProjectileKind::HandGrenade;
    let (damage, damage_type, blast_radius) = monster_projectile_profile(kind, difficulty);
    ProjectileRequest {
        kind,
        owner,
        origin,
        velocity: monster_projectile_velocity(kind, difficulty, origin, aim),
        damage,
        damage_type,
        blast_radius,
        target: Some(level.player),
    }
}

#[derive(Clone, Copy)]
enum Exposure {
    Owner,
    Ally,
    Safe,
}

// Keep the generated room, actors and admission prerequisites together.
#[allow(clippy::too_many_lines)]
fn scene(kind: &str, exposure: Exposure) -> (crate::Game, Entity) {
    use crate::test_support::entity_block;
    let target_x = if matches!(exposure, Exposure::Owner) {
        400.0
    } else {
        800.0
    };
    let panel_x = if matches!(exposure, Exposure::Owner) {
        192.0
    } else {
        600.0
    };
    let mut text = format!(
        "{{\"classname\" \"worldspawn\"}}{}{}",
        entity_block("info_player_start", [target_x, 0.0, 36.0], 180.0, &[]),
        entity_block(kind, [0.0; 3], 0.0, &[("targetname", "ohl_thrower")])
    );
    if !matches!(exposure, Exposure::Safe) {
        text.push_str(&entity_block(
            "monster_generic",
            [panel_x, 0.0, 0.0],
            0.0,
            &[
                ("targetname", "ohl_panel"),
                ("model", "models/ohl_safety_panel.mdl"),
            ],
        ));
    }
    if matches!(exposure, Exposure::Ally) {
        text.push_str(&entity_block(
            "monster_barney",
            [320.0, 64.0, 0.0],
            0.0,
            &[("targetname", "ohl_ally")],
        ));
    }
    let mut bsp = Bsp30Builder::new();
    bsp.set_entities_text(&text);
    let heads = bsp.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -64.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -(target_x + 160.0)),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -160.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -160.0),
    ]);
    bsp.push_model(
        [-64.0, -160.0, 0.0],
        [target_x + 160.0, 160.0, 512.0],
        [0.0; 3],
        heads,
        1,
        0,
        0,
    );
    let mut assets = crate::MemoryAssets::new();
    let (mdl, _) = build_minimal_mdl10_with_hitbox([-8.0, -24.0, 0.0], [8.0, 24.0, 480.0]);
    assets.insert("models/ohl_safety_panel.mdl", mdl);
    let mut game =
        crate::Game::from_map_bytes(&assets, crate::test_support::AI_MAP, &bsp.build()).unwrap();
    let owner = game.registry().find("ohl_thrower")[0];
    for entity in crate::test_support::monster_entities(&game) {
        game.registry_mut()
            .world
            .insert_one(entity, ScriptHold)
            .unwrap();
    }
    if matches!(exposure, Exposure::Ally) {
        // Explicit directional relationship fixture, not same-class friendliness.
        let ally = game.registry().find("ohl_ally")[0];
        let (level, systems) = game.level_and_systems_mut();
        let source = level.registry.world.get::<&Actor>(owner).unwrap();
        let target = level.registry.world.get::<&Actor>(ally).unwrap();
        assert_ne!(source.classification, target.classification);
        systems.ai_mut().world.relationships_mut().set(
            source.classification,
            target.classification,
            ohl_ai::Relationship::Ally,
        );
        assert_ne!(
            systems
                .ai_mut()
                .world
                .relationships()
                .get(target.classification, source.classification),
            ohl_ai::Relationship::Ally
        );
    }
    game.tick(crate::TICK_SECONDS, &crate::Input::default());
    let (level, systems) = game.level_and_systems_mut();
    for (_, actor) in &mut level.registry.world.query::<(Entity, &Actor)>() {
        assert!(actor.alive && actor.health > 0.0);
        let point = level.collision.as_ref().unwrap().trace(
            actor.hull,
            actor.query_origin(),
            actor.query_origin(),
        );
        assert!(
            !point.start_solid && !point.all_solid,
            "authored bodies are not embedded"
        );
    }
    let request = forced_request(level, owner, systems.ai_mut().difficulty);
    let player_eye = level
        .registry
        .world
        .get::<&Actor>(level.player)
        .unwrap()
        .eye();
    assert!(grenade_lob_clear(
        level.collision.as_ref().unwrap(),
        systems.ai_mut().difficulty,
        request.origin,
        player_eye
    ));
    assert_eq!(game.projectile_count(), 0);
    (game, owner)
}

pub(super) fn safe_scene(kind: &str) -> (crate::Game, Entity) {
    scene(kind, Exposure::Safe)
}

// This uses actual engine spawn/tick/blast dispatch, independently of the safety predicate.
fn forced_exposure(
    game: &mut crate::Game,
    owner: Entity,
    require_rebound: bool,
) -> Vec<QueuedDamage> {
    let panel = game.registry().find("ohl_panel").first().copied();
    let (level, systems) = game.level_and_systems_mut();
    let request = forced_request(level, owner, systems.ai_mut().difficulty);
    let (mut projectiles, hitboxes) = context_parts(level);
    if let Some(panel) = panel {
        assert!(level.registry.world.get::<&StudioAnim>(panel).is_ok());
        assert!(
            hitboxes
                .entries()
                .iter()
                .any(|entry| entry.id == crate::ids::entity_id(panel))
        );
    }
    projectiles.spawn_request(level, &request).unwrap();
    let mut damage = Vec::new();
    let mut sprites = crate::sprites::TransientSprites::default();
    let mut reflected = false;
    for _ in 0..502 {
        let before = projectiles.snapshot(level).projectiles[0];
        let start = Vec3::from_array(before.position);
        let velocity = Vec3::from_array(before.velocity)
            - Vec3::Z * ohl_physics::MoveConfig::default().gravity * crate::TICK_SECONDS;
        let trace = ohl_combat::trace_attack_filtered(
            level.collision.as_ref().unwrap(),
            &hitboxes,
            start,
            start + velocity * crate::TICK_SECONDS,
            ohl_combat::TraceFilter::ignoring(
                ohl_combat::TraceMask::SHOT,
                crate::ids::entity_id(owner),
            ),
        );
        projectiles.tick(
            level,
            &hitboxes,
            crate::TICK_SECONDS,
            &mut damage,
            &mut sprites,
        );
        let state = projectiles.snapshot(level);
        if let Some(after) = state.projectiles.first() {
            assert!(damage.is_empty(), "contact precedes ordinary timed BLAST");
            if velocity.x > 0.0 && after.velocity[0] < 0.0 && !reflected {
                assert_eq!(trace.entity, panel.map(crate::ids::entity_id));
                assert_eq!(trace.surface_normal, Vec3::NEG_X);
                assert!(after.fuse.is_some_and(|fuse| fuse > 0.0));
                reflected = true;
            }
        } else {
            assert!(before.fuse.is_some_and(|fuse| fuse <= crate::TICK_SECONDS));
            assert!(
                !require_rebound || reflected,
                "real posed-face return precedes fuse expiry"
            );
            assert!(damage.iter().all(|d| d.info.kind == DamageType::BLAST));
            return damage;
        }
    }
    panic!("ordinary hand-grenade fuse must complete");
}

pub(super) fn assert_safe_exposure(game: &mut crate::Game, owner: Entity) {
    let player = game.player_entity();
    let hits = forced_exposure(game, owner, false);
    assert!(
        !hits
            .iter()
            .any(|hit| hit.target == owner && hit.info.amount > 0.0),
        "safe owner margin"
    );
    assert!(
        hits.iter()
            .any(|hit| hit.target == player && hit.info.amount > 0.0),
        "real player exposure remains"
    );
}

fn naturally_attempt(game: &mut crate::Game, owner: Entity) -> Vec<ProjectileRequest> {
    let (level, systems) = game.level_and_systems_mut();
    level
        .registry
        .world
        .remove_one::<ScriptHold>(owner)
        .unwrap();
    let actor = *level.registry.world.get::<&Actor>(owner).unwrap();
    let enemy = *level.registry.world.get::<&Actor>(level.player).unwrap();
    {
        let mut brain = level.registry.world.get::<&mut MonsterAi>(owner).unwrap();
        brain.memory = Some(EnemyMemory {
            entity: level.player,
            last_known_position: actor
                .body_frame
                .anchor_to_query(actor.hull, enemy.navigation_anchor()),
            time_since_seen: 0.0,
            occluded: false,
            last_known_distance: actor.eye().distance(enemy.eye()),
        });
        brain.state = MonsterState::Combat;
        brain.runner.clear();
        brain.pending_conditions = Conditions::EMPTY;
    }
    let (projectiles, hitboxes) = context_parts(level);
    let context = GrenadeSafetyContext {
        projectiles: &projectiles,
        hitboxes: &hitboxes,
    };
    let ai = systems.ai_mut();
    ai.update_secondary_opportunities(level, crate::TICK_SECONDS);
    assert!(
        level
            .registry
            .world
            .get::<&MonsterAi>(owner)
            .unwrap()
            .pending_conditions
            .contains(Conditions::CAN_RANGE_ATTACK2),
        "ordinary readiness is eligible before the emission veto"
    );
    for _ in 0..16 {
        ai.think(level, crate::TICK_SECONDS, &mut Vec::new(), &context);
        let requests = ai.take_projectile_requests();
        if !requests.is_empty() {
            return requests;
        }
    }
    Vec::new()
}

#[test]
fn grenade_emission_refuses_actual_owner_return_blast() {
    for kind in ["monster_human_grunt", "monster_human_assassin"] {
        let (mut game, owner) = scene(kind, Exposure::Owner);
        let hits = forced_exposure(&mut game, owner, true);
        assert!(
            hits.iter()
                .any(|hit| hit.target == owner && hit.info.amount > 0.0),
            "forced actual fuse exposes the owner"
        );
        assert!(
            naturally_attempt(&mut game, owner).is_empty(),
            "unsafe owner exposure vetoes emission"
        );
        assert!(
            !game
                .level_and_systems_mut()
                .1
                .ai_mut()
                .secondary_cooldowns
                .contains_key(&owner)
        );
    }
}

#[test]
fn grenade_emission_refuses_ally_exposure_with_owner_clear() {
    let (mut game, owner) = scene("monster_human_grunt", Exposure::Ally);
    let ally = game.registry().find("ohl_ally")[0];
    {
        let (level, systems) = game.level_and_systems_mut();
        let source = level.registry.world.get::<&Actor>(owner).unwrap();
        let target = level.registry.world.get::<&Actor>(ally).unwrap();
        assert_eq!(
            systems
                .ai_mut()
                .world
                .relationships()
                .get(source.classification, target.classification),
            ohl_ai::Relationship::Ally
        );
    }
    let hits = forced_exposure(&mut game, owner, true);
    assert!(
        !hits
            .iter()
            .any(|hit| hit.target == owner && hit.info.amount > 0.0),
        "owner is independently outside exposure"
    );
    assert!(
        hits.iter()
            .any(|hit| hit.target == ally && hit.info.amount > 0.0),
        "forced actual fuse exposes the ally"
    );
    assert!(
        naturally_attempt(&mut game, owner).is_empty(),
        "unsafe Ally exposure vetoes emission"
    );
    assert!(
        !game
            .level_and_systems_mut()
            .1
            .ai_mut()
            .secondary_cooldowns
            .contains_key(&owner)
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn grenade_emission_safe_throw_preserves_cooldown_fuse_and_player_damage() {
    for kind in ["monster_human_grunt", "monster_human_assassin"] {
        let (mut game, owner) = safe_scene(kind);
        assert_safe_exposure(&mut game, owner);
        let expected = {
            let (level, systems) = game.level_and_systems_mut();
            forced_request(level, owner, systems.ai_mut().difficulty)
        };
        let requests = naturally_attempt(&mut game, owner);
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        assert_eq!(request.kind, expected.kind);
        assert_eq!(request.owner, expected.owner);
        assert_eq!(request.origin, expected.origin);
        assert_eq!(request.velocity, expected.velocity);
        assert_eq!(request.damage, expected.damage);
        assert_eq!(request.damage_type, expected.damage_type);
        assert_eq!(request.blast_radius, expected.blast_radius);
        assert_eq!(request.target, expected.target);
        assert_eq!(
            game.level_and_systems_mut()
                .1
                .ai_mut()
                .secondary_cooldowns
                .get(&owner),
            Some(&6.0)
        );
        game.registry_mut()
            .world
            .insert_one(owner, ScriptHold)
            .unwrap();
        game.level_and_systems_mut()
            .1
            .ai_mut()
            .projectiles
            .spawn_projectile(&request);
        let health = game.player_health();
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_eq!(game.projectile_count(), 1);
        assert_eq!(game.player_health(), health);
        let snapshot = game.to_save(0).projectiles.unwrap().projectiles[0];
        assert_eq!(snapshot.age, 0.0);
        assert_eq!(
            snapshot.fuse,
            Some(ohl_combat::projectile::HAND_GRENADE_FUSE_SECONDS)
        );
        for _ in 0..530 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
        }
        assert_eq!(game.projectile_count(), 0);
        assert!(
            game.player_health() < health,
            "safe admitted throw still hurts the player"
        );
    }
}
