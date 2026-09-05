//! [`GameplayBridge`]: turns `ohl-combat` simulation output — combat events,
//! weapon actions and pickup outcomes — into `ohl-ui` HUD updates, queued
//! [`SoundCue`]s and queued [`ViewModelAction`]s.
//!
//! The bridge holds no simulation state of its own beyond its two bounded
//! output queues (see [`crate::queue::BoundedQueue`]): every `on_*` method
//! is a pure function of its arguments plus those queues, and `HudState` is
//! always written from the caller's current `Health`/`Armor`/`Inventory`
//! rather than accumulated by repeated subtraction, so replaying the same
//! input sequence from the same starting state always produces the same
//! HUD contents and queue contents. `ohl-combat` has no dependency on
//! `ohl-audio` or `ohl-ui` (see its own module docs); this crate is exactly
//! the composition-root-side glue that closes that gap without either of
//! those crates needing to depend on `ohl-combat` in turn.

use ohl_audio::ChannelClass;
use ohl_combat::{
    Armor, CombatEvent, EntityId, Health, Inventory, PickupKind, PickupOutcome, WeaponAction,
    WeaponId, spec,
};
use ohl_ui::hud::HudState;

use crate::queue::BoundedQueue;
use crate::sounds::{SoundCue, pickup_sound_path, weapon_sound_path};
use crate::viewmodel::{ViewModelAction, from_weapon_action};

/// Default bounded capacity for both output queues: generous for a busy
/// tick (a multi-pellet shot plus a pickup) with room to spare, the same
/// reasoning `ohl_combat::CombatEventQueue::DEFAULT_CAPACITY` documents.
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;

/// How long a pickup's HUD message stays on screen. A project-defined UI
/// choice (see `ohl_ui::HudState::show_message`), not gameplay data.
pub const PICKUP_MESSAGE_SECONDS: f32 = 2.0;

/// Bridges combat simulation output into presentation. See the module docs.
#[derive(Debug)]
pub struct GameplayBridge {
    sounds: BoundedQueue<SoundCue>,
    viewmodel: BoundedQueue<ViewModelAction>,
}

impl Default for GameplayBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl GameplayBridge {
    /// A bridge with [`DEFAULT_QUEUE_CAPACITY`] on both output queues.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacities(DEFAULT_QUEUE_CAPACITY, DEFAULT_QUEUE_CAPACITY)
    }

    /// A bridge with explicit output-queue capacities (each at least one).
    #[must_use]
    pub fn with_capacities(sound_capacity: usize, viewmodel_capacity: usize) -> Self {
        Self {
            sounds: BoundedQueue::with_capacity(sound_capacity),
            viewmodel: BoundedQueue::with_capacity(viewmodel_capacity),
        }
    }

    /// The currently queued sound cues, oldest first.
    #[must_use]
    pub fn queued_sounds(&self) -> &[SoundCue] {
        self.sounds.items()
    }

    /// The currently queued viewmodel actions, oldest first.
    #[must_use]
    pub fn queued_viewmodel_actions(&self) -> &[ViewModelAction] {
        self.viewmodel.items()
    }

    /// How many sound cues have been dropped since the last drain, because
    /// the queue was full.
    #[must_use]
    pub fn dropped_sounds(&self) -> usize {
        self.sounds.dropped()
    }

    /// How many viewmodel actions have been dropped since the last drain.
    #[must_use]
    pub fn dropped_viewmodel_actions(&self) -> usize {
        self.viewmodel.dropped()
    }

    /// The sound-cue output queue's capacity.
    #[must_use]
    pub fn sound_capacity(&self) -> usize {
        self.sounds.capacity()
    }

    /// The viewmodel-action output queue's capacity.
    #[must_use]
    pub fn viewmodel_capacity(&self) -> usize {
        self.viewmodel.capacity()
    }

    /// Drains every queued sound cue, oldest first.
    pub fn drain_sounds(&mut self) -> impl Iterator<Item = SoundCue> + '_ {
        self.sounds.drain()
    }

    /// Drains every queued viewmodel action, oldest first.
    pub fn drain_viewmodel_actions(&mut self) -> impl Iterator<Item = ViewModelAction> + '_ {
        self.viewmodel.drain()
    }

    /// Applies one `WeaponAction` produced by the player's `weapon` this
    /// tick: refreshes `hud`'s clip/reserve ammo from `inventory`, and
    /// queues the viewmodel action and sound cue the action implies, if
    /// any (see [`crate::viewmodel::from_weapon_action`]).
    pub fn on_weapon_action(
        &mut self,
        hud: &mut HudState,
        player_entity: u32,
        weapon: WeaponId,
        action: WeaponAction,
        inventory: &Inventory,
    ) {
        let entry = spec(weapon);
        #[allow(clippy::cast_possible_wrap)]
        {
            hud.clip_ammo = entry.clip_size.map(|_| inventory.clip(weapon) as i32);
            hud.reserve_ammo = entry.ammo.map(|kind| inventory.ammo(kind).current() as i32);
        }

        let (view_model_action, cue) = from_weapon_action(action);
        if let Some(view_model_action) = view_model_action {
            self.viewmodel.push(view_model_action);
        }
        if let Some(cue) = cue {
            self.sounds.push(SoundCue {
                entity: player_entity,
                class: ChannelClass::Weapon,
                path: weapon_sound_path(weapon, cue),
            });
        }
    }

    /// Applies one `CombatEvent` concerning `player_entity`: on damage to
    /// the player, syncs `hud`'s health/armor from `health`/`armor` (read
    /// after the event was applied, rather than accumulated by repeated
    /// subtraction here) and triggers the damage flash. Events about any
    /// other entity are ignored, as are `CombatEvent::Killed` and
    /// `CombatEvent::Impact`: this bridge only drives the player's HUD, and
    /// a death/impact effect is presentation work outside the HUD's remit.
    pub fn on_combat_event(
        &mut self,
        hud: &mut HudState,
        player_entity: EntityId,
        event: CombatEvent,
        health: &Health,
        armor: &Armor,
    ) {
        if let CombatEvent::DamageDealt { target, .. } = event
            && target == player_entity
        {
            #[allow(clippy::cast_possible_truncation)]
            {
                hud.health = health.current.round() as i32;
                hud.armor = armor.current.round() as i32;
            }
            hud.trigger_damage_flash();
        }
    }

    /// Applies one pickup outcome: a taken pickup shows its HUD message and
    /// queues its sound cue; an untaken pickup (an already-full pool, an
    /// already-owned flag item) changes nothing.
    pub fn on_pickup(
        &mut self,
        hud: &mut HudState,
        player_entity: u32,
        kind: PickupKind,
        outcome: PickupOutcome,
    ) {
        if !outcome.taken {
            return;
        }
        hud.show_message(pickup_label(kind), PICKUP_MESSAGE_SECONDS);
        self.sounds.push(SoundCue {
            entity: player_entity,
            class: ChannelClass::Item,
            path: pickup_sound_path(kind),
        });
    }
}

/// A short, human-readable label for `kind`'s HUD pickup message. These are
/// this project's own UI copy (weapon and ammo *names*, not gameplay
/// numbers), matching the vocabulary `docs/FORMAT_SOURCES.md` already uses
/// for each weapon.
fn pickup_label(kind: PickupKind) -> &'static str {
    match kind {
        PickupKind::Weapon(id) => weapon_label(id),
        PickupKind::Ammo(ammo_kind) => ammo_label(ammo_kind),
        PickupKind::HealthKit => "Health Kit",
        PickupKind::Battery => "Battery",
        PickupKind::Suit => "HEV Suit",
        PickupKind::LongJump => "Long Jump Module",
        PickupKind::HealthCharger | PickupKind::SuitCharger => "Charger",
        _ => "Pickup",
    }
}

fn weapon_label(id: WeaponId) -> &'static str {
    match id {
        WeaponId::Crowbar => "Crowbar",
        WeaponId::Glock => "9mm Handgun",
        WeaponId::Python => ".357 Magnum",
        WeaponId::Mp5 => "MP5",
        WeaponId::Shotgun => "Shotgun",
        WeaponId::Crossbow => "Crossbow",
        WeaponId::Rpg => "RPG",
        WeaponId::Gauss => "Gauss Gun",
        WeaponId::Egon => "Egon",
        WeaponId::HornetGun => "Hornet Gun",
        WeaponId::HandGrenade => "Hand Grenade",
        WeaponId::Satchel => "Satchel Charge",
        WeaponId::Tripmine => "Tripmine",
        WeaponId::Snark => "Snark",
    }
}

fn ammo_label(kind: ohl_combat::AmmoType) -> &'static str {
    use ohl_combat::AmmoType;
    match kind {
        AmmoType::NineMillimeter => "9mm Ammo",
        AmmoType::ThreeFiveSeven => ".357 Ammo",
        AmmoType::Buckshot => "Buckshot",
        AmmoType::Bolts => "Crossbow Bolts",
        AmmoType::Rockets => "Rockets",
        AmmoType::Uranium => "Uranium Ammo",
        AmmoType::Hornets => "Hornets",
        AmmoType::HandGrenades => "Hand Grenades",
        AmmoType::Satchels => "Satchel Charges",
        AmmoType::Tripmines => "Tripmines",
        AmmoType::Snarks => "Snarks",
        AmmoType::Mp5Grenades => "MP5 Grenades",
    }
}

#[cfg(test)]
mod tests {
    use super::{GameplayBridge, PICKUP_MESSAGE_SECONDS};
    use ohl_combat::{
        AmmoPool, AmmoType, Armor, CombatEvent, DamageType, EntityId, Health, Inventory,
        PickupKind, PickupOutcome, Sequence, WeaponAction, WeaponId,
    };
    use ohl_ui::hud::HudState;

    #[test]
    fn a_hitscan_shot_updates_ammo_queues_fire_and_stays_within_capacity() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Glock);
        inventory.give_ammo(AmmoType::NineMillimeter, 250);
        inventory.set_clip(WeaponId::Glock, 16);

        bridge.on_weapon_action(
            &mut hud,
            1,
            WeaponId::Glock,
            WeaponAction::Hitscan {
                count: 1,
                spread: 0.0,
            },
            &inventory,
        );

        assert_eq!(hud.clip_ammo, Some(16));
        assert_eq!(hud.reserve_ammo, Some(250));
        assert_eq!(bridge.queued_viewmodel_actions().len(), 1);
        assert_eq!(bridge.queued_sounds().len(), 1);
        assert_eq!(bridge.queued_sounds()[0].entity, 1);
    }

    #[test]
    fn drawing_a_weapon_updates_the_viewmodel_but_queues_no_sound() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();
        let inventory = Inventory::new();

        bridge.on_weapon_action(
            &mut hud,
            1,
            WeaponId::Crowbar,
            WeaponAction::PlaySequence(Sequence::Draw),
            &inventory,
        );

        assert_eq!(bridge.queued_viewmodel_actions().len(), 1);
        assert!(bridge.queued_sounds().is_empty());
    }

    #[test]
    fn damage_to_the_player_syncs_the_hud_and_triggers_the_flash() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();
        let player = EntityId(1);
        let health = Health {
            current: 70.0,
            max: 100.0,
        };
        let armor = Armor {
            current: 20.0,
            max: 100.0,
        };

        bridge.on_combat_event(
            &mut hud,
            player,
            CombatEvent::DamageDealt {
                target: player,
                attacker: None,
                health_lost: 30.0,
                armor_lost: 0.0,
                kind: DamageType::BULLET,
            },
            &health,
            &armor,
        );

        assert_eq!(hud.health, 70);
        assert_eq!(hud.armor, 20);
        assert!((hud.damage_flash - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn damage_to_another_entity_does_not_touch_the_players_hud() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();
        let player = EntityId(1);
        let health = Health::new(100.0);
        let armor = Armor::empty(100.0);

        bridge.on_combat_event(
            &mut hud,
            player,
            CombatEvent::DamageDealt {
                target: EntityId(2),
                attacker: None,
                health_lost: 50.0,
                armor_lost: 0.0,
                kind: DamageType::BULLET,
            },
            &health,
            &armor,
        );

        let default = HudState::default();
        assert_eq!(hud.health, default.health);
        assert_eq!(hud.armor, default.armor);
        assert!((hud.damage_flash - default.damage_flash).abs() < f32::EPSILON);
    }

    #[test]
    fn a_taken_pickup_shows_a_message_and_queues_its_sound() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();

        bridge.on_pickup(
            &mut hud,
            1,
            PickupKind::HealthKit,
            PickupOutcome {
                taken: true,
                remaining: 0.0,
            },
        );

        let message = hud.message.expect("a message was shown");
        assert_eq!(message.text, "Health Kit");
        assert!((message.seconds_remaining - PICKUP_MESSAGE_SECONDS).abs() < f32::EPSILON);
        assert_eq!(bridge.queued_sounds().len(), 1);
    }

    #[test]
    fn an_untaken_pickup_changes_nothing() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();

        bridge.on_pickup(
            &mut hud,
            1,
            PickupKind::Ammo(AmmoType::Buckshot),
            PickupOutcome {
                taken: false,
                remaining: 100.0,
            },
        );

        assert!(hud.message.is_none());
        assert!(bridge.queued_sounds().is_empty());
    }

    #[test]
    fn a_fixed_event_sequence_produces_the_expected_hud_state() {
        let mut bridge = GameplayBridge::new();
        let mut hud = HudState::default();
        let mut inventory = Inventory::new();
        let player = EntityId(7);
        let mut health = Health::new(100.0);
        let armor = Armor::empty(100.0);

        // 1. Pick up a shotgun (and its bundled ammo).
        inventory.give_weapon(WeaponId::Shotgun);
        inventory.give_ammo(AmmoType::Buckshot, 8);
        bridge.on_pickup(
            &mut hud,
            u32::try_from(player.0).unwrap(),
            PickupKind::Weapon(WeaponId::Shotgun),
            PickupOutcome {
                taken: true,
                remaining: 0.0,
            },
        );

        // 2. Fire it once, draining the clip by one round.
        inventory.set_clip(WeaponId::Shotgun, 8);
        let mut pool = AmmoPool::new(AmmoType::Buckshot);
        pool.add(inventory.ammo(AmmoType::Buckshot).current());
        inventory.set_clip(WeaponId::Shotgun, 7);
        bridge.on_weapon_action(
            &mut hud,
            u32::try_from(player.0).unwrap(),
            WeaponId::Shotgun,
            WeaponAction::Hitscan {
                count: 6,
                spread: 0.0,
            },
            &inventory,
        );

        // 3. Take a hit.
        health.current -= 15.0;
        bridge.on_combat_event(
            &mut hud,
            player,
            CombatEvent::DamageDealt {
                target: player,
                attacker: None,
                health_lost: 15.0,
                armor_lost: 0.0,
                kind: DamageType::BULLET,
            },
            &health,
            &armor,
        );

        assert_eq!(hud.clip_ammo, Some(7));
        assert_eq!(hud.reserve_ammo, Some(8));
        assert_eq!(hud.health, 85);
        assert_eq!(hud.armor, 0);
        assert!((hud.damage_flash - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            hud.message.as_ref().map(|m| m.text.as_str()),
            Some("Shotgun")
        );
        // One pickup sound, one weapon-fire sound, one Fire viewmodel action.
        assert_eq!(bridge.queued_sounds().len(), 2);
        assert_eq!(bridge.queued_viewmodel_actions().len(), 1);
    }
}
