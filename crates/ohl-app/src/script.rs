//! The deterministic scripted-input format used by `--script`.
//!
//! See `docs/m79-design.md` §7. The format is project-owned and
//! project-authored (no published source is needed, per that section's
//! "Clean-room" note): a script is plain text, one line per action,
//!
//! ```text
//! # comment
//! 10 forward
//! 2  look 0 -30
//! 1  attack
//! 40 wait
//! 1  use
//! ```
//!
//! `<ticks> <token> [args] [<token> [args] ...]`: a leading tick count
//! (how many fixed simulation steps the rest of the line applies for),
//! followed by one to eight tokens from a closed set. A `#` line, or a
//! blank line, is a comment. Tokens:
//!
//! - `forward`/`back`, `left`/`right`, `up`/`down` — movement axes, held
//!   for every tick of the line.
//! - `jump`, `duck`, `attack`, `attack2` — held buttons, held for every
//!   tick of the line.
//! - `use`, `reload`, `flashlight` — edges (a press, not a hold); applied
//!   on the line's first tick only, exactly like the [`ohl_engine::Input`]
//!   fields they set.
//! - `slot <1..5>` — selects a HUD weapon slot, on the first tick only.
//! - `look <dpitch> <dyaw>` — turns the view by this many degrees over the
//!   line's ticks, spread evenly so a multi-tick turn is smooth rather than
//!   a single snap. Converted to [`ohl_engine::Input::mouse_delta`] through
//!   [`ohl_engine::MOUSE_SENSITIVITY`], the same constant the real input
//!   path scales by, inverted the same way
//!   [`ohl_render::FreeFlyCamera::apply_mouse_delta`] applies it.
//! - `wait` — no-op; holds nothing for the line's ticks.
//! - `guard` — hold this spot and defend it for the line's ticks. Unlike
//!   every other token this one carries no fixed input at all: each of the
//!   line's ticks is expanded, *while the script is running*, by
//!   [`ohl_engine::guard_input`] from the game state at that tick — select
//!   the best carried weapon with ammo, turn toward the nearest hostile
//!   monster in line of sight, fire when aligned, reload when empty, and
//!   otherwise stand still. It must therefore be the only token on its
//!   line ([`ScriptError::GuardNotAlone`]), and a script carrying it is a
//!   closed loop around the simulation rather than a fixed recording. The
//!   loop draws on no randomness of its own, so a guarded script is as
//!   reproducible as any other.
//!
//! Limits (§7): at most 4,096 non-comment lines, at most 100,000 ticks in
//! total, at most 8 tokens on one line. Anything outside the grammar is a
//! parse error with one of the fixed messages in [`ScriptError`]; the
//! parser never panics, including on arbitrary (possibly non-UTF-8) bytes
//! — see the `parse_never_panics_on_arbitrary_bytes` proptest below.

use ohl_engine::{Input, MOUSE_SENSITIVITY};

/// The most script lines (excluding comments and blank lines) a script may
/// contain.
pub const MAX_LINES: usize = 4096;

/// The most simulation ticks a script may schedule in total.
pub const MAX_TOTAL_TICKS: u64 = 100_000;

/// The most tokens (not counting their arguments) one line may carry.
pub const MAX_TOKENS_PER_LINE: usize = 8;

/// Why a script failed to parse.
///
/// Every variant has one fixed [`Display`](std::fmt::Display) message and
/// never carries a byte of the offending line or file: a script is
/// user-supplied, untrusted input, so an error report from it must be safe
/// to log unconditionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptError {
    /// More than [`MAX_LINES`] non-comment lines.
    TooManyLines,
    /// The running tick total exceeded [`MAX_TOTAL_TICKS`].
    TooManyTicks,
    /// A line carried more than [`MAX_TOKENS_PER_LINE`] tokens.
    TooManyTokensOnLine,
    /// A line's leading tick count is missing or not a valid non-negative
    /// integer.
    InvalidTickCount,
    /// A token outside the closed set documented on this module.
    UnknownToken,
    /// A token's arguments were missing, not numeric, non-finite, or (for
    /// `slot`) out of the published `1..=5` range.
    InvalidArguments,
    /// The script contained no scripted ticks at all.
    Empty,
    /// A line combined `guard` with another token. `guard` decides every
    /// button of its own ticks, so there is nothing for another token on
    /// the same line to mean.
    GuardNotAlone,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TooManyLines => "the script has more lines than the documented limit",
            Self::TooManyTicks => "the script schedules more ticks than the documented limit",
            Self::TooManyTokensOnLine => "a script line has more tokens than the documented limit",
            Self::InvalidTickCount => {
                "a script line's tick count is not a valid non-negative integer"
            }
            Self::UnknownToken => "a script line uses a token outside the documented set",
            Self::InvalidArguments => "a script token's arguments are missing or invalid",
            Self::Empty => "the script contains no scripted ticks",
            Self::GuardNotAlone => "a script line combines the guard token with another token",
        })
    }
}

impl std::error::Error for ScriptError {}

/// One scheduled tick: either a fixed [`Input`] the file spelled out, or a
/// tick of the guard loop, whose input only exists once the game state it
/// is computed from does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScriptStep {
    /// Hand this exact input to one [`ohl_engine::Game::tick`] call.
    Fixed(Input),
    /// Ask [`ohl_engine::guard_input`] what to press, from the game as it
    /// is at this tick, and hand *that* to [`ohl_engine::Game::tick`].
    Guard,
}

/// A parsed scripted-input file: one [`ScriptStep`] per simulation tick,
/// in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Script {
    steps: Vec<ScriptStep>,
}

impl Script {
    /// The parsed ticks, in order; one step drives one
    /// [`ohl_engine::Game::tick`] call.
    #[must_use]
    pub fn steps(&self) -> &[ScriptStep] {
        &self.steps
    }

    /// How many ticks this script schedules.
    #[allow(dead_code)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether this script schedules no ticks. Never true for a value
    /// returned by [`Self::parse`], which rejects an empty script.
    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Parses `bytes` per this module's grammar.
    ///
    /// Never panics: invalid UTF-8 is replaced (`String::from_utf8_lossy`)
    /// rather than rejected outright, and every other malformed input is a
    /// fixed [`ScriptError`] instead.
    pub fn parse(bytes: &[u8]) -> Result<Self, ScriptError> {
        let text = String::from_utf8_lossy(bytes);
        let mut steps = Vec::new();
        let mut total_ticks: u64 = 0;
        let mut line_count: usize = 0;

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            line_count += 1;
            if line_count > MAX_LINES {
                return Err(ScriptError::TooManyLines);
            }
            parse_line(line, &mut steps, &mut total_ticks)?;
        }

        if steps.is_empty() {
            return Err(ScriptError::Empty);
        }
        Ok(Self { steps })
    }
}

/// One line's held state, split into what holds for every tick of the
/// line (`base`) and what is an edge, applied on the first tick only
/// (`first_tick_only`).
#[derive(Debug, Clone, Copy, Default)]
struct LineState {
    base: Input,
    first_tick_only: Input,
    look_total: Option<(f32, f32)>,
    /// Set by the `guard` token, which owns its whole line.
    guard: bool,
}

fn parse_line(
    line: &str,
    steps: &mut Vec<ScriptStep>,
    total_ticks: &mut u64,
) -> Result<(), ScriptError> {
    let mut words = line.split_ascii_whitespace();
    let ticks: u32 = words
        .next()
        .ok_or(ScriptError::InvalidTickCount)?
        .parse()
        .map_err(|_| ScriptError::InvalidTickCount)?;

    let mut state = LineState::default();
    let mut token_count: usize = 0;
    while let Some(word) = words.next() {
        token_count += 1;
        if token_count > MAX_TOKENS_PER_LINE {
            return Err(ScriptError::TooManyTokensOnLine);
        }
        apply_token(word, &mut words, &mut state)?;
    }

    *total_ticks = total_ticks.saturating_add(u64::from(ticks));
    if *total_ticks > MAX_TOTAL_TICKS {
        return Err(ScriptError::TooManyTicks);
    }

    if state.guard {
        if token_count > 1 {
            return Err(ScriptError::GuardNotAlone);
        }
        for _ in 0..ticks {
            steps.push(ScriptStep::Guard);
        }
        return Ok(());
    }

    let per_tick_mouse_delta = state.look_total.map(|(dpitch, dyaw)| {
        let divisor = f64::from(ticks.max(1));
        #[allow(clippy::cast_possible_truncation)]
        let dpitch_per_tick = (f64::from(dpitch) / divisor) as f32;
        #[allow(clippy::cast_possible_truncation)]
        let dyaw_per_tick = (f64::from(dyaw) / divisor) as f32;
        // Inverted from `ohl_render::FreeFlyCamera::apply_mouse_delta`:
        // `yaw -= delta_x * sensitivity`, `pitch += delta_y * sensitivity`.
        let delta_x = -dyaw_per_tick / MOUSE_SENSITIVITY;
        let delta_y = dpitch_per_tick / MOUSE_SENSITIVITY;
        (delta_x, delta_y)
    });

    for tick_index in 0..ticks {
        let mut input = state.base;
        if tick_index == 0 {
            input.use_pressed = state.first_tick_only.use_pressed;
            input.reload = state.first_tick_only.reload;
            input.flashlight_pressed = state.first_tick_only.flashlight_pressed;
            input.select_slot = state.first_tick_only.select_slot;
        }
        if let Some(delta) = per_tick_mouse_delta {
            input.mouse_delta = delta;
        }
        steps.push(ScriptStep::Fixed(input));
    }
    Ok(())
}

fn apply_token(
    word: &str,
    words: &mut std::str::SplitAsciiWhitespace<'_>,
    state: &mut LineState,
) -> Result<(), ScriptError> {
    match word {
        "forward" => state.base.forward = 1,
        "back" => state.base.forward = -1,
        "left" => state.base.right = -1,
        "right" => state.base.right = 1,
        "up" => state.base.up = 1,
        "down" => state.base.up = -1,
        "jump" => state.base.jump = true,
        "duck" => state.base.duck = true,
        "attack" => state.base.attack = true,
        "attack2" => state.base.attack2 = true,
        "use" => state.first_tick_only.use_pressed = true,
        "reload" => state.first_tick_only.reload = true,
        "flashlight" => state.first_tick_only.flashlight_pressed = true,
        "wait" => {}
        "guard" => state.guard = true,
        "slot" => {
            let raw = words.next().ok_or(ScriptError::InvalidArguments)?;
            let slot: u8 = raw.parse().map_err(|_| ScriptError::InvalidArguments)?;
            if !(1..=5).contains(&slot) {
                return Err(ScriptError::InvalidArguments);
            }
            state.first_tick_only.select_slot = Some(slot);
        }
        "look" => {
            let dpitch: f32 = words
                .next()
                .ok_or(ScriptError::InvalidArguments)?
                .parse()
                .map_err(|_| ScriptError::InvalidArguments)?;
            let dyaw: f32 = words
                .next()
                .ok_or(ScriptError::InvalidArguments)?
                .parse()
                .map_err(|_| ScriptError::InvalidArguments)?;
            if !dpitch.is_finite() || !dyaw.is_finite() {
                return Err(ScriptError::InvalidArguments);
            }
            state.look_total = Some((dpitch, dyaw));
        }
        _ => return Err(ScriptError::UnknownToken),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The fixed input at `index`, for a test that expects one there.
    fn fixed(script: &Script, index: usize) -> Input {
        match script.steps()[index] {
            ScriptStep::Fixed(input) => input,
            ScriptStep::Guard => panic!("a fixed input was expected at {index}"),
        }
    }

    #[test]
    fn a_guard_line_parses_into_one_guard_step_per_tick() {
        let script = Script::parse(b"2 forward\n3 guard\n").expect("parses");
        assert_eq!(
            script.steps(),
            &[
                ScriptStep::Fixed(Input {
                    forward: 1,
                    ..Input::default()
                }),
                ScriptStep::Fixed(Input {
                    forward: 1,
                    ..Input::default()
                }),
                ScriptStep::Guard,
                ScriptStep::Guard,
                ScriptStep::Guard,
            ]
        );
    }

    #[test]
    fn guard_may_not_share_its_line_with_another_token() {
        let error = Script::parse(b"5 guard forward").unwrap_err();
        assert_eq!(error, ScriptError::GuardNotAlone);
        assert_eq!(
            error.to_string(),
            "a script line combines the guard token with another token"
        );
        assert_eq!(
            Script::parse(b"5 duck guard").unwrap_err(),
            ScriptError::GuardNotAlone
        );
    }

    #[test]
    fn a_zero_tick_guard_line_schedules_nothing() {
        assert_eq!(Script::parse(b"0 guard\n"), Err(ScriptError::Empty));
    }

    #[test]
    fn parses_the_documented_grammar() {
        let script =
            Script::parse(b"# comment\n10 forward\n2  look 0 -30\n1  attack\n40 wait\n1  use\n")
                .expect("the documented example parses");
        assert_eq!(script.len(), 10 + 2 + 1 + 40 + 1);
        assert!(!script.is_empty());
        assert_eq!(fixed(&script, 0).forward, 1);
        assert!(fixed(&script, 12).attack);
        assert!(fixed(&script, 10 + 2 + 1 + 40).use_pressed);
    }

    #[test]
    fn a_blank_or_comment_only_script_is_rejected_as_empty() {
        assert_eq!(
            Script::parse(b"# nothing here\n\n"),
            Err(ScriptError::Empty)
        );
        assert_eq!(Script::parse(b""), Err(ScriptError::Empty));
    }

    #[test]
    fn rejects_an_unknown_token_with_a_fixed_message() {
        let error = Script::parse(b"1 teleport").unwrap_err();
        assert_eq!(error, ScriptError::UnknownToken);
        assert_eq!(
            error.to_string(),
            "a script line uses a token outside the documented set"
        );
    }

    #[test]
    fn rejects_an_invalid_tick_count() {
        assert_eq!(
            Script::parse(b"abc forward").unwrap_err(),
            ScriptError::InvalidTickCount
        );
        assert_eq!(
            Script::parse(b"-1 forward").unwrap_err(),
            ScriptError::InvalidTickCount
        );
    }

    #[test]
    fn rejects_more_than_the_documented_tokens_per_line() {
        let line = "1 forward left up jump duck attack attack2 reload flashlight\n";
        assert_eq!(
            Script::parse(line.as_bytes()).unwrap_err(),
            ScriptError::TooManyTokensOnLine
        );
    }

    #[test]
    fn rejects_more_than_the_documented_line_count() {
        let mut text = String::new();
        for _ in 0..=MAX_LINES {
            text.push_str("1 wait\n");
        }
        assert_eq!(
            Script::parse(text.as_bytes()).unwrap_err(),
            ScriptError::TooManyLines
        );
    }

    #[test]
    fn rejects_more_than_the_documented_total_ticks() {
        let text = format!("{} wait\n", MAX_TOTAL_TICKS + 1);
        assert_eq!(
            Script::parse(text.as_bytes()).unwrap_err(),
            ScriptError::TooManyTicks
        );
    }

    #[test]
    fn rejects_a_slot_out_of_the_published_range() {
        assert_eq!(
            Script::parse(b"1 slot 6").unwrap_err(),
            ScriptError::InvalidArguments
        );
        assert_eq!(
            Script::parse(b"1 slot 0").unwrap_err(),
            ScriptError::InvalidArguments
        );
    }

    #[test]
    fn rejects_a_non_finite_look_argument() {
        assert_eq!(
            Script::parse(b"1 look nan 0").unwrap_err(),
            ScriptError::InvalidArguments
        );
    }

    #[test]
    fn look_spreads_the_turn_evenly_across_the_lines_ticks() {
        let script = Script::parse(b"4 look 0 -40\n").expect("parses");
        assert_eq!(script.len(), 4);
        let first = fixed(&script, 0).mouse_delta;
        for index in 0..script.len() {
            assert_eq!(
                fixed(&script, index).mouse_delta,
                first,
                "an even turn is the same every tick"
            );
        }
    }

    proptest! {
        /// The parser never panics, on any byte sequence at all: not on
        /// invalid UTF-8, not on adversarial numbers, not on a token list
        /// designed to overflow a counter.
        #[test]
        fn parse_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = Script::parse(&bytes);
        }
    }
}
