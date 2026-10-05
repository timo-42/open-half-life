//! Player-facing main and pause menus and the `Screen` state machine that
//! governs input capture between gameplay, the console and the menus.
//!
//! The menu is a tree of panes the player walks with the mouse (or `Tab`
//! and `Enter`) and leaves one level at a time with BACK or `Escape`:
//!
//! - **Single Player**: a new game at a chosen difficulty, a level picked
//!   chapter by chapter, the Hazard Course, or a saved game.
//! - **Multiplayer**: an offline skirmish against bots, plus a LAN server
//!   setup and a LAN game browser. Nothing is networked yet: those two
//!   panes gather their settings and report [`MenuAction::HostLanServer`]
//!   and [`MenuAction::JoinLanServer`] so the host can say so.
//! - **Options**: video, audio, controls (with rebindable keys) and
//!   gameplay settings.
//!
//! As everywhere in this crate, the menu only holds state and reports
//! intent as [`MenuAction`]s; the host owns the window, the game, the save
//! directory and the settings file, and supplies what the panes list
//! through [`MenuData`] every frame.

mod multiplayer;
mod options;
mod single_player;
mod theme;

use egui::{Align2, Color32, RichText, Vec2};

pub use multiplayer::{
    BotSkill, LAN_MAX_PLAYERS_RANGE, LanServerSettings, SKIRMISH_BOTS_DEFAULT, SKIRMISH_BOTS_RANGE,
    SKIRMISH_FRAG_LIMIT_DEFAULT, SKIRMISH_FRAG_LIMIT_RANGE, SKIRMISH_TIME_LIMIT_DEFAULT_MINUTES,
    SKIRMISH_TIME_LIMIT_RANGE, skirmish_action,
};
pub use options::{
    AudioOptions, ControlOptions, DEFAULT_FOV, DisplayMode, DisplaySettings, FOV_RANGE, FPS_LIMITS,
    GameplayOptions, OptionsState, OptionsTab, Resolution, SENSITIVITY_RANGE, VOLUME_RANGE,
    VideoOptions,
};

use crate::bindings::{Action, Binding};

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
    /// Start a single-player map at the selected difficulty.
    StartSinglePlayer {
        /// The map to start on.
        map: &'static str,
        /// The selected gameplay difficulty.
        difficulty: Difficulty,
    },
    /// Load the saved game in this slot (a [`SaveEntry::slot`]).
    LoadSave(String),
    /// Save the running game (the quicksave slot).
    SaveGame,
    /// Resume gameplay from the pause menu.
    Resume,
    /// Leave the running game for the main menu.
    ReturnToMainMenu,
    /// Quit the application.
    Quit,
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
    /// Host a LAN game with these settings. LAN play is not implemented
    /// yet; the pane exists so the host can tell the player so.
    HostLanServer(LanServerSettings),
    /// Join the LAN game at this address. Not implemented yet, as
    /// [`Self::HostLanServer`].
    JoinLanServer {
        /// What the player typed, trimmed.
        address: String,
    },
    /// A live setting in [`MenuState::options`] changed (a slider, a
    /// checkbox, a key binding); the host re-applies and saves them.
    OptionsChanged,
    /// The player pressed APPLY on a new display mode or resolution, now
    /// also stored in [`MenuState::options`].
    ApplyDisplay(DisplaySettings),
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

impl Difficulty {
    /// Every difficulty, easiest first.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// The player-facing name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }
}

/// One chapter of the level-select pane, supplied by the host so this UI
/// crate does not own campaign ordering or map names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterEntry {
    /// Player-facing chapter title.
    pub title: &'static str,
    /// The chapter's levels in play order; may be empty when the host
    /// knows the chapter but not its maps.
    pub levels: Vec<LevelEntry>,
}

/// One level of a [`ChapterEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelEntry {
    /// The map, passed back in [`MenuAction::StartSinglePlayer`].
    pub map: &'static str,
    /// Whether the player's game data holds this map; an unavailable level
    /// is listed but cannot be started.
    pub available: bool,
}

/// One saved game the load pane lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveEntry {
    /// The slot, passed back in [`MenuAction::LoadSave`].
    pub slot: String,
    /// The first line shown, for example the chapter.
    pub title: String,
    /// The second line shown, for example the map and how long ago.
    pub detail: String,
}

/// Everything the host supplies for one frame of the menu.
#[derive(Debug, Clone, Copy)]
pub struct MenuData<'a> {
    /// Whether a game is running behind the menu (the pause menu).
    pub in_game: bool,
    /// Whether SAVE GAME is offered (not in a skirmish, and only with a
    /// save directory).
    pub can_save: bool,
    /// The map NEW GAME starts on, if the host has one.
    pub new_game_map: Option<&'static str>,
    /// The map HAZARD COURSE starts on, if the host has one.
    pub training_map: Option<&'static str>,
    /// The level-select pane's chapters.
    pub chapters: &'a [ChapterEntry],
    /// The deathmatch maps the skirmish and LAN server panes offer.
    pub skirmish_maps: &'a [String],
    /// The saved games the load pane lists, newest first.
    pub saves: &'a [SaveEntry],
    /// The resolutions the video options offer, largest first.
    pub resolutions: &'a [Resolution],
    /// Whether sound actually reaches an output device; the audio options
    /// say so when it does not.
    pub audio_output: bool,
}

impl Default for MenuData<'_> {
    fn default() -> Self {
        Self {
            in_game: false,
            can_save: false,
            new_game_map: None,
            training_map: None,
            chapters: &[],
            skirmish_maps: &[],
            saves: &[],
            resolutions: &[],
            audio_output: true,
        }
    }
}

/// Every pane of the menu tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuPane {
    /// The main menu's (or pause menu's) own button list.
    #[default]
    Root,
    /// The single-player hub: new game, level select, training, load.
    SinglePlayer,
    /// The new-game difficulty choice.
    NewGame,
    /// Chapter and level select.
    LevelSelect,
    /// The saved-game list.
    LoadGame,
    /// The multiplayer hub: skirmish, LAN server, LAN browser.
    Multiplayer,
    /// The offline skirmish setup.
    Skirmish,
    /// The LAN server setup (not networked yet).
    CreateServer,
    /// The LAN game browser (not networked yet).
    LanBrowser,
    /// Video, audio, controls and gameplay options.
    Options,
}

impl MenuPane {
    /// The pane's player-facing title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Root => "Main Menu",
            Self::SinglePlayer => "Single Player",
            Self::NewGame => "New Game",
            Self::LevelSelect => "Select Level",
            Self::LoadGame => "Load Game",
            Self::Multiplayer => "Multiplayer",
            Self::Skirmish => "Skirmish",
            Self::CreateServer => "Create LAN Server",
            Self::LanBrowser => "LAN Games",
            Self::Options => "Options",
        }
    }
}

/// A transient message shown at the bottom of the menu, for example after
/// saving.
#[derive(Debug, Clone, PartialEq)]
struct Notice {
    text: String,
    /// The egui time the notice was first drawn at; `None` until then.
    shown_at: Option<f64>,
}

/// How long a notice stays up, in seconds.
const NOTICE_SECONDS: f64 = 4.0;

/// Owns the menu's local navigation state (which pane, and the way back)
/// and the values its panes edit in place. `Screen` tracks which top-level
/// screen is active; this only matters while that screen is
/// [`Screen::MainMenu`] or [`Screen::Pause`].
#[derive(Debug, Clone)]
pub struct MenuState {
    /// The currently visible pane.
    pub pane: MenuPane,
    /// The panes BACK returns through, most recent last.
    history: Vec<MenuPane>,
    /// The options every pane edits; the host applies and saves them.
    pub options: OptionsState,
    /// Which options tab is showing.
    pub options_tab: OptionsTab,
    /// The display mode and resolution being edited, applied to
    /// [`OptionsState::video`] only by APPLY.
    pub display_draft: DisplaySettings,
    /// The action waiting for a key or mouse button, while rebinding.
    capturing: Option<Action>,
    /// Actions produced outside [`draw`] (a captured binding), reported by
    /// the next draw.
    pending: Vec<MenuAction>,
    /// Index into the host's chapter list.
    pub selected_chapter: usize,
    /// Index into the selected chapter's levels.
    pub selected_level: usize,
    /// Difficulty selected for a new single-player game.
    pub difficulty: Difficulty,
    /// Index into the host's save list, once one is picked.
    pub selected_save: Option<usize>,
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
    /// The LAN server pane's settings.
    pub lan_server: LanServerSettings,
    /// The LAN browser's address field.
    pub lan_address: String,
    /// The egui time the LAN browser's last search started, if any.
    lan_search_started: Option<f64>,
    notice: Option<Notice>,
    /// Whether a dropdown was open at the end of the last draw.
    popup_open: bool,
}

impl Default for MenuState {
    fn default() -> Self {
        let options = OptionsState::default();
        Self {
            pane: MenuPane::default(),
            history: Vec::new(),
            display_draft: options.video.display,
            options,
            options_tab: OptionsTab::default(),
            capturing: None,
            pending: Vec::new(),
            selected_chapter: 0,
            selected_level: 0,
            difficulty: Difficulty::default(),
            selected_save: None,
            selected_skirmish_map: 0,
            skirmish_bots: SKIRMISH_BOTS_DEFAULT,
            skirmish_skill: BotSkill::default(),
            skirmish_frag_limit: SKIRMISH_FRAG_LIMIT_DEFAULT,
            skirmish_time_limit_minutes: SKIRMISH_TIME_LIMIT_DEFAULT_MINUTES,
            lan_server: LanServerSettings::default(),
            lan_address: String::new(),
            lan_search_started: None,
            notice: None,
            popup_open: false,
        }
    }
}

impl MenuState {
    /// Creates a menu at its root pane with default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a menu at its root pane editing `options` (for example the
    /// player's saved settings).
    #[must_use]
    pub fn with_options(options: OptionsState) -> Self {
        Self {
            display_draft: options.video.display,
            options,
            ..Self::default()
        }
    }

    /// Opens `pane`, remembering the current one for [`Self::back`].
    pub fn open(&mut self, pane: MenuPane) {
        if pane == self.pane {
            return;
        }
        if pane == MenuPane::Options {
            self.display_draft = self.options.video.display;
        }
        self.capturing = None;
        self.history.push(self.pane);
        self.pane = pane;
    }

    /// Goes back one step: cancels a key capture first, then returns to the
    /// previous pane. Returns `false` (doing nothing) at the root, where
    /// BACK has nowhere left to go.
    pub fn back(&mut self) -> bool {
        if self.capturing.take().is_some() {
            return true;
        }
        match self.history.pop() {
            Some(previous) => {
                self.pane = previous;
                true
            }
            None if self.pane != MenuPane::Root => {
                self.pane = MenuPane::Root;
                true
            }
            None => false,
        }
    }

    /// Returns to the root pane and forgets the way back, as when a game
    /// starts or ends.
    pub fn reset(&mut self) {
        self.pane = MenuPane::Root;
        self.history.clear();
        self.capturing = None;
    }

    /// The panes leading to the current one, root first, for the
    /// breadcrumb.
    #[must_use]
    pub fn trail(&self) -> Vec<MenuPane> {
        let mut trail: Vec<MenuPane> = self
            .history
            .iter()
            .copied()
            .filter(|pane| *pane != MenuPane::Root)
            .collect();
        trail.push(self.pane);
        trail
    }

    /// Shows `text` at the bottom of the menu for a few seconds.
    pub fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            shown_at: None,
        });
    }

    /// The notice currently queued or showing, if any.
    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    /// The action waiting for a key or mouse button, while the controls
    /// tab is rebinding one. The host routes the next press to
    /// [`Self::capture_binding`] instead of gameplay or the menu.
    #[must_use]
    pub fn capturing_binding(&self) -> Option<Action> {
        self.capturing
    }

    /// Binds the action being captured to `binding` and reports
    /// [`MenuAction::OptionsChanged`] on the next draw. Returns `false`,
    /// leaving the capture running, when nothing is being captured or
    /// `binding` is `None` (a key no binding may use).
    pub fn capture_binding(&mut self, binding: Option<Binding>) -> bool {
        let (Some(action), Some(binding)) = (self.capturing, binding) else {
            return false;
        };
        self.options.controls.bindings.bind(action, binding);
        self.capturing = None;
        self.pending.push(MenuAction::OptionsChanged);
        true
    }

    /// Stops waiting for a binding, keeping the old one.
    pub fn cancel_capture(&mut self) {
        self.capturing = None;
    }

    /// Whether a dropdown (a combo box's list) was open when the menu was
    /// last drawn. `Escape` then only closes it, which egui does itself, so
    /// the host should not also step back a pane.
    #[must_use]
    pub fn popup_open(&self) -> bool {
        self.popup_open
    }
}

/// Draws the menu (main or pause, depending on [`MenuData::in_game`]) and
/// returns the actions the player triggered this frame. `ui` is the frame's
/// root `Ui`; see [`crate::root_ui`].
pub fn draw(ui: &mut egui::Ui, state: &mut MenuState, data: &MenuData<'_>) -> Vec<MenuAction> {
    let mut actions = std::mem::take(&mut state.pending);
    let now = ui.ctx().input(|input| input.time);
    let backdrop = if data.in_game {
        theme::pause_backdrop()
    } else {
        theme::BACKDROP
    };
    theme::with_style(ui, |ui| {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(backdrop))
            .show(ui, |ui| match state.pane {
                MenuPane::Root => draw_root(ui, state, data, &mut actions),
                pane => theme::page(ui, state, |ui, state| match pane {
                    MenuPane::Root => {}
                    MenuPane::SinglePlayer => {
                        single_player::draw_hub(ui, state, data, &mut actions);
                    }
                    MenuPane::NewGame => {
                        single_player::draw_new_game(ui, state, data, &mut actions);
                    }
                    MenuPane::LevelSelect => {
                        single_player::draw_level_select(ui, state, data, &mut actions);
                    }
                    MenuPane::LoadGame => single_player::draw_load(ui, state, data, &mut actions),
                    MenuPane::Multiplayer => multiplayer::draw_hub(ui, state),
                    MenuPane::Skirmish => {
                        multiplayer::draw_skirmish(ui, state, data, &mut actions);
                    }
                    MenuPane::CreateServer => {
                        multiplayer::draw_create_server(ui, state, data, &mut actions);
                    }
                    MenuPane::LanBrowser => {
                        multiplayer::draw_lan_browser(ui, state, now, &mut actions);
                    }
                    MenuPane::Options => options::draw(ui, state, data, &mut actions),
                }),
            });
        draw_notice(ui.ctx(), state, now);
        state.popup_open = egui::Popup::is_any_open(ui.ctx());
    });
    actions
}

/// The root pane: the title and the top-level buttons.
fn draw_root(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    let full = ui.max_rect();
    let column = egui::Rect::from_min_size(
        egui::pos2(
            full.left() + (full.width() * 0.09).max(24.0),
            full.top() + (full.height() * 0.12).max(16.0),
        ),
        Vec2::new(theme::NAV_WIDTH + 40.0, full.height()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new("λ").size(64.0).color(theme::ACCENT));
            ui.vertical(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("OPEN HALF-LIFE")
                        .size(30.0)
                        .strong()
                        .color(theme::TITLE),
                );
                if data.in_game {
                    ui.label(RichText::new("Paused").size(16.0).color(theme::TEXT_DIM));
                }
            });
        });
        ui.add_space(26.0);
        if data.in_game {
            if theme::nav_button(ui, "Resume Game").clicked() {
                actions.push(MenuAction::Resume);
            }
            if ui
                .add_enabled_ui(data.can_save, |ui| theme::nav_button(ui, "Save Game"))
                .inner
                .clicked()
            {
                actions.push(MenuAction::SaveGame);
            }
            if theme::nav_button(ui, "Load Game").clicked() {
                state.open(MenuPane::LoadGame);
            }
            ui.add_space(10.0);
        }
        if theme::nav_button(ui, "Single Player").clicked() {
            state.open(MenuPane::SinglePlayer);
        }
        if theme::nav_button(ui, "Multiplayer").clicked() {
            state.open(MenuPane::Multiplayer);
        }
        if theme::nav_button(ui, "Options").clicked() {
            state.open(MenuPane::Options);
        }
        ui.add_space(10.0);
        if data.in_game && theme::nav_button(ui, "Main Menu").clicked() {
            actions.push(MenuAction::ReturnToMainMenu);
        }
        if theme::nav_button(ui, "Quit").clicked() {
            actions.push(MenuAction::Quit);
        }
    });
    // A large, faint lambda fills the empty side of the screen.
    ui.painter().text(
        egui::pos2(
            full.left() + full.width() * 0.72,
            full.top() + full.height() * 0.5,
        ),
        Align2::CENTER_CENTER,
        "λ",
        egui::FontId::proportional((full.height() * 0.62).max(120.0)),
        Color32::from_rgba_unmultiplied(246, 154, 38, 14),
    );
    ui.painter().text(
        full.right_bottom() - Vec2::new(16.0, 12.0),
        Align2::RIGHT_BOTTOM,
        "Requires your own Half-Life game data",
        egui::FontId::proportional(13.0),
        theme::TEXT_DIM,
    );
}

/// The notice toast, while one is up.
fn draw_notice(ctx: &egui::Context, state: &mut MenuState, now: f64) {
    let Some(notice) = state.notice.as_mut() else {
        return;
    };
    let shown_at = *notice.shown_at.get_or_insert(now);
    if now - shown_at > NOTICE_SECONDS {
        state.notice = None;
        return;
    }
    let text = notice.text.clone();
    egui::Area::new(egui::Id::new("ohl_menu_notice"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -36.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(20, 26, 21, 240))
                .stroke(egui::Stroke::new(1.0, theme::ACCENT))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(18, 10))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).size(16.0).color(theme::TITLE));
                });
        });
}

#[cfg(test)]
mod tests;
