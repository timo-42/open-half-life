//! The per-tick drivers for the boss and aircraft components, and their
//! attachment at spawn.
//!
//! [`crate::AiWorld::tick`] runs [`pre_think`] on every monster just before
//! its ordinary sense/decide/schedule/move step. It looks for the
//! components this module knows — a [`GonarchTrail`], a
//! [`NihilanthShield`], a [`FlightPlan`] — and does nothing at all for a
//! monster carrying none, so every other kind's tick is unchanged.
//!
//! What the drivers do is deliberately confined to the same handles a
//! `scripted_sequence` or a follower already uses: they set the route,
//! move target and move speed (which the ordinary movement step then
//! follows, walking a Gonarch and flying an aircraft on its point hull),
//! add a pending [`Conditions`] bit the kind's brain reads
//! ([`Conditions::SPECIAL1`] for "travelling the trail",
//! [`Conditions::SPECIAL2`] for "not yet activated"), and push
//! [`AiEventKind`]s for the host to act on. While a script holds the
//! monster ([`ScriptHold`]) the script owns its route, and the drivers
//! leave it alone. Health is never touched here:
//! [`crate::monsters::lifecycle`] does that, asking the same components
//! whether a hit gets through.
//!
//! A `use` of the monster's own name reaches it through
//! [`MonsterActivation`], the counter `ohl_game::logic::Simulation`
//! bumps like it opens a door: it activates a Nihilanth, starts a `Start
//! Inactive` aircraft, and ends a Gonarch's indefinite wait.
//!
//! [`attach`] is the spawn-time half, called by
//! [`crate::spawn::attach_monsters`] once the monster's `Actor`/`MonsterAi`
//! exist: it builds the Gonarch's trail from its `netname`, an aircraft's
//! flight plan from its `target`, and the Nihilanth's shield from its
//! health.

use hecs::{Entity, World};
use ohl_game::registry::MonsterActivation;
use ohl_game::{EntityDef, PathChain, Registry};

use crate::movement::Route;
use crate::scripts::ScriptHold;
use crate::state::Conditions;
use crate::world::{Actor, AiEvent, AiEventKind, MonsterAi};

use super::aircraft::{FLIGHT_SPEED, FlightOrder, FlightPlan};
use super::bigmomma::{FIRST_NODE_KEY, GonarchTrail, Trail};
use super::nihilanth::{NihilanthCrystal, NihilanthShield};
use super::table::{
    BIGMOMMA_HEALTH_FACTOR, Difficulty, MonsterKind, NIHILANTH_SPHERE_COUNT,
    SPAWNFLAG_AIRCRAFT_START_INACTIVE,
};

/// Attaches whichever boss/aircraft component `def`'s kind needs onto
/// `entity`, which already carries its `Actor` and `MonsterAi`, together
/// with the [`MonsterActivation`] counter a `use` of it bumps.
///
/// `difficulty` scales the Gonarch's node health; `defs` is the whole
/// level's entity list (the trail nodes live there); `registry` resolves
/// an aircraft's `path_corner` chain. Kinds with nothing to attach are
/// left alone.
pub fn attach(
    registry: &mut Registry,
    entity: Entity,
    def: &EntityDef,
    defs: &[EntityDef],
    difficulty: Difficulty,
) {
    let kind = MonsterKind::from_classname(&def.classname);
    let health = registry
        .world
        .get::<&Actor>(entity)
        .map_or(0.0, |actor| actor.health);
    match kind {
        MonsterKind::BigMomma => {
            let trail = def
                .keyvalues
                .get(FIRST_NODE_KEY)
                .and_then(|first| Trail::from_defs(defs, first))
                .unwrap_or_default();
            let factor = BIGMOMMA_HEALTH_FACTOR[difficulty.index()];
            registry
                .world
                .insert(
                    entity,
                    (
                        GonarchTrail::new(trail, factor, health),
                        MonsterActivation::default(),
                    ),
                )
                .ok();
        }
        MonsterKind::Nihilanth => {
            let crystals = registry.world.query::<&NihilanthCrystal>().iter().count();
            let shield = NihilanthShield::for_health(
                health,
                NIHILANTH_SPHERE_COUNT,
                u32::try_from(crystals).unwrap_or(u32::MAX),
            );
            registry
                .world
                .insert(entity, (shield, MonsterActivation::default()))
                .ok();
        }
        MonsterKind::Apache | MonsterKind::Osprey => {
            let plan = def
                .target
                .as_deref()
                .map(str::trim)
                .filter(|target| !target.is_empty())
                .and_then(|target| PathChain::build_with_reentry(registry, target, 0.0))
                .map_or_else(
                    || FlightPlan::new(Vec::new(), false, FLIGHT_SPEED),
                    |(chain, reentry)| FlightPlan::from_chain(&chain, reentry, FLIGHT_SPEED),
                );
            let plan = if def.spawnflags & SPAWNFLAG_AIRCRAFT_START_INACTIVE != 0 {
                plan.starting_inactive()
            } else {
                plan
            };
            registry
                .world
                .insert(entity, (plan, MonsterActivation::default()))
                .ok();
        }
        _ => {}
    }
}

/// Whether `kind`'s `netname` names its trail rather than a squad.
#[must_use]
pub fn netname_is_not_a_squad(kind: &MonsterKind) -> bool {
    matches!(kind, MonsterKind::BigMomma)
}

fn push(events: &mut Vec<AiEvent>, entity: Entity, kind: AiEventKind) {
    if events.len() < crate::world::MAX_EVENTS_PER_TICK {
        events.push(AiEvent { entity, kind });
    }
}

/// Whether `entity` counts as alive for the crystal count: an `Actor` that
/// is alive, or (for a tagged map entity with no `Actor`) still present.
fn is_alive(world: &World, entity: Entity) -> bool {
    match world.get::<&Actor>(entity) {
        Ok(actor) => actor.alive,
        Err(_) => world.contains(entity),
    }
}

/// Runs before `entity`'s ordinary think step; `speeds` is its brain's
/// `(walk, run)`.
pub fn pre_think(
    world: &mut World,
    entity: Entity,
    dt: f32,
    speeds: (f32, f32),
    events: &mut Vec<AiEvent>,
) {
    let used = world
        .get::<&mut MonsterActivation>(entity)
        .map_or(0, |mut activation| activation.take());
    if used > 0 {
        activate(world, entity);
    }
    let scripted = world.get::<&ScriptHold>(entity).is_ok();
    drive_trail(world, entity, dt, speeds, scripted, events);
    drive_shield(world, entity, dt);
    drive_flight(world, entity, dt, scripted, events);
}

/// What a `use` of the monster's own name does: activates a Nihilanth,
/// starts an aircraft, and ends a Gonarch's indefinite wait.
fn activate(world: &mut World, entity: Entity) {
    if let Ok(mut shield) = world.get::<&mut NihilanthShield>(entity) {
        shield.activate();
    }
    if let Ok(mut plan) = world.get::<&mut FlightPlan>(entity) {
        plan.activate();
    }
    if let Ok(mut trail) = world.get::<&mut GonarchTrail>(entity) {
        let _ = trail.release();
    }
}

fn drive_trail(
    world: &mut World,
    entity: Entity,
    dt: f32,
    speeds: (f32, f32),
    scripted: bool,
    events: &mut Vec<AiEvent>,
) {
    let Ok(mut trail) = world.get::<&mut GonarchTrail>(entity) else {
        return;
    };
    let Ok(mut actor) = world.get::<&mut Actor>(entity) else {
        return;
    };
    let Ok(mut ai) = world.get::<&mut MonsterAi>(entity) else {
        return;
    };
    if !actor.alive {
        return;
    }
    trail.tick(dt);
    if scripted {
        // A `reachsequence` (or any other script) has the Gonarch: its
        // route is the script's until it lets go.
        return;
    }
    // Reaching the node, or a leg that has stalled (`GonarchTrail::
    // note_progress`, a project-authored fallback so a node the Gonarch
    // cannot reach never leaves it shielded for good).
    let stalled = trail.note_progress(actor.origin, dt);
    if (trail.has_reached_destination(actor.origin) || stalled)
        && let Some(effects) = trail.arrive()
    {
        if let Some(health) = effects.set_health {
            actor.health = health;
        }
        if let Some(name) = effects.fire {
            push(events, entity, AiEventKind::FireTarget(name));
        }
        if let Some(name) = effects.kill {
            push(events, entity, AiEventKind::KillTarget(name));
        }
        if let Some(name) = effects.sequence {
            push(events, entity, AiEventKind::ScriptRequested(name));
        }
        ai.route = Route::new();
        ai.move_speed = 0.0;
        ai.stuck.reset();
    }
    if let Some(destination) = trail.destination() {
        // Travelling: own the route the way a script does, so no sound or
        // enemy can redirect it, and tell the brain not to fight.
        ai.move_target = Some(destination);
        if ai.route.is_finished() || (ai.route.goal - destination).length() > 1.0 {
            ai.route = Route::straight_line(destination);
            ai.stuck.reset();
        }
        ai.move_speed = if trail.runs_to_destination() {
            speeds.1
        } else {
            speeds.0
        };
        ai.pending_conditions |= Conditions::SPECIAL1;
    }
}

fn drive_shield(world: &mut World, entity: Entity, dt: f32) {
    if world.get::<&NihilanthShield>(entity).is_err() {
        return;
    }
    let standing = world
        .query::<(Entity, &NihilanthCrystal)>()
        .iter()
        .filter(|(crystal, _)| is_alive(world, *crystal))
        .count();
    let Ok(mut shield) = world.get::<&mut NihilanthShield>(entity) else {
        return;
    };
    let Ok(mut ai) = world.get::<&mut MonsterAi>(entity) else {
        return;
    };
    shield.sync_crystals(u32::try_from(standing).unwrap_or(u32::MAX));
    shield.tick(dt);
    if !shield.is_active() {
        ai.pending_conditions |= Conditions::SPECIAL2;
    }
}

fn drive_flight(
    world: &mut World,
    entity: Entity,
    dt: f32,
    scripted: bool,
    events: &mut Vec<AiEvent>,
) {
    let Ok(mut plan) = world.get::<&mut FlightPlan>(entity) else {
        return;
    };
    let Ok(actor) = world.get::<&Actor>(entity) else {
        return;
    };
    let Ok(mut ai) = world.get::<&mut MonsterAi>(entity) else {
        return;
    };
    if scripted {
        return;
    }
    if !actor.alive {
        // No wreck fall is modelled: a dead aircraft stops where it died.
        ai.route = Route::new();
        ai.move_speed = 0.0;
        return;
    }
    let step = plan.steer(actor.origin, dt);
    // `TWHL:Path_corner`'s "Fire On Pass": the node just reached fires its
    // `message` by name.
    if let Some(name) = step.arrived_at.and_then(|index| plan.message(index)) {
        push(events, entity, AiEventKind::FireTarget(name.to_string()));
    }
    match step.order {
        FlightOrder::Hold => {
            ai.route = Route::new();
            ai.move_speed = 0.0;
            ai.stuck.reset();
        }
        FlightOrder::FlyTo(destination) => {
            ai.move_target = Some(destination);
            if ai.route.is_finished() || (ai.route.goal - destination).length() > 1.0 {
                ai.route = Route::straight_line(destination);
                ai.stuck.reset();
            }
            ai.move_speed = plan.speed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{attach, netname_is_not_a_squad, pre_think};
    use crate::monsters::aircraft::{FLIGHT_SPEED, FlightPlan};
    use crate::monsters::bigmomma::{GonarchTrail, TrailPhase};
    use crate::monsters::nihilanth::{NihilanthCrystal, NihilanthShield};
    use crate::monsters::table::{Difficulty, MonsterKind};
    use crate::scripts::ScriptHold;
    use crate::state::{Classification, Conditions};
    use crate::world::{Actor, AiEventKind, BrainId, MonsterAi};
    use glam::Vec3;
    use hecs::World;
    use ohl_game::keyvalues::{Limits, RenderProps};
    use ohl_game::registry::MonsterActivation;
    use ohl_game::{EntityDef, Registry};
    use std::collections::BTreeMap;

    // Every name and position below is project-authored and synthetic.

    fn def(
        classname: &str,
        name: Option<&str>,
        origin: [f32; 3],
        keys: &[(&str, &str)],
        spawnflags: u32,
    ) -> EntityDef {
        let keyvalues: BTreeMap<String, String> = keys
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        EntityDef {
            classname: classname.to_string(),
            target: keyvalues.get("target").cloned(),
            keyvalues,
            origin,
            angles: [0.0; 3],
            targetname: name.map(str::to_string),
            spawnflags,
            model: None,
            render: RenderProps::default(),
        }
    }

    fn registry_with(defs: &[EntityDef]) -> Registry {
        Registry::build(defs, &BTreeMap::new(), &Limits::default())
    }

    fn give_ai(registry: &mut Registry, index: usize, classification: Classification, health: f32) {
        let entity = registry.entities[index];
        let origin = Vec3::from_array(
            registry
                .world
                .get::<&ohl_game::registry::Transform>(entity)
                .map_or([0.0; 3], |transform| transform.origin.to_array()),
        );
        registry
            .world
            .insert(
                entity,
                (
                    Actor::new(classification, origin).with_health(health),
                    MonsterAi::new(BrainId(0)),
                ),
            )
            .expect("live entity");
    }

    fn use_it(world: &World, entity: hecs::Entity) {
        world
            .get::<&mut MonsterActivation>(entity)
            .expect("a monster that waits to be switched on")
            .activate();
    }

    #[test]
    fn the_gonarchs_netname_is_a_trail_and_not_a_squad() {
        assert!(netname_is_not_a_squad(&MonsterKind::BigMomma));
        assert!(!netname_is_not_a_squad(&MonsterKind::HumanGrunt));
    }

    fn gonarch_defs(spawnflags_first_node: u32) -> Vec<EntityDef> {
        vec![
            def("worldspawn", None, [0.0; 3], &[], 0),
            def(
                "monster_bigmomma",
                Some("mother"),
                [0.0, 0.0, 0.0],
                &[("netname", "n1")],
                0,
            ),
            def(
                "info_bigmomma",
                Some("n1"),
                [100.0, 0.0, 0.0],
                &[
                    ("target", "n2"),
                    ("reachtarget", "door_a"),
                    ("killtarget", "crate_b"),
                    ("reachsequence", "seq_c"),
                    ("health", "100"),
                ],
                spawnflags_first_node,
            ),
            def("info_bigmomma", Some("n2"), [200.0, 0.0, 0.0], &[], 0),
        ]
    }

    #[test]
    fn a_gonarch_is_driven_along_its_trail_and_its_arrival_effects_are_reported() {
        let defs = gonarch_defs(1);
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::AlienMonster, 225.0);
        let gonarch = registry.entities[1];
        attach(&mut registry, gonarch, &defs[1], &defs, Difficulty::Medium);
        let world = &mut registry.world;
        {
            let trail = world.get::<&GonarchTrail>(gonarch).expect("attached");
            assert_eq!(trail.trail().len(), 2);
            assert_eq!(trail.phase(), TrailPhase::Traveling { to: 0 });
        }

        // Far away: the driver owns the route, runs (flag 1), and marks it.
        let mut events = Vec::new();
        pre_think(world, gonarch, 0.01, (40.0, 160.0), &mut events);
        {
            let ai = world.get::<&MonsterAi>(gonarch).expect("ai");
            assert!(ai.pending_conditions.contains(Conditions::SPECIAL1));
            assert_eq!(ai.move_target, Some(Vec3::new(100.0, 0.0, 0.0)));
            assert!((ai.move_speed - 160.0).abs() < 1e-6);
            assert_eq!(ai.route.waypoint(), Some(Vec3::new(100.0, 0.0, 0.0)));
        }
        assert!(events.is_empty());

        // Standing on the node: arrival sets the node's health (scaled by
        // the medium factor), reports its three named effects, stops the
        // route. (The ordinary think step consumes pending conditions
        // between ticks; this test stands in for it.)
        world.get::<&mut Actor>(gonarch).expect("actor").origin = Vec3::new(100.0, 0.0, 0.0);
        world
            .get::<&mut MonsterAi>(gonarch)
            .expect("ai")
            .pending_conditions = Conditions::EMPTY;
        let mut events = Vec::new();
        pre_think(world, gonarch, 0.01, (40.0, 160.0), &mut events);
        let kinds: Vec<&AiEventKind> = events.iter().map(|event| &event.kind).collect();
        assert_eq!(
            kinds,
            [
                &AiEventKind::FireTarget("door_a".to_string()),
                &AiEventKind::KillTarget("crate_b".to_string()),
                &AiEventKind::ScriptRequested("seq_c".to_string()),
            ]
        );
        let actor = *world.get::<&Actor>(gonarch).expect("actor");
        assert!((actor.health - 150.0).abs() < 1e-6);
        let ai = world.get::<&MonsterAi>(gonarch).expect("ai");
        assert!(!ai.pending_conditions.contains(Conditions::SPECIAL1));
        assert!((ai.move_speed).abs() < 1e-6);
        assert_eq!(
            world.get::<&GonarchTrail>(gonarch).expect("trail").phase(),
            TrailPhase::Holding { at: 0 }
        );
    }

    /// A script holding the Gonarch owns its route: the trail driver
    /// neither steers it nor counts it as arriving until the script lets
    /// go.
    #[test]
    fn a_scripted_gonarch_is_left_to_its_script() {
        let defs = gonarch_defs(0);
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::AlienMonster, 225.0);
        let gonarch = registry.entities[1];
        attach(&mut registry, gonarch, &defs[1], &defs, Difficulty::Easy);
        let world = &mut registry.world;
        world.insert_one(gonarch, ScriptHold).expect("spawned");
        world.get::<&mut Actor>(gonarch).expect("actor").origin = Vec3::new(100.0, 0.0, 0.0);
        let mut events = Vec::new();
        pre_think(world, gonarch, 0.01, (40.0, 160.0), &mut events);
        assert!(events.is_empty());
        {
            let ai = world.get::<&MonsterAi>(gonarch).expect("ai");
            assert!(ai.route.is_finished());
            assert!(ai.pending_conditions.is_empty());
        }
        assert_eq!(
            world.get::<&GonarchTrail>(gonarch).expect("trail").phase(),
            TrailPhase::Traveling { to: 0 }
        );
        world.remove_one::<ScriptHold>(gonarch).expect("held");
        pre_think(world, gonarch, 0.01, (40.0, 160.0), &mut events);
        assert_eq!(events.len(), 3, "arrives once the script lets go");
    }

    /// `Wait Indefinitely` (2) holds the Gonarch on a node with no health
    /// until a `use` of its name releases it.
    #[test]
    fn a_use_of_the_gonarch_ends_an_indefinite_wait() {
        let defs = vec![
            def("worldspawn", None, [0.0; 3], &[], 0),
            def(
                "monster_bigmomma",
                Some("mother"),
                [0.0; 3],
                &[("netname", "a")],
                0,
            ),
            def("info_bigmomma", Some("a"), [0.0; 3], &[("target", "b")], 2),
            def("info_bigmomma", Some("b"), [300.0, 0.0, 0.0], &[], 0),
        ];
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::AlienMonster, 150.0);
        let gonarch = registry.entities[1];
        attach(&mut registry, gonarch, &defs[1], &defs, Difficulty::Easy);
        let world = &mut registry.world;
        let mut events = Vec::new();
        for _ in 0..200 {
            pre_think(world, gonarch, 0.05, (40.0, 160.0), &mut events);
        }
        assert_eq!(
            world.get::<&GonarchTrail>(gonarch).expect("trail").phase(),
            TrailPhase::Holding { at: 0 },
            "ten seconds and still waiting"
        );
        use_it(world, gonarch);
        pre_think(world, gonarch, 0.05, (40.0, 160.0), &mut events);
        assert_eq!(
            world.get::<&GonarchTrail>(gonarch).expect("trail").phase(),
            TrailPhase::Traveling { to: 1 }
        );
        assert_eq!(
            world.get::<&MonsterAi>(gonarch).expect("ai").move_target,
            Some(Vec3::new(300.0, 0.0, 0.0))
        );
    }

    #[test]
    fn a_gonarch_without_a_trail_is_free_and_not_driven() {
        let defs = vec![
            def("worldspawn", None, [0.0; 3], &[], 0),
            def("monster_bigmomma", None, [0.0; 3], &[], 0),
        ];
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::AlienMonster, 150.0);
        let gonarch = registry.entities[1];
        attach(&mut registry, gonarch, &defs[1], &defs, Difficulty::Easy);
        let world = &mut registry.world;
        assert_eq!(
            world
                .get::<&GonarchTrail>(gonarch)
                .expect("attached")
                .phase(),
            TrailPhase::Free
        );
        let mut events = Vec::new();
        pre_think(world, gonarch, 0.01, (40.0, 160.0), &mut events);
        let ai = world.get::<&MonsterAi>(gonarch).expect("ai");
        assert!(ai.pending_conditions.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn a_nihilanth_is_dormant_until_used_and_its_crystals_are_counted_live() {
        let defs = vec![
            def("worldspawn", None, [0.0; 3], &[], 0),
            def("monster_nihilanth", Some("boss"), [0.0, 0.0, 512.0], &[], 0),
            def("func_breakable", Some("crystal_a"), [0.0; 3], &[], 0),
            def("func_breakable", Some("crystal_b"), [0.0; 3], &[], 0),
        ];
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::AlienMilitary, 800.0);
        let boss = registry.entities[1];
        let crystal_a = registry.entities[2];
        let crystal_b = registry.entities[3];
        registry.world.insert_one(crystal_a, NihilanthCrystal).ok();
        registry.world.insert_one(crystal_b, NihilanthCrystal).ok();
        attach(&mut registry, boss, &defs[1], &defs, Difficulty::Hard);
        let world = &mut registry.world;
        {
            let shield = world.get::<&NihilanthShield>(boss).expect("attached");
            assert_eq!(shield.crystals(), 2);
            assert!(!shield.is_active());
            assert!((shield.reserve_capacity() - 800.0).abs() < 1e-4);
        }
        let mut events = Vec::new();
        for _ in 0..100 {
            pre_think(world, boss, 0.1, (0.0, 0.0), &mut events);
        }
        assert!(
            world
                .get::<&MonsterAi>(boss)
                .expect("ai")
                .pending_conditions
                .contains(Conditions::SPECIAL2),
            "dormant however long it waits"
        );

        // Used, with one crystal despawned: active, and the count follows.
        use_it(world, boss);
        world.despawn(crystal_a).ok();
        world
            .get::<&mut MonsterAi>(boss)
            .expect("ai")
            .pending_conditions = Conditions::EMPTY;
        pre_think(world, boss, 0.01, (0.0, 0.0), &mut events);
        assert!(
            !world
                .get::<&MonsterAi>(boss)
                .expect("ai")
                .pending_conditions
                .contains(Conditions::SPECIAL2)
        );
        let shield = world.get::<&NihilanthShield>(boss).expect("shield");
        assert!(shield.is_active());
        assert_eq!(shield.crystals(), 1);
        assert!(events.is_empty());
    }

    /// Two Apaches on the same two-node loop: one flying, one parked by
    /// `Start Inactive` (64). Returns the registry and the pair.
    fn two_apaches() -> (Registry, hecs::Entity, hecs::Entity) {
        let defs = vec![
            def("worldspawn", None, [0.0; 3], &[], 0),
            def(
                "monster_apache",
                None,
                [0.0, 0.0, 256.0],
                &[("target", "p1")],
                0,
            ),
            def(
                "path_corner",
                Some("p1"),
                [500.0, 0.0, 256.0],
                &[("target", "p2"), ("message", "ohl_pass")],
                0,
            ),
            def(
                "path_corner",
                Some("p2"),
                [500.0, 500.0, 300.0],
                &[("target", "p1")],
                0,
            ),
            def(
                "monster_apache",
                Some("parked"),
                [0.0, 0.0, 256.0],
                &[("target", "p1")],
                64,
            ),
        ];
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::HumanMilitary, 250.0);
        give_ai(&mut registry, 4, Classification::HumanMilitary, 250.0);
        let flying = registry.entities[1];
        let parked = registry.entities[4];
        attach(&mut registry, flying, &defs[1], &defs, Difficulty::Medium);
        attach(&mut registry, parked, &defs[4], &defs, Difficulty::Medium);
        (registry, flying, parked)
    }

    /// The flight driver hands the movement step a route at the plan's
    /// speed; a `Start Inactive` aircraft gets none until it is used.
    #[test]
    fn an_apache_is_given_its_route_and_start_inactive_parks_it_until_used() {
        let (mut registry, flying, parked) = two_apaches();
        let world = &mut registry.world;
        {
            let plan = world.get::<&FlightPlan>(flying).expect("attached");
            assert_eq!(plan.waypoints().len(), 2);
            assert!(plan.is_looped());
            assert!(plan.is_active());
            assert!(
                !world
                    .get::<&FlightPlan>(parked)
                    .expect("attached")
                    .is_active()
            );
        }
        let mut events = Vec::new();
        pre_think(world, flying, 0.01, (0.0, 0.0), &mut events);
        pre_think(world, parked, 0.01, (0.0, 0.0), &mut events);
        {
            let ai = world.get::<&MonsterAi>(flying).expect("ai");
            assert_eq!(ai.route.waypoint(), Some(Vec3::new(500.0, 0.0, 256.0)));
            assert!((ai.move_speed - FLIGHT_SPEED).abs() < 1e-6);
            let still = world.get::<&MonsterAi>(parked).expect("ai");
            assert!(still.route.is_finished());
            assert!(still.move_speed.abs() < 1e-6);
        }
        use_it(world, parked);
        pre_think(world, parked, 0.01, (0.0, 0.0), &mut events);
        let started = world.get::<&MonsterAi>(parked).expect("ai");
        assert_eq!(started.route.waypoint(), Some(Vec3::new(500.0, 0.0, 256.0)));
        assert!(events.is_empty());
    }

    /// Reaching a node moves the order on to the next one and fires the
    /// node's fire-on-pass `message`, and a dead aircraft is given no
    /// route at all.
    #[test]
    fn an_apache_moves_on_at_each_node_and_stops_when_dead() {
        let (mut registry, flying, _) = two_apaches();
        let world = &mut registry.world;
        world.get::<&mut Actor>(flying).expect("actor").origin = Vec3::new(500.0, 0.0, 256.0);
        let mut events = Vec::new();
        pre_think(world, flying, 0.01, (0.0, 0.0), &mut events);
        assert_eq!(
            world
                .get::<&MonsterAi>(flying)
                .expect("ai")
                .route
                .waypoint(),
            Some(Vec3::new(500.0, 500.0, 300.0))
        );
        let fired: Vec<&AiEventKind> = events.iter().map(|event| &event.kind).collect();
        assert_eq!(
            fired,
            [&AiEventKind::FireTarget("ohl_pass".to_string())],
            "passing p1 fires its message"
        );
        world.get::<&mut Actor>(flying).expect("actor").alive = false;
        pre_think(world, flying, 0.01, (0.0, 0.0), &mut events);
        let ai = world.get::<&MonsterAi>(flying).expect("ai");
        assert!(ai.route.is_finished());
        assert!(ai.move_speed.abs() < 1e-6);
    }

    /// An aircraft whose route leads into a cycle part-way along is given
    /// a plan that goes round that cycle rather than dead-ending.
    #[test]
    fn an_aircraft_on_a_lead_in_route_is_planned_round_its_cycle() {
        let defs = vec![
            def("worldspawn", None, [0.0; 3], &[], 0),
            def("monster_osprey", None, [0.0; 3], &[("target", "p1")], 0),
            def(
                "path_corner",
                Some("p1"),
                [100.0, 0.0, 0.0],
                &[("target", "p2")],
                0,
            ),
            def(
                "path_corner",
                Some("p2"),
                [200.0, 0.0, 0.0],
                &[("target", "p3")],
                0,
            ),
            def(
                "path_corner",
                Some("p3"),
                [300.0, 0.0, 0.0],
                &[("target", "p2")],
                0,
            ),
        ];
        let mut registry = registry_with(&defs);
        give_ai(&mut registry, 1, Classification::Machine, 400.0);
        let osprey = registry.entities[1];
        attach(&mut registry, osprey, &defs[1], &defs, Difficulty::Easy);
        let plan = registry.world.get::<&FlightPlan>(osprey).expect("attached");
        assert_eq!(plan.waypoints().len(), 3);
        assert_eq!(plan.loop_to(), Some(1));
    }

    #[test]
    fn a_monster_with_no_boss_component_is_left_entirely_alone() {
        let mut world = World::new();
        let monster = world.spawn((
            Actor::new(Classification::AlienPrey, Vec3::ONE),
            MonsterAi::new(BrainId(0)),
        ));
        let mut events = Vec::new();
        pre_think(&mut world, monster, 0.01, (40.0, 160.0), &mut events);
        assert!(events.is_empty());
        let ai = world.get::<&MonsterAi>(monster).expect("ai");
        assert!(ai.pending_conditions.is_empty());
        assert!(ai.route.is_finished());
        assert_eq!(
            world.get::<&Actor>(monster).expect("actor").origin,
            Vec3::ONE
        );
    }
}
