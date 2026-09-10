//! The guard loop: one tick of "defend where you stand", computed from the
//! live game state.
//!
//! A planned route can end by standing still while a chain of map logic
//! runs ([`crate::route_plan`]'s scripted goals). Standing still is not a
//! safe thing to do on a map that has monsters on it: whatever the route
//! walked into can walk back, and a script that only holds "nothing
//! pressed" for a minute is a script that gets the player killed for as
//! long as the chain takes.
//!
//! [`guard_input`] is what a caller holds instead. It is a *policy*, not a
//! recording: given the game as it is this tick, it returns the [`Input`]
//! a defending player would deliver — select the best carried weapon that
//! can fire, turn toward the nearest hostile monster in line of sight,
//! fire once aligned, reload an empty clip, and otherwise stand still. The
//! caller runs it every tick for as long as it wants to guard, so the loop
//! is closed around the simulation rather than fixed in advance.
//!
//! A player with nothing that can reach what is shooting at them does not
//! stand there either: with no usable weapon (or only the crowbar against
//! something out of its reach) the loop *backs away* instead, along
//! whichever nearby heading its own hull traces say is clear and has floor
//! under it. Retreating is not a second policy bolted on; it is the same
//! "do not simply stand there and die" rule with the one tool a player
//! without a weapon still has.
//!
//! # Clean room
//!
//! **Project-authored tactic.** Nothing here reproduces any published
//! behaviour: the engagement rules below (which weapon is preferred, how
//! fast the view turns, how closely it has to be aligned before the
//! trigger is held, when a reload is pressed) are this project's own
//! choices for a headless harness, and no published source describes them.
//! Every *engine* number they lean on — a weapon's clip size, its ammo
//! type, the melee reach, the hitscan reach — belongs to `ohl-combat` and
//! `crate::combat`, cited or marked black-box there, and is read back here
//! rather than restated.
//!
//! # Determinism
//!
//! [`guard_input`] is a pure function of the game state: it reads the
//! inventory, the player's view angles and the level's monsters, and it
//! draws on no random stream at all (the engine's own seeded RNG keeps
//! driving the monsters, unchanged). Two games in the same state produce
//! the same input, which is what makes a guarded route replayable.
//!
//! # Logging
//!
//! Nothing here logs. Monster positions, classifications and counts are
//! media-derived and are returned to the caller as data, never written to
//! a diagnostic.

use glam::Vec3;
use ohl_combat::{Inventory, WeaponId, WeaponKind, hud_slot, spec};
use ohl_physics::Hull;

use crate::combat::{HITSCAN_RANGE, MELEE_RANGE};
use crate::{Game, Input, MOUSE_SENSITIVITY};

/// How far the guard turns in one tick, in degrees.
///
/// Project-authored: fast enough to bring a target that walked up behind
/// the player into the sights inside a second, slow enough that the turn
/// is a turn rather than a snap (the same reason `ohl-app`'s `look` line
/// spreads its rotation across a line's ticks).
pub const GUARD_TURN_DEGREES_PER_TICK: f32 = 12.0;

/// The widest the view may be off the target, in degrees, before the guard
/// holds the trigger down — the cap on a tolerance that otherwise narrows
/// with range (see [`GUARD_AIM_RADIUS`]). Project-authored.
pub const GUARD_AIM_TOLERANCE_DEGREES: f32 = 3.0;

/// How near the target the shot has to pass, in world units, for the guard
/// to consider itself aimed.
///
/// **Project-authored**, and the reason the tolerance is not a fixed
/// angle: a few degrees is a hit at arm's length and several feet wide
/// across a room, so a fixed angle empties a clip into the wall behind a
/// distant target. Converting a distance into the angle it subtends at the
/// target's own range fires only when the shot can land.
pub const GUARD_AIM_RADIUS: f32 = 8.0;

/// How far ahead a retreat probes with the player's own standing hull
/// before it is willing to walk that way, in world units.
///
/// Project-authored, and short on purpose: the loop re-probes every tick,
/// so this only has to be far enough that a step cannot end inside
/// something.
pub const GUARD_RETREAT_PROBE: f32 = 48.0;

/// The most a retreat is willing to step *down*, in world units: past this
/// the probed heading is treated as a ledge and skipped. Project-authored.
pub const GUARD_RETREAT_MAX_DROP: f32 = 64.0;

/// The headings a retreat considers, in degrees off "straight away from
/// the threat", tried in this order.
///
/// Project-authored: directly away first, then progressively more
/// sideways, so a player backed into a corner slides along the wall
/// instead of pressing into it. Fixed and ordered, so which way a retreat
/// goes never depends on anything but the geometry.
pub const GUARD_RETREAT_OFFSETS: [f32; 7] = [0.0, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0];

/// The farthest a hostile monster may be and still be guarded against, in
/// world units.
///
/// Read back from the engine's own reach for a hitscan shot
/// ([`crate::combat::HITSCAN_RANGE`]), so the guard never aims at
/// something its own shot could not reach.
pub const GUARD_ENGAGE_RANGE: f32 = HITSCAN_RANGE;

/// The weapons the guard will carry, best first.
///
/// **Project-authored.** This is an ordering for a headless harness, not a
/// published ranking, and what it ranks by is *how much one trigger pull
/// puts into one target*: the guard's whole problem is time-to-kill while
/// something is already shooting back, so the weapon that ends a fight in
/// the fewest pulls comes first and the magazine-fed ones follow. The
/// crowbar is not in the list at all — it is the last resort
/// [`best_weapon`] falls back to, because it only reaches
/// [`MELEE_RANGE`]. Thrown and placed weapons (grenades, satchels,
/// tripmines, snarks) and the rocket launcher are deliberately absent:
/// all of them either need a throw the guard does not model or put an
/// explosion where a standing player is.
pub const GUARD_WEAPON_PREFERENCE: [WeaponId; 8] = [
    WeaponId::Python,
    WeaponId::Shotgun,
    WeaponId::Crossbow,
    WeaponId::Mp5,
    WeaponId::Gauss,
    WeaponId::Egon,
    WeaponId::HornetGun,
    WeaponId::Glock,
];

/// What the guard decided to do this tick, as data.
///
/// Returned alongside the [`Input`] so a test (or a harness probe) can
/// assert on the decision without re-deriving it, and so nothing has to be
/// logged to find out what happened.
// The flags below are genuinely independent facts about one tick's
// decision, exactly as `ohl_engine::Input`'s own buttons are; folding them
// into an enum would only move the same fan-out to the caller.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GuardDecision {
    /// Whether a hostile monster was in line of sight and in range.
    pub has_target: bool,
    /// How far away that monster was, in world units.
    pub target_distance: Option<f32>,
    /// The weapon the guard wants in hand, if it owns one it can use.
    pub weapon: Option<WeaponId>,
    /// Whether this tick presses a weapon-slot key to get there.
    pub selecting: bool,
    /// Whether this tick holds the trigger.
    pub firing: bool,
    /// Whether this tick presses reload.
    pub reloading: bool,
    /// Whether this tick backs away from the threat instead of engaging
    /// it (nothing carried can reach it).
    pub retreating: bool,
}

/// One tick of the guard loop for `game`: see the module docs.
///
/// Holds no movement key ever — guarding is standing your ground, and a
/// route that has arrived where it meant to arrive must not wander off
/// mid-wait.
#[must_use]
pub fn guard_input(game: &Game) -> Input {
    guard_step(game).0
}

/// [`guard_input`] plus the [`GuardDecision`] behind it.
#[must_use]
pub fn guard_step(game: &Game) -> (Input, GuardDecision) {
    let mut input = Input::default();
    let mut decision = GuardDecision::default();
    if game.player_health() <= 0.0 {
        return (input, decision);
    }

    let eye = Vec3::from_array(game.eye_position());
    let inventory = game.inventory();
    let target = nearest_visible_hostile(game, eye);
    decision.has_target = target.is_some();

    let distance = target.map_or(f32::MAX, |position| eye.distance(position));
    decision.target_distance = target.map(|position| eye.distance(position));
    let weapon = best_weapon(&inventory);
    decision.weapon = weapon;

    // Nothing carried can reach what is shooting *right now*: back away
    // rather than stand still and be shot. `None` covers an empty
    // loadout; the melee arm covers a crowbar against something across
    // the room; and an unloaded weapon covers the seconds a reload takes,
    // which are seconds the player has no answer for either.
    let can_engage = match weapon {
        None => false,
        Some(id) => {
            is_loaded(&inventory, id)
                && (!matches!(spec(id).kind, WeaponKind::Melee) || distance <= MELEE_RANGE)
        }
    };
    if let Some(target) = target
        && !can_engage
    {
        decision.retreating = true;
        let mut input = retreat_input(game, target);
        // Backing away, drawing and reloading are not alternatives: both
        // of the latter are edges the engine latches, and walking
        // interrupts neither. A weapon that is never *selected* is never
        // reloaded either, so the draw has to happen even while running.
        if let Some(id) = weapon
            && inventory.selected() != Some(id)
        {
            input.select_slot = Some(hud_slot(id).slot);
            decision.selecting = true;
        }
        if weapon.is_some_and(|id| needs_reload(&inventory, id)) {
            input.reload = true;
            decision.reloading = true;
        }
        return (input, decision);
    }
    let Some(weapon) = weapon else {
        // Nothing to fight with and nothing to fight: standing still is
        // the right answer, and the route is where it means to be.
        return (input, decision);
    };

    if inventory.selected() != Some(weapon) {
        // `Inventory::select_slot` selects the lowest owned position in a
        // slot, and cycles within the slot when the selection is already
        // in it, so pressing the wanted weapon's own slot converges on it
        // in at most as many ticks as that slot holds weapons.
        input.select_slot = Some(hud_slot(weapon).slot);
        decision.selecting = true;
    }

    let Some(target) = target else {
        // Nothing in sight: top up *every* carried clip, best weapon
        // first, so the first tick that does see something is a tick that
        // can shoot — and so a weapon that runs dry mid-fight has a loaded
        // one to switch to instead of a reload to stand through.
        if let Some(id) = weapon_wanting_reload(&inventory) {
            decision.weapon = Some(id);
            if inventory.selected() == Some(id) {
                input.reload = true;
                decision.reloading = true;
            } else {
                input.select_slot = Some(hud_slot(id).slot);
                decision.selecting = true;
            }
        }
        return (input, decision);
    };

    let (delta, aimed) = turn_toward(game, eye, target);
    input.mouse_delta = delta;

    if needs_reload(&inventory, weapon) {
        input.reload = true;
        decision.reloading = true;
        return (input, decision);
    }
    let in_reach = if matches!(spec(weapon).kind, WeaponKind::Melee) {
        distance <= MELEE_RANGE
    } else {
        distance <= GUARD_ENGAGE_RANGE
    };
    if aimed && in_reach && !decision.selecting {
        input.attack = true;
        decision.firing = true;
    }
    (input, decision)
}

/// The eye position of the nearest living monster hostile to the player
/// that is in range, has an unobstructed line from `eye`, and that a shot
/// aimed at it would actually *reach*.
///
/// That last test is not belt-and-braces. A monster whose model this map
/// never loaded carries no hitbox, and a hitscan shot passes straight
/// through it however carefully it is aimed: without the test the loop
/// empties its clip into something it cannot hurt and is caught reloading
/// when a monster it *can* hurt arrives. Asking the engine's own attack
/// trace ([`Game::shot_would_reach`]) is the only honest way to tell the
/// two apart, and it subsumes the line-of-sight test for anything solid
/// in between.
///
/// Ties are broken the way [`crate::AiState::hostile_monster_eyes`] orders
/// its result (ascending entity id), so the choice never depends on query
/// iteration order.
fn nearest_visible_hostile(game: &Game, eye: Vec3) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for (entity, position) in game.hostile_monster_eyes() {
        let distance = eye.distance(position);
        if !distance.is_finite() || distance > GUARD_ENGAGE_RANGE {
            continue;
        }
        if best.is_some_and(|(best_distance, _)| distance >= best_distance) {
            continue;
        }
        if !has_line_of_sight(game, eye, position) {
            continue;
        }
        if game.shot_would_reach(position) != Some(entity) {
            continue;
        }
        best = Some((distance, position));
    }
    best.map(|(_, position)| position)
}

/// Whether a point-hull trace from `from` to `to` reaches, the same test
/// `ohl_ai::senses` makes before a monster may shoot at what it sees.
fn has_line_of_sight(game: &Game, from: Vec3, to: Vec3) -> bool {
    let Some(collision) = game.collision() else {
        return true;
    };
    let trace = collision.trace(Hull::Point, from, to);
    trace.fraction >= 1.0 && !trace.start_solid
}

/// The best weapon in `inventory` that can fire, or `None`.
///
/// Two passes over [`GUARD_WEAPON_PREFERENCE`], then the crowbar: a
/// weapon that can shoot *this tick* beats a better one that would have to
/// reload first (a reload is seconds of standing still being shot at), and
/// the crowbar is only ever picked when nothing else can fire at all —
/// even then only because holding it is better than holding nothing.
fn best_weapon(inventory: &Inventory) -> Option<WeaponId> {
    GUARD_WEAPON_PREFERENCE
        .into_iter()
        .find(|id| inventory.has_weapon(*id) && is_loaded(inventory, *id))
        .or_else(|| {
            GUARD_WEAPON_PREFERENCE
                .into_iter()
                .find(|id| inventory.has_weapon(*id) && can_fire(inventory, *id))
        })
        .or_else(|| {
            inventory
                .has_weapon(WeaponId::Crowbar)
                .then_some(WeaponId::Crowbar)
        })
}

/// Whether `id` can put a shot downrange without reloading first.
fn is_loaded(inventory: &Inventory, id: WeaponId) -> bool {
    let spec = spec(id);
    let Some(ammo) = spec.ammo else {
        return true;
    };
    match spec.clip_size {
        Some(_) => inventory.clip(id) > 0,
        None => inventory.ammo(ammo).current() > 0,
    }
}

/// Whether `id` has anything to shoot with: no ammo type at all, a loaded
/// round, or a reserve a reload can draw on.
fn can_fire(inventory: &Inventory, id: WeaponId) -> bool {
    let spec = spec(id);
    let Some(ammo) = spec.ammo else {
        return true;
    };
    inventory.clip(id) > 0 || inventory.ammo(ammo).current() > 0
}

/// The first carried weapon, in [`GUARD_WEAPON_PREFERENCE`] order, whose
/// clip is empty while its reserve is not.
///
/// Used only when nothing is in sight: quiet seconds are the ones a
/// reload is free in, and a second loaded weapon is what turns "stand
/// still for four seconds while it reloads" into "switch and keep
/// firing".
fn weapon_wanting_reload(inventory: &Inventory) -> Option<WeaponId> {
    GUARD_WEAPON_PREFERENCE
        .into_iter()
        .find(|id| inventory.has_weapon(*id) && needs_reload(inventory, *id))
}

/// Whether `id` is a clip weapon whose clip is empty while its reserve is
/// not — the one case a reload is the only useful thing to press.
fn needs_reload(inventory: &Inventory, id: WeaponId) -> bool {
    let spec = spec(id);
    let (Some(_), Some(ammo)) = (spec.clip_size, spec.ammo) else {
        return false;
    };
    inventory.clip(id) == 0 && inventory.ammo(ammo).current() > 0
}

/// The mouse delta that turns the view from where it is toward `target`,
/// clamped to [`GUARD_TURN_DEGREES_PER_TICK`] on each axis, and whether
/// the view *after* that turn is inside
/// [`GUARD_AIM_TOLERANCE_DEGREES`] of the target.
///
/// The turn is reported as a mouse delta because that is the only way an
/// [`Input`] can move the view, and it is converted through the same
/// [`MOUSE_SENSITIVITY`] the real input path scales by, inverted exactly
/// as `ohl_physics::PlayerController::apply_mouse_delta` applies it.
fn turn_toward(game: &Game, eye: Vec3, target: Vec3) -> ((f32, f32), bool) {
    let to_target = target - eye;
    let horizontal = to_target.truncate().length();
    let tolerance = aim_tolerance(to_target.length());
    if !to_target.is_finite() || (horizontal == 0.0 && to_target.z == 0.0) {
        return ((0.0, 0.0), false);
    }
    let wanted_yaw = to_target.y.atan2(to_target.x).to_degrees();
    // `PlayerController::view_direction` builds its z from `-sin(pitch)`,
    // so a target above the eye is a *negative* pitch.
    let wanted_pitch = -to_target.z.atan2(horizontal).to_degrees();

    let (yaw, pitch) = game.player_view_angles();
    let yaw_delta = shortest_turn(yaw, wanted_yaw)
        .clamp(-GUARD_TURN_DEGREES_PER_TICK, GUARD_TURN_DEGREES_PER_TICK);
    let pitch_delta =
        (wanted_pitch - pitch).clamp(-GUARD_TURN_DEGREES_PER_TICK, GUARD_TURN_DEGREES_PER_TICK);

    let aimed = shortest_turn(yaw + yaw_delta, wanted_yaw).abs() <= tolerance
        && (wanted_pitch - (pitch + pitch_delta)).abs() <= tolerance;

    // Inverted from `apply_mouse_delta`: `yaw -= delta_x * sensitivity`,
    // `pitch += delta_y * sensitivity`.
    let delta_x = -yaw_delta / MOUSE_SENSITIVITY;
    let delta_y = pitch_delta / MOUSE_SENSITIVITY;
    ((delta_x, delta_y), aimed)
}

/// The input that backs the player away from `target`: turn toward the
/// first heading in [`GUARD_RETREAT_OFFSETS`] whose hull trace is clear
/// and has floor under it, and walk that way once the view has come round
/// far enough to actually go there.
///
/// Deterministic: the candidate headings are a fixed list in a fixed
/// order, and whether one is taken is decided entirely by two traces
/// against the level's own collision.
fn retreat_input(game: &Game, target: Vec3) -> Input {
    let mut input = Input::default();
    let eye = Vec3::from_array(game.eye_position());
    let origin = Vec3::from_array(game.player_origin());
    let away = (eye - target).truncate();
    let Some(away) = away.try_normalize() else {
        return input;
    };
    let away_yaw = away.y.atan2(away.x).to_degrees();

    let Some(collision) = game.collision() else {
        return input;
    };
    let heading = GUARD_RETREAT_OFFSETS.into_iter().find_map(|offset| {
        let yaw = (away_yaw + offset).rem_euclid(360.0);
        let radians = yaw.to_radians();
        let step = Vec3::new(radians.cos(), radians.sin(), 0.0) * GUARD_RETREAT_PROBE;
        let ahead = collision.trace(Hull::Standing, origin, origin + step);
        if ahead.fraction < 1.0 || ahead.start_solid {
            return None;
        }
        let landing = origin + step;
        let down = collision.trace(
            Hull::Standing,
            landing,
            landing - Vec3::Z * GUARD_RETREAT_MAX_DROP,
        );
        (down.fraction < 1.0).then_some(yaw)
    });
    let Some(heading) = heading else {
        return input;
    };

    let (yaw, _) = game.player_view_angles();
    let delta = shortest_turn(yaw, heading)
        .clamp(-GUARD_TURN_DEGREES_PER_TICK, GUARD_TURN_DEGREES_PER_TICK);
    input.mouse_delta = (-delta / MOUSE_SENSITIVITY, 0.0);
    // Walking follows the view, so only walk once the view is actually
    // pointing where the probe said it was safe to go.
    if shortest_turn(yaw + delta, heading).abs() <= GUARD_AIM_TOLERANCE_DEGREES {
        input.forward = 1;
    }
    input
}

/// The angle, in degrees, that [`GUARD_AIM_RADIUS`] subtends at
/// `distance` — capped at [`GUARD_AIM_TOLERANCE_DEGREES`] so a target in
/// the player's face does not widen the tolerance to nonsense.
fn aim_tolerance(distance: f32) -> f32 {
    if !distance.is_finite() || distance <= 0.0 {
        return GUARD_AIM_TOLERANCE_DEGREES;
    }
    GUARD_AIM_RADIUS
        .atan2(distance)
        .to_degrees()
        .min(GUARD_AIM_TOLERANCE_DEGREES)
}

/// The signed turn, in `(-180, 180]` degrees, from `from` to `to`.
fn shortest_turn(from: f32, to: f32) -> f32 {
    let delta = (to - from).rem_euclid(360.0);
    if delta > 180.0 { delta - 360.0 } else { delta }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shortest_turn_never_goes_the_long_way_round() {
        assert!((shortest_turn(350.0, 10.0) - 20.0).abs() < 1e-3);
        assert!((shortest_turn(10.0, 350.0) + 20.0).abs() < 1e-3);
        assert!((shortest_turn(0.0, 180.0) - 180.0).abs() < 1e-3);
    }

    #[test]
    fn the_aim_tolerance_narrows_with_range() {
        let near = aim_tolerance(64.0);
        let far = aim_tolerance(1024.0);
        assert!(far < near, "a distant target has to be aimed at harder");
        assert!(near <= GUARD_AIM_TOLERANCE_DEGREES);
        assert!((aim_tolerance(0.0) - GUARD_AIM_TOLERANCE_DEGREES).abs() < f32::EPSILON);
        // The angle really is the one the radius subtends.
        let expected = (GUARD_AIM_RADIUS / 1024.0).atan().to_degrees();
        assert!((far - expected).abs() < 0.01);
    }

    #[test]
    fn a_weapon_with_neither_clip_nor_reserve_is_not_pickable() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Mp5);
        assert_eq!(best_weapon(&inventory), None);
        inventory.give_ammo(spec(WeaponId::Mp5).ammo.expect("the mp5 uses ammo"), 25);
        assert_eq!(best_weapon(&inventory), Some(WeaponId::Mp5));
    }

    #[test]
    fn the_crowbar_is_the_last_resort_and_never_the_first_choice() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Crowbar);
        assert_eq!(best_weapon(&inventory), Some(WeaponId::Crowbar));
        inventory.give_weapon(WeaponId::Glock);
        inventory.give_ammo(spec(WeaponId::Glock).ammo.expect("the glock uses ammo"), 17);
        assert_eq!(best_weapon(&inventory), Some(WeaponId::Glock));
    }

    #[test]
    fn a_loaded_weapon_beats_a_better_one_that_would_have_to_reload_first() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Mp5);
        inventory.give_ammo(spec(WeaponId::Mp5).ammo.expect("the mp5 uses ammo"), 50);
        inventory.give_weapon(WeaponId::Glock);
        inventory.give_ammo(spec(WeaponId::Glock).ammo.expect("the glock uses ammo"), 17);
        inventory.set_clip(WeaponId::Glock, 17);
        assert_eq!(
            best_weapon(&inventory),
            Some(WeaponId::Glock),
            "the loaded pistol beats the empty-clipped submachine gun"
        );
        inventory.set_clip(WeaponId::Mp5, 10);
        assert_eq!(best_weapon(&inventory), Some(WeaponId::Mp5));
    }

    #[test]
    fn a_quiet_moment_tops_up_every_carried_clip_best_weapon_first() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Mp5);
        inventory.give_ammo(spec(WeaponId::Mp5).ammo.expect("the mp5 uses ammo"), 50);
        inventory.give_weapon(WeaponId::Python);
        inventory.give_ammo(
            spec(WeaponId::Python).ammo.expect("the python uses ammo"),
            12,
        );
        assert_eq!(weapon_wanting_reload(&inventory), Some(WeaponId::Python));
        inventory.set_clip(WeaponId::Python, 6);
        assert_eq!(
            weapon_wanting_reload(&inventory),
            Some(WeaponId::Mp5),
            "the next one down still wants loading"
        );
        inventory.set_clip(WeaponId::Mp5, 50);
        assert_eq!(weapon_wanting_reload(&inventory), None);
    }

    #[test]
    fn a_reload_is_pressed_only_for_an_empty_clip_with_a_reserve() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Glock);
        let ammo = spec(WeaponId::Glock).ammo.expect("the glock uses ammo");
        assert!(!needs_reload(&inventory, WeaponId::Glock));
        inventory.give_ammo(ammo, 17);
        assert!(needs_reload(&inventory, WeaponId::Glock));
        inventory.set_clip(WeaponId::Glock, 5);
        assert!(!needs_reload(&inventory, WeaponId::Glock));
        // The crowbar has no clip at all, so it never wants a reload.
        inventory.give_weapon(WeaponId::Crowbar);
        assert!(!needs_reload(&inventory, WeaponId::Crowbar));
    }
}
