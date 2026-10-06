//! Third-person bot presentation. Asset-path and suffix provenance is in
//! `docs/FORMAT_SOURCES.md`, "Skirmish held weapons and combat animations".
//! Sequence labels are matched as data against project-authored intents.

use ohl_combat::WeaponId;
use ohl_game::hecs::Entity;
use ohl_gameplay::ViewModelAction;
use ohl_world::StudioModel;

use crate::components::{HeldWeapon, StudioAnim, StudioGait};
use crate::level::Level;

/// Model slots loaded once when the match starts, shared by all bots.
pub(crate) struct BotModels {
    pub(crate) player: Option<usize>,
    pub(crate) weapons: Vec<(WeaponId, usize)>,
}

/// Optional third-person weapon models; missing assets remain optional.
pub(crate) const fn held_model_path(weapon: WeaponId) -> &'static str {
    match weapon {
        WeaponId::Crowbar => "models/p_crowbar.mdl",
        WeaponId::Glock => "models/p_9mmhandgun.mdl",
        WeaponId::Python => "models/p_357.mdl",
        WeaponId::Mp5 => "models/p_9mmar.mdl",
        WeaponId::Shotgun => "models/p_shotgun.mdl",
        WeaponId::Crossbow => "models/p_crossbow.mdl",
        WeaponId::Rpg => "models/p_rpg.mdl",
        WeaponId::Gauss => "models/p_gauss.mdl",
        WeaponId::Egon => "models/p_egon.mdl",
        WeaponId::HornetGun => "models/p_hgun.mdl",
        WeaponId::HandGrenade => "models/p_grenade.mdl",
        WeaponId::Satchel => "models/p_satchel.mdl",
        WeaponId::Tripmine => "models/p_tripmine.mdl",
        WeaponId::Snark => "models/p_squeak.mdl",
    }
}

const fn weapon_suffix(weapon: WeaponId) -> &'static str {
    match weapon {
        WeaponId::Crowbar => "crowbar",
        WeaponId::Python => "python",
        WeaponId::Mp5 => "mp5",
        WeaponId::Shotgun => "shotgun",
        WeaponId::Crossbow => "bow",
        WeaponId::Rpg => "rpg",
        WeaponId::Gauss => "gauss",
        WeaponId::Egon => "egon",
        WeaponId::Tripmine => "trip",
        WeaponId::Snark => "squeak",
        WeaponId::Glock | WeaponId::HornetGun | WeaponId::HandGrenade | WeaponId::Satchel => {
            "onehanded"
        }
    }
}

fn crouched_sequence(label: &str) -> bool {
    label
        .split('_')
        .any(|token| token.starts_with("crouch") || token.starts_with("duck"))
}

fn intent_token(token: &str, intent: &str) -> bool {
    let token = token
        .strip_prefix("crouch")
        .or_else(|| token.strip_prefix("duck"))
        .unwrap_or(token)
        .trim_end_matches(|character: char| character.is_ascii_digit());
    token == intent
}

fn sequence_for(
    model: &StudioModel,
    weapon: Option<WeaponId>,
    intents: &[&str],
    crouched: bool,
) -> Option<usize> {
    let suffix = weapon.map(weapon_suffix);
    // Prefer the right stance, an armed variant, then intents in caller order.
    for stance in [crouched, !crouched] {
        for armed in [true, false] {
            for intent in intents {
                if let Some(index) = model.sequence_names.iter().position(|label| {
                    crouched_sequence(label) == stance
                        && (!armed
                            || suffix
                                .is_some_and(|suffix| label.rsplit('_').next() == Some(suffix)))
                        && (armed
                            || label
                                .rsplit('_')
                                .next()
                                .is_some_and(|last| intent_token(last, intent)))
                        && label.split('_').any(|token| intent_token(token, intent))
                }) {
                    return Some(index);
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy)]
pub(crate) struct BotMotion {
    pub(crate) speed: f32,
    pub(crate) crouched: bool,
    pub(crate) on_ground: bool,
}

fn locomotion(
    model: &StudioModel,
    weapon: Option<WeaponId>,
    motion: BotMotion,
) -> (Option<usize>, f32) {
    let intents: &[&str] = if motion.speed > 150.0 && !motion.crouched {
        &["run", "walk"]
    } else if motion.speed > 10.0 {
        &["walk", "run"]
    } else {
        &["idle", "aim"]
    };
    let sequence_for_motion = |intents: &[&str]| {
        sequence_for(model, None, intents, motion.crouched)
            .or_else(|| sequence_for(model, weapon, intents, motion.crouched))
    };
    let sequence = (!motion.on_ground)
        .then(|| sequence_for_motion(&["jump", "hop", "leap"]))
        .flatten()
        .or_else(|| sequence_for_motion(intents));
    let rate = sequence
        .filter(|_| motion.on_ground && motion.speed > 10.0)
        .and_then(|sequence| model.sequence_ground_speed(sequence))
        .map_or(1.0, |authored_speed| {
            (motion.speed / authored_speed).clamp(0.1, 4.0)
        });
    (sequence, rate)
}

pub(crate) struct BotAnimation {
    models: Vec<(WeaponId, usize)>,
    weapon: Option<WeaponId>,
    action: Option<(ViewModelAction, usize)>,
}

impl BotAnimation {
    pub(crate) fn new(models: &BotModels) -> Self {
        Self {
            models: models.weapons.clone(),
            weapon: None,
            action: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.weapon = None;
        self.action = None;
    }

    pub(crate) fn update(
        &mut self,
        level: &mut Level,
        entity: Entity,
        weapon: Option<WeaponId>,
        firing: bool,
        reloading: bool,
        motion: BotMotion,
    ) {
        if self.weapon != weapon {
            self.weapon = weapon;
            self.action = None;
        }
        if let Ok(mut held) = level.registry.world.get::<&mut HeldWeapon>(entity) {
            held.model = self
                .models
                .iter()
                .find_map(|(id, model)| (Some(*id) == weapon).then_some(*model));
        }
        let Ok(mut anim) = level.registry.world.get::<&mut StudioAnim>(entity) else {
            return;
        };
        let Some(model) = level.studio_models.get(anim.model) else {
            return;
        };
        let (locomotion, movement_rate) = locomotion(model, weapon, motion);
        // This must happen before an action's early return: a shot restarts
        // only the upper body, never the independently advancing stride.
        let layered = if let Ok(mut gait) = level.registry.world.get::<&mut StudioGait>(entity) {
            gait.play(locomotion, movement_rate);
            locomotion.is_some()
        } else {
            false
        };
        let requested = if firing {
            Some((ViewModelAction::Fire, &["shoot", "fire", "attack"][..]))
        } else if reloading {
            Some((ViewModelAction::Reload, &["reload"][..]))
        } else {
            None
        };
        if let Some((action, intents)) = requested
            && let Some(sequence) = sequence_for(model, weapon, intents, motion.crouched)
            && (action == ViewModelAction::Fire || self.action != Some((action, sequence)))
        {
            anim.sequence = sequence;
            anim.cycle = 0.0;
            anim.frame_rate = 1.0;
            self.action = Some((action, sequence));
        }
        if let Some((action, sequence)) = self.action {
            let playing = match action {
                ViewModelAction::Fire => model
                    .sequences
                    .get(sequence)
                    .is_some_and(|sequence| anim.cycle < sequence.duration()),
                ViewModelAction::Reload => reloading,
                _ => false,
            };
            if playing {
                return;
            }
            self.action = None;
        }
        let sequence = if layered || (motion.on_ground && motion.speed <= 10.0) {
            sequence_for(model, weapon, &["aim", "idle"], motion.crouched).or(locomotion)
        } else {
            locomotion
        };
        if let Some(sequence) =
            sequence.or_else(|| sequence_for(model, weapon, &["aim", "idle"], motion.crouched))
        {
            anim.play(sequence);
            anim.frame_rate = if layered { 1.0 } else { movement_rate };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BotAnimation, BotModels, BotMotion};
    use crate::components::{StudioAnim, StudioGait};
    use crate::test_support::{AI_MAP, deathmatch_room_bsp};
    use crate::{Game, MemoryAssets, SkirmishConfig};
    use ohl_combat::WeaponId;

    const PISTOL: Option<WeaponId> = Some(WeaponId::Glock);
    const RUNNING: BotMotion = BotMotion {
        speed: 200.0,
        crouched: false,
        on_ground: true,
    };

    fn motion(speed: f32, crouched: bool, on_ground: bool) -> BotMotion {
        BotMotion {
            speed,
            crouched,
            on_ground,
        }
    }

    const SEQUENCES: [&str; 9] = [
        "ohl_idle1",
        "ohl_walk",
        "ohl_run",
        "ohl_crouchwalk",
        "ohl_crouch_idle",
        "ohl_jump",
        "ohl_aim_onehanded",
        "ohl_shoot_onehanded",
        "ohl_reload_onehanded",
    ];

    fn game(sequences: &[&str]) -> (Game, BotAnimation, ohl_game::hecs::Entity) {
        let bytes = deathmatch_room_bsp(&[(-192.0, 0.0), (192.0, 0.0)], false, "");
        let mut assets = MemoryAssets::new();
        assets.insert(
            super::super::PLAYER_MODEL_PATH,
            ohl_formats::test_support::build_biped_mdl10_with_sequences(sequences),
        );
        let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).unwrap();
        game.start_skirmish(
            &assets,
            &SkirmishConfig {
                bots: 1,
                ..SkirmishConfig::default()
            },
        )
        .unwrap();
        let entity = game.skirmish_bots()[0];
        let animation = BotAnimation::new(&BotModels {
            player: None,
            weapons: Vec::new(),
        });
        (game, animation, entity)
    }

    fn advance(level: &mut crate::level::Level, entity: ohl_game::hecs::Entity, dt: f32) {
        level
            .registry
            .world
            .get::<&mut StudioAnim>(entity)
            .unwrap()
            .advance(dt);
        level
            .registry
            .world
            .get::<&mut StudioGait>(entity)
            .unwrap()
            .advance(dt);
    }

    fn pose(level: &crate::level::Level, entity: ohl_game::hecs::Entity) -> ohl_world::StudioPose {
        let anim = level.registry.world.get::<&StudioAnim>(entity).unwrap();
        let gait = level.registry.world.get::<&StudioGait>(entity).unwrap();
        anim.sample(&level.studio_models[anim.model], Some(&gait))
            .unwrap()
    }

    fn matrix_changed(left: &ohl_world::BoneMatrix, right: &ohl_world::BoneMatrix) -> bool {
        left.iter().zip(right).any(|(a, b)| (a - b).abs() > 1e-5)
    }

    #[test]
    fn repeated_shots_and_reload_do_not_freeze_or_restart_the_stride() {
        let (mut game, mut animation, entity) = game(&SEQUENCES);
        let (level, _) = game.level_and_systems_mut();
        animation.update(level, entity, PISTOL, false, false, RUNNING);
        advance(level, entity, 0.05);
        let before = pose(level, entity);
        animation.update(level, entity, PISTOL, true, false, RUNNING);
        assert!(!matrix_changed(
            &pose(level, entity).matrices[2],
            &before.matrices[2]
        ));
        assert_eq!(
            level
                .registry
                .world
                .get::<&StudioAnim>(entity)
                .unwrap()
                .sequence,
            7
        );

        advance(level, entity, 0.05);
        animation.update(level, entity, PISTOL, true, false, RUNNING);
        let after = pose(level, entity);
        assert!(
            matrix_changed(&after.matrices[2], &before.matrices[2]),
            "a real leg transform must advance"
        );
        assert!(
            level
                .registry
                .world
                .get::<&StudioAnim>(entity)
                .unwrap()
                .cycle
                .abs()
                < f32::EPSILON
        );
        let stride = level.registry.world.get::<&StudioGait>(entity).unwrap();
        assert_eq!(stride.sequence, Some(2));
        assert!((stride.cycle - 0.1).abs() < 1e-5);
        drop(stride);

        animation.update(level, entity, PISTOL, false, true, RUNNING);
        advance(level, entity, 0.05);
        animation.update(level, entity, PISTOL, false, true, RUNNING);
        let reload = level.registry.world.get::<&StudioAnim>(entity).unwrap();
        assert_eq!(reload.sequence, 8);
        assert!((reload.cycle - 0.05).abs() < 1e-5);
        drop(reload);
        assert!(
            (level
                .registry
                .world
                .get::<&StudioGait>(entity)
                .unwrap()
                .cycle
                - 0.15)
                .abs()
                < 1e-5
        );

        animation.update(level, entity, PISTOL, false, true, motion(0.0, false, true));
        assert_eq!(
            level
                .registry
                .world
                .get::<&StudioGait>(entity)
                .unwrap()
                .sequence,
            Some(0)
        );
        assert_eq!(
            level
                .registry
                .world
                .get::<&StudioAnim>(entity)
                .unwrap()
                .sequence,
            8
        );
    }

    #[test]
    fn locomotion_tracks_speed_crouching_and_airborne_state() {
        let (mut game, mut animation, entity) = game(&SEQUENCES);
        let (level, _) = game.level_and_systems_mut();
        for (speed, crouched, grounded, sequence, rate) in [
            (100.0, false, true, 1, 0.5),
            (200.0, false, true, 2, 1.0),
            (100.0, true, true, 3, 0.5),
            (100.0, true, false, 5, 1.0),
            (0.0, true, true, 4, 1.0),
        ] {
            animation.update(
                level,
                entity,
                PISTOL,
                false,
                false,
                motion(speed, crouched, grounded),
            );
            let gait = level.registry.world.get::<&StudioGait>(entity).unwrap();
            assert_eq!(gait.sequence, Some(sequence));
            assert!((gait.frame_rate - rate).abs() < 1e-5);
            assert!(gait.cycle.abs() < f32::EPSILON);
        }
    }

    #[test]
    fn missing_walk_uses_run_at_the_movement_speed() {
        let (mut game, mut animation, entity) = game(&["ohl_aim_onehanded", "ohl_run"]);
        let (level, _) = game.level_and_systems_mut();
        animation.update(
            level,
            entity,
            PISTOL,
            false,
            false,
            motion(60.0, false, true),
        );
        let gait = level.registry.world.get::<&StudioGait>(entity).unwrap();
        assert_eq!(gait.sequence, Some(1));
        assert!((gait.frame_rate - 0.3).abs() < 1e-5);
    }

    #[test]
    fn missing_locomotion_returns_to_aim_after_a_shot() {
        let (mut game, mut animation, entity) = game(&["ohl_aim_onehanded", "ohl_shoot_onehanded"]);
        let (level, _) = game.level_and_systems_mut();
        animation.update(level, entity, PISTOL, true, false, RUNNING);
        advance(level, entity, 1.0);
        animation.update(level, entity, PISTOL, false, false, RUNNING);
        assert_eq!(
            level
                .registry
                .world
                .get::<&StudioGait>(entity)
                .unwrap()
                .sequence,
            None
        );
        assert_eq!(
            level
                .registry
                .world
                .get::<&StudioAnim>(entity)
                .unwrap()
                .sequence,
            0
        );
    }

    #[test]
    fn real_bot_ticks_advance_the_stride_while_the_weapon_fires() {
        let (mut game, _, entity) = game(&SEQUENCES);
        for _ in 0..1000 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
            let (level, _) = game.level_and_systems_mut();
            let anim = level.registry.world.get::<&StudioAnim>(entity).unwrap();
            let gait = level.registry.world.get::<&StudioGait>(entity).unwrap();
            if anim.sequence == 7 && gait.sequence == Some(2) && gait.cycle > 0.0 {
                let drawn = anim
                    .sample(&level.studio_models[anim.model], Some(&gait))
                    .unwrap();
                let frozen = ohl_world::StudioPose::sample(
                    &level.studio_models[anim.model],
                    anim.sequence,
                    anim.cycle,
                )
                .unwrap();
                assert!(matrix_changed(&drawn.matrices[2], &frozen.matrices[2]));
                return;
            }
        }
        panic!("a moving bot must animate its legs while firing in a real match");
    }

    #[test]
    fn hitboxes_use_the_combined_leg_and_weapon_pose() {
        let (mut game, mut animation, entity) = game(&SEQUENCES);
        let (level, _) = game.level_and_systems_mut();
        animation.update(level, entity, PISTOL, false, false, RUNNING);
        advance(level, entity, 0.05);
        animation.update(level, entity, PISTOL, true, false, RUNNING);
        let mut index = ohl_combat::HitboxIndex::default();
        crate::combat::rebuild_hitbox_index(&mut index, level);
        let boxes = &index
            .entries()
            .iter()
            .find(|entry| entry.id == crate::ids::entity_id(entity))
            .unwrap()
            .boxes;
        let anim = level.registry.world.get::<&StudioAnim>(entity).unwrap();
        let model = &level.studio_models[anim.model];
        let combined = pose(level, entity);
        let action = ohl_world::StudioPose::sample(model, anim.sequence, anim.cycle).unwrap();
        for (hitbox, volume) in model.hitboxes.iter().zip(boxes) {
            let (min, max) = combined.hitbox_bounds(hitbox).unwrap();
            assert_eq!(volume.min, glam::Vec3::from_array(min));
            assert_eq!(volume.max, glam::Vec3::from_array(max));
        }
        assert_ne!(
            combined.hitbox_bounds(&model.hitboxes[0]),
            action.hitbox_bounds(&model.hitboxes[0])
        );
    }

    #[test]
    fn death_stops_the_stride_and_respawn_starts_a_fresh_one() {
        let (mut game, _, entity) = game(&SEQUENCES);
        let player = game.player_entity();
        crate::test_support::queue_engine_damage_from(&mut game, entity, player, 500.0);
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        let gait = game.registry().world.get::<&StudioGait>(entity).unwrap();
        assert_eq!(gait.sequence, None);
        assert!(gait.cycle.abs() < f32::EPSILON);
        drop(gait);
        for _ in 0..250 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
            if game
                .registry()
                .world
                .get::<&crate::skirmish::BotBody>(entity)
                .unwrap()
                .alive
            {
                let gait = game.registry().world.get::<&StudioGait>(entity).unwrap();
                assert_eq!(gait.sequence, Some(0));
                assert!(gait.cycle.abs() < f32::EPSILON);
                return;
            }
        }
        panic!("the bot must respawn with a fresh locomotion cursor");
    }
}
