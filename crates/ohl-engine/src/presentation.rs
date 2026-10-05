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
//! Built-in weapon/pickup cues use a bounded reviewed lookup. Three reviewed
//! HEV sentence identifiers use the runtime sentence table instead (see
//! `docs/FORMAT_SOURCES.md`, "Bounded HEV damage sentence audio").
//! Cues whose path the *map* supplies are a different matter, and
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

    /// Adds sounds from other combatants without forwarding their HUD or
    /// viewmodel actions to the human player.
    pub(crate) fn add_sounds(&mut self, sounds: impl IntoIterator<Item = ohl_gameplay::SoundCue>) {
        self.events
            .extend(sounds.into_iter().map(PresentationEvent::Sound));
    }

    /// Forgets every tracked `ambient_generic`, so the next
    /// [`Self::ambient`] re-announces the level's whole soundscape. Called
    /// when a level is attached or the systems are reset.
    pub(crate) fn forget_ambients(&mut self) {
        self.ambients.clear();
    }

    /// Adds one sentence cue per mapped, already-emitted suit event.
    /// The producer owns eligibility and cooldown; the normal tick still
    /// forwards its complete metadata. No condition is reconstructed here.
    ///
    /// Project-authored: immediate listener-relative Voice playback at normal
    /// gain/pitch. TODO(black-box): original delay/priority/channel policy;
    /// multiple same-owner cues replace playback rather than queue speech.
    pub(crate) fn suit_audio(
        &mut self,
        player_tag: u32,
        sentences: &SentenceLookup,
        player_events: &[PlayerEvent],
    ) {
        for event in player_events {
            let PlayerEvent::Suit(suit) = event else {
                continue;
            };
            let Some(name) = suit_sentence(suit.occasion) else {
                continue;
            };
            let asset = ohl_gameplay::SoundAsset::sentence(
                sentences.words(name).into_iter().map(|word| word.0),
            );
            self.events
                .push(PresentationEvent::Sound(ohl_gameplay::SoundCue::new(
                    player_tag,
                    ohl_gameplay::ChannelClass::Voice,
                    asset,
                )));
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

/// Exact identifiers and broad associations from the reviewed public HEV
/// Quotes source; no WAV paths, word lists or trigger thresholds are implied.
fn suit_sentence(occasion: ohl_player::SuitOccasion) -> Option<&'static str> {
    use ohl_player::SuitOccasion;
    match occasion {
        SuitOccasion::HeatDamage => Some("HEV_FIRE"),
        SuitOccasion::ShockDamage => Some("HEV_SHOCK"),
        SuitOccasion::MinorFracture => Some("HEV_DMG4"),
        _ => None,
    }
}

/// The published `ATTN_*` falloff each published radius spawnflag (and each
/// `scripted_sentence` "Sound Radius" choice, which publishes the same four
/// radii) is read as. "Play everywhere" is `ATTN_NONE` by its own wording; of the three
/// radii, the ordering is forced — a larger radius must be the slower
/// falloff — so the three remaining published constants line up in exactly
/// one way. See `docs/FORMAT_SOURCES.md`, "`ambient_generic`".
pub(crate) fn attenuation_of(radius: AmbientRadius) -> f32 {
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

#[cfg(test)]
mod hev_audio_tests {
    use super::{Presentation, PresentationEvent, SentenceLookup};
    use ohl_gameplay::{ChannelClass, SoundAsset};
    use ohl_player::{DamageKind, Player, PlayerEvent, SuitEvent, SuitOccasion};

    const PLAYER_TAG: u32 = 71;
    const SENTENCES: &[u8] = b"HEV_FIRE ohl/heat_first ohl/heat_second\n\
        HEV_SHOCK ohl/shock_first ohl/shock_second\n\
        HEV_DMG4 ohl/fall_first ohl/fall_second\n";

    fn suited_player() -> Player {
        let mut player = Player::default();
        player.equip_suit(&mut Vec::new());
        player
    }

    fn present(
        presentation: &mut Presentation,
        player: &Player,
        events: Vec<PlayerEvent>,
        sentences: &SentenceLookup,
    ) -> Vec<PresentationEvent> {
        presentation.suit_audio(PLAYER_TAG, sentences, &events);
        presentation.tick(
            crate::TICK_SECONDS,
            &mut ohl_ui::hud::HudState::default(),
            player,
            events,
            &mut crate::viewmodel::ViewModel::new(),
        );
        presentation.drain_events()
    }

    fn suits(events: &[PlayerEvent]) -> Vec<SuitEvent> {
        events
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Suit(suit) => Some(*suit),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn hev_audio_three_damage_producers_preserve_metadata_and_ordered_words() {
        let mut player = suited_player();
        let mut events = Vec::new();
        for kind in [DamageKind::Burn, DamageKind::Shock, DamageKind::Fall] {
            player.apply_damage(1.0, kind, &mut events);
        }
        let expected_suits = suits(&events);
        assert_eq!(expected_suits.len(), 3);
        assert!(expected_suits.iter().any(|suit| suit.delay > 0.0));
        let mut presentation = Presentation::new();
        let output = present(
            &mut presentation,
            &player,
            events,
            &SentenceLookup::from_bytes(SENTENCES),
        );
        let mut actual_suits = Vec::new();
        let mut assets = Vec::new();
        for event in output {
            match event {
                PresentationEvent::Sound(cue) => {
                    assert_eq!(cue.entity, PLAYER_TAG);
                    assert_eq!(cue.class, ChannelClass::Voice);
                    assert_eq!(cue.origin, None);
                    assert!((cue.volume - 1.0).abs() < f32::EPSILON);
                    assert!((cue.pitch - 1.0).abs() < f32::EPSILON);
                    assert!(!cue.stop);
                    assets.push(cue.asset);
                }
                PresentationEvent::Suit(suit) => actual_suits.push(suit),
                _ => panic!("only sound and suit events expected"),
            }
        }
        assert_eq!(actual_suits, expected_suits);
        let expected_assets: Vec<_> = ["heat", "shock", "fall"]
            .into_iter()
            .map(|label| {
                SoundAsset::sentence(vec![
                    format!("sound/ohl/{label}_first.wav"),
                    format!("sound/ohl/{label}_second.wav"),
                ])
            })
            .collect();
        assert_eq!(assets, expected_assets);
        assert!(presentation.drain_events().is_empty());
        assert!(
            present(
                &mut presentation,
                &player,
                Vec::new(),
                &SentenceLookup::new()
            )
            .is_empty()
        );
    }

    #[test]
    fn hev_audio_existing_cooldown_suppresses_repeat_but_not_later_event() {
        let mut player = suited_player();
        let mut presentation = Presentation::new();
        let sentences = SentenceLookup::from_bytes(SENTENCES);
        for (elapsed, expected) in [(0.0, 1), (0.1, 0), (10.0, 1)] {
            player.voice.tick(elapsed);
            let mut events = Vec::new();
            player.apply_damage(1.0, DamageKind::Burn, &mut events);
            assert_eq!(suits(&events).len(), expected);
            let output = present(&mut presentation, &player, events, &sentences);
            assert_eq!(
                output
                    .iter()
                    .filter(|event| matches!(event, PresentationEvent::Sound(_)))
                    .count(),
                expected
            );
        }
    }

    #[test]
    fn hev_audio_no_suit_invalid_and_generic_damage_do_not_create_cues() {
        for (suited, kind, amount) in [
            (false, DamageKind::Burn, 1.0),
            (true, DamageKind::Generic, 1.0),
            (true, DamageKind::Burn, 0.0),
            (true, DamageKind::Shock, f32::NAN),
        ] {
            let mut player = if suited {
                suited_player()
            } else {
                Player::default()
            };
            let mut events = Vec::new();
            player.apply_damage(amount, kind, &mut events);
            let output = present(
                &mut Presentation::new(),
                &player,
                events,
                &SentenceLookup::from_bytes(SENTENCES),
            );
            assert!(
                !output
                    .iter()
                    .any(|event| matches!(event, PresentationEvent::Sound(_)))
            );
        }
    }

    #[test]
    fn hev_audio_unmapped_occasion_ignores_a_mapped_display_name() {
        let suit = SuitEvent {
            occasion: SuitOccasion::AmmoPickup,
            name: "HEV_FIRE",
            priority: 9,
            delay: 3.0,
        };
        let output = present(
            &mut Presentation::new(),
            &suited_player(),
            vec![PlayerEvent::Suit(suit)],
            &SentenceLookup::from_bytes(SENTENCES),
        );
        assert_eq!(output.len(), 1);
        assert!(matches!(output[0], PresentationEvent::Suit(actual) if actual == suit));
    }

    #[test]
    fn hev_audio_existing_256_word_limit_rejects_257_without_losing_metadata() {
        for words in [0, 256, 257] {
            let text = if words == 0 {
                String::new()
            } else {
                format!(
                    "HEV_FIRE {}\n",
                    (0..words)
                        .map(|index| format!("ohl/word_{index}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };
            let mut player = suited_player();
            let mut input = Vec::new();
            player.apply_damage(1.0, DamageKind::Burn, &mut input);
            let expected = suits(&input);
            let output = present(
                &mut Presentation::new(),
                &player,
                input,
                &SentenceLookup::from_bytes(text.as_bytes()),
            );
            let mut metadata = Vec::new();
            let mut assets = Vec::new();
            for event in output {
                match event {
                    PresentationEvent::Sound(cue) => assets.push(cue.asset),
                    PresentationEvent::Suit(suit) => metadata.push(suit),
                    _ => panic!("only sound and suit events expected"),
                }
            }
            assert_eq!(metadata, expected);
            assert_eq!(assets.len(), 1);
            if words == 256 {
                assert_eq!(
                    assets[0],
                    SoundAsset::sentence(
                        (0..words).map(|index| format!("sound/ohl/word_{index}.wav"))
                    )
                );
            } else {
                assert_eq!(assets[0], SoundAsset::Unresolved);
            }
        }
    }
}
