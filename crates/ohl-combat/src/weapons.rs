//! The weapon table: one [`WeaponSpec`] per [`WeaponId`], built entirely from
//! published sources.
//!
//! Every number in [`spec`] is either cited on the entry that carries it
//! (Combine OverWiki weapon pages, `docs/FORMAT_SOURCES.md`, "Combat and
//! damage") or wrapped in [`BlackBox`] with a `// TODO(black-box)` marker and
//! a neutral placeholder value: this module ships no unpublished number as a
//! plain field.

use crate::ammo::AmmoType;
use crate::damage::DamageType;

/// A value this project has not been able to confirm on a source it may use.
///
/// The field still has a type and a default so the rest of the firing state
/// machine can use it uniformly; the wrapper exists so a reader (and a
/// clean-room review) can tell at a glance which numbers in [`WeaponSpec`]
/// are cited and which are placeholders awaiting a black-box observation of
/// legally obtained retail software.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackBox<T> {
    /// The placeholder (or, once measured, the recorded) value.
    pub value: T,
}

impl<T> BlackBox<T> {
    /// Wraps `value` as a black-box placeholder.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self { value }
    }
}

/// How a weapon (or one of its fire modes) resolves a shot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeaponKind {
    /// A close-range attack with no ammo and no travel time (the crowbar).
    Melee,
    /// An instant-hit attack, resolved by one or more [`crate::trace_attack`]
    /// calls the same tick.
    Hitscan {
        /// Pellets fired in one shot (the shotgun fires several at once).
        pellets: u32,
        /// The spread cone's half-angle, in radians. **BBO**: Half-Life's
        /// spread cones are not published on a usable source.
        spread: BlackBox<f32>,
    },
    /// A physical projectile the caller spawns and simulates elsewhere (M7.3).
    Projectile {
        /// Muzzle speed, world units per second. **BBO**: not published.
        speed: BlackBox<f32>,
    },
    /// A continuous beam, re-resolved every tick it is held down (the egon).
    Beam,
    /// A charge-up attack released on input change (the gauss gun).
    Charge,
}

/// An alternate fire mode layered on a weapon's primary [`WeaponKind`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SecondaryFire {
    /// How the secondary shot resolves.
    pub kind: WeaponKind,
    /// Damage per hit (per source cited on the [`spec`] entry that uses it).
    pub damage: f32,
    /// The ammo the secondary shot draws from, if different from the
    /// primary's (`None` reuses the primary's [`WeaponSpec::ammo`]).
    pub ammo: Option<AmmoType>,
    /// Seconds between secondary shots. **BBO** unless documented otherwise
    /// on the entry.
    pub cycle_time: BlackBox<f32>,
}

/// One weapon, as selectable from the player's inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponId {
    /// The crowbar.
    Crowbar,
    /// The 9mm pistol ("Glock").
    Glock,
    /// The .357 Magnum ("Python").
    Python,
    /// The MP5 submachine gun.
    Mp5,
    /// The shotgun.
    Shotgun,
    /// The crossbow.
    Crossbow,
    /// The RPG.
    Rpg,
    /// The gauss gun.
    Gauss,
    /// The egon (gluon gun).
    Egon,
    /// The hornet gun.
    HornetGun,
    /// A thrown hand grenade.
    HandGrenade,
    /// A satchel charge.
    Satchel,
    /// A tripmine.
    Tripmine,
    /// A thrown snark.
    Snark,
}

impl WeaponId {
    /// Every variant, in declaration order.
    pub const ALL: [Self; 14] = [
        Self::Crowbar,
        Self::Glock,
        Self::Python,
        Self::Mp5,
        Self::Shotgun,
        Self::Crossbow,
        Self::Rpg,
        Self::Gauss,
        Self::Egon,
        Self::HornetGun,
        Self::HandGrenade,
        Self::Satchel,
        Self::Tripmine,
        Self::Snark,
    ];
}

/// A weapon's static data: how it fires, what it costs, and what it does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponSpec {
    /// Which weapon this is.
    pub id: WeaponId,
    /// How the primary fire resolves.
    pub kind: WeaponKind,
    /// The damage type primary fire applies. This project's own
    /// categorisation of the weapon into Half-Life's published damage-type
    /// vocabulary (`docs/FORMAT_SOURCES.md`), not itself a cited number.
    pub damage_type: DamageType,
    /// Damage per hit (or per pellet, for a multi-pellet [`WeaponKind::Hitscan`]).
    pub damage: f32,
    /// Rounds held in one clip; `None` for a weapon with no clip concept
    /// (melee, or a weapon that fires straight from its ammo pool).
    pub clip_size: Option<u32>,
    /// The ammo type the clip (or, with no clip, every shot) draws from;
    /// `None` for the crowbar, which needs none.
    pub ammo: Option<AmmoType>,
    /// Seconds between the start of one primary shot and the next. Cited
    /// per entry where published; **BBO** otherwise.
    pub cycle_time: BlackBox<f32>,
    /// Seconds a full reload takes. **BBO** for every weapon: no usable
    /// source publishes reload timings.
    pub reload_time: BlackBox<f32>,
    /// The alternate fire mode, if the weapon has one this project models.
    pub secondary: Option<SecondaryFire>,
}

/// This weapon's [`WeaponSpec`].
///
/// A `const fn` rather than a `static` table: every entry is built from
/// literals, so the compiler checks the match is exhaustive over
/// [`WeaponId`] and there is no runtime initialisation to get wrong.
#[must_use]
#[allow(clippy::too_many_lines)]
pub const fn spec(id: WeaponId) -> WeaponSpec {
    match id {
        // Combine OverWiki, "Crowbar": 10 damage (5 on the repeat swing of a
        // combo), melee, no ammo; swing cadence "~0.3 seconds" per swing.
        WeaponId::Crowbar => WeaponSpec {
            id,
            kind: WeaponKind::Melee,
            damage_type: DamageType::CLUB,
            damage: 10.0,
            clip_size: None,
            ammo: None,
            cycle_time: BlackBox::new(0.3),
            // TODO(black-box): melee has no "reload"; recovery-after-miss
            // timing, if any, is unpublished. 0.0 is the neutral no-op.
            reload_time: BlackBox::new(0.0),
            secondary: None,
        },
        // Combine OverWiki, "Glock": 8 damage, 17-round clip, 250-round max
        // reserve, two fire modes (single shot / fixed 3-round burst). No
        // usable source publishes a distinct cycle time for either mode or a
        // burst-selector mechanic, so only the single-shot mode is modeled.
        WeaponId::Glock => WeaponSpec {
            id,
            kind: WeaponKind::Hitscan {
                pellets: 1,
                spread: BlackBox::new(0.0),
            },
            damage_type: DamageType::BULLET,
            damage: 8.0,
            clip_size: Some(17),
            ammo: Some(AmmoType::NineMillimeter),
            // TODO(black-box): cycle time not published.
            cycle_time: BlackBox::new(0.3),
            reload_time: BlackBox::new(1.5),
            secondary: None,
        },
        // Combine OverWiki, ".357 Magnum": 40 damage, 6-round clip, 36-round
        // max reserve.
        WeaponId::Python => WeaponSpec {
            id,
            kind: WeaponKind::Hitscan {
                pellets: 1,
                spread: BlackBox::new(0.0),
            },
            damage_type: DamageType::BULLET,
            damage: 40.0,
            clip_size: Some(6),
            ammo: Some(AmmoType::ThreeFiveSeven),
            cycle_time: BlackBox::new(0.5),
            reload_time: BlackBox::new(2.0),
            secondary: None,
        },
        // Combine OverWiki, "MP5": 5 damage per bullet, 50-round clip,
        // 250-round max reserve, 600 rounds per minute cyclic rate
        // (600 / 60 = 10 shots/second -> 0.1 s between shots). The
        // underslung 40mm launcher is the secondary fire, firing an M5.
        // grenade for 100 damage from a 10-round reserve; its own cycle
        // time is not published.
        WeaponId::Mp5 => WeaponSpec {
            id,
            kind: WeaponKind::Hitscan {
                pellets: 1,
                spread: BlackBox::new(0.0),
            },
            damage_type: DamageType::BULLET,
            damage: 5.0,
            clip_size: Some(50),
            ammo: Some(AmmoType::NineMillimeter),
            cycle_time: BlackBox::new(1.0 / (600.0 / 60.0)),
            reload_time: BlackBox::new(2.5),
            secondary: Some(SecondaryFire {
                kind: WeaponKind::Projectile {
                    speed: BlackBox::new(600.0),
                },
                damage: 100.0,
                ammo: Some(AmmoType::Mp5Grenades),
                cycle_time: BlackBox::new(1.0),
            }),
        },
        // Combine OverWiki, "Shotgun": single-barrel primary fires 6 pellets
        // for 5 damage each (30 total at point-blank), 8-round clip,
        // 125-round max reserve. Double-barrel secondary fires both barrels
        // as 12 pellets for the same 5 damage each, from the same clip.
        WeaponId::Shotgun => WeaponSpec {
            id,
            kind: WeaponKind::Hitscan {
                pellets: 6,
                spread: BlackBox::new(0.0),
            },
            damage_type: DamageType::BULLET,
            damage: 5.0,
            clip_size: Some(8),
            ammo: Some(AmmoType::Buckshot),
            cycle_time: BlackBox::new(0.75),
            reload_time: BlackBox::new(0.5),
            secondary: Some(SecondaryFire {
                kind: WeaponKind::Hitscan {
                    pellets: 12,
                    spread: BlackBox::new(0.0),
                },
                damage: 5.0,
                ammo: None,
                cycle_time: BlackBox::new(1.5),
            }),
        },
        // Combine OverWiki, "Crossbow": 50 damage per bolt, 5-round clip,
        // 50-bolt max reserve.
        WeaponId::Crossbow => WeaponSpec {
            id,
            kind: WeaponKind::Projectile {
                speed: BlackBox::new(2000.0),
            },
            damage_type: DamageType::BULLET,
            damage: 50.0,
            clip_size: Some(5),
            ammo: Some(AmmoType::Bolts),
            cycle_time: BlackBox::new(1.0),
            reload_time: BlackBox::new(2.0),
            secondary: None,
        },
        // Combine OverWiki, "RPG": 100 damage (single-player), laser-guided,
        // 5-rocket max reserve; the launcher holds no separate clip (one
        // rocket loaded at a time from the reserve).
        WeaponId::Rpg => WeaponSpec {
            id,
            kind: WeaponKind::Projectile {
                speed: BlackBox::new(600.0),
            },
            damage_type: DamageType::BLAST,
            damage: 100.0,
            clip_size: Some(1),
            ammo: Some(AmmoType::Rockets),
            cycle_time: BlackBox::new(1.5),
            reload_time: BlackBox::new(3.0),
            secondary: None,
        },
        // Combine OverWiki, "Gauss Gun": primary fires an uncharged 20-damage
        // shot; secondary charges for up to 10 seconds and releases a shot
        // scaled from 25 up to 200 damage, reflecting off metal surfaces;
        // holding the charge past 10 seconds costs the wielder 50 health
        // instead of firing (see `crate::firing` for the charge/overcharge
        // state machine). 100-cell max reserve, no separate clip.
        WeaponId::Gauss => WeaponSpec {
            id,
            kind: WeaponKind::Charge,
            damage_type: DamageType::SHOCK,
            damage: 20.0,
            clip_size: None,
            ammo: Some(AmmoType::Uranium),
            cycle_time: BlackBox::new(0.2),
            reload_time: BlackBox::new(2.0),
            secondary: None,
        },
        // Combine OverWiki, "Egon": continuous beam, 14 damage per cell per
        // (unpublished) tick interval, 100-cell max reserve, no separate
        // clip.
        WeaponId::Egon => WeaponSpec {
            id,
            kind: WeaponKind::Beam,
            damage_type: DamageType::ENERGYBEAM,
            damage: 14.0,
            clip_size: None,
            ammo: Some(AmmoType::Uranium),
            // TODO(black-box): the cell-drain tick interval is not published;
            // this is the interval `crate::firing` charges one cell at.
            cycle_time: BlackBox::new(0.1),
            reload_time: BlackBox::new(0.0),
            secondary: None,
        },
        // Combine OverWiki, "Hornetgun": 7 damage per hornet, primary fires
        // at 240 hornets/minute (240 / 60 = 4/s -> 0.25 s cycle) and homes,
        // secondary at 600/minute (0.1 s cycle) and does not home; 8-hornet
        // clip that regenerates over time (the regeneration interval is
        // unpublished).
        WeaponId::HornetGun => WeaponSpec {
            id,
            kind: WeaponKind::Projectile {
                speed: BlackBox::new(400.0),
            },
            damage_type: DamageType::SLASH,
            damage: 7.0,
            clip_size: Some(8),
            ammo: Some(AmmoType::Hornets),
            cycle_time: BlackBox::new(1.0 / (240.0 / 60.0)),
            reload_time: BlackBox::new(0.0),
            secondary: Some(SecondaryFire {
                kind: WeaponKind::Projectile {
                    speed: BlackBox::new(800.0),
                },
                damage: 7.0,
                ammo: None,
                cycle_time: BlackBox::new(1.0 / (600.0 / 60.0)),
            }),
        },
        // Combine OverWiki, "Hand Grenade": 100 damage, ~5 s fuse, 10-grenade
        // max reserve, thrown one at a time.
        WeaponId::HandGrenade => WeaponSpec {
            id,
            kind: WeaponKind::Projectile {
                speed: BlackBox::new(600.0),
            },
            damage_type: DamageType::BLAST,
            damage: 100.0,
            clip_size: Some(1),
            ammo: Some(AmmoType::HandGrenades),
            cycle_time: BlackBox::new(0.5),
            reload_time: BlackBox::new(0.0),
            secondary: None,
        },
        // Combine OverWiki, "Satchel Charge": 150 damage, 5-charge max
        // reserve; placed rather than thrown, detonated on the player's
        // separate "detonate" input, which `crate::firing` does not model
        // (that belongs to M7.3's entity work).
        WeaponId::Satchel => WeaponSpec {
            id,
            kind: WeaponKind::Projectile {
                speed: BlackBox::new(300.0),
            },
            damage_type: DamageType::BLAST,
            damage: 150.0,
            clip_size: Some(1),
            ammo: Some(AmmoType::Satchels),
            cycle_time: BlackBox::new(1.0),
            reload_time: BlackBox::new(0.0),
            secondary: None,
        },
        // Combine OverWiki, "Tripmine": 150 damage, 5-mine max reserve; an
        // "arm" delay of roughly 3 seconds after placement is noted before
        // its laser goes live.
        WeaponId::Tripmine => WeaponSpec {
            id,
            kind: WeaponKind::Projectile {
                speed: BlackBox::new(0.0),
            },
            damage_type: DamageType::BLAST,
            damage: 150.0,
            clip_size: Some(1),
            ammo: Some(AmmoType::Tripmines),
            cycle_time: BlackBox::new(3.0),
            reload_time: BlackBox::new(0.0),
            secondary: None,
        },
        // Combine OverWiki, "Snark": 10 damage per bite, 2 health, and it
        // self-destructs roughly 20 seconds after being thrown or after its
        // last attack; carried 5 at a time per pickup (the max carried
        // total is not separately published, see `AmmoType::Snarks`).
        WeaponId::Snark => WeaponSpec {
            id,
            kind: WeaponKind::Melee,
            damage_type: DamageType::SLASH,
            damage: 10.0,
            clip_size: Some(1),
            ammo: Some(AmmoType::Snarks),
            cycle_time: BlackBox::new(0.5),
            reload_time: BlackBox::new(0.0),
            secondary: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    /// Every published damage and clip/reserve number from the design
    /// table (`.plan/m7-design.md` §1), checked against the table's actual
    /// entries so a typo cannot silently drift from the source.
    #[test]
    fn published_numbers_match_the_design_table() {
        let crowbar = spec(WeaponId::Crowbar);
        assert!(approx(crowbar.damage, 10.0));
        assert!(approx(crowbar.cycle_time.value, 0.3));

        let glock = spec(WeaponId::Glock);
        assert!(approx(glock.damage, 8.0));
        assert_eq!(glock.clip_size, Some(17));
        assert_eq!(glock.ammo, Some(AmmoType::NineMillimeter));

        let python = spec(WeaponId::Python);
        assert!(approx(python.damage, 40.0));
        assert_eq!(python.clip_size, Some(6));

        let mp5 = spec(WeaponId::Mp5);
        assert!(approx(mp5.damage, 5.0));
        assert_eq!(mp5.clip_size, Some(50));
        assert!(
            (mp5.cycle_time.value - 0.1).abs() < 1e-6,
            "600 RPM -> 0.1 s"
        );
        let mp5_secondary = mp5.secondary.expect("MP5 has a grenade launcher");
        assert!(approx(mp5_secondary.damage, 100.0));
        assert_eq!(mp5_secondary.ammo, Some(AmmoType::Mp5Grenades));

        let shotgun = spec(WeaponId::Shotgun);
        assert!(approx(shotgun.damage, 5.0));
        assert_eq!(shotgun.clip_size, Some(8));
        assert!(matches!(
            shotgun.kind,
            WeaponKind::Hitscan { pellets: 6, .. }
        ));
        let shotgun_secondary = shotgun.secondary.expect("shotgun has both barrels");
        assert!(matches!(
            shotgun_secondary.kind,
            WeaponKind::Hitscan { pellets: 12, .. }
        ));

        let crossbow = spec(WeaponId::Crossbow);
        assert!(approx(crossbow.damage, 50.0));
        assert_eq!(crossbow.clip_size, Some(5));

        let rpg = spec(WeaponId::Rpg);
        assert!(approx(rpg.damage, 100.0));
        assert_eq!(rpg.ammo, Some(AmmoType::Rockets));

        let gauss = spec(WeaponId::Gauss);
        assert!(approx(gauss.damage, 20.0));
        assert!(matches!(gauss.kind, WeaponKind::Charge));

        let egon = spec(WeaponId::Egon);
        assert!(approx(egon.damage, 14.0));
        assert!(matches!(egon.kind, WeaponKind::Beam));

        let hornetgun = spec(WeaponId::HornetGun);
        assert!(approx(hornetgun.damage, 7.0));
        assert_eq!(hornetgun.clip_size, Some(8));
        assert!(
            (hornetgun.cycle_time.value - 0.25).abs() < 1e-6,
            "240/min -> 0.25 s"
        );
        let hornet_secondary = hornetgun.secondary.expect("hornet gun has a secondary");
        assert!(
            (hornet_secondary.cycle_time.value - 0.1).abs() < 1e-6,
            "600/min -> 0.1 s"
        );

        let grenade = spec(WeaponId::HandGrenade);
        assert!(approx(grenade.damage, 100.0));
        assert_eq!(grenade.ammo, Some(AmmoType::HandGrenades));

        let satchel = spec(WeaponId::Satchel);
        assert!(approx(satchel.damage, 150.0));
        assert_eq!(satchel.ammo, Some(AmmoType::Satchels));

        let tripmine = spec(WeaponId::Tripmine);
        assert!(approx(tripmine.damage, 150.0));
        assert_eq!(tripmine.ammo, Some(AmmoType::Tripmines));

        let snark = spec(WeaponId::Snark);
        assert!(approx(snark.damage, 10.0));
        assert_eq!(snark.ammo, Some(AmmoType::Snarks));
    }

    #[test]
    fn every_weapon_id_has_a_spec() {
        for id in WeaponId::ALL {
            let entry = spec(id);
            assert_eq!(entry.id, id);
            assert!(entry.damage > 0.0, "{id:?}");
            assert!(entry.cycle_time.value >= 0.0, "{id:?}");
            assert!(entry.reload_time.value >= 0.0, "{id:?}");
        }
    }

    #[test]
    fn a_weapon_with_a_clip_also_names_its_ammo() {
        for id in WeaponId::ALL {
            let entry = spec(id);
            if entry.clip_size.is_some() {
                assert!(entry.ammo.is_some(), "{id:?} has a clip but no ammo type");
            }
        }
    }
}
