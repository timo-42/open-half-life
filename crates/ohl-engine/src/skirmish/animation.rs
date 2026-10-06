//! Third-person bot presentation. Asset-path and suffix provenance is in
//! `docs/FORMAT_SOURCES.md`, "Skirmish held weapons and combat animations".
//! Sequence labels are matched as data against project-authored intents.

use ohl_combat::WeaponId;
use ohl_game::hecs::Entity;
use ohl_gameplay::ViewModelAction;
use ohl_world::StudioModel;

use crate::components::{HeldWeapon, StudioAnim};
use crate::level::Level;

/// Model slots loaded once when the match starts, shared by all bots.
pub(crate) struct BotModels {
    pub(crate) player: Option<usize>,
    pub(crate) weapons: Vec<(WeaponId, usize)>,
}

/// Optional third-person weapon models; missing assets remain optional.
pub(crate) const fn held_model_path(weapon: WeaponId) -> &'static str {
    match weapon {
        WeaponId::Crowbar => "models/p_crowbar.mdl",
        WeaponId::Glock => "models/p_9mmhandgun.mdl",
        WeaponId::Python => "models/p_357.mdl",
        WeaponId::Mp5 => "models/p_9mmar.mdl",
        WeaponId::Shotgun => "models/p_shotgun.mdl",
        WeaponId::Crossbow => "models/p_crossbow.mdl",
        WeaponId::Rpg => "models/p_rpg.mdl",
        WeaponId::Gauss => "models/p_gauss.mdl",
        WeaponId::Egon => "models/p_egon.mdl",
        WeaponId::HornetGun => "models/p_hgun.mdl",
        WeaponId::HandGrenade => "models/p_grenade.mdl",
        WeaponId::Satchel => "models/p_satchel.mdl",
        WeaponId::Tripmine => "models/p_tripmine.mdl",
        WeaponId::Snark => "models/p_squeak.mdl",
    }
}

const fn weapon_suffix(weapon: WeaponId) -> &'static str {
    match weapon {
        WeaponId::Crowbar => "crowbar",
        WeaponId::Python => "python",
        WeaponId::Mp5 => "mp5",
        WeaponId::Shotgun => "shotgun",
        WeaponId::Crossbow => "bow",
        WeaponId::Rpg => "rpg",
        WeaponId::Gauss => "gauss",
        WeaponId::Egon => "egon",
        WeaponId::Tripmine => "trip",
        WeaponId::Snark => "squeak",
        WeaponId::Glock | WeaponId::HornetGun | WeaponId::HandGrenade | WeaponId::Satchel => {
            "onehanded"
        }
    }
}

fn crouched_sequence(label: &str) -> bool {
    label
        .split('_')
        .any(|token| matches!(token, "crouch" | "duck"))
}

fn sequence_for(
    model: &StudioModel,
    weapon: Option<WeaponId>,
    intents: &[&str],
    crouched: bool,
) -> Option<usize> {
    let suffix = weapon.map(weapon_suffix);
    // Prefer an armed sequence with the right stance, then a generic one.
    for armed in [true, false] {
        for stance in [crouched, !crouched] {
            if let Some(index) = model.sequence_names.iter().position(|label| {
                crouched_sequence(label) == stance
                    && (!armed
                        || suffix.is_some_and(|suffix| label.rsplit('_').next() == Some(suffix)))
                    && (armed
                        || label
                            .rsplit('_')
                            .next()
                            .is_some_and(|last| intents.contains(&last)))
                    && label.split('_').any(|token| intents.contains(&token))
            }) {
                return Some(index);
            }
        }
    }
    None
}

pub(crate) struct BotAnimation {
    models: Vec<(WeaponId, usize)>,
    weapon: Option<WeaponId>,
    action: Option<(ViewModelAction, usize)>,
}

impl BotAnimation {
    pub(crate) fn new(models: &BotModels) -> Self {
        Self {
            models: models.weapons.clone(),
            weapon: None,
            action: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.weapon = None;
        self.action = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update(
        &mut self,
        level: &mut Level,
        entity: Entity,
        weapon: Option<WeaponId>,
        firing: bool,
        reloading: bool,
        speed: f32,
        crouched: bool,
    ) {
        if self.weapon != weapon {
            self.weapon = weapon;
            self.action = None;
        }
        if let Ok(mut held) = level.registry.world.get::<&mut HeldWeapon>(entity) {
            held.model = self
                .models
                .iter()
                .find_map(|(id, model)| (Some(*id) == weapon).then_some(*model));
        }
        let Ok(mut anim) = level.registry.world.get::<&mut StudioAnim>(entity) else {
            return;
        };
        let Some(model) = level.studio_models.get(anim.model) else {
            return;
        };
        let requested = if firing {
            Some((ViewModelAction::Fire, &["shoot", "fire", "attack"][..]))
        } else if reloading {
            Some((ViewModelAction::Reload, &["reload"][..]))
        } else {
            None
        };
        if let Some((action, intents)) = requested
            && let Some(sequence) = sequence_for(model, weapon, intents, crouched)
            && (action == ViewModelAction::Fire || self.action != Some((action, sequence)))
        {
            anim.sequence = sequence;
            anim.cycle = 0.0;
            anim.frame_rate = 1.0;
            self.action = Some((action, sequence));
        }
        if let Some((action, sequence)) = self.action {
            let playing = match action {
                ViewModelAction::Fire => model
                    .sequences
                    .get(sequence)
                    .is_some_and(|sequence| anim.cycle < sequence.duration()),
                ViewModelAction::Reload => reloading,
                _ => false,
            };
            if playing {
                return;
            }
            self.action = None;
        }
        let intents: &[&str] = if speed > 150.0 {
            &["run"]
        } else if speed > 10.0 {
            &["walk"]
        } else {
            &["aim", "idle"]
        };
        if let Some(sequence) = sequence_for(model, weapon, intents, crouched)
            .or_else(|| model.sequence_by_name("idle"))
        {
            anim.play(sequence);
            anim.frame_rate = 1.0;
        }
    }
}
