use ohl_engine::test_support::{script_game, script_room_entities};
use ohl_engine::{Input, TICK_SECONDS};

#[test]
fn an_explicit_viewpoint_keeps_its_eye_across_an_idle_step() {
    for duck in [false, true] {
        let mut game = script_game(&script_room_entities([0.0, 0.0, 36.0], ""));
        assert!(game.has_collision());
        assert!(game.player_health() > 0.0);
        let standing_offset = game.eye_position()[2] - game.player_origin()[2];
        let idle = Input {
            duck,
            ..Input::default()
        };
        if duck {
            game.tick(TICK_SECONDS, &idle);
            let crouched_offset = game.eye_position()[2] - game.player_origin()[2];
            assert!(crouched_offset > 0.0 && crouched_offset < standing_offset);
        }

        let requested_eye = game.eye_position();
        game.set_viewpoint(requested_eye, 13.0, 47.0);
        assert_eq!(
            game.eye_position().map(f32::to_bits),
            requested_eye.map(f32::to_bits)
        );
        let center = game.player_origin();
        assert!(center.into_iter().all(f32::is_finite));

        game.tick(TICK_SECONDS, &idle);
        assert!(game.has_collision());
        assert!(game.player_health() > 0.0);
        assert_eq!(
            game.player_origin().map(f32::to_bits),
            center.map(f32::to_bits),
            "the idle noclip controller must not move"
        );
        let (yaw, pitch) = game.player_view_angles();
        assert_eq!(yaw.to_bits(), 47.0_f32.to_bits());
        assert_eq!(pitch.to_bits(), 13.0_f32.to_bits());
        assert_eq!(
            game.eye_position().map(f32::to_bits),
            requested_eye.map(f32::to_bits),
            "an explicit eye must remain fixed across an idle step"
        );
    }
}
