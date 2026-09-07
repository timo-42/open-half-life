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
    /// Whether each trigger event is worth exactly one spawned child
    /// (rather than the non-cyclic on/off toggle [`Self::trigger`]
    /// otherwise applies), still bounded by `monstercount`/
    /// `m_imaxlivechildren`. See [`Self::trigger`] for the exact rule.
    pub cyclic: bool,

    active: bool,
    spawned_total: u32,
    timer: f32,
    children: Vec<Entity>,
    /// Trigger events a [`Self::cyclic`] maker has received but not yet
    /// spent on a spawn (each is worth exactly one child; see
    /// [`Self::trigger`]).
    cyclic_pending: u32,
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
            active: start_on && !cyclic,
            spawned_total: 0,
            timer: 0.0,
            children: Vec::new(),
            // `Start On` on a `Cyclic` maker reads as an implicit first
            // trigger (one child), consistent with `Self::trigger`'s
            // documented per-trigger rule for cyclic makers.
            cyclic_pending: u32::from(start_on && cyclic),
        }
    }

    /// Reacts to a `target`-firing activation, per this milestone's
    /// documented "Start On"/"Cyclic" semantics (see
    /// `docs/FORMAT_SOURCES.md`, "Monster definitions", for the published
    /// keyvalues/flags; the *retrigger* behaviour below — toggle for a
    /// non-cyclic maker, one discrete spawn per trigger for a cyclic one —
    /// is this milestone's own product decision, since no reachable public
    /// page described it, and is recorded here rather than in the module
    /// doc so a reader sees it beside the code it governs):
    ///
    /// - A non-[`Self::cyclic`] maker toggles: triggering an inactive maker
    ///   starts it spawning (subject to [`Self::delay`]/[`Self::monstercount`]/
    ///   [`Self::max_live_children`] exactly like `Start On`); triggering an
    ///   already-active one stops it, without resetting its quota.
    /// - A [`Self::cyclic`] maker never enters the continuous `active`
    ///   state at all: each trigger is worth exactly one child, spawned as
    ///   soon as [`Self::tick`] finds room and quota for it (immediately if
    ///   both are already free), independent of any other pending trigger.
    pub fn trigger(&mut self) {
        if self.cyclic {
            self.cyclic_pending = self.cyclic_pending.saturating_add(1);
            return;
        }
        self.active = !self.active;
        if self.active {
            self.timer = 0.0;
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

    /// The total number of children spawned so far. Save-file bookkeeping
    /// (`ohl-engine`'s `SECTION_MOVER_STATE`, tag 28); see
    /// [`Self::restore_counters`].
    #[must_use]
    pub const fn spawned_total(&self) -> u32 {
        self.spawned_total
    }

    /// Whether a non-cyclic maker is currently spawning (always `false`
    /// for a cyclic one, which never uses this flag; see [`Self::trigger`]).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Trigger events queued but not yet spent on a spawn, for a
    /// [`Self::cyclic`] maker (always `0` for a non-cyclic one).
    #[must_use]
    pub const fn cyclic_pending(&self) -> u32 {
        self.cyclic_pending
    }

    /// Seconds left before a non-cyclic, active maker's next spawn.
    #[must_use]
    pub const fn timer(&self) -> f32 {
        self.timer
    }

    /// Restores this maker's counters from a save
    /// (`ohl-engine`'s `SECTION_MOVER_STATE`, tag 28). Every value is
    /// sanitized (non-finite floats, and `spawned_total` clamped to
    /// `monstercount` when it is bounded) so a corrupt save cannot hand
    /// the simulation a `NaN` timer or a quota already past its own cap.
    ///
    /// This does **not** restore [`Self::live_children`]'s underlying
    /// entity list: a `monstermaker`'s children are not indexed by
    /// `Registry::entities` at all (see `crate::save_state`'s module doc
    /// in `ohl-engine`, "Monstermaker children are not saved"), so there
    /// is no save-stable handle to reconstruct them from. A save/load
    /// therefore still forgets which entities were this maker's live
    /// children — a pre-existing, documented gap — even though the
    /// *counters* above round-trip exactly.
    pub fn restore_counters(
        &mut self,
        spawned_total: u32,
        active: bool,
        cyclic_pending: u32,
        timer: f32,
    ) {
        self.spawned_total = match u32::try_from(self.monstercount) {
            Ok(cap) => spawned_total.min(cap),
            Err(_) => spawned_total,
        };
        self.active = active && !self.cyclic;
        self.cyclic_pending = if self.cyclic { cyclic_pending } else { 0 };
        self.timer = if timer.is_finite() {
            timer.max(0.0)
        } else {
            0.0
        };
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
        if self.remaining() == Some(0) {
            // The quota is exhausted for good: a non-cyclic maker stops
            // spawning, and a cyclic one drops any trigger it had queued
            // rather than owing a child it can never legally create.
            self.active = false;
            self.cyclic_pending = 0;
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
        if self.cyclic {
            return self.tick_cyclic();
        }
        if !self.active {
            return false;
        }
        if self.remaining() == Some(0) {
            self.active = false;
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

    /// [`Self::tick`]'s cyclic path: spends one queued trigger on one
    /// spawn, as soon as quota and room allow it. Untimed — see
    /// [`Self::trigger`]'s doc comment for why.
    fn tick_cyclic(&mut self) -> bool {
        if self.cyclic_pending == 0 {
            return false;
        }
        if self.remaining() == Some(0) {
            self.cyclic_pending = 0;
            return false;
        }
        if !self.has_room() {
            return false;
        }
        self.cyclic_pending -= 1;
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
    fn a_cyclic_spawner_spawns_exactly_one_child_per_trigger() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, true);
        assert!(!spawner.tick(0.0, &alive_in(&world)), "no trigger yet");
        spawner.trigger();
        assert!(spawner.tick(0.0, &alive_in(&world)));
        let e1 = world.spawn(());
        spawner.note_spawned(e1);
        assert!(
            !spawner.tick(0.0, &alive_in(&world)),
            "spent its one trigger"
        );
        spawner.trigger();
        assert!(
            spawner.tick(0.0, &alive_in(&world)),
            "a second trigger is worth a second child"
        );
        let e2 = world.spawn(());
        spawner.note_spawned(e2);
        assert!(!spawner.tick(0.0, &alive_in(&world)));
    }

    #[test]
    fn triggering_an_active_non_cyclic_spawner_toggles_it_off() {
        let world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, false);
        spawner.trigger();
        assert!(spawner.tick(0.0, &alive_in(&world)), "started spawning");
        spawner.trigger();
        assert!(
            !spawner.tick(0.0, &alive_in(&world)),
            "the second trigger toggled it off"
        );
    }

    #[test]
    fn a_cyclic_spawner_never_exceeds_monstercount_across_many_triggers() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 2, 0.0, 0, false, true);
        for _ in 0..5 {
            spawner.trigger();
        }
        let mut spawn_count = 0;
        while spawner.tick(0.0, &alive_in(&world)) {
            let e = world.spawn(());
            spawner.note_spawned(e);
            spawn_count += 1;
        }
        assert_eq!(spawn_count, 2, "monstercount caps a cyclic maker too");
    }

    #[test]
    fn restore_counters_round_trips_and_clamps_to_monstercount() {
        let mut spawner = Spawner::new("monster_headcrab", 3, 1.5, 0, false, false);
        spawner.trigger();
        assert!(spawner.is_active());
        assert_eq!(spawner.spawned_total(), 0);

        spawner.restore_counters(2, true, 0, 0.4);
        assert_eq!(spawner.spawned_total(), 2);
        assert!(spawner.is_active());
        assert!((spawner.timer() - 0.4).abs() < f32::EPSILON);

        // A corrupt or forward-written save cannot push the quota past its
        // own monstercount, and a non-finite timer sanitizes to zero.
        spawner.restore_counters(100, true, 0, f32::NAN);
        assert_eq!(spawner.spawned_total(), 3);
        assert!(spawner.timer().abs() < f32::EPSILON);
    }

    #[test]
    fn restore_counters_never_activates_a_cyclic_spawner_or_pends_a_non_cyclic_one() {
        let mut cyclic = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, true);
        cyclic.restore_counters(0, true, 3, 0.0);
        assert!(!cyclic.is_active(), "cyclic makers never use `active`");
        assert_eq!(cyclic.cyclic_pending(), 3);

        let mut non_cyclic = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, false);
        non_cyclic.restore_counters(0, false, 5, 0.0);
        assert_eq!(
            non_cyclic.cyclic_pending(),
            0,
            "a non-cyclic maker never carries pending cyclic triggers"
        );
    }
}
