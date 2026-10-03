//! Pickup touch tests and use-and-hold chargers.
//!
//! Weapons, ammo boxes and items (`weapon_*`/`ammo_*`/`item_*`) and
//! chargers (`func_healthcharger`/`func_recharge`) are classified from
//! `Level::defs` the first time this phase runs for a level (see
//! [`PickupsState::ensure_spawned`]) and attached as
//! [`crate::components::Pickup`]/[`crate::components::Charger`] components
//! on the matching registry entity — a lazy, one-time pass rather than a
//! `Level`/`level.rs` change, so this package's touch list stays new files
//! only (§8, P1 of the M7.9 design plan, recorded in local design notes
//! and not part of the repository).
//!
//! This module never calls `ohl_combat::try_pickup`: that function owns a
//! target's health and armour by `&mut Health`/`&mut Armor`, but this
//! engine's player health and armour live in `ohl_player::Player`, not in
//! those components (see `crate::damage_map`'s module docs for the same
//! split). Instead each [`ohl_combat::PickupKind`] is applied directly,
//! against the same published constants `try_pickup` itself uses
//! (`ohl_combat::pickups`), so no number is invented here.
//!
//! `TODO(black-box)`: the touch radius below, and single-player pickup
//! respawn behaviour (not modeled at all: a taken pickup stays taken).

use glam::Vec3;
use ohl_combat::{
    AmmoType, BATTERY_AMOUNT, ChargerState, Difficulty, HEALTHKIT_AMOUNT, PickupKind,
    ammo_pickup_amount, spec, weapon_pickup_ammo,
};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_player::Player;

use crate::components::{Charger, Pickup, WeaponBox};
use crate::level::Level;
use crate::systems::LatchedInput;

/// How close the player must stand to a pickup or a charger for it to
/// register. **To be black-box observed**: Half-Life's touch volume is the
/// pickup's own bounding box, not a sphere; this radius is a neutral
/// placeholder standing in for it.
// TODO(black-box): replace with a real bounding-box touch test.
pub const PICKUP_TOUCH_RADIUS: f32 = 32.0;

/// Which reservoir one [`Charger`] entity restores. `ohl_combat::Charger`'s
/// wrapped [`ChargerState`] does not record this itself, so this engine
/// remembers it from the classification that created the component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChargerKind {
    Health,
    Suit,
}

/// Chargers, pickups and their one-time classification pass.
pub(crate) struct PickupsState {
    spawned: bool,
    /// One entry per charger entity, in classification order; bounded by
    /// how many `func_healthcharger`/`func_recharge` a map can declare, so
    /// this never grows without limit.
    charger_kinds: Vec<(Entity, ChargerKind)>,
    /// How many pickups have actually been taken since this level was
    /// attached. Media-derived: data, never a log line from this crate —
    /// `ohl-app`'s `--script-log` reads it to emit "A pickup was
    /// collected." at most once per run.
    taken_count: u64,
}

/// The largest number of chargers one level may classify, so a pathological
/// map cannot make [`PickupsState::charger_kinds`] grow without bound.
const MAX_CHARGERS: usize = 256;

impl PickupsState {
    pub(crate) fn new() -> Self {
        Self {
            spawned: false,
            charger_kinds: Vec::new(),
            taken_count: 0,
        }
    }

    /// How many pickups have actually been taken since this level was
    /// attached. Media-derived: data, never a log line from this crate.
    #[must_use]
    pub(crate) fn taken_count(&self) -> u64 {
        self.taken_count
    }

    /// Attaches [`Pickup`]/[`Charger`] components to every entity
    /// `ohl_gameplay::classify_entity` recognises, once per level.
    fn ensure_spawned(&mut self, level: &mut Level) {
        if self.spawned {
            return;
        }
        self.spawned = true;
        for (def, entity) in level.defs.iter().zip(level.registry.entities.iter()) {
            let Some(kind) = ohl_gameplay::classify_entity(def) else {
                continue;
            };
            let entity = *entity;
            match kind {
                PickupKind::HealthCharger => {
                    let _ = level
                        .registry
                        .world
                        .insert_one(entity, Charger(ChargerState::health()));
                    if self.charger_kinds.len() < MAX_CHARGERS {
                        self.charger_kinds.push((entity, ChargerKind::Health));
                    }
                }
                PickupKind::SuitCharger => {
                    // The suit charger's reservoir is published per
                    // difficulty; medium is this engine's neutral default
                    // until `Game::difficulty` is threaded through here.
                    let _ = level
                        .registry
                        .world
                        .insert_one(entity, Charger(ChargerState::suit(Difficulty::Medium)));
                    if self.charger_kinds.len() < MAX_CHARGERS {
                        self.charger_kinds.push((entity, ChargerKind::Suit));
                    }
                }
                PickupKind::WeaponBox => {
                    // A `weaponbox`'s contents *are* its keyvalues, so
                    // they are parsed once here, against the published key
                    // table in `ohl_combat`, rather than re-read on every
                    // touch test.
                    let contents = weaponbox_contents(def);
                    let _ = level.registry.world.insert_one(entity, Pickup::new(kind));
                    let _ = level
                        .registry
                        .world
                        .insert_one(entity, WeaponBox { contents });
                }
                _ => {
                    let _ = level.registry.world.insert_one(entity, Pickup::new(kind));
                }
            }
        }
    }

    fn kind_of(&self, entity: Entity) -> Option<ChargerKind> {
        self.charger_kinds
            .iter()
            .find(|(candidate, _)| *candidate == entity)
            .map(|(_, kind)| *kind)
    }

    /// Phase 11 — touch tests every untaken [`Pickup`] against the player's
    /// origin, and drains every [`Charger`] within reach while `use` is
    /// held this step.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(
        &mut self,
        level: &mut Level,
        player_origin: Vec3,
        player_tag: u32,
        input: LatchedInput,
        dt: f32,
        inventory: &mut ohl_combat::Inventory,
        ammo: &mut crate::combat::AmmoBank,
        player: &mut Player,
        hud: &mut ohl_ui::hud::HudState,
        presentation: &mut crate::presentation::Presentation,
    ) {
        self.ensure_spawned(level);
        self.taken_count += touch_pickups(
            level,
            player_origin,
            player_tag,
            inventory,
            ammo,
            player,
            hud,
            presentation,
        ) as u64;
        if input.use_held {
            self.drain_chargers(level, player_origin, dt, player);
        }
    }

    /// Drains every charger within [`PICKUP_TOUCH_RADIUS`] of
    /// `player_origin` by `dt` seconds' worth of charge into `player`.
    fn drain_chargers(&self, level: &mut Level, player_origin: Vec3, dt: f32, player: &mut Player) {
        let mut nearby: Vec<Entity> = Vec::new();
        for (entity, _charger, transform) in &mut level
            .registry
            .world
            .query::<(Entity, &Charger, &Transform)>()
        {
            if transform.origin.distance(player_origin) <= PICKUP_TOUCH_RADIUS {
                nearby.push(entity);
            }
        }

        for entity in nearby {
            let Some(kind) = self.kind_of(entity) else {
                continue;
            };
            let Ok(mut charger) = level.registry.world.get::<&mut Charger>(entity) else {
                continue;
            };
            let mut events = Vec::new();
            match kind {
                ChargerKind::Suit => {
                    if !player.state.suit_equipped || player.state.armor >= player.config.max_armor
                    {
                        continue;
                    }
                    let mut armor = ohl_combat::Armor {
                        current: player.state.armor,
                        max: player.config.max_armor,
                    };
                    let restored = charger.0.drain_armor(&mut armor, dt);
                    if restored > 0.0 {
                        player.add_armor(restored, &mut events);
                    }
                }
                ChargerKind::Health => {
                    if player.state.health >= player.config.max_health {
                        continue;
                    }
                    let mut health = ohl_combat::Health {
                        current: player.state.health,
                        max: player.config.max_health,
                    };
                    let restored = charger.0.drain_health(&mut health, dt);
                    if restored > 0.0 {
                        player.heal(restored, &mut events);
                    }
                }
            }
        }
    }
}

/// Touches every untaken pickup within [`PICKUP_TOUCH_RADIUS`] of
/// `player_origin`, notifying the gameplay bridge (its HUD message and
/// sound cue) for each one actually taken.
#[allow(clippy::too_many_arguments)]
fn touch_pickups(
    level: &mut Level,
    player_origin: Vec3,
    player_tag: u32,
    inventory: &mut ohl_combat::Inventory,
    ammo: &mut crate::combat::AmmoBank,
    player: &mut Player,
    hud: &mut ohl_ui::hud::HudState,
    presentation: &mut crate::presentation::Presentation,
) -> usize {
    let mut touched: Vec<Entity> = Vec::new();
    for (entity, pickup, transform) in &mut level
        .registry
        .world
        .query::<(Entity, &Pickup, &Transform)>()
    {
        if !pickup.taken && transform.origin.distance(player_origin) <= PICKUP_TOUCH_RADIUS {
            touched.push(entity);
        }
    }

    let mut taken_count = 0usize;
    for entity in touched {
        let Ok(kind) = level
            .registry
            .world
            .get::<&Pickup>(entity)
            .map(|pickup| pickup.kind)
        else {
            continue;
        };
        let taken = if kind == PickupKind::WeaponBox {
            take_weapon_box(level, entity, ammo)
        } else {
            apply_pickup(kind, inventory, ammo, player)
        };
        if taken {
            if let Ok(mut pickup) = level.registry.world.get::<&mut Pickup>(entity) {
                pickup.taken = true;
            }
            if kind == PickupKind::SecurityCard {
                fire_pickup_target(level, entity);
            }
            presentation.bridge.on_pickup(
                hud,
                player_tag,
                kind,
                ohl_combat::PickupOutcome {
                    taken: true,
                    remaining: 0.0,
                },
            );
            taken_count += 1;
        }
    }
    taken_count
}

/// Fires the `target` of a pickup that was just taken, the way a map wires an
/// `item_security` to the rest of its logic: TWHL's `item_security` page
/// documents its "pickup ability" as "often used as a way to unlock other kinds
/// of entities (by targeting a `multisource` which is the master of a door for
/// example)" (`docs/FORMAT_SOURCES.md`, "Map entities the registry used to
/// drop").
///
/// Only the security card does this here. No reviewed page says the same
/// of a weapon, ammo box or other item, and firing a target nobody
/// documented would change what every campaign map does on a pickup.
fn fire_pickup_target(level: &mut Level, entity: Entity) {
    let Some(target) = level
        .registry
        .world
        .get::<&ohl_game::registry::Target>(entity)
        .ok()
        .map(|target| target.0.clone())
    else {
        return;
    };
    level.simulation.fire(target, None, 0.0);
}

/// The (ammo type, units) pairs `def`'s own keyvalues stock a `weaponbox`
/// with, in the keyvalue table's own (sorted) key order.
///
/// Every key is matched exactly, through
/// [`ohl_combat::weaponbox_ammo_key`], so a key that is not one of the
/// published (case-sensitive) ammo names — `classname`, `origin`, `angles`
/// and everything else a map editor writes — simply stocks nothing. A value
/// that is not a non-negative integer stocks nothing either, rather than
/// being guessed at. Each key appears once in an [`ohl_game::EntityDef`],
/// so the list holds at most one entry per published key — well inside the
/// page's own "limit of 32 ammo inputs".
fn weaponbox_contents(def: &ohl_game::EntityDef) -> Vec<(AmmoType, u32)> {
    let mut contents = Vec::new();
    for (key, value) in &def.keyvalues {
        let Some(kind) = ohl_combat::weaponbox_ammo_key(key.as_str()) else {
            continue;
        };
        let Ok(amount) = value.trim().parse::<u32>() else {
            continue;
        };
        if amount > 0 {
            contents.push((kind, amount));
        }
    }
    contents
}

/// Applies one `weaponbox`'s stocked ammo, which [`apply_pickup`] cannot:
/// its contents are per-entity map data held in a [`WeaponBox`] component
/// rather than a published constant.
///
/// Always reports the box taken, unlike an `ammo_*` box that finds no
/// room: TWHL's `weaponbox` page documents that "even if the ammunition
/// load of a carried weapon is full, this entity will be picked up
/// permanently". Whatever does not fit is simply lost with it.
fn take_weapon_box(level: &Level, entity: Entity, ammo: &mut crate::combat::AmmoBank) -> bool {
    if let Ok(contents) = level
        .registry
        .world
        .get::<&WeaponBox>(entity)
        .map(|stock| stock.contents.clone())
    {
        for (kind, amount) in contents {
            ammo.add(kind, amount);
        }
    }
    true
}

/// Applies one pickup's effect; returns whether anything was actually
/// taken (a full pool, an already-owned flag item and a battery with no
/// suit yet all report `false` and leave the entity untaken).
fn apply_pickup(
    kind: PickupKind,
    inventory: &mut ohl_combat::Inventory,
    ammo: &mut crate::combat::AmmoBank,
    player: &mut Player,
) -> bool {
    let mut events = Vec::new();
    match kind {
        PickupKind::Weapon(id) => {
            let is_new = inventory.give_weapon(id);
            let ammo_taken = spec(id)
                .ammo
                .is_some_and(|kind: AmmoType| ammo.add(kind, weapon_pickup_ammo(id).value) > 0);
            is_new || ammo_taken
        }
        PickupKind::Ammo(kind) => ammo.add(kind, ammo_pickup_amount(kind).value) > 0,
        PickupKind::HealthKit => {
            let before = player.state.health;
            player.heal(HEALTHKIT_AMOUNT.value, &mut events);
            player.state.health > before
        }
        PickupKind::Battery => {
            if !player.state.suit_equipped {
                return false;
            }
            let before = player.state.armor;
            player.add_armor(BATTERY_AMOUNT.value, &mut events);
            player.state.armor > before
        }
        PickupKind::Suit => {
            let was_new = inventory.give_suit();
            if was_new {
                player.equip_suit(&mut events);
            }
            was_new
        }
        PickupKind::LongJump => {
            let was_new = inventory.give_long_jump();
            if was_new {
                player.give_long_jump(&mut events);
            }
            was_new
        }
        // The published `item_security` has no "game related behavior"
        // besides being carried, and "can be picked up without the suit":
        // always taken. Its `target` is fired by `touch_pickups`, which has
        // the map logic this function is not given.
        PickupKind::SecurityCard => true,
        // `PickupKind` is `#[non_exhaustive]`: `HealthCharger`/`SuitCharger`
        // are use-and-hold entities handled by `PickupsState::drain_chargers`
        // instead, `WeaponBox` is applied by `take_weapon_box` (its contents
        // are per-entity map data this function is not given), and a future
        // `ohl-combat` variant this engine does not yet know about is simply
        // not taken, rather than panicking.
        PickupKind::HealthCharger | PickupKind::SuitCharger | PickupKind::WeaponBox | _ => false,
    }
}
