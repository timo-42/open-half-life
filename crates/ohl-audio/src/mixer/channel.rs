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

/// The published `VOL_NORM`: a sound played at its authored loudness.
///
/// This and [`PITCH_NORM`] are the GoldSrc sound constants published in the
/// [AMX Mod X scripting API reference](https://www.amxmodx.org/api/amxconst)
/// (`amxconst`), recorded in `docs/FORMAT_SOURCES.md` under "Sound
/// playback".
pub const VOL_NORM: f32 = 1.0;

/// The published `PITCH_NORM`, `100`: the integer a map's `pitch` keyvalue
/// carries for unmodified playback speed. Divide a published `pitch` by
/// this to get the [`PlayRequest::pitch`] multiplier. See [`VOL_NORM`].
pub const PITCH_NORM: f32 = 100.0;

/// The most frames [`SoundBuffer::concatenate`] will produce, so a
/// pathological sentence (a `sentences.txt` entry naming hundreds of long
/// words) cannot allocate without bound. Project-owned, sized at roughly
/// ten minutes of 44.1 kHz audio.
pub const MAX_CONCATENATED_FRAMES: u32 = 26_460_000;

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

    /// How many bytes of sample data this buffer holds, for a cache that
    /// bounds itself by size rather than by entry count.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.samples.len() * core::mem::size_of::<f32>()
    }

    /// Wraps one [`crate::wav::DecodedWav`] as a playable buffer, carrying
    /// its [`crate::wav::DecodedWav::effective_loop`] across.
    #[must_use]
    pub fn from_decoded(decoded: &crate::wav::DecodedWav) -> Self {
        Self {
            channels: decoded.format.channels,
            sample_rate: decoded.format.sample_rate,
            loop_range: decoded.effective_loop(),
            samples: Arc::from(decoded.samples.clone()),
        }
    }

    /// Joins `parts` end to end into one non-looping buffer, so a sentence
    /// (several word samples "strung together back to back", the published
    /// `sentences.txt` behaviour) plays as a single channel rather than as
    /// several simultaneous ones.
    ///
    /// The result takes the first part's sample rate and the widest channel
    /// count in `parts`; every other part is linearly resampled to that
    /// rate and mono/stereo-matched, the same interpolation
    /// [`crate::mixer::Mixer::render`] itself uses. Returns `None` for an
    /// empty `parts`, for a first part with no frames, or once the joined
    /// length would exceed [`MAX_CONCATENATED_FRAMES`]. Never panics.
    #[must_use]
    pub fn concatenate(parts: &[Arc<Self>]) -> Option<Self> {
        let first = parts.first()?;
        let sample_rate = first.sample_rate.max(1);
        let channels = parts.iter().map(|part| part.channels).max()?.clamp(1, 2);
        let out_channels = usize::from(channels);

        let mut samples: Vec<f32> = Vec::new();
        let mut frames_written: u32 = 0;
        for part in parts {
            let src_channels = usize::from(part.channels.max(1));
            let src_frames = part.frame_count();
            if src_frames == 0 {
                continue;
            }
            // How many output frames this part becomes at the target rate.
            let ratio = f64::from(sample_rate) / f64::from(part.sample_rate.max(1));
            let out_frames = (f64::from(src_frames) * ratio).floor();
            let out_frames = if out_frames.is_finite() && out_frames >= 1.0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    out_frames.min(f64::from(MAX_CONCATENATED_FRAMES)) as u32
                }
            } else {
                continue;
            };
            if frames_written.saturating_add(out_frames) > MAX_CONCATENATED_FRAMES {
                return None;
            }
            for frame in 0..out_frames {
                let position = f64::from(frame) / ratio;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let index = position.floor().max(0.0) as u32;
                let index = index.min(src_frames.saturating_sub(1));
                #[allow(clippy::cast_possible_truncation)]
                let frac = (position - f64::from(index)).clamp(0.0, 1.0) as f32;
                let next = (index + 1).min(src_frames.saturating_sub(1));
                for channel in 0..out_channels {
                    let a = sample_at(&part.samples, src_channels, index, channel);
                    let b = sample_at(&part.samples, src_channels, next, channel);
                    samples.push(a + (b - a) * frac);
                }
            }
            frames_written = frames_written.saturating_add(out_frames);
        }

        if frames_written == 0 {
            return None;
        }
        Some(Self {
            channels,
            sample_rate,
            samples: Arc::from(samples),
            loop_range: None,
        })
    }
}

/// One channel of one frame, with a mono source read into either ear and a
/// stereo source read into the ear asked for. Out-of-range reads are
/// silence rather than a panic.
fn sample_at(samples: &[f32], src_channels: usize, frame: u32, channel: usize) -> f32 {
    let base = frame as usize * src_channels;
    let offset = if src_channels == 1 {
        0
    } else {
        channel.min(src_channels - 1)
    };
    samples.get(base + offset).copied().unwrap_or(0.0)
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

    fn ramp(frames: usize, sample_rate: u32) -> Arc<SoundBuffer> {
        #[allow(clippy::cast_precision_loss)]
        let samples: Vec<f32> = (0..frames).map(|index| index as f32 / 100.0).collect();
        Arc::new(SoundBuffer {
            channels: 1,
            sample_rate,
            samples: Arc::from(samples),
            loop_range: None,
        })
    }

    #[test]
    fn concatenating_words_appends_their_frames_in_order() {
        let joined = SoundBuffer::concatenate(&[ramp(4, 8_000), ramp(3, 8_000)])
            .expect("two non-empty words join");
        assert_eq!(joined.frame_count(), 7);
        assert_eq!(joined.sample_rate, 8_000);
        assert_eq!(joined.loop_range, None);
        assert!(
            (joined.samples[4] - 0.0).abs() < 1e-6,
            "second word restarts"
        );
    }

    #[test]
    fn concatenating_resamples_a_word_recorded_at_another_rate() {
        // The second word runs at half the first's rate, so it takes twice
        // as many output frames.
        let joined = SoundBuffer::concatenate(&[ramp(4, 8_000), ramp(4, 4_000)])
            .expect("two non-empty words join");
        assert_eq!(joined.sample_rate, 8_000);
        assert_eq!(joined.frame_count(), 12);
    }

    #[test]
    fn concatenating_nothing_or_only_silence_yields_no_buffer() {
        assert!(SoundBuffer::concatenate(&[]).is_none());
        let empty = Arc::new(SoundBuffer {
            channels: 1,
            sample_rate: 8_000,
            samples: Arc::from(Vec::new()),
            loop_range: None,
        });
        assert!(SoundBuffer::concatenate(&[empty]).is_none());
    }

    #[test]
    fn a_decoded_wav_becomes_a_buffer_with_its_loop_range() {
        let decoded = crate::wav::DecodedWav {
            format: crate::wav::WavFormat {
                channels: 1,
                sample_rate: 22_050,
                bits_per_sample: 16,
            },
            samples: vec![0.0, 0.5, 1.0, 0.5],
            cue_points: vec![crate::wav::CuePoint {
                id: 1,
                sample_offset: 1,
            }],
            sample_loops: Vec::new(),
        };
        let buffer = SoundBuffer::from_decoded(&decoded);
        assert_eq!(buffer.frame_count(), 4);
        assert_eq!(buffer.loop_range, Some((1, 4)));
        assert_eq!(buffer.byte_len(), 16);
    }

    #[test]
    fn stream_class_always_replaces_the_single_slot() {
        let channels = vec![dummy_channel(1, ChannelClass::Stream, 0)];
        assert_eq!(slot_to_replace(&channels, 2, ChannelClass::Stream), Some(0));
    }
}
