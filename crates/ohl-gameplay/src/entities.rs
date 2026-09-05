//! The thin seam between `ohl-game`'s parsed BSP entities and
//! `ohl-combat`'s pickup classification.

use ohl_combat::PickupKind;
use ohl_game::EntityDef;

/// Classifies a parsed map entity's pickup kind, delegating to
/// [`ohl_combat::classify_classname`] on its `classname`. Exists so the
/// composition root can go straight from an `ohl_game::EntityDef` (the
/// typed view of one BSP entities-lump entry) to a `PickupKind` without
/// itself depending on `ohl_combat::pickups`.
#[must_use]
pub fn classify_entity(def: &EntityDef) -> Option<PickupKind> {
    ohl_combat::classify_classname(&def.classname)
}

#[cfg(test)]
mod tests {
    use super::classify_entity;
    use ohl_combat::{PickupKind, WeaponId};
    use ohl_game::EntityDef;
    use std::collections::BTreeMap;

    fn def(classname: &str) -> EntityDef {
        EntityDef {
            classname: classname.to_string(),
            keyvalues: BTreeMap::new(),
            origin: [0.0, 0.0, 0.0],
            angles: [0.0, 0.0, 0.0],
            targetname: None,
            target: None,
            spawnflags: 0,
            model: None,
            render: ohl_game::keyvalues::RenderProps::default(),
        }
    }

    #[test]
    fn a_weapon_entity_def_classifies_the_same_way_its_classname_does() {
        assert_eq!(
            classify_entity(&def("weapon_crowbar")),
            Some(PickupKind::Weapon(WeaponId::Crowbar))
        );
    }

    #[test]
    fn an_unrelated_entity_def_classifies_to_nothing() {
        assert_eq!(classify_entity(&def("func_door")), None);
    }
}
