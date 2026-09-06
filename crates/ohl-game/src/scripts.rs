//! `scripted_sequence`, `aiscripted_sequence` and `scripted_sentence`: the
//! entity half.
//!
//! This module owns the *keyvalue* side of the scripting entities — which
//! keys exist, what their published values mean, and which spawnflag bit is
//! which — plus the one component that lets the existing map-logic
//! [`crate::logic::Simulation`] activation path reach a script. The state
//! machine that runs a script lives in `ohl_ai::scripts`, and the entity
//! bookkeeping (picking a target monster, moving it, playing its sequences,
//! firing `target`) lives in `ohl-engine`'s `ai` module.
//!
//! # Clean room
//!
//! Every key name, choice value and spawnflag bit below is quoted from the
//! public Valve Developer Community and TWHL wiki pages recorded in
//! `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk monsters". No SDK
//! or engine source was consulted; see `docs/CLEAN_ROOM.md`. Behaviour the
//! wikis do not define is marked `TODO(black-box)` where it is decided, not
//! guessed from unofficial sources.

use glam::Vec3;

use crate::keyvalues::EntityDef;

/// The `scripted_sequence` classname.
pub const SCRIPTED_SEQUENCE_CLASSNAME: &str = "scripted_sequence";

/// The `aiscripted_sequence` classname.
pub const AISCRIPTED_SEQUENCE_CLASSNAME: &str = "aiscripted_sequence";

/// The `scripted_sentence` classname.
pub const SCRIPTED_SENTENCE_CLASSNAME: &str = "scripted_sentence";

/// The published `Repeatable` spawnflag bit: the sequence may run more than
/// once instead of the entity being removed when it completes.
pub const SPAWNFLAG_SCRIPT_REPEATABLE: u32 = 4;

/// The published `Leave Corpse` spawnflag bit.
pub const SPAWNFLAG_SCRIPT_LEAVE_CORPSE: u32 = 8;

/// The published `No Interruptions` spawnflag bit: the monster ignores
/// damage until the sequence completes.
pub const SPAWNFLAG_SCRIPT_NO_INTERRUPTIONS: u32 = 32;

/// The published `Override AI` spawnflag bit: the script possesses its
/// target even when that monster is already in combat.
pub const SPAWNFLAG_SCRIPT_OVERRIDE_AI: u32 = 64;

/// The published `No Script Movement` spawnflag bit: when the sequence
/// completes the monster is put back where the action animation started.
pub const SPAWNFLAG_SCRIPT_NO_SCRIPT_MOVEMENT: u32 = 128;

/// The published `Fire Once` spawnflag bit of `scripted_sentence`.
pub const SPAWNFLAG_SENTENCE_FIRE_ONCE: u32 = 1;

/// The published `Followers Only` spawnflag bit of `scripted_sentence`.
pub const SPAWNFLAG_SENTENCE_FOLLOWERS_ONLY: u32 = 2;

/// The published `Interrupt Speech` spawnflag bit of `scripted_sentence`.
pub const SPAWNFLAG_SENTENCE_INTERRUPT_SPEECH: u32 = 4;

/// The published `Concurrent` spawnflag bit of `scripted_sentence`.
pub const SPAWNFLAG_SENTENCE_CONCURRENT: u32 = 8;

/// How the target monster gets to the script's mark before the action
/// animation plays: the published `m_fMoveTo` choices.
///
/// The published GoldSrc set is `0`, `1`, `2`, `4` and `5`; `3` is a
/// Source-engine-only "Custom movement" value and is deliberately not
/// accepted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MoveTo {
    /// `0` — "No": the monster neither moves nor turns.
    #[default]
    No,
    /// `1` — "Walk": the monster walks to the script, then animates.
    Walk,
    /// `2` — "Run": the monster runs to the script, then animates.
    Run,
    /// `4` — "Instantaneous": the monster warps to the script's location.
    Instantaneous,
    /// `5` — "No - Turn to Face": the monster does not move, but turns to
    /// the script's own facing before animating.
    TurnToFace,
}

impl MoveTo {
    /// The published choice `raw` names, or `None` for a value outside the
    /// documented GoldSrc set.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Option<Self> {
        Some(match raw {
            0 => Self::No,
            1 => Self::Walk,
            2 => Self::Run,
            4 => Self::Instantaneous,
            5 => Self::TurnToFace,
            _ => return None,
        })
    }

    /// The published choice number.
    #[must_use]
    pub const fn to_raw(self) -> i32 {
        match self {
            Self::No => 0,
            Self::Walk => 1,
            Self::Run => 2,
            Self::Instantaneous => 4,
            Self::TurnToFace => 5,
        }
    }

    /// Whether this mode walks or runs a route to the mark, as opposed to
    /// warping, turning in place or doing nothing.
    #[must_use]
    pub const fn travels(self) -> bool {
        matches!(self, Self::Walk | Self::Run)
    }
}

/// One `scripted_sequence`/`aiscripted_sequence`'s published keyvalues.
///
/// Pure map data: parsing never fails and never logs. A key the map omits
/// takes this project's own conservative default (empty name, zero radius,
/// no repeat, [`MoveTo::No`]), because no public page states a GoldSrc
/// default for any of them — `TODO(black-box)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptDef {
    /// `m_iszEntity`, "Target Monster": a `targetname` or a classname.
    pub target_monster: String,
    /// `m_iszPlay`, "Action Animation": the sequence *name* looked up in
    /// the target monster's own model.
    pub play: String,
    /// `m_iszIdle`, "Idle Animation": looped until the script is triggered.
    pub idle: String,
    /// `m_flRadius`, "Search Radius", in world units; only consulted when
    /// [`Self::target_monster`] is a classname.
    pub radius: f32,
    /// `m_flRepeat`, "Repeat Rate".
    ///
    /// **`TODO(black-box)`**: the published label says milliseconds while
    /// the published prose describes it as how often the search radius is
    /// re-checked, and no page states the unit. This project reads it as
    /// seconds of delay before a repeatable script plays again, which is
    /// the reading the rest of this entity's timing keys (`delay`) use.
    pub repeat: f32,
    /// `m_fMoveTo`, "Move to Position".
    pub move_to: MoveTo,
    /// `target`: fired when the sequence completes.
    pub target: String,
    /// `delay`, "Delay before trigger", in seconds.
    pub delay: f32,
    /// `killtarget`: removed when the sequence completes.
    pub kill_target: String,
    /// The raw `spawnflags` bits.
    pub spawnflags: u32,
    /// The script's own `origin`: the mark the monster moves to.
    pub origin: Vec3,
    /// The script's own yaw, which "Turn to Face" and "Instantaneous"
    /// adopt.
    pub yaw: f32,
    /// Whether this was an `aiscripted_sequence` rather than a
    /// `scripted_sequence`.
    pub ai_script: bool,
}

/// `key` as a finite `f32`, or `default`.
fn number(def: &EntityDef, key: &str, default: f32) -> f32 {
    def.keyvalues
        .get(key)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

/// The published `attenuation` choice, clamped to the documented `0`-`3`
/// range.
fn attenuation(def: &EntityDef) -> u8 {
    def.keyvalues
        .get("attenuation")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0)
        .clamp(0, 3)
        .try_into()
        .unwrap_or(0)
}

/// `key`'s trimmed text, or `""`.
fn text(def: &EntityDef, key: &str) -> String {
    def.keyvalues
        .get(key)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

impl ScriptDef {
    /// Reads one `scripted_sequence`/`aiscripted_sequence` definition, or
    /// `None` for any other classname.
    #[must_use]
    pub fn from_def(def: &EntityDef) -> Option<Self> {
        let ai_script = match def.classname.as_str() {
            SCRIPTED_SEQUENCE_CLASSNAME => false,
            AISCRIPTED_SEQUENCE_CLASSNAME => true,
            _ => return None,
        };
        Some(Self {
            target_monster: text(def, "m_iszEntity"),
            play: text(def, "m_iszPlay"),
            idle: text(def, "m_iszIdle"),
            radius: number(def, "m_flRadius", 0.0).max(0.0),
            repeat: number(def, "m_flRepeat", 0.0).max(0.0),
            move_to: def
                .keyvalues
                .get("m_fMoveTo")
                .and_then(|value| value.trim().parse::<i32>().ok())
                .and_then(MoveTo::from_raw)
                .unwrap_or_default(),
            target: def.target.clone().unwrap_or_default(),
            delay: number(def, "delay", 0.0).max(0.0),
            kill_target: text(def, "killtarget"),
            spawnflags: def.spawnflags,
            origin: Vec3::from_array(def.origin),
            yaw: def.angles[1],
            ai_script,
        })
    }

    /// The published `Repeatable` flag: the script may run more than once.
    #[must_use]
    pub const fn repeatable(&self) -> bool {
        self.spawnflags & SPAWNFLAG_SCRIPT_REPEATABLE != 0
    }

    /// The published `Leave Corpse` flag.
    #[must_use]
    pub const fn leaves_corpse(&self) -> bool {
        self.spawnflags & SPAWNFLAG_SCRIPT_LEAVE_CORPSE != 0
    }

    /// The published `No Interruptions` flag. An `aiscripted_sequence`
    /// behaves as if it were always set, which is the documented difference
    /// between the two entities.
    #[must_use]
    pub const fn no_interruptions(&self) -> bool {
        self.ai_script || self.spawnflags & SPAWNFLAG_SCRIPT_NO_INTERRUPTIONS != 0
    }

    /// The published `Override AI` flag. As with [`Self::no_interruptions`],
    /// an `aiscripted_sequence` always overrides.
    #[must_use]
    pub const fn overrides_ai(&self) -> bool {
        self.ai_script || self.spawnflags & SPAWNFLAG_SCRIPT_OVERRIDE_AI != 0
    }

    /// The published `No Script Movement` flag.
    #[must_use]
    pub const fn no_script_movement(&self) -> bool {
        self.spawnflags & SPAWNFLAG_SCRIPT_NO_SCRIPT_MOVEMENT != 0
    }

    /// The idle animation name, when this entity has a working one.
    ///
    /// `m_iszIdle` is documented as non-functional on `aiscripted_sequence`,
    /// so it is reported as absent there rather than silently played.
    #[must_use]
    pub fn idle_sequence(&self) -> Option<&str> {
        if self.ai_script || self.idle.is_empty() {
            None
        } else {
            Some(self.idle.as_str())
        }
    }

    /// The action animation name, when the map named one.
    #[must_use]
    pub fn play_sequence(&self) -> Option<&str> {
        (!self.play.is_empty()).then_some(self.play.as_str())
    }
}

/// One `scripted_sentence`'s published keyvalues.
#[derive(Debug, Clone, PartialEq)]
pub struct SentenceDef {
    /// `sentence`, "Sentence Name": a `sentences.txt` name or group.
    pub sentence: String,
    /// `entity`, "Speaker Type": a `targetname` or a classname.
    pub speaker: String,
    /// `listener`, "Listener Type": who the speaker looks at.
    pub listener: String,
    /// `radius`, "Search Radius", in world units.
    pub radius: f32,
    /// `duration`, "Sentence Time", in seconds.
    pub duration: f32,
    /// `refire`, "Delay Before Refire", in seconds.
    pub refire: f32,
    /// `delay`, "Delay before trigger", in seconds.
    pub delay: f32,
    /// `volume`, published range `0`–`10`.
    pub volume: f32,
    /// `attenuation`, "Sound Radius": `0` small, `1` medium, `2` large,
    /// `3` play everywhere.
    pub attenuation: u8,
    /// `target`: fired after the sentence starts, `delay` seconds later.
    pub target: String,
    /// The raw `spawnflags` bits.
    pub spawnflags: u32,
}

impl SentenceDef {
    /// Reads one `scripted_sentence` definition, or `None` for any other
    /// classname.
    #[must_use]
    pub fn from_def(def: &EntityDef) -> Option<Self> {
        if def.classname != SCRIPTED_SENTENCE_CLASSNAME {
            return None;
        }
        Some(Self {
            sentence: text(def, "sentence"),
            speaker: text(def, "entity"),
            listener: text(def, "listener"),
            radius: number(def, "radius", 0.0).max(0.0),
            duration: number(def, "duration", 0.0).max(0.0),
            refire: number(def, "refire", 0.0).max(0.0),
            delay: number(def, "delay", 0.0).max(0.0),
            volume: number(def, "volume", 0.0).clamp(0.0, 10.0),
            attenuation: attenuation(def),
            target: def.target.clone().unwrap_or_default(),
            spawnflags: def.spawnflags,
        })
    }

    /// The published `Fire Once` flag.
    #[must_use]
    pub const fn fire_once(&self) -> bool {
        self.spawnflags & SPAWNFLAG_SENTENCE_FIRE_ONCE != 0
    }

    /// The published `Followers Only` flag: the sentence only plays while
    /// the speaker is following the player.
    #[must_use]
    pub const fn followers_only(&self) -> bool {
        self.spawnflags & SPAWNFLAG_SENTENCE_FOLLOWERS_ONLY != 0
    }
}

/// How many times the map logic has activated a scripting entity that has
/// not been consumed yet.
///
/// This is what puts `scripted_sequence` and `scripted_sentence` on the
/// existing `target`-firing path: [`crate::logic::Simulation::activate`]
/// bumps the counter exactly like it opens a door, and the engine's AI
/// phase drains it. No parallel trigger system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScriptActivation {
    /// Activations not yet consumed.
    pub pending: u32,
}

impl ScriptActivation {
    /// The most activations kept between two AI phases, so a pathological
    /// `multi_manager` chain cannot grow this without bound. Project-owned.
    pub const MAX_PENDING: u32 = 64;

    /// Records one activation, saturating at [`Self::MAX_PENDING`].
    pub const fn activate(&mut self) {
        if self.pending < Self::MAX_PENDING {
            self.pending += 1;
        }
    }

    /// Consumes one activation, reporting whether there was one.
    pub const fn take(&mut self) -> bool {
        if self.pending == 0 {
            return false;
        }
        self.pending -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MoveTo, SCRIPTED_SEQUENCE_CLASSNAME, SPAWNFLAG_SCRIPT_NO_INTERRUPTIONS, ScriptActivation,
        ScriptDef, SentenceDef,
    };
    use crate::keyvalues::{EntityDef, Limits, parse_entity};
    use ohl_formats::bsp30::Entity as RawEntity;

    fn def(pairs: &[(&str, &str)]) -> EntityDef {
        let raw: RawEntity = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        parse_entity(&raw, &Limits::default())
    }

    #[test]
    fn the_documented_move_to_choices_round_trip_and_three_is_rejected() {
        for mode in [
            MoveTo::No,
            MoveTo::Walk,
            MoveTo::Run,
            MoveTo::Instantaneous,
            MoveTo::TurnToFace,
        ] {
            assert_eq!(MoveTo::from_raw(mode.to_raw()), Some(mode));
        }
        assert_eq!(MoveTo::from_raw(3), None, "3 is Source-only, not GoldSrc");
        assert_eq!(MoveTo::from_raw(-1), None);
        assert_eq!(MoveTo::from_raw(6), None);
    }

    #[test]
    fn a_script_definition_reads_its_published_keys() {
        let script = ScriptDef::from_def(&def(&[
            ("classname", SCRIPTED_SEQUENCE_CLASSNAME),
            ("origin", "16 32 48"),
            ("angles", "0 90 0"),
            ("m_iszEntity", "ohl_actor"),
            ("m_iszPlay", "ohl_play"),
            ("m_iszIdle", "ohl_idle"),
            ("m_flRadius", "256"),
            ("m_flRepeat", "2"),
            ("m_fMoveTo", "1"),
            ("target", "ohl_after"),
            ("delay", "3"),
            ("spawnflags", "32"),
        ]))
        .expect("a scripted_sequence parses");
        assert_eq!(script.target_monster, "ohl_actor");
        assert_eq!(script.play_sequence(), Some("ohl_play"));
        assert_eq!(script.idle_sequence(), Some("ohl_idle"));
        assert!((script.radius - 256.0).abs() < f32::EPSILON);
        assert!((script.repeat - 2.0).abs() < f32::EPSILON);
        assert_eq!(script.move_to, MoveTo::Walk);
        assert_eq!(script.target, "ohl_after");
        assert!((script.delay - 3.0).abs() < f32::EPSILON);
        assert!((script.yaw - 90.0).abs() < f32::EPSILON);
        assert!(script.no_interruptions());
        assert!(!script.repeatable());
        assert!(!script.ai_script);
    }

    #[test]
    fn an_ai_script_is_always_uninterruptible_and_has_no_idle_animation() {
        let script = ScriptDef::from_def(&def(&[
            ("classname", "aiscripted_sequence"),
            ("m_iszIdle", "ohl_idle"),
        ]))
        .expect("an aiscripted_sequence parses");
        assert!(script.ai_script);
        assert!(script.no_interruptions());
        assert!(script.overrides_ai());
        assert_eq!(script.idle_sequence(), None);
        assert_eq!(script.spawnflags & SPAWNFLAG_SCRIPT_NO_INTERRUPTIONS, 0);
    }

    #[test]
    fn another_classname_is_not_a_script() {
        assert!(ScriptDef::from_def(&def(&[("classname", "func_door")])).is_none());
        assert!(SentenceDef::from_def(&def(&[("classname", "func_door")])).is_none());
    }

    #[test]
    fn a_sentence_definition_reads_its_published_keys() {
        let sentence = SentenceDef::from_def(&def(&[
            ("classname", "scripted_sentence"),
            ("sentence", "OHL_GROUP"),
            ("entity", "ohl_speaker"),
            ("listener", "player"),
            ("radius", "512"),
            ("duration", "4"),
            ("refire", "8"),
            ("volume", "42"),
            ("attenuation", "2"),
            ("spawnflags", "1"),
        ]))
        .expect("a scripted_sentence parses");
        assert_eq!(sentence.sentence, "OHL_GROUP");
        assert_eq!(sentence.speaker, "ohl_speaker");
        assert_eq!(sentence.listener, "player");
        assert!((sentence.radius - 512.0).abs() < f32::EPSILON);
        assert!((sentence.duration - 4.0).abs() < f32::EPSILON);
        assert!((sentence.refire - 8.0).abs() < f32::EPSILON);
        assert!(
            (sentence.volume - 10.0).abs() < f32::EPSILON,
            "the published range is 0-10"
        );
        assert_eq!(sentence.attenuation, 2);
        assert!(sentence.fire_once());
    }

    #[test]
    fn activations_are_counted_and_bounded() {
        let mut activation = ScriptActivation::default();
        assert!(!activation.take());
        for _ in 0..(ScriptActivation::MAX_PENDING * 4) {
            activation.activate();
        }
        assert_eq!(activation.pending, ScriptActivation::MAX_PENDING);
        assert!(activation.take());
        assert_eq!(activation.pending, ScriptActivation::MAX_PENDING - 1);
    }
}
