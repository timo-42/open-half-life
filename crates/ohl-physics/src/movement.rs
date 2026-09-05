//! The player movement step.
//!
//! One call to [`player_move`] advances a [`PlayerState`] by one fixed
//! timestep: it categorizes the player's position (on ground, in water),
//! applies friction and acceleration toward the wished-for direction, adds
//! gravity, and slides the resulting velocity through the world with
//! [`crate::hull`] traces, stepping up over small obstructions.
//!
//! Every tunable lives in [`MoveConfig`]. The defaults are the values that
//! public community documentation records for Half-Life's default cvars
//! (`sv_gravity 800`, `sv_friction 4`, `sv_stopspeed 100`, `sv_accelerate
//! 10`, `sv_airaccelerate 10`, `sv_maxspeed 320`, `sv_stepsize 18`, an air
//! speed cap of 30 units/s, and a jump impulse of `sqrt(2 * 800 * 45)`
//! ≈ 268.3 that produces the documented 45-unit jump); see
//! `docs/FORMAT_SOURCES.md`. They are configuration, not measurements: each
//! one still has to be verified against the real game before this crate can
//! claim behavioural parity.

use glam::Vec3;

use crate::hull::{CollisionModel, Hull, Trace, contents};

/// How deep in a liquid the player is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaterLevel {
    /// Not touching a liquid.
    #[default]
    Dry,
    /// Feet in a liquid.
    Feet,
    /// At least waist-deep: swimming rules apply.
    Waist,
    /// Fully submerged.
    Eyes,
}

/// The tunable constants of one movement step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveConfig {
    /// Maximum ground speed, units per second.
    pub max_speed: f32,
    /// Downward acceleration, units per second squared.
    pub gravity: f32,
    /// Ground acceleration coefficient.
    pub accelerate: f32,
    /// Air acceleration coefficient.
    pub air_accelerate: f32,
    /// The largest speed component along the wish direction that air
    /// acceleration may add to, which is what makes air strafing work.
    pub air_speed_cap: f32,
    /// Ground friction coefficient.
    pub friction: f32,
    /// Extra friction multiplier applied when standing near a ledge.
    pub edge_friction: f32,
    /// Speed below which friction is applied as if the player were moving
    /// at this speed, so slow movement stops promptly.
    pub stop_speed: f32,
    /// Upward velocity applied by a jump.
    pub jump_velocity: f32,
    /// The tallest obstruction the player walks up without jumping.
    pub step_size: f32,
    /// Upward speed above which the player is treated as having left the
    /// ground regardless of what is under them, so a jump is not cancelled
    /// on the tick it starts.
    pub leave_ground_speed: f32,
    /// The smallest ground-plane `z` normal component the player can stand
    /// on; steeper surfaces are slopes they slide down.
    pub slope_limit: f32,
    /// Overbounce factor used when clipping velocity into a plane. `1.0`
    /// slides exactly along the surface.
    pub overbounce: f32,
    /// How many times one move re-clips its velocity and tries again.
    pub max_bumps: u32,
    /// How many distinct planes one move remembers while sliding.
    pub max_clip_planes: usize,
    /// Absolute per-axis speed clamp (`sv_maxvelocity`).
    pub max_velocity: f32,
    /// Fraction of [`Self::max_speed`] available while ducked.
    pub duck_speed_fraction: f32,
    /// Friction coefficient while swimming.
    pub water_friction: f32,
    /// Acceleration coefficient while swimming.
    pub water_accelerate: f32,
    /// Fraction of [`Self::max_speed`] available while swimming.
    pub water_speed_fraction: f32,
    /// Speed of the noclip camera.
    pub noclip_speed: f32,
    /// Eye height above the origin while standing.
    pub view_height_standing: f32,
    /// Eye height above the origin while ducked.
    pub view_height_ducked: f32,
}

impl Default for MoveConfig {
    fn default() -> Self {
        Self {
            max_speed: 320.0,
            gravity: 800.0,
            accelerate: 10.0,
            air_accelerate: 10.0,
            air_speed_cap: 30.0,
            friction: 4.0,
            edge_friction: 2.0,
            stop_speed: 100.0,
            // sqrt(2 * 800 * 45): the impulse that reaches the documented
            // 45-unit standing jump under 800 units/s^2 of gravity.
            jump_velocity: 268.328_16,
            step_size: 18.0,
            leave_ground_speed: 180.0,
            slope_limit: 0.7,
            overbounce: 1.0,
            max_bumps: 4,
            max_clip_planes: 5,
            max_velocity: 2000.0,
            duck_speed_fraction: 0.333,
            water_friction: 1.0,
            water_accelerate: 10.0,
            water_speed_fraction: 0.8,
            noclip_speed: 320.0,
            view_height_standing: 28.0,
            view_height_ducked: 12.0,
        }
    }
}

/// What the player is asking to do this step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MoveInput {
    /// The world-space direction the player wants to move in. Its length
    /// scales the wished-for speed (`1.0` meaning full speed); the `z`
    /// component is only used while swimming or in noclip.
    pub wish_move: Vec3,
    /// Whether the jump key is held.
    pub jump: bool,
    /// Whether the duck key is held.
    pub duck: bool,
}

/// Everything the movement step reads and writes about one player.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct PlayerState {
    /// The entity origin (the centre of the hull horizontally, and 36 units
    /// above the floor while standing).
    pub origin: Vec3,
    /// Velocity in units per second.
    pub velocity: Vec3,
    /// Whether the player is crouched, which selects the smaller hull.
    pub ducked: bool,
    /// Whether the player is standing on a walkable surface.
    pub on_ground: bool,
    /// The normal of the surface being stood on; zero when airborne.
    pub ground_normal: Vec3,
    /// How deep the player is in a liquid.
    pub water_level: WaterLevel,
    /// Whether collision is disabled.
    pub noclip: bool,
    /// Whether a jump has been held since it last took effect, so holding
    /// the key does not auto-repeat.
    pub jump_held: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            ducked: false,
            on_ground: false,
            ground_normal: Vec3::ZERO,
            water_level: WaterLevel::Dry,
            noclip: false,
            jump_held: false,
        }
    }
}

impl PlayerState {
    /// A player standing at `origin`.
    #[must_use]
    pub fn at(origin: Vec3) -> Self {
        Self {
            origin,
            ..Self::default()
        }
    }

    /// The hull this player currently collides with.
    #[must_use]
    pub fn hull(&self) -> Hull {
        if self.ducked {
            Hull::Crouched
        } else {
            Hull::Standing
        }
    }

    /// The eye position for the current stance.
    #[must_use]
    pub fn eye_position(&self, config: &MoveConfig) -> Vec3 {
        let height = if self.ducked {
            config.view_height_ducked
        } else {
            config.view_height_standing
        };
        self.origin + Vec3::Z * height
    }

    /// Whether swimming rules apply.
    #[must_use]
    pub fn is_swimming(&self) -> bool {
        matches!(self.water_level, WaterLevel::Waist | WaterLevel::Eyes)
    }
}

/// Removes the component of `velocity` that points into a surface with
/// normal `normal`. `overbounce` of `1.0` leaves a pure slide.
#[must_use]
pub fn clip_velocity(velocity: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = velocity.dot(normal) * overbounce;
    let clipped = velocity - normal * backoff;
    // Snap tiny residuals to zero so repeated clipping cannot creep into a
    // surface.
    Vec3::new(
        snap_to_zero(clipped.x),
        snap_to_zero(clipped.y),
        snap_to_zero(clipped.z),
    )
}

fn snap_to_zero(value: f32) -> f32 {
    if value > -0.1 && value < 0.1 {
        0.0
    } else {
        value
    }
}

fn horizontal(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, 0.0)
}

/// Accelerates `velocity` toward `wish_dir` up to `wish_speed`, adding at
/// most `accel * wish_speed * dt` this step (the standard ground rule).
#[must_use]
pub fn accelerate(velocity: Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) -> Vec3 {
    let current = velocity.dot(wish_dir);
    let add_speed = wish_speed - current;
    if add_speed <= 0.0 {
        return velocity;
    }
    let accel_speed = (accel * wish_speed * dt).min(add_speed);
    velocity + wish_dir * accel_speed
}

/// Accelerates `velocity` toward `wish_dir` while airborne. The wished-for
/// speed used for the "how much is already there" test is capped, which is
/// what allows air strafing to keep adding speed sideways.
#[must_use]
pub fn air_accelerate(
    velocity: Vec3,
    wish_dir: Vec3,
    wish_speed: f32,
    config: &MoveConfig,
    dt: f32,
) -> Vec3 {
    let capped = wish_speed.min(config.air_speed_cap);
    let current = velocity.dot(wish_dir);
    let add_speed = capped - current;
    if add_speed <= 0.0 {
        return velocity;
    }
    let accel_speed = (config.air_accelerate * wish_speed * dt).min(add_speed);
    velocity + wish_dir * accel_speed
}

/// Applies ground (or water) friction to `state.velocity`.
fn apply_friction(model: &CollisionModel, state: &mut PlayerState, config: &MoveConfig, dt: f32) {
    let speed = state.velocity.length();
    if speed < 0.1 {
        state.velocity = Vec3::ZERO;
        return;
    }

    let mut drop = 0.0;
    if state.on_ground && !state.is_swimming() {
        let mut friction = config.friction;
        if is_near_ledge(model, state, config) {
            friction *= config.edge_friction;
        }
        let control = speed.max(config.stop_speed);
        drop += control * friction * dt;
    } else if state.is_swimming() {
        drop += speed * config.water_friction * dt;
    }

    let new_speed = (speed - drop).max(0.0);
    state.velocity *= new_speed / speed;
}

/// Whether the player is standing close enough to a drop for the extra
/// edge-friction multiplier to apply: the check looks a short way ahead of
/// the player along their velocity and traces down further than a step.
fn is_near_ledge(model: &CollisionModel, state: &PlayerState, config: &MoveConfig) -> bool {
    let horizontal_velocity = horizontal(state.velocity);
    if horizontal_velocity.length_squared() <= 0.0 {
        return false;
    }
    let ahead = state.origin + horizontal_velocity.normalize() * 16.0;
    let foot = ahead - Vec3::Z * state.hull().foot_offset();
    let below = foot - Vec3::Z * (config.step_size + 16.0);
    let trace = model.trace(Hull::Point, foot, below);
    !trace.blocked()
}

/// Refreshes [`PlayerState::on_ground`], [`PlayerState::ground_normal`] and
/// [`PlayerState::water_level`].
pub fn categorize_position(model: &CollisionModel, state: &mut PlayerState, config: &MoveConfig) {
    state.water_level = water_level(model, state, config);

    // Moving up fast enough means the player has definitely left the ground
    // (this is what stops a jump from being cancelled on its first step).
    if state.velocity.z > config.leave_ground_speed {
        state.on_ground = false;
        state.ground_normal = Vec3::ZERO;
        return;
    }

    let below = state.origin - Vec3::Z * 2.0;
    let trace = model.trace(state.hull(), state.origin, below);
    if trace.fraction < 1.0 && !trace.all_solid && trace.plane_normal.z >= config.slope_limit {
        state.on_ground = true;
        state.ground_normal = trace.plane_normal;
        if !trace.start_solid {
            state.origin = trace.end_pos;
        }
        // Standing on ground cancels any residual downward velocity.
        if state.velocity.z < 0.0 {
            state.velocity.z = 0.0;
        }
    } else {
        state.on_ground = false;
        state.ground_normal = Vec3::ZERO;
    }
}

fn water_level(model: &CollisionModel, state: &PlayerState, config: &MoveConfig) -> WaterLevel {
    let foot_offset = state.hull().foot_offset();
    let feet = state.origin - Vec3::Z * (foot_offset - 1.0);
    if !contents::is_liquid(model.point_contents(feet)) {
        return WaterLevel::Dry;
    }
    if contents::is_liquid(model.point_contents(state.eye_position(config))) {
        return WaterLevel::Eyes;
    }
    if contents::is_liquid(model.point_contents(state.origin)) {
        return WaterLevel::Waist;
    }
    WaterLevel::Feet
}

/// Slides `state` through the world for `dt` seconds, clipping its velocity
/// against every plane it runs into. Returns the planes that stopped it.
fn slide_move(model: &CollisionModel, state: &mut PlayerState, config: &MoveConfig, dt: f32) {
    let hull = state.hull();
    let primal_velocity = state.velocity;
    let mut planes: [Vec3; 8] = [Vec3::ZERO; 8];
    let mut plane_count = 0usize;
    let plane_capacity = config.max_clip_planes.min(planes.len());
    let mut time_left = dt;

    for _ in 0..config.max_bumps {
        if state.velocity.length_squared() <= 0.0 {
            break;
        }
        let end = state.origin + state.velocity * time_left;
        let trace = model.trace(hull, state.origin, end);

        if trace.all_solid {
            // Stuck: refuse to move rather than tunnel out of the world.
            state.velocity = Vec3::ZERO;
            return;
        }
        if trace.fraction > 0.0 {
            state.origin = trace.end_pos;
            plane_count = 0;
        }
        if trace.fraction >= 1.0 {
            return;
        }

        time_left -= time_left * trace.fraction;
        if plane_count >= plane_capacity {
            // More planes than the move can reason about: stop dead, which
            // is the documented behaviour for a pinched corner.
            state.velocity = Vec3::ZERO;
            return;
        }
        planes[plane_count] = trace.plane_normal;
        plane_count += 1;

        // First try sliding along each plane in turn; accept the first
        // result that does not push back into any of the others.
        let mut resolved = None;
        for i in 0..plane_count {
            let candidate = clip_velocity(primal_velocity, planes[i], config.overbounce);
            if (0..plane_count)
                .filter(|j| *j != i)
                .all(|j| candidate.dot(planes[j]) >= 0.0)
            {
                resolved = Some(candidate);
                break;
            }
        }
        state.velocity = match resolved {
            Some(velocity) => velocity,
            None if plane_count == 2 => {
                // A crease: slide along the line where the two planes meet.
                let direction = planes[0].cross(planes[1]);
                direction * direction.dot(state.velocity)
            }
            None => {
                state.velocity = Vec3::ZERO;
                return;
            }
        };

        if state.velocity.dot(primal_velocity) <= 0.0 {
            // Turned back on itself; stop instead of oscillating.
            state.velocity = Vec3::ZERO;
            return;
        }
    }
}

/// Moves the player, first straight ahead and then again over a step of up
/// to [`MoveConfig::step_size`], keeping whichever attempt travelled
/// further horizontally.
fn step_move(model: &CollisionModel, state: &mut PlayerState, config: &MoveConfig, dt: f32) {
    let hull = state.hull();
    let start = *state;

    // Attempt 1: the plain slide move.
    let mut flat = *state;
    slide_move(model, &mut flat, config, dt);

    if !start.on_ground {
        *state = flat;
        return;
    }

    // Attempt 2: step up, move, then drop back down.
    let mut stepped = start;
    let up = stepped.origin + Vec3::Z * config.step_size;
    let up_trace = model.trace(hull, stepped.origin, up);
    if up_trace.start_solid || up_trace.all_solid {
        *state = flat;
        return;
    }
    stepped.origin = up_trace.end_pos;
    slide_move(model, &mut stepped, config, dt);

    let down = stepped.origin - Vec3::Z * config.step_size;
    let down_trace = model.trace(hull, stepped.origin, down);
    if down_trace.plane_normal.z < config.slope_limit && down_trace.fraction < 1.0 {
        // Landed on something too steep to stand on: the plain move wins.
        *state = flat;
        return;
    }
    if !down_trace.start_solid {
        stepped.origin = down_trace.end_pos;
    }

    let flat_distance = horizontal(flat.origin - start.origin).length_squared();
    let stepped_distance = horizontal(stepped.origin - start.origin).length_squared();
    if stepped_distance > flat_distance {
        // Keep the horizontal velocity the step move ended with, but let the
        // vertical component follow the flat move so stepping up does not add
        // upward speed.
        stepped.velocity.z = flat.velocity.z;
        *state = stepped;
    } else {
        *state = flat;
    }
}

fn wish_direction(input: &MoveInput) -> (Vec3, f32) {
    let length = input.wish_move.length();
    if length <= 0.0 {
        (Vec3::ZERO, 0.0)
    } else {
        (input.wish_move / length, length.min(1.0))
    }
}

fn ground_max_speed(state: &PlayerState, config: &MoveConfig) -> f32 {
    if state.ducked {
        config.max_speed * config.duck_speed_fraction
    } else {
        config.max_speed
    }
}

/// Applies the duck/unduck request, moving the origin so the player's feet
/// stay put while their standing height changes.
fn apply_duck(model: &CollisionModel, state: &mut PlayerState, input: &MoveInput) {
    let standing_foot = Hull::Standing.foot_offset();
    let ducked_foot = Hull::Crouched.foot_offset();
    let difference = standing_foot - ducked_foot;

    if input.duck && !state.ducked {
        state.ducked = true;
        if state.on_ground {
            state.origin.z -= difference;
        }
    } else if !input.duck && state.ducked {
        // Only stand up where there is room for the taller hull.
        let target = if state.on_ground {
            state.origin + Vec3::Z * difference
        } else {
            state.origin
        };
        let trace = model.trace(Hull::Standing, target, target);
        if !trace.start_solid && !trace.all_solid {
            state.ducked = false;
            state.origin = target;
        }
    }
}

/// Advances `state` by one fixed timestep of `dt` seconds.
pub fn player_move(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    dt: f32,
) {
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let (wish_dir, wish_scale) = wish_direction(input);

    if state.noclip {
        state.ducked = input.duck;
        state.on_ground = false;
        state.ground_normal = Vec3::ZERO;
        state.velocity = wish_dir * (config.noclip_speed * wish_scale);
        state.origin += state.velocity * dt;
        state.water_level = WaterLevel::Dry;
        return;
    }

    apply_duck(model, state, input);
    categorize_position(model, state, config);
    apply_friction(model, state, config, dt);

    if state.is_swimming() {
        water_move(model, state, input, config, wish_dir, wish_scale, dt);
    } else {
        walk_or_air_move(model, state, input, config, wish_dir, wish_scale, dt);
    }

    clamp_velocity(state, config);
    categorize_position(model, state, config);
}

fn clamp_velocity(state: &mut PlayerState, config: &MoveConfig) {
    if !state.velocity.is_finite() {
        state.velocity = Vec3::ZERO;
        return;
    }
    state.velocity = state.velocity.clamp(
        Vec3::splat(-config.max_velocity),
        Vec3::splat(config.max_velocity),
    );
}

#[allow(clippy::too_many_arguments)]
fn walk_or_air_move(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    wish_dir: Vec3,
    wish_scale: f32,
    dt: f32,
) {
    let wish_speed = ground_max_speed(state, config) * wish_scale;

    if input.jump {
        if state.on_ground && !state.jump_held {
            state.velocity.z = config.jump_velocity;
            state.on_ground = false;
            state.ground_normal = Vec3::ZERO;
        }
        state.jump_held = true;
    } else {
        state.jump_held = false;
    }

    if state.on_ground {
        // Walking accelerates horizontally; slopes and steps are climbed by
        // the move itself (the slide clips the velocity into the surface and
        // the step move lifts over small obstructions), so the stored
        // velocity stays flat while the player is on the ground.
        state.velocity.z = 0.0;
        state.velocity = accelerate(state.velocity, wish_dir, wish_speed, config.accelerate, dt);
        step_move(model, state, config, dt);
        state.velocity.z = 0.0;
    } else {
        // Half the gravity before the move and half after, so the height a
        // jump reaches does not depend on the timestep.
        state.velocity.z -= config.gravity * dt * 0.5;
        state.velocity = air_accelerate(state.velocity, wish_dir, wish_speed, config, dt);
        step_move(model, state, config, dt);
        state.velocity.z -= config.gravity * dt * 0.5;
    }
}

/// A deliberately simple swimming mode: friction has already been applied,
/// so this accelerates toward the (three-dimensional) wish direction, sinks
/// slowly when idle, and slides like any other move.
#[allow(clippy::too_many_arguments)]
fn water_move(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    wish_dir: Vec3,
    wish_scale: f32,
    dt: f32,
) {
    // Holding jump is a swim-up wish in its own right, so it sets the
    // wished-for speed even when no direction key is held.
    let scale = if input.jump {
        wish_scale.max(1.0)
    } else {
        wish_scale
    };
    let wish_speed = config.max_speed * config.water_speed_fraction * scale;
    let mut wish_dir = wish_dir;
    if input.jump {
        // Holding jump swims upward.
        wish_dir = (wish_dir + Vec3::Z).normalize_or_zero();
    } else if wish_dir.length_squared() <= 0.0 {
        // Idle in water: sink gently.
        state.velocity.z -= config.gravity * 0.1 * dt;
    }
    state.velocity = accelerate(
        state.velocity,
        wish_dir,
        wish_speed,
        config.water_accelerate,
        dt,
    );
    slide_move(model, state, config, dt);
}

/// Traces the player's current hull straight down by `distance`, the query
/// hosts use to place a foot marker or test for ground.
#[must_use]
pub fn trace_ground(model: &CollisionModel, state: &PlayerState, distance: f32) -> Trace {
    model.trace(
        state.hull(),
        state.origin,
        state.origin - Vec3::Z * distance,
    )
}
