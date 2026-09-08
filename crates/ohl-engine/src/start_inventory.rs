//! `--start-inventory`: seeding the player's weapons and ammo at map load
//! (dev-tools only), so a single-map probe or scenario can model the
//! inventory a real campaign run would have carried across a `changelevel`
//! without walking the whole campaign chain first.
//!
//! [`.plan/progress-probe-5.md`] found that a cold, single-map load always
//! starts with [`ohl_combat::Inventory::new`]'s empty inventory — no
//! weapon, not even the crowbar — and that at least two mid-campaign maps
//! (`c1a2`, `c3a2`) place their only weapon pickup far past a
//! `func_breakable`/`func_pushable` frontier a fresh, unarmed spawn cannot
//! cross, a chicken-and-egg a real campaign run never hits because the
//! player already carries whatever they picked up on an earlier map. This
//! module lets a dev-tools caller seed that carried-over inventory
//! directly, entirely through the same [`ohl_combat::Inventory::give_weapon`]/
//! [`ohl_combat::Inventory::give_ammo`] API a `weapon_*`/`ammo_*` pickup
//! touch already uses (`crate::pickups::apply_pickup`) — this is not a new
//! grant path, and it never touches the save format (inventory is save
//! tag 23; nothing here changes what that tag records or how it is read).
//!
//! [`parse_start_inventory`] accepts a comma-separated list of the exact
//! same `weapon_*`/`ammo_*` classnames `ohl_combat::classify_classname`
//! already recognises (TWHL-cited, `docs/FORMAT_SOURCES.md`, "Pickups and
//! chargers") — no new classname literal is introduced here. Any other
//! token (an unrecognised classname, or a recognised one that is not a
//! weapon or ammo, such as `item_suit`) is a clear, fixed parse error; see
//! [`StartInventoryError`].

use ohl_combat::{AmmoType, PickupKind, WeaponId, classify_classname};

/// One `--start-inventory` list entry, resolved from its classname.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartInventoryItem {
    /// A `weapon_*` classname: unlocks the weapon and grants its bundled
    /// ammo, exactly as touching the same pickup in the map would
    /// (`ohl_combat::weapon_pickup_ammo`).
    Weapon(WeaponId),
    /// An `ammo_*` classname: tops up one ammo type by one pickup's worth
    /// (`ohl_combat::ammo_pickup_amount`).
    Ammo(AmmoType),
}

/// Why a `--start-inventory` list could not be parsed. Carries no user
/// text: the invalid token itself is not media-derived (it is typed on the
/// command line, not read from a map), but this project's logging policy
/// keeps every error path uniform regardless of source, so the two cases
/// below stay these two fixed messages rather than echoing it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartInventoryError {
    /// The token is not one of `ohl_combat::classify_classname`'s
    /// recognised classnames at all.
    Unrecognised,
    /// The token is a recognised classname, but not a `weapon_*`/`ammo_*`
    /// one (for example `item_suit` or `func_healthcharger`) — this list
    /// only ever grants weapons and ammo.
    NotWeaponOrAmmo,
}

impl std::fmt::Display for StartInventoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Unrecognised => {
                "--start-inventory named an item classname this project does not recognise"
            }
            Self::NotWeaponOrAmmo => {
                "--start-inventory only accepts weapon_*/ammo_* item classnames"
            }
        };
        write!(formatter, "{message}")
    }
}

impl std::error::Error for StartInventoryError {}

/// Parses a comma-separated `--start-inventory` list (surrounding
/// whitespace around each token is trimmed; empty tokens, including a
/// trailing comma, are skipped) into an ordered list of
/// [`StartInventoryItem`]s. Repeating a classname stacks it — for example
/// `ammo_buckshot,ammo_buckshot` grants two ammo boxes' worth — the same
/// way touching two ammo boxes on a map would.
///
/// # Errors
///
/// Returns [`StartInventoryError`] on the first token that is not a
/// `weapon_*`/`ammo_*` classname `ohl_combat::classify_classname`
/// recognises.
pub fn parse_start_inventory(spec: &str) -> Result<Vec<StartInventoryItem>, StartInventoryError> {
    spec.split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| match classify_classname(token) {
            Some(PickupKind::Weapon(id)) => Ok(StartInventoryItem::Weapon(id)),
            Some(PickupKind::Ammo(kind)) => Ok(StartInventoryItem::Ammo(kind)),
            Some(_) => Err(StartInventoryError::NotWeaponOrAmmo),
            None => Err(StartInventoryError::Unrecognised),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_weapons_and_ammo_by_the_same_cited_classnames_pickups_use() {
        let items = parse_start_inventory("weapon_shotgun, ammo_buckshot ,weapon_crowbar")
            .expect("every token is a recognised weapon/ammo classname");
        assert_eq!(
            items,
            vec![
                StartInventoryItem::Weapon(WeaponId::Shotgun),
                StartInventoryItem::Ammo(AmmoType::Buckshot),
                StartInventoryItem::Weapon(WeaponId::Crowbar),
            ]
        );
    }

    #[test]
    fn repeating_a_classname_stacks_it() {
        let items = parse_start_inventory("ammo_buckshot,ammo_buckshot")
            .expect("both tokens are the same recognised classname");
        assert_eq!(
            items,
            vec![
                StartInventoryItem::Ammo(AmmoType::Buckshot),
                StartInventoryItem::Ammo(AmmoType::Buckshot),
            ]
        );
    }

    #[test]
    fn empty_and_whitespace_only_tokens_are_skipped() {
        let items =
            parse_start_inventory(" weapon_egon ,, ,").expect("only one real token is present");
        assert_eq!(items, vec![StartInventoryItem::Weapon(WeaponId::Egon)]);
    }

    #[test]
    fn empty_list_is_empty() {
        assert_eq!(
            parse_start_inventory("").expect("an empty list is valid"),
            vec![]
        );
    }

    #[test]
    fn an_unrecognised_classname_is_a_clear_error() {
        assert_eq!(
            parse_start_inventory("weapon_not_a_real_weapon"),
            Err(StartInventoryError::Unrecognised)
        );
    }

    #[test]
    fn a_recognised_non_weapon_non_ammo_classname_is_a_clear_error() {
        assert_eq!(
            parse_start_inventory("item_suit"),
            Err(StartInventoryError::NotWeaponOrAmmo)
        );
        assert_eq!(
            parse_start_inventory("func_healthcharger"),
            Err(StartInventoryError::NotWeaponOrAmmo)
        );
    }
}
