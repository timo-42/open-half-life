//! Default studio models for pickups that do not name a model in map data.
//!
//! Paths and the tripmine's display pose come from public editor metadata:
//! FreeSlave's Half-Life FGD and Sven Co-op's published FGD (snark nest and
//! weapon container). Item paths are also listed by TWHL's "Reference:
//! Entities and their models". Every path's source is recorded in
//! `docs/FORMAT_SOURCES.md`, "Pickup models". No asset listing or engine
//! implementation is used by this lookup.

pub(crate) struct PickupModel {
    pub path: &'static str,
    pub sequence: usize,
    pub body: u32,
}

/// A classname's documented display model and initial pose. Classnames
/// remain case-sensitive, just like the pickup classifier. Ammo is matched
/// by classname because the pistol clip and assault-rifle magazine share
/// an ammo type but have different models.
pub(crate) fn default_model(classname: &str) -> Option<PickupModel> {
    let path = match classname {
        "weapon_crowbar" => "models/w_crowbar.mdl",
        "weapon_9mmhandgun" | "weapon_glock" => "models/w_9mmhandgun.mdl",
        "weapon_357" => "models/w_357.mdl",
        "weapon_9mmAR" => "models/w_9mmar.mdl",
        "weapon_shotgun" => "models/w_shotgun.mdl",
        "weapon_crossbow" => "models/w_crossbow.mdl",
        "weapon_rpg" => "models/w_rpg.mdl",
        "weapon_gauss" => "models/w_gauss.mdl",
        "weapon_egon" => "models/w_egon.mdl",
        "weapon_hornetgun" => "models/w_hgun.mdl",
        "weapon_handgrenade" => "models/w_grenade.mdl",
        "weapon_satchel" => "models/w_satchel.mdl",
        "weapon_tripmine" => {
            // The published FGD selects the ground pose and the body
            // without the player's hands from this shared view model.
            return Some(PickupModel {
                path: "models/v_tripmine.mdl",
                sequence: 8,
                body: 3,
            });
        }
        "weapon_snark" => "models/w_sqknest.mdl",
        "ammo_9mmclip" | "ammo_glockclip" => "models/w_9mmclip.mdl",
        "ammo_9mmAR" => "models/w_9mmarclip.mdl",
        "ammo_ARgrenades" | "ammo_mp5grenades" => "models/w_argrenade.mdl",
        "ammo_357" => "models/w_357ammobox.mdl",
        "ammo_buckshot" => "models/w_shotbox.mdl",
        "ammo_crossbow" => "models/w_crossbow_clip.mdl",
        "ammo_rpgclip" => "models/w_rpgammo.mdl",
        "ammo_gaussclip" => "models/w_gaussammo.mdl",
        "item_healthkit" => "models/w_medkit.mdl",
        "item_battery" => "models/w_battery.mdl",
        "item_suit" => "models/w_suit.mdl",
        "item_longjump" => "models/w_longjump.mdl",
        "item_security" => "models/w_security.mdl",
        "weaponbox" => "models/w_weaponbox.mdl",
        _ => return None,
    };
    Some(PickupModel {
        path,
        sequence: 0,
        body: 0,
    })
}
