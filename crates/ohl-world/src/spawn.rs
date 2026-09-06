//! The player start read from the entities lump.

use ohl_formats::bsp30::Entity;

/// A `info_player_start` position and facing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerSpawn {
    /// Origin in GoldSrc world units.
    pub origin: [f32; 3],
    /// Yaw in degrees, counter-clockwise around +Z from +X.
    pub yaw: f32,
    /// Pitch in degrees; positive looks down, matching GoldSrc's convention.
    pub pitch: f32,
}

impl Default for PlayerSpawn {
    fn default() -> Self {
        Self {
            origin: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

fn parse_vec3(value: &str) -> Option<[f32; 3]> {
    let mut parts = value.split_ascii_whitespace();
    let mut out = [0.0f32; 3];
    for slot in &mut out {
        *slot = parts.next()?.parse::<f32>().ok()?;
        if !slot.is_finite() {
            return None;
        }
    }
    Some(out)
}

fn parse_angle(value: &str) -> Option<f32> {
    let angle = value.trim().parse::<f32>().ok()?;
    angle.is_finite().then_some(angle)
}

/// The documented `angle` sentinel meaning "face straight up" (see the
/// "Player spawn facing" citation in `docs/FORMAT_SOURCES.md`).
const ANGLE_UP: f32 = -1.0;

/// The documented `angle` sentinel meaning "face straight down".
const ANGLE_DOWN: f32 = -2.0;

/// Resolves a raw scalar `angle` keyvalue into `(pitch, yaw)`, honouring
/// the two documented sentinels: `-1` (straight up) and `-2` (straight
/// down) each override the field to a fixed pitch with no meaningful yaw
/// (recorded as `0`, since the public docs describing these sentinels do
/// not assign one); any other value is an ordinary yaw in degrees at
/// pitch `0`, matching every other `info_player_start` in this crate's
/// tests. See `docs/FORMAT_SOURCES.md`, "Player spawn facing".
#[allow(clippy::float_cmp)]
fn pitch_yaw_from_scalar_angle(angle: f32) -> (f32, f32) {
    if angle == ANGLE_UP {
        (-90.0, 0.0)
    } else if angle == ANGLE_DOWN {
        (90.0, 0.0)
    } else {
        (0.0, angle)
    }
}

/// Finds the player start and reads its origin and facing.
///
/// GoldSrc entities carry facing either as a scalar `angle` (the legacy yaw
/// key; see the Valve Developer Community `angle` page) or as an `angles`
/// triple of `pitch yaw roll` (see the VDC `angles` page). Editors of that
/// era routinely emit both keys, with `angles` left at its default
/// `"0 0 0"` and the real facing only in `angle`. Per the TWHL/VDC keyvalue
/// docs, when both are present the non-zero/more specific value wins, so
/// this reads `angles` only when it parses *and* at least one component is
/// non-zero; otherwise it falls back to `angle` (resolving its `-1`/`-2`
/// "face up"/"face down" sentinels through [`pitch_yaw_from_scalar_angle`]),
/// and finally to the zero-yaw, zero-pitch default.
///
/// When a map places more than one `info_player_start`, the first one in
/// entity-lump order is used. Per TWHL's coverage of multiple player
/// starts, the engine is documented to start the player at the first
/// `info_player_start` it encounters (all other placed starts are only
/// used by mods/logic that explicitly relocate the player, e.g. team
/// spawn selection, which this project does not implement).
/// TODO(black-box): no primary Valve source was fetchable to confirm the
/// exact tie-break beyond "first in the entity lump" (the public pages
/// describing this returned HTTP 403 to automated fetches from this
/// environment, the same failure mode already recorded elsewhere in
/// `docs/FORMAT_SOURCES.md`); this is a defensible, documented choice,
/// not a confirmed engine constant.
#[must_use]
pub fn find_player_start(entities: &[Entity]) -> Option<PlayerSpawn> {
    let entity = entities
        .iter()
        .find(|entity| entity.get("classname").map(String::as_str) == Some("info_player_start"))?;
    let origin = entity
        .get("origin")
        .and_then(|value| parse_vec3(value))
        .unwrap_or([0.0, 0.0, 0.0]);
    let angles = entity
        .get("angles")
        .and_then(|value| parse_vec3(value))
        .filter(|angles| angles.iter().any(|component| *component != 0.0));
    let (pitch, yaw) = match angles {
        Some(angles) => (angles[0], angles[1]),
        None => entity
            .get("angle")
            .and_then(|value| parse_angle(value))
            .map_or((0.0, 0.0), pitch_yaw_from_scalar_angle),
    };
    Some(PlayerSpawn { origin, yaw, pitch })
}

#[cfg(test)]
mod tests {
    use super::find_player_start;
    use ohl_formats::bsp30::Entity;

    fn entity(pairs: &[(&str, &str)]) -> Entity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn reads_origin_and_scalar_angle() {
        let entities = vec![
            entity(&[("classname", "worldspawn")]),
            entity(&[
                ("classname", "info_player_start"),
                ("origin", "16 -32 64"),
                ("angle", "90"),
            ]),
        ];
        let spawn = find_player_start(&entities).expect("found");
        assert_eq!(spawn.origin, [16.0, -32.0, 64.0]);
        assert!((spawn.yaw - 90.0).abs() < f32::EPSILON);
        assert!(spawn.pitch.abs() < f32::EPSILON);
    }

    #[test]
    fn angles_triple_takes_precedence() {
        let entities = vec![entity(&[
            ("classname", "info_player_start"),
            ("angle", "90"),
            ("angles", "10 20 30"),
        ])];
        let spawn = find_player_start(&entities).expect("found");
        assert!((spawn.pitch - 10.0).abs() < f32::EPSILON);
        assert!((spawn.yaw - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn zero_angles_triple_defers_to_scalar_angle() {
        // Editors commonly emit both keys with `angles` left at its default
        // "0 0 0" and the real facing only in `angle`; the zero triple must
        // not silently override the real yaw.
        let entities = vec![entity(&[
            ("classname", "info_player_start"),
            ("origin", "0 0 0"),
            ("angle", "90"),
            ("angles", "0 0 0"),
        ])];
        let spawn = find_player_start(&entities).expect("found");
        assert!((spawn.yaw - 90.0).abs() < f32::EPSILON);
        assert!(spawn.pitch.abs() < f32::EPSILON);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn missing_or_malformed_fields_fall_back() {
        assert!(find_player_start(&[]).is_none());
        let entities = vec![entity(&[
            ("classname", "info_player_start"),
            ("origin", "not a vector"),
        ])];
        let spawn = find_player_start(&entities).expect("found");
        assert_eq!(spawn.origin, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn angle_sentinel_minus_one_faces_straight_up() {
        let entities = vec![entity(&[
            ("classname", "info_player_start"),
            ("angle", "-1"),
        ])];
        let spawn = find_player_start(&entities).expect("found");
        assert!((spawn.pitch - -90.0).abs() < f32::EPSILON);
        assert!(spawn.yaw.abs() < f32::EPSILON);
    }

    #[test]
    fn angle_sentinel_minus_two_faces_straight_down() {
        let entities = vec![entity(&[
            ("classname", "info_player_start"),
            ("angle", "-2"),
        ])];
        let spawn = find_player_start(&entities).expect("found");
        assert!((spawn.pitch - 90.0).abs() < f32::EPSILON);
        assert!(spawn.yaw.abs() < f32::EPSILON);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn ordinary_negative_yaw_below_the_sentinels_is_not_mistaken_for_one() {
        // -3 is an ordinary (if unusual) yaw, not a documented sentinel;
        // it must be read as a literal angle, not fall through to the
        // up/down special cases.
        let entities = vec![entity(&[
            ("classname", "info_player_start"),
            ("angle", "-3"),
        ])];
        let spawn = find_player_start(&entities).expect("found");
        assert_eq!(spawn.yaw, -3.0);
        assert_eq!(spawn.pitch, 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn first_info_player_start_in_lump_order_wins() {
        // TODO(black-box): see `find_player_start`'s doc comment — this
        // pins "first in entity-lump order" as the tested contract.
        let entities = vec![
            entity(&[("classname", "worldspawn")]),
            entity(&[
                ("classname", "info_player_start"),
                ("origin", "1 2 3"),
                ("angle", "45"),
            ]),
            entity(&[
                ("classname", "info_player_start"),
                ("origin", "100 200 300"),
                ("angle", "180"),
            ]),
        ];
        let spawn = find_player_start(&entities).expect("found");
        assert_eq!(spawn.origin, [1.0, 2.0, 3.0]);
        assert!((spawn.yaw - 45.0).abs() < f32::EPSILON);
    }
}
