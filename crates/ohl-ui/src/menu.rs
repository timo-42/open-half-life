//! Player-facing main and pause menus and the `Screen` state machine that
//! governs input capture between gameplay, the console and the menus.
//!
//! The multiplayer pane is an offline skirmish setup: the player picks one of
//! the host-supplied deathmatch maps, a bot count, a bot skill, a frag limit
//! and a time limit, then presses START SKIRMISH, which reports
//! [`MenuAction::StartSkirmish`]. Nothing is networked; the pane only
//! gathers the settings. With no maps supplied it explains that and keeps the
//! start button disabled.

use egui::{Color32, RichText, Slider, Stroke, Vec2};

/// Which top-level UI screen currently owns the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Gameplay has focus; the console and menus are hidden.
    #[default]
    InGame,
    /// The main menu is shown (no gameplay session running, or the player
    /// returned to it).
    MainMenu,
    /// The pause menu is shown over a running gameplay session.
    Pause,
    /// The developer console is shown.
    Console,
}

/// Which inputs the current [`Screen`] captures. `InGame` releases the
/// cursor to gameplay (mouselook); every other screen captures keyboard and
/// mouse for widget interaction and shows the OS cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCapture {
    /// The screen consumes keyboard input (text entry, menu navigation).
    pub keyboard: bool,
    /// The screen consumes mouse input (clicking widgets).
    pub mouse: bool,
    /// The OS cursor should be shown and free to move, rather than locked
    /// and hidden for mouselook.
    pub release_cursor: bool,
}

impl Screen {
    /// The input capture rule for this screen.
    #[must_use]
    pub fn input_capture(self) -> InputCapture {
        match self {
            Self::InGame => InputCapture {
                keyboard: false,
                mouse: false,
                release_cursor: false,
            },
            Self::MainMenu | Self::Pause | Self::Console => InputCapture {
                keyboard: true,
                mouse: true,
                release_cursor: true,
            },
        }
    }
}

/// An action a menu screen requests from the host application. The menu
/// itself never performs these; it only reports intent, mirroring
/// [`crate::console::ConsoleEvent`].
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    /// Start a single-player mission at the selected difficulty.
    StartSinglePlayer {
        /// The mission's starting map.
        map: &'static str,
        /// The selected gameplay difficulty.
        difficulty: Difficulty,
    },
    /// Open the load-game screen (not itself implemented here).
    LoadGame,
    /// Open the save-game screen (not itself implemented here).
    SaveGame,
    /// Resume gameplay from the pause menu.
    Resume,
    /// Quit the application.
    Quit,
    /// Mouse look sensitivity changed, in the options screen's own units.
    SetSensitivity(f32),
    /// Output volume changed, `0.0..=1.0`.
    SetVolume(f32),
    /// Field of view changed, in degrees.
    SetFov(f32),
    /// Start an offline deathmatch skirmish against bots.
    StartSkirmish {
        /// The chosen map, exactly as the host listed it.
        map: String,
        /// Number of bot opponents, within [`SKIRMISH_BOTS_RANGE`].
        bots: u8,
        /// How capable the bots are.
        skill: BotSkill,
        /// Frags that end the match; `0` means no limit.
        frag_limit: u32,
        /// Match length in minutes; `0` means no limit.
        time_limit_minutes: u32,
    },
}

/// How capable skirmish bots are. The application maps this
/// presentation-level value onto its bot behaviour tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotSkill {
    /// Slow to react and inaccurate.
    Easy,
    /// The default balance.
    #[default]
    Normal,
    /// Fast to react and accurate.
    Hard,
}

/// A difficulty exposed by the new-game menu. The application maps this
/// presentation-level value onto its campaign configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Difficulty {
    Easy,
    #[default]
    Medium,
    Hard,
}

/// One selectable single-player mission. Mission data is supplied by the
/// host so this UI crate does not own campaign ordering or map names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mission {
    /// Player-facing mission title.
    pub title: &'static str,
    /// Starting map passed back to the host when this mission is chosen.
    pub map: &'static str,
}

/// Bounded options state backing the options screen's sliders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptionsState {
    /// Mouse sensitivity, `0.1..=10.0`.
    pub sensitivity: f32,
    /// Output volume, `0.0..=1.0`.
    pub volume: f32,
    /// Field of view in degrees, `60.0..=120.0`.
    pub fov: f32,
}

impl Default for OptionsState {
    fn default() -> Self {
        Self {
            sensitivity: 3.0,
            volume: 1.0,
            fov: 90.0,
        }
    }
}

/// Sensitivity bounds shown by the options screen.
pub const SENSITIVITY_RANGE: std::ops::RangeInclusive<f32> = 0.1..=10.0;
/// Volume bounds shown by the options screen.
pub const VOLUME_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1.0;
/// Field-of-view bounds shown by the options screen.
pub const FOV_RANGE: std::ops::RangeInclusive<f32> = 60.0..=120.0;

/// Bot-count bounds shown by the skirmish setup pane.
pub const SKIRMISH_BOTS_RANGE: std::ops::RangeInclusive<u8> = 1..=15;
/// Frag-limit bounds shown by the skirmish setup pane; `0` means no limit.
pub const SKIRMISH_FRAG_LIMIT_RANGE: std::ops::RangeInclusive<u32> = 0..=100;
/// Time-limit bounds, in minutes, shown by the skirmish setup pane; `0`
/// means no limit.
pub const SKIRMISH_TIME_LIMIT_RANGE: std::ops::RangeInclusive<u32> = 0..=60;

/// Bot count a fresh [`MenuState`] starts with.
pub const SKIRMISH_BOTS_DEFAULT: u8 = 3;
/// Frag limit a fresh [`MenuState`] starts with.
pub const SKIRMISH_FRAG_LIMIT_DEFAULT: u32 = 10;
/// Time limit, in minutes, a fresh [`MenuState`] starts with.
pub const SKIRMISH_TIME_LIMIT_DEFAULT_MINUTES: u32 = 10;

/// Which pane of the menu is showing: the root list or the options/bindings
/// sub-screens reachable from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuPane {
    #[default]
    Root,
    SinglePlayer,
    Multiplayer,
    Options,
    Bindings,
}

/// Owns the menu's local navigation state (which pane) and the options
/// values it edits in place. `Screen` (in [`crate`]) tracks which top-level
/// screen is active; this only matters while that screen is [`Screen::MainMenu`]
/// or [`Screen::Pause`].
#[derive(Debug, Clone)]
pub struct MenuState {
    /// The currently visible pane.
    pub pane: MenuPane,
    /// The options screen's editable values.
    pub options: OptionsState,
    /// Index into the host-provided mission list.
    pub selected_mission: usize,
    /// Difficulty selected for a new single-player game.
    pub difficulty: Difficulty,
    /// Index into the host-provided skirmish map list; clamped to the list
    /// length whenever the pane is drawn.
    pub selected_skirmish_map: usize,
    /// Number of bot opponents for a skirmish, within [`SKIRMISH_BOTS_RANGE`].
    pub skirmish_bots: u8,
    /// Bot skill for a skirmish.
    pub skirmish_skill: BotSkill,
    /// Skirmish frag limit; `0` means no limit.
    pub skirmish_frag_limit: u32,
    /// Skirmish time limit in minutes; `0` means no limit.
    pub skirmish_time_limit_minutes: u32,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            pane: MenuPane::default(),
            options: OptionsState::default(),
            selected_mission: 0,
            difficulty: Difficulty::default(),
            selected_skirmish_map: 0,
            skirmish_bots: SKIRMISH_BOTS_DEFAULT,
            skirmish_skill: BotSkill::default(),
            skirmish_frag_limit: SKIRMISH_FRAG_LIMIT_DEFAULT,
            skirmish_time_limit_minutes: SKIRMISH_TIME_LIMIT_DEFAULT_MINUTES,
        }
    }
}

impl MenuState {
    /// Creates a menu at its root pane with default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Clamps `value` into `range`.
fn clamp_to<T: Ord + Copy>(value: T, range: &std::ops::RangeInclusive<T>) -> T {
    value.clamp(*range.start(), *range.end())
}

/// Builds the [`MenuAction::StartSkirmish`] for the current setup, or `None`
/// when `maps` is empty (there is nothing to start). Out-of-range values in
/// `state` are clamped into their ranges and the selected map index is
/// clamped to the list, so the action is always valid for the host.
#[must_use]
pub fn skirmish_action(state: &MenuState, maps: &[String]) -> Option<MenuAction> {
    let last = maps.len().checked_sub(1)?;
    Some(MenuAction::StartSkirmish {
        map: maps[state.selected_skirmish_map.min(last)].clone(),
        bots: clamp_to(state.skirmish_bots, &SKIRMISH_BOTS_RANGE),
        skill: state.skirmish_skill,
        frag_limit: clamp_to(state.skirmish_frag_limit, &SKIRMISH_FRAG_LIMIT_RANGE),
        time_limit_minutes: clamp_to(
            state.skirmish_time_limit_minutes,
            &SKIRMISH_TIME_LIMIT_RANGE,
        ),
    })
}

fn menu_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        [260.0, 34.0],
        egui::Button::new(RichText::new(label).size(18.0).strong()),
    )
}

fn draw_root(ui: &mut egui::Ui, in_game: bool, actions: &mut Vec<MenuAction>, pane: &mut MenuPane) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(
            RichText::new("λ")
                .size(72.0)
                .color(Color32::from_rgb(246, 154, 38)),
        );
        ui.label(
            RichText::new("OPEN HALF-LIFE")
                .size(25.0)
                .strong()
                .color(Color32::from_rgb(235, 225, 195)),
        );
        ui.add_space(22.0);
        if in_game {
            if menu_button(ui, "RESUME GAME").clicked() {
                actions.push(MenuAction::Resume);
            }
        } else {
            if menu_button(ui, "SINGLE PLAYER").clicked() {
                *pane = MenuPane::SinglePlayer;
            }
            if menu_button(ui, "MULTIPLAYER").clicked() {
                *pane = MenuPane::Multiplayer;
            }
        }
        if menu_button(ui, "LOAD GAME").clicked() {
            actions.push(MenuAction::LoadGame);
        }
        if in_game && menu_button(ui, "SAVE GAME").clicked() {
            actions.push(MenuAction::SaveGame);
        }
        if menu_button(ui, "OPTIONS").clicked() {
            *pane = MenuPane::Options;
        }
        if menu_button(ui, "KEYBOARD").clicked() {
            *pane = MenuPane::Bindings;
        }
        if menu_button(ui, "QUIT").clicked() {
            actions.push(MenuAction::Quit);
        }
    });
}

fn draw_single_player(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    missions: &[Mission],
    actions: &mut Vec<MenuAction>,
) {
    ui.vertical_centered(|ui| {
        ui.heading("Single Player");
        ui.label("Choose a mission and difficulty");
        ui.add_space(18.0);

        if missions.is_empty() {
            ui.label("No playable missions are available.");
        } else {
            state.selected_mission = state.selected_mission.min(missions.len() - 1);
            egui::ComboBox::from_label("Mission")
                .selected_text(missions[state.selected_mission].title)
                .width(260.0)
                .popup_style(ui.style().as_ref().clone().into())
                .show_ui(ui, |ui| {
                    for (index, mission) in missions.iter().enumerate() {
                        ui.selectable_value(&mut state.selected_mission, index, mission.title);
                    }
                });
            ui.add_space(16.0);
            ui.label("Difficulty");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.difficulty, Difficulty::Easy, "Easy");
                ui.selectable_value(&mut state.difficulty, Difficulty::Medium, "Medium");
                ui.selectable_value(&mut state.difficulty, Difficulty::Hard, "Hard");
            });
            ui.add_space(24.0);
            if menu_button(ui, "BEGIN MISSION").clicked() {
                actions.push(MenuAction::StartSinglePlayer {
                    map: missions[state.selected_mission].map,
                    difficulty: state.difficulty,
                });
            }
        }
        if menu_button(ui, "BACK").clicked() {
            state.pane = MenuPane::Root;
        }
    });
}

/// The offline skirmish setup pane (reached from the root's MULTIPLAYER
/// button): map list, bot count, bot skill, frag and time limits, START
/// SKIRMISH and BACK. `maps` is the host's list of deathmatch map names.
fn draw_skirmish(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    maps: &[String],
    actions: &mut Vec<MenuAction>,
) {
    ui.vertical_centered(|ui| {
        // The pane has many rows; tighten the spacing so it fits the menu's
        // fixed-size area.
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.spacing_mut().slider_width = 240.0;
        ui.heading("Skirmish (offline, against bots)");
        ui.add_space(10.0);

        if let Some(last) = maps.len().checked_sub(1) {
            state.selected_skirmish_map = state.selected_skirmish_map.min(last);
            ui.label("Map");
            ui.group(|ui| {
                ui.set_min_width(300.0);
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            for (index, name) in maps.iter().enumerate() {
                                ui.selectable_value(&mut state.selected_skirmish_map, index, name);
                            }
                        });
                    });
            });
        } else {
            ui.label("No deathmatch maps were found in the imported game data.");
        }

        ui.add_space(10.0);
        ui.label("Bots");
        ui.add(Slider::new(&mut state.skirmish_bots, SKIRMISH_BOTS_RANGE));
        ui.label("Bot skill");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.skirmish_skill, BotSkill::Easy, "Easy");
            ui.selectable_value(&mut state.skirmish_skill, BotSkill::Normal, "Normal");
            ui.selectable_value(&mut state.skirmish_skill, BotSkill::Hard, "Hard");
        });
        ui.label("Frag limit (0 = no limit)");
        ui.add(Slider::new(
            &mut state.skirmish_frag_limit,
            SKIRMISH_FRAG_LIMIT_RANGE,
        ));
        ui.label("Time limit, minutes (0 = no limit)");
        ui.add(Slider::new(
            &mut state.skirmish_time_limit_minutes,
            SKIRMISH_TIME_LIMIT_RANGE,
        ));

        ui.add_space(14.0);
        let can_start = !maps.is_empty();
        if ui
            .add_enabled_ui(can_start, |ui| menu_button(ui, "START SKIRMISH"))
            .inner
            .clicked()
        {
            actions.extend(skirmish_action(state, maps));
        }
        if menu_button(ui, "BACK").clicked() {
            state.pane = MenuPane::Root;
        }
    });
}

fn draw_options(
    ui: &mut egui::Ui,
    options: &mut OptionsState,
    actions: &mut Vec<MenuAction>,
    pane: &mut MenuPane,
) {
    ui.vertical_centered(|ui| {
        ui.heading("Options");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Mouse sensitivity");
            if ui
                .add(Slider::new(&mut options.sensitivity, SENSITIVITY_RANGE))
                .changed()
            {
                actions.push(MenuAction::SetSensitivity(options.sensitivity));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Volume");
            if ui
                .add(Slider::new(&mut options.volume, VOLUME_RANGE))
                .changed()
            {
                actions.push(MenuAction::SetVolume(options.volume));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Field of view");
            if ui.add(Slider::new(&mut options.fov, FOV_RANGE)).changed() {
                actions.push(MenuAction::SetFov(options.fov));
            }
        });
        ui.add_space(16.0);
        if ui.button("Back").clicked() {
            *pane = MenuPane::Root;
        }
    });
}

fn draw_bindings(ui: &mut egui::Ui, pane: &mut MenuPane) {
    ui.vertical_centered(|ui| {
        ui.heading("Bindings");
        ui.label("Key bindings are not editable yet.");
        ui.add_space(16.0);
        if ui.button("Back").clicked() {
            *pane = MenuPane::Root;
        }
    });
}

/// Draws the menu (main or pause, depending on `in_game`) and returns the
/// actions the player triggered this frame. `ui` is the frame's root `Ui`;
/// see [`crate::root_ui`]. `missions` feeds the single-player pane and
/// `skirmish_maps` the skirmish setup pane; the host supplies both.
pub fn draw(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    in_game: bool,
    missions: &[Mission],
    skirmish_maps: &[String],
) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    // The menu is always dark, independently of the system's theme.
    let mut visuals = egui::Visuals::dark();
    let text_color = Color32::from_rgb(235, 225, 195);
    visuals.override_text_color = Some(text_color);
    visuals.selection.bg_fill = Color32::from_rgb(145, 76, 20);
    visuals.selection.stroke = Stroke::new(1.0, text_color);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(35, 42, 35);
    // Buttons use weak_bg_fill; bg_fill is used by sliders and other controls.
    visuals.widgets.inactive.weak_bg_fill = visuals.widgets.inactive.bg_fill;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(104, 111, 82));
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(124, 68, 22);
    visuals.widgets.hovered.weak_bg_fill = visuals.widgets.hovered.bg_fill;
    visuals.widgets.active.bg_fill = visuals.selection.bg_fill;
    visuals.widgets.active.weak_bg_fill = visuals.widgets.active.bg_fill;

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(Color32::from_rgb(12, 18, 14)))
        .show(ui, |ui| {
            *ui.visuals_mut() = visuals;
            ui.centered_and_justified(|ui| {
                ui.allocate_ui(Vec2::new(380.0, 560.0), |ui| match state.pane {
                    MenuPane::Root => draw_root(ui, in_game, &mut actions, &mut state.pane),
                    MenuPane::SinglePlayer => {
                        draw_single_player(ui, state, missions, &mut actions);
                    }
                    MenuPane::Multiplayer => {
                        draw_skirmish(ui, state, skirmish_maps, &mut actions);
                    }
                    MenuPane::Options => {
                        draw_options(ui, &mut state.options, &mut actions, &mut state.pane);
                    }
                    MenuPane::Bindings => draw_bindings(ui, &mut state.pane),
                });
            });
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::{
        BotSkill, FOV_RANGE, MenuAction, MenuPane, MenuState, OptionsState, SENSITIVITY_RANGE,
        SKIRMISH_BOTS_RANGE, SKIRMISH_FRAG_LIMIT_RANGE, SKIRMISH_TIME_LIMIT_RANGE, Screen,
        VOLUME_RANGE, draw, skirmish_action,
    };

    fn maps() -> Vec<String> {
        vec!["ohltest_a".to_owned(), "ohltest_b".to_owned()]
    }

    #[test]
    fn in_game_releases_neither_keyboard_nor_mouse_nor_cursor() {
        let capture = Screen::InGame.input_capture();
        assert!(!capture.keyboard);
        assert!(!capture.mouse);
        assert!(!capture.release_cursor);
    }

    #[test]
    fn menu_and_console_screens_capture_input_and_release_the_cursor() {
        for screen in [Screen::MainMenu, Screen::Pause, Screen::Console] {
            let capture = screen.input_capture();
            assert!(capture.keyboard, "{screen:?}");
            assert!(capture.mouse, "{screen:?}");
            assert!(capture.release_cursor, "{screen:?}");
        }
    }

    #[test]
    fn default_screen_is_in_game() {
        assert_eq!(Screen::default(), Screen::InGame);
    }

    #[test]
    fn menu_state_starts_at_the_root_pane() {
        let state = MenuState::new();
        assert_eq!(state.pane, MenuPane::Root);
        assert_eq!(state.difficulty, super::Difficulty::Medium);
        assert_eq!(state.selected_mission, 0);
    }

    #[test]
    fn default_options_are_within_their_own_ranges() {
        let options = OptionsState::default();
        assert!(SENSITIVITY_RANGE.contains(&options.sensitivity));
        assert!(VOLUME_RANGE.contains(&options.volume));
        assert!(FOV_RANGE.contains(&options.fov));
    }

    #[test]
    fn skirmish_defaults_are_three_normal_bots_ten_frags_ten_minutes() {
        let state = MenuState::default();
        assert_eq!(state.selected_skirmish_map, 0);
        assert_eq!(state.skirmish_bots, 3);
        assert_eq!(state.skirmish_skill, BotSkill::Normal);
        assert_eq!(state.skirmish_frag_limit, 10);
        assert_eq!(state.skirmish_time_limit_minutes, 10);
        // `new` goes through the same defaults.
        let fresh = MenuState::new();
        assert_eq!(fresh.skirmish_bots, 3);
        assert_eq!(fresh.pane, MenuPane::Root);
    }

    #[test]
    fn skirmish_defaults_are_within_their_own_ranges() {
        let state = MenuState::default();
        assert!(SKIRMISH_BOTS_RANGE.contains(&state.skirmish_bots));
        assert!(SKIRMISH_FRAG_LIMIT_RANGE.contains(&state.skirmish_frag_limit));
        assert!(SKIRMISH_TIME_LIMIT_RANGE.contains(&state.skirmish_time_limit_minutes));
        assert_eq!(SKIRMISH_BOTS_RANGE, 1..=15);
        assert_eq!(SKIRMISH_FRAG_LIMIT_RANGE, 0..=100);
        assert_eq!(SKIRMISH_TIME_LIMIT_RANGE, 0..=60);
    }

    #[test]
    fn bot_skill_defaults_to_normal() {
        assert_eq!(BotSkill::default(), BotSkill::Normal);
    }

    #[test]
    fn skirmish_action_is_none_without_maps() {
        assert_eq!(skirmish_action(&MenuState::default(), &[]), None);
    }

    #[test]
    fn skirmish_action_reports_the_current_setup() {
        let state = MenuState {
            selected_skirmish_map: 1,
            skirmish_bots: 7,
            skirmish_skill: BotSkill::Hard,
            skirmish_frag_limit: 0,
            skirmish_time_limit_minutes: 25,
            ..MenuState::default()
        };
        assert_eq!(
            skirmish_action(&state, &maps()),
            Some(MenuAction::StartSkirmish {
                map: "ohltest_b".to_owned(),
                bots: 7,
                skill: BotSkill::Hard,
                frag_limit: 0,
                time_limit_minutes: 25,
            })
        );
    }

    #[test]
    fn skirmish_action_with_default_state_uses_the_first_map_and_defaults() {
        assert_eq!(
            skirmish_action(&MenuState::default(), &maps()),
            Some(MenuAction::StartSkirmish {
                map: "ohltest_a".to_owned(),
                bots: 3,
                skill: BotSkill::Normal,
                frag_limit: 10,
                time_limit_minutes: 10,
            })
        );
    }

    #[test]
    fn skirmish_action_clamps_out_of_range_values() {
        let state = MenuState {
            selected_skirmish_map: 99,
            skirmish_bots: 200,
            skirmish_frag_limit: 5000,
            skirmish_time_limit_minutes: 5000,
            ..MenuState::default()
        };
        assert_eq!(
            skirmish_action(&state, &maps()),
            Some(MenuAction::StartSkirmish {
                map: "ohltest_b".to_owned(),
                bots: 15,
                skill: BotSkill::Normal,
                frag_limit: 100,
                time_limit_minutes: 60,
            })
        );
        let no_bots = MenuState {
            skirmish_bots: 0,
            ..MenuState::default()
        };
        assert!(matches!(
            skirmish_action(&no_bots, &maps()),
            Some(MenuAction::StartSkirmish { bots: 1, .. })
        ));
    }

    /// Draws one headless frame of the skirmish pane and returns the actions.
    fn draw_skirmish_pane(state: &mut MenuState, maps: &[String]) -> Vec<MenuAction> {
        state.pane = MenuPane::Multiplayer;
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..egui::RawInput::default()
        });
        let mut ui = crate::root_ui(&ctx);
        let actions = draw(&mut ui, state, false, &[], maps);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        actions
    }

    #[test]
    fn drawing_the_skirmish_pane_clamps_the_selected_map_to_the_list() {
        let mut state = MenuState {
            selected_skirmish_map: 99,
            ..MenuState::default()
        };
        let actions = draw_skirmish_pane(&mut state, &maps());
        assert!(actions.is_empty());
        assert_eq!(state.selected_skirmish_map, 1);
        assert_eq!(state.pane, MenuPane::Multiplayer);
    }

    #[test]
    fn drawing_the_skirmish_pane_without_maps_offers_nothing_to_start() {
        let mut state = MenuState::default();
        let actions = draw_skirmish_pane(&mut state, &[]);
        assert!(actions.is_empty());
        assert_eq!(state.selected_skirmish_map, 0);
    }
}
