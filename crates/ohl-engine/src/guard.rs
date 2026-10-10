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

/// Project-authored reactive warning window; no ballistic prediction is implied.
const TIMED_BLAST_RETREAT_SECONDS: f32 = 2.0;

/// Project-authored fixed work bound for a complete supported escape.
const MAX_BLAST_ESCAPE_SUPPORT_SEGMENTS: u8 = 16;

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
    /// Whether this tick backs away from an unengageable monster or timed blast.
    pub retreating: bool,
}

/// One tick of the guard loop for `game`: see the module docs.
///
/// Holds position unless the existing retreat policy finds a safe escape from
/// an unengageable monster or an imminent live timed blast.
#[must_use]
pub fn guard_input(game: &Game) -> Input {
    guard_step(game).0
}

/// [`guard_input`] plus the [`GuardDecision`] behind it.
#[must_use]
pub fn guard_step(game: &Game) -> (Input, GuardDecision) {
    let (mut input, mut decision) = combat_guard_step(game);
    if game.player_health() > 0.0
        && !decision.retreating
        && let Some(threat) = game.timed_blast_threat(TIMED_BLAST_RETREAT_SECONDS)
    {
        input = complete_blast_escape_input(game, threat, input)
            .unwrap_or_else(|| blast_retreat_input(game, threat.position, input));
        decision.retreating = true;
    }
    (input, decision)
}

fn combat_guard_step(game: &Game) -> (Input, GuardDecision) {
    let mut input = Input::default();
    let mut decision = GuardDecision::default();
    if game.player_health() <= 0.0 {
        return (input, decision);
    }

    let eye = Vec3::from_array(game.eye_position());
    let inventory = game.inventory();
    let context = game.guard_world_context();
    let target = nearest_visible_hostile(game, &context, eye);
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

/// A reachable aim point on the nearest living monster hostile to the player
/// that is in range, has an unobstructed line from `eye`, and that a shot
/// aimed at it would actually *reach*.
///
/// That last test is not belt-and-braces. A monster whose model this map
/// never loaded carries no hitbox, and a hitscan shot passes straight
/// through it however carefully it is aimed: without the test the loop
/// empties its clip into something it cannot hurt and is caught reloading
/// when a monster it *can* hurt arrives. Asking the engine's own attack
/// trace over a complete current-state index is the only honest way to tell the
/// two apart, and it subsumes the line-of-sight test for anything solid
/// in between.
///
/// Ties are broken the way [`crate::AiState::hostile_monster_eyes`] orders
/// its result (ascending entity id), so the choice never depends on query
/// iteration order.
fn nearest_visible_hostile(
    game: &Game,
    context: &crate::game::GuardWorldContext<'_>,
    eye: Vec3,
) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for (entity, position) in game.hostile_monster_eyes() {
        let distance = eye.distance(position);
        if !distance.is_finite() || distance > GUARD_ENGAGE_RANGE {
            continue;
        }
        if best.is_some_and(|(best_distance, _)| distance >= best_distance) {
            continue;
        }
        let reachable = |aim: Vec3| {
            let aim_distance = eye.distance(aim);
            aim.is_finite()
                && aim_distance.is_finite()
                && aim_distance <= GUARD_ENGAGE_RANGE
                && has_line_of_sight(game, eye, aim)
                && context.shot_would_reach(aim) == Some(entity)
        };
        // Keep the eye first and keep candidate ranking measured to that eye.
        // A valid model's damage boxes need not contain its sight position.
        let aim = if reachable(position) {
            Some(position)
        } else {
            context
                .hitbox_centers(entity)
                .find(|&point| reachable(point))
        };
        if let Some(aim) = aim {
            best = Some((distance, aim));
        }
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

// TODO(black-box): project-authored complete current-radius escape preference.
// A full supported corridor avoids the artificial corner made by local lookahead.
// This is not a forecast, arrival deadline, or guarantee against other hazards.
fn complete_blast_escape_input(
    game: &Game,
    threat: crate::projectiles::TimedBlastThreat,
    input: Input,
) -> Option<Input> {
    let collision = game.collision()?;
    let origin = Vec3::from_array(game.player_origin());
    let slope_limit = game.move_config().slope_limit;
    if !origin.is_finite()
        || !threat.position.is_finite()
        || !threat.radius.is_finite()
        || threat.radius <= 0.0
        || !slope_limit.is_finite()
    {
        return None;
    }
    let mut best: Option<(f32, Input)> = None;
    for (forward, right) in [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ] {
        let candidate = Input {
            forward,
            right,
            ..input
        };
        let Some((hull, wish)) = game.guard_movement_wish(&candidate) else {
            continue;
        };
        if !matches!(hull, Hull::Standing | Hull::Crouched) || wish.z.abs() > f32::EPSILON {
            continue;
        }
        let Some(direction) = Vec3::new(wish.x, wish.y, 0.0).try_normalize() else {
            continue;
        };
        let (min, max) = hull.bounds();
        let nearest_projection = Vec3::new(
            if direction.x >= 0.0 { min.x } else { max.x },
            if direction.y >= 0.0 { min.y } else { max.y },
            0.0,
        )
        .dot(direction);
        let distance = threat.radius + ohl_physics::DIST_EPSILON
            - (origin - threat.position).dot(direction)
            - nearest_projection;
        if !distance.is_finite()
            || distance <= 0.0
            || best.is_some_and(|(shortest, _)| distance >= shortest)
        {
            continue;
        }
        let Some(intervals) = (1..=MAX_BLAST_ESCAPE_SUPPORT_SEGMENTS)
            .find(|count| distance <= GUARD_RETREAT_PROBE * f32::from(*count))
        else {
            continue;
        };
        let mut endpoint = origin + direction * distance;
        endpoint.z = origin.z;
        if !endpoint.is_finite() || !(endpoint + min).is_finite() || !(endpoint + max).is_finite() {
            continue;
        }
        let nearest = threat.position.clamp(endpoint + min, endpoint + max);
        let separation = threat.position.distance(nearest);
        if !separation.is_finite()
            || separation < threat.radius
            || !complete_blast_corridor(collision, hull, origin, endpoint, intervals, slope_limit)
        {
            continue;
        }
        best = Some((distance, candidate));
    }
    best.map(|(_, candidate)| candidate)
}

fn complete_blast_corridor(
    collision: &ohl_physics::CollisionModel,
    hull: Hull,
    origin: Vec3,
    endpoint: Vec3,
    intervals: u8,
    slope_limit: f32,
) -> bool {
    let chord = collision.trace(hull, origin, endpoint);
    if !chord.fraction.is_finite()
        || !chord.end_pos.is_finite()
        || !chord.plane_normal.is_finite()
        || chord.fraction < 1.0
        || chord.start_solid
        || chord.all_solid
        || !chord
            .end_pos
            .abs_diff_eq(endpoint, ohl_physics::DIST_EPSILON)
    {
        return false;
    }
    // At most 17 existing down-probes per candidate, including both endpoints.
    (0..=intervals).all(|index| {
        let mut at = if index == 0 {
            origin
        } else if index == intervals {
            endpoint
        } else {
            origin.lerp(endpoint, f32::from(index) / f32::from(intervals))
        };
        at.z = origin.z;
        if !at.is_finite() {
            return false;
        }
        let floor = collision.trace(hull, at, at - Vec3::Z * GUARD_RETREAT_MAX_DROP);
        floor.fraction.is_finite()
            && floor.end_pos.is_finite()
            && floor.plane_normal.is_finite()
            && floor.fraction >= 0.0
            && floor.fraction < 1.0
            && !floor.start_solid
            && !floor.all_solid
            && floor.plane_normal.z >= slope_limit
    })
}

/// Project-authored: probe all eight walking inputs after the existing combat
/// turn. Preserve that view and its firing decision; rank safe wishes away from
/// the current blast, but permit a toward-side exit when cornered. This is a
/// local clearance policy, not a guarantee against a future blast trajectory.
fn blast_retreat_input(game: &Game, threat: Vec3, mut input: Input) -> Input {
    input.forward = 0;
    input.right = 0;
    let origin = Vec3::from_array(game.player_origin());
    let Some(collision) = game.collision() else {
        return input;
    };
    if !origin.is_finite() || !threat.is_finite() {
        return input;
    }
    let away = (origin - threat)
        .truncate()
        .try_normalize()
        .unwrap_or(glam::Vec2::X);
    let mut best: Option<(f32, Input)> = None;
    for (forward, right) in [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ] {
        let candidate = Input {
            forward,
            right,
            ..input
        };
        let Some((hull, wish)) = game.guard_movement_wish(&candidate) else {
            continue;
        };
        let score = wish.truncate().dot(away);
        if best.is_some_and(|(best_score, _)| score <= best_score) {
            continue;
        }
        let landing = origin + wish * GUARD_RETREAT_PROBE;
        let ahead = collision.trace(hull, origin, landing);
        if ahead.fraction < 1.0 || ahead.start_solid || ahead.all_solid {
            continue;
        }
        let down = collision.trace(hull, landing, landing - Vec3::Z * GUARD_RETREAT_MAX_DROP);
        if down.fraction < 1.0 && !down.start_solid && !down.all_solid {
            best = Some((score, candidate));
        }
    }
    best.map_or(input, |(_, candidate)| candidate)
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

    // New BSP/Studio bytes are project-authored. The model's sight position
    // and broad bounds intentionally do not promise a damage box at its eye.
    fn guard_aim_model(boxes: &[([f32; 3], [f32; 3])]) -> Vec<u8> {
        let (mut bytes, layout) = ohl_formats::test_support::build_minimal_mdl10();
        for (offset, values) in [
            (76, [0.0_f32, 0.0, 64.0]),
            (88, [-48.0, -48.0, 0.0]),
            (100, [48.0, 48.0, 72.0]),
            (112, [-48.0, -48.0, 0.0]),
            (124, [48.0, 48.0, 72.0]),
        ] {
            for (axis, value) in values.into_iter().enumerate() {
                bytes[offset + axis * 4..offset + axis * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        // A real two-frame, ten-second sequence with a static root channel.
        bytes[layout.sequences_offset + 32..layout.sequences_offset + 36]
            .copy_from_slice(&0.1_f32.to_le_bytes());
        bytes[layout.anim_data_offset + 26..layout.anim_data_offset + 30].fill(0);
        assert_eq!(bytes.len(), layout.hitboxes_offset);
        for (min, max) in boxes {
            bytes.extend_from_slice(&0_i32.to_le_bytes()); // root bone
            bytes.extend_from_slice(&0_i32.to_le_bytes()); // generic hit group
            for value in min.iter().chain(max) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes[156..160].copy_from_slice(&u32::try_from(boxes.len()).unwrap().to_le_bytes());
        let length = u32::try_from(bytes.len()).unwrap();
        bytes[72..76].copy_from_slice(&length.to_le_bytes());
        bytes
    }

    // Ordinary auto-triggered No movement script, loadout and fixed-step input.
    #[allow(clippy::too_many_lines)]
    fn guard_aim_fixture(
        boxes: &[([f32; 3], [f32; 3])],
        yaw: f32,
        blocker: GuardBlocker,
    ) -> (Game, ohl_game::hecs::Entity) {
        use crate::test_support::{entity_block, script_room_entities};
        use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
        let mut extra = [
            entity_block(
                "monster_human_grunt",
                [256.0, 0.0, 36.0],
                yaw,
                &[("targetname", "aim_target")],
            ),
            entity_block(
                "scripted_sequence",
                [256.0, 0.0, 36.0],
                yaw,
                &[
                    ("targetname", "aim_hold"),
                    ("m_iszEntity", "aim_target"),
                    ("m_fMoveTo", "0"),
                    ("m_iszPlay", "idle"),
                    ("spawnflags", "32"),
                ],
            ),
            entity_block("trigger_auto", [0.0; 3], 0.0, &[("target", "aim_hold")]),
        ]
        .concat();
        match blocker {
            GuardBlocker::None => {}
            GuardBlocker::Door => extra.push_str(&entity_block(
                "func_door",
                [0.0; 3],
                90.0,
                &[
                    ("targetname", "aim_blocker"),
                    ("model", "*1"),
                    ("speed", "100"),
                ],
            )),
            GuardBlocker::Friendly => extra.push_str(&entity_block(
                "monster_barney",
                [128.0, 0.0, 36.0],
                0.0,
                &[("targetname", "aim_blocker"), ("spawnflags", "16")],
            )),
        }
        let mut builder = Bsp30Builder::new();
        builder.set_entities_text(&script_room_entities([0.0, 0.0, 36.0], &extra));
        let floor =
            builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
        builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], floor, 2, 0, 0);
        if matches!(blocker, GuardBlocker::Door) {
            let min = [120.0, -128.0, 0.0];
            let max = [136.0, 128.0, 128.0];
            let hulls = builder.push_collision_hulls(&[CollisionBrush::box_brush(min, max)]);
            builder.push_model(min, max, [0.0; 3], hulls, 2, 0, 0);
        }
        let mut assets = crate::MemoryAssets::new();
        assets.insert("maps/ohlguardaim.bsp", builder.build());
        assets.insert(
            ohl_ai::MonsterKind::HumanGrunt
                .default_model_path()
                .unwrap(),
            guard_aim_model(boxes),
        );
        assets.insert(
            ohl_ai::MonsterKind::Barney.default_model_path().unwrap(),
            guard_aim_model(&[([-24.0, -24.0, 0.0], [24.0, 24.0, 72.0])]),
        );
        let mut game = Game::load(&assets, "ohlguardaim").expect("authored aim room");
        game.give_start_inventory(&[crate::StartInventoryItem::Weapon(WeaponId::Glock)]);
        let target = game.registry().find("aim_target")[0];
        for _ in 0..16 {
            game.tick(crate::TICK_SECONDS, &Input::default());
        }
        let body = *game.registry().world.get::<&ohl_ai::Actor>(target).unwrap();
        assert!(body.alive && body.health > 0.0 && game.player_health() > 0.0);
        assert!(
            game.registry()
                .world
                .get::<&ohl_ai::MonsterAi>(target)
                .is_ok()
        );
        assert!(
            game.registry()
                .world
                .get::<&ohl_ai::ScriptHold>(target)
                .is_ok()
        );
        assert_eq!(game.script_start_count(), 1);
        assert_eq!(game.script_completion_count(), 0);
        assert_eq!(game.active_script_count(), 1);
        assert_eq!(game.hostile_monster_eyes(), vec![(target, body.eye())]);
        let transform = *game
            .registry()
            .world
            .get::<&ohl_game::registry::Transform>(target)
            .unwrap();
        assert_eq!(body.origin, transform.origin);
        assert!((body.origin.truncate() - Vec3::new(256.0, 0.0, 0.0).truncate()).length() < 0.001);
        assert!((body.eye() - body.origin - Vec3::Z * 64.0).length() < 0.001);
        let query = body.query_origin();
        let at = game.collision().unwrap().trace(body.hull, query, query);
        assert!(!at.start_solid && !at.all_solid);
        let support = game
            .collision()
            .unwrap()
            .trace(body.hull, query, query - Vec3::Z * 2.0);
        assert!(!support.start_solid && support.fraction < 1.0 && support.plane_normal.z >= 0.7);
        assert!(
            game.registry()
                .world
                .get::<&crate::StudioAnim>(target)
                .unwrap()
                .cycle
                > 0.0
        );
        (game, target)
    }

    fn guard_aim_entry(
        game: &mut Game,
        target: ohl_game::hecs::Entity,
    ) -> ohl_combat::EntityHitboxes {
        let index = fresh_guard_index(game);
        index
            .entries()
            .iter()
            .find(|entry| entry.id == crate::ids::entity_id(target))
            .expect("accepted full-generation current posed entry")
            .clone()
    }

    // Keep ordinary geometry, read-only state and final real-input consequence together.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn guard_aim_fallback_acquires_and_hits_below_the_eye() {
        use crate::components::{StudioAnim, StudioGait};
        use ohl_ai::Actor;
        use ohl_game::registry::Transform;
        let (mut game, target) = guard_aim_fixture(
            &[([-12.0, -12.0, 12.0], [12.0, 12.0, 44.0])],
            0.0,
            GuardBlocker::None,
        );
        let entry = guard_aim_entry(&mut game, target);
        assert_eq!(entry.boxes.len(), 1);
        let body = *game.registry().world.get::<&Actor>(target).unwrap();
        let transform = *game.registry().world.get::<&Transform>(target).unwrap();
        let anim = *game.registry().world.get::<&StudioAnim>(target).unwrap();
        let gait = game
            .registry()
            .world
            .get::<&StudioGait>(target)
            .ok()
            .map(|g| (*g).clone());
        let ai = game.ai_state_hash();
        let cached = {
            let (_, systems) = game.level_and_systems_mut();
            systems.hitboxes().entries().to_vec()
        };
        let eye = Vec3::from_array(game.eye_position());
        assert!(has_line_of_sight(&game, eye, body.eye()));
        assert!(eye.distance(body.eye()) < GUARD_ENGAGE_RANGE);
        let local_eye = entry.rotation.conjugate() * (body.eye() - entry.origin);
        assert!(
            local_eye.z > entry.boxes[0].max.z,
            "real eye lies above every damage box"
        );
        // Independent authored centre, not obtained from the new context helper.
        let centre = transform.origin + Vec3::Z * 28.0;
        let fresh = fresh_guard_index(&mut game);
        assert_eq!(trace_guard_index(&game, &fresh, body.eye()), None);
        assert_eq!(trace_guard_index(&game, &fresh, centre), Some(target));
        assert!(has_line_of_sight(&game, eye, centre));
        assert!(centre.is_finite() && eye.distance(centre) < GUARD_ENGAGE_RANGE);
        let decision = guard_step(&game);
        assert_eq!(
            guard_step(&game),
            decision,
            "repeated current queries are deterministic"
        );
        assert_eq!(*game.registry().world.get::<&Actor>(target).unwrap(), body);
        assert_eq!(
            *game.registry().world.get::<&Transform>(target).unwrap(),
            transform
        );
        assert_eq!(
            *game.registry().world.get::<&StudioAnim>(target).unwrap(),
            anim
        );
        assert_eq!(
            game.registry()
                .world
                .get::<&StudioGait>(target)
                .ok()
                .map(|g| (*g).clone()),
            gait
        );
        assert_eq!(game.ai_state_hash(), ai);
        let (_, systems) = game.level_and_systems_mut();
        assert_eq!(
            systems.hitboxes().entries(),
            cached.as_slice(),
            "phase5 snapshot unchanged"
        );
        assert!(
            decision.1.has_target,
            "GUARD AIM PRIMARY: a clear posed body is acquired when its eye ray misses"
        );

        let fired_before = game.weapon_fired_count();
        let hits_before = game.shot_hit_count();
        for _ in 0..240 {
            let input = guard_step(&game).0;
            game.tick(crate::TICK_SECONDS, &input);
            if game.registry().world.get::<&Actor>(target).unwrap().health < body.health {
                break;
            }
        }
        assert!(
            game.weapon_fired_count() > fired_before,
            "ordinary Guard input fires a real weapon"
        );
        assert!(
            game.shot_hit_count() > hits_before,
            "ordinary shot resolves a real entity hit"
        );
        assert!(game.registry().world.get::<&Actor>(target).unwrap().health < body.health);
    }

    #[test]
    fn guard_aim_fallback_respects_a_closed_brush() {
        let (mut game, target) = guard_aim_fixture(
            &[([-12.0, -12.0, 12.0], [12.0, 12.0, 44.0])],
            0.0,
            GuardBlocker::Door,
        );
        let body = *game.registry().world.get::<&ohl_ai::Actor>(target).unwrap();
        let entry = guard_aim_entry(&mut game, target);
        let fresh = fresh_guard_index(&mut game);
        let door = game.registry().find("aim_blocker")[0];
        assert_eq!(
            game.registry()
                .world
                .get::<&ohl_game::registry::Door>(door)
                .unwrap()
                .state,
            ohl_game::registry::MoverState::Closed
        );
        for point in [body.eye(), entry.origin + Vec3::Z * 28.0] {
            assert!(!has_line_of_sight(
                &game,
                Vec3::from_array(game.eye_position()),
                point
            ));
            assert_ne!(trace_guard_index(&game, &fresh, point), Some(target));
        }
        assert!(!guard_step(&game).1.has_target);
    }

    #[test]
    fn guard_aim_fallback_respects_the_first_friendly_hit() {
        let (mut game, target) = guard_aim_fixture(
            &[([-12.0, -12.0, 12.0], [12.0, 12.0, 44.0])],
            0.0,
            GuardBlocker::Friendly,
        );
        let body = *game.registry().world.get::<&ohl_ai::Actor>(target).unwrap();
        let entry = guard_aim_entry(&mut game, target);
        let fresh = fresh_guard_index(&mut game);
        let friendly = game.registry().find("aim_blocker")[0];
        for point in [body.eye(), entry.origin + Vec3::Z * 28.0] {
            assert!(has_line_of_sight(
                &game,
                Vec3::from_array(game.eye_position()),
                point
            ));
            assert_eq!(trace_guard_index(&game, &fresh, point), Some(friendly));
        }
        assert!(!guard_step(&game).1.has_target);
    }

    #[test]
    fn guard_aim_fallback_keeps_a_reachable_eye_first() {
        let (mut game, target) = guard_aim_fixture(
            &[([-12.0, -12.0, 12.0], [12.0, 12.0, 72.0])],
            0.0,
            GuardBlocker::None,
        );
        let body = *game.registry().world.get::<&ohl_ai::Actor>(target).unwrap();
        let centre = body.origin + Vec3::Z * 42.0;
        let fresh = fresh_guard_index(&mut game);
        assert_eq!(trace_guard_index(&game, &fresh, body.eye()), Some(target));
        assert_eq!(trace_guard_index(&game, &fresh, centre), Some(target));
        assert!(centre.distance(body.eye()) > 16.0);
        let context = game.guard_world_context();
        assert_eq!(
            nearest_visible_hostile(&game, &context, Vec3::from_array(game.eye_position())),
            Some(body.eye()),
            "successful historical eye aim retains priority"
        );
    }

    #[test]
    fn guard_aim_fallback_uses_split_rotated_boxes_not_the_union_centre() {
        let boxes = [
            ([-6.0, -40.0, 12.0], [6.0, -24.0, 44.0]),
            ([-6.0, 24.0, 12.0], [6.0, 40.0, 44.0]),
        ];
        let (mut game, target) = guard_aim_fixture(&boxes, 45.0, GuardBlocker::None);
        let body = *game.registry().world.get::<&ohl_ai::Actor>(target).unwrap();
        let entry = guard_aim_entry(&mut game, target);
        assert_eq!(entry.boxes.len(), 2);
        let rotation = glam::Quat::from_rotation_z(45.0_f32.to_radians());
        let expected = [
            body.origin + rotation * Vec3::new(0.0, -32.0, 28.0),
            body.origin + rotation * Vec3::new(0.0, 32.0, 28.0),
        ];
        let union_centre = body.origin + Vec3::Z * 28.0;
        let fresh = fresh_guard_index(&mut game);
        assert_eq!(trace_guard_index(&game, &fresh, body.eye()), None);
        assert_eq!(
            trace_guard_index(&game, &fresh, union_centre),
            None,
            "union centre is empty space"
        );
        for point in expected {
            assert!(has_line_of_sight(
                &game,
                Vec3::from_array(game.eye_position()),
                point
            ));
            assert_eq!(trace_guard_index(&game, &fresh, point), Some(target));
        }
        let context = game.guard_world_context();
        let selected =
            nearest_visible_hostile(&game, &context, Vec3::from_array(game.eye_position()))
                .expect("a clear individual box is selected");
        assert!(
            selected.distance(expected[0]) < 0.001,
            "the first local box centre is transformed once; the empty union is never aimed at"
        );
    }

    #[derive(Clone, Copy)]
    enum GuardBlocker {
        None,
        Door,
        Friendly,
    }

    // Entirely project-authored geometry, model and map logic. An ordinary
    // trigger_auto starts an instantaneous script; no actor or index is seeded.
    #[allow(clippy::too_many_lines, clippy::float_cmp)]
    fn relocated_guard_fixture(blocker: GuardBlocker) -> (Game, ohl_game::hecs::Entity, Vec3) {
        use crate::test_support::{entity_block, script_room_entities};
        use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
        let mut extra = [
            entity_block(
                "monster_human_grunt",
                [96.0, -96.0, 1.0],
                0.0,
                &[("targetname", "guard_target")],
            ),
            entity_block(
                "aiscripted_sequence",
                [96.0, 96.0, 1.0],
                0.0,
                &[
                    ("targetname", "guard_relocate"),
                    ("m_iszEntity", "guard_target"),
                    ("m_fMoveTo", "4"),
                    ("m_iszPlay", "idle"),
                ],
            ),
            entity_block(
                "trigger_auto",
                [0.0; 3],
                0.0,
                &[("target", "guard_relocate")],
            ),
        ]
        .concat();
        match blocker {
            GuardBlocker::None => {}
            GuardBlocker::Door => extra.push_str(&entity_block(
                "func_door",
                [0.0; 3],
                90.0,
                &[
                    ("targetname", "guard_blocker"),
                    ("model", "*1"),
                    ("speed", "100"),
                ],
            )),
            GuardBlocker::Friendly => extra.push_str(&entity_block(
                "monster_barney",
                [-32.0, 48.0, 1.0],
                0.0,
                &[("targetname", "guard_blocker"), ("spawnflags", "16")],
            )),
        }
        let entities = script_room_entities([-160.0, 0.0, 36.0], &extra);
        let mut builder = Bsp30Builder::new();
        builder.set_entities_text(&entities);
        let heads =
            builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
        builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], heads, 2, 0, 0);
        if matches!(blocker, GuardBlocker::Door) {
            let min = [-40.0, 0.0, 0.0];
            let max = [-24.0, 96.0, 128.0];
            let heads = builder.push_collision_hulls(&[CollisionBrush::box_brush(min, max)]);
            builder.push_model(min, max, [0.0; 3], heads, 2, 0, 0);
        }
        let (model, _) = ohl_formats::test_support::build_minimal_mdl10_with_hitbox(
            [-24.0, -24.0, 0.0],
            [24.0, 24.0, 72.0],
        );
        let mut assets = crate::MemoryAssets::new();
        assets.insert("maps/ohlguardrelocate.bsp", builder.build());
        for kind in [ohl_ai::MonsterKind::HumanGrunt, ohl_ai::MonsterKind::Barney] {
            assets.insert(kind.default_model_path().unwrap(), model.clone());
        }
        let mut game = Game::load(&assets, "ohlguardrelocate").expect("authored room");
        let target = game.registry().find("guard_target")[0];
        let mark = Vec3::new(96.0, 96.0, 1.0);
        let initial_health = game
            .registry()
            .world
            .get::<&ohl_ai::Actor>(target)
            .unwrap()
            .health;
        let player_health = game.player_health();
        for _ in 0..16 {
            let old = game
                .registry()
                .world
                .get::<&ohl_ai::Actor>(target)
                .unwrap()
                .eye();
            game.tick(crate::TICK_SECONDS, &Input::default());
            let body = *game.registry().world.get::<&ohl_ai::Actor>(target).unwrap();
            if body.origin == mark {
                assert!(body.alive && game.player_health() > 0.0);
                assert_eq!(body.health, initial_health);
                assert_eq!(game.player_health(), player_health);
                assert!(
                    game.registry()
                        .world
                        .get::<&ohl_ai::MonsterAi>(target)
                        .is_ok()
                );
                assert!(
                    game.registry()
                        .world
                        .get::<&ohl_ai::ScriptHold>(target)
                        .is_ok()
                );
                assert_eq!(
                    game.registry()
                        .world
                        .get::<&ohl_game::registry::Transform>(target)
                        .unwrap()
                        .origin,
                    mark
                );
                assert!(
                    old.distance(body.eye()) > 128.0,
                    "genuine ordinary script relocation"
                );
                assert_eq!(game.hostile_monster_eyes(), vec![(target, body.eye())]);
                return (game, target, old);
            }
        }
        panic!("ordinary trigger/script did not relocate its living hostile");
    }

    fn fresh_guard_index(game: &mut Game) -> ohl_combat::HitboxIndex {
        let (level, _) = game.level_and_systems_mut();
        let mut hitboxes = ohl_combat::HitboxIndex::default();
        crate::combat::rebuild_current_actor_hitbox_index(&mut hitboxes, level);
        assert_eq!(hitboxes.rejected(), 0);
        hitboxes
    }

    fn trace_guard_index(
        game: &Game,
        index: &ohl_combat::HitboxIndex,
        target: Vec3,
    ) -> Option<ohl_game::hecs::Entity> {
        let eye = Vec3::from_array(game.eye_position());
        let end = eye + (target - eye).normalize() * HITSCAN_RANGE;
        let filter = ohl_combat::TraceFilter::ignoring(
            ohl_combat::TraceMask::SHOT,
            crate::ids::entity_id(game.player_entity()),
        );
        ohl_combat::trace_attack_filtered(game.collision().unwrap(), index, eye, end, filter)
            .entity
            .and_then(crate::ids::entity_of)
    }

    // The physical/ordinary-input prerequisites must survive the cached-query mutant.
    #[test]
    #[allow(clippy::too_many_lines, clippy::float_cmp)]
    fn guard_sees_a_hostile_at_its_current_script_relocation() {
        use crate::components::{StudioAnim, StudioGait};
        use ohl_ai::Actor;
        let (mut game, target, old_eye) = relocated_guard_fixture(GuardBlocker::None);
        let body = *game.registry().world.get::<&Actor>(target).unwrap();
        let eye = body.eye();
        let health = body.health;
        let anim = *game.registry().world.get::<&StudioAnim>(target).unwrap();
        let gait = game
            .registry()
            .world
            .get::<&StudioGait>(target)
            .ok()
            .map(|g| (*g).clone());
        let ai_before = game.ai_state_hash();
        let cached = {
            let (_, systems) = game.level_and_systems_mut();
            systems.hitboxes().entries().to_vec()
        };
        let query = body.query_origin();
        let at = game.collision().unwrap().trace(body.hull, query, query);
        assert!(!at.start_solid && !at.all_solid);
        let floor = game
            .collision()
            .unwrap()
            .trace(body.hull, query, query - Vec3::Z * 2.0);
        assert!(!floor.start_solid && floor.fraction < 1.0 && floor.plane_normal.z >= 0.7);
        assert!(has_line_of_sight(
            &game,
            Vec3::from_array(game.eye_position()),
            eye
        ));
        assert!(Vec3::from_array(game.eye_position()).distance(eye) < GUARD_ENGAGE_RANGE);
        assert_eq!(
            game.shot_would_reach(old_eye),
            Some(target),
            "actual phase5 old ray hits"
        );
        assert_eq!(
            game.shot_would_reach(eye),
            None,
            "actual phase5 new ray misses"
        );
        let fresh = fresh_guard_index(&mut game);
        let entry = fresh
            .entries()
            .iter()
            .find(|entry| entry.id == crate::ids::entity_id(target))
            .expect("current full-generational posed target");
        assert_eq!(entry.origin, body.origin);
        assert_eq!(entry.boxes.len(), 1, "one genuine root-bone hitbox");
        let local_eye = entry.rotation.conjugate() * (eye - entry.origin);
        assert!(
            local_eye.cmpge(entry.boxes[0].min).all() && local_eye.cmple(entry.boxes[0].max).all(),
            "current eye lies in sampled pose"
        );
        assert_eq!(
            trace_guard_index(&game, &fresh, eye),
            Some(target),
            "independent current all-entry damage trace reaches this same entity"
        );

        let decision = guard_step(&game).1;
        assert_eq!(game.ai_state_hash(), ai_before, "Guard is read-only");
        assert_eq!(
            *game.registry().world.get::<&StudioAnim>(target).unwrap(),
            anim
        );
        assert_eq!(
            game.registry()
                .world
                .get::<&StudioGait>(target)
                .ok()
                .map(|g| (*g).clone()),
            gait
        );
        assert_eq!(
            game.registry().world.get::<&Actor>(target).unwrap().health,
            health
        );
        let (_, systems) = game.level_and_systems_mut();
        assert_eq!(
            systems.hitboxes().entries(),
            cached.as_slice(),
            "shared phase5 index unchanged"
        );
        assert!(
            decision.has_target,
            "GUARD PRIMARY: current eligible posed hostile is targetable"
        );
    }

    #[test]
    fn guard_current_geometry_respects_a_closed_door() {
        let (mut game, target, _) = relocated_guard_fixture(GuardBlocker::Door);
        let eye = game
            .registry()
            .world
            .get::<&ohl_ai::Actor>(target)
            .unwrap()
            .eye();
        let door = game.registry().find("guard_blocker")[0];
        assert_eq!(
            game.registry()
                .world
                .get::<&ohl_game::registry::Door>(door)
                .unwrap()
                .state,
            ohl_game::registry::MoverState::Closed
        );
        assert!(!has_line_of_sight(
            &game,
            Vec3::from_array(game.eye_position()),
            eye
        ));
        let fresh = fresh_guard_index(&mut game);
        assert_ne!(trace_guard_index(&game, &fresh, eye), Some(target));
        assert!(!guard_step(&game).1.has_target);
    }

    #[test]
    fn guard_current_geometry_respects_an_intervening_friendly() {
        let (mut game, target, _) = relocated_guard_fixture(GuardBlocker::Friendly);
        let eye = game
            .registry()
            .world
            .get::<&ohl_ai::Actor>(target)
            .unwrap()
            .eye();
        let friend = game.registry().find("guard_blocker")[0];
        assert!(has_line_of_sight(
            &game,
            Vec3::from_array(game.eye_position()),
            eye
        ));
        let fresh = fresh_guard_index(&mut game);
        assert_eq!(
            trace_guard_index(&game, &fresh, eye),
            Some(friend),
            "complete current index keeps the intervening non-hostile actor"
        );
        assert!(!guard_step(&game).1.has_target);
    }

    // Keep the corner/setup/input checks together; exact authored inputs are the oracle.
    #[allow(clippy::too_many_lines, clippy::float_cmp)]
    #[test]
    fn a_blast_corner_exit_preserves_the_aimed_combat_input() {
        use crate::save_state::FiringSnapshot;
        use crate::test_support::{
            PLAN_SCRIPTED_MAP, PLAN_SCRIPTED_MONSTER_MODEL, ScriptedStart, plan_scripted_goal_bsp,
            plan_scripted_monster_model_bytes,
        };
        use crate::{MemoryAssets, StartInventoryItem, TICK_SECONDS};

        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{PLAN_SCRIPTED_MAP}.bsp"),
            plan_scripted_goal_bsp("ohlplannext", ScriptedStart::ByHostileMonster),
        );
        assets.insert(
            PLAN_SCRIPTED_MONSTER_MODEL,
            plan_scripted_monster_model_bytes(),
        );
        let mut game = Game::load(&assets, PLAN_SCRIPTED_MAP).expect("authored corridor");
        game.give_start_inventory(&[StartInventoryItem::Weapon(WeaponId::Mp5)]);
        let mut save = game.to_save(0);
        save.view.position = [-239.0, 79.0, 36.03125];
        save.view.yaw = 0.0;
        save.view.pitch = 0.0;
        let index = WeaponId::ALL
            .iter()
            .position(|id| *id == WeaponId::Mp5)
            .unwrap();
        let selected = u8::try_from(index).expect("small weapon table");
        let inventory = save.inventory.as_mut().expect("inventory");
        inventory.weapons[index].clip = spec(WeaponId::Mp5).clip_size.unwrap();
        inventory.selected = Some(selected);
        inventory.firing = Some(FiringSnapshot {
            weapon: selected,
            state_tag: 0,
            timer: 0.0,
        });
        let mut game = Game::from_save(&assets, &save).expect("authored corner placement");
        let origin = Vec3::from_array(game.player_origin());
        assert_eq!(origin.to_array(), save.view.position);
        let health = game.player_health();
        let selected = game.inventory().selected();
        let clip = game.inventory().clip(WeaponId::Mp5);
        let hostiles: Vec<_> = game
            .hostile_monster_eyes()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        // Save loading leaves the index empty; a real phase 5 builds posed hitboxes.
        game.tick(TICK_SECONDS, &Input::default());
        assert!(
            game.player_health() == health
                && game.inventory().selected() == selected
                && game.inventory().clip(WeaponId::Mp5) == clip
                && game
                    .hostile_monster_eyes()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>()
                    == hostiles,
            "index warmup preserves health, weapon state and living hostile identities"
        );
        let origin = Vec3::from_array(game.player_origin());
        let threat = Vec3::new(-100.0, 0.0, 1.0);
        let collision = game.collision().expect("real world hulls");
        let at = collision.trace(Hull::Standing, origin, origin);
        assert!(
            !at.start_solid && !at.all_solid,
            "corner is a clear standing placement"
        );
        let away = (origin - threat).truncate().normalize();
        let yaw = away.y.atan2(away.x).to_degrees();
        for offset in GUARD_RETREAT_OFFSETS {
            let radians = (yaw + offset).to_radians();
            let heading = Vec3::new(radians.cos(), radians.sin(), 0.0);
            let trace = collision.trace(
                Hull::Standing,
                origin,
                origin + heading * GUARD_RETREAT_PROBE,
            );
            assert!(
                trace.fraction < 1.0,
                "every old away-semicircle heading hits a wall"
            );
        }
        let (combat, decision) = combat_guard_step(&game);
        assert!(
            decision.has_target && decision.firing && combat.attack,
            "loaded weapon has a real aimed target"
        );
        assert_ne!(
            combat.mouse_delta,
            (0.0, 0.0),
            "movement must use the committed turn"
        );
        let complete = complete_blast_escape_input(
            &game,
            crate::projectiles::TimedBlastThreat {
                position: threat,
                radius: 200.0,
            },
            combat,
        )
        .expect("the same corner admits a complete supported escape");
        assert_eq!(
            Input {
                forward: combat.forward,
                right: combat.right,
                ..complete
            },
            combat,
            "complete escape preserves every nonmovement combat input"
        );
        let input = blast_retreat_input(&game, threat, combat);
        assert_ne!(
            (input.forward, input.right),
            (0, 0),
            "the full input circle has a clear corner exit"
        );
        assert_eq!(
            Input {
                forward: combat.forward,
                right: combat.right,
                ..input
            },
            combat,
            "aim, attack, selection, reload and every other input remain unchanged"
        );
        let (hull, wish) = game
            .guard_movement_wish(&input)
            .expect("actual committed movement");
        assert_eq!(hull, Hull::Standing);
        assert!(
            wish.truncate().dot(away) < 0.0,
            "the clear exit is outside the old hemisphere"
        );
        let landing = origin + wish * GUARD_RETREAT_PROBE;
        let ahead = collision.trace(hull, origin, landing);
        assert!(ahead.fraction >= 1.0 && !ahead.start_solid && !ahead.all_solid);
        let down = collision.trace(hull, landing, landing - Vec3::Z * GUARD_RETREAT_MAX_DROP);
        assert!(down.fraction < 1.0 && !down.start_solid && !down.all_solid);
        game.tick(TICK_SECONDS, &input);
        let moved = Vec3::from_array(game.player_origin()) - origin;
        assert!(
            moved.truncate().dot(wish.truncate()) > 0.0,
            "the real controller moves along the checked input"
        );
    }

    // Exact restored positions and complete input/decision equality are the oracle.
    #[allow(clippy::too_many_lines, clippy::float_cmp)]
    #[test]
    fn a_blast_priority_preserves_an_existing_combat_retreat() {
        use crate::MemoryAssets;
        use crate::save_state::{
            ProjectileAttackSnapshot, ProjectileRuntimeSnapshot, ProjectileSnapshot,
        };
        use crate::test_support::{
            PLAN_SCRIPTED_MAP, PLAN_SCRIPTED_MONSTER_MODEL, ScriptedStart, plan_scripted_goal_bsp,
            plan_scripted_monster_model_bytes,
        };

        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{PLAN_SCRIPTED_MAP}.bsp"),
            plan_scripted_goal_bsp("ohlplannext", ScriptedStart::ByHostileMonster),
        );
        assets.insert(
            PLAN_SCRIPTED_MONSTER_MODEL,
            plan_scripted_monster_model_bytes(),
        );
        let game = Game::load(&assets, PLAN_SCRIPTED_MAP).expect("authored unarmed corridor");
        let mut save = game.to_save(0);
        save.view.position = [-200.0, 0.0, 36.03125];
        save.view.yaw = 180.0;
        let threat = Vec3::new(-224.0, 0.0, 1.0);
        let projectiles = save.projectiles.as_mut().expect("physics snapshot");
        projectiles.projectile_next_id = 1;
        projectiles.projectiles = vec![ProjectileSnapshot {
            id: 0,
            kind_tag: 3,
            owner: None,
            position: threat.to_array(),
            velocity: [0.0; 3],
            age: 3.0,
            fuse: Some(1.0),
            guide_point: None,
            target: None,
            attack_cooldown: 0.0,
            hop_cooldown: 0.0,
            resting: true,
        }];
        save.projectile_runtime = Some(ProjectileRuntimeSnapshot {
            attacks: vec![ProjectileAttackSnapshot {
                id: 0,
                damage: 100.0,
                damage_bits: ohl_combat::DamageType::BLAST.bits(),
                blast_radius: Some(200.0),
                owner: None,
                target: None,
            }],
            ..ProjectileRuntimeSnapshot::default()
        });
        let mut game = Game::from_save(&assets, &save).expect("authored competing threats");
        assert_eq!(game.player_origin(), save.view.position);
        let health = game.player_health();
        let selected = game.inventory().selected();
        let clip = game.inventory().clip(WeaponId::Mp5);
        let hostiles: Vec<_> = game
            .hostile_monster_eyes()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        // Exercise the real phase-5 path before asking whether a shot can reach.
        game.tick(crate::TICK_SECONDS, &Input::default());
        assert!(
            game.player_health() == health
                && game.inventory().selected() == selected
                && game.inventory().clip(WeaponId::Mp5) == clip
                && game
                    .hostile_monster_eyes()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>()
                    == hostiles,
            "index warmup preserves health, weapon state and living hostile identities"
        );
        assert_eq!(
            game.timed_blast_threat(TIMED_BLAST_RETREAT_SECONDS),
            Some(crate::projectiles::TimedBlastThreat {
                position: threat,
                radius: 200.0,
            }),
            "the timed threat is admissible, not filtered out by setup"
        );
        let expected = combat_guard_step(&game);
        assert!(
            expected.1.has_target && expected.1.retreating,
            "the original unarmed retreat is already active"
        );
        assert_ne!(
            blast_retreat_input(&game, threat, expected.0),
            expected.0,
            "the competing movement policy would change the existing escape"
        );
        assert_eq!(
            guard_step(&game),
            expected,
            "an existing combat retreat retains its complete input and decision"
        );
    }

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
