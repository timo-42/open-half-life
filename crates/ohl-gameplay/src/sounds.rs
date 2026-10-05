//! Sound cues, the asset a cue names, and reviewed weapon/pickup lookups.
//!
//! [`SoundCue`] is this crate's own lightweight "please play this" record —
//! an owning entity, an `ohl_audio::ChannelClass`, the asset it names and
//! how it should be spatialised — rather than an `ohl_audio::PlayRequest`
//! itself. A `PlayRequest` embeds a decoded `Arc<SoundBuffer>`
//! (`ohl_audio::mixer::channel`), and this crate never touches a sound
//! file, so it has no buffer to put there; the composition root resolves a
//! cue's [`SoundAsset`] against its own loaded-buffer cache and only then
//! builds the real `PlayRequest`.
//!
//! # Where a cue's asset path comes from
//!
//! Runtime asset paths and built-in lookup paths have separate provenance.
//!
//! A **map- or data-authored** path is one the payload itself supplies at
//! run time: an `ambient_generic`'s published `message` keyvalue (a
//! `sound/`-relative WAV name, or `!NAME` for a `sentences.txt` group), or
//! the word samples a `sentences.txt` entry names. Those flow through
//! [`SoundAsset::File`]/[`SoundAsset::Sentence`] as owned strings read out
//! of the user's own installation. No literal enters this repository for
//! them, and they are what makes a level audible today.
//!
//! A **built-in** path is one the *engine* would have to know: which WAV a
//! 9mm handgun fires with, which one a medkit is picked up with, which one
//! a suit charger hums. **No path literal here is drawn from any user
//! medium.** `docs/CLEAN_ROOM.md` rule 7 requires an explicit clean-room
//! provenance review before any name or path literal derived from user
//! media enters source. A bounded public documentation review admits the
//! weapon/pickup identifiers below (see `docs/FORMAT_SOURCES.md`, "Skirmish
//! sky and combat sound compatibility"). Unmapped actions remain unresolved.
//! A separate public provenance review approved a
//! small HEV sentence-identifier subset; engine presentation resolves those
//! identifiers through the runtime sentence table, not a hardcoded WAV table.
//! See `docs/FORMAT_SOURCES.md`, "Bounded HEV damage sentence audio".
//! Approval of an identifier does not establish trigger timing or add a
//! missing event producer. Other categories may need producer work as well
//! as reviewed asset mappings before they become audible.

use std::sync::Arc;

use ohl_audio::{ATTN_NORM, ChannelClass, VOL_NORM};
use ohl_combat::PickupKind;
use ohl_combat::WeaponId;

use crate::viewmodel::WeaponCue;

/// What a [`SoundCue`] asks the composition root to play.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum SoundAsset {
    /// Nothing playable: the cue happened, but this project has no
    /// reviewed asset path for it (see the module docs). The composition
    /// root drops it silently.
    #[default]
    Unresolved,
    /// One sound file, by game-relative asset path (`sound/...`).
    File(Arc<str>),
    /// A sentence: several word samples played back to back on one
    /// channel, in the order given. The published `sentences.txt`
    /// behaviour; see `ohl_engine::SentenceLookup`.
    Sentence(Arc<[Arc<str>]>),
}

impl SoundAsset {
    /// A file cue from a game-relative asset path.
    #[must_use]
    pub fn file(path: impl AsRef<str>) -> Self {
        Self::File(Arc::from(path.as_ref()))
    }

    /// A sentence cue from its word samples' asset paths, in speaking
    /// order. An empty list is [`SoundAsset::Unresolved`]: a sentence with
    /// no words is nothing to play.
    #[must_use]
    pub fn sentence(words: impl IntoIterator<Item = String>) -> Self {
        let words: Vec<Arc<str>> = words.into_iter().map(Arc::from).collect();
        if words.is_empty() {
            return Self::Unresolved;
        }
        Self::Sentence(Arc::from(words))
    }

    /// A file cue from one of this module's reviewed `*_sound_path` lookups.
    #[must_use]
    pub fn from_static(path: Option<&'static str>) -> Self {
        match path {
            Some(path) => Self::File(Arc::from(path)),
            None => Self::Unresolved,
        }
    }

    /// Whether this names nothing playable.
    #[must_use]
    pub const fn is_unresolved(&self) -> bool {
        matches!(self, Self::Unresolved)
    }
}

/// A request to play (or stop) a sound, named by asset path rather than by
/// a decoded buffer. See the module docs for why this is not an
/// `ohl_audio::PlayRequest`.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundCue {
    /// The entity/owner this sound is associated with, matching
    /// `ohl_audio::mixer::PlayRequest::entity`'s replacement semantics.
    pub entity: u32,
    /// Which channel class the composition root should play this on.
    pub class: ChannelClass,
    /// The asset to play. See [`SoundAsset`].
    pub asset: SoundAsset,
    /// Where the sound is in world space, or `None` to play it at the
    /// listener with no distance falloff or panning (a first-person weapon,
    /// a pickup the player just walked over, a UI sound).
    pub origin: Option<[f32; 3]>,
    /// `0.0..=1.0`, `ohl_audio::VOL_NORM` for an unattenuated sound.
    pub volume: f32,
    /// Playback rate multiplier: `1.0` is unmodified pitch, i.e. a
    /// published `pitch` keyvalue divided by `ohl_audio::PITCH_NORM`.
    pub pitch: f32,
    /// The published GoldSrc `ATTN_*` falloff (`ohl_audio::ATTN_NORM` and
    /// friends). Ignored when `origin` is `None`.
    pub attenuation: f32,
    /// Stop whatever this `(entity, class)` pair is playing instead of
    /// starting anything. What turning an `ambient_generic` back off does.
    pub stop: bool,
    /// Play the sample once even if its WAV carries loop markers. Weapon
    /// and pickup actions use this; map ambience retains the sample's loop.
    pub one_shot: bool,
}

impl SoundCue {
    /// A cue that starts `asset` on `(entity, class)`, played at the
    /// listener (no spatialisation) at normal volume and pitch.
    #[must_use]
    pub fn new(entity: u32, class: ChannelClass, asset: SoundAsset) -> Self {
        Self {
            entity,
            class,
            asset,
            origin: None,
            volume: VOL_NORM,
            pitch: 1.0,
            attenuation: ATTN_NORM,
            stop: false,
            one_shot: false,
        }
    }

    /// Plays this cue once, ignoring sample loop markers.
    #[must_use]
    pub fn once(mut self) -> Self {
        self.one_shot = true;
        self
    }

    /// The same cue, spatialised at `origin` with `attenuation`.
    #[must_use]
    pub fn at(mut self, origin: [f32; 3], attenuation: f32) -> Self {
        self.origin = Some(origin);
        self.attenuation = attenuation;
        self
    }

    /// The same cue at `volume` and pitch multiplier `pitch`.
    #[must_use]
    pub fn with_gain(mut self, volume: f32, pitch: f32) -> Self {
        self.volume = volume;
        self.pitch = pitch;
        self
    }

    /// A cue that stops whatever `(entity, class)` is playing.
    #[must_use]
    pub fn stopping(entity: u32, class: ChannelClass) -> Self {
        Self {
            stop: true,
            ..Self::new(entity, class, SoundAsset::Unresolved)
        }
    }
}

/// A reviewed sample for an existing weapon cue. The mapping is
/// project-authored; exact original variants/timing remain unmeasured.
/// See `docs/FORMAT_SOURCES.md`, "Skirmish sky and combat sound compatibility".
#[must_use]
pub const fn weapon_sound_path(weapon: WeaponId, cue: WeaponCue) -> Option<&'static str> {
    use WeaponCue::{Empty, Fire, Reload};
    use WeaponId::{Crossbow, Crowbar, Egon, Gauss, Glock, Mp5, Python, Rpg, Shotgun};
    match (weapon, cue) {
        (Crowbar, Fire) => Some("sound/weapons/cbar_miss1.wav"),
        (Glock, Fire) => Some("sound/weapons/pl_gun3.wav"),
        (Python, Fire) => Some("sound/weapons/357_shot1.wav"),
        (Mp5, Fire) => Some("sound/weapons/hks1.wav"),
        (Shotgun, Fire) => Some("sound/weapons/sbarrel1.wav"),
        (Crossbow, Fire) => Some("sound/weapons/xbow_fire1.wav"),
        (Rpg, Fire) => Some("sound/weapons/rocketfire1.wav"),
        (Gauss, Fire) => Some("sound/weapons/gauss2.wav"),
        (Egon, Fire) => Some("sound/weapons/egon_run3.wav"),
        (Glock | Mp5, Reload) => Some("sound/items/cliprelease1.wav"),
        (Python, Reload) => Some("sound/weapons/357_reload1.wav"),
        (Shotgun, Reload) => Some("sound/weapons/reload1.wav"),
        (Crossbow, Reload) => Some("sound/weapons/xbow_reload1.wav"),
        (Glock | Python | Mp5 | Shotgun | Crossbow | Rpg | Gauss | Egon, Empty) => {
            Some("sound/weapons/dryfire1.wav")
        }
        _ => None,
    }
}

/// A reviewed sample for a taken pickup; see [`weapon_sound_path`].
#[must_use]
pub const fn pickup_sound_path(kind: PickupKind) -> Option<&'static str> {
    match kind {
        PickupKind::Weapon(_) | PickupKind::Battery | PickupKind::LongJump => {
            Some("sound/items/gunpickup2.wav")
        }
        PickupKind::Ammo(_) | PickupKind::WeaponBox => Some("sound/items/ammopickup1.wav"),
        PickupKind::HealthKit => Some("sound/items/smallmedkit1.wav"),
        _ => None,
    }
}

/// The asset path for a health/suit charger's use loop. **To be black-box
/// observed**; see [`weapon_sound_path`].
// TODO(black-box): fill in once a clean-room provenance review admits a
// charger sound asset path.
#[must_use]
pub const fn charger_sound_path() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::{SoundAsset, SoundCue, charger_sound_path, pickup_sound_path, weapon_sound_path};
    use ohl_audio::{ATTN_NORM, ChannelClass, VOL_NORM};
    use ohl_combat::{PickupKind, WeaponId};

    #[test]
    fn unmapped_actions_remain_unresolved() {
        assert_eq!(
            weapon_sound_path(WeaponId::Glock, crate::viewmodel::WeaponCue::Holster),
            None
        );
        assert_eq!(pickup_sound_path(PickupKind::Suit), None);
        assert_eq!(charger_sound_path(), None);
        assert!(SoundAsset::from_static(None).is_unresolved());
    }

    #[test]
    fn a_new_cue_plays_at_the_listener_at_normal_gain() {
        let cue = SoundCue::new(3, ChannelClass::Weapon, SoundAsset::Unresolved);
        assert_eq!(cue.origin, None);
        assert!((cue.volume - VOL_NORM).abs() < 1e-6);
        assert!((cue.pitch - 1.0).abs() < 1e-6);
        assert!((cue.attenuation - ATTN_NORM).abs() < 1e-6);
        assert!(!cue.stop);
    }

    #[test]
    fn a_map_authored_path_becomes_a_spatialised_file_cue() {
        // A project-authored synthetic path: nothing here is drawn from
        // any user medium (see the module docs).
        let cue = SoundCue::new(
            9,
            ChannelClass::Static,
            SoundAsset::file("sound/ohl/synthetic.wav"),
        )
        .at([64.0, 0.0, 32.0], ATTN_NORM)
        .with_gain(0.5, 1.2);
        assert_eq!(cue.origin, Some([64.0, 0.0, 32.0]));
        assert!((cue.volume - 0.5).abs() < 1e-6);
        assert!((cue.pitch - 1.2).abs() < 1e-6);
        assert!(!cue.asset.is_unresolved());
    }

    #[test]
    fn a_sentence_with_no_words_is_not_playable() {
        assert!(SoundAsset::sentence(Vec::new()).is_unresolved());
        let spoken = SoundAsset::sentence(vec![
            "sound/ohl/one.wav".to_string(),
            "sound/ohl/two.wav".to_string(),
        ]);
        match spoken {
            SoundAsset::Sentence(words) => assert_eq!(words.len(), 2),
            other => panic!("expected a sentence, got {other:?}"),
        }
    }

    #[test]
    fn a_stopping_cue_names_no_asset() {
        let cue = SoundCue::stopping(4, ChannelClass::Static);
        assert!(cue.stop);
        assert!(cue.asset.is_unresolved());
    }
}
