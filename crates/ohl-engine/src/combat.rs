//! Weapons, hit resolution and damage routing.
//!
//! [`CombatState`] owns everything M7.9 P1 adds to combat: the posed
//! [`HitboxIndex`] rebuilt every step (phase 5) and the player's weapon
//! [`Inventory`] and currently drawn weapon [`FiringState`] (phase 6).
//! [`QueuedDamage`] is the record [`CombatState::weapons`] deposits into
//! `crate::systems::Systems`'s one damage queue — shared with whatever
//! else deposits into it (monster attacks, projectiles, in later
//! packages) — which [`resolve_damage`] drains once (phase 9).
//!
//! # Two ammo ledgers, on purpose
//!
//! [`ohl_combat::Inventory`] tracks owned weapons, per-weapon clips and
//! selection natively, but its ammo pools only ever grow
//! ([`Inventory::give_ammo`] is the sole mutator, and it only adds). Firing
//! a weapon has to *spend* ammo, which that API cannot express. Rather than
//! edit `ohl-combat` (out of scope for this package; see
//! `.plan/m79-design.md` §0), this module keeps the engine's own
//! [`AmmoBank`] as the single source of truth for how much reserve ammo is
//! actually left, and never calls [`Inventory::give_ammo`] on the
//! long-lived [`Inventory`] this state owns — a pickup credits the bank
//! directly instead (see `crate::pickups`). [`CombatState::display_inventory`]
//! produces a short-lived [`Inventory`] with the bank's numbers stamped in,
//! exactly once, for a caller (the gameplay bridge, `Game::inventory`) that
//! needs a real `Inventory` value to read ammo from.
//!
//! # Monster health is P2's authority, not this module's
//!
//! [`resolve_damage`] (phase 9) leaves any [`crate::systems::QueuedDamage`]
//! whose target carries an `ohl_ai::MonsterAi` untouched in the queue: a
//! monster's live health is `ohl_ai::Actor::health`, moved only by
//! `ohl_ai::apply_monster_damage` in phase 10's `Systems::lifecycle`
//! (`crate::ai`), which is the one place that can report a death exactly
//! once. [`apply_entity_damage`] is therefore a generic fallback for a
//! non-player, non-monster entity that happens to carry a bare
//! [`ohl_combat::Health`] component (there is no such entity in this tree
//! yet, but the path costs nothing to keep general) — see that function's
//! own doc comment.
//!
//! # Clean-room
//!
//! Every number this module consumes — weapon damage, cycle time, clip
//! size, hit groups — is `ohl_combat`'s own published or explicitly
//! black-box-marked data; this module adds no number of its own beyond a
//! project-authored engagement range for hitscan and melee attacks, which
//! is itself a `TODO(black-box)` placeholder (see [`HITSCAN_RANGE`] and
//! [`MELEE_RANGE`]).

use ohl_combat::{
    AmmoPool, AmmoType, Armor, CombatEvent, DamageInfo, DamageType, EntityHitboxes, EntityId,
    FiringState, Health, HitboxIndex, Inventory, TraceFilter, TraceMask, WeaponAction, WeaponId,
    WeaponInput, WeaponSpec, resolve_hitscan_with_amount, spec, trace_attack_filtered,
};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_physics::{CollisionModel, PlayerController};
use ohl_world::StudioPose;

use crate::components::StudioAnim;
use crate::damage_map;
use crate::ids::{entity_id, entity_of};
use crate::level::Level;
use crate::presentation::Presentation;
use crate::systems::{LatchedInput, QueuedDamage};

/// How far a hitscan shot reaches. **To be black-box observed**: Half-Life's
/// per-weapon maximum range is not published on a source this project may
/// use; this is a neutral placeholder comfortably larger than any published
/// map's extents, not a measurement.
// TODO(black-box): replace with the measured per-weapon range, if one exists.
pub const HITSCAN_RANGE: f32 = 8192.0;

/// How far a melee swing reaches. **To be black-box observed**: see
/// [`HITSCAN_RANGE`].
// TODO(black-box): replace with the measured crowbar swing range.
pub const MELEE_RANGE: f32 = 48.0;

/// How many distinct ammo types [`AmmoBank`] tracks — one slot per
/// [`AmmoType::ALL`] entry.
const AMMO_SLOTS: usize = AmmoType::ALL.len();

/// The engine's own reserve-ammo ledger; see the module docs for why this
/// exists alongside [`Inventory`]'s own (grow-only) ammo pools.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AmmoBank {
    current: [u32; AMMO_SLOTS],
}

impl AmmoBank {
    fn new() -> Self {
        Self {
            current: [0; AMMO_SLOTS],
        }
    }

    fn slot(kind: AmmoType) -> usize {
        AmmoType::ALL
            .iter()
            .position(|candidate| *candidate == kind)
            .unwrap_or(0)
    }

    pub(crate) fn current(&self, kind: AmmoType) -> u32 {
        self.current[Self::slot(kind)]
    }

    /// Adds `amount`, clamped to `kind`'s published (or black-box) carry
    /// cap, matching [`Inventory::give_ammo`]'s own clamp exactly so the
    /// two ledgers agree on capacity even though only this one ever spends.
    pub(crate) fn add(&mut self, kind: AmmoType, amount: u32) -> u32 {
        let slot = Self::slot(kind);
        let cap = kind.default_capacity();
        let added = amount.min(cap.saturating_sub(self.current[slot]));
        self.current[slot] += added;
        added
    }

    fn set(&mut self, kind: AmmoType, value: u32) {
        self.current[Self::slot(kind)] = value.min(kind.default_capacity());
    }
}

/// Weapons, hit resolution and damage routing state, owned by
/// [`crate::systems::Systems`].
pub(crate) struct CombatState {
    inventory: Inventory,
    ammo: AmmoBank,
    /// The weapon [`Self::firing`] currently belongs to; `None` until a
    /// weapon has ever been selected.
    firing_weapon: Option<WeaponId>,
    firing: FiringState,
    /// How many times the player has actually fired a weapon (a hitscan,
    /// melee swing, beam tick or spawned projectile — not a dry-fire, a
    /// reload or a holster) since this level was attached. Media-derived:
    /// data, never a log line from this crate.
    fired_count: u64,
    /// How many of those shots landed on an entity. Media-derived: data,
    /// never a log line from this crate.
    hit_count: u64,
}

impl CombatState {
    pub(crate) fn new() -> Self {
        Self {
            inventory: Inventory::new(),
            ammo: AmmoBank::new(),
            firing_weapon: None,
            firing: FiringState::new(spec(WeaponId::Crowbar)),
            fired_count: 0,
            hit_count: 0,
        }
    }

    /// How many times the player has actually fired a weapon. Media-derived:
    /// data, never a log line from this crate.
    #[must_use]
    pub(crate) fn fired_count(&self) -> u64 {
        self.fired_count
    }

    /// How many of those shots landed on an entity. Media-derived: data,
    /// never a log line from this crate.
    #[must_use]
    pub(crate) fn hit_count(&self) -> u64 {
        self.hit_count
    }

    /// Mutable access to the weapon inventory and the reserve-ammo ledger
    /// together, for [`crate::pickups`] (the only other module allowed to
    /// grant weapons, ammo, suit or long jump). Returned as one disjoint
    /// pair rather than two separate accessors, so a caller can hold both
    /// mutably at once without the borrow checker seeing two overlapping
    /// borrows of `self`.
    pub(crate) fn inventory_and_ammo_mut(&mut self) -> (&mut Inventory, &mut AmmoBank) {
        (&mut self.inventory, &mut self.ammo)
    }

    /// A short-lived [`Inventory`] with the same owned weapons, clips and
    /// selection as the long-lived one, but with every ammo pool stamped
    /// from [`AmmoBank`] instead of the (never-written) pools inside it.
    /// This is the only `Inventory` value this crate ever shows a caller.
    pub(crate) fn display_inventory(&self) -> Inventory {
        let mut shown = self.inventory.clone();
        for kind in AmmoType::ALL {
            shown.give_ammo(kind, self.ammo.current(kind));
        }
        shown
    }

    /// Serializes owned weapons (and their clips), reserve ammo, the HEV
    /// suit and the long jump module into an opaque byte blob for
    /// [`crate::transition::PlayerCarryState::extra`], so a level change (or
    /// a save/load) carries them across.
    ///
    /// A save/load round trip already preserves this blob today: `extra`
    /// is plain data inside `PlayerCarryState`, and `GameSave` (`extra`
    /// blob included) is serialized whole into the `ohl-save` container, so
    /// no dedicated section is needed for that to work. `TODO(P4)`: fold
    /// this ad hoc encoding into its own `SECTION_INVENTORY`
    /// (`.plan/m79-design.md` §6) instead, so a save's inventory section is
    /// self-describing independent of `PlayerCarryState`'s shape. Weapon
    /// selection is deliberately not carried: `ohl_combat::Inventory`'s
    /// selection API is cycle-only (`select_next`/`select_prev`/
    /// `select_slot`), with no way to force an exact weapon back into
    /// place, so a level change simply holsters.
    pub(crate) fn capture_carry(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(WeaponId::ALL.len() * 5 + AmmoType::ALL.len() * 4 + 2);
        for id in WeaponId::ALL {
            bytes.push(u8::from(self.inventory.has_weapon(id)));
            bytes.extend_from_slice(&self.inventory.clip(id).to_le_bytes());
        }
        for kind in AmmoType::ALL {
            bytes.extend_from_slice(&self.ammo.current(kind).to_le_bytes());
        }
        bytes.push(u8::from(self.inventory.has_suit()));
        bytes.push(u8::from(self.inventory.has_long_jump()));
        bytes
    }

    /// Restores what [`Self::capture_carry`] serialized. A short or
    /// otherwise malformed blob (a save from before this existed, or a
    /// corrupt one) simply stops applying fields from where it runs out
    /// rather than panicking; anything not covered is left at its fresh
    /// (`CombatState::new`) default.
    pub(crate) fn restore_carry(&mut self, bytes: &[u8], player: &mut ohl_player::Player) {
        let mut cursor = 0usize;
        for id in WeaponId::ALL {
            let Some(&owned) = bytes.get(cursor) else {
                return;
            };
            cursor += 1;
            let Some(clip_bytes) = bytes.get(cursor..cursor + 4) else {
                return;
            };
            cursor += 4;
            if owned != 0 {
                self.inventory.give_weapon(id);
                let clip = u32::from_le_bytes(
                    clip_bytes
                        .try_into()
                        .expect("a 4-byte slice always converts to [u8; 4]"),
                );
                self.inventory.set_clip(id, clip);
            }
        }
        for kind in AmmoType::ALL {
            let Some(ammo_bytes) = bytes.get(cursor..cursor + 4) else {
                return;
            };
            cursor += 4;
            let amount = u32::from_le_bytes(
                ammo_bytes
                    .try_into()
                    .expect("a 4-byte slice always converts to [u8; 4]"),
            );
            self.ammo.set(kind, amount);
        }
        if let Some(&suit) = bytes.get(cursor) {
            cursor += 1;
            if suit != 0 {
                self.inventory.give_suit();
            }
            // Mirrors the flag directly onto `ohl_player::Player`'s own
            // state rather than through `Player::equip_suit`, which raises
            // a "suit equipped" voice event this restore is not one of.
            player.state.suit_equipped = suit != 0;
        }
        if let Some(&longjump) = bytes.get(cursor) {
            if longjump != 0 {
                self.inventory.give_long_jump();
            }
            player.state.longjump_owned = longjump != 0;
        }
    }

    /// A typed [`crate::save_state::InventorySnapshot`] of everything this
    /// state owns, for `SECTION_INVENTORY` (23). See that type's own docs
    /// for why [`Self::capture_carry`]'s opaque blob still rides along.
    pub(crate) fn snapshot(&self) -> crate::save_state::InventorySnapshot {
        let weapons = WeaponId::ALL
            .iter()
            .map(|&id| crate::save_state::WeaponSnapshot {
                owned: self.inventory.has_weapon(id),
                clip: self.inventory.clip(id),
            })
            .collect();
        let ammo = AmmoType::ALL
            .iter()
            .map(|&kind| self.ammo.current(kind))
            .collect();
        #[allow(clippy::cast_possible_truncation)]
        let selected = self
            .inventory
            .selected()
            .and_then(|id| WeaponId::ALL.iter().position(|&w| w == id))
            .map(|index| index as u8);
        #[allow(clippy::cast_possible_truncation)]
        let firing = self
            .firing_weapon
            .and_then(|id| {
                WeaponId::ALL
                    .iter()
                    .position(|&w| w == id)
                    .map(|index| index as u8)
            })
            .map(|weapon| {
                let (state_tag, timer) = self.firing.state_tag_and_timer();
                crate::save_state::FiringSnapshot {
                    weapon,
                    state_tag,
                    timer,
                }
            });
        crate::save_state::InventorySnapshot {
            weapons,
            ammo,
            selected,
            has_suit: self.inventory.has_suit(),
            has_long_jump: self.inventory.has_long_jump(),
            firing,
            legacy_carry: self.capture_carry(),
        }
    }

    /// Restores everything [`Self::snapshot`] captured. A `weapons`/`ammo`
    /// list shorter than `WeaponId::ALL`/`AmmoType::ALL` (a save from a
    /// build with fewer of either) simply leaves the remaining entries at
    /// their fresh default, the same "stop where it runs out" tolerance
    /// `restore_carry` already applies to the legacy blob.
    pub(crate) fn restore_snapshot(
        &mut self,
        snapshot: &crate::save_state::InventorySnapshot,
        player: &mut ohl_player::Player,
    ) {
        self.inventory = Inventory::new();
        self.ammo = AmmoBank::new();
        for (id, weapon) in WeaponId::ALL.iter().zip(&snapshot.weapons) {
            if weapon.owned {
                self.inventory.give_weapon(*id);
                self.inventory.set_clip(*id, weapon.clip);
            }
        }
        for (kind, &amount) in AmmoType::ALL.iter().zip(&snapshot.ammo) {
            self.ammo.set(*kind, amount);
        }
        if snapshot.has_suit {
            self.inventory.give_suit();
            player.state.suit_equipped = true;
        }
        if snapshot.has_long_jump {
            self.inventory.give_long_jump();
            player.state.longjump_owned = true;
        }
        if let Some(index) = snapshot.selected
            && let Some(&id) = WeaponId::ALL.get(index as usize)
        {
            // `Inventory`'s selection API is cycle-only; a bounded
            // `select_next` walk is the only way to land on an exact
            // weapon (see `crate::transition`'s own note on the same
            // limitation). Bounded by the closed weapon set, so a
            // save naming a weapon this player does not own cannot
            // loop.
            for _ in 0..=WeaponId::ALL.len() {
                if self.inventory.selected() == Some(id) {
                    break;
                }
                if self.inventory.select_next().is_none() {
                    break;
                }
            }
        }
        self.firing_weapon = None;
        if let Some(firing) = &snapshot.firing
            && let Some(&id) = WeaponId::ALL.get(firing.weapon as usize)
        {
            self.firing = FiringState::restore(
                spec(id),
                self.inventory.clip(id),
                firing.state_tag,
                firing.timer,
            );
            self.firing_weapon = Some(id);
        }
    }

    /// Selects `slot`'s weapon (as [`Inventory::select_slot`]) and resets
    /// the firing state machine to it when the selection actually changed.
    fn select_slot(&mut self, slot: u8) {
        if let Some(id) = self.inventory.select_slot(slot) {
            self.switch_to(id);
        }
    }

    fn switch_to(&mut self, id: WeaponId) {
        if self.firing_weapon != Some(id) {
            let mut firing = FiringState::new(spec(id));
            firing.set_clip(self.inventory.clip(id));
            self.firing = firing;
            self.firing_weapon = Some(id);
        }
    }

    /// Phase 6 — the firing state machine, its hitscan/melee/beam
    /// resolution (traced against world geometry and `hitboxes`, always
    /// ignoring `player_id`) and the gameplay bridge's viewmodel and sound
    /// reaction.
    ///
    /// `hitboxes` is rebuilt once per step by [`rebuild_hitbox_index`] and
    /// owned by `crate::systems::Systems` directly (not nested here), so a
    /// later package's own attack resolution (monster attacks, projectiles)
    /// can trace against the exact same index this phase built.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) fn weapons(
        &mut self,
        level: &Level,
        controller: &PlayerController,
        dt: f32,
        input: LatchedInput,
        player_id: Entity,
        hitboxes: &HitboxIndex,
        damage_queue: &mut Vec<QueuedDamage>,
        hud: &mut ohl_ui::hud::HudState,
        presentation: &mut Presentation,
    ) {
        let player_combat_id = entity_id(player_id);
        if let Some(slot) = input.select_slot {
            self.select_slot(slot);
        }
        let Some(selected) = self.inventory.selected() else {
            return;
        };
        if self.firing_weapon != Some(selected) {
            self.switch_to(selected);
        }

        let current_spec = *self.firing.spec();
        let mut pool = match current_spec.ammo {
            Some(kind) => {
                let mut pool = AmmoPool::new(kind);
                pool.add(self.ammo.current(kind));
                pool
            }
            None => AmmoPool::new(AmmoType::NineMillimeter),
        };

        let weapon_input = WeaponInput {
            primary: input.attack,
            secondary: input.attack2,
            reload: input.reload_pressed,
            select: self.firing.is_holstered(),
        };
        let action = self.firing.tick(dt, weapon_input, &mut pool);
        if let Some(kind) = current_spec.ammo {
            self.ammo.set(kind, pool.current());
        }
        self.inventory.set_clip(selected, self.firing.clip());

        if let Some(collision) = level.collision.as_ref() {
            match action {
                // TODO(black-box): `spread` (the cone half-angle a real
                // multi-pellet shot samples per-pellet) is discarded here;
                // every pellet of a `count > 1` shot (a shotgun) currently
                // traces the identical ray, so all `count` pellets always
                // land on the same entity. No usable source publishes
                // Half-Life's spread cones (see `ohl_combat::weapons`'s own
                // `BlackBox` marker on this field), and sampling one needs a
                // random source this package does not yet own.
                WeaponAction::Hitscan { count, .. } => {
                    self.fired_count += 1;
                    self.hit_count += Self::queue_ranged(
                        hitboxes,
                        collision,
                        controller,
                        player_combat_id,
                        &current_spec,
                        count,
                        HITSCAN_RANGE,
                        damage_queue,
                    ) as u64;
                }
                WeaponAction::Melee => {
                    self.fired_count += 1;
                    self.hit_count += Self::queue_ranged(
                        hitboxes,
                        collision,
                        controller,
                        player_combat_id,
                        &current_spec,
                        1,
                        MELEE_RANGE,
                        damage_queue,
                    ) as u64;
                }
                WeaponAction::BeamTick => {
                    self.fired_count += 1;
                    self.hit_count += Self::queue_ranged(
                        hitboxes,
                        collision,
                        controller,
                        player_combat_id,
                        &current_spec,
                        1,
                        HITSCAN_RANGE,
                        damage_queue,
                    ) as u64;
                }
                // A physical projectile from `WeaponAction::SpawnProjectile` is
                // simulated and resolved by `crate::projectiles` (M7.9 P3);
                // whether it lands is that module's own event, not counted
                // as a hit here, but firing it still counts as firing.
                WeaponAction::SpawnProjectile { .. } => {
                    self.fired_count += 1;
                }
                WeaponAction::PlaySequence(_) | WeaponAction::Sound(_) | WeaponAction::Empty => {}
            }
            if let Some(damage) = self.firing.take_charge_damage() {
                self.hit_count += Self::queue_amount(
                    hitboxes,
                    collision,
                    controller,
                    player_combat_id,
                    damage,
                    current_spec.damage_type,
                    damage_queue,
                ) as u64;
            }
        }
        if let Some(self_damage) = self.firing.take_self_damage() {
            // The gauss overcharge hurts its own wielder, not whatever is
            // downrange: queue it directly against the player.
            damage_queue.push(QueuedDamage {
                target: player_id,
                info: DamageInfo::new(self_damage, current_spec.damage_type)
                    .from_entities(player_combat_id, player_combat_id),
            });
        }

        #[allow(clippy::cast_possible_truncation)]
        let player_tag = player_combat_id.0 as u32;
        presentation.bridge.on_weapon_action(
            hud,
            player_tag,
            selected,
            action,
            &self.display_inventory(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_ranged(
        hitboxes: &HitboxIndex,
        collision: &CollisionModel,
        controller: &PlayerController,
        player_id: EntityId,
        spec: &WeaponSpec,
        count: u32,
        range: f32,
        damage_queue: &mut Vec<QueuedDamage>,
    ) -> usize {
        let start = controller.eye_position();
        let end = start + controller.view_direction() * range;
        let filter = TraceFilter::ignoring(TraceMask::SHOT, player_id);
        let action = WeaponAction::Hitscan { count, spread: 0.0 };
        let traces: Vec<_> = (0..count)
            .map(|_| trace_attack_filtered(collision, hitboxes, start, end, filter))
            .collect();
        // Built with the exact same filter `resolve_hitscan_with_amount`
        // applies internally (skip a trace with no `entity`), so this list
        // and `hits` stay aligned pair-for-pair.
        let targets: Vec<EntityId> = traces
            .iter()
            .take(count as usize)
            .filter_map(|trace| trace.entity)
            .collect();
        let hits = resolve_hitscan_with_amount(
            action,
            spec.damage,
            spec.damage_type,
            &traces,
            Some(player_id),
            Some(player_id),
        );
        let mut hit_count = 0usize;
        for (target_id, info) in targets.into_iter().zip(hits) {
            if let Some(target) = entity_of(target_id) {
                damage_queue.push(QueuedDamage { target, info });
                hit_count += 1;
            }
        }
        hit_count
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_amount(
        hitboxes: &HitboxIndex,
        collision: &CollisionModel,
        controller: &PlayerController,
        player_id: EntityId,
        amount: f32,
        damage_type: DamageType,
        damage_queue: &mut Vec<QueuedDamage>,
    ) -> usize {
        if !amount.is_finite() || amount <= 0.0 {
            return 0;
        }
        let start = controller.eye_position();
        let end = start + controller.view_direction() * HITSCAN_RANGE;
        let filter = TraceFilter::ignoring(TraceMask::SHOT, player_id);
        let trace = trace_attack_filtered(collision, hitboxes, start, end, filter);
        if let Some(target) = trace.entity.and_then(entity_of) {
            let info = DamageInfo::new(amount, damage_type)
                .from_point(trace.end, trace.surface_normal)
                .from_entities(player_id, player_id);
            damage_queue.push(QueuedDamage { target, info });
            1
        } else {
            0
        }
    }
}

/// Phase 5 — rebuilds `hitboxes` from every entity carrying a
/// [`StudioAnim`] (and so a pose to sample), cleared and refilled each
/// step. A monster or prop whose `anim.model` names no loaded slot
/// contributes nothing — never a panic; one whose sequence fails to
/// *sample* still contributes, but at its model's bind pose
/// ([`StudioPose::bind`]) rather than the requested frame, so a hitbox is
/// still there to be hit, just not posed the way the sequence would have
/// left it.
///
/// A free function, and `hitboxes` is owned by `crate::systems::Systems`
/// directly (not nested in `CombatState`), so a later package's own attack
/// resolution (monster attacks, projectiles) traces against the exact same
/// index this phase built, instead of rebuilding its own. Every
/// model-backed entity is included, deliberately: this project keeps a
/// projectile's own drawn model (a flying rocket) and a placed
/// deployable's (a tripmine, a satchel) in the shared index rather than
/// excluding either from it, because a deployable must stay shootable and
/// damageable by anyone else's trace. What a projectile's own movement
/// trace must not hit — itself, and its owner — is instead ignored per
/// trace, in `ohl_combat::ProjectileSet::tick` itself (see
/// `crate::projectiles`' module doc), not by narrowing this index.
pub(crate) fn rebuild_hitbox_index(hitboxes: &mut HitboxIndex, level: &Level) {
    hitboxes.clear();
    for (entity, anim, transform) in &mut level
        .registry
        .world
        .query::<(Entity, &StudioAnim, &Transform)>()
    {
        let Some(model) = level.studio_models.get(anim.model) else {
            continue;
        };
        let pose = StudioPose::sample(model, anim.sequence, anim.cycle)
            .unwrap_or_else(|_| StudioPose::bind(model));
        let mut entry = EntityHitboxes::from_transform(entity_id(entity), transform);
        entry.push_studio_hitboxes(&pose, &model.hitboxes);
        hitboxes.push(entry);
    }
}

/// Phase 9 — drains the damage queue once, in insertion order. Damage aimed
/// at the player is routed to `player` (the player's own health/armor/suit
/// reactions, per `.plan/m79-design.md` §3); damage aimed at anything else
/// is routed to that entity's [`ohl_combat::Health`]/[`ohl_combat::Armor`]
/// components, when it still has them.
///
/// A free function (not a `CombatState` method) and keyed by `Entity`
/// rather than `ohl_combat::EntityId`: [`QueuedDamage`] is shared with
/// whatever else deposits into `crate::systems::Systems`'s one damage
/// queue (monster attacks, projectiles, in later packages), none of which
/// need to reach back into `CombatState` to resolve it.
///
/// A `QueuedDamage` whose target carries an `ohl_ai::MonsterAi` is left
/// untouched in `damage_queue` rather than drained here: phase 10
/// (`Systems::lifecycle`, `crate::ai`) is the one place a monster's health
/// moves, through `ohl_ai::apply_monster_damage`, and it runs its own drain
/// immediately after this phase. Everything else in the queue — the
/// player, or any other entity that is not a monster — is this function's
/// to resolve, so damage aimed at the player is never silently dropped by
/// phase 10's drain finding a target it does not recognise.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_damage(
    damage_queue: &mut Vec<QueuedDamage>,
    level: &mut Level,
    player: &mut ohl_player::Player,
    player_id: Entity,
    hud: &mut ohl_ui::hud::HudState,
    presentation: &mut Presentation,
    player_events: &mut Vec<ohl_player::PlayerEvent>,
    player_damage_events: &mut u64,
) {
    let player_combat_id = entity_id(player_id);
    let mut left_for_lifecycle = Vec::with_capacity(damage_queue.len());
    for queued in damage_queue.drain(..) {
        if level
            .registry
            .world
            .get::<&ohl_ai::MonsterAi>(queued.target)
            .is_ok()
        {
            left_for_lifecycle.push(queued);
            continue;
        }
        let QueuedDamage { target, info } = queued;
        if target == player_id {
            *player_damage_events += 1;
            let kind = damage_map::damage_kind_of(info.kind);
            let mut events = Vec::new();
            player.apply_damage(info.amount, kind, &mut events);
            sync_player_components(level, player);

            let combat_health = Health {
                current: player.state.health,
                max: player.config.max_health,
            };
            let combat_armor = Armor {
                current: player.state.armor,
                max: player.config.max_armor,
            };
            presentation.bridge.on_combat_event(
                hud,
                player_combat_id,
                CombatEvent::DamageDealt {
                    target: player_combat_id,
                    attacker: info.attacker,
                    health_lost: info.amount,
                    armor_lost: 0.0,
                    kind: info.kind,
                },
                &combat_health,
                &combat_armor,
            );
            player_events.extend(events);
        } else if level.registry.world.contains(target) {
            apply_entity_damage(level, target, &info);
        }
    }
    *damage_queue = left_for_lifecycle;
}

/// Mirrors `player`'s health and armor onto the player entity's
/// [`ohl_combat::Health`]/[`ohl_combat::Armor`] components, so any other
/// system reading the world (a monster's target selection, in a later
/// package) sees the same numbers `ohl_player::Player` owns.
pub(crate) fn sync_player_components(level: &mut Level, player: &ohl_player::Player) {
    if let Ok(mut health) = level.registry.world.get::<&mut Health>(level.player) {
        health.current = player.state.health;
    }
    if let Ok(mut armor) = level.registry.world.get::<&mut Armor>(level.player) {
        armor.current = player.state.armor;
    }
}

/// Applies `info` to `entity`'s [`Health`]/[`Armor`] components, when it has
/// a [`Health`] to apply to. The armour split uses the neutral
/// [`ohl_combat::ArmorRule::default`]: the real HEV/monster-armour split is
/// **to be black-box observed** (`ohl_combat::damage`'s own module docs).
///
/// **Not the path a monster's health should resolve through once M7.9 P2
/// lands.** P1 has no monsters (`ohl-ai` is not a dependency of this
/// package), so this is a generic fallback for *any* non-player entity that
/// carries a bare [`Health`] component, not a monster-specific one. P2
/// gives every monster an `ohl_combat::Health` written once at spawn and
/// never updated from there; the entity's *live* health is
/// `ohl_ai::Actor::health`, moved only by `ohl_ai::apply_monster_damage`.
/// The explicit choice for the merged engine is: monster damage routes
/// through `apply_monster_damage` (`Actor` stays authoritative), and if a
/// monster's `Health` component is kept at all it is mirrored from `Actor`
/// after resolution, never written independently — this function must not
/// be the thing that mutates a monster's `Health` once P2's `Actor` exists.
fn apply_entity_damage(level: &mut Level, entity: Entity, info: &DamageInfo) {
    let Ok(mut health) = level.registry.world.get::<&mut Health>(entity) else {
        return;
    };
    let mut armor = level.registry.world.get::<&mut Armor>(entity).ok();
    let _ = ohl_combat::apply_damage(
        &mut health,
        armor.as_deref_mut(),
        info,
        ohl_combat::ArmorRule::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::AmmoBank;
    use ohl_combat::AmmoType;

    #[test]
    fn a_bank_never_exceeds_the_published_cap_and_tracks_each_type_separately() {
        let mut bank = AmmoBank::new();
        assert_eq!(bank.add(AmmoType::NineMillimeter, 1_000), 250);
        assert_eq!(bank.current(AmmoType::NineMillimeter), 250);
        assert_eq!(bank.current(AmmoType::Buckshot), 0);
        bank.set(AmmoType::Buckshot, 40);
        assert_eq!(bank.current(AmmoType::Buckshot), 40);
        assert_eq!(bank.current(AmmoType::NineMillimeter), 250);
    }
}

#[cfg(test)]
mod weapon_wiring_tests {
    use super::{CombatState, QueuedDamage};
    use crate::assets::MemoryAssets;
    use crate::level::Level;
    use crate::presentation::Presentation;
    use crate::systems::LatchedInput;
    use crate::test_support::synthetic_map_bsp;
    use glam::Vec3;
    use ohl_combat::{
        AmmoType, EntityHitboxes, HitGroup, HitboxIndex, HitboxLimits, WeaponId, hud_slot, spec,
    };
    use ohl_game::hecs::Entity;
    use ohl_physics::PlayerController;

    fn synthetic_level() -> Level {
        let bytes = synthetic_map_bsp();
        let mut assets = MemoryAssets::new();
        assets.insert("maps/ohlsynth.bsp", bytes.clone());
        Level::from_bytes(&assets, "ohlsynth", &bytes).expect("synthetic level loads")
    }

    /// A level with one weaponized player, a hitbox index the test controls
    /// directly (as `crate::systems::Systems` would own it, rebuilt each
    /// step by [`rebuild_hitbox_index`]), and a bare `Entity` (no other
    /// components) standing in for a shootable target — positioned
    /// straight down the spawn's forward axis at eye height, well short of
    /// the room's own far wall so the trace hits the entity rather than
    /// world geometry.
    fn armed_glock() -> (CombatState, Level, PlayerController, HitboxIndex, Entity) {
        let mut level = synthetic_level();
        let spawn = level.spawn.expect("fixture publishes a spawn");
        let controller =
            PlayerController::spawn_at(Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch);
        let mut combat = CombatState::new();
        combat.inventory.give_weapon(WeaponId::Glock);
        combat.ammo.add(AmmoType::NineMillimeter, 250);
        combat.select_slot(hud_slot(WeaponId::Glock).slot);
        combat
            .firing
            .set_clip(spec(WeaponId::Glock).clip_size.unwrap_or(0));

        let mut hitboxes = HitboxIndex::new(HitboxLimits::default());
        let target = level.registry.world.spawn(());
        let eye_z = controller.eye_position().z;
        let mut target_hitbox =
            EntityHitboxes::new(crate::ids::entity_id(target), Vec3::new(64.0, 0.0, eye_z));
        target_hitbox.push_box(0, Vec3::splat(-8.0), Vec3::splat(8.0), HitGroup::Generic);
        hitboxes.push(target_hitbox);
        (combat, level, controller, hitboxes, target)
    }

    fn firing_input() -> LatchedInput {
        LatchedInput {
            attack: true,
            ..LatchedInput::default()
        }
    }

    /// Fires twice: the first tick only draws the weapon (`Holstered` ->
    /// `Idle`), the second is the one that actually fires.
    #[allow(clippy::too_many_arguments)]
    fn fire_twice(
        combat: &mut CombatState,
        level: &Level,
        controller: &PlayerController,
        player_id: Entity,
        hitboxes: &HitboxIndex,
        damage_queue: &mut Vec<QueuedDamage>,
        hud: &mut ohl_ui::hud::HudState,
        presentation: &mut Presentation,
    ) {
        for _ in 0..2 {
            combat.weapons(
                level,
                controller,
                0.001,
                firing_input(),
                player_id,
                hitboxes,
                damage_queue,
                hud,
                presentation,
            );
        }
    }

    /// Firing at a synthetic target deposits exactly the weapon's published
    /// per-shot damage into the damage queue.
    #[test]
    fn firing_at_a_synthetic_target_deposits_the_published_damage() {
        let (mut combat, level, controller, hitboxes, target) = armed_glock();
        let mut presentation = Presentation::new();
        let mut hud = ohl_ui::hud::HudState::default();
        let mut damage_queue = Vec::new();

        fire_twice(
            &mut combat,
            &level,
            &controller,
            level.player,
            &hitboxes,
            &mut damage_queue,
            &mut hud,
            &mut presentation,
        );

        assert_eq!(damage_queue.len(), 1);
        assert_eq!(damage_queue[0].target, target);
        let expected = spec(WeaponId::Glock).damage;
        assert!(
            (damage_queue[0].info.amount - expected).abs() < f32::EPSILON,
            "expected {expected}, got {}",
            damage_queue[0].info.amount
        );
    }

    /// A trace that would otherwise pass through the player entity on its
    /// way to a target behind it must ignore the player and hit the target
    /// instead — never the player.
    #[test]
    fn a_shot_never_hits_the_player_even_when_the_player_is_in_the_index() {
        let (mut combat, level, controller, mut hitboxes, target) = armed_glock();

        // The player's own entity sits in the index too, closer to the
        // shooter than the real target, on the same forward axis.
        let eye_z = controller.eye_position().z;
        let mut player_hitbox = EntityHitboxes::new(
            crate::ids::entity_id(level.player),
            Vec3::new(32.0, 0.0, eye_z),
        );
        player_hitbox.push_box(0, Vec3::splat(-8.0), Vec3::splat(8.0), HitGroup::Generic);
        hitboxes.push(player_hitbox);

        let mut presentation = Presentation::new();
        let mut hud = ohl_ui::hud::HudState::default();
        let mut damage_queue = Vec::new();
        fire_twice(
            &mut combat,
            &level,
            &controller,
            level.player,
            &hitboxes,
            &mut damage_queue,
            &mut hud,
            &mut presentation,
        );

        assert_eq!(damage_queue.len(), 1);
        assert_eq!(
            damage_queue[0].target, target,
            "the player must never be the hit entity"
        );
    }
}
