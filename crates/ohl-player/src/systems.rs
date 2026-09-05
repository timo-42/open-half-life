//! The per-tick player systems: damage intake, drowning, fall damage,
//! flashlight, and the HEV suit's reactions to all of it.

use ohl_physics::LiquidKind;

use crate::damage::{DamageKind, absorb, fall_damage};
use crate::input::{ContentsQuery, PhysicsOutput, PlayerInput};
use crate::state::{PlayerConfig, PlayerState};
use crate::suit::{SuitEvent, SuitOccasion, SuitVoice};

/// Something the player systems did this tick that the rest of the game has
/// to know about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerEvent {
    /// The player took damage. `amount` is the health actually lost,
    /// rounded for display.
    Damaged {
        /// Health lost.
        amount: i32,
        /// What caused it.
        kind: DamageKind,
    },
    /// Health reached zero. Raised exactly once.
    Died,
    /// The HEV suit wants to say something.
    Suit(SuitEvent),
    /// The flashlight was switched on (`true`) or off (`false`).
    FlashlightToggled(bool),
    /// The player's air ran out and drowning damage has started.
    DrowningStarted,
    /// The player got their head back above the surface.
    Surfaced,
    /// A long jump fired, so the host can play the matching effect.
    LongJumped,
}

/// The hook `ohl-engine` drives once per fixed tick.
///
/// It is a trait rather than a bare method so a host can substitute a
/// recording or no-op implementation in tests without depending on this
/// crate's concrete state.
pub trait PlayerSystems {
    /// Advances the player by `dt` seconds and returns everything that
    /// happened, in the order it happened.
    fn tick(
        &mut self,
        dt: f32,
        input: &PlayerInput,
        physics: &PhysicsOutput,
        contents: &dyn ContentsQuery,
    ) -> Vec<PlayerEvent>;
}

/// The largest number of events one tick may report, so a pathological map
/// (a hundred stacked `trigger_hurt` volumes) cannot make one tick allocate
/// without bound.
pub const MAX_EVENTS_PER_TICK: usize = 64;

/// The player: [`PlayerState`] plus the timers the systems need between
/// ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    /// The saved player state.
    pub state: PlayerState,
    /// The tunables in force.
    pub config: PlayerConfig,
    /// HEV voice cooldowns.
    pub voice: SuitVoice,
    /// Seconds until the next hit from a damaging volume.
    hurt_timer: f32,
    /// Seconds until the next drowning hit.
    drown_timer: f32,
    /// Whether the player was drowning last tick, so the event fires once.
    drowning: bool,
    /// Whether the player's head was under last tick.
    was_submerged: bool,
}

impl Default for Player {
    fn default() -> Self {
        Self::new(PlayerConfig::default())
    }
}

impl Player {
    /// A fresh player.
    #[must_use]
    pub fn new(config: PlayerConfig) -> Self {
        Self {
            state: PlayerState::new(&config),
            config,
            voice: SuitVoice::new(),
            hurt_timer: 0.0,
            drown_timer: 0.0,
            drowning: false,
            was_submerged: false,
        }
    }

    /// Gives the player the HEV suit, which is what enables armor, the
    /// flashlight and the suit voice.
    pub fn equip_suit(&mut self, events: &mut Vec<PlayerEvent>) {
        if self.state.suit_equipped {
            return;
        }
        self.state.suit_equipped = true;
        self.raise(SuitOccasion::SuitEquipped, events);
    }

    /// Installs the long jump module.
    pub fn give_long_jump(&mut self, events: &mut Vec<PlayerEvent>) {
        if self.state.longjump_owned {
            return;
        }
        self.state.longjump_owned = true;
        self.raise(SuitOccasion::LongJumpActivated, events);
    }

    /// Adds armor, up to [`PlayerConfig::max_armor`]. Does nothing without
    /// the suit, which is the documented behaviour of an HEV charger.
    pub fn add_armor(&mut self, amount: f32, events: &mut Vec<PlayerEvent>) {
        if !self.state.suit_equipped || !amount.is_finite() || amount <= 0.0 {
            return;
        }
        let was_empty = self.state.armor <= 0.0;
        self.state.armor = (self.state.armor + amount).clamp(0.0, self.config.max_armor);
        if was_empty && self.state.armor > 0.0 {
            self.raise(SuitOccasion::ArmorRestored, events);
        }
    }

    /// Heals up to [`PlayerConfig::max_health`]. A dead player cannot be
    /// healed.
    pub fn heal(&mut self, amount: f32, events: &mut Vec<PlayerEvent>) {
        if self.state.dead || !amount.is_finite() || amount <= 0.0 {
            return;
        }
        self.state.health = (self.state.health + amount).clamp(0.0, self.config.max_health);
        self.raise(SuitOccasion::MedkitPickup, events);
    }

    /// Applies one hit and records the events it produced.
    ///
    /// The armor split is [`crate::damage::absorb`]; the health result is
    /// always clamped into `0..=max_health`, so no caller can drive health
    /// out of range.
    pub fn apply_damage(&mut self, amount: f32, kind: DamageKind, events: &mut Vec<PlayerEvent>) {
        if self.state.dead || !amount.is_finite() || amount <= 0.0 {
            return;
        }
        let armor = if self.state.suit_equipped {
            self.state.armor
        } else {
            0.0
        };
        let absorbed = absorb(amount, armor, kind);
        let armor_was_positive = armor > 0.0;
        self.state.armor = absorbed.armor_left.clamp(0.0, self.config.max_armor);
        let before = self.state.health;
        self.state.health = (before - absorbed.health_loss).clamp(0.0, self.config.max_health);
        let lost = before - self.state.health;
        self.state.damage_flags.insert(kind);
        push(
            events,
            PlayerEvent::Damaged {
                amount: display_amount(lost),
                kind,
            },
        );

        if armor_was_positive && self.state.armor <= 0.0 {
            self.raise(SuitOccasion::ArmorGone, events);
        }
        if let Some(occasion) = kind.suit_occasion() {
            self.raise(occasion, events);
        }
        self.check_health_warnings(events);
    }

    /// Turns the flashlight on or off, if the player has the suit and any
    /// charge left.
    pub fn toggle_flashlight(&mut self, events: &mut Vec<PlayerEvent>) {
        if !self.state.suit_equipped {
            return;
        }
        if !self.state.flashlight.on && self.state.flashlight.charge <= 0.0 {
            return;
        }
        self.state.flashlight.on = !self.state.flashlight.on;
        push(
            events,
            PlayerEvent::FlashlightToggled(self.state.flashlight.on),
        );
    }

    fn raise(&mut self, occasion: SuitOccasion, events: &mut Vec<PlayerEvent>) {
        if !self.state.suit_equipped {
            return;
        }
        if let Some(event) = self.voice.raise(occasion) {
            push(events, PlayerEvent::Suit(event));
        }
    }

    fn check_health_warnings(&mut self, events: &mut Vec<PlayerEvent>) {
        if self.state.health <= 0.0 {
            if !self.state.dead {
                self.state.dead = true;
                push(events, PlayerEvent::Died);
            }
            return;
        }
        let fraction = self.state.health / self.config.max_health.max(1.0);
        if fraction <= self.config.near_death_fraction {
            self.raise(SuitOccasion::NearDeath, events);
        } else if fraction <= self.config.health_critical_fraction {
            self.raise(SuitOccasion::HealthCritical, events);
        }
    }

    fn tick_flashlight(&mut self, dt: f32, input: &PlayerInput, events: &mut Vec<PlayerEvent>) {
        if input.flashlight_pressed {
            self.toggle_flashlight(events);
        }
        let flashlight = &mut self.state.flashlight;
        if flashlight.on {
            flashlight.charge =
                (flashlight.charge - self.config.flashlight_drain_per_second * dt).clamp(0.0, 1.0);
            if flashlight.charge <= 0.0 {
                flashlight.on = false;
                push(events, PlayerEvent::FlashlightToggled(false));
            }
        } else {
            flashlight.charge = (flashlight.charge
                + self.config.flashlight_recharge_per_second * dt)
                .clamp(0.0, 1.0);
        }
    }

    fn tick_air(&mut self, dt: f32, physics: &PhysicsOutput, events: &mut Vec<PlayerEvent>) {
        let submerged = physics.is_submerged();
        if submerged {
            self.state.air_time = (self.state.air_time - dt).max(0.0);
            if self.state.air_time <= 0.0 {
                if !self.drowning {
                    self.drowning = true;
                    self.drown_timer = 0.0;
                    push(events, PlayerEvent::DrowningStarted);
                }
                self.drown_timer -= dt;
                if self.drown_timer <= 0.0 {
                    self.drown_timer = self.config.drown_interval_seconds;
                    self.apply_damage(self.config.drown_damage, DamageKind::Drown, events);
                }
            }
        } else {
            if self.was_submerged {
                push(events, PlayerEvent::Surfaced);
            }
            self.drowning = false;
            self.drown_timer = 0.0;
            self.state.air_time = (self.state.air_time + dt * self.config.air_recovery_rate)
                .clamp(0.0, self.config.air_capacity_seconds);
        }
        self.was_submerged = submerged;
    }

    /// Damage from standing in a damaging volume or a hostile liquid, on
    /// the documented half-second `trigger_hurt` cadence.
    fn tick_contact_damage(
        &mut self,
        dt: f32,
        input: &PlayerInput,
        physics: &PhysicsOutput,
        events: &mut Vec<PlayerEvent>,
    ) {
        self.hurt_timer -= dt;
        if self.hurt_timer > 0.0 {
            return;
        }
        self.hurt_timer = self.config.hurt_interval_seconds;
        let interval = self.config.hurt_interval_seconds;

        for hurt in input.hurt.iter().take(crate::input::MAX_HURT_VOLUMES) {
            // Published: one hit every half second worth half of `dmg`. A
            // negative `dmg` heals, the documented way healing pools are
            // built.
            let amount = hurt.damage_per_second * interval;
            if amount > 0.0 {
                self.apply_damage(amount, hurt.kind(), events);
            } else if amount < 0.0 {
                self.heal(-amount, events);
            }
        }

        if physics.water_level != ohl_physics::WaterLevel::Dry {
            let (rate, kind) = match physics.liquid {
                LiquidKind::Slime => (self.config.slime_damage_per_second, DamageKind::Acid),
                LiquidKind::Lava => (self.config.lava_damage_per_second, DamageKind::Burn),
                LiquidKind::Water | LiquidKind::None => (0.0, DamageKind::Generic),
            };
            if rate > 0.0 {
                self.apply_damage(rate * interval, kind, events);
            }
        }
    }
}

impl PlayerSystems for Player {
    fn tick(
        &mut self,
        dt: f32,
        input: &PlayerInput,
        physics: &PhysicsOutput,
        contents: &dyn ContentsQuery,
    ) -> Vec<PlayerEvent> {
        let mut events = Vec::new();
        if !dt.is_finite() || dt <= 0.0 {
            return events;
        }
        self.voice.tick(dt);
        self.state.waterlevel = physics.water_level.as_index();

        self.tick_flashlight(dt, input, &mut events);
        self.tick_air(dt, physics, &mut events);
        self.tick_contact_damage(dt, input, physics, &mut events);

        if let Some(speed) = physics.landed_speed {
            let damage = fall_damage(speed);
            if damage > 0.0 {
                self.apply_damage(damage, DamageKind::Fall, &mut events);
            }
        }
        if physics.long_jumped {
            push(&mut events, PlayerEvent::LongJumped);
        }

        // A hazard the player is standing in but that does no damage yet
        // (a radioactive room marked with a `trigger_hurt` the player has
        // not entered, say) is still worth a suit warning, which is what
        // the contents query is for.
        if ohl_physics::LiquidKind::from_contents(contents.point_contents(physics.eye))
            == LiquidKind::Slime
        {
            self.raise(SuitOccasion::ChemicalDetected, &mut events);
        }

        events.truncate(MAX_EVENTS_PER_TICK);
        events
    }
}

/// Rounds a damage amount for reporting. The value is clamped into
/// `0..=32767` before the conversion, so it is exact.
#[allow(clippy::cast_possible_truncation)]
fn display_amount(lost: f32) -> i32 {
    if !lost.is_finite() || lost <= 0.0 {
        return 0;
    }
    lost.ceil().min(f32::from(i16::MAX)) as i32
}

fn push(events: &mut Vec<PlayerEvent>, event: PlayerEvent) {
    if events.len() < MAX_EVENTS_PER_TICK {
        events.push(event);
    }
}
