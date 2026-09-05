//! The in-game heads-up display: health, armor, ammo, crosshair, damage
//! flash and the message/title area used later by `env_message` and
//! `game_text`.

use egui::{Align2, Color32, FontId, Pos2, Rect, Vec2};

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
        });
}

#[cfg(test)]
mod tests {
    use super::HudState;

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
}
