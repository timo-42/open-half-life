//! Chapter titles, HUD messages and sentence lookups.
//!
//! `titles.txt` and `sentences.txt` are already decoded, bounded and
//! never-panicking in [`ohl_formats`]; this module owns the *game* view of
//! them: a `titles.txt` entry becomes a [`MessageBlock`] with the fade/hold
//! timings the HUD needs, and a `sentences.txt` entry becomes the ordered
//! list of sound assets a later audio pass will play. See
//! `docs/FORMAT_SOURCES.md` ("Campaign flow") for the sources.

use std::collections::BTreeMap;

use ohl_formats::{sentences, titles};

use crate::assets::AssetSource;

/// Where `titles.txt` lives inside a mod directory.
pub const TITLES_PATH: &str = "titles.txt";

/// Where `sentences.txt` lives inside a mod directory.
pub const SENTENCES_PATH: &str = "sound/sentences.txt";

/// Where `skill.cfg` lives inside a mod directory.
pub const SKILL_PATH: &str = "skill.cfg";

/// How long a message with no `$holdtime` directive and no entity override
/// is shown for. A project-chosen fallback: the public `titles.txt`
/// documentation describes the directive but not an engine default.
pub const DEFAULT_HOLD_SECONDS: f32 = 4.0;

/// One resolved HUD message: the text to show and how long to show it.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageBlock {
    /// The `titles.txt` entry name, or the `game_text` entity's name when
    /// the text was literal.
    pub name: String,
    /// The text itself, already decoded lossily from the file's bytes.
    pub text: String,
    /// `$fadein` seconds.
    pub fadein: f32,
    /// `$fadeout` seconds.
    pub fadeout: f32,
    /// `$holdtime` seconds.
    pub holdtime: f32,
    /// `$color`, as `r g b`.
    pub color: [u8; 3],
    /// `$position`, when the entry sets one.
    pub position: Option<(f32, f32)>,
}

impl MessageBlock {
    /// A literal block with default timings, for `game_text` and for the
    /// chapter title.
    #[must_use]
    pub fn literal(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
            fadein: 0.0,
            fadeout: 0.0,
            holdtime: DEFAULT_HOLD_SECONDS,
            color: [255, 255, 255],
            position: None,
        }
    }

    /// How long the HUD should keep this message up in total: the fade in,
    /// the hold, and the fade out.
    #[must_use]
    pub fn total_seconds(&self) -> f32 {
        (self.fadein.max(0.0) + self.holdtime.max(0.0) + self.fadeout.max(0.0)).max(0.0)
    }

    /// Applies an `env_message`/`game_text` entity's own timing overrides.
    #[must_use]
    pub fn with_overrides(mut self, message: &ohl_game::registry::Message) -> Self {
        if let Some(fadein) = message.fadein.filter(|value| value.is_finite()) {
            self.fadein = fadein;
        }
        if let Some(fadeout) = message.fadeout.filter(|value| value.is_finite()) {
            self.fadeout = fadeout;
        }
        if let Some(holdtime) = message.holdtime.filter(|value| value.is_finite()) {
            self.holdtime = holdtime;
        }
        self
    }
}

/// Every message a `titles.txt` publishes, owned so it outlives the bytes
/// it was parsed from.
#[derive(Debug, Clone, Default)]
pub struct TitleLibrary {
    blocks: BTreeMap<String, MessageBlock>,
}

impl TitleLibrary {
    /// An empty library, used when the payload publishes no `titles.txt`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a `titles.txt`, returning an empty library when the bytes do
    /// not parse (a missing or malformed file only costs captions).
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let limits = titles::Limits::conservative();
        let Ok(file) = titles::parse(bytes, &limits) else {
            return Self::new();
        };
        let mut blocks = BTreeMap::new();
        for message in file.messages() {
            let state = &message.state;
            blocks.insert(
                message.name.to_ascii_uppercase(),
                MessageBlock {
                    name: message.name.to_string(),
                    text: message.text_lossy(),
                    fadein: state.fadein.unwrap_or(0.0),
                    fadeout: state.fadeout.unwrap_or(0.0),
                    holdtime: state.holdtime.unwrap_or(DEFAULT_HOLD_SECONDS),
                    color: state.color.map_or([255, 255, 255], |(r, g, b)| [r, g, b]),
                    position: state.position,
                },
            );
        }
        Self { blocks }
    }

    /// Reads `titles.txt` through `source`.
    #[must_use]
    pub fn load(source: &dyn AssetSource) -> Self {
        source
            .read(TITLES_PATH)
            .map_or_else(Self::new, |bytes| Self::from_bytes(&bytes))
    }

    /// The message named `name`, matched case-insensitively the way the
    /// entity keyvalues that reference it are authored.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&MessageBlock> {
        self.blocks.get(&name.to_ascii_uppercase())
    }

    /// How many messages the library holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether the library is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Resolves an `env_message`/`game_text` component into the block the
    /// HUD shows: the literal text for `game_text`, the named `titles.txt`
    /// entry for `env_message`, and (when the entry is missing) the name
    /// itself so a caption is never silently lost.
    #[must_use]
    pub fn resolve(&self, message: &ohl_game::registry::Message) -> MessageBlock {
        if message.literal {
            return MessageBlock::literal("game_text", message.message.clone())
                .with_overrides(message);
        }
        self.find(&message.message)
            .cloned()
            .unwrap_or_else(|| MessageBlock::literal(&message.message, message.message.clone()))
            .with_overrides(message)
    }
}

/// A mod-relative asset path, as [`AssetSource`] resolves them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AssetPath(pub String);

impl AssetPath {
    /// The path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The `sentences.txt` name -> word-sample lookup used by the HEV suit and
/// scientist voice lines.
///
/// This is lookup only: nothing here reads, decodes or plays a sound.
#[derive(Debug, Clone, Default)]
pub struct SentenceLookup {
    sentences: BTreeMap<String, Vec<String>>,
}

impl SentenceLookup {
    /// An empty lookup.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a `sentences.txt`, returning an empty lookup when the bytes
    /// do not parse.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let limits = sentences::Limits::conservative();
        let Ok(file) = sentences::parse(bytes, &limits) else {
            return Self::new();
        };
        let mut lookup = BTreeMap::new();
        for sentence in file.sentences() {
            lookup.insert(
                sentence.name.to_ascii_uppercase(),
                sentence
                    .words
                    .iter()
                    .map(|word| word.token.to_string())
                    .collect(),
            );
        }
        Self { sentences: lookup }
    }

    /// Reads `sentences.txt` through `source`.
    #[must_use]
    pub fn load(source: &dyn AssetSource) -> Self {
        source
            .read(SENTENCES_PATH)
            .map_or_else(Self::new, |bytes| Self::from_bytes(&bytes))
    }

    /// The sound assets one sentence names, in speaking order.
    ///
    /// A word token is a `sound/`-relative WAV path without its extension
    /// (`vox/hello` becomes `sound/vox/hello.wav`). Group/wildcard tokens
    /// (the documented `V_DISTS`-style expansions) are returned unexpanded:
    /// selecting from a group is a playback-time decision this lookup does
    /// not make.
    #[must_use]
    pub fn words(&self, name: &str) -> Vec<AssetPath> {
        self.sentences
            .get(&name.to_ascii_uppercase())
            .map(|words| {
                words
                    .iter()
                    .map(|word| AssetPath(format!("sound/{word}.wav")))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many sentences the lookup holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sentences.len()
    }

    /// Whether the lookup is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sentences.is_empty()
    }
}

/// Reads `skill.cfg` through `source` into the campaign crate's
/// difficulty-aware table, returning an empty table when the payload does
/// not publish one or its bytes do not parse.
#[must_use]
pub fn load_skill_table(source: &dyn AssetSource) -> ohl_campaign::SkillTable {
    let Some(bytes) = source.read(SKILL_PATH) else {
        return ohl_campaign::SkillTable::default();
    };
    let limits = ohl_formats::skill_cfg::Limits::conservative();
    let Ok(cfg) = ohl_formats::skill_cfg::parse(&bytes, &limits) else {
        return ohl_campaign::SkillTable::default();
    };
    ohl_campaign::SkillTable::from_entries(
        cfg.entries().iter().map(|entry| (entry.cvar, entry.value)),
        &ohl_campaign::Limits::conservative(),
    )
    .unwrap_or_default()
}
