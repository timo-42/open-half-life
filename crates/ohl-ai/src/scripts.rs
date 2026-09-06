//! The `scripted_sequence`/`aiscripted_sequence` state machine.
//!
//! `ohl_game::scripts` owns the *keyvalues* — which key is which, which
//! spawnflag bit means what — and `ohl-engine` owns the *entity work*:
//! choosing which monster a script possesses, driving that monster's route
//! through the existing [`crate::Navigator`]/`NavBridge` seam, resolving a
//! sequence *name* against the model the monster actually loaded, and
//! firing `target` through the map-logic simulation. What is left, and what
//! lives here, is the pure state machine in between: given "the monster is
//! at the mark", "the action animation finished" and "something disturbed
//! the monster", decide what the script wants done next.
//!
//! Keeping it pure is what makes every published rule — each `m_fMoveTo`
//! mode, the `No Interruptions` flag, `Repeatable`, `m_flRepeat` — testable
//! without a world, a model or a navigator.
//!
//! # Suspending the normal brain
//!
//! A monster running a script carries [`ScriptHold`]. [`crate::AiWorld`]
//! still senses and remembers for it — that is what lets an interruptible
//! script notice damage — but selects no state and runs no schedule while
//! the marker is present, and follows whatever route the script set. The
//! marker is removed when the script releases the monster, and its normal
//! brain picks up on the next tick.
//!
//! # Clean room
//!
//! The published behaviour this implements is cited in
//! `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk monsters". The
//! phase set, the `ScriptAction` vocabulary and the runner's tick rules are
//! this project's own design; no SDK or engine source was consulted.

use ohl_game::scripts::{MoveTo, ScriptDef};

/// Marks a monster whose AI a script has taken over.
///
/// See the module docs: the presence of this component is the whole
/// suspension contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptHold;

/// Where one script stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptPhase {
    /// Not running: waiting for its first (or next) activation. The idle
    /// animation, when the map named one, loops here.
    #[default]
    Dormant,
    /// Getting the monster to the mark, however `m_fMoveTo` says to.
    Moving,
    /// Playing the action animation.
    Playing,
    /// A repeatable script counting `m_flRepeat` down before running again.
    Repeating,
    /// Finished for good; a non-repeatable script never leaves this phase.
    Done,
}

impl ScriptPhase {
    /// Whether the script currently possesses its monster.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Moving | Self::Playing)
    }

    /// A stable byte tag, for determinism hashes and save files.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Dormant => 0,
            Self::Moving => 1,
            Self::Playing => 2,
            Self::Repeating => 3,
            Self::Done => 4,
        }
    }
}

/// What the script wants done with its monster this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptAction {
    /// Nothing at all: the script does not own the monster.
    #[default]
    None,
    /// Play the idle animation, if the map named one, and stand still.
    Idle,
    /// Route to the mark and follow it; `run` picks the running speed.
    Approach {
        /// `true` for `m_fMoveTo` "Run", `false` for "Walk".
        run: bool,
    },
    /// Put the monster on the mark, at the script's own facing, at once.
    Teleport,
    /// Stay put but turn to the script's own facing.
    Face,
    /// Play the action animation.
    Play,
}

/// What the engine observed about the monster this tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScriptSense {
    /// Seconds since the last update.
    pub dt: f32,
    /// The monster has reached the mark (or never had to move).
    pub at_mark: bool,
    /// The monster's yaw matches the script's.
    pub facing_mark: bool,
    /// The action animation has played through.
    pub sequence_finished: bool,
    /// Something happened that interrupts an interruptible script: damage,
    /// or the monster acquiring an enemy.
    pub disturbed: bool,
}

/// What one [`ScriptRunner::update`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptStep {
    /// What to do with the monster.
    pub action: ScriptAction,
    /// The script took possession of its monster this tick.
    pub started: bool,
    /// The script let go of its monster this tick, for any reason.
    pub released: bool,
    /// The action animation completed this tick, so `target` fires.
    pub completed: bool,
    /// The script was abandoned because something disturbed the monster.
    pub interrupted: bool,
}

/// One `scripted_sequence`/`aiscripted_sequence`, running.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptRunner {
    def: ScriptDef,
    phase: ScriptPhase,
    timer: f32,
    completions: u32,
    warped: bool,
}

impl ScriptRunner {
    /// A dormant runner for `def`.
    #[must_use]
    pub fn new(def: ScriptDef) -> Self {
        Self {
            def,
            phase: ScriptPhase::Dormant,
            timer: 0.0,
            completions: 0,
            warped: false,
        }
    }

    /// The keyvalues this script was built from.
    #[must_use]
    pub fn def(&self) -> &ScriptDef {
        &self.def
    }

    /// Where the script stands.
    #[must_use]
    pub const fn phase(&self) -> ScriptPhase {
        self.phase
    }

    /// Whether the script currently possesses its monster.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.phase.is_active()
    }

    /// How many times the action animation has completed.
    #[must_use]
    pub const fn completions(&self) -> u32 {
        self.completions
    }

    /// The seconds left of a `m_flRepeat` wait; zero outside
    /// [`ScriptPhase::Repeating`].
    #[must_use]
    pub const fn timer(&self) -> f32 {
        self.timer
    }

    /// Handles one activation from the map logic, reporting whether it
    /// started the script.
    ///
    /// A script that is already running, counting down `m_flRepeat`, or
    /// finished for good ignores the activation: the published `Repeatable`
    /// flag is what allows a second run, and it is honoured when the first
    /// one *ends*, not when a stray trigger arrives mid-sequence.
    pub fn trigger(&mut self) -> bool {
        if self.phase != ScriptPhase::Dormant {
            return false;
        }
        self.begin();
        true
    }

    /// Enters [`ScriptPhase::Moving`] from the start.
    fn begin(&mut self) {
        self.phase = ScriptPhase::Moving;
        self.timer = 0.0;
        self.warped = false;
    }

    /// Abandons a running script without completing it.
    ///
    /// The monster is released and the script becomes dormant again, so a
    /// later trigger replays it from the beginning — including its idle
    /// animation, which is the documented behaviour of an interrupted
    /// script that is triggered again.
    fn abandon(&mut self) -> ScriptStep {
        self.phase = ScriptPhase::Dormant;
        self.timer = 0.0;
        self.warped = false;
        ScriptStep {
            action: ScriptAction::Idle,
            released: true,
            interrupted: true,
            ..ScriptStep::default()
        }
    }

    /// Advances the script by one tick.
    #[must_use]
    pub fn update(&mut self, sense: &ScriptSense) -> ScriptStep {
        let dt = if sense.dt.is_finite() && sense.dt > 0.0 {
            sense.dt
        } else {
            0.0
        };
        if sense.disturbed && self.phase.is_active() && !self.def.no_interruptions() {
            return self.abandon();
        }
        match self.phase {
            ScriptPhase::Dormant => ScriptStep {
                action: ScriptAction::Idle,
                ..ScriptStep::default()
            },
            ScriptPhase::Done => ScriptStep::default(),
            ScriptPhase::Repeating => {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.begin();
                    return ScriptStep {
                        action: self.approach_action(),
                        started: true,
                        ..ScriptStep::default()
                    };
                }
                ScriptStep {
                    action: ScriptAction::Idle,
                    ..ScriptStep::default()
                }
            }
            ScriptPhase::Moving => self.tick_moving(sense),
            ScriptPhase::Playing => self.tick_playing(sense),
        }
    }

    /// The action [`ScriptPhase::Moving`] asks for, by `m_fMoveTo` mode.
    const fn approach_action(&self) -> ScriptAction {
        match self.def.move_to {
            MoveTo::No => ScriptAction::Idle,
            MoveTo::Walk => ScriptAction::Approach { run: false },
            MoveTo::Run => ScriptAction::Approach { run: true },
            MoveTo::Instantaneous => ScriptAction::Teleport,
            MoveTo::TurnToFace => ScriptAction::Face,
        }
    }

    fn tick_moving(&mut self, sense: &ScriptSense) -> ScriptStep {
        let arrived = match self.def.move_to {
            // "The monster will not move or turn": nothing to wait for.
            MoveTo::No => true,
            // Walking and running wait for the engine to report arrival;
            // `TODO(black-box)`: no page says what a script does when its
            // monster cannot reach the mark at all, so it simply keeps
            // waiting rather than inventing a give-up rule.
            MoveTo::Walk | MoveTo::Run => sense.at_mark,
            // One tick of `Teleport` is enough: the engine places the
            // monster, and the next update finds it there.
            MoveTo::Instantaneous => {
                let done = self.warped;
                self.warped = true;
                done
            }
            MoveTo::TurnToFace => sense.facing_mark,
        };
        if !arrived {
            return ScriptStep {
                action: self.approach_action(),
                ..ScriptStep::default()
            };
        }
        self.phase = ScriptPhase::Playing;
        ScriptStep {
            action: ScriptAction::Play,
            ..ScriptStep::default()
        }
    }

    fn tick_playing(&mut self, sense: &ScriptSense) -> ScriptStep {
        if !sense.sequence_finished {
            return ScriptStep {
                action: ScriptAction::Play,
                ..ScriptStep::default()
            };
        }
        self.completions = self.completions.saturating_add(1);
        // A repeatable script with a repeat rate plays again on its own; a
        // repeatable script without one waits for the next trigger; a
        // script that is neither is finished for good.
        let (phase, timer) = if self.def.repeatable() && self.def.repeat > 0.0 {
            (ScriptPhase::Repeating, self.def.repeat)
        } else if self.def.repeatable() {
            (ScriptPhase::Dormant, 0.0)
        } else {
            (ScriptPhase::Done, 0.0)
        };
        self.phase = phase;
        self.timer = timer;
        self.warped = false;
        ScriptStep {
            action: if phase == ScriptPhase::Done {
                ScriptAction::None
            } else {
                ScriptAction::Idle
            },
            released: true,
            completed: true,
            ..ScriptStep::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScriptAction, ScriptPhase, ScriptRunner, ScriptSense};
    use ohl_game::keyvalues::{EntityDef, Limits, parse_entity};
    use ohl_game::scripts::ScriptDef;

    fn script(pairs: &[(&str, &str)]) -> ScriptRunner {
        let mut all: Vec<(&str, &str)> = vec![("classname", "scripted_sequence")];
        all.extend_from_slice(pairs);
        let raw: ohl_formats::bsp30::Entity = all
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let def: EntityDef = parse_entity(&raw, &Limits::default());
        ScriptRunner::new(ScriptDef::from_def(&def).expect("a scripted_sequence"))
    }

    fn sense() -> ScriptSense {
        ScriptSense {
            dt: 0.1,
            ..ScriptSense::default()
        }
    }

    #[test]
    fn a_dormant_script_asks_only_for_its_idle_animation() {
        let mut runner = script(&[("m_iszIdle", "ohl_idle")]);
        let step = runner.update(&sense());
        assert_eq!(step.action, ScriptAction::Idle);
        assert!(!step.started && !step.completed);
        assert_eq!(runner.phase(), ScriptPhase::Dormant);
    }

    #[test]
    fn move_to_no_plays_the_action_without_moving() {
        let mut runner = script(&[("m_fMoveTo", "0")]);
        assert!(runner.trigger());
        let step = runner.update(&sense());
        assert_eq!(step.action, ScriptAction::Play);
        assert_eq!(runner.phase(), ScriptPhase::Playing);
    }

    #[test]
    fn move_to_walk_and_run_route_to_the_mark_first() {
        for (raw, run) in [("1", false), ("2", true)] {
            let mut runner = script(&[("m_fMoveTo", raw)]);
            assert!(runner.trigger());
            assert_eq!(
                runner.update(&sense()).action,
                ScriptAction::Approach { run }
            );
            assert_eq!(runner.phase(), ScriptPhase::Moving);
            let arrived = ScriptSense {
                at_mark: true,
                ..sense()
            };
            assert_eq!(runner.update(&arrived).action, ScriptAction::Play);
            assert_eq!(runner.phase(), ScriptPhase::Playing);
        }
    }

    #[test]
    fn move_to_instantaneous_warps_once_then_plays() {
        let mut runner = script(&[("m_fMoveTo", "4")]);
        assert!(runner.trigger());
        assert_eq!(runner.update(&sense()).action, ScriptAction::Teleport);
        assert_eq!(runner.update(&sense()).action, ScriptAction::Play);
    }

    #[test]
    fn move_to_turn_to_face_waits_for_the_turn_and_never_routes() {
        let mut runner = script(&[("m_fMoveTo", "5")]);
        assert!(runner.trigger());
        assert_eq!(runner.update(&sense()).action, ScriptAction::Face);
        assert_eq!(runner.update(&sense()).action, ScriptAction::Face);
        let faced = ScriptSense {
            facing_mark: true,
            ..sense()
        };
        assert_eq!(runner.update(&faced).action, ScriptAction::Play);
    }

    #[test]
    fn an_unfinished_action_animation_keeps_playing() {
        let mut runner = script(&[("m_fMoveTo", "0")]);
        assert!(runner.trigger());
        let _ = runner.update(&sense());
        for _ in 0..8 {
            let step = runner.update(&sense());
            assert_eq!(step.action, ScriptAction::Play);
            assert!(!step.completed);
        }
    }

    #[test]
    fn completing_the_action_releases_the_monster_and_fires_once() {
        let mut runner = script(&[("m_fMoveTo", "0")]);
        assert!(runner.trigger());
        let _ = runner.update(&sense());
        let finished = ScriptSense {
            sequence_finished: true,
            ..sense()
        };
        let step = runner.update(&finished);
        assert!(step.completed && step.released);
        assert_eq!(runner.phase(), ScriptPhase::Done);
        assert_eq!(runner.completions(), 1);
        // A finished, non-repeatable script stays finished and never fires
        // its target a second time, however long it is ticked.
        for _ in 0..16 {
            let step = runner.update(&finished);
            assert!(!step.completed);
            assert_eq!(step.action, ScriptAction::None);
        }
        assert!(!runner.trigger(), "a spent script cannot be re-triggered");
        assert_eq!(runner.completions(), 1);
    }

    #[test]
    fn a_repeatable_script_without_a_repeat_rate_waits_for_a_new_trigger() {
        let mut runner = script(&[("m_fMoveTo", "0"), ("spawnflags", "4")]);
        assert!(runner.trigger());
        let _ = runner.update(&sense());
        let finished = ScriptSense {
            sequence_finished: true,
            ..sense()
        };
        assert!(runner.update(&finished).completed);
        assert_eq!(runner.phase(), ScriptPhase::Dormant);
        assert!(runner.trigger(), "Repeatable allows a second run");
        assert_eq!(runner.update(&sense()).action, ScriptAction::Play);
        assert!(runner.update(&finished).completed);
        assert_eq!(runner.completions(), 2);
    }

    #[test]
    fn a_repeat_rate_replays_the_script_on_its_own() {
        let mut runner = script(&[
            ("m_fMoveTo", "0"),
            ("spawnflags", "4"),
            ("m_flRepeat", "0.25"),
        ]);
        assert!(runner.trigger());
        let _ = runner.update(&sense());
        let finished = ScriptSense {
            sequence_finished: true,
            ..sense()
        };
        assert!(runner.update(&finished).completed);
        assert_eq!(runner.phase(), ScriptPhase::Repeating);
        // 0.25s at 0.1s a tick: two ticks still waiting, the third restarts.
        assert_eq!(runner.update(&sense()).action, ScriptAction::Idle);
        assert_eq!(runner.update(&sense()).action, ScriptAction::Idle);
        let step = runner.update(&sense());
        assert!(step.started);
        assert_eq!(runner.phase(), ScriptPhase::Moving);
        assert_eq!(runner.update(&sense()).action, ScriptAction::Play);
        assert!(runner.update(&finished).completed);
        assert_eq!(runner.completions(), 2);
    }

    #[test]
    fn a_disturbance_abandons_an_ordinary_script() {
        let mut runner = script(&[("m_fMoveTo", "1")]);
        assert!(runner.trigger());
        let _ = runner.update(&sense());
        let hurt = ScriptSense {
            disturbed: true,
            ..sense()
        };
        let step = runner.update(&hurt);
        assert!(step.interrupted && step.released && !step.completed);
        assert_eq!(runner.phase(), ScriptPhase::Dormant);
        assert_eq!(runner.completions(), 0);
        assert!(runner.trigger(), "an interrupted script can run again");
    }

    #[test]
    fn no_interruptions_ignores_a_disturbance() {
        let mut runner = script(&[("m_fMoveTo", "0"), ("spawnflags", "32")]);
        assert!(runner.trigger());
        let hurt = ScriptSense {
            disturbed: true,
            ..sense()
        };
        assert_eq!(runner.update(&hurt).action, ScriptAction::Play);
        assert_eq!(runner.phase(), ScriptPhase::Playing);
        let both = ScriptSense {
            sequence_finished: true,
            ..hurt
        };
        assert!(runner.update(&both).completed);
    }

    #[test]
    fn an_ai_script_is_uninterruptible_without_the_flag() {
        let raw: ohl_formats::bsp30::Entity = [("classname", "aiscripted_sequence")]
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let def = parse_entity(&raw, &Limits::default());
        let mut runner = ScriptRunner::new(ScriptDef::from_def(&def).expect("an aiscript"));
        assert!(runner.trigger());
        let hurt = ScriptSense {
            disturbed: true,
            ..sense()
        };
        assert_eq!(runner.update(&hurt).action, ScriptAction::Play);
    }

    #[test]
    fn a_disturbance_before_the_script_runs_changes_nothing() {
        let mut runner = script(&[("m_fMoveTo", "1")]);
        let hurt = ScriptSense {
            disturbed: true,
            ..sense()
        };
        let step = runner.update(&hurt);
        assert!(!step.interrupted && !step.released);
        assert_eq!(runner.phase(), ScriptPhase::Dormant);
    }

    #[test]
    fn a_non_finite_step_never_advances_a_repeat_timer() {
        let mut runner = script(&[
            ("m_fMoveTo", "0"),
            ("spawnflags", "4"),
            ("m_flRepeat", "1"),
        ]);
        assert!(runner.trigger());
        let _ = runner.update(&sense());
        let finished = ScriptSense {
            sequence_finished: true,
            ..sense()
        };
        let _ = runner.update(&finished);
        for dt in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            let step = runner.update(&ScriptSense {
                dt,
                ..ScriptSense::default()
            });
            assert!(!step.started);
        }
        assert_eq!(runner.timer(), 1.0);
    }
}
