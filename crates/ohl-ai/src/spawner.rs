//! `monstermaker` spawner semantics.
//!
//! Published `monstermaker` keyvalues and spawnflags (see
//! `docs/FORMAT_SOURCES.md`, "Monster definitions"): `monstercount` (total
//! monsters this maker will ever create; `-1` means unlimited),
//! `m_imaxlivechildren` (the largest number of its children allowed alive at
//! once; new spawns wait for one to die once at the cap), a spawn `delay` in
//! seconds between spawns, a `Start On` spawnflag (spawn immediately rather
//! than waiting to be triggered) and a `Cyclic` spawnflag (keep spawning
//! according to `monstercount`/`delay` rather than stopping after one).
//!
//! [`Spawner`] only decides *when* to spawn and *whether room remains*; it
//! does not itself create the `hecs` entity (that needs `ohl-game`'s
//! `Registry`/`MonsterSpawn`, package 7.7's own `spawn` module territory) or
//! know which of its children are still alive on its own — the caller
//! reports both back via [`Spawner::note_spawned`] and the `is_alive`
//! closure passed to [`Spawner::tick`], exactly like
//! [`crate::spawn::MonsterSpawnRules`] already keeps `ohl-ai` from owning a
//! second entity registry.

use hecs::Entity;

/// No cap on live children ([`Spawner::max_live_children`] of `0`) or on the
/// total spawn count ([`Spawner::monstercount`] of `-1`).
pub const UNLIMITED: i32 = -1;

/// A `monstermaker`'s spawn bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawner {
    /// The classname to spawn; carried through rather than interpreted here,
    /// since turning it into a [`crate::spawn::MonsterSpawn`] is package
    /// 7.7's table's job.
    pub monster_classname: String,
    /// Total monsters this maker will ever create; `-1` (`UNLIMITED`) for no
    /// limit.
    pub monstercount: i32,
    /// Seconds between spawns.
    pub delay: f32,
    /// The largest number of this maker's children allowed alive at once;
    /// `0` for no limit.
    pub max_live_children: u32,
    /// Whether it starts spawning immediately rather than waiting for
    /// [`Spawner::trigger`].
    pub start_on: bool,
    /// Whether it keeps spawning (per `monstercount`/`delay`) rather than
    /// stopping once triggered and having spawned one batch.
    pub cyclic: bool,

    active: bool,
    spawned_total: u32,
    timer: f32,
    children: Vec<Entity>,
}

impl Spawner {
    /// A spawner for `monster_classname`, not yet triggered unless
    /// `start_on` is set.
    #[must_use]
    pub fn new(
        monster_classname: impl Into<String>,
        monstercount: i32,
        delay: f32,
        max_live_children: u32,
        start_on: bool,
        cyclic: bool,
    ) -> Self {
        Self {
            monster_classname: monster_classname.into(),
            monstercount,
            delay: delay.max(0.0),
            max_live_children,
            start_on,
            cyclic,
            active: start_on,
            spawned_total: 0,
            timer: 0.0,
            children: Vec::new(),
        }
    }

    /// Starts (or restarts, if [`Self::cyclic`]) spawning, as if a `target`
    /// fired it.
    pub fn trigger(&mut self) {
        if !self.active || self.cyclic {
            self.active = true;
            self.timer = 0.0;
            if self.cyclic {
                self.spawned_total = 0;
            }
        }
    }

    /// How many more monsters this maker will ever create, or `None` for
    /// unlimited.
    #[must_use]
    pub fn remaining(&self) -> Option<u32> {
        if self.monstercount < 0 {
            None
        } else {
            Some(
                u32::try_from(self.monstercount)
                    .unwrap_or(0)
                    .saturating_sub(self.spawned_total),
            )
        }
    }

    /// The number of this maker's children currently reported alive.
    #[must_use]
    pub fn live_children(&self) -> usize {
        self.children.len()
    }

    /// Whether room remains for one more live child.
    #[must_use]
    pub fn has_room(&self) -> bool {
        self.max_live_children == 0 || self.children.len() < self.max_live_children as usize
    }

    /// Drops any tracked child `is_alive` reports as gone.
    pub fn prune_dead(&mut self, is_alive: &dyn Fn(Entity) -> bool) {
        self.children.retain(|&entity| is_alive(entity));
    }

    /// Records that `entity` was just spawned by this maker.
    pub fn note_spawned(&mut self, entity: Entity) {
        self.spawned_total += 1;
        self.children.push(entity);
        if self.remaining() == Some(0) && !self.cyclic {
            self.active = false;
        }
    }

    /// Advances the spawn timer by `dt` seconds and, first pruning dead
    /// children via `is_alive`, reports whether a spawn should happen now.
    ///
    /// The caller must call [`Self::note_spawned`] with the resulting
    /// entity when it does spawn one, so the next call sees an accurate
    /// live-child count and total.
    #[must_use]
    pub fn tick(&mut self, dt: f32, is_alive: &dyn Fn(Entity) -> bool) -> bool {
        self.prune_dead(is_alive);
        if !self.active {
            return false;
        }
        if self.remaining() == Some(0) {
            self.active = self.cyclic;
            return false;
        }
        if !self.has_room() {
            return false;
        }
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        self.timer -= dt;
        if self.timer > 0.0 {
            return false;
        }
        self.timer = self.delay;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Spawner, UNLIMITED};
    use hecs::World;

    fn alive_in(world: &World) -> impl Fn(hecs::Entity) -> bool + '_ {
        move |entity| world.contains(entity)
    }

    #[test]
    fn a_start_on_spawner_spawns_immediately_then_waits_for_delay() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 3, 1.0, 0, true, false);
        assert!(spawner.tick(0.0, &alive_in(&world)));
        let e1 = world.spawn(());
        spawner.note_spawned(e1);
        assert!(!spawner.tick(0.5, &alive_in(&world)));
        assert!(spawner.tick(0.6, &alive_in(&world)));
    }

    #[test]
    fn a_triggered_spawner_waits_for_trigger() {
        let world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 1, 0.0, 0, false, false);
        assert!(!spawner.tick(1.0, &alive_in(&world)));
        spawner.trigger();
        assert!(spawner.tick(0.0, &alive_in(&world)));
    }

    #[test]
    fn monstercount_stops_a_non_cyclic_spawner_after_its_quota() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 2, 0.0, 0, true, false);
        for _ in 0..2 {
            assert!(spawner.tick(0.0, &alive_in(&world)));
            let e = world.spawn(());
            spawner.note_spawned(e);
        }
        assert_eq!(spawner.remaining(), Some(0));
        assert!(!spawner.tick(0.0, &alive_in(&world)));
    }

    #[test]
    fn unlimited_monstercount_never_reports_zero_remaining() {
        let spawner = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, true, false);
        assert_eq!(spawner.remaining(), None);
    }

    #[test]
    fn max_live_children_withholds_spawns_until_one_dies() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 1, true, false);
        assert!(spawner.tick(0.0, &alive_in(&world)));
        let child = world.spawn(());
        spawner.note_spawned(child);
        assert!(!spawner.tick(0.0, &alive_in(&world)), "at the cap");
        world.despawn(child).unwrap();
        assert!(spawner.tick(0.0, &alive_in(&world)), "room freed up");
    }

    #[test]
    fn a_cyclic_spawner_restarts_its_quota_when_retriggered() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 1, 0.0, 0, true, true);
        assert!(spawner.tick(0.0, &alive_in(&world)));
        let e = world.spawn(());
        spawner.note_spawned(e);
        assert!(!spawner.tick(0.0, &alive_in(&world)));
        spawner.trigger();
        assert!(spawner.tick(0.0, &alive_in(&world)));
    }
}
