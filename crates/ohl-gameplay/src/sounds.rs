//! Sound cues and the (currently all-`None`) asset-path lookup table.
//!
//! [`SoundCue`] is this crate's own lightweight "please play this" record —
//! an owning entity, an `ohl_audio::ChannelClass`, and an optional
//! game-relative asset path — rather than an `ohl_audio::PlayRequest`
//! itself. A `PlayRequest` embeds a decoded `Arc<SoundBuffer>`
//! (`ohl_audio::mixer::channel`), and this crate never touches a sound
//! file, so it has no buffer to put there; the composition root resolves a
//! non-`None` [`SoundCue::path`] against its own loaded-buffer cache and
//! only then builds the real `PlayRequest`. A `None` path names a cue this
//! project cannot yet resolve to an asset (see below) and the composition
//! root simply plays nothing for it.
//!
//! **No path literal here is drawn from any user medium.**
//! `docs/CLEAN_ROOM.md` rule 7 requires an explicit clean-room provenance
//! review before any name or path literal derived from user media enters
//! source, and no source this project may use publishes Half-Life's actual
//! sound file layout as reusable data (the per-weapon wiki pages this
//! project already cites describe gameplay behaviour, not asset paths).
//! Every lookup below is therefore `None`, each with its own
//! `// TODO(black-box)` marker, ready to be filled in once that review
//! happens and never invented meanwhile.

use ohl_audio::ChannelClass;
use ohl_combat::PickupKind;
use ohl_combat::WeaponId;

use crate::viewmodel::WeaponCue;

/// A request to play a sound, named by asset path rather than a decoded
/// buffer. See the module docs for why this is not an `ohl_audio::PlayRequest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundCue {
    /// The entity/owner this sound is associated with, matching
    /// `ohl_audio::mixer::PlayRequest::entity`'s replacement semantics.
    pub entity: u32,
    /// Which channel class the composition root should play this on.
    pub class: ChannelClass,
    /// The game-relative asset path, when a usable public source names one.
    pub path: Option<&'static str>,
}

/// The asset path for `weapon`'s `cue` sound. **To be black-box observed**:
/// see the module docs. Always `None` today.
// TODO(black-box): fill in once a clean-room provenance review admits a
// per-weapon sound asset path.
#[must_use]
pub const fn weapon_sound_path(_weapon: WeaponId, _cue: WeaponCue) -> Option<&'static str> {
    None
}

/// The asset path for `kind`'s pickup sound. **To be black-box observed**;
/// see [`weapon_sound_path`].
// TODO(black-box): fill in once a clean-room provenance review admits a
// pickup sound asset path.
#[must_use]
pub const fn pickup_sound_path(_kind: PickupKind) -> Option<&'static str> {
    None
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
    use super::{charger_sound_path, pickup_sound_path, weapon_sound_path};
    use ohl_combat::{PickupKind, WeaponId};

    #[test]
    fn no_asset_path_is_shipped_without_a_provenance_review() {
        assert_eq!(
            weapon_sound_path(WeaponId::Glock, crate::viewmodel::WeaponCue::Fire),
            None
        );
        assert_eq!(pickup_sound_path(PickupKind::HealthKit), None);
        assert_eq!(charger_sound_path(), None);
    }
}
