//! Mapping between `hecs` entities and `ohl-combat`'s opaque
//! [`ohl_combat::EntityId`].
//!
//! `ohl-combat` deliberately has no ECS: it takes whatever stable number the
//! caller calls an entity. This crate drives a [`ohl_game::hecs::World`], so
//! the number is the entity's own bit pattern — generation included, so a
//! recycled slot never resolves to the entity that used to live in it.
//!
//! The mapping is stable within one loaded level. A save records entities by
//! their index in `ohl_game::Registry::entities`, never by these bits, so a
//! reloaded level rebuilds the mapping deterministically.

use ohl_combat::EntityId;
use ohl_game::hecs::Entity;

/// The combat-side id of `entity`.
#[must_use]
pub fn entity_id(entity: Entity) -> EntityId {
    EntityId(entity.to_bits().get())
}

/// The entity `id` names, or `None` when `id` is not a bit pattern any
/// entity ever had (`0`, or a malformed value out of a corrupt save).
#[must_use]
pub fn entity_of(id: EntityId) -> Option<Entity> {
    Entity::from_bits(id.0)
}

#[cfg(test)]
mod tests {
    use ohl_combat::EntityId;
    use ohl_game::hecs::World;

    use super::{entity_id, entity_of};

    #[test]
    fn a_spawned_entity_round_trips() {
        let mut world = World::new();
        let entity = world.spawn((7u32,));
        assert_eq!(entity_of(entity_id(entity)), Some(entity));
    }

    #[test]
    fn distinct_entities_get_distinct_ids() {
        let mut world = World::new();
        let first = world.spawn((1u32,));
        let second = world.spawn((2u32,));
        assert_ne!(entity_id(first), entity_id(second));
    }

    #[test]
    fn a_despawned_entity_still_round_trips_to_itself() {
        // The bits stay decodable; what they no longer do is resolve to a
        // live entity, which the world reports rather than this mapping.
        let mut world = World::new();
        let entity = world.spawn((3u32,));
        let id = entity_id(entity);
        world.despawn(entity).expect("the entity was alive");
        assert_eq!(entity_of(id), Some(entity));
        assert!(!world.contains(entity));
    }

    #[test]
    fn a_recycled_slot_does_not_reuse_the_old_id() {
        let mut world = World::new();
        let first = world.spawn((4u32,));
        let first_id = entity_id(first);
        world.despawn(first).expect("the entity was alive");
        let second = world.spawn((5u32,));
        assert_ne!(
            first_id,
            entity_id(second),
            "the generation counter keeps a recycled slot distinct"
        );
    }

    #[test]
    fn a_zero_id_names_no_entity() {
        assert_eq!(entity_of(EntityId(0)), None);
    }
}
