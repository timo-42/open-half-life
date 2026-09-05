//! Channel classes and the fixed-capacity slot/replacement model.
//!
//! GoldSrc-family engines group every playing sound into a small, fixed set
//! of channel classes (documented across public engine/modding references
//! as `CHAN_AUTO`, `CHAN_WEAPON`, `CHAN_VOICE`, `CHAN_ITEM`, `CHAN_BODY`,
//! `CHAN_STREAM`, `CHAN_STATIC`), each with its own limited pool of
//! simultaneous voices. This module implements a self-contained version of
//! that model: each class gets a fixed capacity below, and starting a sound
//! that would exceed a class's capacity evicts that class's oldest active
//! channel, while starting a sound on the same `(entity, class)` pair that
//! already has an active channel replaces it outright (the documented
//! "restart" behavior used for things like a weapon's fire loop or an
//! NPC's single voice line).

use crate::mixer::spatial::SoundSpatial;
use std::sync::Arc;

/// A decoded sound's PCM data, ready to be played back by the mixer.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundBuffer {
    pub channels: u16,
    pub sample_rate: u32,
    /// Interleaved samples, `frame_count() * channels` long.
    pub samples: Arc<[f32]>,
    /// Loop range in frames (`start`, exclusive `end`), if the sound loops.
    pub loop_range: Option<(u32, u32)>,
}

impl SoundBuffer {
    #[must_use]
    pub fn frame_count(&self) -> u32 {
        if self.channels == 0 {
            0
        } else {
            u32::try_from(self.samples.len())
                .unwrap_or(u32::MAX)
                .wrapping_div(u32::from(self.channels))
        }
    }
}

/// GoldSrc-style channel classes, each with a fixed voice-pool capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChannelClass {
    Auto,
    Weapon,
    Voice,
    Item,
    Body,
    Stream,
    Static,
}

impl ChannelClass {
    /// The fixed number of simultaneous voices this class allows before its
    /// oldest channel is evicted to make room for a new one.
    #[must_use]
    pub const fn capacity(self) -> usize {
        match self {
            Self::Auto => 8,
            Self::Weapon | Self::Voice | Self::Item | Self::Body => 4,
            Self::Stream => 1,
            Self::Static => 64,
        }
    }
}

/// A request to start a new sound.
#[derive(Debug, Clone)]
pub struct PlayRequest {
    /// The entity/owner this sound is associated with. Combined with
    /// `class`, this is the replacement key: a second request with the same
    /// `(entity, class)` restarts (replaces) the first.
    pub entity: u32,
    pub class: ChannelClass,
    pub buffer: Arc<SoundBuffer>,
    pub volume: f32,
    /// Playback rate multiplier: `1.0` is unmodified pitch/speed.
    pub pitch: f32,
    /// `None` plays the sound non-positionally (full volume, no pan
    /// reduction; used for UI sounds and stereo music).
    pub spatial: Option<SoundSpatial>,
}

/// One currently playing sound.
pub(crate) struct ActiveChannel {
    pub entity: u32,
    pub class: ChannelClass,
    pub buffer: Arc<SoundBuffer>,
    pub volume: f32,
    pub pitch: f32,
    pub spatial: Option<SoundSpatial>,
    /// Fractional playback position, in source frames.
    pub position: f64,
    /// Monotonic start order, used to find the oldest channel in a class.
    pub start_order: u64,
    pub finished: bool,
}

impl ActiveChannel {
    pub(crate) fn new(request: PlayRequest, start_order: u64) -> Self {
        Self {
            entity: request.entity,
            class: request.class,
            buffer: request.buffer,
            volume: request.volume,
            pitch: request.pitch,
            spatial: request.spatial,
            position: 0.0,
            start_order,
            finished: false,
        }
    }
}

/// Finds the slot to evict (if any) before inserting a channel for
/// `(entity, class)`, applying the replacement rules documented above.
/// Returns the index in `channels` to remove, or `None` when the new
/// channel can simply be appended.
pub(crate) fn slot_to_replace(
    channels: &[ActiveChannel],
    entity: u32,
    class: ChannelClass,
) -> Option<usize> {
    if let Some(index) = channels
        .iter()
        .position(|channel| channel.entity == entity && channel.class == class)
    {
        return Some(index);
    }

    let same_class_count = channels
        .iter()
        .filter(|channel| channel.class == class)
        .count();
    if same_class_count < class.capacity() {
        return None;
    }

    channels
        .iter()
        .enumerate()
        .filter(|(_, channel)| channel.class == class)
        .min_by_key(|(_, channel)| channel.start_order)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_buffer() -> Arc<SoundBuffer> {
        Arc::new(SoundBuffer {
            channels: 1,
            sample_rate: 8_000,
            samples: Arc::from(vec![0.0f32; 8]),
            loop_range: None,
        })
    }

    fn dummy_channel(entity: u32, class: ChannelClass, start_order: u64) -> ActiveChannel {
        ActiveChannel::new(
            PlayRequest {
                entity,
                class,
                buffer: dummy_buffer(),
                volume: 1.0,
                pitch: 1.0,
                spatial: None,
            },
            start_order,
        )
    }

    #[test]
    fn same_entity_and_class_replaces_in_place() {
        let channels = vec![dummy_channel(1, ChannelClass::Voice, 0)];
        assert_eq!(slot_to_replace(&channels, 1, ChannelClass::Voice), Some(0));
    }

    #[test]
    fn different_entity_appends_until_capacity() {
        let channels = vec![dummy_channel(1, ChannelClass::Weapon, 0)];
        assert_eq!(slot_to_replace(&channels, 2, ChannelClass::Weapon), None);
    }

    #[test]
    fn class_at_capacity_evicts_oldest() {
        let channels: Vec<_> = (0..ChannelClass::Weapon.capacity())
            .map(|index| {
                let index = u32::try_from(index).expect("small test index");
                dummy_channel(index + 10, ChannelClass::Weapon, u64::from(index))
            })
            .collect();
        // Oldest is start_order 0, at index 0.
        assert_eq!(
            slot_to_replace(&channels, 999, ChannelClass::Weapon),
            Some(0)
        );
    }

    #[test]
    fn stream_class_always_replaces_the_single_slot() {
        let channels = vec![dummy_channel(1, ChannelClass::Stream, 0)];
        assert_eq!(slot_to_replace(&channels, 2, ChannelClass::Stream), Some(0));
    }
}
