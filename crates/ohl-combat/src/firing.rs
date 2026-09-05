//! The per-weapon firing state machine.
//!
//! [`FiringState::tick`] is a pure function of a weapon's [`WeaponSpec`], its
//! own internal state, an elapsed time step, and the caller's
//! [`WeaponInput`] for that tick: given the same sequence of calls it always
//! produces the same sequence of [`WeaponAction`]s and the same ammo
//! consumption, so it can be replayed, fuzzed and predicted the same way the
//! rest of the simulation is.
//!
//! [`resolve_hitscan`] is the resolution glue: it turns a [`WeaponAction::Hitscan`]
//! together with the caller's `trace_attack` results into the
//! [`crate::DamageInfo`] records `crate::apply_damage` consumes.

use crate::ammo::AmmoPool;
use crate::damage::{DamageInfo, DamageType};
use crate::trace::{AttackTrace, EntityId};
use crate::weapons::{WeaponId, WeaponKind, WeaponSpec};

/// How long the gauss gun's secondary charge may be held before it forces a
/// release. Combine OverWiki, "Gauss Gun" (`docs/FORMAT_SOURCES.md`): "10
/// seconds".
pub const GAUSS_OVERCHARGE_SECONDS: f32 = 10.0;

/// The self-damage a forced gauss overcharge release deals instead of firing.
/// Combine OverWiki, "Gauss Gun": "50 HP".
pub const GAUSS_OVERCHARGE_SELF_DAMAGE: f32 = 50.0;

/// The gauss gun's published charged-shot damage range (25 at no charge, 200
/// at a full 10-second charge). Combine OverWiki, "Gauss Gun".
pub const GAUSS_CHARGE_DAMAGE_RANGE: (f32, f32) = (25.0, 200.0);

/// One tick's worth of input for a weapon's [`FiringState`].
///
/// Four independent held/pressed flags, matching the package's assignment
/// (`primary`, `secondary`, `reload`, `select`) rather than a state machine
/// of its own: the caller's actual input system already debounces and
/// disambiguates these, so this type is deliberately a plain snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct WeaponInput {
    /// Primary fire is held.
    pub primary: bool,
    /// Secondary fire is held.
    pub secondary: bool,
    /// A reload was requested this tick.
    pub reload: bool,
    /// This weapon is (or should become) the selected weapon.
    pub select: bool,
}

/// Which animation the presentation layer should play.
///
/// Named, not string-keyed: the sequence names Half-Life's model viewers
/// print (`idle1`, `fire1`, `reload1`, `deploy`, `holster`) are per-model
/// QC data this crate never loads, so this is this project's own small,
/// closed vocabulary instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sequence {
    /// No shot fired or reload in progress; the idle loop.
    Idle,
    /// A shot was just fired.
    Fire,
    /// A reload just started.
    Reload,
    /// The weapon was just drawn.
    Draw,
    /// The weapon is being holstered.
    Holster,
}

/// Which cue sound the presentation layer should play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundKind {
    /// A shot fired.
    Fire,
    /// A reload started.
    Reload,
    /// The weapon was drawn.
    Draw,
    /// The weapon was holstered.
    Holster,
    /// Primary or secondary fire was pressed with nothing to fire.
    DryFire,
    /// A gauss charge was held past [`GAUSS_OVERCHARGE_SECONDS`].
    Overcharge,
}

/// What one [`FiringState::tick`] call produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeaponAction {
    /// Resolve `count` instant-hit shots, each with `spread` radians of cone
    /// half-angle around the aim direction (the caller samples the cone;
    /// this crate does not, since sampling needs a random source it has no
    /// opinion on).
    Hitscan {
        /// How many shots to resolve (pellets, for a shotgun blast).
        count: u32,
        /// The spread cone's half-angle in radians.
        spread: f32,
    },
    /// Resolve one melee swing.
    Melee,
    /// Spawn a physical projectile of `kind`'s weapon at `speed` world units
    /// per second along the aim direction.
    SpawnProjectile {
        /// Which weapon fired the projectile (selects its damage and blast
        /// behaviour in M7.3).
        kind: WeaponId,
        /// Muzzle speed, world units per second.
        speed: f32,
    },
    /// One tick of a continuous beam; the caller re-traces every tick this
    /// is produced.
    BeamTick,
    /// Play a view-model animation.
    PlaySequence(Sequence),
    /// Play a cue sound.
    Sound(SoundKind),
    /// Nothing happened this tick (cycling, reloading, no input, or a dry
    /// weapon).
    Empty,
}

/// One weapon's firing state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FireState {
    Idle,
    Firing { until: f32 },
    Reloading { until: f32 },
    Charging { since: f32 },
    Beam,
    Holstered,
}

/// The state machine driving one weapon: how much is loaded, whether it is
/// cycling, reloading, charging or beaming, and what it should do next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FiringState {
    spec: WeaponSpec,
    state: FireState,
    clip: u32,
    elapsed: f32,
    beam_accum: f32,
    pending_charge_damage: Option<f32>,
    pending_self_damage: Option<f32>,
}

impl FiringState {
    /// A holstered weapon with an empty clip.
    #[must_use]
    pub const fn new(spec: WeaponSpec) -> Self {
        Self {
            spec,
            state: FireState::Holstered,
            clip: 0,
            elapsed: 0.0,
            beam_accum: 0.0,
            pending_charge_damage: None,
            pending_self_damage: None,
        }
    }

    /// This weapon's static data.
    #[must_use]
    pub const fn spec(&self) -> &WeaponSpec {
        &self.spec
    }

    /// Rounds currently loaded.
    #[must_use]
    pub const fn clip(&self) -> u32 {
        self.clip
    }

    /// Whether the weapon is holstered.
    #[must_use]
    pub const fn is_holstered(&self) -> bool {
        matches!(self.state, FireState::Holstered)
    }

    /// Whether the weapon is mid-reload.
    #[must_use]
    pub const fn is_reloading(&self) -> bool {
        matches!(self.state, FireState::Reloading { .. })
    }

    /// Whether a gauss-style charge is in progress.
    #[must_use]
    pub const fn is_charging(&self) -> bool {
        matches!(self.state, FireState::Charging { .. })
    }

    /// Sets the loaded clip directly (a save load, or an initial loadout),
    /// clamped to the weapon's clip size (or to zero, with no clip concept).
    pub fn set_clip(&mut self, rounds: u32) {
        let cap = self.spec.clip_size.unwrap_or(0);
        self.clip = rounds.min(cap);
    }

    /// Takes the damage a just-released gauss charge dealt, if any. `None`
    /// on every tick but the one the charged shot released on.
    pub fn take_charge_damage(&mut self) -> Option<f32> {
        self.pending_charge_damage.take()
    }

    /// Takes the self-damage a forced gauss overcharge dealt, if any. `None`
    /// on every tick but the one the overcharge triggered on.
    pub fn take_self_damage(&mut self) -> Option<f32> {
        self.pending_self_damage.take()
    }

    /// Advances the state machine by `dt` seconds under `input`, consuming
    /// `pool` ammo as needed, and returns what happened.
    ///
    /// `dt` is clamped to zero when it is not finite or negative, so a
    /// corrupt frame time cannot run the state machine backwards; a zero or
    /// negative `dt` still processes `input` (a paused game can still, for
    /// example, register a reload request queued for the next real tick).
    pub fn tick(&mut self, dt: f32, input: WeaponInput, pool: &mut AmmoPool) -> WeaponAction {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        self.elapsed += dt;

        match self.state {
            FireState::Holstered => self.tick_holstered(input),
            FireState::Firing { until } => self.tick_firing(until),
            FireState::Reloading { until } => self.tick_reloading(until, pool),
            FireState::Beam => self.tick_beam(dt, input, pool),
            FireState::Charging { since } => self.tick_charging(since, input, pool),
            FireState::Idle => self.tick_idle(input, pool),
        }
    }

    fn tick_holstered(&mut self, input: WeaponInput) -> WeaponAction {
        if !input.select {
            return WeaponAction::Empty;
        }
        self.state = FireState::Idle;
        WeaponAction::PlaySequence(Sequence::Draw)
    }

    fn tick_firing(&mut self, until: f32) -> WeaponAction {
        if self.elapsed >= until {
            self.state = FireState::Idle;
        }
        WeaponAction::Empty
    }

    fn tick_reloading(&mut self, until: f32, pool: &mut AmmoPool) -> WeaponAction {
        if self.elapsed < until {
            return WeaponAction::Empty;
        }
        let Some(clip_size) = self.spec.clip_size else {
            self.state = FireState::Idle;
            return WeaponAction::PlaySequence(Sequence::Idle);
        };
        let wanted = clip_size.saturating_sub(self.clip);
        self.clip += pool.take_up_to(wanted);
        self.state = FireState::Idle;
        WeaponAction::PlaySequence(Sequence::Idle)
    }

    fn tick_beam(&mut self, dt: f32, input: WeaponInput, pool: &mut AmmoPool) -> WeaponAction {
        if !input.primary || pool.is_empty() {
            self.state = FireState::Idle;
            return WeaponAction::PlaySequence(Sequence::Idle);
        }
        // The cell-drain interval is **BBO**; `spec.cycle_time` is this
        // package's placeholder for it (see `weapons::spec`'s egon entry).
        let interval = self.spec.cycle_time.value.max(f32::MIN_POSITIVE);
        self.beam_accum += dt;
        while self.beam_accum >= interval {
            self.beam_accum -= interval;
            if pool.take_up_to(1) == 0 {
                self.state = FireState::Idle;
                return WeaponAction::BeamTick;
            }
        }
        WeaponAction::BeamTick
    }

    fn tick_charging(
        &mut self,
        since: f32,
        input: WeaponInput,
        pool: &mut AmmoPool,
    ) -> WeaponAction {
        let held = self.elapsed - since;
        if held >= GAUSS_OVERCHARGE_SECONDS {
            self.state = FireState::Idle;
            self.pending_self_damage = Some(GAUSS_OVERCHARGE_SELF_DAMAGE);
            return WeaponAction::Sound(SoundKind::Overcharge);
        }
        if input.secondary {
            return WeaponAction::Empty;
        }
        // Released before the overcharge: fire a shot scaled linearly across
        // the published 25..=200 range by how long the charge was held.
        self.state = FireState::Idle;
        let fraction = (held / GAUSS_OVERCHARGE_SECONDS).clamp(0.0, 1.0);
        let (low, high) = GAUSS_CHARGE_DAMAGE_RANGE;
        self.pending_charge_damage = Some(low + fraction * (high - low));
        // One cell for the charged shot; the true per-charge drain rate is
        // **BBO**, so this crate spends the minimum that keeps ammo bounded.
        pool.take_up_to(1);
        WeaponAction::Hitscan {
            count: 1,
            spread: 0.0,
        }
    }

    fn tick_idle(&mut self, input: WeaponInput, pool: &mut AmmoPool) -> WeaponAction {
        if input.reload && self.wants_reload(pool) {
            let reload_time = self.spec.reload_time.value.max(0.0);
            self.state = FireState::Reloading {
                until: self.elapsed + reload_time,
            };
            return WeaponAction::Sound(SoundKind::Reload);
        }
        if input.primary {
            return self.fire_primary(pool);
        }
        if input.secondary {
            return self.fire_secondary(pool);
        }
        WeaponAction::Empty
    }

    fn wants_reload(&self, pool: &AmmoPool) -> bool {
        match self.spec.clip_size {
            Some(clip_size) => self.clip < clip_size && !pool.is_empty(),
            None => false,
        }
    }

    /// Whether the primary shot has something to fire from: the clip, or —
    /// for a weapon with no clip concept — the ammo pool, or — for a
    /// weapon with neither — always (the crowbar).
    fn has_primary_ammo(&self, pool: &AmmoPool) -> bool {
        match self.spec.clip_size {
            Some(_) => self.clip > 0,
            None => self.spec.ammo.is_none() || !pool.is_empty(),
        }
    }

    fn consume_primary_round(&mut self, pool: &mut AmmoPool) {
        match self.spec.clip_size {
            Some(_) => self.clip = self.clip.saturating_sub(1),
            None => {
                if self.spec.ammo.is_some() {
                    pool.take_up_to(1);
                }
            }
        }
    }

    fn fire_primary(&mut self, pool: &mut AmmoPool) -> WeaponAction {
        match self.spec.kind {
            WeaponKind::Beam => {
                if pool.is_empty() {
                    return WeaponAction::Sound(SoundKind::DryFire);
                }
                self.state = FireState::Beam;
                self.beam_accum = 0.0;
                WeaponAction::BeamTick
            }
            WeaponKind::Charge => {
                // The gauss gun's primary is an instant, uncharged shot at
                // its base published damage; charging is secondary fire.
                if !self.has_primary_ammo(pool) {
                    return WeaponAction::Sound(SoundKind::DryFire);
                }
                self.consume_primary_round(pool);
                self.state = FireState::Firing {
                    until: self.elapsed + self.spec.cycle_time.value.max(0.0),
                };
                WeaponAction::Hitscan {
                    count: 1,
                    spread: 0.0,
                }
            }
            kind => {
                if !self.has_primary_ammo(pool) {
                    return WeaponAction::Sound(SoundKind::DryFire);
                }
                self.consume_primary_round(pool);
                self.state = FireState::Firing {
                    until: self.elapsed + self.spec.cycle_time.value.max(0.0),
                };
                Self::action_for_kind(kind, self.spec.id)
            }
        }
    }

    fn fire_secondary(&mut self, pool: &mut AmmoPool) -> WeaponAction {
        if matches!(self.spec.kind, WeaponKind::Charge) {
            if self.spec.ammo.is_some() && pool.is_empty() {
                return WeaponAction::Sound(SoundKind::DryFire);
            }
            self.state = FireState::Charging {
                since: self.elapsed,
            };
            return WeaponAction::Empty;
        }
        let Some(secondary) = self.spec.secondary else {
            return WeaponAction::Empty;
        };
        if secondary.ammo.is_some() {
            if pool.is_empty() {
                return WeaponAction::Sound(SoundKind::DryFire);
            }
            pool.take_up_to(1);
        } else {
            // Reuses the primary clip. A double-barrel discharge is modeled
            // as costing one clip round, the same as a single-barrel shot:
            // no usable source publishes a separate per-barrel shell cost,
            // so this crate does not invent one.
            if self.clip == 0 {
                return WeaponAction::Sound(SoundKind::DryFire);
            }
            self.clip = self.clip.saturating_sub(1);
        }
        self.state = FireState::Firing {
            until: self.elapsed + secondary.cycle_time.value.max(0.0),
        };
        Self::action_for_kind(secondary.kind, self.spec.id)
    }

    fn action_for_kind(kind: WeaponKind, id: WeaponId) -> WeaponAction {
        match kind {
            WeaponKind::Melee => WeaponAction::Melee,
            WeaponKind::Hitscan { pellets, spread } => WeaponAction::Hitscan {
                count: pellets,
                spread: spread.value,
            },
            WeaponKind::Projectile { speed } => WeaponAction::SpawnProjectile {
                kind: id,
                speed: speed.value,
            },
            WeaponKind::Beam | WeaponKind::Charge => WeaponAction::Empty,
        }
    }
}

/// Turns a [`WeaponAction::Hitscan`] and its resolved traces into damage.
///
/// `traces` should have one entry per pellet the action's `count` calls for,
/// in the order the caller resolved them; entries whose
/// [`AttackTrace::entity`] is `None` (a miss, or a shot that only hit world
/// geometry) contribute no [`DamageInfo`]. Every returned record uses
/// `spec.damage` and `spec.damage_type`, so a shotgun blast that hits three
/// of its six pellets yields exactly three records, each at the per-pellet
/// damage.
///
/// Returns an empty vector for any other [`WeaponAction`] variant.
#[must_use]
pub fn resolve_hitscan(
    action: WeaponAction,
    spec: &WeaponSpec,
    traces: &[AttackTrace],
    attacker: Option<EntityId>,
    inflictor: Option<EntityId>,
) -> Vec<DamageInfo> {
    resolve_hitscan_with_amount(
        action,
        spec.damage,
        spec.damage_type,
        traces,
        attacker,
        inflictor,
    )
}

/// As [`resolve_hitscan`], but with an explicit damage amount overriding
/// `spec.damage` — needed for the gauss gun, whose charged-shot damage
/// varies per shot (see [`FiringState::take_charge_damage`]).
#[must_use]
pub fn resolve_hitscan_with_amount(
    action: WeaponAction,
    amount: f32,
    damage_type: DamageType,
    traces: &[AttackTrace],
    attacker: Option<EntityId>,
    inflictor: Option<EntityId>,
) -> Vec<DamageInfo> {
    let WeaponAction::Hitscan { count, .. } = action else {
        return Vec::new();
    };
    if !amount.is_finite() || amount <= 0.0 {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for trace in traces.iter().take(count as usize) {
        if trace.entity.is_none() {
            continue;
        }
        let mut info =
            DamageInfo::new(amount, damage_type).from_point(trace.end, trace.surface_normal);
        if let (Some(attacker), Some(inflictor)) = (attacker, inflictor) {
            info = info.from_entities(attacker, inflictor);
        }
        hits.push(info);
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ammo::AmmoType;
    use crate::weapons::spec;

    fn primary(down: bool) -> WeaponInput {
        WeaponInput {
            primary: down,
            ..Default::default()
        }
    }

    fn drawn(id: WeaponId, clip: u32, pool: &mut AmmoPool) -> FiringState {
        let mut state = FiringState::new(spec(id));
        assert_eq!(
            state.tick(
                1.0,
                WeaponInput {
                    select: true,
                    ..Default::default()
                },
                pool
            ),
            WeaponAction::PlaySequence(Sequence::Draw)
        );
        state.set_clip(clip);
        state
    }

    #[test]
    fn a_holstered_weapon_does_nothing_until_selected() {
        let mut pool = AmmoPool::new(AmmoType::NineMillimeter);
        let mut state = FiringState::new(spec(WeaponId::Glock));
        assert_eq!(
            state.tick(1.0, primary(true), &mut pool),
            WeaponAction::Empty
        );
        assert!(state.is_holstered());
    }

    #[test]
    fn fire_then_cycle_then_fire_again() {
        let mut pool = AmmoPool::new(AmmoType::NineMillimeter);
        pool.add(250);
        let mut state = drawn(WeaponId::Glock, 17, &mut pool);
        let cycle = spec(WeaponId::Glock).cycle_time.value;

        let first = state.tick(0.001, primary(true), &mut pool);
        assert_eq!(
            first,
            WeaponAction::Hitscan {
                count: 1,
                spread: 0.0
            }
        );
        assert_eq!(state.clip(), 16);

        // Still cycling: holding primary does not fire again early.
        let mid = state.tick(cycle / 2.0, primary(true), &mut pool);
        assert_eq!(mid, WeaponAction::Empty);
        assert_eq!(state.clip(), 16);

        // Past the cycle time, the tick that crosses `until` only completes
        // the cycle (back to `Idle`); the *next* tick with primary held
        // fires again, matching every other `Firing -> Idle` transition in
        // this state machine.
        let cycle_done = state.tick(cycle, primary(true), &mut pool);
        assert_eq!(cycle_done, WeaponAction::Empty);
        let second = state.tick(0.0, primary(true), &mut pool);
        assert_eq!(
            second,
            WeaponAction::Hitscan {
                count: 1,
                spread: 0.0
            }
        );
        assert_eq!(state.clip(), 15);
    }

    #[test]
    fn an_empty_clip_does_not_fire_and_a_full_pool_reloads() {
        let mut pool = AmmoPool::new(AmmoType::ThreeFiveSeven);
        pool.add(36);
        let mut state = drawn(WeaponId::Python, 0, &mut pool);

        let dry = state.tick(0.1, primary(true), &mut pool);
        assert_eq!(dry, WeaponAction::Sound(SoundKind::DryFire));
        assert_eq!(state.clip(), 0);

        let reload_started = state.tick(
            0.1,
            WeaponInput {
                reload: true,
                ..Default::default()
            },
            &mut pool,
        );
        assert_eq!(reload_started, WeaponAction::Sound(SoundKind::Reload));
        assert!(state.is_reloading());

        let reload_time = spec(WeaponId::Python).reload_time.value;
        let finished = state.tick(reload_time + 0.001, WeaponInput::default(), &mut pool);
        assert_eq!(finished, WeaponAction::PlaySequence(Sequence::Idle));
        assert_eq!(state.clip(), 6);
        assert_eq!(pool.current(), 30);
    }

    #[test]
    fn a_weapon_with_no_ammo_left_anywhere_stays_dry() {
        let mut pool = AmmoPool::new(AmmoType::ThreeFiveSeven);
        let mut state = drawn(WeaponId::Python, 0, &mut pool);
        assert_eq!(
            state.tick(0.1, primary(true), &mut pool),
            WeaponAction::Sound(SoundKind::DryFire)
        );
        // No reload starts either: the pool is empty too.
        assert!(!state.wants_reload(&pool));
    }

    #[test]
    fn a_quick_gauss_release_scales_damage_with_charge_time() {
        let mut pool = AmmoPool::new(AmmoType::Uranium);
        pool.add(100);
        let mut state = drawn(WeaponId::Gauss, 0, &mut pool);

        let charge_input = WeaponInput {
            secondary: true,
            ..Default::default()
        };
        assert_eq!(
            state.tick(0.001, charge_input, &mut pool),
            WeaponAction::Empty
        );
        assert!(state.is_charging());

        // Hold for 5 of the 10 published seconds, then release.
        assert_eq!(
            state.tick(5.0, charge_input, &mut pool),
            WeaponAction::Empty
        );
        let released = state.tick(0.0, WeaponInput::default(), &mut pool);
        assert_eq!(
            released,
            WeaponAction::Hitscan {
                count: 1,
                spread: 0.0
            }
        );
        let damage = state.take_charge_damage().expect("a charged shot released");
        assert!(
            (100.0..=115.0).contains(&damage),
            "damage {damage} should be roughly mid-range for ~5s of a 10s charge"
        );
        assert!(state.take_self_damage().is_none());
    }

    #[test]
    fn holding_the_gauss_charge_past_ten_seconds_hurts_the_wielder_instead() {
        let mut pool = AmmoPool::new(AmmoType::Uranium);
        pool.add(100);
        let mut state = drawn(WeaponId::Gauss, 0, &mut pool);
        let charge_input = WeaponInput {
            secondary: true,
            ..Default::default()
        };
        state.tick(0.001, charge_input, &mut pool);
        let overcharged = state.tick(GAUSS_OVERCHARGE_SECONDS + 1.0, charge_input, &mut pool);
        assert_eq!(overcharged, WeaponAction::Sound(SoundKind::Overcharge));
        assert_eq!(state.take_self_damage(), Some(GAUSS_OVERCHARGE_SELF_DAMAGE));
        assert!(state.take_charge_damage().is_none());
        assert!(!state.is_charging());
    }

    #[test]
    fn the_hornet_guns_clip_is_documented_as_regenerating_but_the_rate_is_a_black_box() {
        // The clip itself does not regenerate inside `FiringState`: the
        // regeneration interval is unpublished (`AmmoType::Hornets`'
        // published cap is cited, but no source gives a refill rate), so
        // this package only asserts that the placeholder exists and that a
        // depleted hornet gun simply goes dry like any other clip weapon,
        // rather than inventing a regen tick here.
        let mut pool = AmmoPool::new(AmmoType::Hornets);
        let mut state = drawn(WeaponId::HornetGun, 0, &mut pool);
        assert_eq!(
            state.tick(0.1, primary(true), &mut pool),
            WeaponAction::Sound(SoundKind::DryFire)
        );
    }

    #[test]
    fn an_egon_beam_drains_one_cell_per_cycle_and_stops_when_released() {
        let mut pool = AmmoPool::new(AmmoType::Uranium);
        pool.add(10);
        let mut state = drawn(WeaponId::Egon, 0, &mut pool);
        let interval = spec(WeaponId::Egon).cycle_time.value;

        let started = state.tick(0.001, primary(true), &mut pool);
        assert_eq!(started, WeaponAction::BeamTick);

        state.tick(interval, primary(true), &mut pool);
        assert_eq!(pool.current(), 9);

        let released = state.tick(0.001, WeaponInput::default(), &mut pool);
        assert_eq!(released, WeaponAction::PlaySequence(Sequence::Idle));
    }

    #[test]
    fn resolve_hitscan_reports_one_damage_info_per_entity_hit() {
        let hit = AttackTrace {
            fraction: 0.5,
            end: Vec3_ZERO,
            entity: Some(EntityId(1)),
            hitbox: Some(0),
            hitgroup: crate::trace::HitGroup::Generic,
            surface_normal: Vec3_ZERO,
        };
        let miss = AttackTrace::miss(Vec3_ZERO);
        let action = WeaponAction::Hitscan {
            count: 2,
            spread: 0.0,
        };
        let entry = spec(WeaponId::Python);
        let hits = resolve_hitscan(action, &entry, &[hit, miss], None, None);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].amount - entry.damage).abs() < f32::EPSILON);
        assert_eq!(hits[0].kind, entry.damage_type);
    }

    #[allow(non_upper_case_globals)]
    const Vec3_ZERO: glam::Vec3 = glam::Vec3::ZERO;
}
