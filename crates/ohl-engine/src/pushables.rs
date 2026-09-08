//! Pushing a `func_pushable` around (M9.10, `docs/FORMAT_SOURCES.md`, item
//! 30).
//!
//! TWHL wiki `func_pushable` (`https://twhl.info/wiki/page/func_pushable`,
//! fetched directly, HTTP 200, reviewed 2026-09-08) documents this entity as
//! "the only type of brush entity that can be pushed, pulled, lifted (not by
//! the player) and fall", with a `friction` keyvalue that "determines the
//! amount of resistance the brush will give when the player pushes it. Range
//! is 0 to 400, where 400 is the most resistance". It states neither the
//! speed a pushed brush travels at, nor the law mapping `friction` onto that
//! speed, nor how contact is detected — so what this module implements
//! there is **this project's own behaviour, recorded in
//! `docs/FORMAT_SOURCES.md` item 32** rather than a claim about the original
//! engine.
//!
//! This lives in `ohl-engine` rather than `ohl-game` because pushing needs
//! two things only this crate can see: the player (`ohl_physics::
//! PlayerController`, not a `hecs` entity) and the live
//! [`ohl_physics::CollisionModel`] every solid brush is attached to. The
//! push itself is written back into `ohl_game::registry::Pushable::offset`,
//! which `ohl_game::pose::brush_offset` already folds into the one
//! displacement the renderer, the collision sync
//! ([`crate::Level::sync_brush_collision`]) and the `use`-proximity search
//! all read — so a pushed crate stays in agreement everywhere, with no
//! second placement path.

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::registry::{Breakable, BrushBounds, PUSHABLE_MAX_FRICTION, Pushable};
use ohl_physics::{BrushId, CollisionModel, ControllerInput, Hull, PlayerController};

use crate::level::Level;

/// The fastest this project ever moves a pushed brush, in units per second
/// — the speed a `friction = 0` brush reaches when the player walks
/// straight into it at full speed. Project-chosen (the cited page states no
/// speed at all): the published walking speed a player pushes with is
/// itself the input, and this only caps the result so a brush can never
/// outrun the trace that keeps it out of walls.
pub const MAX_PUSH_SPEED: f32 = 200.0;

/// How far outside a pushable's own box the player's box may sit and still
/// count as pushing it, in units. Project-chosen: contact has to be
/// slightly generous, because the player's own move stops
/// `ohl_physics::DIST_EPSILON` short of whatever it collides with, so an
/// exact box-touching test would never fire.
pub const CONTACT_SLOP: f32 = 2.0;

/// Moves every `func_pushable` the player is currently pushing against.
///
/// Runs once per fixed step, after the player has moved (so "is the player
/// pushing into it" is asked of the position the player actually reached)
/// and before the next step's [`crate::Level::sync_brush_collision`] moves
/// the brush's own hull to match.
///
/// The rule, all of it this project's own (see this module's doc comment):
///
/// * **Contact.** The player's own hull box, grown by [`CONTACT_SLOP`], must
///   overlap the pushable's current box (its spawn-time [`BrushBounds`] plus
///   its accumulated [`Pushable::offset`]).
/// * **Direction.** The player's own movement *wish*
///   ([`PlayerController::wish_move`], the horizontal direction their keys
///   and view yaw ask for), with the player required to be pressing *toward*
///   the brush (positive dot product with the horizontal vector from the
///   player to the brush's centre). The wish, not the resulting velocity:
///   a player leaning on a crate is stopped dead by the crate's own solid
///   hull, so their velocity along the push axis is already clipped to zero
///   by the time this phase runs. This is also what makes the push "away
///   from the player" by construction, so a pushed brush can never be
///   driven *into* the player and leave them inside solid.
/// * **Speed.** The player's own configured ground speed
///   ([`ohl_physics::MoveConfig::max_speed`], reduced by the documented
///   ducking fraction while crouched), scaled by the documented `friction`
///   resistance (`1 - friction / 400`, so the documented `400` "most
///   resistance" end refuses to move at all and `0` keeps pace with the
///   player), capped at [`MAX_PUSH_SPEED`].
/// * **Blocking.** The move is traced through the collision model with the
///   pushable's own brush ignored ([`CollisionModel::trace_ignoring`]) and
///   stops at whatever it hits, so a crate cannot be shoved through a wall
///   or through another brush entity.
///
/// **Documented gaps** (`docs/FORMAT_SOURCES.md`, item 32): the cited
/// "pulled, lifted... and fall" behaviours are not implemented — this
/// project's pushables never fall under gravity, are never pulled back
/// toward the player, and ignore the documented `bouyancy` keyvalue. A
/// broken pushable is skipped entirely (it is gone from the world).
pub(crate) fn push_pushables(
    level: &mut Level,
    controller: &PlayerController,
    input: ControllerInput,
    dt: f32,
) {
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let Some(collision) = level.collision.as_ref() else {
        return;
    };
    let player_origin = controller.state.origin;
    let wish = controller.wish_move(&input);
    let direction = Vec3::new(wish.x, wish.y, 0.0).normalize_or_zero();
    if direction == Vec3::ZERO || !player_origin.is_finite() {
        return;
    }
    let speed = if controller.state.ducked {
        controller.config.max_speed * controller.config.duck_speed_fraction
    } else {
        controller.config.max_speed
    };
    if !speed.is_finite() || speed <= 0.0 {
        return;
    }
    let (hull_mins, hull_maxs) = controller.state.hull().bounds();
    let player_mins = player_origin + hull_mins - Vec3::splat(CONTACT_SLOP);
    let player_maxs = player_origin + hull_maxs + Vec3::splat(CONTACT_SLOP);

    // Collected first (and sorted, so a step touching several pushables is
    // deterministic) because the write-back below needs `&mut` on the same
    // world this query borrows.
    let mut candidates: Vec<(Entity, Vec3, Vec3, f32)> = level
        .registry
        .world
        .query::<(Entity, &Pushable, &BrushBounds, Option<&Breakable>)>()
        .iter()
        .filter(|(_, _, _, breakable)| !breakable.is_some_and(|breakable| breakable.broken))
        .map(|(entity, pushable, bounds, _)| {
            (
                entity,
                bounds.mins + pushable.offset,
                bounds.maxs + pushable.offset,
                pushable.friction,
            )
        })
        .filter(|(_, mins, maxs, _)| {
            player_mins.cmple(*maxs).all() && player_maxs.cmpge(*mins).all()
        })
        .collect();
    candidates.sort_unstable_by_key(|(entity, _, _, _)| entity.id());

    // Every move is computed first, against the collision model borrowed
    // immutably, and only then written back: the write needs `&mut Level`,
    // which cannot coexist with that borrow.
    let mut moves: Vec<(Entity, Vec3)> = Vec::new();
    for (entity, mins, maxs, friction) in candidates {
        let centre = mins.midpoint(maxs);
        let toward = Vec3::new(centre.x - player_origin.x, centre.y - player_origin.y, 0.0);
        if direction.dot(toward) <= 0.0 {
            // The player is moving away from (or alongside) the brush, not
            // into it: nothing pushes.
            continue;
        }
        let resistance = 1.0 - (friction / PUSHABLE_MAX_FRICTION).clamp(0.0, 1.0);
        let push_speed = (speed * resistance).min(MAX_PUSH_SPEED);
        if push_speed <= f32::EPSILON {
            continue;
        }
        let step = direction * push_speed * dt;
        // A pushable on a map with no usable hulls has no attached brush at
        // all; nothing can block it there, so it simply moves.
        let displacement = match brush_of(level, entity) {
            Some(brush) => {
                // The trace's own query point is where *that hull's* origin
                // would sit for this brush: horizontally its centre, and
                // vertically its own floor plus the hull's documented foot
                // offset, since a collision hull's clip tree is expanded
                // about an origin that height above the ground (see
                // `ohl_physics::Hull::foot_offset`). Handing it the box's
                // geometric centre instead reports a crate standing on the
                // floor as embedded in it.
                let hull = Hull::for_size(mins, maxs);
                let query = Vec3::new(centre.x, centre.y, mins.z + hull.foot_offset());
                traced_step(collision, hull, query, step, brush)
            }
            None => step,
        };
        if displacement.length_squared() <= f32::EPSILON {
            continue;
        }
        moves.push((entity, displacement));
    }
    for (entity, step) in moves {
        apply_push(level, entity, step);
    }
}

/// How far the brush actually gets: `step`, trimmed by the first solid it
/// runs into with its own hull ignored. A move that starts inside solid
/// (a crate a mapper embedded in a wall) reports zero rather than teleporting
/// it out.
fn traced_step(
    collision: &CollisionModel,
    hull: Hull,
    query: Vec3,
    step: Vec3,
    brush: BrushId,
) -> Vec3 {
    let trace = collision.trace_ignoring(hull, query, query + step, Some(brush));
    if trace.start_solid {
        return Vec3::ZERO;
    }
    step * trace.fraction.clamp(0.0, 1.0)
}

/// The attached collision brush belonging to `entity`, when it has one.
fn brush_of(level: &Level, entity: Entity) -> Option<BrushId> {
    level
        .brush_collision
        .iter()
        .find(|(attached, _)| *attached == entity)
        .map(|(_, brush)| *brush)
}

/// Adds `step` to this entity's accumulated [`Pushable::offset`]; the next
/// [`crate::Level::sync_brush_collision`] is what moves its hull to match.
fn apply_push(level: &mut Level, entity: Entity, step: Vec3) {
    if let Ok(mut pushable) = level.registry.world.get::<&mut Pushable>(entity) {
        pushable.offset += step;
    }
}
