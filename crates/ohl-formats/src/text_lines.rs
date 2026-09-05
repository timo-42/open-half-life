//! Bounded, allocation-free line splitting shared by every line-oriented
//! text format decoder in this crate (`titles.txt`, `sentences.txt`,
//! `skill.cfg`, `liblist.gam`, `sprites/hud.txt` and
//! `sprites/weapon_*.txt`).
//!
//! Every one of those formats is plain, line-oriented text; per the M8.1
//! assignment the bytes are treated as bytes (not necessarily valid UTF-8:
//! `titles.txt` message text in particular may use an arbitrary code page),
//! and both bare `\n` and `\r\n` line endings are tolerated by stripping one
//! trailing `\r` from each line.

use crate::error::{FormatError, Result};

/// Bounded iterator over the lines of a byte buffer.
///
/// This is deliberately not a plain [`Iterator`]: a malformed/adversarial
/// file must never be able to force unbounded looping, so every call is
/// counted against a caller-supplied ceiling and returns
/// [`FormatError::LimitExceeded`] once exceeded instead of yielding forever.
pub(crate) struct Lines<'a> {
    remaining: &'a [u8],
    limit: usize,
    yielded: usize,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(data: &'a [u8], limit: usize) -> Self {
        Self {
            remaining: data,
            limit,
            yielded: 0,
        }
    }

    /// Returns the next line (without its terminator), `Ok(None)` at end of
    /// input, or `Err(FormatError::LimitExceeded)` once `limit` lines have
    /// already been yielded.
    pub(crate) fn next_bounded(&mut self) -> Result<Option<&'a [u8]>> {
        if self.remaining.is_empty() {
            return Ok(None);
        }
        if self.yielded >= self.limit {
            return Err(FormatError::LimitExceeded);
        }
        self.yielded += 1;
        let line = if let Some(pos) = self.remaining.iter().position(|&b| b == b'\n') {
            let (line, rest) = self.remaining.split_at(pos);
            // `rest[0]` is the `\n` itself; skip it. `pos < len`, so `rest`
            // is non-empty and this slice is always in bounds.
            self.remaining = &rest[1..];
            line
        } else {
            let line = self.remaining;
            self.remaining = &[];
            line
        };
        Ok(Some(line.strip_suffix(b"\r").unwrap_or(line)))
    }
}

/// Trims ASCII whitespace (space, tab, and friends) from both ends of
/// `bytes`, without requiring `bytes` to be valid UTF-8.
pub(crate) fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &bytes[start..end]
}

/// Splits `line` on runs of ASCII spaces/tabs, dropping empty fields.
pub(crate) fn split_ws(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|&b| b == b' ' || b == b'\t')
        .filter(|field| !field.is_empty())
}

/// Splits `line` into the substring before the first `"` / `'`-delimited
/// quoted field, and the contents of that quoted field itself (without the
/// quotes). Returns `None` if `line` does not contain a matching pair of
/// double quotes.
pub(crate) fn quoted_field(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let start = line.iter().position(|&b| b == b'"')?;
    let after_start = start + 1;
    let rel_end = line[after_start..].iter().position(|&b| b == b'"')?;
    let end = after_start + rel_end;
    Some((&line[..start], &line[after_start..end]))
}
