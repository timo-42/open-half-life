//! Output backend selection: a real device on macOS/Windows, always a
//! [`NullSink`] on Linux and in every headless/test build.
//!
//! See the crate-level docs and `docs/RENDER_DEPENDENCIES.md` ("Audio
//! backend") for why: `cpal`'s only Linux backend links libasound through
//! `alsa-sys`'s build-time `pkg-config` lookup, which is exactly the
//! link-time C-library dependency `PROMPT.md`'s "No FFI" rule forbids.
//! `cpal` is therefore declared only as a `[target.'cfg(...)']` dependency
//! for `macos`/`windows` in `Cargo.toml`, so it (and `alsa-sys`) is never
//! even resolved when building for Linux; there is no Linux code path in
//! this module to gate behind a feature flag in the first place.
//!
//! [`open_default_device`] never panics: a machine with no output device
//! (any CI runner) gets a working [`NullSink`] instead of an error.

use crate::error::Result;
use crate::mixer::Mixer;
use std::sync::{Arc, Mutex};

/// An audio output backend that can be handed a [`Mixer`] to pull
/// interleaved stereo `f32` frames from.
pub trait OutputDevice {
    /// The device's actual output sample rate.
    fn sample_rate(&self) -> u32;

    /// Starts delivering audio, rendered on demand from `mixer`. Calling
    /// this again after a previous `start` restarts delivery.
    fn start(&mut self, mixer: Arc<Mutex<Mixer>>) -> Result<()>;

    /// Stops delivering audio. Idempotent.
    fn stop(&mut self);
}

/// A headless output backend that never touches real hardware: used in
/// tests, on Linux (see the module docs), and as the fallback when no
/// device is available. Rendering only happens when [`NullSink::pump`] is
/// called, so it never spins a background thread.
pub struct NullSink {
    sample_rate: u32,
    mixer: Option<Arc<Mutex<Mixer>>>,
}

impl NullSink {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            mixer: None,
        }
    }

    /// Pulls and discards `frame_count` stereo frames from the attached
    /// mixer, exercising the same render path a real device's callback
    /// would use. A no-op (not an error) when no mixer is attached yet.
    pub fn pump(&self, frame_count: usize) {
        let Some(mixer) = &self.mixer else {
            return;
        };
        let mut buffer = vec![0.0f32; frame_count * 2];
        if let Ok(mut mixer) = mixer.lock() {
            mixer.render(&mut buffer);
        }
    }
}

impl OutputDevice for NullSink {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self, mixer: Arc<Mutex<Mixer>>) -> Result<()> {
        self.mixer = Some(mixer);
        Ok(())
    }

    fn stop(&mut self) {
        self.mixer = None;
    }
}

/// Opens the platform's default output device on macOS/Windows, or a
/// [`NullSink`] everywhere else (including when no device is present, so
/// this never fails and never panics).
#[must_use]
pub fn open_default_device(preferred_sample_rate: u32) -> Box<dyn OutputDevice> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if let Ok(device) = cpal_backend::CpalDevice::open_default() {
            return Box::new(device);
        }
    }
    let _ = preferred_sample_rate;
    Box::new(NullSink::new(preferred_sample_rate))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod cpal_backend {
    use super::{Mutex, OutputDevice, Result};
    use crate::error::AudioError;
    use crate::mixer::Mixer;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::Arc;

    /// A real output device reached through `cpal`'s CoreAudio (macOS) or
    /// WASAPI (Windows) backend: OS-API surface, not a third-party C
    /// library (see the module docs).
    pub struct CpalDevice {
        sample_rate: u32,
        stream: Option<cpal::Stream>,
    }

    impl CpalDevice {
        pub fn open_default() -> Result<Self> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or(AudioError::DeviceUnavailable)?;
            let config = device
                .default_output_config()
                .map_err(|_| AudioError::DeviceUnavailable)?;
            Ok(Self {
                sample_rate: config.sample_rate(),
                stream: None,
            })
        }
    }

    impl OutputDevice for CpalDevice {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn start(&mut self, mixer: Arc<Mutex<Mixer>>) -> Result<()> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or(AudioError::DeviceUnavailable)?;
            let config = device
                .default_output_config()
                .map_err(|_| AudioError::DeviceUnavailable)?;
            let stream_config: cpal::StreamConfig = config.into();
            let channels = usize::from(stream_config.channels).max(1);

            let stream = device
                .build_output_stream(
                    stream_config,
                    move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                        render_into(&mixer, data, channels);
                    },
                    |_error| {},
                    None,
                )
                .map_err(|_| AudioError::DeviceUnavailable)?;
            stream.play().map_err(|_| AudioError::DeviceUnavailable)?;
            self.stream = Some(stream);
            Ok(())
        }

        fn stop(&mut self) {
            self.stream = None;
        }
    }

    /// Renders one callback's worth of audio from `mixer` (always stereo)
    /// into `data`, which may have a different channel count: stereo
    /// devices get the mix directly, and any other channel count is
    /// filled from the same left/right pair (mono sums both, and any
    /// wider layout repeats the stereo pair across extra channels).
    fn render_into(mixer: &Arc<Mutex<Mixer>>, data: &mut [f32], channels: usize) {
        if channels == 2 {
            if let Ok(mut mixer) = mixer.lock() {
                mixer.render(data);
            } else {
                data.fill(0.0);
            }
            return;
        }

        let frames = data.len() / channels;
        let mut stereo = vec![0.0f32; frames * 2];
        if let Ok(mut mixer) = mixer.lock() {
            mixer.render(&mut stereo);
        }
        for (frame_index, frame) in data.chunks_mut(channels).enumerate() {
            let left = stereo.get(frame_index * 2).copied().unwrap_or(0.0);
            let right = stereo.get(frame_index * 2 + 1).copied().unwrap_or(0.0);
            if channels == 1 {
                frame[0] = f32::midpoint(left, right);
            } else {
                for (index, sample) in frame.iter_mut().enumerate() {
                    *sample = if index % 2 == 0 { left } else { right };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::{ChannelClass, PlayRequest, SoundBuffer};
    use std::sync::Arc;

    #[test]
    fn null_sink_never_panics_without_a_mixer() {
        let sink = NullSink::new(44_100);
        sink.pump(128);
        assert_eq!(sink.sample_rate(), 44_100);
    }

    #[test]
    fn null_sink_pumps_a_headless_mix() {
        let mut sink = NullSink::new(8_000);
        let mixer = Arc::new(Mutex::new(Mixer::new(8_000)));
        {
            let mut locked = mixer.lock().expect("lock mixer");
            locked.play(PlayRequest {
                entity: 1,
                class: ChannelClass::Auto,
                buffer: Arc::new(SoundBuffer {
                    channels: 1,
                    sample_rate: 8_000,
                    samples: Arc::from(vec![1.0f32; 8]),
                    loop_range: None,
                }),
                volume: 1.0,
                pitch: 1.0,
                spatial: None,
            });
        }
        sink.start(Arc::clone(&mixer)).expect("start null sink");
        sink.pump(4);
        assert_eq!(mixer.lock().expect("lock mixer").active_channel_count(), 1);
        sink.stop();
    }

    #[test]
    fn open_default_device_never_panics() {
        // On Linux this always resolves to a NullSink; on macOS/Windows it
        // falls back to one if no real device is present. Either way it
        // must not panic in a headless CI environment.
        let device = open_default_device(44_100);
        assert!(device.sample_rate() > 0);
    }
}
