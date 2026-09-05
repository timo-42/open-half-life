//! Viewmodel presentation actions, and the mapping from an
//! `ohl_combat::WeaponAction` to one (plus, separately, a [`WeaponCue`]
//! naming which sound cue it implies).
//!
//! Neither type carries any published data: `ViewModelAction` is this
//! project's own small, closed vocabulary for "which animation should the
//! viewmodel play next", the same design choice `ohl_combat::firing`
//! documents for its own `Sequence` enum (per-model sequence *names* like
//! `fire1` are QC data this crate never loads).

use ohl_combat::{Sequence, SoundKind, WeaponAction};

/// Which viewmodel animation the presentation layer should play next. Later
/// M7 work (viewmodel rendering) is the actual consumer; this crate only
/// decides *which* one, from combat's `WeaponAction`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewModelAction {
    /// The weapon was just drawn.
    Draw,
    /// No shot fired or reload in progress; the idle loop.
    Idle,
    /// A shot was just fired.
    Fire,
    /// A reload just started.
    Reload,
    /// The weapon is being holstered.
    Holster,
}

/// Which sound cue a [`crate::sounds::SoundCue`] names for a weapon action.
/// A closed, project-defined vocabulary (see [`ViewModelAction`]'s docs);
/// not itself a published sound name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponCue {
    /// A shot fired.
    Fire,
    /// A reload started.
    Reload,
    /// Primary or secondary fire was pressed with nothing to fire.
    Empty,
    /// The weapon was drawn.
    Draw,
    /// The weapon was holstered.
    Holster,
}

/// Maps one `WeaponAction` to the [`ViewModelAction`] (if any) and
/// [`WeaponCue`] (if any) it implies. A single action can carry both (a shot
/// both fires the animation and cues its sound), either, or neither
/// (`WeaponAction::Empty`, and the gauss overcharge cue, which this crate
/// leaves to a dedicated presentation effect rather than the ordinary fire
/// cue).
#[must_use]
pub fn from_weapon_action(action: WeaponAction) -> (Option<ViewModelAction>, Option<WeaponCue>) {
    match action {
        WeaponAction::PlaySequence(Sequence::Draw) => (Some(ViewModelAction::Draw), None),
        WeaponAction::PlaySequence(Sequence::Idle) => (Some(ViewModelAction::Idle), None),
        WeaponAction::PlaySequence(Sequence::Fire) => (Some(ViewModelAction::Fire), None),
        WeaponAction::PlaySequence(Sequence::Reload) => (Some(ViewModelAction::Reload), None),
        WeaponAction::PlaySequence(Sequence::Holster) => (Some(ViewModelAction::Holster), None),
        WeaponAction::Hitscan { .. }
        | WeaponAction::Melee
        | WeaponAction::SpawnProjectile { .. }
        | WeaponAction::BeamTick => (Some(ViewModelAction::Fire), Some(WeaponCue::Fire)),
        WeaponAction::Sound(SoundKind::Fire) => (None, Some(WeaponCue::Fire)),
        WeaponAction::Sound(SoundKind::Reload) => (None, Some(WeaponCue::Reload)),
        WeaponAction::Sound(SoundKind::Draw) => (None, Some(WeaponCue::Draw)),
        WeaponAction::Sound(SoundKind::Holster) => (None, Some(WeaponCue::Holster)),
        WeaponAction::Sound(SoundKind::DryFire) => (None, Some(WeaponCue::Empty)),
        WeaponAction::Sound(SoundKind::Overcharge) | WeaponAction::Empty => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::{ViewModelAction, WeaponCue, from_weapon_action};
    use ohl_combat::{Sequence, SoundKind, WeaponAction};

    #[test]
    fn a_hitscan_shot_fires_the_animation_and_cues_the_fire_sound() {
        let action = WeaponAction::Hitscan {
            count: 1,
            spread: 0.0,
        };
        assert_eq!(
            from_weapon_action(action),
            (Some(ViewModelAction::Fire), Some(WeaponCue::Fire))
        );
    }

    #[test]
    fn a_dry_fire_cues_the_empty_sound_with_no_animation_change() {
        assert_eq!(
            from_weapon_action(WeaponAction::Sound(SoundKind::DryFire)),
            (None, Some(WeaponCue::Empty))
        );
    }

    #[test]
    fn drawing_plays_the_draw_animation_with_no_sound_cue_of_its_own() {
        assert_eq!(
            from_weapon_action(WeaponAction::PlaySequence(Sequence::Draw)),
            (Some(ViewModelAction::Draw), None)
        );
    }

    #[test]
    fn an_empty_action_implies_nothing() {
        assert_eq!(from_weapon_action(WeaponAction::Empty), (None, None));
    }
}
