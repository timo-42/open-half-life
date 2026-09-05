//! `titles.txt` HUD message/caption definitions.
//!
//! See `docs/FORMAT_SOURCES.md` ("Game text formats") for the public
//! documentation this module was implemented from
//! (developer.valvesoftware.com/wiki/Titles.txt).
//!
//! Grammar (bounded summary): directive lines beginning with `$` set state
//! (`$position`, `$effect`, `$color`, `$color2`, `$fadein`, `$fadeout`,
//! `$holdtime`, `$fxtime`) that applies to every following message block
//! until a directive of the same kind reoccurs; a message block is a
//! `NAME` line, a lone `{` line, zero or more lines of raw caption text,
//! and a closing lone `}` line. Message text bytes are kept as-is (not
//! required to be valid UTF-8, matching the code-page-dependent text some
//! `titles.txt` files ship with); both `\n` and `\r\n` line endings are
//! accepted.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{FormatError, Result};
use crate::text_lines::{Lines, split_ws, trim_ascii};

/// Bounds enforced while parsing a `titles.txt` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest whole-file size this parser will look at.
    pub max_bytes: usize,
    /// The largest number of physical lines this parser will scan.
    pub max_lines: usize,
    /// The largest number of message blocks this parser will collect.
    pub max_messages: usize,
    /// The largest total text size (sum of line lengths) for one message
    /// block's body.
    pub max_message_bytes: usize,
}

impl Limits {
    /// Conservative defaults, generous enough for the shipping `titles.txt`
    /// (a few hundred short messages) but far below what would let a
    /// malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024,
            max_lines: 65_536,
            max_messages: 8_192,
            max_message_bytes: 16 * 1024,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// The directive state in effect for a message block (each field is `None`
/// until the corresponding directive is first seen).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DirectiveState {
    /// `$position x y` (normalized screen-space coordinates).
    pub position: Option<(f32, f32)>,
    /// `$effect n`.
    pub effect: Option<i32>,
    /// `$color r g b`.
    pub color: Option<(u8, u8, u8)>,
    /// `$color2 r g b`.
    pub color2: Option<(u8, u8, u8)>,
    /// `$fadein seconds`.
    pub fadein: Option<f32>,
    /// `$fadeout seconds`.
    pub fadeout: Option<f32>,
    /// `$holdtime seconds`.
    pub holdtime: Option<f32>,
    /// `$fxtime seconds`.
    pub fxtime: Option<f32>,
}

/// One parsed `NAME { ... }` message block.
#[derive(Debug, Clone)]
pub struct Message<'a> {
    /// The block's name (the key `game_text`/`ClientPrint` and
    /// `CHAPTER<N>_TITLE` entries are looked up by).
    pub name: &'a str,
    /// The raw text lines between the block's `{` and `}` lines, in file
    /// order, exactly as they appear (not re-encoded; a caller wanting the
    /// full body joined by `\n` can `text_lines.join(...)`).
    pub text_lines: Vec<&'a [u8]>,
    /// The directive state in effect when this block was parsed.
    pub state: DirectiveState,
}

/// A parsed `titles.txt` file: zero or more messages, in file order.
#[derive(Debug, Clone, Default)]
pub struct TitleFile<'a> {
    messages: Vec<Message<'a>>,
}

impl<'a> TitleFile<'a> {
    /// Every parsed message block, in file order.
    #[must_use]
    pub fn messages(&self) -> &[Message<'a>] {
        &self.messages
    }

    /// Finds a message block by case-insensitive name.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Message<'a>> {
        self.messages
            .iter()
            .find(|message| message.name.eq_ignore_ascii_case(name))
    }
}

fn parse_f32(bytes: &[u8]) -> Option<f32> {
    core::str::from_utf8(bytes).ok()?.parse::<f32>().ok()
}

fn parse_u8(bytes: &[u8]) -> Option<u8> {
    core::str::from_utf8(bytes).ok()?.parse::<u8>().ok()
}

fn parse_i32(bytes: &[u8]) -> Option<i32> {
    core::str::from_utf8(bytes).ok()?.parse::<i32>().ok()
}

/// Applies one directive line (already known to start with `$`) to `state`.
/// Unknown directive names, and directives with the wrong argument count or
/// unparsable arguments, are silently ignored (state is left unchanged);
/// this parser never fails on a malformed directive.
fn apply_directive(state: &mut DirectiveState, line: &[u8]) {
    let mut tokens = split_ws(line);
    let Some(keyword) = tokens.next() else {
        return;
    };
    let args: Vec<&[u8]> = tokens.collect();
    match keyword {
        b"$position" => {
            if let [x, y] = args[..]
                && let (Some(x), Some(y)) = (parse_f32(x), parse_f32(y))
            {
                state.position = Some((x, y));
            }
        }
        b"$effect" => {
            if let [value] = args[..]
                && let Some(value) = parse_i32(value)
            {
                state.effect = Some(value);
            }
        }
        b"$color" => {
            if let [r, g, b] = args[..]
                && let (Some(r), Some(g), Some(b)) = (parse_u8(r), parse_u8(g), parse_u8(b))
            {
                state.color = Some((r, g, b));
            }
        }
        b"$color2" => {
            if let [r, g, b] = args[..]
                && let (Some(r), Some(g), Some(b)) = (parse_u8(r), parse_u8(g), parse_u8(b))
            {
                state.color2 = Some((r, g, b));
            }
        }
        b"$fadein" => {
            if let [value] = args[..]
                && let Some(value) = parse_f32(value)
            {
                state.fadein = Some(value);
            }
        }
        b"$fadeout" => {
            if let [value] = args[..]
                && let Some(value) = parse_f32(value)
            {
                state.fadeout = Some(value);
            }
        }
        b"$holdtime" => {
            if let [value] = args[..]
                && let Some(value) = parse_f32(value)
            {
                state.holdtime = Some(value);
            }
        }
        b"$fxtime" => {
            if let [value] = args[..]
                && let Some(value) = parse_f32(value)
            {
                state.fxtime = Some(value);
            }
        }
        _ => {}
    }
}

/// Parses a `titles.txt` file.
///
/// Never panics on malformed input: an unknown or malformed directive
/// leaves the directive state unchanged, a name line not followed by a
/// lone `{` line is dropped (not treated as a message), and an
/// unterminated block at end-of-input is dropped rather than reported as
/// an error. Returns [`FormatError::LimitExceeded`] if `data`, the line
/// count, the message count, or one message's text exceeds `limits`.
pub fn parse<'a>(data: &'a [u8], limits: &Limits) -> Result<TitleFile<'a>> {
    if data.len() > limits.max_bytes {
        return Err(FormatError::LimitExceeded);
    }

    let mut messages = Vec::new();
    let mut state = DirectiveState::default();
    let mut lines = Lines::new(data, limits.max_lines);

    while let Some(line) = lines.next_bounded()? {
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() {
            continue;
        }
        if trimmed[0] == b'$' {
            apply_directive(&mut state, trimmed);
            continue;
        }

        let Ok(name) = core::str::from_utf8(trimmed) else {
            continue;
        };

        // The next non-blank line must be a lone "{" to open a block;
        // anything else means `name` was not actually a message header.
        let mut saw_open_brace = false;
        while let Some(candidate) = lines.next_bounded()? {
            let candidate = trim_ascii(candidate);
            if candidate.is_empty() {
                continue;
            }
            saw_open_brace = candidate == b"{";
            break;
        }
        if !saw_open_brace {
            continue;
        }

        let mut text_lines = Vec::new();
        let mut text_bytes = 0usize;
        let mut closed = false;
        while let Some(line) = lines.next_bounded()? {
            if trim_ascii(line) == b"}" {
                closed = true;
                break;
            }
            text_bytes = text_bytes.saturating_add(line.len());
            if text_bytes > limits.max_message_bytes {
                return Err(FormatError::LimitExceeded);
            }
            text_lines.push(line);
        }
        if !closed {
            // Unterminated block at end-of-input: drop it and stop, rather
            // than reporting a hard parse error over otherwise-valid data
            // collected so far.
            break;
        }

        if messages.len() >= limits.max_messages {
            return Err(FormatError::LimitExceeded);
        }
        messages.push(Message {
            name,
            text_lines,
            state,
        });
    }

    Ok(TitleFile { messages })
}

impl Message<'_> {
    /// Joins [`Message::text_lines`] with `\n`, replacing any byte that is
    /// not valid UTF-8 with `\u{FFFD}` (message text is not guaranteed to be
    /// UTF-8; this is a lossy convenience for callers that only need a
    /// displayable approximation).
    #[must_use]
    pub fn text_lossy(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.text_lines.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&alloc::string::String::from_utf8_lossy(line));
        }
        out
    }
}
