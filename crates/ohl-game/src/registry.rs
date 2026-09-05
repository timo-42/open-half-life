//! A [`hecs::World`] populated from parsed [`EntityDef`]s, plus a bounded
//! `targetname -> entity` index.
//!
//! Component semantics (which keyvalues a `func_door`, `func_button`,
//! `func_plat`, a light entity, `path_corner`/`path_track`,
//! `trigger_changelevel`/`info_landmark` and `multi_manager` carry, and how
//! `angles` becomes a movement direction) are taken only from public
//! mapping documentation; see `docs/FORMAT_SOURCES.md` ("Entity keyvalues
//! and map logic"). No SDK source or decompiled logic was consulted.

use std::collections::BTreeMap;

use glam::Vec3;
use hecs::{Entity, World};

use crate::keyvalues::{self, EntityDef, Limits, ModelRef, RenderProps};

/// Largest number of entities the name index keeps for one `targetname`.
/// GoldSrc maps rarely share a name across more than a handful of
/// entities (e.g. a bank of lights); this simply bounds a worst case.
const MAX_ENTITIES_PER_NAME: usize = 64;

/// `classname`, kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassName(pub String);

/// Position and facing, in GoldSrc world units and degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// World-space origin.
    pub origin: Vec3,
    /// `pitch yaw roll`, in degrees.
    pub angles: Vec3,
}

/// `model` when it names a brush submodel (`*N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrushModel(pub u32);

/// The world-space centre of a brush entity's submodel bounding box, when
/// `model_bounds` was supplied for its `BrushModel` index. Brush entities'
/// own `origin` keyvalue is conventionally `0 0 0` (their placement is baked
/// into the compiled brush geometry, unlike a point entity), so proximity
/// checks such as "use the nearest door" should prefer this over
/// [`Transform::origin`] when it is present.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushCenter(pub Vec3);

/// `targetname`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetName(pub String);

/// `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target(pub String);

/// `spawnflags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnFlags(pub u32);

/// `rendermode`/`renderamt`/`rendercolor`, re-exported from
/// [`crate::keyvalues`] so a query only needs one component type.
pub type RenderPropsComponent = RenderProps;

/// A translating brush mover's open/close state, shared by [`Door`] and
/// [`Platform`] (documented on the Half-Life mapping wikis as the visible
/// behaviour of these entities: they sit closed/at rest, travel to the far
/// position, optionally wait there, then travel back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoverState {
    /// At the starting position.
    Closed,
    /// Travelling from closed to open.
    Opening,
    /// At the far position, possibly about to auto-return.
    Open,
    /// Travelling from open back to closed.
    Closing,
}

/// `func_door` (and the same keys on `func_door_rotating`'s translating
/// cousins): `speed`, `wait`, `lip`, a movement direction derived from
/// `angles`/`angle`, `dmg`, `health`, `delay` and the `movesnd`/`stopsnd`
/// sound indices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Door {
    /// Units per second.
    pub speed: f32,
    /// Seconds the door stays open before auto-closing; `<= 0` means it
    /// stays open once opened.
    pub wait: f32,
    /// Units subtracted from the travel distance.
    pub lip: f32,
    /// Unit vector the door travels along when opening.
    pub movedir: Vec3,
    /// Damage dealt to anything blocking the door.
    pub dmg: f32,
    /// Hit points before the door can be shot open; `0` means unbreakable
    /// by damage (it only opens via `use`/trigger).
    pub health: f32,
    /// Seconds between being triggered and starting to move.
    pub delay: f32,
    /// `movesnd`/`stopsnd` indices into the built-in door sound tables.
    pub sounds: (u8, u8),
    /// The distance travelled when opening, precomputed from the brush
    /// model's bounding box (`maxs - mins`, projected onto `movedir`) minus
    /// `lip`, since the map logic simulation never touches BSP data
    /// directly.
    pub travel_distance: f32,
    /// Current animation state.
    pub state: MoverState,
    /// Seconds remaining in the current state's motion or wait.
    pub timer: f32,
}

/// `func_button`: `speed`, `wait`, `health`, `delay` and a `sounds` index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Button {
    /// Units per second the button travels in when pressed.
    pub speed: f32,
    /// Seconds before the button returns/can be pressed again.
    pub wait: f32,
    /// Hit points before a `func_button` with no `health` responds only to
    /// `use`; `0` means it only responds to `use`/touch, not damage.
    pub health: f32,
    /// Seconds between being pressed and firing `target`.
    pub delay: f32,
    /// `sounds` index into the built-in button sound table.
    pub sound: u8,
    /// Current animation state (buttons only ever travel forward and
    /// return, so [`MoverState::Opening`]/[`MoverState::Closing`] stand in
    /// for "pressing in" and "returning").
    pub state: MoverState,
    /// Seconds remaining in the current state.
    pub timer: f32,
}

/// `func_plat`: the same translating-mover keys as [`Door`], with an
/// optional explicit `height` overriding the bounding-box-derived travel
/// distance (the public-documented behaviour when a mapper wants a platform
/// to travel further or less than its own brush height).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    /// Units per second.
    pub speed: f32,
    /// Seconds the platform waits at the top before returning.
    pub wait: f32,
    /// Unit vector the platform travels along when activated (usually
    /// straight down, since platforms are triggered from their raised
    /// position in the common mapping pattern).
    pub movedir: Vec3,
    /// The distance travelled, from an explicit `height` keyvalue when
    /// present, else the bounding-box-derived distance minus `lip`.
    pub travel_distance: f32,
    /// `sounds` index into the built-in platform sound table.
    pub sounds: (u8, u8),
    /// Current animation state.
    pub state: MoverState,
    /// Seconds remaining in the current state's motion or wait.
    pub timer: f32,
}

/// `light`/`light_spot`/`light_environment`: brightness, colour, style and
/// (for spot/environment lights) an aim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    /// Brightness, the first number of the `_light`/`light` keyvalue.
    pub brightness: f32,
    /// Colour, the following three numbers of the same keyvalue, or white
    /// when only brightness was given.
    pub color: [u8; 3],
    /// Light style/animation index (`0` is the always-on default style).
    pub style: u8,
    /// `_cone` for `light_spot`, in degrees.
    pub cone: Option<f32>,
}

/// Marks the `info_player_start` entity; its pose lives in [`Transform`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerStart;

/// Marks an `info_landmark` entity, used together with [`ChangeLevel`] to
/// align the player across a level transition; its position lives in
/// [`Transform`] and its name in [`TargetName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landmark;

/// `trigger_changelevel`: the destination map and the shared landmark name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeLevel {
    /// The `map` keyvalue.
    pub map: String,
    /// The `landmark` keyvalue, matched against an [`Landmark`] entity's
    /// [`TargetName`] in the destination map.
    pub landmark: String,
}

/// `path_corner`/`path_track`: the next node's name, a pause, and (for
/// `func_tracktrain`) a `path_track`-only speed override and stop flag. See
/// `docs/FORMAT_SOURCES.md` ("Track trains and paths") for the public
/// sources these last two fields were taken from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Path {
    /// Seconds a follower waits at this node.
    pub wait: f32,
    /// `path_track`'s documented "New Train Speed": reassigns a
    /// `func_tracktrain`'s speed as it passes this node.
    pub speed: Option<f32>,
    /// The documented "Wait for retrigger" spawnflag: a `func_tracktrain`
    /// stops here until explicitly re-triggered, rather than continuing
    /// after `wait` seconds. See
    /// [`crate::track_train::path_stop_from_flags`].
    pub stop: bool,
}

/// `multi_manager`: every non-standard keyvalue is a `target -> delay`
/// pair (a `#N` suffix distinguishing repeated targets is stripped, per
/// the public documentation of how the entity is authored). Bounded to the
/// documented limit of 16 fan-out targets.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiManager {
    /// `(target, delay in seconds)`, in authored order.
    pub targets: Vec<(String, f32)>,
}

/// The documented cap on one `multi_manager`'s fan-out targets.
pub const MAX_MULTI_MANAGER_TARGETS: usize = 16;

/// `trigger_once`/`trigger_multiple` and any other `trigger_*` this crate
/// does not give a dedicated component to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trigger {
    /// `true` for `trigger_once` (fires at most once); `false` for
    /// `trigger_multiple` and other repeatable triggers.
    pub once: bool,
    /// Seconds before the trigger can fire again (ignored when `once`).
    pub wait: f32,
    /// Seconds between activation and firing `target`.
    pub delay: f32,
}

/// Any classname not otherwise recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unknown;

/// `worldspawn`'s map-wide keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Worldspawn {
    /// `skyname`.
    pub skyname: String,
    /// `wad`, split into individual package paths.
    pub wads: Vec<String>,
}

/// Standard GoldSrc `angle` sentinel meaning "straight up".
const ANGLE_UP: f32 = -1.0;
/// Standard GoldSrc `angle` sentinel meaning "straight down".
const ANGLE_DOWN: f32 = -2.0;

/// Converts a mover's `angles`/`angle` keyvalue into a unit movement
/// direction, honouring the special "straight up"/"straight down" sentinel
/// values (`-1`/`-2`) that Half-Life's editors present as an "Up"/"Down"
/// choice in place of a numeric angle; any other value is a yaw in degrees,
/// counter-clockwise around `+Z` from `+X`, matching `info_player_start`'s
/// convention.
#[must_use]
pub fn movedir_from_angles(angles: Vec3) -> Vec3 {
    let yaw = angles.y;
    if (yaw - ANGLE_UP).abs() < f32::EPSILON {
        Vec3::Z
    } else if (yaw - ANGLE_DOWN).abs() < f32::EPSILON {
        -Vec3::Z
    } else {
        let radians = yaw.to_radians();
        Vec3::new(radians.cos(), radians.sin(), 0.0)
    }
}

/// Clamps a float keyvalue field into `0..=255` for storage as a `u8`
/// (colour channels, style/sound indices), rounding towards zero the same
/// way GoldSrc's own integer keyvalue fields do.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_u8(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

fn parse_light_value(value: Option<&String>) -> (f32, [u8; 3]) {
    let Some(value) = value else {
        return (200.0, [255, 255, 255]);
    };
    let parts: Vec<f32> = value
        .split_ascii_whitespace()
        .filter_map(|part| part.parse::<f32>().ok())
        .collect();
    match parts.as_slice() {
        [brightness] => (*brightness, [255, 255, 255]),
        [r, g, b] => (200.0, [clamp_u8(*r), clamp_u8(*g), clamp_u8(*b)]),
        [r, g, b, brightness] => (*brightness, [clamp_u8(*r), clamp_u8(*g), clamp_u8(*b)]),
        _ => (200.0, [255, 255, 255]),
    }
}

fn strip_multi_manager_suffix(key: &str) -> &str {
    match key.rfind('#') {
        Some(index) => &key[..index],
        None => key,
    }
}

fn brush_travel_distance(
    def: &EntityDef,
    movedir: Vec3,
    lip: f32,
    model_bounds: &BTreeMap<u32, ([f32; 3], [f32; 3])>,
) -> f32 {
    let Some(ModelRef::Brush(index)) = &def.model else {
        return 0.0;
    };
    let Some((mins, maxs)) = model_bounds.get(index) else {
        return 0.0;
    };
    let size = Vec3::new(maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]);
    let projected = size.x * movedir.x.abs() + size.y * movedir.y.abs() + size.z * movedir.z.abs();
    (projected - lip).max(0.0)
}

/// The entity registry: a [`hecs::World`] plus a bounded name index.
pub struct Registry {
    /// The entity-component world.
    pub world: World,
    name_index: BTreeMap<String, Vec<Entity>>,
    /// `worldspawn`'s keys, when the map had a `worldspawn` entity.
    pub worldspawn: Option<Worldspawn>,
    /// The `entity index within `defs`, matching hecs `entity` -> the
    /// spawn order, kept so callers can align with brush-model indices.
    pub entities: Vec<Entity>,
}

impl Registry {
    /// Builds a registry from parsed entity definitions.
    ///
    /// `model_bounds` maps a brush submodel index (`ohl_formats::bsp30`'s
    /// `Model` slot, the same index a `*N` `model` keyvalue names) to its
    /// `(mins, maxs)` bounding box, used to derive door/platform travel
    /// distances; pass an empty map when that data is unavailable (travel
    /// distance then falls back to `0`, a safe default that trigger and
    /// timing logic can still exercise).
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn build(
        defs: &[EntityDef],
        model_bounds: &BTreeMap<u32, ([f32; 3], [f32; 3])>,
        limits: &Limits,
    ) -> Self {
        let mut world = World::new();
        let mut name_index: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
        let mut worldspawn = None;
        let mut entities = Vec::with_capacity(defs.len());

        for def in defs {
            let transform = Transform {
                origin: Vec3::from_array(def.origin),
                angles: Vec3::from_array(def.angles),
            };
            let entity = world.spawn((
                ClassName(def.classname.clone()),
                transform,
                SpawnFlags(def.spawnflags),
                def.render,
            ));
            entities.push(entity);

            if let Some(name) = &def.targetname {
                world.insert_one(entity, TargetName(name.clone())).ok();
                let bucket = name_index.entry(name.clone()).or_default();
                if bucket.len() < MAX_ENTITIES_PER_NAME {
                    bucket.push(entity);
                }
            }
            if let Some(target) = &def.target {
                world.insert_one(entity, Target(target.clone())).ok();
            }
            if let Some(ModelRef::Brush(index)) = &def.model {
                world.insert_one(entity, BrushModel(*index)).ok();
                if let Some((mins, maxs)) = model_bounds.get(index) {
                    let center = Vec3::new(
                        f32::midpoint(mins[0], maxs[0]),
                        f32::midpoint(mins[1], maxs[1]),
                        f32::midpoint(mins[2], maxs[2]),
                    );
                    world.insert_one(entity, BrushCenter(center)).ok();
                }
            }

            match def.classname.as_str() {
                "worldspawn" => {
                    let wads = def
                        .keyvalues
                        .get("wad")
                        .map(|value| keyvalues::parse_wad_list(value, limits))
                        .unwrap_or_default();
                    worldspawn = Some(Worldspawn {
                        skyname: def.keyvalues.get("skyname").cloned().unwrap_or_default(),
                        wads,
                    });
                }
                "func_door" | "func_door_rotating" => {
                    let lip = def
                        .keyvalues
                        .get("lip")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let movedir = movedir_from_angles(transform.angles);
                    let travel = brush_travel_distance(def, movedir, lip, model_bounds);
                    let door = Door {
                        speed: numeric(def, "speed", 100.0),
                        wait: numeric(def, "wait", 4.0),
                        lip,
                        movedir,
                        dmg: numeric(def, "dmg", 0.0),
                        health: numeric(def, "health", 0.0),
                        delay: numeric(def, "delay", 0.0),
                        sounds: (
                            clamp_u8(numeric(def, "movesnd", 0.0)),
                            clamp_u8(numeric(def, "stopsnd", 0.0)),
                        ),
                        travel_distance: travel,
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, door).ok();
                }
                "func_button" => {
                    let button = Button {
                        speed: numeric(def, "speed", 40.0),
                        wait: numeric(def, "wait", 1.0),
                        health: numeric(def, "health", 0.0),
                        delay: numeric(def, "delay", 0.0),
                        sound: clamp_u8(numeric(def, "sounds", 0.0)),
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, button).ok();
                }
                "func_plat" | "func_platform" => {
                    let lip = def
                        .keyvalues
                        .get("lip")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let movedir = def
                        .keyvalues
                        .get("angles")
                        .or_else(|| def.keyvalues.get("angle"))
                        .map_or(-Vec3::Z, |_| movedir_from_angles(transform.angles));
                    let explicit_height = def
                        .keyvalues
                        .get("height")
                        .and_then(|v| v.trim().parse::<f32>().ok());
                    let travel = explicit_height
                        .unwrap_or_else(|| brush_travel_distance(def, movedir, lip, model_bounds));
                    let platform = Platform {
                        speed: numeric(def, "speed", 150.0),
                        wait: numeric(def, "wait", 3.0),
                        movedir,
                        travel_distance: travel.max(0.0),
                        sounds: (
                            clamp_u8(numeric(def, "movesnd", 0.0)),
                            clamp_u8(numeric(def, "stopsnd", 0.0)),
                        ),
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, platform).ok();
                }
                "light" | "light_spot" | "light_environment" => {
                    let (brightness, color) = parse_light_value(
                        def.keyvalues
                            .get("_light")
                            .or_else(|| def.keyvalues.get("light")),
                    );
                    let light = Light {
                        brightness,
                        color,
                        style: clamp_u8(numeric(def, "style", 0.0)),
                        cone: def
                            .keyvalues
                            .get("_cone")
                            .and_then(|v| v.trim().parse::<f32>().ok()),
                    };
                    world.insert_one(entity, light).ok();
                }
                "info_player_start" => {
                    world.insert_one(entity, PlayerStart).ok();
                }
                "info_landmark" => {
                    world.insert_one(entity, Landmark).ok();
                }
                "trigger_changelevel" => {
                    let change = ChangeLevel {
                        map: def.keyvalues.get("map").cloned().unwrap_or_default(),
                        landmark: def.keyvalues.get("landmark").cloned().unwrap_or_default(),
                    };
                    world.insert_one(entity, change).ok();
                }
                "path_corner" | "path_track" => {
                    let path = Path {
                        wait: numeric(def, "wait", 0.0),
                        speed: def
                            .keyvalues
                            .get("speed")
                            .and_then(|v| v.trim().parse::<f32>().ok())
                            .filter(|v| v.is_finite()),
                        stop: crate::track_train::path_stop_from_flags(def.spawnflags),
                    };
                    world.insert_one(entity, path).ok();
                }
                "func_train" | "func_tracktrain" => {
                    let train = crate::track_train::TrackTrain {
                        turns_to_face: def.classname == "func_tracktrain",
                        speed: numeric(def, "speed", 100.0),
                        start_speed: numeric(def, "startspeed", 0.0),
                        height: numeric(def, "height", 4.0),
                        bank: numeric(def, "bank", 0.0),
                        dmg: numeric(def, "dmg", 0.0),
                        wheels: numeric(def, "wheels", 0.0),
                        no_user_control: crate::track_train::TrackTrain::no_user_control_from_flags(
                            def.spawnflags,
                        ),
                    };
                    world.insert_one(entity, train).ok();
                }
                "multi_manager" => {
                    let reserved = [
                        "classname",
                        "origin",
                        "angles",
                        "angle",
                        "targetname",
                        "target",
                        "spawnflags",
                        "model",
                        "rendermode",
                        "renderamt",
                        "rendercolor",
                    ];
                    let mut targets = Vec::new();
                    for (key, value) in &def.keyvalues {
                        if reserved.contains(&key.as_str()) {
                            continue;
                        }
                        if let Ok(delay) = value.trim().parse::<f32>() {
                            if targets.len() >= MAX_MULTI_MANAGER_TARGETS {
                                break;
                            }
                            targets.push((strip_multi_manager_suffix(key).to_string(), delay));
                        }
                    }
                    world.insert_one(entity, MultiManager { targets }).ok();
                }
                name if name.starts_with("trigger_") => {
                    let trigger = Trigger {
                        once: name == "trigger_once",
                        wait: numeric(def, "wait", 0.2),
                        delay: numeric(def, "delay", 0.0),
                    };
                    world.insert_one(entity, trigger).ok();
                }
                _ => {
                    world.insert_one(entity, Unknown).ok();
                }
            }
        }

        let mut registry = Self {
            world,
            name_index,
            worldspawn,
            entities,
        };
        // Second pass: a train's first `path_track`/`path_corner` node
        // commonly appears later in the entities lump than the train
        // itself, so the whole `targetname` index must exist first.
        crate::track_train::spawn_all(&mut registry);
        registry
    }

    /// Entities whose `targetname` is `name`, bounded to
    /// [`MAX_ENTITIES_PER_NAME`].
    #[must_use]
    pub fn find(&self, name: &str) -> &[Entity] {
        self.name_index
            .get(name)
            .map_or(&[] as &[Entity], Vec::as_slice)
    }
}

fn numeric(def: &EntityDef, key: &str, default: f32) -> f32 {
    def.keyvalues
        .get(key)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvalues::parse_entities;
    use ohl_formats::bsp30::Entity as RawEntity;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn builds_door_component_and_name_index() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("targetname", "door1"),
            ("angle", "90"),
            ("speed", "50"),
            ("wait", "2"),
            ("lip", "8"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let found = registry.find("door1");
        assert_eq!(found.len(), 1);
        let door = registry.world.get::<&Door>(found[0]).expect("door");
        assert!((door.speed - 50.0).abs() < f32::EPSILON);
        assert!((door.wait - 2.0).abs() < f32::EPSILON);
        assert!((door.lip - 8.0).abs() < f32::EPSILON);
        assert!((door.movedir - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn movedir_handles_up_and_down_sentinels() {
        assert_eq!(movedir_from_angles(Vec3::new(0.0, -1.0, 0.0)), Vec3::Z);
        assert_eq!(movedir_from_angles(Vec3::new(0.0, -2.0, 0.0)), -Vec3::Z);
    }

    #[test]
    fn multi_manager_collects_target_delay_pairs() {
        let entities = vec![raw(&[
            ("classname", "multi_manager"),
            ("targetname", "mm1"),
            ("light1", "0.0"),
            ("door1", "1.5"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("mm1")[0];
        let mm = registry.world.get::<&MultiManager>(entity).expect("mm");
        assert_eq!(mm.targets.len(), 2);
    }

    #[test]
    fn worldspawn_wad_list_is_parsed() {
        let entities = vec![raw(&[
            ("classname", "worldspawn"),
            ("skyname", "desert"),
            ("wad", "halflife.wad;xeno.wad"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let ws = registry.worldspawn.expect("worldspawn");
        assert_eq!(ws.skyname, "desert");
        assert_eq!(ws.wads, vec!["halflife.wad", "xeno.wad"]);
    }

    #[test]
    fn name_index_is_bounded_per_name() {
        let mut entities = Vec::new();
        for _ in 0..(MAX_ENTITIES_PER_NAME + 10) {
            entities.push(raw(&[("classname", "light"), ("targetname", "many")]));
        }
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        assert_eq!(registry.find("many").len(), MAX_ENTITIES_PER_NAME);
    }
}
