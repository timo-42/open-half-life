//! Menu skeleton (main menu, pause, bindings placeholder) and the
//! `Screen` state machine that governs input capture between gameplay, the
//! console and the menus.

use egui::{Slider, Vec2};

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
    /// Start a new game.
    NewGame,
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
}

impl MenuState {
    /// Creates a menu at its root pane with default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn draw_root(ui: &mut egui::Ui, in_game: bool, actions: &mut Vec<MenuAction>, pane: &mut MenuPane) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.heading("Open Half-Life");
        ui.add_space(16.0);
        if in_game {
            if ui.button("Resume").clicked() {
                actions.push(MenuAction::Resume);
            }
        } else if ui.button("New game").clicked() {
            actions.push(MenuAction::NewGame);
        }
        if ui.button("Load game").clicked() {
            actions.push(MenuAction::LoadGame);
        }
        if in_game && ui.button("Save game").clicked() {
            actions.push(MenuAction::SaveGame);
        }
        if ui.button("Options").clicked() {
            *pane = MenuPane::Options;
        }
        if ui.button("Bindings").clicked() {
            *pane = MenuPane::Bindings;
        }
        if ui.button("Quit").clicked() {
            actions.push(MenuAction::Quit);
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
pub fn draw(ui: &mut egui::Ui, state: &mut MenuState, in_game: bool) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    egui::CentralPanel::default().show(ui, |ui| {
        ui.centered_and_justified(|ui| {
            ui.allocate_ui(Vec2::new(320.0, 420.0), |ui| match state.pane {
                MenuPane::Root => draw_root(ui, in_game, &mut actions, &mut state.pane),
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
    }

    #[test]
    fn default_options_are_within_their_own_ranges() {
        let options = OptionsState::default();
        assert!(SENSITIVITY_RANGE.contains(&options.sensitivity));
        assert!(VOLUME_RANGE.contains(&options.volume));
        assert!(FOV_RANGE.contains(&options.fov));
    }
}
