//! The options: what the player can set, the plain-text settings file it
//! is kept in, and the tabbed pane that edits it.
//!
//! Every value is bounded here, whatever the source (a slider or a settings
//! file someone edited by hand), so the host can apply what it is handed
//! without re-checking it. The settings file is `key = value` lines; a
//! line this build does not recognise, or whose value does not parse, is
//! skipped and that setting keeps its default.

use std::fmt::Write as _;

use egui::{RichText, Slider, Vec2};

use super::theme;
use super::{MenuAction, MenuData, MenuState};
use crate::bindings::{Action, ActionGroup, Binding, Bindings};

/// How the window is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    /// A normal, resizable window of the chosen resolution.
    #[default]
    Windowed,
    /// A borderless window covering the monitor at the desktop resolution.
    Borderless,
    /// Exclusive fullscreen at the chosen resolution.
    Fullscreen,
}

impl DisplayMode {
    /// Every mode, in the order the options pane offers them.
    pub const ALL: [Self; 3] = [Self::Windowed, Self::Borderless, Self::Fullscreen];

    /// The player-facing name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::Borderless => "Borderless",
            Self::Fullscreen => "Fullscreen",
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Windowed => "windowed",
            Self::Borderless => "borderless",
            Self::Fullscreen => "fullscreen",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

/// A display resolution in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    /// The smallest resolution accepted on either axis.
    pub const MIN: u32 = 320;
    /// The largest resolution accepted on either axis.
    pub const MAX: u32 = 16_384;

    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// The player-facing form, `1920 × 1080`.
    #[must_use]
    pub fn label(self) -> String {
        format!("{} × {}", self.width, self.height)
    }

    /// Parses `1920x1080`, rejecting a size outside [`Self::MIN`]..=[`Self::MAX`].
    fn parse(text: &str) -> Option<Self> {
        let (width, height) = text.split_once('x')?;
        let width: u32 = width.trim().parse().ok()?;
        let height: u32 = height.trim().parse().ok()?;
        let bounds = Self::MIN..=Self::MAX;
        (bounds.contains(&width) && bounds.contains(&height)).then_some(Self { width, height })
    }
}

/// The display mode and resolution: the options that need APPLY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySettings {
    pub mode: DisplayMode,
    /// Ignored in [`DisplayMode::Borderless`], which uses the desktop's.
    pub resolution: Resolution,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Windowed,
            resolution: Resolution::new(1280, 720),
        }
    }
}

/// The frame-rate caps offered; `0` is no cap.
pub const FPS_LIMITS: [u32; 8] = [0, 30, 60, 75, 120, 144, 165, 240];
/// Vertical field-of-view bounds, in degrees.
pub const FOV_RANGE: std::ops::RangeInclusive<f32> = 60.0..=100.0;
/// The vertical field of view a fresh install uses, in degrees: the
/// renderer camera's own default.
pub const DEFAULT_FOV: f32 = 75.0;
/// Volume bounds.
pub const VOLUME_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1.0;
/// Mouse sensitivity bounds. `3.0` is the engine's own turn rate; the
/// host scales mouse motion by `sensitivity / 3.0`.
pub const SENSITIVITY_RANGE: std::ops::RangeInclusive<f32> = 0.1..=10.0;

/// Video options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoOptions {
    pub display: DisplaySettings,
    /// Wait for vertical blank when presenting.
    pub vsync: bool,
    /// Frames per second cap, one of [`FPS_LIMITS`]; `0` is uncapped.
    pub fps_limit: u32,
    /// Vertical field of view in degrees, within [`FOV_RANGE`].
    pub fov: f32,
    /// Show the frame-time and renderer overlay.
    pub performance_overlay: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            display: DisplaySettings::default(),
            vsync: true,
            fps_limit: 0,
            fov: DEFAULT_FOV,
            performance_overlay: false,
        }
    }
}

/// Audio options; each volume is within [`VOLUME_RANGE`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioOptions {
    /// Scales everything.
    pub master: f32,
    /// Weapons, impacts, items and everything else not below.
    pub effects: f32,
    /// Speech.
    pub voice: f32,
    /// Map ambience and music.
    pub ambience: f32,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            master: 1.0,
            effects: 1.0,
            voice: 1.0,
            ambience: 1.0,
        }
    }
}

/// Mouse and keyboard options.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlOptions {
    /// Within [`SENSITIVITY_RANGE`].
    pub sensitivity: f32,
    /// Moving the mouse forward looks down.
    pub invert_mouse: bool,
    pub bindings: Bindings,
}

impl Default for ControlOptions {
    fn default() -> Self {
        Self {
            sensitivity: 3.0,
            invert_mouse: false,
            bindings: Bindings::default(),
        }
    }
}

/// Gameplay options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameplayOptions {
    /// Draw the crosshair.
    pub crosshair: bool,
    /// Save automatically after every level change.
    pub autosave: bool,
    /// Let the backquote key open the developer console.
    pub console: bool,
}

impl Default for GameplayOptions {
    fn default() -> Self {
        Self {
            crosshair: true,
            autosave: true,
            console: true,
        }
    }
}

/// Everything the options pane edits and the settings file keeps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptionsState {
    pub video: VideoOptions,
    pub audio: AudioOptions,
    pub controls: ControlOptions,
    pub gameplay: GameplayOptions,
}

/// The settings file's first line.
const SETTINGS_HEADER: &str = "# Open Half-Life settings, written by the options menu.";

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Parses a finite number and clamps it into `range`.
fn parse_bounded(value: &str, range: &std::ops::RangeInclusive<f32>) -> Option<f32> {
    let parsed: f32 = value.parse().ok()?;
    parsed
        .is_finite()
        .then(|| parsed.clamp(*range.start(), *range.end()))
}

impl OptionsState {
    /// Every value clamped into its bounds, so whatever produced them the
    /// host may apply them as they are.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let clamp = |value: f32, range: &std::ops::RangeInclusive<f32>, default: f32| {
            if value.is_finite() {
                value.clamp(*range.start(), *range.end())
            } else {
                default
            }
        };
        let defaults = Self::default();
        self.video.fov = clamp(self.video.fov, &FOV_RANGE, defaults.video.fov);
        if !FPS_LIMITS.contains(&self.video.fps_limit) {
            self.video.fps_limit = defaults.video.fps_limit;
        }
        for volume in [
            &mut self.audio.master,
            &mut self.audio.effects,
            &mut self.audio.voice,
            &mut self.audio.ambience,
        ] {
            *volume = clamp(*volume, &VOLUME_RANGE, 1.0);
        }
        self.controls.sensitivity = clamp(
            self.controls.sensitivity,
            &SENSITIVITY_RANGE,
            defaults.controls.sensitivity,
        );
        self
    }

    /// The settings file's text for these options.
    #[must_use]
    pub fn to_settings_text(&self) -> String {
        let mut text = String::new();
        let video = &self.video;
        let audio = &self.audio;
        let controls = &self.controls;
        let gameplay = &self.gameplay;
        let _ = writeln!(text, "{SETTINGS_HEADER}");
        let _ = writeln!(text, "video.display_mode = {}", video.display.mode.id());
        let resolution = video.display.resolution;
        let _ = writeln!(
            text,
            "video.resolution = {}x{}",
            resolution.width, resolution.height
        );
        let _ = writeln!(text, "video.vsync = {}", video.vsync);
        let _ = writeln!(text, "video.fps_limit = {}", video.fps_limit);
        let _ = writeln!(text, "video.fov = {}", video.fov);
        let _ = writeln!(
            text,
            "video.performance_overlay = {}",
            video.performance_overlay
        );
        let _ = writeln!(text, "audio.master = {}", audio.master);
        let _ = writeln!(text, "audio.effects = {}", audio.effects);
        let _ = writeln!(text, "audio.voice = {}", audio.voice);
        let _ = writeln!(text, "audio.ambience = {}", audio.ambience);
        let _ = writeln!(text, "controls.sensitivity = {}", controls.sensitivity);
        let _ = writeln!(text, "controls.invert_mouse = {}", controls.invert_mouse);
        for action in Action::ALL {
            let binding = controls
                .bindings
                .get(action)
                .map_or_else(|| "none".to_owned(), Binding::id);
            let _ = writeln!(text, "bind.{} = {binding}", action.id());
        }
        let _ = writeln!(text, "gameplay.crosshair = {}", gameplay.crosshair);
        let _ = writeln!(text, "gameplay.autosave = {}", gameplay.autosave);
        let _ = writeln!(text, "gameplay.console = {}", gameplay.console);
        text
    }

    /// Reads a settings file's text: defaults, overridden by every line
    /// this build recognises and can parse, bounded as [`Self::sanitized`].
    #[must_use]
    pub fn from_settings_text(text: &str) -> Self {
        let mut options = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            options.apply_setting(key.trim(), value.trim());
        }
        options.sanitized()
    }

    /// Applies one `key = value` line, ignoring it when either part is not
    /// understood.
    fn apply_setting(&mut self, key: &str, value: &str) {
        if let Some(action) = key.strip_prefix("bind.").and_then(Action::from_id) {
            if value == "none" {
                self.controls.bindings.unbind(action);
            } else if let Some(binding) = Binding::from_id(value) {
                self.controls.bindings.bind(action, binding);
            }
            return;
        }
        match key {
            "video.display_mode" => {
                if let Some(mode) = DisplayMode::from_id(value) {
                    self.video.display.mode = mode;
                }
            }
            "video.resolution" => {
                if let Some(resolution) = Resolution::parse(value) {
                    self.video.display.resolution = resolution;
                }
            }
            "video.vsync" => set_bool(&mut self.video.vsync, value),
            "video.fps_limit" => {
                if let Some(limit) = value
                    .parse()
                    .ok()
                    .filter(|limit| FPS_LIMITS.contains(limit))
                {
                    self.video.fps_limit = limit;
                }
            }
            "video.fov" => set_bounded(&mut self.video.fov, value, &FOV_RANGE),
            "video.performance_overlay" => set_bool(&mut self.video.performance_overlay, value),
            "audio.master" => set_bounded(&mut self.audio.master, value, &VOLUME_RANGE),
            "audio.effects" => set_bounded(&mut self.audio.effects, value, &VOLUME_RANGE),
            "audio.voice" => set_bounded(&mut self.audio.voice, value, &VOLUME_RANGE),
            "audio.ambience" => set_bounded(&mut self.audio.ambience, value, &VOLUME_RANGE),
            "controls.sensitivity" => {
                set_bounded(&mut self.controls.sensitivity, value, &SENSITIVITY_RANGE);
            }
            "controls.invert_mouse" => set_bool(&mut self.controls.invert_mouse, value),
            "gameplay.crosshair" => set_bool(&mut self.gameplay.crosshair, value),
            "gameplay.autosave" => set_bool(&mut self.gameplay.autosave, value),
            "gameplay.console" => set_bool(&mut self.gameplay.console, value),
            _ => {}
        }
    }
}

fn set_bool(field: &mut bool, value: &str) {
    if let Some(parsed) = parse_bool(value) {
        *field = parsed;
    }
}

fn set_bounded(field: &mut f32, value: &str, range: &std::ops::RangeInclusive<f32>) {
    if let Some(parsed) = parse_bounded(value, range) {
        *field = parsed;
    }
}

/// The options pane's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionsTab {
    #[default]
    Video,
    Audio,
    Controls,
    Gameplay,
}

impl OptionsTab {
    /// Every tab, in display order.
    pub const ALL: [Self; 4] = [Self::Video, Self::Audio, Self::Controls, Self::Gameplay];

    /// The tab's label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Controls => "Controls",
            Self::Gameplay => "Gameplay",
        }
    }
}

/// Draws the options pane.
pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    ui.horizontal(|ui| {
        for tab in OptionsTab::ALL {
            let text = RichText::new(tab.label().to_uppercase()).size(16.0);
            if ui
                .add(
                    egui::Button::selectable(state.options_tab == tab, text)
                        .min_size(Vec2::new(120.0, 30.0)),
                )
                .clicked()
            {
                state.options_tab = tab;
                state.capturing = None;
            }
        }
    });
    ui.add_space(4.0);
    let before = state.options.clone();
    theme::content(ui, |ui| {
        egui::Grid::new(("ohl_menu_options", state.options_tab as u8))
            .num_columns(2)
            .spacing([28.0, 12.0])
            .min_col_width(200.0)
            .show(ui, |ui| match state.options_tab {
                OptionsTab::Video => draw_video(ui, state, data, actions),
                OptionsTab::Audio => draw_audio(ui, &mut state.options.audio, data),
                OptionsTab::Controls => draw_controls(ui, state),
                OptionsTab::Gameplay => draw_gameplay(ui, &mut state.options.gameplay),
            });
    });
    theme::footer(ui, state, |ui, state| {
        if theme::button(ui, "Restore Defaults").clicked() {
            restore_defaults(state);
        }
    });
    if state.options != before {
        actions.push(MenuAction::OptionsChanged);
    }
}

/// Puts the current tab's options back to their defaults. The video tab's
/// display mode and resolution go back in the draft only, to be applied.
fn restore_defaults(state: &mut MenuState) {
    let defaults = OptionsState::default();
    match state.options_tab {
        OptionsTab::Video => {
            state.display_draft = defaults.video.display;
            state.options.video = VideoOptions {
                display: state.options.video.display,
                ..defaults.video
            };
        }
        OptionsTab::Audio => state.options.audio = defaults.audio,
        OptionsTab::Controls => {
            state.capturing = None;
            state.options.controls = defaults.controls;
        }
        OptionsTab::Gameplay => state.options.gameplay = defaults.gameplay,
    }
}

fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0));
}

fn percent_slider(ui: &mut egui::Ui, value: &mut f32) -> egui::Response {
    ui.add(
        Slider::new(value, VOLUME_RANGE)
            .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
            .custom_parser(|text| {
                text.trim()
                    .trim_end_matches('%')
                    .parse::<f64>()
                    .ok()
                    .map(|percent| percent / 100.0)
            }),
    )
}

fn fps_label(limit: u32) -> String {
    if limit == 0 {
        "Unlimited".to_owned()
    } else {
        format!("{limit} FPS")
    }
}

fn draw_video(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    row_label(ui, "Display mode");
    ui.horizontal(|ui| {
        for mode in DisplayMode::ALL {
            ui.selectable_value(&mut state.display_draft.mode, mode, mode.label());
        }
    });
    ui.end_row();

    row_label(ui, "Resolution");
    let borderless = state.display_draft.mode == DisplayMode::Borderless;
    ui.add_enabled_ui(!borderless, |ui| {
        let current = state.display_draft.resolution;
        egui::ComboBox::from_id_salt("ohl_menu_resolution")
            .width(200.0)
            .selected_text(if borderless {
                "Desktop".to_owned()
            } else {
                current.label()
            })
            .show_ui(ui, |ui| {
                if !data.resolutions.contains(&current) {
                    ui.selectable_value(
                        &mut state.display_draft.resolution,
                        current,
                        current.label(),
                    );
                }
                for resolution in data.resolutions {
                    ui.selectable_value(
                        &mut state.display_draft.resolution,
                        *resolution,
                        resolution.label(),
                    );
                }
            });
    });
    ui.end_row();

    ui.label("");
    let pending = state.display_draft != state.options.video.display;
    ui.horizontal(|ui| {
        if ui
            .add_enabled_ui(pending, |ui| theme::button(ui, "Apply"))
            .inner
            .clicked()
        {
            state.options.video.display = state.display_draft;
            actions.push(MenuAction::ApplyDisplay(state.display_draft));
        }
        if pending {
            theme::hint(ui, "Press Apply to use the new display settings.");
        }
    });
    ui.end_row();

    row_label(ui, "Vertical sync");
    ui.checkbox(
        &mut state.options.video.vsync,
        "Wait for the display's refresh",
    );
    ui.end_row();

    row_label(ui, "Frame rate limit");
    let limit = &mut state.options.video.fps_limit;
    egui::ComboBox::from_id_salt("ohl_menu_fps_limit")
        .width(200.0)
        .selected_text(fps_label(*limit))
        .show_ui(ui, |ui| {
            for candidate in FPS_LIMITS {
                ui.selectable_value(limit, candidate, fps_label(candidate));
            }
        });
    ui.end_row();

    row_label(ui, "Field of view (vertical)");
    ui.add(
        Slider::new(&mut state.options.video.fov, FOV_RANGE)
            .step_by(1.0)
            .fixed_decimals(0)
            .suffix("°"),
    );
    ui.end_row();

    row_label(ui, "Performance overlay");
    ui.checkbox(
        &mut state.options.video.performance_overlay,
        "Frame times and renderer statistics",
    );
    ui.end_row();
}

fn draw_audio(ui: &mut egui::Ui, audio: &mut AudioOptions, data: &MenuData<'_>) {
    if !data.audio_output {
        ui.label("");
        ui.label(
            RichText::new(
                "No audio output is available in this build, so nothing is heard. \
                 These settings are kept for when it is.",
            )
            .color(theme::WARNING),
        );
        ui.end_row();
    }
    for (label, value) in [
        ("Master volume", &mut audio.master),
        ("Effects", &mut audio.effects),
        ("Voices", &mut audio.voice),
        ("Ambience", &mut audio.ambience),
    ] {
        row_label(ui, label);
        percent_slider(ui, value);
        ui.end_row();
    }
}

fn draw_controls(ui: &mut egui::Ui, state: &mut MenuState) {
    let controls = &mut state.options.controls;
    row_label(ui, "Mouse sensitivity");
    ui.add(Slider::new(&mut controls.sensitivity, SENSITIVITY_RANGE).step_by(0.1));
    ui.end_row();

    row_label(ui, "Invert mouse");
    ui.checkbox(
        &mut controls.invert_mouse,
        "Moving the mouse forward looks down",
    );
    ui.end_row();

    ui.label("");
    theme::hint(
        ui,
        "Click a binding, then press a key or mouse button. Esc cancels; right-click clears.",
    );
    ui.end_row();

    for group in ActionGroup::ALL {
        ui.label(
            RichText::new(group.label())
                .size(17.0)
                .strong()
                .color(theme::ACCENT),
        );
        ui.end_row();
        for action in Action::ALL
            .into_iter()
            .filter(|action| action.group() == group)
        {
            row_label(ui, action.label());
            let capturing = state.capturing == Some(action);
            let text = if capturing {
                RichText::new("Press a key…").color(theme::ACCENT)
            } else {
                match state.options.controls.bindings.get(action) {
                    Some(binding) => RichText::new(binding.label()),
                    None => RichText::new("—").color(theme::TEXT_DIM),
                }
            };
            let response = ui.add(
                egui::Button::selectable(capturing, text)
                    .frame_when_inactive(true)
                    .min_size(Vec2::new(160.0, 26.0)),
            );
            if response.clicked() {
                state.capturing = Some(action);
            }
            if response.secondary_clicked() {
                state.capturing = None;
                state.options.controls.bindings.unbind(action);
            }
            ui.end_row();
        }
    }

    ui.label(
        RichText::new("Fixed")
            .size(17.0)
            .strong()
            .color(theme::ACCENT),
    );
    ui.end_row();
    for (label, key) in [("Menu / pause", "Esc"), ("Developer console", "`")] {
        row_label(ui, label);
        ui.label(RichText::new(key).color(theme::TEXT_DIM));
        ui.end_row();
    }
}

fn draw_gameplay(ui: &mut egui::Ui, gameplay: &mut GameplayOptions) {
    row_label(ui, "Crosshair");
    ui.checkbox(&mut gameplay.crosshair, "Show the crosshair");
    ui.end_row();

    row_label(ui, "Autosave");
    ui.checkbox(
        &mut gameplay.autosave,
        "Save automatically at every level change",
    );
    ui.end_row();

    row_label(ui, "Developer console");
    ui.checkbox(&mut gameplay.console, "Open the console with the ` key");
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayMode, FOV_RANGE, OptionsState, Resolution, SENSITIVITY_RANGE, VOLUME_RANGE,
    };
    use crate::bindings::{Action, Binding};
    use winit::event::MouseButton;
    use winit::keyboard::KeyCode;

    #[test]
    fn default_options_are_within_their_own_ranges() {
        let options = OptionsState::default();
        assert!(SENSITIVITY_RANGE.contains(&options.controls.sensitivity));
        assert!(VOLUME_RANGE.contains(&options.audio.master));
        assert!(FOV_RANGE.contains(&options.video.fov));
        assert_eq!(options.clone().sanitized(), options);
    }

    #[test]
    fn the_settings_text_round_trips_every_option() {
        let mut options = OptionsState::default();
        options.video.display.mode = DisplayMode::Fullscreen;
        options.video.display.resolution = Resolution::new(1920, 1080);
        options.video.vsync = false;
        options.video.fps_limit = 144;
        options.video.fov = 90.0;
        options.video.performance_overlay = true;
        options.audio.master = 0.5;
        options.audio.effects = 0.25;
        options.audio.voice = 0.75;
        options.audio.ambience = 0.0;
        options.controls.sensitivity = 4.5;
        options.controls.invert_mouse = true;
        options
            .controls
            .bindings
            .bind(Action::Jump, Binding::Mouse(MouseButton::Right));
        options.controls.bindings.unbind(Action::QuickLoad);
        options
            .controls
            .bindings
            .bind(Action::MoveForward, Binding::Key(KeyCode::ArrowUp));
        options.gameplay.crosshair = false;
        options.gameplay.autosave = false;
        options.gameplay.console = false;

        let text = options.to_settings_text();
        assert_eq!(OptionsState::from_settings_text(&text), options);
    }

    #[test]
    fn unknown_or_malformed_lines_keep_their_defaults() {
        let options = OptionsState::from_settings_text(
            "garbage\n\
             # a comment\n\
             video.fov = banana\n\
             video.resolution = 10x10\n\
             video.display_mode = sideways\n\
             video.fps_limit = 77\n\
             audio.master = NaN\n\
             controls.invert_mouse = maybe\n\
             bind.jump = Escape\n\
             bind.nothing = KeyW\n\
             future.setting = 3\n",
        );
        assert_eq!(options, OptionsState::default());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let options = OptionsState::from_settings_text(
            "video.fov = 500\naudio.voice = -3\ncontrols.sensitivity = 0\n",
        );
        assert!((options.video.fov - FOV_RANGE.end()).abs() < f32::EPSILON);
        assert!(options.audio.voice.abs() < f32::EPSILON);
        assert!((options.controls.sensitivity - SENSITIVITY_RANGE.start()).abs() < f32::EPSILON);
    }

    #[test]
    fn a_binding_read_later_takes_the_key_from_an_earlier_action() {
        let options = OptionsState::from_settings_text("bind.jump = KeyW\n");
        let bindings = &options.controls.bindings;
        assert_eq!(bindings.action_for_key(KeyCode::KeyW), Some(Action::Jump));
        assert_eq!(bindings.get(Action::MoveForward), None);
    }
}
