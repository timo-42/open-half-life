//! The bounded list of transient sprites: muzzle flashes, impact puffs and
//! explosion flashes.
//!
//! These are not map-placed [`crate::level::SpritePlacement`]s; they are
//! spawned by [`crate::projectiles`] from a [`ohl_combat::projectile::ProjectileEvent`]
//! or a [`ohl_combat::deployables::DeployableEvent`] and aged out over a few
//! seconds. [`Renderers::draw_sprites`](crate::render) appends them to the
//! same [`ohl_render::SpriteInstance`] list the map's own sprite entities
//! draw with, reusing the sprite asset the level already loaded rather than
//! naming a new one (see the module doc on [`TransientSprite::asset`]).
//!
//! Bounded at [`MAX_TRANSIENT_SPRITES`]: a burst of impacts drops the
//! oldest sprite and counts it, the same "drop and count" policy the rest
//! of the project uses for a crowded tick. Nothing here logs; the count is
//! returned as data.

use ohl_render::RenderProps;

/// The most transient sprites drawn at once. A project-chosen bound, not an
/// observed value.
pub const MAX_TRANSIENT_SPRITES: usize = 64;

/// One live transient sprite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransientSprite {
    /// Index into [`crate::level::Level::sprite_assets`]. A transient
    /// sprite has no asset of its own to load — this package cannot add a
    /// new asset-loading path to [`crate::level::Level`] without touching
    /// files outside its scope — so it reuses whichever sprite asset the
    /// map itself already published. A map with no `env_sprite`-family
    /// entities therefore has no transient sprite to draw with; that is a
    /// supported (if visually quiet) state, not an error.
    ///
    /// TODO(black-box): which sprite a given impact surface or projectile
    /// kind should draw with is not derived here; the caller picks the
    /// index.
    pub asset: usize,
    /// World-space centre.
    pub origin: [f32; 3],
    /// Uniform scale, matching [`ohl_render::SpriteInstance::scale`].
    ///
    /// TODO(black-box): muzzle-flash and impact-sprite scale are not
    /// published.
    pub scale: f32,
    /// Blend/brightness parameters for the draw.
    pub render: RenderProps,
    /// Seconds remaining before the sprite is dropped.
    ///
    /// TODO(black-box): muzzle-flash and impact-sprite duration are not
    /// published.
    pub seconds_left: f32,
    /// Seconds since the sprite was spawned, driving its animation frame
    /// the same way [`crate::level::Level::sprites`] uses `elapsed`.
    pub age: f32,
}

/// A bounded, oldest-drops-first list of [`TransientSprite`]s.
#[derive(Debug, Clone, Default)]
pub(crate) struct TransientSprites {
    items: Vec<TransientSprite>,
    dropped: usize,
}

impl TransientSprites {
    /// An empty list.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Adds one sprite, dropping the oldest live one first when the list is
    /// already at [`MAX_TRANSIENT_SPRITES`].
    pub(crate) fn push(&mut self, sprite: TransientSprite) {
        if self.items.len() >= MAX_TRANSIENT_SPRITES {
            self.items.remove(0);
            self.dropped += 1;
        }
        self.items.push(sprite);
    }

    /// Ages every sprite by `dt` seconds, dropping the ones that expire.
    /// Non-finite or non-positive `dt` ages nothing.
    pub(crate) fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        for sprite in &mut self.items {
            sprite.age += dt;
            sprite.seconds_left -= dt;
        }
        self.items.retain(|sprite| sprite.seconds_left > 0.0);
    }

    /// The sprites currently live, oldest first.
    #[allow(dead_code)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &TransientSprite> {
        self.items.iter()
    }

    /// The live sprites as a slice, for [`crate::render`].
    pub(crate) fn as_slice(&self) -> &[TransientSprite] {
        &self.items
    }

    /// How many live sprites there are.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// How many sprites have ever been dropped for being past the cap.
    /// Data, never logged.
    #[allow(dead_code)]
    pub(crate) fn dropped_count(&self) -> usize {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use ohl_render::RenderProps;

    use super::{MAX_TRANSIENT_SPRITES, TransientSprite, TransientSprites};

    fn sprite(seconds_left: f32) -> TransientSprite {
        TransientSprite {
            asset: 0,
            origin: [0.0, 0.0, 0.0],
            scale: 1.0,
            render: RenderProps::from_entity(0, 0, [255, 255, 255], 0),
            seconds_left,
            age: 0.0,
        }
    }

    #[test]
    fn the_cap_drops_the_oldest_and_counts_it() {
        let mut sprites = TransientSprites::new();
        for _ in 0..MAX_TRANSIENT_SPRITES {
            sprites.push(sprite(5.0));
        }
        assert_eq!(sprites.len(), MAX_TRANSIENT_SPRITES);
        assert_eq!(sprites.dropped_count(), 0);

        sprites.push(sprite(5.0));
        assert_eq!(sprites.len(), MAX_TRANSIENT_SPRITES);
        assert_eq!(sprites.dropped_count(), 1);
    }

    #[test]
    fn aging_removes_expired_sprites_only() {
        let mut sprites = TransientSprites::new();
        sprites.push(sprite(0.5));
        sprites.push(sprite(5.0));
        sprites.tick(1.0);
        assert_eq!(sprites.len(), 1);
        assert!((sprites.iter().next().unwrap().age - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn non_finite_or_non_positive_dt_ages_nothing() {
        let mut sprites = TransientSprites::new();
        sprites.push(sprite(1.0));
        sprites.tick(f32::NAN);
        sprites.tick(-1.0);
        sprites.tick(0.0);
        assert_eq!(sprites.len(), 1);
        assert!((sprites.iter().next().unwrap().age - 0.0).abs() < f32::EPSILON);
    }
}
