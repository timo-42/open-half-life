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

/// Finds the first `info_player_start` and reads its origin and facing.
///
/// GoldSrc entities carry facing either as a scalar `angle` (the legacy yaw
/// key; see the Valve Developer Community `angle` page) or as an `angles`
/// triple of `pitch yaw roll` (see the VDC `angles` page). Editors of that
/// era routinely emit both keys, with `angles` left at its default
/// `"0 0 0"` and the real facing only in `angle`. Per the TWHL/VDC keyvalue
/// docs, when both are present the non-zero/more specific value wins, so
/// this reads `angles` only when it parses *and* at least one component is
/// non-zero; otherwise it falls back to `angle`, and finally to the
/// zero-yaw, zero-pitch default.
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
        None => (
            0.0,
            entity
                .get("angle")
                .and_then(|value| parse_angle(value))
                .unwrap_or(0.0),
        ),
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
}
