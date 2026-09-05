//! `sprites/hud.txt` and `sprites/weapon_*.txt` HUD sprite-sheet layout
//! declarations (identical grammar, shared parser).
//!
//! See `docs/FORMAT_SOURCES.md` ("Game text formats") for the public
//! documentation this module was implemented from (TWHL "hud.txt and
//! weapon_*.txt").
//!
//! Grammar (bounded summary): an optional leading count line (a bare
//! decimal integer, informational only — not required to match the actual
//! number of rows that follow), then one row per non-blank, non-comment
//! line: `object resolution filename x y w h`, seven whitespace-separated
//! fields where `resolution` is conventionally `320`/`640` (community and
//! 25th-anniversary updates add `1280`/`2560`) and the remaining four
//! fields are the sprite-sheet offset and size. `//` starts a whole-line
//! comment.

use alloc::vec::Vec;

use crate::error::{FormatError, Result};
use crate::text_lines::{Lines, split_ws, trim_ascii};

/// Bounds enforced while parsing a `hud.txt`/`weapon_*.txt` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest whole-file size this parser will look at.
    pub max_bytes: usize,
    /// The largest number of physical lines this parser will scan.
    pub max_lines: usize,
    /// The largest number of sprite rows this parser will collect.
    pub max_rows: usize,
}

impl Limits {
    /// Conservative defaults, generous enough for the shipping sprite
    /// declaration files (at most a few hundred rows) but far below what
    /// would let a malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_bytes: 256 * 1024,
            max_lines: 16_384,
            max_rows: 4_096,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// One parsed sprite-sheet row.
#[derive(Debug, Clone, Copy)]
pub struct SpriteRow<'a> {
    /// The declared object/icon name (for example `weapon_crowbar`).
    pub name: &'a str,
    /// The resolution bucket this row applies to (`320`, `640`, ...).
    pub resolution: i64,
    /// The `.spr` sheet file this row's rectangle is taken from.
    pub sprite_file: &'a str,
    /// Left offset of the rectangle within the sprite sheet.
    pub x: i64,
    /// Top offset of the rectangle within the sprite sheet.
    pub y: i64,
    /// Rectangle width.
    pub w: i64,
    /// Rectangle height.
    pub h: i64,
}

/// A parsed sprite declaration file: an optional informational row count,
/// plus zero or more rows in file order.
#[derive(Debug, Clone, Default)]
pub struct SpriteList<'a> {
    /// The declared count from the leading count line, if present and
    /// parsed successfully. This is informational only: it is not checked
    /// against the number of rows actually parsed, since real files are
    /// tolerated even when the header is stale.
    pub declared_count: Option<usize>,
    rows: Vec<SpriteRow<'a>>,
}

impl<'a> SpriteList<'a> {
    /// Every parsed row, in file order.
    #[must_use]
    pub fn rows(&self) -> &[SpriteRow<'a>] {
        &self.rows
    }
}

fn parse_i64(bytes: &[u8]) -> Option<i64> {
    core::str::from_utf8(bytes).ok()?.parse::<i64>().ok()
}

/// Parses a `hud.txt`/`weapon_*.txt` file.
///
/// Never panics on malformed input: a leading count line that does not
/// parse as a bare integer is treated as the first data row instead (its
/// absence is not an error); a row with fewer than seven fields, or with a
/// non-numeric numeric field, is skipped rather than reported as an error.
/// Returns [`FormatError::LimitExceeded`] if `data`, the line count, the
/// row count, or a declared count that is clearly implausible exceeds
/// `limits`.
pub fn parse<'a>(data: &'a [u8], limits: &Limits) -> Result<SpriteList<'a>> {
    if data.len() > limits.max_bytes {
        return Err(FormatError::LimitExceeded);
    }

    let mut lines = Lines::new(data, limits.max_lines);
    let mut declared_count = None;

    // Look at the first non-blank, non-comment line: if it is a single bare
    // integer token, treat it as the informational row count and consume
    // it; otherwise leave it to be parsed as an ordinary row below.
    let mut first_data_line = None;
    while let Some(line) = lines.next_bounded()? {
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() || trimmed.starts_with(b"//") {
            continue;
        }
        let mut tokens = split_ws(trimmed);
        let first = tokens.next();
        if let (Some(first), None) = (first, tokens.next())
            && let Some(count) = parse_i64(first)
        {
            if let Ok(count) = usize::try_from(count) {
                if count > limits.max_rows {
                    return Err(FormatError::LimitExceeded);
                }
                declared_count = Some(count);
            }
            break;
        }
        first_data_line = Some(trimmed);
        break;
    }

    let mut rows = Vec::new();
    if let Some(line) = first_data_line
        && let Some(row) = parse_row(line)
    {
        rows.push(row);
    }

    while let Some(line) = lines.next_bounded()? {
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() || trimmed.starts_with(b"//") {
            continue;
        }
        if rows.len() >= limits.max_rows {
            return Err(FormatError::LimitExceeded);
        }
        if let Some(row) = parse_row(trimmed) {
            rows.push(row);
        }
    }

    Ok(SpriteList {
        declared_count,
        rows,
    })
}

fn parse_row(line: &[u8]) -> Option<SpriteRow<'_>> {
    let mut tokens = split_ws(line);
    let name = tokens.next()?;
    let resolution = tokens.next()?;
    let sprite_file = tokens.next()?;
    let x = tokens.next()?;
    let y = tokens.next()?;
    let w = tokens.next()?;
    let h = tokens.next()?;

    let name = core::str::from_utf8(name).ok()?;
    let sprite_file = core::str::from_utf8(sprite_file).ok()?;
    let resolution = parse_i64(resolution)?;
    let x = parse_i64(x)?;
    let y = parse_i64(y)?;
    let w = parse_i64(w)?;
    let h = parse_i64(h)?;

    Some(SpriteRow {
        name,
        resolution,
        sprite_file,
        x,
        y,
        w,
        h,
    })
}
