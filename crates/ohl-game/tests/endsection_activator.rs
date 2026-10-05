//! Project-authored dispatch fixtures for the published player-only end rule.
//! TWHL: https://twhl.info/wiki/page/trigger_endsection . No retail inputs.

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_formats::bsp30::Entity as RawEntity;
use ohl_game::effects::EffectPlayer;
use ohl_game::hecs::Entity;
use ohl_game::keyvalues::{Limits, parse_entities};
use ohl_game::registry::{ClassName, Door, EndSection, MoverState, Registry};
use ohl_game::{Event, Simulation};

fn raw(pairs: &[(&str, &str)]) -> RawEntity {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

struct Fixture {
    registry: Registry,
    sim: Simulation,
    player: Entity,
    monster: Entity,
    fake_player: Entity,
    end: Entity,
}

impl Fixture {
    fn new(extra: Vec<RawEntity>) -> Self {
        Self::build(extra, "0")
    }

    fn build(extra: Vec<RawEntity>, flags: &str) -> Self {
        let mut entities = vec![
            raw(&[
                ("classname", "trigger_endsection"),
                ("targetname", "end"),
                ("section", "ohl_test_section"),
                ("model", "*1"),
                ("wait", "10"),
                ("spawnflags", flags),
                ("target", "touch_proof"),
            ]),
            raw(&[("classname", "func_door"), ("targetname", "touch_proof")]),
        ];
        entities.extend(extra);
        let bounds = BTreeMap::from([(1, ([-32.0; 3], [32.0; 3])), (2, ([-8.0; 3], [8.0; 3]))]);
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let end = registry.find("end")[0];
        let player = registry.world.spawn((ClassName("player".to_string()),));
        let monster = registry
            .world
            .spawn((ClassName("monster_scientist".to_string()),));
        let fake_player = registry.world.spawn((ClassName("player".to_string()),));
        let mut sim = Simulation::new();
        sim.set_effect_player(Some(player_facts(player)));
        Self {
            registry,
            sim,
            player,
            monster,
            fake_player,
            end,
        }
    }

    fn use_end(&mut self, actor: Option<Entity>) -> Vec<Event> {
        let mut events = Vec::new();
        self.sim
            .use_entity(&mut self.registry, self.end, actor, &mut events);
        events
    }

    fn touch(&mut self, actor: Entity, events: &mut Vec<Event>) -> u32 {
        self.sim.touch_triggers(
            &mut self.registry,
            Vec3::splat(-8.0),
            Vec3::splat(8.0),
            Some(actor),
            events,
        )
    }
}

fn player_facts(entity: Entity) -> EffectPlayer {
    EffectPlayer {
        entity,
        origin: Vec3::ZERO,
        grounded: true,
    }
}

fn end_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::EndSection(_)))
        .count()
}

#[test]
fn named_use_rejects_nonplayers_then_accepts_the_same_end_for_the_player() {
    let mut f = Fixture::new(vec![]);
    let ordinary = f
        .registry
        .world
        .spawn((ClassName("info_target".to_string()),));
    let stale = f.registry.world.spawn(());
    f.registry.world.despawn(stale).unwrap();
    for actor in [
        Some(f.monster),
        Some(ordinary),
        Some(f.fake_player),
        Some(stale),
        None,
    ] {
        assert_eq!(
            end_count(&f.use_end(actor)),
            0,
            "nonplayer use ended the section"
        );
        assert_eq!(
            end_count(&f.use_end(Some(f.player))),
            1,
            "the same end must still accept its player"
        );
    }
}

#[test]
fn absent_stale_and_nonfinite_host_identity_fail_closed() {
    let mut f = Fixture::new(vec![]);
    f.sim.set_effect_player(None);
    assert_eq!(end_count(&f.use_end(Some(f.player))), 0);
    let mut invalid = player_facts(f.player);
    invalid.origin.x = f32::NAN;
    f.sim.set_effect_player(Some(invalid));
    assert_eq!(end_count(&f.use_end(Some(f.player))), 0);
    f.sim.set_effect_player(Some(player_facts(f.player)));
    assert_eq!(end_count(&f.use_end(Some(f.player))), 1);
    let former_player = f.player;
    f.registry.world.despawn(former_player).unwrap();
    assert_eq!(end_count(&f.use_end(Some(former_player))), 0);
    f.player = f.registry.world.spawn(());
    f.sim.set_effect_player(Some(player_facts(f.player)));
    assert_eq!(end_count(&f.use_end(Some(f.player))), 1);
}

#[test]
fn rejected_touch_leaves_the_same_volume_ready_for_the_player_immediately() {
    let mut f = Fixture::new(vec![]);
    let mut events = Vec::new();
    let rejected = f.touch(f.monster, &mut events);
    let rejected_events = end_count(&events);
    let queued_after_rejection = f.sim.snapshot().pending.len();
    // No tick clears cooldown before this real player dispatch.
    let accepted = f.touch(f.player, &mut events);
    assert_eq!(
        accepted, 1,
        "the rejected toucher spent the player's cooldown"
    );
    assert_eq!(
        end_count(&events),
        1,
        "the player touch must end exactly once"
    );
    assert_eq!(rejected, 0);
    assert_eq!(rejected_events, 0);
    assert_eq!(
        queued_after_rejection, 0,
        "a rejected toucher scheduled a target"
    );
    events.extend(f.sim.tick(&mut f.registry, 0.05));
    let proof = f.registry.find("touch_proof")[0];
    assert_ne!(
        f.registry.world.get::<&Door>(proof).unwrap().state,
        MoverState::Closed
    );
}

fn chain(kind: &str, ends: bool) {
    let intermediary = if kind == "multi_manager" {
        raw(&[
            ("classname", kind),
            ("targetname", "middle"),
            ("end", "0.2"),
        ])
    } else {
        raw(&[
            ("classname", kind),
            ("targetname", "middle"),
            ("target", "end"),
            ("delay", "0.2"),
        ])
    };
    let mut f = Fixture::build(
        vec![
            raw(&[
                ("classname", "trigger_once"),
                ("targetname", "entry"),
                ("target", "middle"),
                ("model", "*2"),
            ]),
            intermediary,
            // The same named fire must open this ordinary mixed target, proving
            // that rejection of EndSection did not suppress the whole chain.
            raw(&[
                ("classname", "func_door"),
                ("targetname", "end"),
                ("wait", "-1"),
            ]),
        ],
        "1",
    );
    let mut events = Vec::new();
    assert_eq!(f.touch(f.player, &mut events), 1);
    events.extend(f.sim.tick(&mut f.registry, 0.05));
    assert_eq!(end_count(&events), 0, "the chain must honor its delay");
    for _ in 0..10 {
        events.extend(f.sim.tick(&mut f.registry, 0.05));
    }
    let proof = f
        .registry
        .find("end")
        .iter()
        .copied()
        .find(|entity| f.registry.world.get::<&EndSection>(*entity).is_err())
        .unwrap();
    assert_ne!(
        f.registry.world.get::<&Door>(proof).unwrap().state,
        MoverState::Closed,
        "the intermediary never dispatched its ordinary target"
    );
    assert_eq!(
        end_count(&events),
        usize::from(ends),
        "wrong end-section actor after intermediary dispatch"
    );
    assert_eq!(
        end_count(&f.use_end(Some(f.player))),
        1,
        "the same end must still accept direct player activation"
    );
}

#[test]
fn player_manager_chain_preserves_player_identity() {
    chain("multi_manager", true);
}

#[test]
fn queued_named_fire_preserves_player_identity_without_an_intermediary() {
    let mut f = Fixture::new(vec![]);
    f.sim.fire("end", Some(f.monster), 0.1);
    assert_eq!(end_count(&f.sim.tick(&mut f.registry, 0.2)), 0);
    f.sim.fire("end", Some(f.player), 0.1);
    assert_eq!(end_count(&f.sim.tick(&mut f.registry, 0.05)), 0);
    assert_eq!(end_count(&f.sim.tick(&mut f.registry, 0.1)), 1);
}

#[test]
fn a_button_preserves_the_player_for_its_delayed_end_target() {
    let mut f = Fixture::build(
        vec![raw(&[
            ("classname", "func_button"),
            ("targetname", "button"),
            ("target", "end"),
            ("delay", "0.2"),
        ])],
        "1",
    );
    let button = f.registry.find("button")[0];
    let mut events = Vec::new();
    f.sim
        .use_entity(&mut f.registry, button, Some(f.player), &mut events);
    events.extend(f.sim.tick(&mut f.registry, 0.05));
    assert_eq!(end_count(&events), 0, "the button must honor its delay");
    for _ in 0..10 {
        events.extend(f.sim.tick(&mut f.registry, 0.05));
    }
    assert_eq!(
        end_count(&events),
        1,
        "the button must retain its player's identity"
    );
}
