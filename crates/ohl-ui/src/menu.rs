//! Player-facing main and pause menus and the `Screen` state machine that
//! governs input capture between gameplay, the console and the menus.

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
#[derive(Debug, Clone, Default)]
pub struct MenuState {
    /// The currently visible pane.
    pub pane: MenuPane,
    /// The options screen's editable values.
    pub options: OptionsState,
    /// Index into the host-provided mission list.
    pub selected_mission: usize,
    /// Difficulty selected for a new single-player game.
    pub difficulty: Difficulty,
}

impl MenuState {
    /// Creates a menu at its root pane with default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn menu_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized(
        [260.0, 34.0],
        egui::Button::new(RichText::new(label).size(18.0)),
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

fn draw_multiplayer(ui: &mut egui::Ui, pane: &mut MenuPane) {
    ui.vertical_centered(|ui| {
        ui.heading("Multiplayer");
        ui.add_space(16.0);
        ui.group(|ui| {
            ui.set_min_width(300.0);
            ui.label(RichText::new("SERVER BROWSER").strong());
            ui.separator();
            ui.label("No servers found");
            ui.label("Multiplayer is a preview and is not connected yet.");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Button::new("Create game"));
                ui.add_enabled(false, egui::Button::new("Join game"));
            });
        });
        ui.add_space(18.0);
        if menu_button(ui, "BACK").clicked() {
            *pane = MenuPane::Root;
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
/// see [`crate::root_ui`].
pub fn draw(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    in_game: bool,
    missions: &[Mission],
) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    let mut visuals = ui.style().visuals.clone();
    visuals.override_text_color = Some(Color32::from_rgb(222, 217, 188));
    visuals.selection.bg_fill = Color32::from_rgb(181, 91, 20);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(35, 42, 35);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(104, 111, 82));
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(124, 68, 22);
    visuals.widgets.active.bg_fill = Color32::from_rgb(181, 91, 20);
    ui.ctx().set_visuals(visuals);

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(Color32::from_rgb(12, 18, 14)))
        .show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.allocate_ui(Vec2::new(380.0, 560.0), |ui| match state.pane {
                    MenuPane::Root => draw_root(ui, in_game, &mut actions, &mut state.pane),
                    MenuPane::SinglePlayer => {
                        draw_single_player(ui, state, missions, &mut actions);
                    }
                    MenuPane::Multiplayer => draw_multiplayer(ui, &mut state.pane),
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
        FOV_RANGE, MenuPane, MenuState, OptionsState, SENSITIVITY_RANGE, Screen, VOLUME_RANGE,
    };

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
}
