//! The in-game heads-up display: health, armor, ammo, crosshair, damage
//! flash and the message/title area used later by `env_message` and
//! `game_text`, plus the deathmatch extras: frag count, match clock, kill
//! feed and a persistent centre notice.

use egui::{Align2, Color32, FontId, Pos2, Rect, Vec2};

use crate::scoreboard::format_clock;

/// How many kill-feed lines are kept (and drawn) at once; pushing more drops
/// the oldest.
pub const KILL_FEED_CAPACITY: usize = 5;

/// How long a kill-feed line stays on screen, in seconds.
pub const KILL_FEED_SECONDS: f32 = 6.0;

/// A message shown in the HUD's title/message area for a limited time.
#[derive(Debug, Clone, Default)]
pub struct HudMessage {
    /// The text to display. Already sanitized by whatever produced it
    /// (`env_message`/`game_text` handling upstream); the HUD does not
    /// interpret escape sequences.
    pub text: String,
    /// Seconds remaining before the message is cleared.
    pub seconds_remaining: f32,
}

/// One line of the kill feed, for example who fragged whom.
#[derive(Debug, Clone, PartialEq)]
pub struct KillFeedEntry {
    /// The line to display, already composed by the host.
    pub text: String,
    /// Seconds remaining before the line expires. The last second fades out.
    pub seconds_remaining: f32,
    /// Whether the local player took part in the event; such lines are drawn
    /// in a highlight colour.
    pub local_involved: bool,
}

/// Data-driven HUD state, updated once per frame by the game and read by
/// [`draw`]. Values arrive already clamped by the gameplay layer; the HUD
/// itself only clamps what it needs to keep the numerals and flash sane to
/// draw.
#[derive(Debug, Clone)]
pub struct HudState {
    /// Current health; may be negative transiently (death) but is clamped to
    /// `0` for display.
    pub health: i32,
    /// Current armor, clamped to `0` for display.
    pub armor: i32,
    /// Ammo in the current magazine/clip, if the active weapon uses one.
    pub clip_ammo: Option<i32>,
    /// Reserve ammo for the active weapon's ammo type, if any.
    pub reserve_ammo: Option<i32>,
    /// `0.0` for no flash, ramping to `1.0` immediately after taking damage
    /// and decaying back to `0.0`; the draw call does not animate this
    /// itself, the caller updates it every frame.
    pub damage_flash: f32,
    /// The current title/message, if one is showing.
    pub message: Option<HudMessage>,
    /// Whether the crosshair is drawn (hidden while a menu or console has
    /// input focus, or the player has no weapon out).
    pub show_crosshair: bool,
    /// The local player's frag count, drawn near the top-right corner when
    /// `Some` (deathmatch only).
    pub frags: Option<i32>,
    /// Seconds left on the match clock, drawn top-centre as `m:ss` when
    /// `Some` (timed matches only).
    pub match_clock: Option<f32>,
    /// A persistent notice drawn in the lower-middle of the screen, for
    /// example a respawn prompt. It stays up while `Some`; the host clears
    /// it.
    pub center_notice: Option<String>,
    /// Recent kill-feed lines, oldest first; drawn below the frag count,
    /// newest at the bottom. Use [`HudState::push_kill_feed`] and
    /// [`HudState::tick_kill_feed`] to maintain it.
    pub kill_feed: Vec<KillFeedEntry>,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            health: 100,
            armor: 0,
            clip_ammo: None,
            reserve_ammo: None,
            damage_flash: 0.0,
            message: None,
            show_crosshair: true,
            frags: None,
            match_clock: None,
            center_notice: None,
            kill_feed: Vec::new(),
        }
    }
}

impl HudState {
    /// Health clamped to `0` for display; the underlying field is left
    /// untouched so a temporarily negative value is still available to
    /// whatever decides the player is dead.
    #[must_use]
    pub fn display_health(&self) -> i32 {
        self.health.max(0)
    }

    /// Armor clamped to `0` for display.
    #[must_use]
    pub fn display_armor(&self) -> i32 {
        self.armor.max(0)
    }

    /// Sets the damage flash to full intensity, e.g. on taking damage.
    pub fn trigger_damage_flash(&mut self) {
        self.damage_flash = 1.0;
    }

    /// Decays the damage flash toward zero at `rate` per second, clamping at
    /// the ends. Call once per frame with the frame's delta time.
    pub fn decay_damage_flash(&mut self, rate_per_second: f32, delta_seconds: f32) {
        self.damage_flash = (self.damage_flash - rate_per_second * delta_seconds).clamp(0.0, 1.0);
    }

    /// Shows `message` for `seconds`.
    pub fn show_message(&mut self, text: impl Into<String>, seconds: f32) {
        self.message = Some(HudMessage {
            text: text.into(),
            seconds_remaining: seconds.max(0.0),
        });
    }

    /// Counts the current message down by `delta_seconds`, clearing it once
    /// its time runs out. Call once per frame.
    pub fn tick_message(&mut self, delta_seconds: f32) {
        if let Some(message) = &mut self.message {
            message.seconds_remaining -= delta_seconds;
            if message.seconds_remaining <= 0.0 {
                self.message = None;
            }
        }
    }

    /// Appends a kill-feed line that lives for [`KILL_FEED_SECONDS`]. The
    /// feed holds at most [`KILL_FEED_CAPACITY`] lines; the oldest are
    /// dropped to make room.
    pub fn push_kill_feed(&mut self, text: impl Into<String>, local_involved: bool) {
        self.kill_feed.push(KillFeedEntry {
            text: text.into(),
            seconds_remaining: KILL_FEED_SECONDS,
            local_involved,
        });
        let excess = self.kill_feed.len().saturating_sub(KILL_FEED_CAPACITY);
        self.kill_feed.drain(..excess);
    }

    /// Ages every kill-feed line by `delta_seconds` and removes the expired
    /// ones. A negative or non-finite `delta_seconds` ages nothing. Call once
    /// per frame.
    pub fn tick_kill_feed(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return;
        }
        for entry in &mut self.kill_feed {
            entry.seconds_remaining -= delta_seconds;
        }
        self.kill_feed.retain(|entry| entry.seconds_remaining > 0.0);
    }
}

/// Highlight colour for kill-feed lines the local player took part in, and
/// for the frag count.
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 214, 90);

/// Paints `text` anchored at `pos` over a translucent dark backing so it
/// stays legible on bright scenery. `fade` (`0.0..=1.0`) scales the
/// opacity of both.
fn backed_text(
    painter: &egui::Painter,
    anchor: Align2,
    pos: Pos2,
    text: &str,
    font_size: f32,
    color: Color32,
    fade: f32,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(font_size), color);
    let rect = anchor.anchor_size(pos, galley.size());
    let pad = font_size * 0.25;
    painter.rect_filled(
        rect.expand2(Vec2::new(pad * 2.0, pad)),
        pad,
        Color32::from_black_alpha(120).gamma_multiply(fade),
    );
    painter.galley(rect.min, galley, color.gamma_multiply(fade));
}

/// Draws the deathmatch additions: frag count and kill feed (top-right),
/// match clock (top-centre) and the centre notice (lower-middle).
fn draw_match_overlays(painter: &egui::Painter, screen_rect: Rect, scale: f32, state: &HudState) {
    let margin = 24.0 * scale;
    let right = screen_rect.right() - margin;

    if let Some(frags) = state.frags {
        backed_text(
            painter,
            Align2::RIGHT_TOP,
            Pos2::new(right, screen_rect.top() + margin),
            &format!("FRAGS {frags}"),
            24.0 * scale,
            HIGHLIGHT,
            1.0,
        );
    }

    if let Some(seconds) = state.match_clock {
        backed_text(
            painter,
            Align2::CENTER_TOP,
            Pos2::new(screen_rect.center().x, screen_rect.top() + 10.0 * scale),
            &format_clock(seconds),
            26.0 * scale,
            Color32::WHITE,
            1.0,
        );
    }

    // Defensive: the field is public, so never draw past the capacity even
    // if the host pushed entries directly.
    let skip = state.kill_feed.len().saturating_sub(KILL_FEED_CAPACITY);
    let mut y = screen_rect.top() + margin + 44.0 * scale;
    for entry in state.kill_feed.iter().skip(skip) {
        let color = if entry.local_involved {
            HIGHLIGHT
        } else {
            Color32::from_gray(225)
        };
        backed_text(
            painter,
            Align2::RIGHT_TOP,
            Pos2::new(right, y),
            &entry.text,
            17.0 * scale,
            color,
            entry.seconds_remaining.clamp(0.0, 1.0),
        );
        y += 26.0 * scale;
    }

    if let Some(notice) = &state.center_notice {
        backed_text(
            painter,
            Align2::CENTER_CENTER,
            Pos2::new(
                screen_rect.center().x,
                screen_rect.top() + screen_rect.height() * 0.72,
            ),
            notice,
            26.0 * scale,
            Color32::WHITE,
            1.0,
        );
    }
}

/// Draws the HUD into `ctx`'s full screen rect, scaled by the current
/// screen size. Layout, not gameplay: this never mutates `state` besides
/// what [`HudState`] documents as caller-driven.
pub fn draw(ctx: &egui::Context, state: &HudState) {
    let screen_rect = ctx.viewport_rect();
    egui::Area::new("ohl_hud".into())
        .fixed_pos(screen_rect.min)
        .interactable(false)
        .show(ctx, |ui| {
            let painter = ui.painter();
            let scale = (screen_rect.height() / 720.0).max(0.1);

            if state.damage_flash > 0.0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let alpha = (state.damage_flash.clamp(0.0, 1.0) * 120.0) as u8;
                painter.rect_filled(
                    screen_rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(180, 0, 0, alpha),
                );
            }

            if state.show_crosshair {
                let center = screen_rect.center();
                let half = 8.0 * scale;
                let stroke = egui::Stroke::new(2.0 * scale, Color32::WHITE);
                painter.line_segment(
                    [
                        Pos2::new(center.x - half, center.y),
                        Pos2::new(center.x + half, center.y),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(center.x, center.y - half),
                        Pos2::new(center.x, center.y + half),
                    ],
                    stroke,
                );
            }

            let numeral_font = FontId::proportional(28.0 * scale);
            let margin = 24.0 * scale;
            painter.text(
                Pos2::new(margin, screen_rect.bottom() - margin),
                Align2::LEFT_BOTTOM,
                format!("{}", state.display_health()),
                numeral_font.clone(),
                Color32::from_rgb(220, 40, 40),
            );
            painter.text(
                Pos2::new(margin + 90.0 * scale, screen_rect.bottom() - margin),
                Align2::LEFT_BOTTOM,
                format!("{}", state.display_armor()),
                numeral_font.clone(),
                Color32::from_rgb(60, 140, 220),
            );

            if let Some(clip) = state.clip_ammo {
                let reserve = state.reserve_ammo.unwrap_or(0);
                painter.text(
                    Pos2::new(screen_rect.right() - margin, screen_rect.bottom() - margin),
                    Align2::RIGHT_BOTTOM,
                    format!("{clip} / {reserve}"),
                    numeral_font,
                    Color32::WHITE,
                );
            }

            if let Some(message) = &state.message {
                let title_rect = Rect::from_center_size(
                    Pos2::new(screen_rect.center().x, screen_rect.top() + 60.0 * scale),
                    Vec2::new(screen_rect.width() * 0.8, 40.0 * scale),
                );
                painter.text(
                    title_rect.center(),
                    Align2::CENTER_CENTER,
                    &message.text,
                    FontId::proportional(24.0 * scale),
                    Color32::WHITE,
                );
            }

            draw_match_overlays(painter, screen_rect, scale, state);
        });
}

#[cfg(test)]
mod tests {
    use super::{
        HudState, KILL_FEED_CAPACITY, KILL_FEED_SECONDS, KillFeedEntry, Pos2, Rect, Vec2, draw,
    };

    #[test]
    fn display_health_and_armor_never_go_negative() {
        let state = HudState {
            health: -30,
            armor: -5,
            ..HudState::default()
        };
        assert_eq!(state.display_health(), 0);
        assert_eq!(state.display_armor(), 0);
    }

    #[test]
    fn damage_flash_triggers_and_decays() {
        let mut state = HudState::default();
        assert!((state.damage_flash - 0.0).abs() < f32::EPSILON);
        state.trigger_damage_flash();
        assert!((state.damage_flash - 1.0).abs() < f32::EPSILON);
        state.decay_damage_flash(2.0, 0.25);
        assert!((state.damage_flash - 0.5).abs() < 1e-6);
        state.decay_damage_flash(10.0, 10.0);
        assert!((state.damage_flash - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn message_counts_down_and_clears() {
        let mut state = HudState::default();
        state.show_message("Welcome", 1.0);
        assert!(state.message.is_some());
        state.tick_message(0.5);
        assert!(state.message.is_some());
        state.tick_message(0.6);
        assert!(state.message.is_none());
    }

    #[test]
    fn default_state_shows_the_crosshair_and_full_health() {
        let state = HudState::default();
        assert!(state.show_crosshair);
        assert_eq!(state.display_health(), 100);
        assert_eq!(state.display_armor(), 0);
    }

    #[test]
    fn default_state_has_no_match_overlays() {
        let state = HudState::default();
        assert_eq!(state.frags, None);
        assert_eq!(state.match_clock, None);
        assert_eq!(state.center_notice, None);
        assert!(state.kill_feed.is_empty());
    }

    #[test]
    fn kill_feed_pushes_with_full_lifetime_in_order() {
        let mut state = HudState::default();
        state.push_kill_feed("a fragged b", false);
        state.push_kill_feed(String::from("you fragged c"), true);
        assert_eq!(
            state.kill_feed,
            vec![
                KillFeedEntry {
                    text: "a fragged b".to_owned(),
                    seconds_remaining: KILL_FEED_SECONDS,
                    local_involved: false,
                },
                KillFeedEntry {
                    text: "you fragged c".to_owned(),
                    seconds_remaining: KILL_FEED_SECONDS,
                    local_involved: true,
                },
            ]
        );
    }

    #[test]
    fn kill_feed_never_exceeds_capacity_and_drops_the_oldest() {
        let mut state = HudState::default();
        for index in 0..KILL_FEED_CAPACITY + 3 {
            state.push_kill_feed(format!("line {index}"), false);
            assert!(state.kill_feed.len() <= KILL_FEED_CAPACITY);
        }
        assert_eq!(state.kill_feed.len(), KILL_FEED_CAPACITY);
        assert_eq!(state.kill_feed[0].text, "line 3");
        assert_eq!(
            state.kill_feed[KILL_FEED_CAPACITY - 1].text,
            format!("line {}", KILL_FEED_CAPACITY + 2)
        );
    }

    #[test]
    fn kill_feed_entries_expire_independently() {
        let mut state = HudState::default();
        state.push_kill_feed("old", false);
        state.tick_kill_feed(4.0);
        state.push_kill_feed("new", false);
        state.tick_kill_feed(KILL_FEED_SECONDS - 4.0 - 0.5);
        assert_eq!(state.kill_feed.len(), 2);
        // The old line runs out first; the new one outlives it.
        state.tick_kill_feed(1.0);
        assert_eq!(state.kill_feed.len(), 1);
        assert_eq!(state.kill_feed[0].text, "new");
        state.tick_kill_feed(KILL_FEED_SECONDS);
        assert!(state.kill_feed.is_empty());
    }

    #[test]
    fn kill_feed_ignores_negative_and_non_finite_time() {
        let mut state = HudState::default();
        state.push_kill_feed("kept", true);
        for dt in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            state.tick_kill_feed(dt);
            assert_eq!(state.kill_feed.len(), 1, "dt = {dt}");
            assert!((state.kill_feed[0].seconds_remaining - KILL_FEED_SECONDS).abs() < 1e-6);
        }
    }

    /// Runs one headless pass drawing `state` and returns how many shapes
    /// egui produced.
    fn painted_shapes(state: &HudState) -> usize {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
            ..egui::RawInput::default()
        });
        draw(&ctx, state);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        output.shapes.len()
    }

    #[test]
    fn match_overlays_add_painted_shapes_only_when_set() {
        let base = painted_shapes(&HudState::default());
        let mut state = HudState {
            frags: Some(4),
            match_clock: Some(125.0),
            center_notice: Some("Press fire to respawn".to_owned()),
            ..HudState::default()
        };
        state.push_kill_feed("a fragged b", false);
        state.push_kill_feed("you fragged c", true);
        assert!(painted_shapes(&state) > base);
    }
}
