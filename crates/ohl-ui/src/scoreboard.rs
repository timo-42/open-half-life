//! The deathmatch scoreboard: a centred, translucent, non-interactive panel
//! listing every player's frags and deaths, with the match limits in its
//! title and a winner banner once the match is over.
//!
//! Like [`crate::hud`], this is pure state plus an egui draw call. The host
//! builds a [`ScoreboardState`] each frame (rows already sorted, in the order
//! they should be shown) and calls [`draw`] while the scoreboard is wanted,
//! for example while a key is held or after the match ends.

use egui::{Align2, Color32, FontId, Frame, Order, Pos2, Rect, Sense, Stroke, Vec2};

/// One player's line on the scoreboard.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreRow {
    /// Player-facing name.
    pub name: String,
    /// Frags scored; may be negative (for example after a self-inflicted
    /// death), so it is signed.
    pub frags: i32,
    /// Times this player has died.
    pub deaths: u32,
    /// Whether this row is the human player; drawn highlighted.
    pub is_local: bool,
    /// Whether the player is currently alive; dead rows are drawn dimmed.
    pub alive: bool,
}

/// Everything the scoreboard shows, supplied by the host each frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScoreboardState {
    /// The rows, already sorted by the host; [`draw`] keeps the given order.
    pub rows: Vec<ScoreRow>,
    /// The frag limit, or `None` for no limit.
    pub frag_limit: Option<u32>,
    /// Seconds left on the match clock, or `None` for no time limit.
    pub seconds_left: Option<f32>,
    /// The winner's name once the match is over.
    pub winner: Option<String>,
}

/// Formats a duration in seconds as `m:ss`.
///
/// Fractions of a second are truncated, negative values clamp to `0:00`, a
/// non-finite value (NaN or infinity) renders as `0:00`, and minutes are not
/// wrapped into hours, so ninety minutes shows as `90:00`.
#[must_use]
pub fn format_clock(seconds: f32) -> String {
    if !seconds.is_finite() {
        return "0:00".to_owned();
    }
    // Truncation is the intent: whole seconds only. The clamp makes the
    // sign loss impossible and the cast saturates for huge inputs.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole = seconds.max(0.0) as u32;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// The title line: `SCOREBOARD`, followed by the frag limit and the time
/// left when they are set. A frag limit of `0` is treated as no limit, the
/// same convention the skirmish setup menu uses.
fn title_line(state: &ScoreboardState) -> String {
    let mut parts = vec!["SCOREBOARD".to_owned()];
    if let Some(limit) = state.frag_limit.filter(|limit| *limit > 0) {
        parts.push(format!("Frag limit {limit}"));
    }
    if let Some(seconds) = state.seconds_left {
        parts.push(format!("{} left", format_clock(seconds)));
    }
    parts.join("  |  ")
}

/// The winner banner text, present once the match is over.
fn winner_banner(state: &ScoreboardState) -> Option<String> {
    state
        .winner
        .as_ref()
        .map(|name| format!("{name} WINS THE MATCH"))
}

/// Text colour for a row: the local player is highlighted, other players are
/// plain, and a dead player's colour is dimmed.
fn row_text_color(row: &ScoreRow) -> Color32 {
    let base = if row.is_local {
        Color32::from_rgb(255, 214, 90)
    } else {
        Color32::from_rgb(225, 225, 215)
    };
    if row.alive {
        base
    } else {
        base.gamma_multiply(0.45)
    }
}

/// Background fill for a row; only the local player's row has one.
fn row_fill(row: &ScoreRow) -> Option<Color32> {
    row.is_local
        .then(|| Color32::from_rgba_unmultiplied(246, 154, 38, 48))
}

/// Shared sizing for one scoreboard frame, derived from the screen height.
struct Metrics {
    scale: f32,
    table_width: f32,
    row_height: f32,
    inset: f32,
    number_column: f32,
}

impl Metrics {
    /// Sizes for a screen of the given dimensions, relative to a 720-pixel
    /// tall reference like [`crate::hud::draw`].
    fn for_screen(screen: Rect) -> Self {
        let scale = (screen.height() / 720.0).max(0.1);
        Self {
            scale,
            table_width: (460.0 * scale).min(screen.width() * 0.95),
            row_height: 28.0 * scale,
            inset: 10.0 * scale,
            number_column: 84.0 * scale,
        }
    }
}

/// The three column rectangles of one table row: the name takes whatever the
/// two fixed-width number columns leave over.
struct Columns {
    name: Rect,
    frags: Rect,
    deaths: Rect,
}

impl Columns {
    fn new(row: Rect, number_column: f32) -> Self {
        let deaths_left = row.right() - number_column;
        let frags_left = deaths_left - number_column;
        Self {
            name: Rect::from_min_max(row.min, Pos2::new(frags_left, row.max.y)),
            frags: Rect::from_min_max(
                Pos2::new(frags_left, row.min.y),
                Pos2::new(deaths_left, row.max.y),
            ),
            deaths: Rect::from_min_max(Pos2::new(deaths_left, row.min.y), row.max),
        }
    }
}

/// Allocates one table row, paints its optional background `fill`, then its
/// three cells (name left-aligned, frags and deaths right-aligned) in
/// `color`. Each cell is clipped to its own column so an over-long name
/// cannot spill into the numbers. Returns the row's rectangle.
fn table_row(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    cells: [&str; 3],
    font: &FontId,
    color: Color32,
    fill: Option<Color32>,
) -> Rect {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(metrics.table_width, metrics.row_height),
        Sense::hover(),
    );
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 2.0 * metrics.scale, fill);
    }
    let columns = Columns::new(rect, metrics.number_column);
    let placements = [
        (columns.name, Align2::LEFT_CENTER),
        (columns.frags, Align2::RIGHT_CENTER),
        (columns.deaths, Align2::RIGHT_CENTER),
    ];
    for ((column, anchor), text) in placements.into_iter().zip(cells) {
        let x = if anchor == Align2::LEFT_CENTER {
            column.left() + metrics.inset
        } else {
            column.right() - metrics.inset
        };
        ui.painter().with_clip_rect(column).text(
            Pos2::new(x, column.center().y),
            anchor,
            text,
            font.clone(),
            color,
        );
    }
    rect
}

/// Draws the scoreboard centred on the screen, scaled by screen height like
/// [`crate::hud::draw`] (relative to 720 pixels). The panel is translucent
/// and non-interactive so it never takes input from gameplay.
pub fn draw(ctx: &egui::Context, state: &ScoreboardState) {
    let metrics = Metrics::for_screen(ctx.viewport_rect());
    let scale = metrics.scale;

    egui::Area::new("ohl_scoreboard".into())
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .order(Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::new()
                .inner_margin(16.0 * scale)
                .corner_radius(6.0 * scale)
                .fill(Color32::from_rgba_unmultiplied(8, 12, 10, 205))
                .stroke(Stroke::new(1.0, Color32::from_gray(90)))
                .show(ui, |ui| {
                    ui.set_width(metrics.table_width);

                    if let Some(banner) = winner_banner(state) {
                        let (rect, _) = ui.allocate_exact_size(
                            Vec2::new(metrics.table_width, 44.0 * scale),
                            Sense::hover(),
                        );
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            banner,
                            FontId::proportional(32.0 * scale),
                            Color32::from_rgb(255, 214, 90),
                        );
                    }

                    let (title_rect, _) = ui.allocate_exact_size(
                        Vec2::new(metrics.table_width, 30.0 * scale),
                        Sense::hover(),
                    );
                    ui.painter().text(
                        title_rect.center(),
                        Align2::CENTER_CENTER,
                        title_line(state),
                        FontId::proportional(20.0 * scale),
                        Color32::from_rgb(235, 225, 195),
                    );

                    let font = FontId::proportional(18.0 * scale);
                    let header_color = Color32::from_gray(150);
                    let header = table_row(
                        ui,
                        &metrics,
                        ["Player", "Frags", "Deaths"],
                        &font,
                        header_color,
                        None,
                    );
                    ui.painter().hline(
                        header.x_range(),
                        header.bottom(),
                        Stroke::new(1.0, Color32::from_gray(90)),
                    );

                    if state.rows.is_empty() {
                        table_row(
                            ui,
                            &metrics,
                            ["No players", "", ""],
                            &font,
                            header_color,
                            None,
                        );
                    }
                    for row in &state.rows {
                        table_row(
                            ui,
                            &metrics,
                            [&row.name, &row.frags.to_string(), &row.deaths.to_string()],
                            &font,
                            row_text_color(row),
                            row_fill(row),
                        );
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::{
        Columns, ScoreRow, ScoreboardState, draw, format_clock, row_fill, row_text_color,
        title_line, winner_banner,
    };
    use egui::{Color32, Pos2, Rect};

    fn row(name: &str, is_local: bool, alive: bool) -> ScoreRow {
        ScoreRow {
            name: name.to_owned(),
            frags: 3,
            deaths: 1,
            is_local,
            alive,
        }
    }

    #[test]
    fn format_clock_formats_minutes_and_zero_padded_seconds() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(5.0), "0:05");
        assert_eq!(format_clock(65.0), "1:05");
        assert_eq!(format_clock(600.0), "10:00");
        assert_eq!(format_clock(5400.0), "90:00");
    }

    #[test]
    fn format_clock_truncates_fractions_of_a_second() {
        assert_eq!(format_clock(59.99), "0:59");
        assert_eq!(format_clock(0.5), "0:00");
    }

    #[test]
    fn format_clock_clamps_negative_and_non_finite_values_to_zero() {
        assert_eq!(format_clock(-1.0), "0:00");
        assert_eq!(format_clock(-0.0), "0:00");
        assert_eq!(format_clock(f32::NEG_INFINITY), "0:00");
        assert_eq!(format_clock(f32::NAN), "0:00");
        assert_eq!(format_clock(f32::INFINITY), "0:00");
    }

    #[test]
    fn default_state_is_empty_with_no_limits() {
        let state = ScoreboardState::default();
        assert!(state.rows.is_empty());
        assert_eq!(state.frag_limit, None);
        assert_eq!(state.seconds_left, None);
        assert_eq!(state.winner, None);
    }

    #[test]
    fn title_line_mentions_only_the_limits_that_are_set() {
        let mut state = ScoreboardState::default();
        assert_eq!(title_line(&state), "SCOREBOARD");
        state.frag_limit = Some(25);
        assert_eq!(title_line(&state), "SCOREBOARD  |  Frag limit 25");
        state.seconds_left = Some(185.0);
        assert_eq!(
            title_line(&state),
            "SCOREBOARD  |  Frag limit 25  |  3:05 left"
        );
        state.frag_limit = None;
        assert_eq!(title_line(&state), "SCOREBOARD  |  3:05 left");
    }

    #[test]
    fn a_zero_frag_limit_is_shown_as_no_limit() {
        let state = ScoreboardState {
            frag_limit: Some(0),
            ..ScoreboardState::default()
        };
        assert_eq!(title_line(&state), "SCOREBOARD");
    }

    #[test]
    fn winner_banner_names_the_winner_only_when_there_is_one() {
        let mut state = ScoreboardState::default();
        assert_eq!(winner_banner(&state), None);
        state.winner = Some("Alpha".to_owned());
        assert_eq!(
            winner_banner(&state).as_deref(),
            Some("Alpha WINS THE MATCH")
        );
    }

    #[test]
    fn local_row_is_highlighted_and_dead_rows_are_dimmed() {
        let other = row("Bot", false, true);
        let local = row("You", true, true);
        let dead_other = row("Bot", false, false);
        let dead_local = row("You", true, false);

        assert_ne!(row_text_color(&local), row_text_color(&other));
        assert!(row_fill(&local).is_some());
        assert!(row_fill(&other).is_none());
        assert!(row_fill(&dead_other).is_none());

        let brightness =
            |color: Color32| u16::from(color.r()) + u16::from(color.g()) + u16::from(color.b());
        assert!(brightness(row_text_color(&dead_other)) < brightness(row_text_color(&other)));
        assert!(brightness(row_text_color(&dead_local)) < brightness(row_text_color(&local)));
    }

    #[test]
    fn columns_tile_the_row_without_overlap() {
        let row = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(510.0, 48.0));
        let columns = Columns::new(row, 80.0);
        assert!((columns.name.left() - row.left()).abs() < f32::EPSILON);
        assert!((columns.name.right() - columns.frags.left()).abs() < f32::EPSILON);
        assert!((columns.frags.right() - columns.deaths.left()).abs() < f32::EPSILON);
        assert!((columns.deaths.right() - row.right()).abs() < f32::EPSILON);
        assert!((columns.frags.width() - 80.0).abs() < 1e-4);
        assert!((columns.deaths.width() - 80.0).abs() < 1e-4);
    }

    /// Runs one headless pass drawing `state` and returns how many shapes
    /// egui produced.
    fn painted_shapes(state: &ScoreboardState) -> usize {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 720.0))),
            ..egui::RawInput::default()
        });
        draw(&ctx, state);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        output.shapes.len()
    }

    #[test]
    fn draw_paints_for_empty_and_populated_states() {
        let empty = ScoreboardState::default();
        assert!(painted_shapes(&empty) > 0);

        let populated = ScoreboardState {
            rows: vec![
                row("You", true, true),
                row("ohltest_bot_1", false, false),
                row("ohltest_bot_2", false, true),
            ],
            frag_limit: Some(10),
            seconds_left: Some(90.0),
            winner: Some("You".to_owned()),
        };
        assert!(painted_shapes(&populated) > painted_shapes(&empty));
    }
}
