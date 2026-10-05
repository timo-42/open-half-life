use super::{
    BotSkill, ChapterEntry, DisplayMode, LevelEntry, MenuAction, MenuData, MenuPane, MenuState,
    OptionsTab, Resolution, SKIRMISH_BOTS_RANGE, SKIRMISH_FRAG_LIMIT_RANGE,
    SKIRMISH_TIME_LIMIT_RANGE, SaveEntry, Screen, draw, skirmish_action,
};
use crate::bindings::{Action, Binding};
use winit::keyboard::KeyCode;

fn maps() -> Vec<String> {
    vec!["ohltest_a".to_owned(), "ohltest_b".to_owned()]
}

fn chapters() -> Vec<ChapterEntry> {
    vec![
        ChapterEntry {
            title: "Training",
            levels: vec![LevelEntry {
                map: "ohltest_t",
                available: true,
            }],
        },
        ChapterEntry {
            title: "One",
            levels: vec![
                LevelEntry {
                    map: "ohltest_1a",
                    available: true,
                },
                LevelEntry {
                    map: "ohltest_1b",
                    available: false,
                },
            ],
        },
        ChapterEntry {
            title: "Unlisted",
            levels: Vec::new(),
        },
    ]
}

/// Draws one headless frame of `state`'s current pane and returns the
/// actions it reported.
fn frame(state: &mut MenuState, data: &MenuData<'_>) -> Vec<MenuAction> {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 720.0),
        )),
        ..egui::RawInput::default()
    });
    let mut ui = crate::root_ui(&ctx);
    let actions = draw(&mut ui, state, data);
    let mut output = ctx.end_pass();
    output.textures_delta.clear();
    actions
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
    assert_eq!(state.selected_chapter, 0);
    assert_eq!(state.trail(), vec![MenuPane::Root]);
}

#[test]
fn back_retraces_the_way_the_player_came() {
    let mut state = MenuState::new();
    state.open(MenuPane::SinglePlayer);
    state.open(MenuPane::LevelSelect);
    assert_eq!(
        state.trail(),
        vec![MenuPane::SinglePlayer, MenuPane::LevelSelect]
    );
    assert!(state.back());
    assert_eq!(state.pane, MenuPane::SinglePlayer);
    assert!(state.back());
    assert_eq!(state.pane, MenuPane::Root);
    assert!(!state.back(), "the root has nowhere to go back to");

    // The pause menu's LOAD GAME goes straight there, and back returns to
    // the root rather than to the single-player hub.
    state.open(MenuPane::LoadGame);
    assert!(state.back());
    assert_eq!(state.pane, MenuPane::Root);
}

#[test]
fn reset_returns_to_the_root_and_forgets_the_way_back() {
    let mut state = MenuState::new();
    state.open(MenuPane::Multiplayer);
    state.open(MenuPane::Skirmish);
    state.reset();
    assert_eq!(state.pane, MenuPane::Root);
    assert!(!state.back());
}

#[test]
fn a_key_capture_is_cancelled_by_back_before_the_pane_is_left() {
    let mut state = MenuState::new();
    state.open(MenuPane::Options);
    state.capturing = Some(Action::Jump);
    assert!(state.back());
    assert_eq!(state.pane, MenuPane::Options, "only the capture ended");
    assert_eq!(state.capturing_binding(), None);
    assert!(state.back());
    assert_eq!(state.pane, MenuPane::Root);
}

#[test]
fn a_captured_key_is_bound_and_reported_on_the_next_frame() {
    let mut state = MenuState::new();
    assert!(
        !state.capture_binding(Some(Binding::Key(KeyCode::KeyQ))),
        "nothing is being captured"
    );
    state.open(MenuPane::Options);
    state.capturing = Some(Action::Use);
    assert!(!state.capture_binding(None), "an unbindable key is ignored");
    assert_eq!(state.capturing_binding(), Some(Action::Use));
    assert!(state.capture_binding(Some(Binding::Key(KeyCode::KeyQ))));
    assert_eq!(state.capturing_binding(), None);
    assert_eq!(
        state.options.controls.bindings.get(Action::Use),
        Some(Binding::Key(KeyCode::KeyQ))
    );
    let actions = frame(&mut state, &MenuData::default());
    assert!(actions.contains(&MenuAction::OptionsChanged));
}

#[test]
fn opening_the_options_starts_the_display_draft_from_the_applied_settings() {
    let mut state = MenuState::new();
    state.options.video.display.mode = DisplayMode::Borderless;
    state.open(MenuPane::Options);
    assert_eq!(state.display_draft.mode, DisplayMode::Borderless);
}

#[test]
fn every_pane_draws_in_the_main_and_the_pause_menu() {
    let chapters = chapters();
    let maps = maps();
    let saves = vec![SaveEntry {
        slot: "quicksave".to_owned(),
        title: "Quick save".to_owned(),
        detail: "ohltest_1a".to_owned(),
    }];
    let resolutions = [Resolution::new(1920, 1080), Resolution::new(1280, 720)];
    for in_game in [false, true] {
        let data = MenuData {
            in_game,
            can_save: in_game,
            new_game_map: Some("ohltest_1a"),
            training_map: Some("ohltest_t"),
            chapters: &chapters,
            skirmish_maps: &maps,
            saves: &saves,
            resolutions: &resolutions,
            audio_output: false,
        };
        for pane in [
            MenuPane::Root,
            MenuPane::SinglePlayer,
            MenuPane::NewGame,
            MenuPane::LevelSelect,
            MenuPane::LoadGame,
            MenuPane::Multiplayer,
            MenuPane::Skirmish,
            MenuPane::CreateServer,
            MenuPane::LanBrowser,
        ] {
            let mut state = MenuState::new();
            state.open(pane);
            assert!(frame(&mut state, &data).is_empty(), "{pane:?}");
            assert_eq!(state.pane, pane);
        }
        for tab in OptionsTab::ALL {
            let mut state = MenuState::new();
            state.open(MenuPane::Options);
            state.options_tab = tab;
            assert!(frame(&mut state, &data).is_empty(), "{tab:?}");
        }
    }
}

#[test]
fn the_level_select_clamps_its_selection_to_the_host_list() {
    let chapters = chapters();
    let data = MenuData {
        chapters: &chapters,
        ..MenuData::default()
    };
    let mut state = MenuState::new();
    state.open(MenuPane::LevelSelect);
    state.selected_chapter = 99;
    state.selected_level = 99;
    frame(&mut state, &data);
    assert_eq!(state.selected_chapter, 2);

    state.selected_chapter = 1;
    frame(&mut state, &data);
    assert_eq!(state.selected_level, 1);
}

#[test]
fn a_notice_is_kept_until_it_has_been_shown() {
    let mut state = MenuState::new();
    state.notify("Game saved.");
    assert_eq!(state.notice(), Some("Game saved."));
    frame(&mut state, &MenuData::default());
    assert_eq!(state.notice(), Some("Game saved."), "still within its time");
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

#[test]
fn drawing_the_skirmish_pane_clamps_the_selected_map_to_the_list() {
    let maps = maps();
    let data = MenuData {
        skirmish_maps: &maps,
        ..MenuData::default()
    };
    let mut state = MenuState {
        selected_skirmish_map: 99,
        ..MenuState::default()
    };
    state.open(MenuPane::Skirmish);
    let actions = frame(&mut state, &data);
    assert!(actions.is_empty());
    assert_eq!(state.selected_skirmish_map, 1);
    assert_eq!(state.pane, MenuPane::Skirmish);
}

#[test]
fn drawing_the_skirmish_pane_without_maps_offers_nothing_to_start() {
    let mut state = MenuState::default();
    state.open(MenuPane::Skirmish);
    let actions = frame(&mut state, &MenuData::default());
    assert!(actions.is_empty());
    assert_eq!(state.selected_skirmish_map, 0);
}
