//! The audio composition root: an output device, one `ohl_audio::Mixer`,
//! and a bounded cache of decoded sound assets.
//!
//! This is the only place in the project that turns an
//! `ohl_engine::GameEvent::Sound` into something audible. The engine
//! produces cues (an owning entity, a channel class, the asset the *map or
//! the payload's own data files* named, and how to spatialise it) and never
//! touches a sound file; this module resolves each cue's asset through the
//! same [`ohl_engine::AssetSource`] the renderer resolves models and
//! sprites through, decodes it with `ohl_audio::wav`, and hands the mixer a
//! `PlayRequest`.
//!
//! # Staying headless-safe
//!
//! [`AudioRuntime::silent`] never opens a device on any platform: it drives
//! an `ohl_audio::device::NullSink`, which renders only when this module
//! pumps it and writes the result nowhere. Every non-interactive run path
//! (`--screenshot`, `--script`, `--chain-script`, `--benchmark`, and every
//! test) uses it, so a smoke run is silent by construction rather than by
//! luck.
//! [`AudioRuntime::open`] — the windowed loop's — asks
//! `ohl_audio::device::open_default_device` for a real backend, which on
//! Linux is *also* a `NullSink` by the recorded no-FFI decision (see
//! `docs/RENDER_DEPENDENCIES.md`, "Audio backend"). Neither ever panics and
//! neither ever fails: a machine with no output device gets silence.
//!
//! # Logging policy
//!
//! The same as the rest of `ohl-app`: no asset path, sentence name, entity
//! count or byte count from a payload ever reaches a log line. Nothing in
//! this module logs at all.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ohl_audio::device::{NullSink, OutputDevice, open_default_device};
use ohl_audio::mixer::{ChannelClass, Mixer, PlayRequest, SoundBuffer, SoundSpatial};
use ohl_audio::{Listener, wav};
use ohl_engine::{AssetSource, SoundAsset, SoundCue};

/// The sample rate a headless mixer runs at. A real device reports its own
/// and that is used instead.
pub(crate) const HEADLESS_SAMPLE_RATE: u32 = 44_100;

/// How much decoded PCM the sound cache holds before it starts evicting
/// least-recently-used entries. Project-owned: about thirty seconds of
/// 44.1 kHz stereo `f32`, generous for a level's ambience and speech and
/// small enough that a long chain walk cannot grow the process without
/// bound.
pub(crate) const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// How many distinct assets the cache tracks, counting the ones it has
/// learned are missing or undecodable (which hold no samples but must
/// still be remembered, or every frame would re-read them).
pub(crate) const MAX_CACHE_ENTRIES: usize = 512;

/// The most frames one [`AudioRuntime::frame`] pumps into a headless sink,
/// so a stalled host frame cannot ask for an unbounded render.
const MAX_PUMP_SECONDS: f32 = 0.25;

/// A bounded, least-recently-used cache of decoded sound assets.
///
/// A miss — the payload does not publish the asset, or the bytes are not a
/// WAV this project decodes — is cached as a `None` entry, so a sound the
/// map names but the installation does not carry is read once and then
/// ignored for free. Nothing here panics on malformed input:
/// `ohl_audio::wav::decode` is a bounded, non-panicking decoder (see its
/// own `proptest` harness).
/// What [`SoundCache::touch`] found: nothing at all, a remembered miss, or
/// a decoded buffer. Kept as its own three-state answer rather than a
/// nested `Option`, since "not cached" and "cached as unplayable" are
/// different instructions to the caller.
enum Cached {
    /// Never resolved before; the caller must read and decode it.
    Absent,
    /// Already resolved, and there is nothing to play.
    Nothing,
    /// Already resolved and playable.
    Buffer(Arc<SoundBuffer>),
}

pub(crate) struct SoundCache {
    entries: HashMap<String, Option<Arc<SoundBuffer>>>,
    /// Keys in least-recently-used order, oldest first.
    order: VecDeque<String>,
    bytes: usize,
    max_bytes: usize,
    max_entries: usize,
}

impl SoundCache {
    /// A cache bounded by [`MAX_CACHE_BYTES`] and [`MAX_CACHE_ENTRIES`].
    pub(crate) fn new() -> Self {
        Self::with_limits(MAX_CACHE_BYTES, MAX_CACHE_ENTRIES)
    }

    pub(crate) fn with_limits(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
            max_entries: max_entries.max(1),
        }
    }

    /// How many bytes of decoded PCM are currently held.
    #[cfg(test)]
    pub(crate) fn byte_len(&self) -> usize {
        self.bytes
    }

    /// How many assets (playable or known-missing) are currently tracked.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The buffer for `asset`, decoding and caching it on first use.
    /// `None` means "nothing to play": an unresolved cue, a missing file,
    /// or bytes that did not decode.
    pub(crate) fn resolve(
        &mut self,
        source: &dyn AssetSource,
        asset: &SoundAsset,
    ) -> Option<Arc<SoundBuffer>> {
        match asset {
            SoundAsset::Unresolved => None,
            SoundAsset::File(path) => self.file(source, path),
            SoundAsset::Sentence(words) => self.sentence(source, words),
        }
    }

    /// One WAV, by game-relative asset path.
    fn file(&mut self, source: &dyn AssetSource, path: &str) -> Option<Arc<SoundBuffer>> {
        match self.touch(path) {
            Cached::Nothing => return None,
            Cached::Buffer(buffer) => return Some(buffer),
            Cached::Absent => {}
        }
        let decoded = source
            .read(path)
            .and_then(|bytes| wav::decode(&bytes).ok())
            .map(|wav| Arc::new(SoundBuffer::from_decoded(&wav)))
            .filter(|buffer| buffer.frame_count() > 0);
        self.insert(path.to_string(), decoded)
    }

    /// A sentence: every word decoded and joined end to end into one
    /// buffer, so it plays as a single channel (the published
    /// `sentences.txt` behaviour of stringing samples together back to
    /// back). Words the payload does not publish are simply skipped; a
    /// sentence with nothing left is cached as a miss.
    fn sentence(
        &mut self,
        source: &dyn AssetSource,
        words: &[Arc<str>],
    ) -> Option<Arc<SoundBuffer>> {
        let key = sentence_key(words);
        match self.touch(&key) {
            Cached::Nothing => return None,
            Cached::Buffer(buffer) => return Some(buffer),
            Cached::Absent => {}
        }
        let mut parts = Vec::with_capacity(words.len());
        for word in words {
            // Deliberately not cached per word: the joined sentence is
            // what gets replayed, and caching both would hold the same
            // samples twice.
            let Some(bytes) = source.read(word) else {
                continue;
            };
            let Ok(decoded) = wav::decode(&bytes) else {
                continue;
            };
            let buffer = SoundBuffer::from_decoded(&decoded);
            if buffer.frame_count() > 0 {
                parts.push(Arc::new(buffer));
            }
        }
        let joined = SoundBuffer::concatenate(&parts).map(Arc::new);
        self.insert(key, joined)
    }

    /// Marks `key` as most recently used and reports what the cache knows
    /// about it.
    fn touch(&mut self, key: &str) -> Cached {
        let Some(entry) = self.entries.get(key).cloned() else {
            return Cached::Absent;
        };
        if let Some(index) = self.order.iter().position(|held| held == key) {
            let key = self.order.remove(index).unwrap_or_else(|| key.to_string());
            self.order.push_back(key);
        }
        entry.map_or(Cached::Nothing, Cached::Buffer)
    }

    /// Inserts `entry` under `key`, evicting least-recently-used entries
    /// until both bounds hold again, and returns it.
    fn insert(&mut self, key: String, entry: Option<Arc<SoundBuffer>>) -> Option<Arc<SoundBuffer>> {
        let size = entry.as_ref().map_or(0, |buffer| buffer.byte_len());
        // A single asset larger than the whole cache is played but never
        // held, rather than evicting everything else to make room for it.
        if size > self.max_bytes {
            return entry;
        }
        self.bytes = self.bytes.saturating_add(size);
        self.entries.insert(key.clone(), entry.clone());
        self.order.push_back(key);
        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                let freed = evicted.as_ref().map_or(0, |buffer| buffer.byte_len());
                self.bytes = self.bytes.saturating_sub(freed);
            }
        }
        entry
    }
}

/// The cache key one sentence's word list gets. A newline cannot appear in
/// a game-relative asset path (`ohl_assets`'s path policy rejects control
/// characters), so joining on one cannot collide with a file path.
fn sentence_key(words: &[Arc<str>]) -> String {
    let mut key = String::from("!");
    for word in words {
        key.push('\n');
        key.push_str(word);
    }
    key
}

/// Where the sink came from, which is also whether it needs pumping.
enum Sink {
    /// A headless sink that only renders when this module asks it to.
    Silent(NullSink),
    /// Whatever `open_default_device` returned. On macOS/Windows that may
    /// be a real device pulling on its own callback; on Linux it is a
    /// `NullSink`, which this still pumps through the trait's own `pump`
    /// (a no-op for a real device).
    Default(Box<dyn OutputDevice>),
}

impl Sink {
    fn as_device_mut(&mut self) -> &mut dyn OutputDevice {
        match self {
            Self::Silent(sink) => sink,
            Self::Default(device) => device.as_mut(),
        }
    }

    fn as_device(&self) -> &dyn OutputDevice {
        match self {
            Self::Silent(sink) => sink,
            Self::Default(device) => device.as_ref(),
        }
    }
}

/// The device, mixer and asset cache one run owns.
pub(crate) struct AudioRuntime {
    sink: Sink,
    mixer: Arc<Mutex<Mixer>>,
    cache: SoundCache,
}

impl AudioRuntime {
    /// A runtime that never opens an output device on any platform. Every
    /// headless run path and every test uses this.
    pub(crate) fn silent() -> Self {
        Self::with_sink(Sink::Silent(NullSink::new(HEADLESS_SAMPLE_RATE)))
    }

    /// A runtime over the platform's default output device, falling back
    /// to silence when there is none. Never fails and never panics.
    pub(crate) fn open() -> Self {
        Self::with_sink(Sink::Default(open_default_device(HEADLESS_SAMPLE_RATE)))
    }

    fn with_sink(mut sink: Sink) -> Self {
        let rate = sink.as_device().sample_rate();
        let mixer = Arc::new(Mutex::new(Mixer::new(rate)));
        // A backend that refuses to start is left alone: the mixer still
        // exists, cues are still resolved, and nothing is heard.
        let _ = sink.as_device_mut().start(Arc::clone(&mixer));
        Self {
            sink,
            mixer,
            cache: SoundCache::new(),
        }
    }

    /// The mixer, for a test that wants to inspect what is playing.
    #[cfg(test)]
    pub(crate) fn mixer(&self) -> &Arc<Mutex<Mixer>> {
        &self.mixer
    }

    /// The asset cache, for a test that wants to inspect its bounds.
    #[cfg(test)]
    pub(crate) fn cache(&self) -> &SoundCache {
        &self.cache
    }

    /// Points the listener at the player's eye, facing `yaw` (the
    /// `ohl_render::FreeFlyCamera` convention: counter-clockwise around
    /// `+Z` from `+X`).
    ///
    /// Only the horizontal pan axis is modelled (`ohl_audio::Listener`
    /// carries no elevation), so pitch is deliberately not read: this
    /// hands the mixer the camera's own right vector — its flat forward
    /// crossed with world up, the same basis `FreeFlyCamera::update`
    /// strafes along.
    pub(crate) fn set_listener(&self, eye: [f32; 3], yaw: f32) {
        let yaw = yaw.to_radians();
        let (sin, cos) = yaw.sin_cos();
        let listener = Listener {
            position: eye,
            // cross([cos, sin, 0], [0, 0, 1]) — already unit length.
            right: [sin, -cos, 0.0],
        };
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.set_listener(listener);
        }
    }

    /// Sets the output volume, `0.0..=1.0`: the options menu's slider. It
    /// scales every channel, sounds already playing included; see
    /// `ohl_audio::Mixer::set_master_volume` for how a bad value is
    /// bounded.
    pub(crate) fn set_volume(&self, volume: f32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.set_master_volume(volume);
        }
    }

    /// Acts on one `GameEvent::Sound`: starts the cue's asset on its
    /// `(entity, class)` channel, or stops that channel when the cue is a
    /// stop. A cue naming nothing playable is dropped silently, which is
    /// what every weapon, pickup and charger cue still does (see
    /// `ohl_gameplay::sounds`).
    pub(crate) fn play(&mut self, source: &dyn AssetSource, cue: &SoundCue) {
        if cue.stop {
            if let Ok(mut mixer) = self.mixer.lock() {
                mixer.stop(cue.entity, cue.class);
            }
            return;
        }
        let Some(buffer) = self.cache.resolve(source, &cue.asset) else {
            return;
        };
        let request = PlayRequest {
            entity: cue.entity,
            class: cue.class,
            buffer,
            volume: sane(cue.volume, 1.0).clamp(0.0, 1.0),
            pitch: sane(cue.pitch, 1.0).clamp(0.05, 4.0),
            spatial: cue.origin.map(|position| SoundSpatial {
                position,
                attenuation: sane(cue.attenuation, ohl_engine::ATTN_NORM).max(0.0),
            }),
        };
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.play(request);
        }
    }

    /// Stops everything currently playing. A level change does this: the
    /// arriving map announces its own soundscape from scratch (see
    /// `ohl_engine`'s presentation phase), and the map being left must not
    /// keep humming underneath it.
    pub(crate) fn stop_all(&mut self) {
        if let Ok(mut mixer) = self.mixer.lock() {
            for class in [
                ChannelClass::Auto,
                ChannelClass::Weapon,
                ChannelClass::Voice,
                ChannelClass::Item,
                ChannelClass::Body,
                ChannelClass::Stream,
                ChannelClass::Static,
            ] {
                mixer.stop_class(class);
            }
        }
    }

    /// Advances the mixer by one host frame's worth of audio.
    ///
    /// A real device's own callback is already pulling, so this is a
    /// no-op there (`OutputDevice::pump`'s default). A headless sink is
    /// pumped for `dt` seconds' worth of frames, so a non-looping sound
    /// still ends and its channel is still reclaimed in a run nobody can
    /// hear.
    pub(crate) fn frame(&mut self, dt: f32) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_PUMP_SECONDS)
        } else {
            0.0
        };
        let rate = f64::from(self.sink.as_device().sample_rate());
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (f64::from(dt) * rate) as usize;
        if frames > 0 {
            self.sink.as_device().pump(frames);
        }
    }
}

/// `fallback` for a value a bad keyvalue could have made non-finite.
fn sane(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// Project-authored sound fixtures shared by this module's tests and
/// `crate::game_run`'s.
#[cfg(test)]
pub(crate) mod fixtures {
    /// A valid mono 16-bit PCM WAV of `frames` frames, written by hand so
    /// this test needs no encoder dependency. Project-authored bytes.
    pub(crate) fn synthetic_wav(frames: usize) -> Vec<u8> {
        let data_bytes = frames * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(
            &u32::try_from(36 + data_bytes)
                .expect("fixture fits")
                .to_le_bytes(),
        );
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&22_050u32.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(
            &u32::try_from(data_bytes)
                .expect("fixture fits")
                .to_le_bytes(),
        );
        for frame in 0..frames {
            let value = i16::try_from(frame % 1000).unwrap_or(0);
            wav.extend_from_slice(&value.to_le_bytes());
        }
        wav
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::synthetic_wav;
    use super::{AudioRuntime, SoundCache};
    use ohl_audio::mixer::ChannelClass;
    use ohl_engine::{MemoryAssets, SoundAsset, SoundCue};
    use std::sync::Arc;

    fn assets() -> MemoryAssets {
        let mut assets = MemoryAssets::new();
        assets.insert("sound/ohl/one.wav", synthetic_wav(64));
        assets.insert("sound/ohl/two.wav", synthetic_wav(32));
        assets.insert("sound/ohl/broken.wav", vec![0x00; 48]);
        assets
    }

    #[test]
    fn a_missing_or_malformed_asset_is_cached_as_nothing_to_play() {
        let assets = assets();
        let mut cache = SoundCache::new();
        assert!(
            cache
                .resolve(&assets, &SoundAsset::file("sound/ohl/absent.wav"))
                .is_none()
        );
        assert!(
            cache
                .resolve(&assets, &SoundAsset::file("sound/ohl/broken.wav"))
                .is_none()
        );
        // Both are remembered, so neither is re-read every frame, and
        // neither holds any samples.
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.byte_len(), 0);
    }

    #[test]
    fn a_sentence_joins_its_words_into_one_buffer() {
        let assets = assets();
        let mut cache = SoundCache::new();
        let sentence = SoundAsset::sentence(vec![
            "sound/ohl/one.wav".to_string(),
            "sound/ohl/two.wav".to_string(),
            // A word the payload does not publish is skipped, not fatal.
            "sound/ohl/absent.wav".to_string(),
        ]);
        let joined = cache.resolve(&assets, &sentence).expect("two words decode");
        assert_eq!(joined.frame_count(), 96);
        assert_eq!(cache.len(), 1, "the sentence caches as one entry");
    }

    #[test]
    fn the_cache_evicts_least_recently_used_entries_to_stay_under_its_bounds() {
        let assets = assets();
        // Room for one 64-frame buffer's samples (64 * 4 bytes) only.
        let mut cache = SoundCache::with_limits(256, 8);
        let one = SoundAsset::file("sound/ohl/one.wav");
        let two = SoundAsset::file("sound/ohl/two.wav");
        assert!(cache.resolve(&assets, &one).is_some());
        assert!(cache.resolve(&assets, &two).is_some());
        assert!(cache.byte_len() <= 256, "{}", cache.byte_len());
        assert!(cache.len() <= 2);
        // Both still resolve afterwards, whichever was evicted.
        assert!(cache.resolve(&assets, &one).is_some());
        assert!(cache.resolve(&assets, &two).is_some());
    }

    #[test]
    fn an_entry_larger_than_the_whole_cache_plays_without_being_held() {
        let assets = assets();
        let mut cache = SoundCache::with_limits(16, 8);
        assert!(
            cache
                .resolve(&assets, &SoundAsset::file("sound/ohl/one.wav"))
                .is_some()
        );
        assert_eq!(cache.byte_len(), 0);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn a_silent_runtime_still_starts_stops_and_ages_channels() {
        let assets = assets();
        let mut audio = AudioRuntime::silent();
        audio.set_listener([0.0, 0.0, 64.0], 90.0);

        let cue = SoundCue::new(
            42,
            ChannelClass::Static,
            SoundAsset::file("sound/ohl/one.wav"),
        )
        .at([128.0, 0.0, 64.0], ohl_engine::ATTN_NORM);
        audio.play(&assets, &cue);
        assert!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .is_playing(42, ChannelClass::Static)
        );

        // 64 frames at 22.05 kHz is under three milliseconds, so one
        // ordinary frame's pump runs the non-looping buffer out.
        audio.frame(1.0 / 60.0);
        assert_eq!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            0,
            "a non-looping sound ends even where nobody can hear it"
        );

        audio.play(&assets, &cue);
        audio.play(&assets, &SoundCue::stopping(42, ChannelClass::Static));
        assert_eq!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            0
        );
        assert!(audio.cache().len() >= 1);
    }

    #[test]
    fn a_cue_naming_nothing_playable_starts_no_channel() {
        let assets = assets();
        let mut audio = AudioRuntime::silent();
        audio.play(
            &assets,
            &SoundCue::new(1, ChannelClass::Weapon, SoundAsset::Unresolved),
        );
        assert_eq!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            0
        );
    }

    #[test]
    fn a_level_change_silences_everything_that_was_playing() {
        let assets = assets();
        let mut audio = AudioRuntime::silent();
        for entity in 0..3u32 {
            audio.play(
                &assets,
                &SoundCue::new(
                    entity,
                    ChannelClass::Static,
                    SoundAsset::file("sound/ohl/one.wav"),
                ),
            );
        }
        assert_eq!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            3
        );
        audio.stop_all();
        assert_eq!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            0
        );
    }

    #[test]
    fn an_implausible_frame_time_never_asks_for_an_unbounded_render() {
        let mut audio = AudioRuntime::silent();
        audio.frame(f32::NAN);
        audio.frame(f32::INFINITY);
        audio.frame(-1.0);
        audio.frame(1_000_000.0);
        assert_eq!(
            audio
                .mixer()
                .lock()
                .expect("lock mixer")
                .active_channel_count(),
            0
        );
    }

    /// Facing `+X` (yaw 0, the `FreeFlyCamera` convention), the player's
    /// right hand points at `-Y`; a quarter turn later it points at `+X`.
    /// This is what makes a sound heard on the correct side.
    #[test]
    fn the_listener_axis_follows_the_cameras_yaw() {
        let audio = AudioRuntime::silent();
        audio.set_listener([8.0, 16.0, 64.0], 0.0);
        let listener = audio.mixer().lock().expect("lock mixer").listener();
        for (actual, expected) in listener.position.iter().zip([8.0, 16.0, 64.0]) {
            assert!((actual - expected).abs() < 1e-6, "{listener:?}");
        }
        assert!((listener.right[0] - 0.0).abs() < 1e-6, "{listener:?}");
        assert!((listener.right[1] + 1.0).abs() < 1e-6, "{listener:?}");

        audio.set_listener([0.0, 0.0, 0.0], 90.0);
        let listener = audio.mixer().lock().expect("lock mixer").listener();
        assert!((listener.right[0] - 1.0).abs() < 1e-6, "{listener:?}");
        assert!((listener.right[1] - 0.0).abs() < 1e-6, "{listener:?}");
    }

    /// The whole path, end to end: a synthetic room with two published
    /// `ambient_generic` entities, its `GameEvent::Sound` stream fed
    /// through this module exactly as the run paths in `crate::game_run`
    /// feed it, against a payload that publishes one of the two sounds.
    ///
    /// The room, the entity block and both `message` values are
    /// project-authored; nothing here comes from any game installation.
    #[test]
    fn a_synthetic_rooms_event_stream_starts_the_channels_its_map_asked_for() {
        use ohl_engine::test_support::{entity_block, script_game, script_room_entities};
        use ohl_engine::{GameEvent, Input, TICK_SECONDS};

        let ambient = |name: &str, origin: [f32; 3], message: &str, spawnflags: &str| {
            entity_block(
                "ambient_generic",
                origin,
                0.0,
                &[
                    ("targetname", name),
                    ("message", message),
                    ("spawnflags", spawnflags),
                ],
            )
        };
        let entities = script_room_entities(
            [-192.0, -192.0, 36.0],
            &format!(
                "{}{}{}",
                // Sounds from the first tick, and the payload has it.
                ambient("ohl_hum", [64.0, 0.0, 48.0], "ohl/one.wav", "0"),
                // Start silent (16), started by name a moment later. The
                // payload does not publish this one, so it must resolve to
                // no channel at all rather than to a guess.
                ambient("ohl_alarm", [-64.0, 0.0, 48.0], "ohl/absent.wav", "16"),
                entity_block(
                    "trigger_auto",
                    [0.0, 0.0, 0.0],
                    0.0,
                    &[("target", "ohl_alarm"), ("delay", "0.1")],
                ),
            ),
        );
        let mut game = script_game(&entities);
        let assets = assets();
        let mut audio = AudioRuntime::silent();

        let mut cues = 0;
        for _ in 0..60 {
            audio.set_listener(game.eye_position(), game.camera().yaw);
            for event in game.tick(TICK_SECONDS, &Input::default()) {
                if let GameEvent::Sound(cue) = event {
                    cues += 1;
                    audio.play(&assets, &cue);
                }
            }
            audio.frame(TICK_SECONDS);
        }

        assert_eq!(cues, 2, "both ambients announced themselves");
        // Both were resolved and both were remembered — one as a playable
        // buffer, one as a known miss.
        assert_eq!(audio.cache().len(), 2);
        // Neither channel is still playing: `ohl/one.wav` is a 64-frame
        // buffer with no loop points, so it ran out, and the absent one
        // never started. What matters is that exactly one of them was
        // ever handed to the mixer.
        let mixer = audio.mixer().lock().expect("lock mixer");
        assert_eq!(mixer.active_channel_count(), 0);
        drop(mixer);

        // Replayed against a fresh mixer without pumping it, the same
        // stream leaves exactly one static channel standing.
        let mut game = script_game(&entities);
        let mut audio = AudioRuntime::silent();
        for _ in 0..60 {
            for event in game.tick(TICK_SECONDS, &Input::default()) {
                if let GameEvent::Sound(cue) = event {
                    audio.play(&assets, &cue);
                }
            }
        }
        let mixer = audio.mixer().lock().expect("lock mixer");
        assert_eq!(
            mixer.active_channel_count(),
            1,
            "one published sound, one the payload does not carry"
        );
    }

    /// A spatialised sound to the player's right is louder in the right
    /// ear, and one far enough away is not heard at all. Rendered through
    /// the same headless sink a smoke run uses.
    #[test]
    fn a_sound_to_the_right_is_rendered_louder_in_the_right_ear() {
        let assets = assets();
        let mut audio = AudioRuntime::silent();
        audio.set_listener([0.0, 0.0, 0.0], 0.0);
        audio.play(
            &assets,
            &SoundCue::new(
                1,
                ChannelClass::Static,
                SoundAsset::file("sound/ohl/one.wav"),
            )
            .at([0.0, -100.0, 0.0], ohl_engine::ATTN_NORM),
        );

        let mut out = vec![0.0f32; 64];
        audio.mixer().lock().expect("lock mixer").render(&mut out);
        let left: f32 = out.iter().step_by(2).map(|sample| sample.abs()).sum();
        let right: f32 = out
            .iter()
            .skip(1)
            .step_by(2)
            .map(|sample| sample.abs())
            .sum();
        assert!(right > left, "left {left}, right {right}");

        // Past the published `ATTN_NORM` radius nothing is audible.
        let mut audio = AudioRuntime::silent();
        audio.set_listener([0.0, 0.0, 0.0], 0.0);
        audio.play(
            &assets,
            &SoundCue::new(
                1,
                ChannelClass::Static,
                SoundAsset::file("sound/ohl/one.wav"),
            )
            .at([0.0, -5_000.0, 0.0], ohl_engine::ATTN_NORM),
        );
        let mut out = vec![0.0f32; 64];
        audio.mixer().lock().expect("lock mixer").render(&mut out);
        assert!(out.iter().all(|sample| sample.abs() < 1e-6));
        let _ = Arc::strong_count(audio.mixer());
    }
}
