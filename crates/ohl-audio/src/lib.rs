//! Bounded WAV decoding and a software mixer for Open Half-Life.
//!
//! - [`wav`] decodes RIFF/WAVE sound assets (PCM 8-bit unsigned or 16-bit
//!   signed, mono or stereo, at a common legacy sample rate) plus the
//!   `cue `/`smpl` loop-point chunks used by looping GoldSrc sounds, all
//!   from the public RIFF/WAVE specifications.
//! - [`mixer`] renders active channels into interleaved `f32` stereo at a
//!   fixed device rate, with linear pitch resampling, loop points, GoldSrc-
//!   style distance attenuation and panning, and a fixed-capacity channel-
//!   class/replacement model (see [`mixer::channel`]).
//! - [`device`] selects an output backend at runtime and never panics when
//!   no device exists; see its module docs and
//!   `docs/RENDER_DEPENDENCIES.md` for why `cpal` is only ever a dependency
//!   on macOS/Windows.
//!
//! This crate contains no `unsafe` code (enforced workspace-wide by
//! `[workspace.lints.rust] unsafe_code = "forbid"`) and, on every platform
//! this actually builds a real device for, reaches the OS's own audio API
//! (CoreAudio, WASAPI) rather than a vendored or bound third-party C
//! library.

pub mod device;
pub mod error;
pub mod mixer;
pub mod wav;

pub use error::{AudioError, Result};
pub use mixer::{ChannelClass, Listener, Mixer, PlayRequest, SoundBuffer, SoundSpatial};
pub use wav::{CuePoint, DecodedWav, SampleLoop, WavFormat};
