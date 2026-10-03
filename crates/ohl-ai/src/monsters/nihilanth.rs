//! `monster_nihilanth` (the final boss): its health reserve, the recharger
//! crystals that refill it, the phases that follow from them, and its
//! activation.
//!
//! ## What is published (see `docs/FORMAT_SOURCES.md`, "Monster
//! definitions", "Wave 1 batch B")
//!
//! - `TWHL:Monster_nihilanth`: "It has a reserve of health in the form of
//!   sprites floating about it, each holding 1/20th of its health, which it
//!   absorbs when injured. When available, it can replenish this reserve
//!   using recharger crystals on the walls of its chamber, making it
//!   impossible to kill it without destroying the rechargers first." It can
//!   "teleport players to scripted locations, summon controllers and
//!   Vortigaunt slaves into its chamber, and also attack with electrical
//!   particles". "When spawned, the Nihilanth does not attack immediately.
//!   You need to 'activate' it with `trigger_auto` (with its TargetState as
//!   ON) or any other way." Its `target` and `TriggerCondition` are listed
//!   as not used ("TriggerCondition is not working for this monster").
//! - Combine OverWiki, "Nihilanth": "When the crystals are destroyed, the
//!   Nihilanth becomes vulnerable, and its head opens up like a flower after
//!   some time"; "When this energy runs low ... the Nihilanth replenishes
//!   the spheres by absorbing energy from one of the crystals".
//!
//! ## This project's reading
//!
//! The reserve is a single pool of `sphere_count × per-sphere` points that
//! every hit drains first ([`NihilanthShield::absorb`]); a hit that
//! overruns the pool is spent, not carried into health. With the pool empty
//! and a crystal still standing, hits are blocked, and a crystal refills a
//! pool that has run low after [`RECHARGE_SECONDS`]. With the pool empty
//! and no crystal left to refill it, the head opens after
//! [`HEAD_OPEN_SECONDS`] and, from then on, every hit costs health. Which
//! map entities are "the crystals" is the host's to say: it tags them with
//! [`NihilanthCrystal`], and [`crate::monsters::bosses`] counts the ones
//! still alive each tick. No map data says which entities those are, so a
//! real map's boss has no rechargers: once its reserve is spent, its head
//! opens. That is the published rule's own reading of a chamber without
//! crystals, not a claim about any map.
//!
//! Activation ([`NihilanthShield::activate`], reached through a `use` of
//! the boss's name, `ohl_game::registry::MonsterActivation`) only lets it
//! attack: the page says it "does not attack immediately", not that it
//! cannot be hurt, so the reserve, the crystals and the head work the same
//! before activation as after. Its teleport ball, its summoning and what
//! happens after its death are not modelled.
//!
//! Placeholders (`TODO(black-box)`): [`RECHARGE_SECONDS`],
//! [`HEAD_OPEN_SECONDS`] ("after some time"), [`RECHARGE_BELOW_FRACTION`]
//! ("When this energy runs low"), and the zap range in the table row.

/// Seconds a standing crystal takes to refill a reserve that has run low.
/// **`TODO(black-box)`**.
pub const RECHARGE_SECONDS: f32 = 6.0;

/// Seconds after the reserve is spent with no crystal standing before the
/// head opens and the boss takes health damage. **`TODO(black-box)`**:
/// "after some time".
pub const HEAD_OPEN_SECONDS: f32 = 8.0;

/// The reserve fraction below which a standing crystal is drawn on.
/// **`TODO(black-box)`**: "When this energy runs low".
pub const RECHARGE_BELOW_FRACTION: f32 = 0.25;

/// Marks a map entity as one of the Nihilanth's recharger crystals. A
/// host-owned tag: this crate never decides which entities are crystals,
/// it only counts the tagged ones whose `Actor` (if any) is still alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NihilanthCrystal;

/// The shield's phase, derived from its reserve and its crystals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NihilanthPhase {
    /// Reserve remaining: hits drain it.
    Shielded,
    /// Reserve empty, crystals standing: hits are blocked, the reserve
    /// refills.
    Recharging,
    /// No reserve and no crystal left; the head is opening.
    Opening {
        /// Seconds until it is open.
        left: f32,
    },
    /// The head is open: hits cost health.
    Exposed,
}

/// What one hit did to the shield.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShieldVerdict {
    /// The reserve took it; no health lost.
    Absorbed,
    /// The reserve is empty but the boss is not exposed; no health lost.
    Blocked,
    /// The hit costs health.
    Applied,
}

/// The part of a [`NihilanthShield`] that changes at runtime, in the form a
/// save file holds. The reserve's size is rebuilt from the boss's health
/// on every load, and the crystal count is re-read every tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShieldProgress {
    /// Whether the boss has been activated.
    pub active: bool,
    /// Points of reserve left.
    pub reserve: f32,
    /// Seconds until a standing crystal refills a low reserve.
    pub recharge_left: f32,
    /// Seconds until the head opens, while it is opening.
    pub open_left: Option<f32>,
    /// Whether the head is open.
    pub exposed: bool,
}

/// The per-monster shield state: a component on a `monster_nihilanth`.
#[derive(Debug, Clone, PartialEq)]
pub struct NihilanthShield {
    active: bool,
    sphere_count: u32,
    per_sphere: f32,
    reserve: f32,
    crystals: u32,
    recharge_left: f32,
    open_left: Option<f32>,
    exposed: bool,
}

impl NihilanthShield {
    /// A dormant boss whose reserve is `sphere_count` sprites of
    /// `per_sphere` points each, with `crystals` rechargers standing.
    #[must_use]
    pub fn new(sphere_count: u32, per_sphere: f32, crystals: u32) -> Self {
        let per_sphere = if per_sphere.is_finite() && per_sphere > 0.0 {
            per_sphere
        } else {
            0.0
        };
        #[allow(clippy::cast_precision_loss)]
        let reserve = per_sphere * sphere_count as f32;
        Self {
            active: false,
            sphere_count,
            per_sphere,
            reserve,
            crystals,
            recharge_left: RECHARGE_SECONDS,
            open_left: None,
            exposed: false,
        }
    }

    /// The reserve as a full pool of `sphere_count` sprites over `health`
    /// — the published "each holding 1/20th of its health".
    #[must_use]
    pub fn for_health(health: f32, sphere_count: u32, crystals: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let per_sphere = if sphere_count == 0 || !health.is_finite() || health <= 0.0 {
            0.0
        } else {
            health / sphere_count as f32
        };
        Self::new(sphere_count, per_sphere, crystals)
    }

    /// Activates the boss (the published `trigger_auto` step): from now on
    /// it attacks.
    pub fn activate(&mut self) {
        self.active = true;
    }

    /// Whether the boss has been activated.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Points of reserve left.
    #[must_use]
    pub fn reserve(&self) -> f32 {
        self.reserve
    }

    /// The full reserve.
    #[must_use]
    pub fn reserve_capacity(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let count = self.sphere_count as f32;
        self.per_sphere * count
    }

    /// How many sprites are still whole or partly charged, for display.
    #[must_use]
    pub fn spheres(&self) -> u32 {
        if self.per_sphere <= 0.0 {
            return 0;
        }
        // Bounded by `sphere_count`, so the narrowing cannot overflow.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let whole = (self.reserve / self.per_sphere).ceil().max(0.0) as u32;
        whole.min(self.sphere_count)
    }

    /// Crystals still standing, as last synced.
    #[must_use]
    pub fn crystals(&self) -> u32 {
        self.crystals
    }

    /// Records how many crystals are still standing (the host counts the
    /// [`NihilanthCrystal`]-tagged entities that are alive).
    pub fn sync_crystals(&mut self, standing: u32) {
        self.crystals = standing;
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> NihilanthPhase {
        if self.exposed {
            NihilanthPhase::Exposed
        } else if let Some(left) = self.open_left {
            NihilanthPhase::Opening { left }
        } else if self.reserve > 0.0 {
            NihilanthPhase::Shielded
        } else if self.crystals > 0 {
            NihilanthPhase::Recharging
        } else {
            // No reserve, no crystals, and the opening has not been started
            // by a tick yet: reported as opening from the full delay.
            NihilanthPhase::Opening {
                left: HEAD_OPEN_SECONDS,
            }
        }
    }

    /// Whether hits currently cost health.
    #[must_use]
    pub fn is_exposed(&self) -> bool {
        self.exposed
    }

    /// Advances the shield by `dt` seconds: a crystal recharge when the
    /// reserve has run low, and the head opening once the reserve is spent
    /// with no crystal left to refill it. Returns `true` on the tick the
    /// head opens.
    pub fn tick(&mut self, dt: f32) -> bool {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        if self.exposed {
            return false;
        }
        if self.crystals == 0 {
            if self.reserve > 0.0 {
                // The sprites still hold energy; nothing can refill them,
                // but the head opens only once they are spent.
                self.open_left = None;
                return false;
            }
            let left = self.open_left.unwrap_or(HEAD_OPEN_SECONDS) - dt;
            if left <= 0.0 {
                self.open_left = None;
                self.exposed = true;
                return true;
            }
            self.open_left = Some(left);
            return false;
        }
        // A crystal stands: the head stays shut whatever was counting.
        self.open_left = None;
        let capacity = self.reserve_capacity();
        if capacity > 0.0 && self.reserve < capacity * RECHARGE_BELOW_FRACTION {
            self.recharge_left -= dt;
            if self.recharge_left <= 0.0 {
                self.reserve = capacity;
                self.recharge_left = RECHARGE_SECONDS;
            }
        } else {
            self.recharge_left = RECHARGE_SECONDS;
        }
        false
    }

    /// Decides what a hit of `amount` does.
    pub fn absorb(&mut self, amount: f32) -> ShieldVerdict {
        if !(amount.is_finite() && amount > 0.0) {
            return ShieldVerdict::Blocked;
        }
        if self.exposed {
            return ShieldVerdict::Applied;
        }
        if self.reserve > 0.0 {
            self.reserve = (self.reserve - amount).max(0.0);
            return ShieldVerdict::Absorbed;
        }
        ShieldVerdict::Blocked
    }

    /// The runtime state, for a save.
    #[must_use]
    pub fn progress(&self) -> ShieldProgress {
        ShieldProgress {
            active: self.active,
            reserve: self.reserve,
            recharge_left: self.recharge_left,
            open_left: self.open_left,
            exposed: self.exposed,
        }
    }

    /// Puts the runtime state back. A reserve outside `0..=capacity` is
    /// clamped into it, and a non-finite or negative timer falls back to
    /// its full delay, so a damaged save cannot leave the boss with an
    /// overfull or poisoned shield.
    pub fn restore(&mut self, progress: ShieldProgress) {
        let timer = |value: f32, full: f32| {
            if value.is_finite() && value >= 0.0 {
                value.min(full)
            } else {
                full
            }
        };
        self.active = progress.active;
        self.reserve = if progress.reserve.is_finite() {
            progress.reserve.clamp(0.0, self.reserve_capacity())
        } else {
            0.0
        };
        self.recharge_left = timer(progress.recharge_left, RECHARGE_SECONDS);
        self.open_left = progress
            .open_left
            .map(|left| timer(left, HEAD_OPEN_SECONDS));
        self.exposed = progress.exposed;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HEAD_OPEN_SECONDS, NihilanthPhase, NihilanthShield, RECHARGE_BELOW_FRACTION,
        RECHARGE_SECONDS, ShieldProgress, ShieldVerdict,
    };

    fn boss() -> NihilanthShield {
        let mut shield = NihilanthShield::for_health(800.0, 20, 3);
        shield.activate();
        shield
    }

    /// Activation only lets it attack: a dormant boss's shield drains,
    /// blocks and opens exactly as an active one's does.
    #[test]
    fn activation_changes_nothing_about_the_shield() {
        let mut dormant = NihilanthShield::for_health(800.0, 20, 0);
        let mut active = NihilanthShield::for_health(800.0, 20, 0);
        active.activate();
        assert!(!dormant.is_active());
        assert!(active.is_active());
        for shield in [&mut dormant, &mut active] {
            assert_eq!(shield.absorb(800.0), ShieldVerdict::Absorbed);
            assert!(shield.tick(HEAD_OPEN_SECONDS + 0.1));
            assert_eq!(shield.absorb(5.0), ShieldVerdict::Applied);
        }
    }

    #[test]
    fn the_reserve_is_twenty_sprites_of_a_twentieth_each_and_drains_first() {
        let mut shield = boss();
        assert_eq!(shield.spheres(), 20);
        assert!((shield.reserve_capacity() - 800.0).abs() < 1e-4);
        assert_eq!(shield.absorb(40.0), ShieldVerdict::Absorbed);
        assert_eq!(shield.spheres(), 19);
        assert_eq!(shield.absorb(10.0), ShieldVerdict::Absorbed);
        assert_eq!(shield.spheres(), 19, "a part-drained sprite still shows");
        assert_eq!(shield.absorb(10_000.0), ShieldVerdict::Absorbed);
        assert_eq!(shield.spheres(), 0);
        assert!((shield.reserve()).abs() < 1e-6);
        // With crystals standing, an empty reserve blocks rather than
        // applies: "impossible to kill it without destroying the
        // rechargers first".
        assert_eq!(shield.phase(), NihilanthPhase::Recharging);
        assert_eq!(shield.absorb(100.0), ShieldVerdict::Blocked);
    }

    #[test]
    fn a_standing_crystal_refills_a_low_reserve_after_the_delay() {
        let mut shield = boss();
        let low = 800.0 * RECHARGE_BELOW_FRACTION - 1.0;
        assert_eq!(shield.absorb(800.0 - low), ShieldVerdict::Absorbed);
        assert!(shield.reserve() < 800.0 * RECHARGE_BELOW_FRACTION);
        assert!(!shield.tick(RECHARGE_SECONDS * 0.5));
        assert!(
            shield.reserve() < 800.0 * RECHARGE_BELOW_FRACTION,
            "not yet"
        );
        assert!(!shield.tick(RECHARGE_SECONDS * 0.6));
        assert!((shield.reserve() - 800.0).abs() < 1e-4, "refilled");
        assert_eq!(shield.phase(), NihilanthPhase::Shielded);
    }

    #[test]
    fn a_healthy_reserve_does_not_count_toward_a_recharge() {
        let mut shield = boss();
        assert_eq!(shield.absorb(10.0), ShieldVerdict::Absorbed);
        shield.tick(RECHARGE_SECONDS * 10.0);
        assert!(
            (shield.reserve() - 790.0).abs() < 1e-4,
            "no refill above the threshold"
        );
    }

    /// The crystal seam: only once every crystal is gone does the head
    /// open, and only then do hits cost health.
    #[test]
    fn destroying_every_crystal_opens_the_head_and_exposes_the_boss() {
        let mut shield = boss();
        assert_eq!(shield.absorb(10_000.0), ShieldVerdict::Absorbed);
        shield.sync_crystals(1);
        assert_eq!(shield.phase(), NihilanthPhase::Recharging);
        shield.tick(RECHARGE_SECONDS * 0.5);
        assert_eq!(
            shield.absorb(1.0),
            ShieldVerdict::Blocked,
            "one crystal still stands"
        );
        // ... and that crystal refills the reserve, so it can never be
        // fought down to health while it stands.
        shield.tick(RECHARGE_SECONDS);
        assert_eq!(
            shield.absorb(1.0),
            ShieldVerdict::Absorbed,
            "recharged from the crystal"
        );
        assert!(shield.reserve() > 700.0);
        assert_eq!(shield.absorb(10_000.0), ShieldVerdict::Absorbed);

        shield.sync_crystals(0);
        assert!(!shield.tick(HEAD_OPEN_SECONDS * 0.5));
        assert!(matches!(shield.phase(), NihilanthPhase::Opening { .. }));
        assert_eq!(shield.absorb(1.0), ShieldVerdict::Blocked, "still opening");
        assert!(
            shield.tick(HEAD_OPEN_SECONDS * 0.6),
            "the tick the head opens"
        );
        assert_eq!(shield.phase(), NihilanthPhase::Exposed);
        assert!(shield.is_exposed());
        assert_eq!(shield.absorb(1.0), ShieldVerdict::Applied);
        assert!(!shield.tick(1.0), "opens once");
        // A crystal re-appearing cannot close an opened head.
        shield.sync_crystals(1);
        shield.tick(1.0);
        assert_eq!(shield.absorb(1.0), ShieldVerdict::Applied);
    }

    #[test]
    fn a_crystal_standing_again_mid_opening_shuts_the_head() {
        let mut shield = boss();
        shield.absorb(10_000.0);
        shield.sync_crystals(0);
        shield.tick(HEAD_OPEN_SECONDS * 0.5);
        shield.sync_crystals(1);
        shield.tick(0.1);
        assert_eq!(shield.phase(), NihilanthPhase::Recharging);
        shield.sync_crystals(0);
        assert!(
            !shield.tick(HEAD_OPEN_SECONDS * 0.9),
            "the delay restarts from the top"
        );
        assert!(shield.tick(HEAD_OPEN_SECONDS * 0.2));
    }

    #[test]
    fn a_boss_with_no_crystals_at_all_is_killable_once_its_reserve_is_gone() {
        let mut shield = NihilanthShield::for_health(1_000.0, 20, 0);
        shield.activate();
        assert_eq!(shield.spheres(), 20);
        assert!((shield.reserve_capacity() - 1_000.0).abs() < 1e-4);
        // With sprites still charged the head stays shut, however long.
        shield.tick(HEAD_OPEN_SECONDS * 3.0);
        assert_eq!(shield.phase(), NihilanthPhase::Shielded);
        assert_eq!(shield.absorb(1_000.0), ShieldVerdict::Absorbed);
        assert!(matches!(shield.phase(), NihilanthPhase::Opening { .. }));
        assert!(shield.tick(HEAD_OPEN_SECONDS + 0.1));
        assert_eq!(shield.absorb(5.0), ShieldVerdict::Applied);
    }

    #[test]
    fn progress_round_trips_and_a_bad_restore_is_clamped() {
        let mut shield = boss();
        shield.absorb(700.0);
        shield.sync_crystals(0);
        shield.absorb(500.0);
        shield.tick(HEAD_OPEN_SECONDS * 0.25);
        let progress = shield.progress();
        assert!(progress.active);
        assert!(progress.open_left.is_some());

        let mut fresh = NihilanthShield::for_health(800.0, 20, 0);
        fresh.restore(progress);
        assert_eq!(fresh.progress(), progress);
        assert_eq!(fresh.phase(), shield.phase());

        fresh.restore(ShieldProgress {
            active: false,
            reserve: 1.0e9,
            recharge_left: f32::NAN,
            open_left: Some(-3.0),
            exposed: false,
        });
        assert!((fresh.reserve() - fresh.reserve_capacity()).abs() < 1e-3);
        assert!((fresh.progress().recharge_left - RECHARGE_SECONDS).abs() < 1e-6);
        assert_eq!(fresh.progress().open_left, Some(HEAD_OPEN_SECONDS));
    }

    #[test]
    fn degenerate_inputs_leave_an_empty_but_sane_reserve() {
        let mut shield = NihilanthShield::for_health(f32::NAN, 20, 0);
        shield.activate();
        assert_eq!(shield.spheres(), 0);
        assert!((shield.reserve_capacity()).abs() < 1e-6);
        assert_eq!(shield.absorb(f32::NAN), ShieldVerdict::Blocked);
        assert_eq!(shield.absorb(-1.0), ShieldVerdict::Blocked);
        let mut none = NihilanthShield::for_health(800.0, 0, 0);
        none.activate();
        assert_eq!(none.spheres(), 0);
        none.tick(f32::INFINITY);
        assert!(none.tick(HEAD_OPEN_SECONDS + 1.0) || none.is_exposed());
    }
}
