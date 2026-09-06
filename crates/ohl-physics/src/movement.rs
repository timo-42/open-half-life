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

impl WaterLevel {
    /// The `0..=3` waterlevel index the Half-Life community documentation
    /// (and this project's HUD/save code) uses for the same four states.
    #[must_use]
    pub const fn as_index(self) -> u8 {
        match self {
            Self::Dry => 0,
            Self::Feet => 1,
            Self::Waist => 2,
            Self::Eyes => 3,
        }
    }

    /// The level `index` names, saturating at [`Self::Eyes`].
    #[must_use]
    pub const fn from_index(index: u8) -> Self {
        match index {
            0 => Self::Dry,
            1 => Self::Feet,
            2 => Self::Waist,
            _ => Self::Eyes,
        }
    }
}

/// Which liquid the player is standing in, taken from the BSP leaf contents
/// at the player's position. The three swimmable contents values are the
/// documented Quake/GoldSrc `CONTENTS_WATER`, `CONTENTS_SLIME` and
/// `CONTENTS_LAVA` (see [`crate::hull::contents`]); what *damage* slime and
/// lava do is not a movement concern and lives in `ohl-player`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiquidKind {
    /// Not in a liquid.
    #[default]
    None,
    /// `CONTENTS_WATER`.
    Water,
    /// `CONTENTS_SLIME`.
    Slime,
    /// `CONTENTS_LAVA`.
    Lava,
}

impl LiquidKind {
    /// Classifies a raw contents value; anything that is not one of the
    /// three liquids is [`Self::None`].
    #[must_use]
    pub const fn from_contents(value: i32) -> Self {
        match value {
            contents::WATER => Self::Water,
            contents::SLIME => Self::Slime,
            contents::LAVA => Self::Lava,
            _ => Self::None,
        }
    }
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
    /// Climbing speed inside a `func_ladder` volume, units per second.
    ///
    /// `TODO(black-box)`: no published figure was found for the GoldSrc
    /// ladder climb speed. This is a neutral placeholder (half the default
    /// ground speed) and must be measured against the retail game before
    /// this crate may claim parity.
    pub ladder_speed: f32,
    /// Speed of the push away from a ladder when the player jumps off it.
    ///
    /// `TODO(black-box)`: neutral placeholder, same magnitude as the jump
    /// impulse, chosen only so the player clears the ladder volume.
    pub ladder_detach_speed: f32,
    /// Seconds after jumping off a ladder during which the player cannot
    /// re-attach to one, so the jump actually leaves the volume.
    ///
    /// `TODO(black-box)`: neutral placeholder.
    pub ladder_reattach_delay: f32,
    /// Horizontal speed of a long jump (`item_longjump`).
    ///
    /// `TODO(black-box)`: the long-jump impulse is not published. Neutral
    /// placeholder: 1.6x the default ground speed.
    pub long_jump_forward_speed: f32,
    /// Upward speed of a long jump.
    ///
    /// `TODO(black-box)`: neutral placeholder, a little over half the
    /// standing jump impulse so the arc is long and flat.
    pub long_jump_up_speed: f32,
    /// How long after the duck key goes down a jump still counts as a long
    /// jump. The public description of the move is that it only fires while
    /// the player is *still crouching down*, not once fully crouched (see
    /// `docs/FORMAT_SOURCES.md`, "Player systems"), so this window stands in
    /// for the duck animation's length.
    ///
    /// `TODO(black-box)`: neutral placeholder.
    pub long_jump_duck_window: f32,
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
            ladder_speed: 160.0,
            ladder_detach_speed: 270.0,
            ladder_reattach_delay: 0.3,
            long_jump_forward_speed: 512.0,
            long_jump_up_speed: 180.0,
            long_jump_duck_window: 0.4,
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
    /// The velocity of whatever the player is standing on or being pushed
    /// by (a `func_plat`/`func_train` underfoot, or a conveyor). It is
    /// added to the player's velocity for the duration of the move and
    /// removed again afterwards, so riding a mover carries the player
    /// without permanently changing their own velocity.
    pub base_velocity: Vec3,
    /// Whether the player owns the long jump module (`item_longjump`), so
    /// a duck-then-jump inside [`MoveConfig::long_jump_duck_window`]
    /// produces the long jump impulse instead of a crouch jump.
    pub long_jump: bool,
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
    /// Which liquid the player is in, if any.
    pub liquid: LiquidKind,
    /// Whether the player is currently attached to a `func_ladder` volume.
    pub on_ladder: bool,
    /// Seconds remaining before the player may attach to a ladder again
    /// after jumping off one.
    pub ladder_lockout: f32,
    /// How long the duck key has been held, in seconds; used for the
    /// long-jump window.
    pub duck_seconds: f32,
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
            liquid: LiquidKind::None,
            on_ladder: false,
            ladder_lockout: 0.0,
            duck_seconds: 0.0,
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

/// Refreshes [`PlayerState::on_ground`], [`PlayerState::ground_normal`],
/// [`PlayerState::water_level`] and [`PlayerState::liquid`].
pub fn categorize_position(model: &CollisionModel, state: &mut PlayerState, config: &MoveConfig) {
    let (level, liquid) = categorize_liquid(model, state, config);
    state.water_level = level;
    state.liquid = liquid;

    // A player hanging on a ladder is not standing on anything, even when
    // the bottom of the ladder is on the floor: letting the ground probe
    // snap them down would cancel every climbing step.
    if state.on_ladder {
        state.on_ground = false;
        state.ground_normal = Vec3::ZERO;
        return;
    }

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
        unstick_from_ground(model, state);
    } else {
        state.on_ground = false;
        state.ground_normal = Vec3::ZERO;
    }
}

/// How far [`unstick_from_ground`] tries nudging the player upward, in
/// one-unit steps, to recover a landing whose own ground-probe trace still
/// reports the hull embedded in solid. A hard-enough closing velocity can
/// resolve [`categorize_position`]'s short probe to a surface the full
/// standing/crouched hull still overlaps by a hair (the probe trace backs
/// off only [`crate::hull::DIST_EPSILON`] short of the plane, not the
/// hull's own bounding box), which otherwise leaves the player permanently
/// embedded rather than resting on top. Bounded rather than searched
/// until success, so a landing spot that is genuinely solid all the way
/// through (a mapper's own error) gives up instead of looping.
const UNSTICK_MAX_NUDGE: f32 = 34.0;

/// The step [`unstick_from_ground`] nudges by on each attempt.
const UNSTICK_STEP: f32 = 1.0;

/// If `state.origin` is embedded in solid (checked with a zero-length trace
/// in the player's own hull, the same test a fresh landing already used),
/// nudges it straight up in [`UNSTICK_STEP`] increments, up to
/// [`UNSTICK_MAX_NUDGE`] units, and keeps the first offset that is not
/// embedded. Leaves `state` untouched if no such offset is found within the
/// bound: a stuck player is no better off, but no worse either.
fn unstick_from_ground(model: &CollisionModel, state: &mut PlayerState) {
    let hull = state.hull();
    if !model.trace(hull, state.origin, state.origin).start_solid {
        return;
    }
    let mut offset = UNSTICK_STEP;
    while offset <= UNSTICK_MAX_NUDGE {
        let candidate = state.origin + Vec3::Z * offset;
        if !model.trace(hull, candidate, candidate).start_solid {
            state.origin = candidate;
            return;
        }
        offset += UNSTICK_STEP;
    }
}

/// How deep the player is in a liquid and which liquid it is, sampled from
/// the BSP leaf contents at the feet, the origin and the eye, exactly the
/// three heights the documented `waterlevel` 0..3 scale distinguishes.
///
/// The reported [`LiquidKind`] is the one at the deepest sample that is in
/// a liquid at all, so a player wading in water whose head is out of it is
/// still "in water", and a player whose feet are in lava is in lava.
#[must_use]
pub fn categorize_liquid(
    model: &CollisionModel,
    state: &PlayerState,
    config: &MoveConfig,
) -> (WaterLevel, LiquidKind) {
    let foot_offset = state.hull().foot_offset();
    let feet = model.point_contents(state.origin - Vec3::Z * (foot_offset - 1.0));
    if !contents::is_liquid(feet) {
        return (WaterLevel::Dry, LiquidKind::None);
    }
    let waist = model.point_contents(state.origin);
    let eye = model.point_contents(state.eye_position(config));
    if contents::is_liquid(eye) {
        return (WaterLevel::Eyes, LiquidKind::from_contents(eye));
    }
    if contents::is_liquid(waist) {
        return (WaterLevel::Waist, LiquidKind::from_contents(waist));
    }
    (WaterLevel::Feet, LiquidKind::from_contents(feet))
}

/// The horizontal directions [`ladder_normal`] probes in, in a fixed order
/// so the result never depends on iteration order.
const LADDER_PROBE_DIRECTIONS: [Vec3; 4] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(-1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, -1.0, 0.0),
];

/// How far out [`ladder_normal`] probes, and in what step, when looking for
/// the face of the ladder volume the player is standing against. The step
/// is smaller than the 32-unit player hull so a thin ladder slab is found.
const LADDER_PROBE_STEP: f32 = 4.0;
const LADDER_PROBE_STEPS: u8 = 12;

/// Whether `point` is inside a climbable volume (`CONTENTS_LADDER`, the
/// contents a `func_ladder` brush is compiled with).
#[must_use]
pub fn is_ladder_contents(value: i32) -> bool {
    value == contents::LADDER
}

/// Whether the player's origin is inside a climbable volume.
#[must_use]
pub fn in_ladder_volume(model: &CollisionModel, state: &PlayerState) -> bool {
    is_ladder_contents(model.point_contents(state.origin))
}

/// The outward normal of the ladder face the player is on: the horizontal
/// direction in which the ladder volume ends soonest without running into
/// solid world. Zero when the player is not in a ladder volume, or when no
/// open face was found (a fully embedded volume), which the caller treats
/// as "not climbable".
#[must_use]
pub fn ladder_normal(model: &CollisionModel, state: &PlayerState) -> Vec3 {
    if !in_ladder_volume(model, state) {
        return Vec3::ZERO;
    }
    let mut best = Vec3::ZERO;
    let mut best_distance = f32::INFINITY;
    for direction in LADDER_PROBE_DIRECTIONS {
        for step in 1..=LADDER_PROBE_STEPS {
            let distance = LADDER_PROBE_STEP * f32::from(step);
            let sample = model.point_contents(state.origin + direction * distance);
            if is_ladder_contents(sample) {
                continue;
            }
            if !contents::is_solid(sample) && distance < best_distance {
                best_distance = distance;
                best = direction;
            }
            break;
        }
    }
    best
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

/// What happened during one movement step that the rest of the game has to
/// react to. Movement itself never applies damage or plays a sound; it only
/// reports, so `ohl-player` owns every gameplay consequence.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct MoveEvents {
    /// The downward speed, in units per second and positive, at the moment
    /// the player touched the ground after being airborne. `None` when the
    /// player did not land this step, and also when they landed in a liquid
    /// or caught a ladder, both of which are documented as cancelling a
    /// fall (see `docs/FORMAT_SOURCES.md`, "Player systems").
    pub landed_speed: Option<f32>,
    /// A long jump fired this step.
    pub long_jumped: bool,
    /// The player attached to a ladder this step.
    pub ladder_attached: bool,
    /// The player left a ladder this step.
    pub ladder_detached: bool,
    /// The player's [`WaterLevel`] changed this step.
    pub water_level_changed: bool,
}

impl MoveEvents {
    /// Folds a later step's events into this one: the fastest landing wins
    /// and every flag is OR-ed, so a caller running several ticks per frame
    /// still sees each thing that happened exactly once.
    pub fn merge(&mut self, other: Self) {
        self.landed_speed = match (self.landed_speed, other.landed_speed) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.long_jumped |= other.long_jumped;
        self.ladder_attached |= other.ladder_attached;
        self.ladder_detached |= other.ladder_detached;
        self.water_level_changed |= other.water_level_changed;
    }
}

/// Advances `state` by one fixed timestep of `dt` seconds.
///
/// This is [`player_move_events`] with the reported events discarded; it is
/// kept as the simple entry point for callers that only want the motion.
pub fn player_move(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    dt: f32,
) {
    let _events = player_move_events(model, state, input, config, dt);
}

/// Advances `state` by one fixed timestep of `dt` seconds and reports what
/// happened, so the caller can turn a landing into fall damage, a water
/// level change into a drowning timer, and so on.
pub fn player_move_events(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    dt: f32,
) -> MoveEvents {
    let mut events = MoveEvents::default();
    if !dt.is_finite() || dt <= 0.0 {
        return events;
    }
    let (wish_dir, wish_scale) = wish_direction(input);

    if state.noclip {
        state.ducked = input.duck;
        state.on_ground = false;
        state.ground_normal = Vec3::ZERO;
        state.velocity = wish_dir * (config.noclip_speed * wish_scale);
        state.origin += state.velocity * dt;
        state.water_level = WaterLevel::Dry;
        state.liquid = LiquidKind::None;
        events.ladder_detached = core::mem::replace(&mut state.on_ladder, false);
        return events;
    }

    let was_on_ground = state.on_ground;
    let was_on_ladder = state.on_ladder;
    let was_water_level = state.water_level;
    let mut entry_velocity_z = state.velocity.z;

    apply_duck(model, state, input);
    if input.duck {
        state.duck_seconds += dt;
    } else {
        state.duck_seconds = 0.0;
    }
    state.ladder_lockout = (state.ladder_lockout - dt).max(0.0);
    categorize_position(model, state, config);
    update_ladder_attachment(model, state, config);

    if state.on_ladder {
        ladder_move(model, state, input, config, wish_dir, wish_scale, dt);
    } else {
        apply_friction(model, state, config, dt);
        // The speed the player is falling at as this step begins. It is
        // taken here, before the move, because the move itself clips the
        // velocity into the ground plane the moment it touches down, which
        // would otherwise leave nothing to report.
        entry_velocity_z = state.velocity.z;
        if state.is_swimming() {
            water_move(model, state, input, config, wish_dir, wish_scale, dt);
        } else {
            events.long_jumped =
                walk_or_air_move(model, state, input, config, wish_dir, wish_scale, dt);
        }
    }

    clamp_velocity(state, config);
    categorize_position(model, state, config);

    events.ladder_attached = state.on_ladder && !was_on_ladder;
    events.ladder_detached = was_on_ladder && !state.on_ladder;
    events.water_level_changed = state.water_level != was_water_level;
    if !was_on_ground
        && state.on_ground
        && !was_on_ladder
        && !state.on_ladder
        && state.water_level == WaterLevel::Dry
        && entry_velocity_z < 0.0
    {
        events.landed_speed = Some(-entry_velocity_z);
    }
    events
}

/// Attaches to or releases from a ladder volume, before the move itself
/// runs. Attaching cancels the player's vertical speed, which is the
/// documented behaviour of grabbing a GoldSrc ladder in mid-air.
fn update_ladder_attachment(model: &CollisionModel, state: &mut PlayerState, config: &MoveConfig) {
    if state.is_swimming() {
        // Swimming wins over climbing: a ladder inside a pool does not stop
        // the player from swimming.
        state.on_ladder = false;
        return;
    }
    let inside = ladder_normal(model, state) != Vec3::ZERO;
    if !inside {
        state.on_ladder = false;
        return;
    }
    if state.on_ladder {
        return;
    }
    if state.ladder_lockout > 0.0 {
        return;
    }
    let _ = config;
    state.on_ladder = true;
    state.velocity = Vec3::ZERO;
}

/// One step of climbing. The wished-for direction is decomposed against the
/// ladder's outward normal: the part pushing *into* the ladder climbs up,
/// the part pulling away climbs down, and whatever is left slides sideways
/// along the ladder plane. Gravity does not apply.
#[allow(clippy::too_many_arguments)]
fn ladder_move(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    wish_dir: Vec3,
    wish_scale: f32,
    dt: f32,
) {
    let normal = ladder_normal(model, state);
    if normal == Vec3::ZERO {
        state.on_ladder = false;
        return;
    }

    if input.jump && !state.jump_held {
        // Jumping off pushes away from the ladder face and locks out
        // re-attachment for long enough to clear the volume.
        state.on_ladder = false;
        state.jump_held = true;
        state.ladder_lockout = config.ladder_reattach_delay;
        state.velocity = normal * config.ladder_detach_speed;
        return;
    }
    state.jump_held = input.jump;

    let into = -wish_dir.dot(normal) * wish_scale;
    let sideways = horizontal(wish_dir - normal * wish_dir.dot(normal)) * wish_scale;
    state.velocity = (Vec3::Z * into + sideways) * config.ladder_speed;
    slide_move(model, state, config, dt);
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

/// Returns whether this step fired a long jump.
#[allow(clippy::too_many_arguments)]
fn walk_or_air_move(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    wish_dir: Vec3,
    wish_scale: f32,
    dt: f32,
) -> bool {
    let wish_speed = ground_max_speed(state, config) * wish_scale;
    let mut long_jumped = false;

    if input.jump {
        if state.on_ground && !state.jump_held {
            if long_jump_ready(state, input, config) && wish_dir.length_squared() > 0.0 {
                let forward = horizontal(wish_dir).normalize_or_zero();
                state.velocity =
                    forward * config.long_jump_forward_speed + Vec3::Z * config.long_jump_up_speed;
                long_jumped = true;
            } else {
                state.velocity.z = config.jump_velocity;
            }
            state.on_ground = false;
            state.ground_normal = Vec3::ZERO;
        }
        state.jump_held = true;
    } else {
        state.jump_held = false;
    }

    let base = if input.base_velocity.is_finite() {
        input.base_velocity
    } else {
        Vec3::ZERO
    };

    if state.on_ground {
        // Walking accelerates horizontally; slopes and steps are climbed by
        // the move itself (the slide clips the velocity into the surface and
        // the step move lifts over small obstructions), so the stored
        // velocity stays flat while the player is on the ground.
        state.velocity.z = 0.0;
        state.velocity = accelerate(state.velocity, wish_dir, wish_speed, config.accelerate, dt);
        state.velocity += base;
        step_move(model, state, config, dt);
        state.velocity -= base;
        state.velocity.z = 0.0;
    } else {
        // Half the gravity before the move and half after, so the height a
        // jump reaches does not depend on the timestep.
        state.velocity.z -= config.gravity * dt * 0.5;
        state.velocity = air_accelerate(state.velocity, wish_dir, wish_speed, config, dt);
        state.velocity += base;
        step_move(model, state, config, dt);
        state.velocity -= base;
        state.velocity.z -= config.gravity * dt * 0.5;
    }
    long_jumped
}

/// Whether a jump pressed right now is a long jump: the player owns the
/// module, is crouching, and the duck key went down less than
/// [`MoveConfig::long_jump_duck_window`] seconds ago. The public
/// description of the move is that it only fires *during* the crouching
/// motion, which is what the window models.
fn long_jump_ready(state: &PlayerState, input: &MoveInput, config: &MoveConfig) -> bool {
    input.long_jump
        && input.duck
        && state.ducked
        && state.duck_seconds > 0.0
        && state.duck_seconds <= config.long_jump_duck_window
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
