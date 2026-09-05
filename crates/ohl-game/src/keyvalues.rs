//! Typed, lenient parsing of BSP entities-lump keyvalues.
//!
//! [`ohl_formats::bsp30::entities::parse`] already turns the raw text block
//! into an ordered list of `{"key": "value"}` maps and bounds their sizes
//! against the caller-supplied [`ohl_formats::bsp30::Limits`]; this module
//! turns each map into a typed, bounded [`EntityDef`] without ever
//! panicking or rejecting an entity outright. Unknown or malformed values
//! fall back to a default instead of failing the whole entity, matching how
//! GoldSrc's own entity spawn functions tolerate missing or garbled
//! keyvalues (see `docs/FORMAT_SOURCES.md`).

use std::collections::BTreeMap;

use ohl_formats::bsp30::Entity;

/// Bounds applied while converting entity keyvalue maps into [`EntityDef`]s.
///
/// These are independent of, and in addition to, the limits already applied
/// by `ohl-formats` when it parsed the entities lump: they defend this
/// crate's own API against a hand-built [`Entity`] (as the property-based
/// tests construct) that did not go through that parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest number of entities converted from one entities lump.
    pub max_entities: usize,
    /// Largest number of keyvalues kept per entity; extra pairs are dropped
    /// deterministically (lowest keys first, since the source map is
    /// ordered) rather than causing an error.
    pub max_keys_per_entity: usize,
    /// Largest key length kept, in bytes; longer keys are dropped.
    pub max_key_bytes: usize,
    /// Largest value length kept, in bytes; longer values are truncated at
    /// a UTF-8 character boundary.
    pub max_value_bytes: usize,
    /// Largest number of `;`-separated entries kept from a `wad` keyvalue.
    pub max_wad_entries: usize,
    /// Largest length, in bytes, kept for one `wad` list entry.
    pub max_wad_entry_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entities: 8192,
            max_keys_per_entity: 64,
            max_key_bytes: 64,
            max_value_bytes: 512,
            max_wad_entries: 32,
            max_wad_entry_bytes: 260,
        }
    }
}

/// The value of an entity's `model` keyvalue: either a brush submodel
/// (`*N`, where `N` indexes `BSP::models`) or an external asset path
/// (studio/sprite models, or a raw `.bsp`-relative path for other cases).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRef {
    /// Brush submodel index, parsed from a leading `*`.
    Brush(u32),
    /// Any other model string, kept verbatim (bounded).
    Asset(String),
}

/// `rendermode`/`renderamt`/`rendercolor`, GoldSrc's render-fx keyvalues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RenderProps {
    /// `rendermode`; `0` (`kRenderNormal`) when absent or unparsable.
    pub mode: i32,
    /// `renderamt`; `0` when absent or unparsable.
    pub amt: i32,
    /// `rendercolor`, as `r g b` in `0..=255`; `[255, 255, 255]` when absent
    /// or unparsable.
    pub color: [u8; 3],
}

/// A typed, bounded view of one entity's keyvalues.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDef {
    /// `classname`; empty when absent.
    pub classname: String,
    /// Every keyvalue this entity carries, bounded by [`Limits`]. Includes
    /// the typed fields below too, so callers that only need one obscure
    /// key (e.g. a `multi_manager` target/delay pair) do not need a second
    /// accessor.
    pub keyvalues: BTreeMap<String, String>,
    /// `origin`, as `x y z`; `[0, 0, 0]` when absent or unparsable.
    pub origin: [f32; 3],
    /// `angles`, as `pitch yaw roll`; falls back to `[0, angle, 0]` when
    /// only the scalar `angle` key is present, and to `[0, 0, 0]` when
    /// neither is present or both are unparsable.
    pub angles: [f32; 3],
    /// `targetname`, when present and non-empty.
    pub targetname: Option<String>,
    /// `target`, when present and non-empty.
    pub target: Option<String>,
    /// `spawnflags`, as an unsigned bitmask; `0` when absent or unparsable.
    pub spawnflags: u32,
    /// `model`, when present.
    pub model: Option<ModelRef>,
    /// `rendermode`/`renderamt`/`rendercolor`.
    pub render: RenderProps,
}

fn truncate_bounded(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
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

fn parse_f32(value: &str) -> Option<f32> {
    let parsed = value.trim().parse::<f32>().ok()?;
    parsed.is_finite().then_some(parsed)
}

/// Parses `spawnflags`, reinterpreting a negative value's bits as the
/// unsigned bitmask GoldSrc itself uses (some tools export the field as a
/// signed integer).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_spawnflags(value: &str) -> u32 {
    let trimmed = value.trim();
    trimmed
        .parse::<i64>()
        .ok()
        .map(|signed| signed as u32)
        .or_else(|| trimmed.parse::<u32>().ok())
        .unwrap_or(0)
}

fn parse_render(keyvalues: &BTreeMap<String, String>) -> RenderProps {
    let mode = keyvalues
        .get("rendermode")
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0);
    let amt = keyvalues
        .get("renderamt")
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0);
    let color = keyvalues
        .get("rendercolor")
        .and_then(|value| {
            let mut parts = value.split_ascii_whitespace();
            let r = parts.next()?.parse::<f32>().ok()?;
            let g = parts.next()?.parse::<f32>().ok()?;
            let b = parts.next()?.parse::<f32>().ok()?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some([
                r.clamp(0.0, 255.0) as u8,
                g.clamp(0.0, 255.0) as u8,
                b.clamp(0.0, 255.0) as u8,
            ])
        })
        .unwrap_or([255, 255, 255]);
    RenderProps { mode, amt, color }
}

fn parse_model(value: &str, limits: &Limits) -> ModelRef {
    if let Some(rest) = value.strip_prefix('*')
        && let Ok(index) = rest.parse::<u32>()
    {
        return ModelRef::Brush(index);
    }
    ModelRef::Asset(truncate_bounded(value, limits.max_value_bytes))
}

/// Converts one raw entity map into an [`EntityDef`], never failing: every
/// field falls back to a default when its keyvalue is missing or
/// unparsable, and oversized keys, values or key counts are bounded rather
/// than rejected.
#[must_use]
pub fn parse_entity(entity: &Entity, limits: &Limits) -> EntityDef {
    let mut keyvalues = BTreeMap::new();
    for (key, value) in entity.iter().take(limits.max_keys_per_entity) {
        if key.is_empty() || key.len() > limits.max_key_bytes {
            continue;
        }
        keyvalues.insert(
            truncate_bounded(key, limits.max_key_bytes),
            truncate_bounded(value, limits.max_value_bytes),
        );
    }

    let classname = keyvalues.get("classname").cloned().unwrap_or_default();
    let origin = keyvalues
        .get("origin")
        .and_then(|value| parse_vec3(value))
        .unwrap_or([0.0, 0.0, 0.0]);
    let angles = if let Some(angles) = keyvalues.get("angles").and_then(|value| parse_vec3(value)) {
        angles
    } else {
        let yaw = keyvalues
            .get("angle")
            .and_then(|value| parse_f32(value))
            .unwrap_or(0.0);
        [0.0, yaw, 0.0]
    };
    let targetname = keyvalues
        .get("targetname")
        .filter(|value| !value.is_empty())
        .cloned();
    let target = keyvalues
        .get("target")
        .filter(|value| !value.is_empty())
        .cloned();
    let spawnflags = keyvalues
        .get("spawnflags")
        .map_or(0, |value| parse_spawnflags(value));
    let model = keyvalues
        .get("model")
        .map(|value| parse_model(value, limits));
    let render = parse_render(&keyvalues);

    EntityDef {
        classname,
        keyvalues,
        origin,
        angles,
        targetname,
        target,
        spawnflags,
        model,
        render,
    }
}

/// Converts every entity in `entities`, bounded to `limits.max_entities`.
#[must_use]
pub fn parse_entities(entities: &[Entity], limits: &Limits) -> Vec<EntityDef> {
    entities
        .iter()
        .take(limits.max_entities)
        .map(|entity| parse_entity(entity, limits))
        .collect()
}

/// Parses a `worldspawn` `wad` keyvalue (a `;`-separated list of WAD3
/// package paths, GoldSrc's own convention for locating external texture
/// packages) into a bounded list of non-empty entries, discarding a
/// trailing separator and any blank segments. Never panics and never logs
/// its input, since WAD paths are map-derived data.
#[must_use]
pub fn parse_wad_list(value: &str, limits: &Limits) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .take(limits.max_wad_entries)
        .map(|entry| truncate_bounded(entry, limits.max_wad_entry_bytes))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Limits, ModelRef, parse_entities, parse_entity, parse_wad_list};
    use ohl_formats::bsp30::Entity;
    use proptest::prelude::*;

    fn entity(pairs: &[(&str, &str)]) -> Entity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn parses_typed_fields() {
        let e = entity(&[
            ("classname", "func_door"),
            ("origin", "1 2 3"),
            ("angles", "0 90 0"),
            ("targetname", "door1"),
            ("target", "relay1"),
            ("spawnflags", "1"),
            ("model", "*3"),
            ("rendermode", "2"),
            ("renderamt", "128"),
            ("rendercolor", "255 0 0"),
        ]);
        let def = parse_entity(&e, &Limits::default());
        assert_eq!(def.classname, "func_door");
        assert_eq!(def.origin, [1.0, 2.0, 3.0]);
        assert_eq!(def.angles, [0.0, 90.0, 0.0]);
        assert_eq!(def.targetname.as_deref(), Some("door1"));
        assert_eq!(def.target.as_deref(), Some("relay1"));
        assert_eq!(def.spawnflags, 1);
        assert_eq!(def.model, Some(ModelRef::Brush(3)));
        assert_eq!(def.render.mode, 2);
        assert_eq!(def.render.amt, 128);
        assert_eq!(def.render.color, [255, 0, 0]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn scalar_angle_falls_back_into_angles() {
        let e = entity(&[("classname", "func_button"), ("angle", "180")]);
        let def = parse_entity(&e, &Limits::default());
        assert_eq!(def.angles, [0.0, 180.0, 0.0]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn missing_and_malformed_fields_default_cleanly() {
        let e = entity(&[
            ("classname", "worldspawn"),
            ("origin", "not a vector"),
            ("spawnflags", "not a number"),
        ]);
        let def = parse_entity(&e, &Limits::default());
        assert_eq!(def.origin, [0.0, 0.0, 0.0]);
        assert_eq!(def.spawnflags, 0);
        assert!(def.targetname.is_none());
    }

    #[test]
    fn asset_model_path_is_kept_verbatim() {
        let e = entity(&[("model", "models/foo.mdl")]);
        let def = parse_entity(&e, &Limits::default());
        assert_eq!(
            def.model,
            Some(ModelRef::Asset("models/foo.mdl".to_string()))
        );
    }

    #[test]
    fn oversized_key_count_is_bounded_not_rejected() {
        let limits = Limits {
            max_keys_per_entity: 2,
            ..Limits::default()
        };
        let e = entity(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let def = parse_entity(&e, &limits);
        assert!(def.keyvalues.len() <= 2);
    }

    #[test]
    fn wad_list_splits_and_trims() {
        let entries = parse_wad_list(" halflife.wad ; xeno.wad ;;", &Limits::default());
        assert_eq!(
            entries,
            vec!["halflife.wad".to_string(), "xeno.wad".to_string()]
        );
    }

    proptest! {
        #[test]
        fn parse_entity_never_panics(
            pairs in prop::collection::vec(
                (".{0,80}", ".{0,300}"),
                0..40,
            )
        ) {
            let e: Entity = pairs.into_iter().collect();
            let _ = parse_entity(&e, &Limits::default());
        }

        #[test]
        fn parse_entities_never_panics(
            batches in prop::collection::vec(
                prop::collection::vec((".{0,20}", ".{0,50}"), 0..10),
                0..20,
            )
        ) {
            let entities: Vec<Entity> = batches
                .into_iter()
                .map(|pairs| pairs.into_iter().collect())
                .collect();
            let _ = parse_entities(&entities, &Limits::default());
        }

        /// Arbitrary values (including quotes, backslashes and other bytes
        /// the entities-lump text format has no escape syntax for) survive
        /// unchanged through `EntityDef::keyvalues`, since this crate never
        /// re-encodes them: the round trip is simply "stored equals read".
        #[test]
        fn keyvalue_values_round_trip_when_within_bounds(
            key in "[a-zA-Z_][a-zA-Z0-9_]{0,20}",
            value in ".{0,64}",
        ) {
            let e = entity(&[(key.as_str(), value.as_str())]);
            let def = parse_entity(&e, &Limits::default());
            prop_assert_eq!(def.keyvalues.get(&key), Some(&value));
        }

        #[test]
        fn wad_list_never_panics(value in ".{0,500}") {
            let _ = parse_wad_list(&value, &Limits::default());
        }

        /// Joining a list of simple, semicolon-free, non-blank entries and
        /// re-parsing it recovers the same list.
        #[test]
        fn wad_list_round_trips_simple_entries(
            entries in prop::collection::vec("[a-zA-Z0-9_./]{1,32}", 0..10)
        ) {
            let joined = entries.join(";");
            let parsed = parse_wad_list(&joined, &Limits::default());
            prop_assert_eq!(parsed, entries);
        }
    }
}
