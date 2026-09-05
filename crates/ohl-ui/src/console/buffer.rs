//! The console's bounded scrollback buffer.
//!
//! The buffer only ever holds strings the game (or the local player, through
//! the input line) explicitly hands it; it never renders raw bytes read from
//! media or the network. As a second line of defense it still strips control
//! characters other than the newlines used to split multi-line pushes, so a
//! stray escape sequence in a passed string cannot corrupt the terminal or
//! egui's text layout.

use std::collections::VecDeque;

/// Maximum number of lines the scrollback buffer retains. Older lines are
/// dropped first once this bound is reached.
pub const MAX_LINES: usize = 4096;

/// Strips ASCII control characters (everything below `0x20` and `0x7f`)
/// except the newline itself, which the caller splits on before this
/// function ever sees a fragment.
fn sanitize_line(line: &str) -> String {
    line.chars()
        .filter(|character| !character.is_control() || *character == '\t')
        .collect()
}

/// A bounded, append-only log of lines shown in the developer console.
#[derive(Debug, Default)]
pub struct ScrollbackBuffer {
    lines: VecDeque<String>,
}

impl ScrollbackBuffer {
    /// Creates an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: VecDeque::new(),
        }
    }

    /// Appends `text`, splitting on `\n` and sanitizing each resulting line,
    /// then trims the buffer back down to [`MAX_LINES`] from the front.
    pub fn push(&mut self, text: &str) {
        for raw_line in text.split('\n') {
            self.lines.push_back(sanitize_line(raw_line));
            while self.lines.len() > MAX_LINES {
                self.lines.pop_front();
            }
        }
    }

    /// The lines currently retained, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// Number of lines currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the buffer holds no lines.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Discards every retained line.
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LINES, ScrollbackBuffer};

    #[test]
    fn push_splits_multiline_text() {
        let mut buffer = ScrollbackBuffer::new();
        buffer.push("first\nsecond");
        assert_eq!(buffer.lines().collect::<Vec<_>>(), vec!["first", "second"]);
    }

    #[test]
    fn bound_drops_oldest_lines_first() {
        let mut buffer = ScrollbackBuffer::new();
        for index in 0..MAX_LINES + 10 {
            buffer.push(&format!("line {index}"));
        }
        assert_eq!(buffer.len(), MAX_LINES);
        assert_eq!(buffer.lines().next(), Some("line 10"));
        assert_eq!(
            buffer.lines().last(),
            Some(format!("line {}", MAX_LINES + 9)).as_deref()
        );
    }

    #[test]
    fn control_characters_other_than_tab_are_stripped() {
        let mut buffer = ScrollbackBuffer::new();
        buffer.push("bad\u{1b}[31mtext\u{7f}\tok");
        assert_eq!(buffer.lines().next(), Some("bad[31mtext\tok"));
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut buffer = ScrollbackBuffer::new();
        buffer.push("hello");
        assert!(!buffer.is_empty());
        buffer.clear();
        assert!(buffer.is_empty());
    }
}
