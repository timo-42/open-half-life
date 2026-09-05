//! `sentences.txt` NPC/`scripted_sentence`/`speaker` sentence definitions.
//!
//! See `docs/FORMAT_SOURCES.md` ("Game text formats") for the public
//! documentation this module was implemented from
//! (developer.valvesoftware.com/wiki/Sentences.txt,
//! `Scripted_sentence_(GoldSrc)`, `Speaker_(GoldSrc)`).
//!
//! Grammar (bounded summary): one sentence per non-blank, non-comment line,
//! `NAME word1 word2(p95) word3(v80,p110) ...`; the first whitespace
//! separated token is the sentence name, every following token is a word
//! (a WAV path fragment) optionally followed directly by a parenthesized,
//! comma-separated list of modifiers, each a single letter (`p` pitch, `t`
//! time/`text` index, `s` start, `e` end, `v` volume) and a decimal
//! integer. Lines whose first non-whitespace characters are `//` are
//! comments and are skipped.

use alloc::vec::Vec;

use crate::error::{FormatError, Result};
use crate::text_lines::{Lines, split_ws, trim_ascii};

/// Bounds enforced while parsing a `sentences.txt` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest whole-file size this parser will look at.
    pub max_bytes: usize,
    /// The largest number of physical lines this parser will scan.
    pub max_lines: usize,
    /// The largest number of sentence entries this parser will collect.
    pub max_sentences: usize,
    /// The largest number of words a single sentence may declare.
    pub max_words_per_sentence: usize,
}

impl Limits {
    /// Conservative defaults, generous enough for the shipping
    /// `sentences.txt` (on the order of a thousand short entries) but far
    /// below what would let a malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024,
            max_lines: 65_536,
            max_sentences: 16_384,
            max_words_per_sentence: 256,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// One `(letter, number)` modifier parsed out of a word's parenthesized
/// suffix, matching the `p`/`t`/`s`/`e`/`v` letters documented for
/// `sentences.txt`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WordModifiers {
    /// `p<n>`: pitch.
    pub pitch: Option<i32>,
    /// `t<n>`: time/text index.
    pub time: Option<i32>,
    /// `s<n>`: start offset.
    pub start: Option<i32>,
    /// `e<n>`: end offset.
    pub end: Option<i32>,
    /// `v<n>`: volume.
    pub volume: Option<i32>,
}

/// One word token within a sentence.
#[derive(Debug, Clone, Copy)]
pub struct Word<'a> {
    /// The word text before any parenthesized modifier suffix (a WAV path
    /// fragment, or a wildcard group token such as `V_DISTS`).
    pub token: &'a str,
    /// Any modifiers parsed from a trailing `(...)` suffix.
    pub modifiers: WordModifiers,
}

/// One parsed sentence entry.
#[derive(Debug, Clone)]
pub struct Sentence<'a> {
    /// The sentence's name (looked up by `scripted_sentence`/`speaker`).
    pub name: &'a str,
    /// The sentence's word tokens, in order.
    pub words: Vec<Word<'a>>,
}

/// A parsed `sentences.txt` file: zero or more sentences, in file order.
#[derive(Debug, Clone, Default)]
pub struct SentenceFile<'a> {
    sentences: Vec<Sentence<'a>>,
}

impl<'a> SentenceFile<'a> {
    /// Every parsed sentence, in file order.
    #[must_use]
    pub fn sentences(&self) -> &[Sentence<'a>] {
        &self.sentences
    }

    /// Finds a sentence by case-insensitive name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Sentence<'a>> {
        self.sentences
            .iter()
            .find(|sentence| sentence.name.eq_ignore_ascii_case(name))
    }
}

/// Applies one `letter<digits>` modifier to `modifiers`. Unknown letters
/// and unparsable numbers are silently ignored.
fn apply_modifier(modifiers: &mut WordModifiers, token: &[u8]) {
    let Some((&letter, digits)) = token.split_first() else {
        return;
    };
    let Ok(digits) = core::str::from_utf8(digits) else {
        return;
    };
    let Ok(value) = digits.parse::<i32>() else {
        return;
    };
    match letter.to_ascii_lowercase() {
        b'p' => modifiers.pitch = Some(value),
        b't' => modifiers.time = Some(value),
        b's' => modifiers.start = Some(value),
        b'e' => modifiers.end = Some(value),
        b'v' => modifiers.volume = Some(value),
        _ => {}
    }
}

/// Parses one word token, splitting off an optional trailing `(...)`
/// modifier group. Malformed/unterminated parentheses fall back to
/// treating the whole token as the word text with no modifiers, never
/// failing.
fn parse_word(token: &[u8]) -> Option<Word<'_>> {
    if token.is_empty() {
        return None;
    }
    let (text, modifier_str) = match token.iter().position(|&b| b == b'(') {
        Some(open) => {
            let rest = &token[open + 1..];
            let body = rest.strip_suffix(b")").unwrap_or(rest);
            (&token[..open], body)
        }
        None => (token, &[][..]),
    };
    let token_str = core::str::from_utf8(text).ok()?;
    let mut modifiers = WordModifiers::default();
    if !modifier_str.is_empty() {
        for part in modifier_str.split(|&b| b == b',') {
            if !part.is_empty() {
                apply_modifier(&mut modifiers, part);
            }
        }
    }
    Some(Word {
        token: token_str,
        modifiers,
    })
}

/// Parses a `sentences.txt` file.
///
/// Never panics on malformed input: a line that is not valid UTF-8, or has
/// no name token, is skipped; a word token whose text is not valid UTF-8 is
/// skipped (the rest of the line still parses); an unparsable modifier is
/// ignored. Returns [`FormatError::LimitExceeded`] if `data`, the line
/// count, the sentence count, or one sentence's word count exceeds
/// `limits`.
pub fn parse<'a>(data: &'a [u8], limits: &Limits) -> Result<SentenceFile<'a>> {
    if data.len() > limits.max_bytes {
        return Err(FormatError::LimitExceeded);
    }

    let mut sentences = Vec::new();
    let mut lines = Lines::new(data, limits.max_lines);

    while let Some(line) = lines.next_bounded()? {
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() || trimmed.starts_with(b"//") {
            continue;
        }

        let mut tokens = split_ws(trimmed);
        let Some(name) = tokens.next() else {
            continue;
        };
        let Ok(name) = core::str::from_utf8(name) else {
            continue;
        };

        let mut words = Vec::new();
        for token in tokens {
            if words.len() >= limits.max_words_per_sentence {
                return Err(FormatError::LimitExceeded);
            }
            if let Some(word) = parse_word(token) {
                words.push(word);
            }
        }

        if sentences.len() >= limits.max_sentences {
            return Err(FormatError::LimitExceeded);
        }
        sentences.push(Sentence { name, words });
    }

    Ok(SentenceFile { sentences })
}
