//! A fixed, project-owned error enum for this crate's WAV decoder, mixer,
//! and output device.
//!
//! Every variant is a fixed code with a fixed `Display` message: none of
//! them ever carry file-derived bytes, offsets, or names, mirroring
//! `ohl_core::SanitizedError` and `ohl_formats::FormatError` (see those
//! crates for the same policy applied to other media). Decoders and the
//! mixer in this crate never panic on malformed or unexpected input; every
//! fallible operation returns one of these variants instead.

use core::fmt;

/// A bounds, validation, or device failure in `ohl-audio`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioError {
    /// The buffer is too short to contain a fixed-size header or chunk.
    Truncated,
    /// A RIFF/WAVE signature (`RIFF`, `WAVE`, `fmt `, `data`) did not match.
    BadSignature,
    /// A chunk or field declared a size that does not fit the containing
    /// buffer.
    OutOfBounds,
    /// The WAV encodes a sample format this decoder does not support (only
    /// 8-bit unsigned and 16-bit signed PCM are accepted).
    UnsupportedFormat,
    /// The WAV declares an unsupported channel count (only mono and
    /// stereo are accepted).
    UnsupportedChannelCount,
    /// The WAV declares an unsupported sample rate.
    UnsupportedSampleRate,
    /// A chunk, cue point, or loop count exceeds this crate's configured
    /// bound.
    LimitExceeded,
    /// The input otherwise failed validation (malformed field or
    /// combination of fields).
    InvalidInput,
    /// The requested output device or backend is not available in this
    /// build or on this platform.
    DeviceUnavailable,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Truncated => "buffer is too short for the expected structure",
            Self::BadSignature => "RIFF/WAVE signature did not match",
            Self::OutOfBounds => "chunk offset or length falls outside the buffer",
            Self::UnsupportedFormat => "only 8-bit unsigned and 16-bit signed PCM are supported",
            Self::UnsupportedChannelCount => "only mono and stereo are supported",
            Self::UnsupportedSampleRate => "sample rate is outside the supported range",
            Self::LimitExceeded => "count exceeds the configured limit",
            Self::InvalidInput => "input failed validation",
            Self::DeviceUnavailable => "no audio output device is available",
        };
        f.write_str(message)
    }
}

impl core::error::Error for AudioError {}

/// A `Result` alias for this crate.
pub type Result<T> = core::result::Result<T, AudioError>;
