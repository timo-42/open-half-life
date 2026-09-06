//! Pickup touch tests and use-and-hold chargers.
//!
//! Weapons, ammo boxes and items (`weapon_*`/`ammo_*`/`item_*`) and
//! chargers (`func_healthcharger`/`func_recharge`) are classified from
//! `Level::defs` the first time this phase runs for a level (see
//! [`PickupsState::ensure_spawned`]) and attached as
//! [`crate::components::Pickup`]/[`crate::components::Charger`] components
//! on the matching registry entity — a lazy, one-time pass rather than a
//! `Level`/`level.rs` change, so this package's touch list stays new files
//! only (`.plan/m79-design.md` §8, P1).
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

use crate::components::{Charger, Pickup};
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
}

/// The largest number of chargers one level may classify, so a pathological
/// map cannot make [`PickupsState::charger_kinds`] grow without bound.
const MAX_CHARGERS: usize = 256;

impl PickupsState {
    pub(crate) fn new() -> Self {
        Self {
            spawned: false,
            charger_kinds: Vec::new(),
        }
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
        touch_pickups(
            level,
            player_origin,
            player_tag,
            inventory,
            ammo,
            player,
            hud,
            presentation,
        );
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
) {
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

    for entity in touched {
        let Ok(kind) = level
            .registry
            .world
            .get::<&Pickup>(entity)
            .map(|pickup| pickup.kind)
        else {
            continue;
        };
        let taken = apply_pickup(kind, inventory, ammo, player);
        if taken {
            if let Ok(mut pickup) = level.registry.world.get::<&mut Pickup>(entity) {
                pickup.taken = true;
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
        }
    }
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
        // `PickupKind` is `#[non_exhaustive]`: `HealthCharger`/`SuitCharger`
        // are use-and-hold entities handled by `PickupsState::drain_chargers`
        // instead, and a future `ohl-combat` variant this engine does not
        // yet know about is simply not taken, rather than panicking.
        PickupKind::HealthCharger | PickupKind::SuitCharger | _ => false,
    }
}
