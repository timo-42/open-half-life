//! The single-player panes: the hub, the new-game difficulty choice, the
//! chapter-by-chapter level select and the saved-game list.

use egui::RichText;

use super::theme;
use super::{Difficulty, MenuAction, MenuData, MenuPane, MenuState};

/// The single-player hub.
pub(super) fn draw_hub(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    theme::content(ui, |ui| {
        ui.add_space(8.0);
        if ui
            .add_enabled_ui(data.new_game_map.is_some(), |ui| {
                theme::hub_entry(ui, "New Game", "Start the campaign from the beginning")
            })
            .inner
            .clicked()
        {
            state.open(MenuPane::NewGame);
        }
        if ui
            .add_enabled_ui(!data.chapters.is_empty(), |ui| {
                theme::hub_entry(ui, "Select Level", "Start from any chapter or level")
            })
            .inner
            .clicked()
        {
            state.open(MenuPane::LevelSelect);
        }
        if let Some(map) = data.training_map
            && theme::hub_entry(ui, "Hazard Course", "Learn the controls in training").clicked()
        {
            actions.push(MenuAction::StartSinglePlayer {
                map,
                difficulty: state.difficulty,
            });
        }
        if theme::hub_entry(ui, "Load Game", "Continue from a saved game").clicked() {
            state.open(MenuPane::LoadGame);
        }
    });
    theme::footer(ui, state, |_, _| {});
}

/// What each difficulty button says about itself.
fn difficulty_blurb(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Easy => "Enemies are weaker and supplies go further.",
        Difficulty::Medium => "The balanced experience.",
        Difficulty::Hard => "Enemies hit hard and take more punishment.",
    }
}

/// The new-game pane: one button per difficulty, each starting the game.
pub(super) fn draw_new_game(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    theme::content(ui, |ui| {
        theme::hint(ui, "Choose a difficulty to begin.");
        ui.add_space(8.0);
        for difficulty in Difficulty::ALL {
            let response = theme::hub_entry(ui, difficulty.label(), difficulty_blurb(difficulty));
            if response.clicked()
                && let Some(map) = data.new_game_map
            {
                state.difficulty = difficulty;
                actions.push(MenuAction::StartSinglePlayer { map, difficulty });
            }
        }
    });
    theme::footer(ui, state, |_, _| {});
}

/// The difficulty toggle the level select shows.
fn difficulty_row(ui: &mut egui::Ui, state: &mut MenuState) {
    ui.horizontal(|ui| {
        ui.label("Difficulty");
        for difficulty in Difficulty::ALL {
            ui.selectable_value(&mut state.difficulty, difficulty, difficulty.label());
        }
    });
}

/// The level select: chapters on the left, the selected chapter's levels on
/// the right, then the difficulty and START LEVEL.
pub(super) fn draw_level_select(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    let Some(last_chapter) = data.chapters.len().checked_sub(1) else {
        theme::content(ui, |ui| theme::hint(ui, "No levels are available."));
        theme::footer(ui, state, |_, _| {});
        return;
    };
    state.selected_chapter = state.selected_chapter.min(last_chapter);
    let chapter = &data.chapters[state.selected_chapter];
    if let Some(last_level) = chapter.levels.len().checked_sub(1) {
        state.selected_level = state.selected_level.min(last_level);
    }
    let selected = chapter
        .levels
        .get(state.selected_level)
        .filter(|level| level.available);

    let list_height = (ui.available_height() - 110.0).max(80.0);
    ui.columns(2, |columns| {
        theme::heading(&mut columns[0], "Chapter");
        egui::ScrollArea::vertical()
            .id_salt("ohl_menu_chapters")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(&mut columns[0], |ui| {
                for (index, entry) in data.chapters.iter().enumerate() {
                    let text = if entry.levels.iter().any(|level| level.available) {
                        RichText::new(entry.title)
                    } else {
                        RichText::new(entry.title).color(theme::TEXT_DIM)
                    };
                    if theme::list_row(ui, state.selected_chapter == index, text, 26.0).clicked()
                        && state.selected_chapter != index
                    {
                        state.selected_chapter = index;
                        state.selected_level = 0;
                    }
                }
            });

        theme::heading(&mut columns[1], "Level");
        egui::ScrollArea::vertical()
            .id_salt("ohl_menu_levels")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(&mut columns[1], |ui| {
                if chapter.levels.is_empty() {
                    theme::hint(ui, "This chapter's levels are not listed yet.");
                }
                for (index, level) in chapter.levels.iter().enumerate() {
                    let response = ui
                        .add_enabled_ui(level.available, |ui| {
                            theme::list_row(ui, state.selected_level == index, level.map, 26.0)
                        })
                        .inner
                        .on_disabled_hover_text("This level is not in your game data.");
                    if response.clicked() {
                        state.selected_level = index;
                    }
                    if response.double_clicked() {
                        actions.push(MenuAction::StartSinglePlayer {
                            map: level.map,
                            difficulty: state.difficulty,
                        });
                    }
                }
            });
    });
    ui.add_space(6.0);
    difficulty_row(ui, state);
    theme::footer(ui, state, |ui, state| {
        if ui
            .add_enabled_ui(selected.is_some(), |ui| theme::button(ui, "Start Level"))
            .inner
            .clicked()
            && let Some(level) = selected
        {
            actions.push(MenuAction::StartSinglePlayer {
                map: level.map,
                difficulty: state.difficulty,
            });
        }
    });
}

/// The saved-game list.
pub(super) fn draw_load(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    data: &MenuData<'_>,
    actions: &mut Vec<MenuAction>,
) {
    if state
        .selected_save
        .is_some_and(|index| index >= data.saves.len())
    {
        state.selected_save = None;
    }
    theme::content(ui, |ui| {
        if data.saves.is_empty() {
            theme::hint(ui, "No saved games yet. Quick save with F6 while playing.");
        }
        for (index, save) in data.saves.iter().enumerate() {
            let selected = state.selected_save == Some(index);
            let mut text = egui::text::LayoutJob::default();
            text.append(
                &save.title,
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(17.0), theme::TITLE),
            );
            text.append(
                &format!("\n{}", save.detail),
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(13.0), theme::TEXT_DIM),
            );
            let response = theme::list_row(ui, selected, text, 50.0);
            if response.clicked() {
                state.selected_save = Some(index);
            }
            if response.double_clicked() {
                actions.push(MenuAction::LoadSave(save.slot.clone()));
            }
        }
    });
    theme::footer(ui, state, |ui, state| {
        let selected = state.selected_save.and_then(|index| data.saves.get(index));
        if ui
            .add_enabled_ui(selected.is_some(), |ui| theme::button(ui, "Load"))
            .inner
            .clicked()
            && let Some(save) = selected
        {
            actions.push(MenuAction::LoadSave(save.slot.clone()));
        }
    });
}
