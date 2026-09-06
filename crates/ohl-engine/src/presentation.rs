//! HUD and audio, through `GameEvent`.
//!
//! [`Presentation`] owns the one [`ohl_gameplay::GameplayBridge`] this
//! engine drives. The bridge itself writes [`ohl_ui::hud::HudState`]
//! directly (health/armour/ammo/damage-flash: state, not a stream, per
//! `.plan/m79-design.md` §5); [`Presentation::tick`] additionally turns
//! `ohl_player::PlayerEvent`s into HUD updates the bridge has no way to see
//! (the player's own health and armour, per `crate::damage_map`'s split),
//! and collects everything a host needs to hear or announce — queued sound
//! cues, queued viewmodel actions, HEV suit occasions and the player's
//! death — as [`PresentationEvent`]s for [`crate::game::Game::tick`] to
//! turn into the four additive `GameEvent` variants.
//!
//! Every [`ohl_gameplay::SoundCue::path`] this package ships is `None`: no
//! clean-room provenance review has yet admitted a sound asset path (see
//! `docs/CLEAN_ROOM.md` rule 7 and `ohl_gameplay::sounds`'s own module
//! docs). The plumbing here is complete; the path table stays empty.

use ohl_player::PlayerEvent;
use ohl_ui::hud::HudState;

/// How fast the HUD's damage flash decays back to zero. A project-defined
/// UI choice (matching `ohl_gameplay::bridge::PICKUP_MESSAGE_SECONDS`'s own
/// framing), not gameplay data.
pub const DAMAGE_FLASH_DECAY_PER_SECOND: f32 = 2.0;

/// One thing the presentation phase collected this step, for
/// [`crate::game::Game::tick`] to map onto a `GameEvent`.
pub(crate) enum PresentationEvent {
    /// A cue the host should play.
    Sound(ohl_gameplay::SoundCue),
    /// An HEV suit voice occasion.
    Suit(ohl_player::SuitEvent),
    /// A viewmodel animation to play next.
    ViewModel(ohl_gameplay::ViewModelAction),
    /// The player's health reached zero this step.
    PlayerDied,
}

/// HUD/audio presentation state, owned by [`crate::systems::Systems`].
pub(crate) struct Presentation {
    pub(crate) bridge: ohl_gameplay::GameplayBridge,
    events: Vec<PresentationEvent>,
}

impl Presentation {
    pub(crate) fn new() -> Self {
        Self {
            bridge: ohl_gameplay::GameplayBridge::new(),
            events: Vec::new(),
        }
    }

    /// Phase 13 — syncs the HUD from this step's player events, decays the
    /// damage flash, and drains the bridge's queued sounds and viewmodel
    /// actions into this step's presentation events (also forwarding each
    /// viewmodel action into `view_model`, so the actual rendered view
    /// model advances, not just the host-visible `GameEvent` stream).
    pub(crate) fn tick(
        &mut self,
        dt: f32,
        hud: &mut HudState,
        player: &ohl_player::Player,
        player_events: Vec<PlayerEvent>,
        view_model: &mut crate::viewmodel::ViewModel,
    ) {
        for event in player_events {
            match event {
                PlayerEvent::Damaged { .. } => {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        hud.health = player.state.health.round() as i32;
                        hud.armor = player.state.armor.round() as i32;
                    }
                    hud.trigger_damage_flash();
                }
                PlayerEvent::Died => self.events.push(PresentationEvent::PlayerDied),
                PlayerEvent::Suit(suit_event) => {
                    self.events.push(PresentationEvent::Suit(suit_event));
                }
                PlayerEvent::FlashlightToggled(_)
                | PlayerEvent::DrowningStarted
                | PlayerEvent::Surfaced
                | PlayerEvent::LongJumped => {}
            }
        }
        hud.decay_damage_flash(DAMAGE_FLASH_DECAY_PER_SECOND, dt);

        for cue in self.bridge.drain_sounds().collect::<Vec<_>>() {
            self.events.push(PresentationEvent::Sound(cue));
        }
        for action in self.bridge.drain_viewmodel_actions().collect::<Vec<_>>() {
            view_model.queue_action(action);
            self.events.push(PresentationEvent::ViewModel(action));
        }
    }

    /// Takes every presentation event collected since the last call.
    pub(crate) fn drain_events(&mut self) -> Vec<PresentationEvent> {
        std::mem::take(&mut self.events)
    }
}
