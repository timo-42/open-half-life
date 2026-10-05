//! The multiplayer panes: the hub, the offline skirmish setup, and the LAN
//! server and LAN browser panes.
//!
//! The skirmish pane picks one of the host-supplied deathmatch maps, a bot
//! count, a bot skill, a frag limit and a time limit, then reports
//! [`MenuAction::StartSkirmish`]. With no maps supplied it explains that and
//! keeps the start button disabled.
//!
//! Nothing is networked yet. The LAN panes are previews: they gather what a
//! LAN game would need and report [`MenuAction::HostLanServer`] or
//! [`MenuAction::JoinLanServer`], and say plainly that LAN play does not
//! exist yet.

use egui::{Slider, Vec2};

use super::theme;
use super::{MenuAction, MenuData, MenuPane, MenuState};

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

/// Player-count bounds the LAN server pane offers.
pub const LAN_MAX_PLAYERS_RANGE: std::ops::RangeInclusive<u8> = 2..=32;

/// How long the LAN browser "searches" before reporting what it found, in
/// seconds.
const LAN_SEARCH_SECONDS: f64 = 1.5;

/// What the LAN server pane gathers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanServerSettings {
    /// The name other players would see.
    pub name: String,
    /// Index into the host's deathmatch map list.
    pub map: usize,
    /// The most players the game admits.
    pub max_players: u8,
    /// An optional password; empty for none.
    pub password: String,
}

impl Default for LanServerSettings {
    fn default() -> Self {
        Self {
            name: "Open Half-Life LAN Game".to_owned(),
            map: 0,
            max_players: 8,
            password: String::new(),
        }
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

/// The multiplayer hub.
pub(super) fn draw_hub(ui: &mut egui::Ui, state: &mut MenuState) {
    theme::content(ui, |ui| {
        ui.add_space(8.0);
        if theme::hub_entry(ui, "Skirmish", "Deathmatch against bots on this computer").clicked() {
            state.open(MenuPane::Skirmish);
        }
        if theme::hub_entry(ui, "Create LAN Server", "Host a game on your local network").clicked()
        {
            state.open(MenuPane::CreateServer);
        }
        if theme::hub_entry(ui, "Find LAN Games", "Join a game on your local network").clicked() {
            state.open(MenuPane::LanBrowser);
        }
        ui.add_space(12.0);
        theme::hint(
            ui,
            "LAN play is a preview: its panes work, but nothing is networked yet.",
        );
    });
    theme::footer(ui, state, |_, _| {});
}

/// The host's deathmatch maps as a selectable list, or a line saying there
/// are none.
fn map_list(ui: &mut egui::Ui, selected: &mut usize, maps: &[String], height: f32) {
    let Some(last) = maps.len().checked_sub(1) else {
        theme::hint(
            ui,
            "No deathmatch maps were found in the imported game data.",
        );
        return;
    };
    *selected = (*selected).min(last);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("ohl_menu_dm_maps")
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    for (index, name) in maps.iter().enumerate() {
                        ui.selectable_value(selected, index, name);
                    }
                });
            });
    });
}

/// The offline skirmish setup pane: map list, bot count, bot skill, frag
/// and time limits, then START SKIRMISH.
pub(super) fn draw_skirmish(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    let maps = data.skirmish_maps;
    theme::content(ui, |ui| {
        ui.columns(2, |columns| {
            theme::heading(&mut columns[0], "Map");
            map_list(
                &mut columns[0],
                &mut state.selected_skirmish_map,
                maps,
                300.0,
            );

            let ui = &mut columns[1];
            theme::heading(ui, "Bots");
            ui.add(Slider::new(&mut state.skirmish_bots, SKIRMISH_BOTS_RANGE));
            theme::heading(ui, "Bot skill");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.skirmish_skill, BotSkill::Easy, "Easy");
                ui.selectable_value(&mut state.skirmish_skill, BotSkill::Normal, "Normal");
                ui.selectable_value(&mut state.skirmish_skill, BotSkill::Hard, "Hard");
            });
            theme::heading(ui, "Frag limit");
            ui.add(Slider::new(
                &mut state.skirmish_frag_limit,
                SKIRMISH_FRAG_LIMIT_RANGE,
            ));
            theme::hint(ui, "0 = no limit");
            theme::heading(ui, "Time limit (minutes)");
            ui.add(Slider::new(
                &mut state.skirmish_time_limit_minutes,
                SKIRMISH_TIME_LIMIT_RANGE,
            ));
            theme::hint(ui, "0 = no limit");
        });
    });
    theme::footer(ui, state, |ui, state| {
        if ui
            .add_enabled_ui(!maps.is_empty(), |ui| theme::button(ui, "Start Skirmish"))
            .inner
            .clicked()
        {
            actions.extend(skirmish_action(state, maps));
        }
    });
}

/// The LAN server setup pane.
pub(super) fn draw_create_server(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    let maps = data.skirmish_maps;
    theme::content(ui, |ui| {
        theme::preview_banner(
            ui,
            "Preview: hosting LAN games is not implemented yet. Use Skirmish to play \
             against bots offline.",
        );
        ui.add_space(6.0);
        ui.columns(2, |columns| {
            let settings = &mut state.lan_server;
            let ui = &mut columns[0];
            theme::heading(ui, "Server name");
            ui.add(
                egui::TextEdit::singleline(&mut settings.name)
                    .margin(egui::Margin::symmetric(6, 5))
                    .char_limit(48)
                    .desired_width(f32::INFINITY),
            );
            theme::heading(ui, "Max players");
            ui.add(Slider::new(
                &mut settings.max_players,
                LAN_MAX_PLAYERS_RANGE,
            ));
            theme::heading(ui, "Password");
            ui.add(
                egui::TextEdit::singleline(&mut settings.password)
                    .margin(egui::Margin::symmetric(6, 5))
                    .password(true)
                    .char_limit(32)
                    .hint_text("none")
                    .desired_width(f32::INFINITY),
            );

            theme::heading(&mut columns[1], "Map");
            map_list(&mut columns[1], &mut settings.map, maps, 220.0);
        });
    });
    theme::footer(ui, state, |ui, state| {
        if ui
            .add_enabled_ui(!maps.is_empty(), |ui| theme::button(ui, "Start Server"))
            .inner
            .clicked()
        {
            actions.push(MenuAction::HostLanServer(LanServerSettings {
                name: state.lan_server.name.trim().to_owned(),
                ..state.lan_server.clone()
            }));
        }
    });
}

/// The LAN browser: an empty game list, a REFRESH that searches (and finds
/// nothing, there being no LAN play yet) and a connect-by-address field.
pub(super) fn draw_lan_browser(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    now: f64,
    actions: &mut Vec<MenuAction>,
) {
    let searching = state
        .lan_search_started
        .is_some_and(|started| now - started < LAN_SEARCH_SECONDS);
    theme::content(ui, |ui| {
        theme::preview_banner(
            ui,
            "Preview: joining LAN games is not implemented yet, so no games can be found.",
        );
        ui.add_space(6.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The column headings of the (always empty) game list.
            let (header, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), egui::Sense::hover());
            for (column, offset) in [
                ("Server", 0.0),
                ("Map", 0.42),
                ("Players", 0.7),
                ("Ping", 0.85),
            ] {
                ui.painter().text(
                    egui::pos2(header.left() + header.width() * offset, header.center().y),
                    egui::Align2::LEFT_CENTER,
                    column,
                    egui::FontId::proportional(15.0),
                    theme::ACCENT,
                );
            }
            ui.separator();
            ui.add_space(18.0);
            ui.vertical_centered(|ui| {
                if searching {
                    ui.spinner();
                    ui.label("Searching your local network…");
                } else if state.lan_search_started.is_some() {
                    ui.label("No LAN games found.");
                } else {
                    theme::hint(ui, "Press Refresh to search your local network.");
                }
            });
            ui.add_space(18.0);
        });
        ui.add_space(10.0);
        theme::heading(ui, "Connect to address");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.lan_address)
                    .margin(egui::Margin::symmetric(6, 5))
                    .hint_text("192.168.0.10")
                    .char_limit(64)
                    .desired_width(260.0),
            );
            let address = state.lan_address.trim();
            if ui
                .add_enabled(!address.is_empty(), egui::Button::new("Connect"))
                .clicked()
            {
                actions.push(MenuAction::JoinLanServer {
                    address: address.to_owned(),
                });
            }
        });
    });
    theme::footer(ui, state, |ui, state| {
        if ui
            .add_enabled_ui(!searching, |ui| theme::button(ui, "Refresh"))
            .inner
            .clicked()
        {
            state.lan_search_started = Some(now);
        }
    });
}
