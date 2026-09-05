//! A software mixer that renders active channels into interleaved `f32`
//! stereo frames at a fixed device sample rate.
//!
//! Each channel tracks its own fractional playback position and is
//! resampled with linear interpolation from its own source rate/pitch to
//! the device rate (see [`channel::ActiveChannel`]); loop points from
//! [`crate::wav::DecodedWav::effective_loop`] are honored by wrapping the
//! playback position back to the loop start. Channel-class capacity and
//! replacement rules live in [`channel`]; distance attenuation and panning
//! live in [`spatial`].

pub mod channel;
pub mod spatial;

pub use channel::{ChannelClass, PlayRequest, SoundBuffer};
pub use spatial::{Listener, SoundSpatial};

use channel::{ActiveChannel, slot_to_replace};

/// The software mixer. Not itself tied to any output backend: call
/// [`Mixer::render`] to fill a caller-owned interleaved stereo buffer,
/// whether that buffer is then handed to a real device or to
/// [`crate::device::NullSink`] in a test.
pub struct Mixer {
    device_sample_rate: u32,
    listener: Listener,
    channels: Vec<ActiveChannel>,
    next_order: u64,
}

impl Mixer {
    /// Creates a mixer that renders at `device_sample_rate` (for example
    /// 44_100 or 48_000).
    #[must_use]
    pub fn new(device_sample_rate: u32) -> Self {
        Self {
            device_sample_rate: device_sample_rate.max(1),
            listener: Listener::default(),
            channels: Vec::new(),
            next_order: 0,
        }
    }

    #[must_use]
    pub fn device_sample_rate(&self) -> u32 {
        self.device_sample_rate
    }

    pub fn set_listener(&mut self, listener: Listener) {
        self.listener = listener;
    }

    /// The number of channels currently playing.
    #[must_use]
    pub fn active_channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Starts a new sound, applying the channel-class capacity and
    /// `(entity, class)` replacement rules documented in [`channel`].
    pub fn play(&mut self, request: PlayRequest) {
        if let Some(index) = slot_to_replace(&self.channels, request.entity, request.class) {
            self.channels.remove(index);
        }
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.channels.push(ActiveChannel::new(request, order));
    }

    /// Stops every channel matching `entity` (all classes).
    pub fn stop_entity(&mut self, entity: u32) {
        self.channels.retain(|channel| channel.entity != entity);
    }

    /// Stops every channel of `class`.
    pub fn stop_class(&mut self, class: ChannelClass) {
        self.channels.retain(|channel| channel.class != class);
    }

    /// Renders `out.len() / 2` stereo frames (any trailing odd sample is
    /// left untouched), mixing every active channel and removing channels
    /// that reach the end of a non-looping buffer. `out` is fully
    /// overwritten (this is not additive into caller content) and the
    /// result is clamped to `[-1.0, 1.0]`.
    pub fn render(&mut self, out: &mut [f32]) {
        let frame_capacity = out.len() / 2;
        for sample in out.iter_mut().take(frame_capacity * 2) {
            *sample = 0.0;
        }

        let device_rate = f64::from(self.device_sample_rate);
        let listener = self.listener;
        let mut finished = Vec::new();

        for (channel_index, channel) in self.channels.iter_mut().enumerate() {
            let buffer = channel.buffer.clone();
            let frame_count = buffer.frame_count();
            if frame_count == 0 {
                channel.finished = true;
            }
            if channel.finished {
                finished.push(channel_index);
                continue;
            }

            let src_channels = usize::from(buffer.channels.max(1));
            let step =
                (f64::from(buffer.sample_rate) / device_rate) * f64::from(channel.pitch.max(0.0));
            let gains = match channel.spatial {
                Some(spatial) => spatial::spatial_gain(&listener, spatial, channel.volume),
                None => spatial::StereoGain {
                    left: channel.volume,
                    right: channel.volume,
                },
            };

            for frame in 0..frame_capacity {
                if let Some((loop_start, loop_end)) = buffer.loop_range {
                    while channel.position >= f64::from(loop_end) && loop_end > loop_start {
                        channel.position =
                            f64::from(loop_start) + (channel.position - f64::from(loop_end));
                    }
                } else if channel.position >= f64::from(frame_count) {
                    channel.finished = true;
                }
                if channel.finished {
                    break;
                }

                // Both casts are safe: `position` is clamped into
                // `[0, frame_count]` (non-negative, and `frame_count` fits
                // `u32` by construction) immediately before the cast, and
                // `frac` is clamped into `[0.0, 1.0]`, which always fits an
                // `f32` exactly.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let idx = channel.position.floor().clamp(0.0, f64::from(frame_count)) as u32;
                let idx = idx.min(frame_count.saturating_sub(1));
                #[allow(clippy::cast_possible_truncation)]
                let frac = (channel.position - f64::from(idx)).clamp(0.0, 1.0) as f32;
                let next_idx = if idx + 1 < frame_count {
                    idx + 1
                } else if let Some((loop_start, _)) = buffer.loop_range {
                    loop_start.min(frame_count.saturating_sub(1))
                } else {
                    idx
                };

                let (l0, r0) = read_frame(&buffer.samples, src_channels, idx);
                let (l1, r1) = read_frame(&buffer.samples, src_channels, next_idx);
                let left = l0 + (l1 - l0) * frac;
                let right = r0 + (r1 - r0) * frac;

                out[frame * 2] += left * gains.left;
                out[frame * 2 + 1] += right * gains.right;

                channel.position += step;
            }

            if channel.finished {
                finished.push(channel_index);
            }
        }

        for &index in finished.iter().rev() {
            self.channels.remove(index);
        }

        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

fn read_frame(samples: &[f32], src_channels: usize, frame_index: u32) -> (f32, f32) {
    let base = frame_index as usize * src_channels;
    if src_channels == 1 {
        let value = samples.get(base).copied().unwrap_or(0.0);
        (value, value)
    } else {
        let left = samples.get(base).copied().unwrap_or(0.0);
        let right = samples.get(base + 1).copied().unwrap_or(0.0);
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn mono_buffer(
        samples: &[f32],
        sample_rate: u32,
        loop_range: Option<(u32, u32)>,
    ) -> Arc<SoundBuffer> {
        Arc::new(SoundBuffer {
            channels: 1,
            sample_rate,
            samples: Arc::from(samples.to_vec()),
            loop_range,
        })
    }

    fn play_request(buffer: Arc<SoundBuffer>) -> PlayRequest {
        PlayRequest {
            entity: 1,
            class: ChannelClass::Auto,
            buffer,
            volume: 1.0,
            pitch: 1.0,
            spatial: None,
        }
    }

    #[test]
    // Deterministic-render check: unmodified 1:1 playback reproduces its
    // source samples exactly, so exact float equality is the point.
    #[allow(clippy::float_cmp)]
    fn non_positional_mono_plays_identically_into_both_ears() {
        let buffer = mono_buffer(&[0.0, 0.25, 0.5, 0.75], 8_000, None);
        let mut mixer = Mixer::new(8_000);
        mixer.play(play_request(buffer));

        let mut out = [0.0f32; 8];
        mixer.render(&mut out);
        assert_eq!(out, [0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75]);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn channel_is_removed_after_non_looping_buffer_ends() {
        let buffer = mono_buffer(&[1.0, 1.0], 8_000, None);
        let mut mixer = Mixer::new(8_000);
        mixer.play(play_request(buffer));

        let mut out = [0.0f32; 8];
        mixer.render(&mut out);
        assert_eq!(mixer.active_channel_count(), 0);
        // Only the first two frames carry sound; the rest is silence.
        assert_eq!(out, [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn looping_buffer_wraps_at_loop_end() {
        // Amplitudes stay within [-1, 1] (the mixer clamps its output like
        // a real mixer summing multiple channels would).
        let buffer = mono_buffer(&[0.0, 0.3, 0.6, 0.9], 8_000, Some((1, 3)));
        let mut mixer = Mixer::new(8_000);
        mixer.play(play_request(buffer));

        let mut out = [0.0f32; 12];
        mixer.render(&mut out);
        // Frame values: 0, 0.3, 0.6, (wrap to loop_start=1), 0.3, 0.6, ...
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let expected = [0.0, 0.3, 0.6, 0.3, 0.6, 0.3];
        for (actual, expected) in left.iter().zip(expected.iter()) {
            assert!((actual - expected).abs() < 1e-6, "{left:?} != {expected:?}");
        }
        assert_eq!(mixer.active_channel_count(), 1);
    }

    #[test]
    fn double_pitch_halves_effective_duration() {
        let buffer = mono_buffer(&[0.0, 0.25, 0.5, 0.75], 8_000, None);
        let mut mixer = Mixer::new(8_000);
        let mut request = play_request(buffer);
        request.pitch = 2.0;
        mixer.play(request);

        let mut out = [0.0f32; 8];
        mixer.render(&mut out);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        // Position advances by 2.0 per output frame: 0, 2, 4(past end)...
        assert!((left[0] - 0.0).abs() < 1e-6);
        assert!((left[1] - 0.5).abs() < 1e-6);
        assert_eq!(mixer.active_channel_count(), 0);
    }

    #[test]
    fn spatial_sound_directly_right_favors_right_ear() {
        let buffer = mono_buffer(&[1.0; 4], 8_000, None);
        let mut mixer = Mixer::new(8_000);
        let mut request = play_request(buffer);
        request.spatial = Some(SoundSpatial {
            position: [10.0, 0.0, 0.0],
            attenuation: 0.0,
        });
        mixer.play(request);

        let mut out = [0.0f32; 8];
        mixer.render(&mut out);
        assert!(out[1] > out[0]);
    }

    #[test]
    fn stream_class_replaces_previous_stream() {
        let buffer_a = mono_buffer(&[1.0; 4], 8_000, None);
        let buffer_b = mono_buffer(&[0.5; 4], 8_000, None);
        let mut mixer = Mixer::new(8_000);
        mixer.play(PlayRequest {
            entity: 1,
            class: ChannelClass::Stream,
            ..play_request(buffer_a)
        });
        mixer.play(PlayRequest {
            entity: 2,
            class: ChannelClass::Stream,
            ..play_request(buffer_b)
        });
        assert_eq!(mixer.active_channel_count(), 1);
    }

    #[test]
    fn render_handles_headless_zero_channel_mix() {
        let mut mixer = Mixer::new(44_100);
        let mut out = vec![0.0f32; 256];
        mixer.render(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
    }
}
