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

// A separate wider authored room: the old safety controls above remain exact.
fn live_snapshot_scene() -> (crate::Game, Entity, Entity) {
    use crate::test_support::entity_block;
    let text = format!(
        "{{\"classname\" \"worldspawn\"}}{}{}{}",
        entity_block("info_player_start", [800.0, 0.0, 36.0], 180.0, &[]),
        entity_block(
            "monster_human_grunt",
            [0.0; 3],
            0.0,
            &[("targetname", "snapshot_owner")]
        ),
        entity_block(
            "monster_barney",
            [800.0, 400.0, 0.0],
            0.0,
            &[("targetname", "snapshot_ally")]
        ),
    );
    let mut bsp = Bsp30Builder::new();
    bsp.set_entities_text(&text);
    let heads = bsp.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -64.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -960.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
    ]);
    bsp.push_model(
        [-64.0, -512.0, 0.0],
        [960.0, 512.0, 512.0],
        [0.0; 3],
        heads,
        1,
        0,
        0,
    );
    let mut game = crate::Game::from_map_bytes(
        &crate::MemoryAssets::new(),
        crate::test_support::AI_MAP,
        &bsp.build(),
    )
    .unwrap();
    let owner = game.registry().find("snapshot_owner")[0];
    let ally = game.registry().find("snapshot_ally")[0];
    for entity in [owner, ally] {
        game.registry_mut()
            .world
            .insert_one(entity, ScriptHold)
            .unwrap();
    }
    game.registry_mut()
        .world
        .insert_one(ally, Prisoner)
        .unwrap();
    let owner_class = game
        .registry()
        .world
        .get::<&Actor>(owner)
        .unwrap()
        .classification;
    let ally_class = game
        .registry()
        .world
        .get::<&Actor>(ally)
        .unwrap()
        .classification;
    assert_ne!(owner_class, ally_class);
    game.systems_mut().ai_mut().world.relationships_mut().set(
        owner_class,
        ally_class,
        ohl_ai::Relationship::Ally,
    );
    game.tick(crate::TICK_SECONDS, &crate::Input::default());
    assert_eq!(game.projectile_count(), 0);
    (game, owner, ally)
}

// Explicit forced physics characterization, using the real ordinary spawn/fuse
// and queued typed blast. It applies no hits and advances no actor or live pool.
fn live_snapshot_exposure(level: &mut Level, request: &ProjectileRequest) -> Vec<QueuedDamage> {
    let (mut projectiles, hitboxes) = context_parts(level);
    projectiles.spawn_request(level, request).unwrap();
    let mut damage = Vec::new();
    let mut sprites = crate::sprites::TransientSprites::default();
    for _ in 0..502 {
        let before = projectiles.snapshot(level).projectiles[0];
        assert!(
            before
                .fuse
                .is_some_and(|left| left.is_finite() && left > 0.0)
        );
        assert!(before.position[1].abs() <= ohl_physics::DIST_EPSILON);
        projectiles.tick(
            level,
            &hitboxes,
            crate::TICK_SECONDS,
            &mut damage,
            &mut sprites,
        );
        if projectiles.snapshot(level).projectiles.is_empty() {
            assert!(before.fuse.unwrap() <= crate::TICK_SECONDS);
            assert!(damage.iter().all(|hit| hit.info.kind == DamageType::BLAST));
            return damage;
        }
        assert!(damage.is_empty());
    }
    panic!("ordinary finite fuse must complete");
}

fn stage_live_snapshot_throw(game: &mut crate::Game, owner: Entity) {
    use ohl_ai::schedule::Task;
    game.registry_mut()
        .world
        .remove_one::<ScriptHold>(owner)
        .unwrap();
    for _ in 0..32 {
        {
            let ai = game.registry().world.get::<&MonsterAi>(owner).unwrap();
            if ai.runner.task() == Some(Task::RangeAttack2) && !ai.runner.started() {
                assert_eq!(
                    ai.runner.schedule_name(),
                    ohl_ai::monsters::brains::GRUNT_GRENADE.name
                );
                return;
            }
        }
        let (level, systems) = game.level_and_systems_mut();
        let (projectiles, hitboxes) = context_parts(level);
        systems.ai_mut().think(
            level,
            crate::TICK_SECONDS,
            &mut Vec::new(),
            &GrenadeSafetyContext {
                projectiles: &projectiles,
                hitboxes: &hitboxes,
            },
        );
        assert!(
            systems.ai_mut().take_projectile_requests().is_empty(),
            "stop before natural emission"
        );
    }
    panic!("ordinary senses and schedules must reach the grenade task");
}

fn snapshot_body_clear(level: &Level, actor: &Actor) {
    let world = level.collision.as_ref().unwrap();
    let origin = actor.query_origin();
    let body = world.trace(actor.hull, origin, origin);
    assert!(!body.start_solid && !body.all_solid);
    let floor = world.trace(actor.hull, origin, origin - Vec3::Z);
    assert!(!floor.start_solid && !floor.all_solid && floor.fraction < 1.0);
}

// The Ally's existing FLEE executor/route is explicit generated setup. The
// owner acquires its ordinary schedule; the decisive movement is real world.tick.
#[allow(clippy::too_many_lines)]
fn live_snapshot_attempt(inward: bool) -> (Vec<ProjectileRequest>, Option<f32>, ProjectileRequest) {
    use ohl_ai::schedule::{Brain, Task};
    let (mut game, owner, ally) = live_snapshot_scene();
    let expected = {
        let (level, systems) = game.level_and_systems_mut();
        forced_request(level, owner, systems.ai_mut().difficulty)
    };
    let player = game.player_entity();
    let initial_hits = live_snapshot_exposure(game.level_and_systems_mut().0, &expected);
    let center = initial_hits
        .iter()
        .find(|hit| hit.target == player && hit.info.amount > 0.0)
        .unwrap()
        .info
        .origin;
    assert!(center.is_finite());
    assert!(
        !initial_hits
            .iter()
            .any(|hit| hit.target == owner && hit.info.amount > 0.0)
    );
    let radius = expected.blast_radius.unwrap();
    let speed = MonsterBrain::for_kind(MonsterKind::Barney)
        .unwrap()
        .speeds()
        .1;
    let epsilon = speed * crate::TICK_SECONDS * 0.5;
    let actor = *game.registry().world.get::<&Actor>(ally).unwrap();
    let owner_before = *game.registry().world.get::<&Actor>(owner).unwrap();
    let (min, max) = actor.fallback_damage_bounds();
    let origin = Vec3::new(
        center.x,
        center.y + radius + epsilon - min.y,
        owner_before.origin.z,
    );
    assert!(center.z >= origin.z + min.z && center.z <= origin.z + max.z);
    place(&game, ally, origin);
    stage_live_snapshot_throw(&mut game, owner);
    game.registry_mut()
        .world
        .remove_one::<ScriptHold>(ally)
        .unwrap();
    let old = *game.registry().world.get::<&Actor>(ally).unwrap();
    let direction = if inward { Vec3::NEG_Y } else { Vec3::Y };
    {
        let mut ai = game.registry().world.get::<&mut MonsterAi>(ally).unwrap();
        ai.runner = ScheduleRunner::restore(ohl_ai::brain::FLEE.name, 5, false, 0.0);
        assert_eq!(ai.runner.task(), Some(Task::WaitForMovement));
        ai.route = Route::straight_line(old.query_origin() + direction * 64.0);
        ai.move_speed = speed;
        assert!(ai.pending_conditions.is_empty());
        assert!(ai.memory.is_none());
    }
    let (level, systems) = game.level_and_systems_mut();
    snapshot_body_clear(level, &old);
    let sweep = level.collision.as_ref().unwrap().trace(
        old.hull,
        old.query_origin(),
        old.query_origin() + direction * speed * crate::TICK_SECONDS,
    );
    assert!(!sweep.start_solid && !sweep.all_solid && sweep.fraction >= 1.0);
    let (projectiles, hitboxes) = context_parts(level);
    let old_entry = hitboxes
        .entries()
        .iter()
        .find(|entry| entry.id == crate::ids::entity_id(ally))
        .unwrap()
        .clone();
    assert_eq!(old_entry.origin, old.origin);
    assert!(projectiles.human_grenade_safe(
        level,
        &expected,
        &hitboxes,
        systems.ai().world.relationships()
    ));
    let player_before = *level.registry.world.get::<&Actor>(player).unwrap();
    systems.ai_mut().think(
        level,
        crate::TICK_SECONDS,
        &mut Vec::new(),
        &GrenadeSafetyContext {
            projectiles: &projectiles,
            hitboxes: &hitboxes,
        },
    );
    let requests = systems.ai_mut().take_projectile_requests();
    let current = *level.registry.world.get::<&Actor>(ally).unwrap();
    assert!(
        (current.origin - old.origin).dot(direction) > epsilon,
        "actual ordinary movement crosses the edge"
    );
    assert_eq!(
        level.registry.world.get::<&Transform>(ally).unwrap().origin,
        old.origin
    );
    assert_eq!(
        hitboxes
            .entries()
            .iter()
            .find(|entry| entry.id == old_entry.id)
            .unwrap(),
        &old_entry
    );
    assert_eq!(
        level.registry.world.get::<&Actor>(owner).unwrap().origin,
        owner_before.origin
    );
    assert_eq!(
        level.registry.world.get::<&Actor>(player).unwrap().origin,
        player_before.origin
    );
    assert!(current.alive && current.health > 0.0 && current.origin.is_finite());
    assert!(level.registry.world.get::<&StudioAnim>(ally).is_err());
    assert!(level.registry.world.get::<&ScriptHold>(ally).is_err());
    assert_eq!(
        systems
            .ai()
            .world
            .relationships()
            .get(owner_before.classification, current.classification),
        ohl_ai::Relationship::Ally
    );
    snapshot_body_clear(level, &current);
    let near_y = current.origin.y + min.y - center.y;
    assert!(near_y > 0.0, "body remains outside the projectile lane");
    assert_eq!(near_y < radius, inward);
    let hits = live_snapshot_exposure(level, &expected);
    assert!(
        !hits
            .iter()
            .any(|hit| hit.target == owner && hit.info.amount > 0.0)
    );
    assert!(
        hits.iter()
            .any(|hit| hit.target == player && hit.info.amount > 0.0)
    );
    assert_eq!(
        hits.iter()
            .any(|hit| hit.target == ally && hit.info.amount > 0.0),
        inward,
        "independent current-body ordinary blast exposure"
    );
    let (fresh, fresh_index) = context_parts(level);
    assert_eq!(
        fresh.human_grenade_safe(
            level,
            &expected,
            &fresh_index,
            systems.ai().world.relationships()
        ),
        !inward
    );
    for request in &requests {
        assert_eq!(request, &expected, "same ordinary request fields");
    }
    (
        requests,
        systems.ai().secondary_cooldowns.get(&owner).copied(),
        expected,
    )
}

#[test]
fn grenade_emission_uses_current_actor_snapshot_after_think() {
    let (requests, cooldown, _) = live_snapshot_attempt(true);
    assert!(
        requests.is_empty(),
        "current moved Ally exposure must refuse emission"
    );
    assert!(cooldown.is_none(), "refusal inserts no emission cooldown");
    let (requests, cooldown, expected) = live_snapshot_attempt(false);
    assert_eq!(
        requests.len(),
        1,
        "outward current-safe movement preserves emission"
    );
    assert_eq!(requests[0].owner, expected.owner);
    assert_eq!(cooldown, Some(6.0));
}
