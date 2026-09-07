//! `monstermaker` spawner semantics.
//!
//! Published `monstermaker` keyvalues and spawnflags (see
//! `docs/FORMAT_SOURCES.md`, "Monster definitions"): `monstercount` (total
//! monsters this maker will ever create; `-1` means unlimited),
//! `m_imaxlivechildren` (the largest number of its children allowed alive at
//! once; new spawns wait for one to die once at the cap), a spawn `delay` in
//! seconds between spawns, a `Start On` spawnflag (spawn immediately rather
//! than waiting to be triggered) and a `Cyclic` spawnflag ("keep spawning
//! rather than stopping after one quota").
//!
//! # What this build could not confirm from a public page, and its own
//! # reading in place of it
//!
//! No reachable page states what a `monstermaker`'s *retrigger* does (firing
//! `target` at an already-active maker), or exactly what distinguishes a
//! `Cyclic` maker's *mechanics* from a plain one beyond the one phrase
//! above. This module's own reading, recorded here rather than guessed at
//! silently:
//!
//! - **Retrigger toggles**, for both a `Cyclic` and a non-`Cyclic` maker
//!   alike: triggering an inactive maker starts it spawning (subject to
//!   [`Spawner::delay`]/[`Spawner::monstercount`]/[`Spawner::max_live_children`],
//!   exactly like `Start On`); triggering an already-active one stops it.
//!   **Project decision**, not sourced — see [`Spawner::trigger`].
//! - **`Cyclic`'s "keep spawning rather than stopping after one quota"**
//!   is read as: one trigger (or `Start On`) starts a *continuous*,
//!   `delay`-paced spawn loop that keeps producing children — the "stopping
//!   after one" the cited phrase rules out — for as long as
//!   `monstercount`/`m_imaxlivechildren` allow, the same continuous loop a
//!   non-`Cyclic` maker's own `Start On`/trigger already runs. `monstercount`
//!   still caps the total either way: no page states an *unbounded*, ever
//!   after `monstercount`, spawn count for `Cyclic`, and this project
//!   prefers not to build an unbounded monster-spawn path on a guess.
//!   Under this reading a `Cyclic` maker's `Spawner` state machine is
//!   currently identical to a non-`Cyclic` one's; the field is still kept
//!   and still recorded (matching the map's own keyvalue), and this is
//!   `TODO(black-box)`: if a public source is later found describing an
//!   actual mechanical difference beyond this, this is the place to add it.
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
    /// The published `Cyclic` spawnflag, carried through and reported (see
    /// [`Spawner::is_cyclic`]), but — see this module's own doc comment —
    /// not (yet) mechanically distinct from a non-`Cyclic` maker in this
    /// build: `TODO(black-box)`.
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

    /// Whether this maker carries the published `Cyclic` spawnflag. See
    /// this module's own doc comment for what this build currently does
    /// (and does not) do differently for one.
    #[must_use]
    pub const fn is_cyclic(&self) -> bool {
        self.cyclic
    }

    /// Reacts to a `target`-firing activation: starts a continuous,
    /// `delay`-paced spawn loop when this maker was idle, or stops one
    /// already running. See this module's own doc comment for why this
    /// toggle is a project decision, not a sourced rule, and applies
    /// uniformly whether or not [`Spawner::cyclic`] is set.
    pub fn trigger(&mut self) {
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

    /// Whether this maker is currently spawning (mid continuous loop,
    /// waiting out [`Self::delay`] between children).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Seconds left before this maker's next spawn, while active.
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
    pub fn restore_counters(&mut self, spawned_total: u32, active: bool, timer: f32) {
        self.spawned_total = match u32::try_from(self.monstercount) {
            Ok(cap) => spawned_total.min(cap),
            Err(_) => spawned_total,
        };
        self.active = active;
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
            // The quota is exhausted for good: `monstercount` is an
            // absolute lifetime cap (see this module's own doc comment for
            // why `Cyclic` does not loop past it in this build).
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

    /// `docs/FORMAT_SOURCES.md`'s "Monster definitions" citation for bit
    /// `4` "Cyclic": "keep spawning rather than stopping after one quota".
    /// One trigger must therefore start a *continuous*, `delay`-paced loop
    /// producing every child `monstercount` allows, not just one — the bug
    /// an earlier build of this milestone had.
    #[test]
    fn a_cyclic_spawner_keeps_spawning_on_delay_after_one_trigger() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 3, 1.0, 0, false, true);
        spawner.trigger();

        // One trigger alone must be worth all three children, paced by
        // `delay` (not instant/untimed), with no further trigger needed in
        // between.
        assert!(
            spawner.tick(0.0, &alive_in(&world)),
            "child #1 is immediate"
        );
        spawner.note_spawned(world.spawn(()));
        assert_eq!(spawner.spawned_total(), 1);

        for expected_total in 2..=3u32 {
            // `delay` (1.0s) has not elapsed yet: no child is due early.
            assert!(!spawner.tick(0.5, &alive_in(&world)));
            assert!(
                spawner.tick(0.6, &alive_in(&world)),
                "child #{expected_total} becomes due once delay elapses"
            );
            spawner.note_spawned(world.spawn(()));
            assert_eq!(spawner.spawned_total(), expected_total);
        }
        assert_eq!(spawner.remaining(), Some(0));
        assert!(
            !spawner.is_active(),
            "the quota is exhausted for good, without a fresh trigger"
        );
    }

    /// `Start On` on a `Cyclic` maker starts the same continuous loop
    /// immediately, without waiting for a trigger.
    #[test]
    fn start_on_and_cyclic_together_spawn_continuously_without_a_trigger() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 2, 0.0, 0, true, true);
        assert!(spawner.is_active(), "Start On begins spawning immediately");
        assert!(spawner.tick(0.0, &alive_in(&world)));
        let e1 = world.spawn(());
        spawner.note_spawned(e1);
        assert!(
            spawner.tick(0.0, &alive_in(&world)),
            "no further trigger is needed for the second child"
        );
    }

    #[test]
    fn triggering_an_active_spawner_toggles_it_off_cyclic_or_not() {
        for cyclic in [false, true] {
            let world = World::new();
            let mut spawner = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, cyclic);
            spawner.trigger();
            assert!(spawner.tick(0.0, &alive_in(&world)), "started spawning");
            spawner.trigger();
            assert!(
                !spawner.tick(0.0, &alive_in(&world)),
                "the second trigger toggled it off (cyclic = {cyclic})"
            );
        }
    }

    #[test]
    fn a_cyclic_spawner_never_exceeds_monstercount_from_one_continuous_run() {
        let mut world = World::new();
        let mut spawner = Spawner::new("monster_headcrab", 2, 0.0, 0, false, true);
        spawner.trigger();
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

        spawner.restore_counters(2, true, 0.4);
        assert_eq!(spawner.spawned_total(), 2);
        assert!(spawner.is_active());
        assert!((spawner.timer() - 0.4).abs() < f32::EPSILON);

        // A corrupt or forward-written save cannot push the quota past its
        // own monstercount, and a non-finite timer sanitizes to zero.
        spawner.restore_counters(100, true, f32::NAN);
        assert_eq!(spawner.spawned_total(), 3);
        assert!(spawner.timer().abs() < f32::EPSILON);
    }

    #[test]
    fn is_cyclic_reports_the_stored_spawnflag() {
        let cyclic = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, true);
        assert!(cyclic.is_cyclic());
        let non_cyclic = Spawner::new("monster_headcrab", UNLIMITED, 0.0, 0, false, false);
        assert!(!non_cyclic.is_cyclic());
    }
}
