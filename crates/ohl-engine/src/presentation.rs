//! HUD and audio, through `GameEvent`.
//!
//! [`Presentation`] owns the one [`ohl_gameplay::GameplayBridge`] this
//! engine drives. The bridge itself writes [`ohl_ui::hud::HudState`]
//! directly (health/armour/ammo/damage-flash: state, not a stream, per
//! §5 of the M7.9 design plan, recorded in local design notes and not
//! part of the repository); [`Presentation::tick`] additionally turns
//! `ohl_player::PlayerEvent`s into HUD updates the bridge has no way to see
//! (the player's own health and armour, per `crate::damage_map`'s split),
//! and collects everything a host needs to hear or announce — queued sound
//! cues, queued viewmodel actions, HEV suit occasions and the player's
//! death — as [`PresentationEvent`]s for [`crate::game::Game::tick`] to
//! turn into the four additive `GameEvent` variants.
//!
//! Every *built-in* asset path this package ships — which WAV a weapon
//! fires with, which one a pickup is taken with — is still `None`: no
//! clean-room provenance review has yet admitted one (see
//! `docs/CLEAN_ROOM.md` rule 7 and `ohl_gameplay::sounds`'s own module
//! docs). Cues whose path the *map* supplies are a different matter, and
//! they do carry one: [`Presentation::ambient`] below emits an
//! `ambient_generic`'s own published `message` keyvalue, and
//! `crate::ai`'s `scripted_sentence` handling emits the word samples the
//! payload's own `sentences.txt` names.

use std::collections::BTreeMap;

use ohl_game::hecs::Entity;
use ohl_game::registry::{AmbientGeneric, AmbientRadius, AmbientState};
use ohl_player::PlayerEvent;
use ohl_ui::hud::HudState;

use crate::level::Level;
use crate::text::SentenceLookup;

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

/// The most `ambient_generic` entities one map's soundscape is tracked
/// for. A map declaring more than this keeps the first
/// [`MAX_TRACKED_AMBIENTS`] in spawn order and leaves the rest silent,
/// rather than growing this table without bound. Project-owned, and far
/// above what any published map is recorded as carrying (see
/// `docs/FORMAT_SOURCES.md`'s per-map entity census).
pub(crate) const MAX_TRACKED_AMBIENTS: usize = 256;

/// HUD/audio presentation state, owned by [`crate::systems::Systems`].
pub(crate) struct Presentation {
    pub(crate) bridge: ohl_gameplay::GameplayBridge,
    events: Vec<PresentationEvent>,
    /// What each tracked `ambient_generic` was last reported as: `Some`
    /// generation while it is sounding, `None` while it is silent. Reset
    /// whenever a level is attached, so a fresh map (or a loaded save,
    /// which rebuilds its registry from the map's own entity defaults)
    /// re-announces every ambient it spawns sounding.
    ambients: BTreeMap<Entity, Option<u32>>,
}

impl Presentation {
    pub(crate) fn new() -> Self {
        Self {
            bridge: ohl_gameplay::GameplayBridge::new(),
            events: Vec::new(),
            ambients: BTreeMap::new(),
        }
    }

    /// Forgets every tracked `ambient_generic`, so the next
    /// [`Self::ambient`] re-announces the level's whole soundscape. Called
    /// when a level is attached or the systems are reset.
    pub(crate) fn forget_ambients(&mut self) {
        self.ambients.clear();
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

    /// Phase 13b — turns this step's `ambient_generic` state changes into
    /// sound cues.
    ///
    /// The simulation's shared `use`/`target` path is what flips
    /// `AmbientState::playing` (see `ohl_game::logic::Simulation`); this
    /// only compares that flag against what was last reported and emits
    /// the difference: a start cue when a silent ambient begins sounding
    /// (or when an unlooped one is retriggered, which bumps its
    /// generation), and a stop cue when a sounding one is switched off.
    /// An ambient that is not sounding and never was produces nothing.
    pub(crate) fn ambient(&mut self, level: &Level, sentences: &SentenceLookup) {
        let mut seen: Vec<Entity> = Vec::new();
        let mut query = level
            .registry
            .world
            .query::<(Entity, &AmbientGeneric, &AmbientState)>();
        for (entity, ambient, state) in query.iter().take(MAX_TRACKED_AMBIENTS) {
            seen.push(entity);
            let reported = self.ambients.get(&entity).copied().flatten();
            match (state.playing, reported) {
                (true, Some(generation)) if generation == state.generation => {}
                (true, _) => {
                    let origin = level
                        .registry
                        .world
                        .get::<&ohl_game::registry::Transform>(entity)
                        .map_or([0.0, 0.0, 0.0], |transform| transform.origin.to_array());
                    self.ambients.insert(entity, Some(state.generation));
                    self.events.push(PresentationEvent::Sound(
                        ohl_gameplay::SoundCue::new(
                            entity.id(),
                            // The published `CHAN_STATIC` class: the pool
                            // GoldSrc reserves for a level's own standing
                            // ambience, which is why it is the widest one
                            // (`ohl_audio::ChannelClass::capacity`).
                            ohl_gameplay::ChannelClass::Static,
                            resolve_ambient_asset(&ambient.message, sentences),
                        )
                        .at(origin, attenuation_of(ambient.radius))
                        .with_gain(ambient.volume, ambient.pitch),
                    ));
                }
                (false, Some(_)) => {
                    self.ambients.insert(entity, None);
                    self.events
                        .push(PresentationEvent::Sound(ohl_gameplay::SoundCue::stopping(
                            entity.id(),
                            ohl_gameplay::ChannelClass::Static,
                        )));
                }
                (false, None) => {
                    self.ambients.insert(entity, None);
                }
            }
        }
        drop(query);
        // A map whose ambients were despawned (a level change reusing this
        // `Presentation`) must not keep their entries alive forever.
        if self.ambients.len() > seen.len() {
            self.ambients.retain(|entity, _| seen.contains(entity));
        }
    }

    /// Takes every presentation event collected since the last call.
    pub(crate) fn drain_events(&mut self) -> Vec<PresentationEvent> {
        std::mem::take(&mut self.events)
    }
}

/// The published `ATTN_*` falloff each published radius spawnflag is read
/// as. "Play everywhere" is `ATTN_NONE` by its own wording; of the three
/// radii, the ordering is forced — a larger radius must be the slower
/// falloff — so the three remaining published constants line up in exactly
/// one way. See `docs/FORMAT_SOURCES.md`, "`ambient_generic`".
fn attenuation_of(radius: AmbientRadius) -> f32 {
    match radius {
        AmbientRadius::Everywhere => ohl_gameplay::ATTN_NONE,
        AmbientRadius::Large => ohl_gameplay::ATTN_NORM,
        AmbientRadius::Medium => ohl_gameplay::ATTN_STATIC,
        AmbientRadius::Small => ohl_gameplay::ATTN_IDLE,
    }
}

/// Resolves an `ambient_generic`'s published `message` keyvalue.
///
/// Published: the value is "in the form path/filename.wav starting from the
/// 'sound' folder", and "the name of a sentence defined in sentences.txt
/// will also be accepted with the form `!SENTENCENAME`" (TWHL's
/// `ambient_generic` page; see `docs/FORMAT_SOURCES.md`). An empty
/// `message`, or a sentence name the payload's own `sentences.txt` does not
/// define, resolves to nothing playable.
fn resolve_ambient_asset(message: &str, sentences: &SentenceLookup) -> ohl_gameplay::SoundAsset {
    let message = message.trim();
    if message.is_empty() {
        return ohl_gameplay::SoundAsset::Unresolved;
    }
    if let Some(name) = message.strip_prefix('!') {
        return ohl_gameplay::SoundAsset::sentence(
            sentences.words(name).into_iter().map(|word| word.0),
        );
    }
    // The `sound/` prefix the published wording describes, applied here so
    // no caller has to know it. Mapper-authored back-slashes are the
    // GoldSrc separator and normalise to the one `ohl_assets` indexes on.
    ohl_gameplay::SoundAsset::file(format!("sound/{}", message.replace('\\', "/")))
}
