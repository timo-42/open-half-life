//! M7.12: `trigger_camera` view-override sequences, over a synthetic room.
//!
//! Every fixture here is project-authored
//! (`ohl_engine::test_support`/`entity_block`); no bytes come from any game
//! installation. Every keyvalue and spawnflag the entity blocks below use is
//! a published one recorded in `docs/FORMAT_SOURCES.md`, "Camera sequences".

// Exact float comparison is the point of these eye-position assertions: the
// override either lands exactly on the resolved position or it does not.
#![allow(clippy::float_cmp)]

use ohl_engine::test_support::{entity_block, script_game, script_room_entities};
use ohl_engine::{Game, GameEvent, Input, TICK_SECONDS};

/// The number of `TICK_SECONDS` ticks `seconds` covers, rounded up, plus a
/// small slack margin for the fixed-point drift of summing many small
/// steps. Test-only arithmetic over small, known-finite, non-negative
/// durations, so the `f32 -> usize` narrowing is never lossy in a way that
/// matters here.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ticks_for(seconds: f32) -> usize {
    (seconds / TICK_SECONDS).ceil() as usize + 20
}

/// A `trigger_camera` at `origin`, `wait` seconds long, firing `target` on
/// completion, with `spawnflags` (this test only ever passes the documented
/// "Freeze Player" bit, `4`, or `0`).
fn camera_block(origin: [f32; 3], wait: f32, spawnflags: u32) -> String {
    entity_block(
        "trigger_camera",
        origin,
        0.0,
        &[
            ("targetname", "cam1"),
            ("target", "after_cam"),
            ("wait", &wait.to_string()),
            ("spawnflags", &spawnflags.to_string()),
        ],
    )
}

/// A `trigger_auto` that fires `target` as soon as the map has loaded.
fn trigger_auto(target: &str) -> String {
    entity_block("trigger_auto", [0.0, 0.0, 0.0], 0.0, &[("target", target)])
}

/// A `trigger_changelevel` the camera's completion `target` names; firing it
/// is visible to the host as a `GameEvent::LevelChange`.
fn exit_trigger(name: &str) -> String {
    entity_block(
        "trigger_changelevel",
        [0.0, 0.0, 0.0],
        0.0,
        &[
            ("targetname", name),
            ("map", "ohlelsewhere"),
            ("landmark", "ohl_landmark"),
        ],
    )
}

/// Steps `game` once with `input`, returning whether a `GameEvent::LevelChange`
/// fired this tick.
fn tick_fired_level_change(game: &mut Game, input: &Input) -> bool {
    game.tick(TICK_SECONDS, input)
        .iter()
        .any(|event| matches!(event, GameEvent::LevelChange { .. }))
}

const PLAYER_ORIGIN: [f32; 3] = [0.0, 0.0, 32.0];
const CAMERA_ORIGIN: [f32; 3] = [96.0, -96.0, 64.0];

/// `trigger_auto` firing a `trigger_camera`: the eye position must move to
/// the camera's own placed origin for the `wait` duration and return
/// afterward, the completion `target` must fire exactly once, and
/// (`Freeze Player` is set) forward input during the hold must not move the
/// player at all, only resuming once the sequence has finished.
#[test]
fn a_triggered_camera_overrides_the_view_then_restores_it_and_fires_once() {
    const HOLD_SECONDS: f32 = 0.3;
    const SPAWNFLAG_FREEZE_PLAYER: u32 = 4;

    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}",
            trigger_auto("cam1"),
            camera_block(CAMERA_ORIGIN, HOLD_SECONDS, SPAWNFLAG_FREEZE_PLAYER),
            exit_trigger("after_cam"),
        ),
    );
    let mut game = script_game(&entities);
    // Noclip, so a plain `forward` press reliably displaces the player
    // within a handful of ticks: this fixture's collision hulls are built
    // for the AI-navigation tests that share it, and a walking player can
    // take far longer than this test's own budget to develop any visible
    // speed against them. `set_viewpoint` is the same public seam
    // `ohl-app`'s own headless capture path already uses for exactly this
    // "stand somewhere and move freely" need.
    game.set_viewpoint(PLAYER_ORIGIN, 0.0, 0.0);
    let player = game.player_entity();
    let forward_input = Input {
        forward: 1,
        ..Input::default()
    };
    let idle_input = Input::default();

    let initial_eye = game.eye_position();
    assert!(!game.camera_sequence_active());

    // Tick 1: the `trigger_auto` fires and the map-logic simulation
    // activates the camera within the very same tick (phase 12), so this
    // tick's own phase 12.5 override already shows the new view — see
    // `crate::camera`'s module doc for why the *input* freeze instead lags
    // one tick behind this.
    assert!(!tick_fired_level_change(&mut game, &idle_input));
    assert!(
        game.camera_sequence_active(),
        "the trigger_auto must have activated the camera on the very first tick"
    );
    let overridden_eye = game.eye_position();
    for axis in 0..3 {
        assert!(
            (overridden_eye[axis] - CAMERA_ORIGIN[axis]).abs() < 1e-2,
            "the eye must sit at the trigger_camera's own placed origin while active: \
             got {overridden_eye:?}, expected {CAMERA_ORIGIN:?}"
        );
    }
    assert_ne!(
        overridden_eye, initial_eye,
        "the view must actually have moved away from the player's own eye"
    );

    let actor_origin_at_activation = ohl_engine::test_support::actor_origin(&game, player);

    // Drive forward input for the rest of the hold (bounded by a small
    // slack margin over the fixed-point ticks a `0.01`-second-at-a-time
    // hold sums to): "Freeze Player" must ignore every one of these
    // presses, and the eye must stay pinned to the camera the whole time.
    let hold_ticks = ticks_for(HOLD_SECONDS);
    let mut completed = false;
    for _ in 0..hold_ticks {
        game.tick(TICK_SECONDS, &forward_input);
        if !game.camera_sequence_active() {
            completed = true;
            break;
        }
        let eye = game.eye_position();
        for axis in 0..3 {
            assert!(
                (eye[axis] - CAMERA_ORIGIN[axis]).abs() < 1e-2,
                "the eye must not drift from the camera origin while the sequence holds"
            );
        }
    }
    assert!(
        completed,
        "the sequence must have completed within the hold plus slack"
    );

    // The sequence has just finished (this exact tick): the view must
    // already have reverted, and the player must not have moved at all
    // during the whole hold, despite every one of those ticks pressing
    // forward.
    let actor_origin_after_hold = ohl_engine::test_support::actor_origin(&game, player);
    assert_eq!(
        actor_origin_at_activation, actor_origin_after_hold,
        "Freeze Player must have ignored every forward press during the hold"
    );
    let restored_eye = game.eye_position();
    assert!(
        (restored_eye[0] - CAMERA_ORIGIN[0]).abs() > 1.0
            || (restored_eye[1] - CAMERA_ORIGIN[1]).abs() > 1.0,
        "the eye must have moved off the camera's own origin once the sequence finished"
    );

    // The completion `target` (`trigger_changelevel "after_cam"`) is only
    // enqueued the tick the sequence ends, and — like every other scheduled
    // fire this crate's `Simulation` queues — is drained by the *next*
    // tick's `advance_queue`, so the `GameEvent::LevelChange` itself lags
    // one idle tick behind `camera_sequence_active()` going false.
    assert!(
        tick_fired_level_change(&mut game, &Input::default()),
        "the completion target must fire exactly once, one tick after completion"
    );
    assert!(!tick_fired_level_change(&mut game, &Input::default()));

    // Ordinary input moves the player again now that the sequence has
    // released it.
    for _ in 0..10 {
        game.tick(TICK_SECONDS, &forward_input);
    }
    let actor_origin_after_release = ohl_engine::test_support::actor_origin(&game, player);
    assert_ne!(
        actor_origin_after_release, actor_origin_after_hold,
        "input must move the player again once the sequence has released it"
    );
}

/// The same map, but the camera's spawnflags carry neither "Freeze Player"
/// nor any other bit (`0`): forward input during the hold must still move
/// the player, since only the documented flag gates movement.
#[test]
fn without_freeze_player_input_still_moves_the_player_during_the_hold() {
    const HOLD_SECONDS: f32 = 0.5;

    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}",
            trigger_auto("cam1"),
            camera_block(CAMERA_ORIGIN, HOLD_SECONDS, 0),
            exit_trigger("after_cam"),
        ),
    );
    let mut game = script_game(&entities);
    game.set_viewpoint(PLAYER_ORIGIN, 0.0, 0.0);
    let player = game.player_entity();
    let forward_input = Input {
        forward: 1,
        ..Input::default()
    };

    game.tick(TICK_SECONDS, &Input::default());
    assert!(game.camera_sequence_active());
    let origin_at_activation = ohl_engine::test_support::actor_origin(&game, player);

    for _ in 0..30 {
        game.tick(TICK_SECONDS, &forward_input);
    }
    let origin_after_pressing_forward = ohl_engine::test_support::actor_origin(&game, player);
    assert_ne!(
        origin_at_activation, origin_after_pressing_forward,
        "without Freeze Player, movement input must still reach the player"
    );
}

/// A second `trigger_auto`, delayed to land mid-hold, re-triggers the same
/// already-active camera: the documented "An active camera can be stopped
/// by triggering it again" behaviour, exercised over the same
/// `Simulation::activate` path a map's own second trigger would use.
/// Stopping this way must not fire the completion `target` — only a
/// natural completion (the other two tests here) does.
#[test]
fn a_second_trigger_mid_hold_stops_the_camera_without_firing_the_completion_target() {
    const RETRIGGER_DELAY: f32 = 0.2;

    let mut second_auto = trigger_auto("cam1");
    // `entity_block` always writes its own `"angle" "0"` line; a distinct
    // `delay` keyvalue on this second block is enough to make it fire later
    // than the first, unmodified `trigger_auto` (`delay` defaults to `0`).
    second_auto = second_auto.replacen("}\n", &format!("\"delay\" \"{RETRIGGER_DELAY}\"\n}}\n"), 1);
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}{}",
            trigger_auto("cam1"),
            second_auto,
            camera_block(CAMERA_ORIGIN, 1000.0, 0),
            exit_trigger("after_cam"),
        ),
    );
    let mut game = script_game(&entities);

    let mut fired = 0usize;
    let ticks = ticks_for(RETRIGGER_DELAY);
    let mut stopped = false;
    for tick_index in 0..ticks {
        if tick_fired_level_change(&mut game, &Input::default()) {
            fired += 1;
        }
        if tick_index == 0 {
            assert!(
                game.camera_sequence_active(),
                "the first trigger_auto must activate the camera immediately"
            );
        }
        if !game.camera_sequence_active() {
            stopped = true;
            break;
        }
    }
    assert!(
        stopped,
        "the second, delayed trigger_auto must have stopped the active camera"
    );
    assert_eq!(
        fired, 0,
        "a manual stop must not fire the completion target"
    );
}
