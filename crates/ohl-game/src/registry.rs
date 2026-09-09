//! A [`hecs::World`] populated from parsed [`EntityDef`]s, plus a bounded
//! `targetname -> entity` index.
//!
//! Component semantics (which keyvalues a `func_door`, `func_button`,
//! `func_plat`, a light entity, `path_corner`/`path_track`,
//! `trigger_changelevel`/`info_landmark` and `multi_manager` carry, and how
//! `angles` becomes a movement direction) are taken only from public
//! mapping documentation; see `docs/FORMAT_SOURCES.md` ("Entity keyvalues
//! and map logic"). No SDK source or decompiled logic was consulted.

use std::collections::BTreeMap;

use glam::Vec3;
use hecs::{Entity, World};

use crate::keyvalues::{self, EntityDef, Limits, ModelRef, RenderProps};

/// Largest number of entities the name index keeps for one `targetname`.
/// GoldSrc maps rarely share a name across more than a handful of
/// entities (e.g. a bank of lights); this simply bounds a worst case.
const MAX_ENTITIES_PER_NAME: usize = 64;

/// `classname`, kept verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClassName(pub String);

/// Position and facing, in GoldSrc world units and degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Transform {
    /// World-space origin.
    pub origin: Vec3,
    /// `pitch yaw roll`, in degrees.
    pub angles: Vec3,
}

/// `model` when it names a brush submodel (`*N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BrushModel(pub u32);

/// The world-space centre of a brush entity's placed submodel bounding
/// box, when `model_bounds` was supplied for its `BrushModel` index: the
/// compiled bounds' midpoint *plus* the entity's `origin` keyvalue.
///
/// Adding `origin` is what makes this a world-space point for every kind of
/// brush entity rather than only for the common one. A brush entity whose
/// geometry was compiled in absolute world space leaves `origin` at
/// `0 0 0`, so the sum is just the midpoint; one built around an "origin
/// brush" has its geometry compiled *relative to* that brush instead, and
/// the compiler writes the brush's own position into `origin` — so without
/// this addition its centre would land near the map's `(0, 0, 0)` rather
/// than anywhere the entity actually is. The same unconditional addition is
/// what `ohl-engine` already applies when it attaches the submodel to the
/// collision model and when it draws it, so this centre and the brush a
/// player collides with agree by construction (see [`crate::pose`]).
///
/// This is the entity's *resting* centre: a mover that has since travelled
/// or swung is at [`crate::pose::brush_center`] instead, which adds its
/// current state-machine displacement on top of this. Proximity checks such
/// as "use the nearest door" go through that function rather than reading
/// this component directly.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BrushCenter(pub Vec3);

/// `targetname`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TargetName(pub String);

/// `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Target(pub String);

/// `spawnflags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SpawnFlags(pub u32);

/// `rendermode`/`renderamt`/`rendercolor`, re-exported from
/// [`crate::keyvalues`] so a query only needs one component type.
pub type RenderPropsComponent = RenderProps;

/// A translating brush mover's open/close state, shared by [`Door`] and
/// [`Platform`] (documented on the Half-Life mapping wikis as the visible
/// behaviour of these entities: they sit closed/at rest, travel to the far
/// position, optionally wait there, then travel back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MoverState {
    /// At the starting position.
    Closed,
    /// Travelling from closed to open.
    Opening,
    /// At the far position, possibly about to auto-return.
    Open,
    /// Travelling from open back to closed.
    Closing,
}

/// `func_door` (and the shared timer/state-machine keys on
/// `func_door_rotating`, whose own axis/distance are carried separately in
/// [`Self::rotation_axis`]/[`Self::travel_distance`]): `speed`, `wait`,
/// `lip`, a movement direction derived from `angles`/`angle`, `dmg`,
/// `health`, `delay` and the `movesnd`/`stopsnd` sound indices.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Door {
    /// Units per second for a translating door; degrees per second for a
    /// rotating one ([`Self::rotation_axis`]) — the same `speed` keyvalue,
    /// interpreted in whichever unit `travel_distance` is currently in.
    pub speed: f32,
    /// Seconds the door stays open before auto-closing; `<= 0` means it
    /// stays open once opened.
    pub wait: f32,
    /// Units subtracted from the travel distance. Unused
    /// (`func_door_rotating`'s `lip` keyvalue is documented as "Not used";
    /// TWHL wiki `func_door_rotating`, see `docs/FORMAT_SOURCES.md`,
    /// "Entity keyvalues and map logic") when [`Self::rotation_axis`] is
    /// `Some`.
    pub lip: f32,
    /// Unit vector the door travels along when opening. Unused (left
    /// `Vec3::ZERO`) when [`Self::rotation_axis`] is `Some`: a rotating
    /// door swings about its pivot instead of sliding.
    pub movedir: Vec3,
    /// Damage dealt to anything blocking the door.
    pub dmg: f32,
    /// Hit points before the door can be shot open; `0` means unbreakable
    /// by damage (it only opens via `use`/trigger).
    pub health: f32,
    /// Seconds between being triggered and starting to move.
    pub delay: f32,
    /// `movesnd`/`stopsnd` indices into the built-in door sound tables.
    pub sounds: (u8, u8),
    /// The distance travelled when opening: for a translating door,
    /// precomputed from the brush model's bounding box (`maxs - mins`,
    /// projected onto `movedir`) minus `lip`, since the map logic
    /// simulation never touches BSP data directly. For a rotating door
    /// ([`Self::rotation_axis`]), this is instead the `distance` keyvalue
    /// itself, in degrees (TWHL wiki `func_door_rotating`: "Distance in
    /// degrees to rotate"); always non-negative — direction lives in
    /// [`Self::rotation_axis`]'s sign, not here, so `ohl_game::logic`'s
    /// shared `distance / speed` timing math (which does not otherwise
    /// special-case a rotating door at all) stays meaningful for either
    /// kind.
    pub travel_distance: f32,
    /// `Some(axis)` for `func_door_rotating`: the signed unit axis
    /// (magnitude 1; sign carries the "Reverse Direction" spawnflag and a
    /// negative `distance` keyvalue, both documented as reversing which
    /// way the door swings) this door rotates about its origin-keyvalue
    /// pivot instead of sliding along [`Self::movedir`]. `None` for an
    /// ordinary translating `func_door`.
    pub rotation_axis: Option<Vec3>,
    /// Current animation state.
    pub state: MoverState,
    /// Seconds remaining in the current state's motion or wait.
    pub timer: f32,
}

/// `func_door_rotating`'s activator-relative swing data: everything the
/// "opens away from whoever opened it" rule needs that is fixed at spawn
/// and so never has to be saved (`ohl-engine` rebuilds it identically from
/// the map every load, exactly as it already does for
/// [`Rotator::axis`]/[`Rotator::speed`]).
///
/// TWHL wiki `func_door_rotating` (`docs/FORMAT_SOURCES.md`, "Entity
/// keyvalues and map logic", item 24; search-summary citation, reviewed
/// 2026-09-07, same HTTP 403 caveat as the other TWHL citations in this
/// crate): "the door will always open away from the player" unless the
/// "One Way" spawnflag ([`SPAWNFLAG_DOOR_ROTATING_ONE_WAY`], "door only
/// opens in the direction set in Distance") is set. *Which* side counts as
/// "away", and what a door whose activator stands exactly on its hinge
/// plane does, are not stated by any public source; both are this
/// project's own bounded reading, recorded as project behaviour in
/// `docs/FORMAT_SOURCES.md` item 26 and implemented by
/// [`Self::opening_axis`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RotatingDoorSwing {
    /// The signed unit axis the spawnflags and `distance` keyvalue alone
    /// select — what [`Door::rotation_axis`] is initialised to, kept
    /// separately because that field's own sign is what
    /// [`Self::opening_axis`] overwrites on each open.
    pub base_axis: Vec3,
    /// A unit vector perpendicular to both [`Self::base_axis`] and the
    /// door leaf, pointing the way the leaf's centre first moves when the
    /// door swings by a small *positive* angle about [`Self::base_axis`]
    /// (the rigid-body relation `v = axis x r`, evaluated at the leaf's
    /// own compiled centre). `Vec3::ZERO` when the leaf's centre sits on
    /// the rotation axis itself — a symmetric double leaf, or a submodel
    /// with no bounds recorded — leaving no side to prefer.
    pub hinge_normal: Vec3,
    /// The "One Way" spawnflag ([`SPAWNFLAG_DOOR_ROTATING_ONE_WAY`]): the
    /// door ignores the activator entirely and always swings the way its
    /// spawnflags/`distance` chose.
    pub one_way: bool,
}

impl RotatingDoorSwing {
    /// Which signed axis this door should swing about to move *away* from
    /// an activator standing at `activator`, given the door pivots about
    /// `pivot` (its `origin` keyvalue). `None` means "keep whatever sign
    /// the door already has": a "One Way" door, a door with no leaf
    /// direction to reason about ([`Self::hinge_normal`] zero), an
    /// activator whose position is unknown or not finite, or an activator
    /// standing exactly on the hinge plane (`side == 0.0`), where "away"
    /// is genuinely undefined and this project deliberately falls back to
    /// the spawnflag/keyvalue direction rather than guessing.
    #[must_use]
    pub fn opening_axis(&self, pivot: Vec3, activator: Option<Vec3>) -> Option<Vec3> {
        if self.one_way || self.hinge_normal == Vec3::ZERO {
            return None;
        }
        let activator = activator?;
        if !activator.is_finite() || !pivot.is_finite() {
            return None;
        }
        let side = (activator - pivot).dot(self.hinge_normal);
        if side > 0.0 {
            // The activator stands on the side the leaf would sweep into
            // under a positive rotation, so swing the other way.
            Some(-self.base_axis)
        } else if side < 0.0 {
            Some(self.base_axis)
        } else {
            None
        }
    }
}

/// Marks a `func_door`/`func_door_rotating` whose "Use Only" spawnflag
/// ([`SPAWNFLAG_DOOR_USE_ONLY`]) is set: fixed at spawn from the map's own
/// `spawnflags` keyvalue, exactly like [`RotatingDoorSwing`] above (see its
/// own doc comment), so it never needs to be saved — `ohl-engine` rebuilds
/// it identically from the map every load. A plain marker rather than a
/// `bool` field on [`Door`] itself: `Door` is reached transitively by the
/// save-file's *required* entity-registry section
/// (`ohl_engine::transition::EntitySnapshot::door`), whose wire shape is
/// frozen (`docs/FORMAT_SOURCES.md` item 28, "New persisted state gets a
/// new optional tag ... never a new field on an existing tag's type or on
/// any type it reaches transitively"); a component the save path never
/// looks at carries this instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DoorUseOnly;

/// Marks a `func_door`/`func_door_rotating` whose "Passable" spawnflag
/// ([`SPAWNFLAG_DOOR_PASSABLE`]) is set: the Sven Co-op wiki's `Func_door`
/// page (`https://wiki.svencoop.com/Func_door`, fetched directly; see
/// `docs/FORMAT_SOURCES.md` item 30) documents it as "the door is entirely
/// non-solid. It also cannot be triggered on-touch anymore then." Fixed at
/// spawn from the map's own `spawnflags` keyvalue, the same rationale
/// [`DoorUseOnly`]'s own doc comment gives for why this is a marker rather
/// than a `Door` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DoorPassable;

/// `func_button`: `speed`, `wait`, `health`, `delay` and a `sounds` index.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Button {
    /// Units per second the button travels in when pressed.
    pub speed: f32,
    /// Seconds before the button returns/can be pressed again.
    pub wait: f32,
    /// Hit points before a `func_button` with no `health` responds only to
    /// `use`; `0` means it only responds to `use`/touch, not damage.
    /// [`crate::logic::Simulation::damage_button`] reads this: once accrued
    /// damage reaches it, the button presses exactly as a `use` would (see
    /// that method's own doc comment for the "TODO(black-box)" this closed,
    /// and `docs/FORMAT_SOURCES.md` item 27).
    pub health: f32,
    /// Seconds between being pressed and firing `target`.
    pub delay: f32,
    /// `sounds` index into the built-in button sound table.
    pub sound: u8,
    /// Current animation state (buttons only ever travel forward and
    /// return, so [`MoverState::Opening`]/[`MoverState::Closing`] stand in
    /// for "pressing in" and "returning").
    pub state: MoverState,
    /// Seconds remaining in the current state.
    pub timer: f32,
}

/// `func_plat`: the same translating-mover keys as [`Door`], with an
/// optional explicit `height` overriding the bounding-box-derived travel
/// distance (the public-documented behaviour when a mapper wants a platform
/// to travel further or less than its own brush height).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Platform {
    /// Units per second.
    pub speed: f32,
    /// Seconds the platform waits at the top before returning.
    pub wait: f32,
    /// Unit vector the platform travels along when activated (usually
    /// straight down, since platforms are triggered from their raised
    /// position in the common mapping pattern).
    pub movedir: Vec3,
    /// The distance travelled, from an explicit `height` keyvalue when
    /// present, else the bounding-box-derived distance minus `lip`.
    pub travel_distance: f32,
    /// `sounds` index into the built-in platform sound table.
    pub sounds: (u8, u8),
    /// Current animation state.
    pub state: MoverState,
    /// Seconds remaining in the current state's motion or wait.
    pub timer: f32,
}

/// `func_rotating`: a brush that spins continuously about a fixed axis
/// through its origin keyvalue, rather than opening/closing between two
/// resting poses like [`Door`]. TWHL wiki `func_rotating` (`docs/
/// FORMAT_SOURCES.md`, "Entity keyvalues and map logic"): `speed` in
/// degrees per second, and spawnflags selecting the rotation axis, initial
/// on/off state, and direction.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rotator {
    /// Signed unit axis (magnitude 1); the "Reverse Direction" spawnflag
    /// bakes into its sign, the same convention as
    /// [`Door::rotation_axis`].
    pub axis: Vec3,
    /// Degrees per second; always non-negative (direction is [`Self::axis`]'s
    /// sign, not this).
    pub speed: f32,
    /// Whether the brush is currently spinning; toggled by `use`/trigger
    /// (`ohl_game::logic::Simulation::activate`). Starts `true` when the
    /// "Start On" spawnflag is set.
    pub spinning: bool,
    /// The accumulated rotation angle, degrees, wrapped into `0.0..360.0`
    /// every step so it never grows without bound over a long play
    /// session.
    pub angle_deg: f32,
}

/// `func_rot_button`: a rotating button sharing `func_button`'s "press,
/// fire `target`, optionally auto-reset" shape (TWHL wiki `func_rot_button`,
/// `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic") but swinging
/// about an origin-keyvalue pivot like [`Door::rotation_axis`]/[`Rotator`]
/// instead of sliding. Kept as its own component rather than folding into
/// [`Door`] or [`Button`]: unlike a plain [`Door`] it fires `target` on
/// reaching its pressed pose, and unlike a plain [`Button`] it can be
/// toggled back and forth (`toggle`) instead of only auto-returning.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RotButton {
    /// Signed unit axis; the "Reverse direction" spawnflag bakes into its
    /// sign, the same convention as [`Door::rotation_axis`]/[`Rotator::axis`].
    pub axis: Vec3,
    /// Degrees per second.
    pub speed: f32,
    /// `distance`: degrees to rotate before firing `target`.
    pub distance: f32,
    /// `wait`: seconds before auto-resetting; `< 0` stays pressed
    /// (documented as "-1 makes it stay set").
    pub wait: f32,
    /// Hit points before the button responds to damage; `0` means it does
    /// not respond to damage at all (only `use`/touch). Read by
    /// [`crate::logic::Simulation::damage_button`], which presses the
    /// button once accrued damage reaches this value — the "(or by being
    /// shot, if Health is > 0)" half of the documented "Touch activates"
    /// wording, closing the `TODO(black-box)` this field previously
    /// recorded (`docs/FORMAT_SOURCES.md` item 27); [`Button::health`],
    /// which this field mirrors, is wired through the same method.
    pub health: f32,
    /// Seconds between activation and starting to rotate.
    pub delay: f32,
    /// `sounds` index into the built-in button sound table.
    pub sound: u8,
    /// The documented "Toggle" spawnflag: using the button while it is
    /// pressed rotates it back (firing `target` again) instead of only ever
    /// auto-returning. This project's own reading (not itself stated by the
    /// cited wording) is that Toggle also suppresses the ordinary `wait`
    /// auto-return entirely: a toggle button only ever returns on a second
    /// `use`/touch, never on a timer — see `Simulation::advance_rot_buttons`'s
    /// `Open` arm.
    pub toggle: bool,
    /// The documented "Touch activates" spawnflag: the button also presses
    /// when a player's hull touches its brush, not only on `use`.
    pub touch: bool,
    /// Current animation state, the same shape [`Door`]/[`Button`] share.
    pub state: MoverState,
    /// Seconds remaining in the current state's motion or wait.
    pub timer: f32,
}

/// `momentary_rot_button`: a rotating valve/wheel that turns while `use` is
/// held and reports a `0.0..=1.0` fraction of its own `distance` sweep (TWHL
/// wiki `momentary_rot_button`, `docs/FORMAT_SOURCES.md`, "Entity keyvalues
/// and map logic"). Its documented `target` is normally a [`MomentaryDoor`]:
/// `crate::logic::Simulation::drive_momentary_rot_button` reads
/// [`Self::fraction`] back out through this entity's `Target` keyvalue every
/// tick it changes and pushes it toward every `momentary_door` sharing that
/// `targetname` (`docs/FORMAT_SOURCES.md`, item 29).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[allow(clippy::struct_excessive_bools)]
pub struct MomentaryRotButton {
    /// Signed unit axis (magnitude 1); this entity has no documented
    /// "Reverse direction" spawnflag, so nothing bakes a sign into it beyond
    /// the plain axis-selection spawnflags.
    pub axis: Vec3,
    /// `speed`: degrees per second while `use` is held.
    pub speed: f32,
    /// `distance`: total degrees of the sweep from `fraction = 0` to
    /// `fraction = 1`.
    pub distance: f32,
    /// `returnspeed`: degrees per second for the documented "Auto return"
    /// spawnflag's automatic return to `fraction = 0`.
    pub return_speed: f32,
    /// The documented "Auto return" spawnflag (16).
    pub auto_return: bool,
    /// The documented "Door Hack" spawnflag (1): "makes this entity solid,
    /// but un-USE-able"; such a button only ever moves because a paired
    /// `momentary_rot_button` sharing its `momentary_door` target moved
    /// it — a linkage this crate does not model (see [`Self`]'s own doc
    /// comment) — so this project's own reading is that a "Door Hack"
    /// button is simply excluded from `use`-proximity search
    /// ([`crate::logic::find_usable_within`]) while still occupying its
    /// brush's collision, exactly as documented.
    pub door_hack: bool,
    /// Current position, `0.0` (rest) to `1.0` (fully turned).
    pub fraction: f32,
    /// Whether the next `use`-hold turn moves toward `fraction = 1.0`
    /// (`true`) or back toward `0.0` (`false`); flips whenever an endpoint
    /// is reached, matching the documented "flip-flops between opening and
    /// closing when it reaches its endpoints" behaviour.
    pub moving_forward: bool,
    /// Whether the "Auto return" spawnflag's return-to-zero animation is
    /// currently running (set the moment `use` stops being held while
    /// [`Self::auto_return`] is set and [`Self::fraction`] is not already
    /// zero).
    pub returning: bool,
}

/// `momentary_door`: a translating brush whose position is a `0.0..=1.0`
/// fraction of its own travel, synchronised with whichever
/// [`MomentaryRotButton`] currently targets it (TWHL wiki
/// `momentary_rot_button`, `docs/FORMAT_SOURCES.md`, "Entity keyvalues and
/// map logic": the linked `momentary_door`'s position "is synchronized with
/// the `momentary_rot_button` currently driving it... a `0` to `1`
/// fraction"; no separate `momentary_door`-specific public source was found
/// in this pass — see `docs/FORMAT_SOURCES.md`, item 29).
///
/// Shares [`Door`]'s translating `speed`/`lip`/`movedir`/`travel_distance`
/// shape (`crate::registry::movedir_from_angles`/`brush_travel_distance`,
/// the same helpers `func_door` uses) rather than [`Door`] itself: unlike a
/// `func_door` this entity has no `wait`/`state`/`timer` open-close cycle of
/// its own — it never moves except when a driving button pushes
/// [`Self::fraction`] toward a commanded value
/// (`crate::logic::Simulation::drive_momentary_rot_button`). **This
/// project's own reading, not itself stated by the cited wording**: the
/// cited "synchronized" text does not say at what rate the door's own
/// position follows the button's, so this project rate-limits it by the
/// door's own `speed` keyvalue (the same FGD-conventional field name/
/// default `func_door` already uses) rather than snapping instantly,
/// recorded as project behaviour where the public source is silent.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MomentaryDoor {
    /// Units per second this door's own [`Self::fraction`] follows a
    /// commanded value at; this project's own choice (see this struct's own
    /// doc comment).
    pub speed: f32,
    /// Units subtracted from the bounding-box-derived travel distance, the
    /// same `lip` convention [`Door::lip`] documents for `func_door`.
    pub lip: f32,
    /// Unit vector this door travels along from `fraction = 0.0` to
    /// `fraction = 1.0`, derived from `angles`/`angle` the same way
    /// [`Door::movedir`] is.
    pub movedir: Vec3,
    /// The distance travelled over the full `0.0..=1.0` sweep, derived from
    /// the brush model's bounding box the same way [`Door::travel_distance`]
    /// is for a translating `func_door`.
    pub travel_distance: f32,
    /// Current position, `0.0` (rest) to `1.0` (fully travelled) — see
    /// [`crate::pose::momentary_door_offset`].
    pub fraction: f32,
}

/// `func_breakable` (and, when its documented "Breakable" spawnflag is set,
/// `func_pushable`): a solid brush that is removed from the world once it
/// has taken its `health` keyvalue's worth of damage, or once it is
/// triggered.
///
/// TWHL wiki `func_breakable` (`https://twhl.info/wiki/page/func_breakable`,
/// fetched directly, HTTP 200, reviewed 2026-09-08; `docs/
/// FORMAT_SOURCES.md`, item 32): "The `func_breakable` entity allows you to
/// create a breakable brush, with the option of spawning items";
/// "Strength (`health`) - The amount of damage the entity will take before
/// breaking"; "Target on Break (`target`) - When the `func_breakable` is
/// broken or triggered, it will activate this entity"; "Delay before fire
/// (`delay`) - Delay before Target on Break is triggered after being
/// broken"; "Material Type (`material`)"; and the flags "Only Trigger (1)",
/// "Touch (2)", "Pressure (4)" and "Instant crowbar (256)".
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
// Five spawnflag/state booleans, each one a documented flag of its own (see
// this struct's own doc comment); the same `allow` `MomentaryRotButton`
// already carries for its own flag set.
#[allow(clippy::struct_excessive_bools)]
pub struct Breakable {
    /// Remaining hit points, seeded from the documented `health`
    /// ("Strength") keyvalue and reduced by
    /// [`crate::logic::Simulation::damage_breakable`]. `0.0` means the
    /// brush cannot be broken by damage at all and waits to be triggered —
    /// the same reading [`Button::health`] already records for a `0`
    /// `health` on a button.
    pub health: f32,
    /// The `material` keyvalue as authored, `0..=8` per the cited option
    /// list (0 Glass, 1 Wood, 2 Metal, 3 Flesh, 4 Cinder Block, 5 Ceiling
    /// Tile, 6 Computer, 7 Unbreakable Glass, 8 Rocks). Carried so a later
    /// package can pick the documented per-material break sound and gib
    /// model; **nothing in this project reads it yet** (see
    /// `docs/FORMAT_SOURCES.md`, item 32's own gap list).
    pub material: u8,
    /// The documented "Only Trigger (1)" flag: the brush can only be broken
    /// by being triggered, never by damage.
    pub trigger_only: bool,
    /// The documented "Touch (2)" flag: "Brush will break on touch."
    pub break_on_touch: bool,
    /// The documented "Pressure (4)" flag: "Brush will break when pressured
    /// (e.g. player walking on it)."
    pub break_on_pressure: bool,
    /// The documented "Instant crowbar (256)" flag: "Whack it with a
    /// crowbar and it will break instantly (regardless of strength)."
    pub instant_crowbar: bool,
    /// The documented "Delay before fire" (`delay`) keyvalue: seconds
    /// between breaking and firing `target`.
    pub delay: f32,
    /// Whether this brush has already broken. A broken brush is skipped by
    /// [`crate::brush::model_instances`]/[`crate::brush::solid_model_instances`]
    /// (so it stops being drawn and stops being re-attached to a collision
    /// model) and detached from the live collision model by
    /// `ohl_engine::Level::sync_brush_collision`.
    pub broken: bool,
}

/// `func_pushable`: the one brush entity a player can push around.
///
/// TWHL wiki `func_pushable` (`https://twhl.info/wiki/page/func_pushable`,
/// fetched directly, HTTP 200, reviewed 2026-09-08; `docs/
/// FORMAT_SOURCES.md`, item 32): "The only type of brush entity that can be
/// pushed, pulled, lifted (not by the player) and fall, most commonly used
/// with crates. It can also optionally be breakable"; "Friction
/// (`friction`) - This determines the amount of resistance the brush will
/// give when the player pushes it. Range is 0 to 400, where 400 is the most
/// resistance"; "Hull size (`size`) - The parameter is supposed to set the
/// entity size, but it doesn't do anything"; and the flag "Breakable (128)
/// - If this is enabled, the entity will act like a `func_breakable`."
///
/// Every `func_pushable` also carries a [`Breakable`]; only one whose
/// "Breakable" flag is set can actually be broken by damage
/// ([`Self::breakable`]).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pushable {
    /// The documented `friction` keyvalue, clamped to the documented
    /// `0..=400` range: how much resistance the brush gives when pushed.
    pub friction: f32,
    /// The documented "Breakable (128)" flag.
    pub breakable: bool,
    /// How far this brush has been pushed from where it was compiled —
    /// this project's own runtime state, reaching the renderer, the
    /// collision model and the `use`-proximity search through
    /// [`crate::pose::pushable_offset`], the same one-offset pipeline every
    /// other translating mover already uses.
    pub offset: Vec3,
}

/// `func_breakable`'s documented "Only Trigger" flag.
pub const SPAWNFLAG_BREAKABLE_ONLY_TRIGGER: u32 = 1;
/// `func_breakable`'s documented "Touch" flag.
pub const SPAWNFLAG_BREAKABLE_TOUCH: u32 = 2;
/// `func_breakable`'s documented "Pressure" flag.
pub const SPAWNFLAG_BREAKABLE_PRESSURE: u32 = 4;
/// `func_breakable`'s documented "Instant crowbar" flag.
pub const SPAWNFLAG_BREAKABLE_INSTANT_CROWBAR: u32 = 256;
/// `func_pushable`'s documented "Breakable" flag.
pub const SPAWNFLAG_PUSHABLE_BREAKABLE: u32 = 128;
/// The documented upper end of `func_pushable`'s `friction` range ("Range
/// is 0 to 400, where 400 is the most resistance").
pub const PUSHABLE_MAX_FRICTION: f32 = 400.0;

/// `func_pendulum`: a brush that swings continuously about an origin-keyvalue
/// pivot like a physical pendulum (TWHL wiki `func_pendulum`,
/// `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic"): `distance`
/// (degrees of swing), `speed`, an optional `damping` that narrows the swing
/// until it settles in the middle, and an "Auto Return"/"Start On" pair of
/// spawnflags. No public source states the exact trigonometric law GoldSrc
/// integrates each step; this project implements a plain damped sinusoid and
/// records that choice at the point of use (`crate::logic::Simulation::
/// advance_pendulums`), per this milestone's own instruction to document
/// rather than guess at an uncited motion law.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pendulum {
    /// Signed unit axis.
    pub axis: Vec3,
    /// `distance`: peak swing amplitude, in degrees, from the rest pose.
    pub distance: f32,
    /// `speed`: this project's own mapping is the peak angular speed at the
    /// rest crossing, in degrees/second (see `advance_pendulums`).
    pub speed: f32,
    /// `damping`: the documented `0..1000` raw keyvalue.
    pub damping: f32,
    /// The documented "Auto Return" spawnflag (16): toggling the pendulum
    /// off animates it back to the rest pose instead of freezing in place.
    pub auto_return: bool,
    /// Whether the pendulum is currently swinging; toggled by `use`/trigger.
    /// Starts `true` when the documented "Start ON" spawnflag
    /// ([`SPAWNFLAG_PENDULUM_START_ON`]) is set.
    pub swinging: bool,
    /// Seconds of swing time accumulated while [`Self::swinging`]; frozen
    /// while paused so resuming continues the same sinusoid rather than
    /// restarting it.
    pub elapsed: f32,
    /// Whether [`Self::auto_return`]'s return-to-rest animation is
    /// currently running.
    pub returning: bool,
    /// Current pose, in degrees from rest.
    pub angle_deg: f32,
}

/// `light`/`light_spot`/`light_environment`: brightness, colour, style and
/// (for spot/environment lights) an aim.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Light {
    /// Brightness, the first number of the `_light`/`light` keyvalue.
    pub brightness: f32,
    /// Colour, the following three numbers of the same keyvalue, or white
    /// when only brightness was given.
    pub color: [u8; 3],
    /// Light style/animation index (`0` is the always-on default style).
    pub style: u8,
    /// `_cone` for `light_spot`, in degrees.
    pub cone: Option<f32>,
}

/// Marks the `info_player_start` entity; its pose lives in [`Transform`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PlayerStart;

/// Marks an `info_landmark` entity, used together with [`ChangeLevel`] to
/// align the player across a level transition; its position lives in
/// [`Transform`] and its name in [`TargetName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Landmark;

/// `trigger_changelevel`: the destination map and the shared landmark name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChangeLevel {
    /// The `map` keyvalue.
    pub map: String,
    /// The `landmark` keyvalue, matched against an [`Landmark`] entity's
    /// [`TargetName`] in the destination map.
    pub landmark: String,
}

/// `globalname`: the cross-level correlation key. Two entities in two
/// different maps that share a `globalname` are the same entity as far as a
/// level transition is concerned, which is how a mover keeps its state
/// across a `changelevel` (public mapping documentation only; see
/// `docs/FORMAT_SOURCES.md`, "Campaign flow").
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GlobalName(pub String);

/// A brush entity's placed submodel bounding box: the box copied from the
/// BSP model lump when `model_bounds` supplied it, offset by the entity's
/// `origin` keyvalue the same unconditional way [`BrushCenter`] is, so the
/// two always agree ([`BrushCenter`] is exactly this box's midpoint). Kept
/// alongside [`BrushCenter`] because a `trigger_transition` volume needs
/// the full box, not just its middle.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BrushBounds {
    /// Lower corner.
    pub mins: Vec3,
    /// Upper corner.
    pub maxs: Vec3,
}

impl BrushBounds {
    /// Whether `point` lies inside (or exactly on) this box.
    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        point.cmpge(self.mins).all() && point.cmple(self.maxs).all()
    }
}

/// Marks a `trigger_transition` volume. Its [`TargetName`] is the
/// `info_landmark` name it gates, and its [`BrushBounds`] bound which
/// entities may travel to the next map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransitionVolume;

/// One global state variable's value.
///
/// The three documented states a global can hold. **To verify:** the M8
/// research pass could not retrieve a public page describing the exact
/// on-disk/save encoding of `env_global` state, so this enum models the
/// documented *behaviour* (a named variable that is off, on, or dead) and
/// not any specific byte layout; see `.plan/m8-research.md` open item 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum GlobalStateValue {
    /// The variable is unset/inactive.
    #[default]
    Off,
    /// The variable is set/active.
    On,
    /// The variable is permanently retired: an entity correlated with it is
    /// not respawned in a later map.
    Dead,
}

/// `env_global`: names a global state variable, the value activating it
/// writes, and the value it seeds on map load.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnvGlobal {
    /// The `globalstate` keyvalue: the variable's name.
    pub global_state: String,
    /// The value written when this entity is activated (`triggermode`;
    /// `None` means "toggle between off and on").
    pub trigger_mode: Option<GlobalStateValue>,
    /// The `initialstate` value seeded on map load when the entity's
    /// documented "Set Initial State" spawnflag (bit `1`) is set.
    pub initial_state: GlobalStateValue,
    /// Whether the spawnflag asking for [`Self::initial_state`] to be
    /// applied on load is set.
    pub sets_initial_state: bool,
}

/// `env_message` (a `titles.txt` entry name) or `game_text` (literal text),
/// with the fade/hold timings the HUD shows it for.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Message {
    /// `message`: a `titles.txt` entry name for `env_message`, the literal
    /// text for `game_text`.
    pub message: String,
    /// `true` when [`Self::message`] is literal text (`game_text`) rather
    /// than a `titles.txt` entry name (`env_message`).
    pub literal: bool,
    /// `fadein` seconds, when the entity overrides it.
    pub fadein: Option<f32>,
    /// `fadeout` seconds, when the entity overrides it.
    pub fadeout: Option<f32>,
    /// `holdtime` seconds, when the entity overrides it.
    pub holdtime: Option<f32>,
}

/// A `path_corner`/`path_track`'s documented fire-on-pass `message`: the
/// name of an entity to fire as a follower passes this node.
///
/// A separate component rather than a field on [`Path`] so that type stays
/// `Copy`; see `docs/FORMAT_SOURCES.md` ("Track trains and paths") for the
/// public source.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PathFireOnPass(pub String);

/// A `path_track`'s documented `netname` ("Fire on dead end"): the name of
/// an entity fired when a `func_tracktrain` reaches this node as the last
/// node of its chain.
///
/// A separate component rather than a field on [`Path`] for the same
/// reason [`PathFireOnPass`] is one — that type stays `Copy`. See
/// `docs/FORMAT_SOURCES.md` ("Track trains and paths") for the public
/// source.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PathFireOnDeadEnd(pub String);

/// `func_trackchange`/`func_trackautochange`: the moving piece of track
/// that carries a `func_tracktrain` from one `path_track` chain to
/// another, rotating and/or travelling between them.
///
/// The names it links ([`TrackChangeLinks`]) live in their own component
/// so this one stays `Copy`, exactly as [`Path`]/[`PathFireOnPass`] are
/// split. See `docs/FORMAT_SOURCES.md` ("Track trains and paths") for the
/// public sources of every field here.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackChange {
    /// `height`: "travel distance, from top to bottom", in units, along
    /// the world up axis. Non-negative; the direction of travel comes
    /// from which end the platform is currently at.
    pub height: f32,
    /// `rotation`: "the spin done by this platform on entire way up/down",
    /// in degrees, about [`Self::axis`]. Signed.
    pub rotation: f32,
    /// `speed`: "speed in which func_trackautochange travel the whole way
    /// up/down (units per seconds)".
    pub speed: f32,
    /// The signed unit axis the platform spins about: the documented
    /// default `Z`, or `X`/`Y` with the matching spawnflag.
    pub axis: Vec3,
    /// The entity's own `spawnflags`, read through
    /// [`Self::auto_activate`]/[`Self::rotate_only`]/
    /// [`Self::start_at_bottom`] rather than unpacked into a field each:
    /// the documented flags are a bit field, and keeping them one is what
    /// lets a flag this project does not act on ("Relink track") still
    /// round-trip untouched.
    pub spawnflags: u32,
    /// How far the platform currently is from the end its geometry was
    /// compiled at, `0..=1`: `0` resting where it spawned, `1` resting at
    /// the other end. This, not an absolute "top"/"bottom", is what the
    /// pose is built from, so one fraction covers both spawn ends.
    pub displaced: f32,
    /// `1.0` while travelling away from the spawn end, `-1.0` while
    /// travelling back to it. Meaningless while [`Self::moving`] is false.
    pub direction: f32,
    /// Whether the platform is travelling right now.
    pub moving: bool,
    /// Whether a train was aboard when this travel started, and so is
    /// being carried between [`Self::carry_from`] and [`Self::carry_to`].
    pub carrying: bool,
    /// The world-space point the carried train rides from: the position of
    /// the `path_track` at the end the platform set off from.
    pub carry_from: Vec3,
    /// The world-space point the carried train rides to: the position of
    /// the `path_track` at the end the platform is travelling to, which is
    /// the node it is "assigned to" on arrival.
    pub carry_to: Vec3,
}

impl TrackChange {
    /// The documented "Auto Activate train" spawnflag
    /// ([`SPAWNFLAG_TRACK_CHANGE_AUTO_ACTIVATE`]). Recorded but not acted
    /// on: see `ohl_game::logic::Simulation::finish_track_change` for why
    /// this project's relinked train rides on either way.
    #[must_use]
    pub const fn auto_activate(&self) -> bool {
        self.spawnflags & SPAWNFLAG_TRACK_CHANGE_AUTO_ACTIVATE != 0
    }

    /// The documented "Rotate Only" spawnflag
    /// ([`SPAWNFLAG_TRACK_CHANGE_ROTATE_ONLY`]): the platform spins
    /// without travelling [`Self::height`].
    #[must_use]
    pub const fn rotate_only(&self) -> bool {
        self.spawnflags & SPAWNFLAG_TRACK_CHANGE_ROTATE_ONLY != 0
    }

    /// The documented "Start at Bottom" spawnflag
    /// ([`SPAWNFLAG_TRACK_CHANGE_START_AT_BOTTOM`]): the platform's
    /// compiled geometry rests at the bottom track rather than the top, so
    /// [`Self::displaced`] is measured *upward* from there.
    #[must_use]
    pub const fn start_at_bottom(&self) -> bool {
        self.spawnflags & SPAWNFLAG_TRACK_CHANGE_START_AT_BOTTOM != 0
    }

    /// Seconds one whole trip takes: the documented `height` at the
    /// documented `speed` ("speed in which func_trackautochange travel the
    /// whole way up/down (units per seconds)"). Zero — an instant trip —
    /// when either is zero, which is also what a "Rotate Only" platform
    /// gets: no public page states how long a rotation-only trip takes,
    /// since the one documented duration is built from a travel distance
    /// this flag removes. TODO(black-box).
    #[must_use]
    pub fn travel_seconds(&self) -> f32 {
        if self.rotate_only() || self.height <= 0.0 || self.speed <= 0.0 {
            return 0.0;
        }
        self.height / self.speed
    }

    /// Whether the platform is resting (or heading for) the *bottom*
    /// track, given which end its geometry was compiled at.
    #[must_use]
    pub fn at_bottom(&self) -> bool {
        self.start_at_bottom() != (self.displaced >= 0.5)
    }

    /// The platform's current displacement from where its geometry was
    /// compiled: along the world up axis, away from its spawn end.
    /// `Vec3::ZERO` for a "Rotate Only" platform, which is documented to
    /// spin without travelling.
    #[must_use]
    pub fn offset(&self) -> Vec3 {
        if self.rotate_only() {
            return Vec3::ZERO;
        }
        let away = if self.start_at_bottom() { 1.0 } else { -1.0 };
        Vec3::Z * (away * self.height * self.displaced.clamp(0.0, 1.0))
    }

    /// The platform's current spin, in degrees about [`Self::axis`],
    /// measured from the pose its geometry was compiled at: the documented
    /// `rotation` ("the spin done by this platform on entire way up/down")
    /// scaled by how far along that way it is.
    #[must_use]
    pub fn degrees(&self) -> f32 {
        self.rotation * self.displaced.clamp(0.0, 1.0)
    }
}

/// The three `targetname`s a [`TrackChange`] links: the train it carries
/// and the last/first `path_track` of the two chains it joins.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TrackChangeLinks {
    /// `train`: "name of the func_tracktrain this platform will transport
    /// to top/bottom track".
    pub train: String,
    /// `toptrack`: "name of last path_track of the top path".
    pub toptrack: String,
    /// `bottomtrack`: "name of first path_track of the bottom track".
    pub bottomtrack: String,
}

/// `path_corner`/`path_track`: the next node's name, a pause, and (for
/// `func_tracktrain`) a `path_track`-only speed override and stop flag. See
/// `docs/FORMAT_SOURCES.md` ("Track trains and paths") for the public
/// sources these last two fields were taken from.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Path {
    /// Seconds a follower waits at this node.
    pub wait: f32,
    /// `path_track`'s documented "New Train Speed": reassigns a
    /// `func_tracktrain`'s speed as it passes this node.
    pub speed: Option<f32>,
    /// The documented "Wait for retrigger" spawnflag: a `func_tracktrain`
    /// stops here until explicitly re-triggered, rather than continuing
    /// after `wait` seconds. See
    /// [`crate::track_train::path_stop_from_flags`].
    pub stop: bool,
}

/// `multi_manager`: every non-standard keyvalue is a `target -> delay`
/// pair (a `#N` suffix distinguishing repeated targets is stripped, per
/// the public documentation of how the entity is authored). Bounded to the
/// documented limit of 16 fan-out targets.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MultiManager {
    /// `(target, delay in seconds)`, in authored order.
    pub targets: Vec<(String, f32)>,
}

/// The documented cap on one `multi_manager`'s fan-out targets.
pub const MAX_MULTI_MANAGER_TARGETS: usize = 16;

/// `trigger_once`/`trigger_multiple` and any other `trigger_*` this crate
/// does not give a dedicated component to.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Trigger {
    /// `true` for `trigger_once` (fires at most once); `false` for
    /// `trigger_multiple` and other repeatable triggers.
    pub once: bool,
    /// Seconds before the trigger can fire again (ignored when `once`).
    pub wait: f32,
    /// Seconds between activation and firing `target`.
    pub delay: f32,
}

/// How many times the map logic has activated a `monstermaker` that has
/// not been consumed yet.
///
/// Mirrors [`crate::scripts::ScriptActivation`]'s role for scripting
/// entities: [`crate::logic::Simulation::activate`] bumps this counter
/// exactly like it opens a door, and `ohl-engine`'s AI phase (which owns
/// the `ohl_ai::Spawner` a `monstermaker` cannot see from this crate)
/// drains it and calls `Spawner::trigger` once per activation. No parallel
/// trigger system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MakerActivation {
    /// Activations not yet consumed.
    pub pending: u32,
}

impl MakerActivation {
    /// The most activations kept between two AI phases, so a pathological
    /// `multi_manager` chain cannot grow this without bound. Project-owned,
    /// matching [`crate::scripts::ScriptActivation::MAX_PENDING`].
    pub const MAX_PENDING: u32 = 64;

    /// Records one activation, saturating at [`Self::MAX_PENDING`].
    pub const fn activate(&mut self) {
        if self.pending < Self::MAX_PENDING {
            self.pending += 1;
        }
    }
}

/// `trigger_hurt`: a volume that damages whatever is inside it.
///
/// The published behaviour (TWHL's `trigger_hurt` page, see
/// `docs/FORMAT_SOURCES.md` "Player systems") is that `dmg` is the damage
/// *per second*, applied as a hit every half second worth half of it, and
/// that `damagetype` is a bit field whose values add up. A negative `dmg`
/// heals, which is the documented way mappers build healing pools.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerHurt {
    /// `dmg`: damage per second while the player is inside the volume.
    pub damage_per_second: f32,
    /// `damagetype`: the documented additive damage-type bit field, kept
    /// verbatim so `ohl-player` can classify the hit.
    pub damage_type: u32,
}

/// The documented interval, in seconds, between two `trigger_hurt` hits:
/// "a `trigger_hurt` performs a hit every 0.5 seconds, and the amount is
/// 0.5x `dmg` per hit".
pub const TRIGGER_HURT_INTERVAL_SECONDS: f32 = 0.5;

/// Marks a `func_ladder`: an invisible brush the player can climb. The
/// entity's own submodel is what a host must attach as a non-solid
/// `CONTENTS_LADDER` volume (`ohl_physics::CollisionModel::
/// attach_contents_brush`, `ohl_physics::ContentsKind::Ladder`) for that
/// climbing to actually work; this component only records that the entity
/// exists so a host can find it (see `crate::brush::contents_model_instances`)
/// or list/disable/move one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ladder;

/// Which of the three documented liquids a `func_water`'s `Contents (skin)`
/// keyvalue selects. TWHL wiki `func_water` (consulted via a search-engine
/// result summary of the page, same HTTP 403 caveat recorded elsewhere in
/// `docs/FORMAT_SOURCES.md`; reviewed 2026-09-07): the keyvalue's choices
/// are the raw `CONTENTS_*` enum values, `-3` water, `-4` slime, `-5` lava;
/// this project defaults to [`Self::Water`] when `skin` is absent or does
/// not parse to one of those three, since `-3` (water) is also the
/// documented default value of the underlying `skin` keyvalue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liquid {
    /// `skin` `-3` (or absent).
    Water,
    /// `skin` `-4`.
    Slime,
    /// `skin` `-5`.
    Lava,
}

impl Liquid {
    /// Reads the `skin` keyvalue the way `func_water` documents it (see
    /// [`Self`]'s doc comment).
    #[must_use]
    fn from_skin_keyvalue(def: &EntityDef) -> Self {
        match def.keyvalues.get("skin").map(|value| value.trim()) {
            Some("-4") => Self::Slime,
            Some("-5") => Self::Lava,
            _ => Self::Water,
        }
    }
}

/// Marks a `func_water`: a swimmable liquid volume, carrying which of the
/// three documented liquids it is (see [`Liquid`]). As with [`Ladder`], the
/// entity's own submodel is what a host must attach as a non-solid contents
/// volume (`ohl_physics::CollisionModel::attach_contents_brush`) for
/// swimming to actually work; see `crate::brush::contents_model_instances`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Water(pub Liquid);

/// The published `Remove On fire` spawnflag bit of `trigger_auto`.
pub const SPAWNFLAG_TRIGGER_AUTO_REMOVE_ON_FIRE: u32 = 1;

/// The published "USE Only" spawnflag bit of `trigger_changelevel`: per the
/// TWHL wiki page ("USE Only (2) - Entity can only be triggered by another
/// event"), when set the volume never fires from a player simply touching
/// it and only responds to `use`/being targeted; see
/// `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map logic").
pub const SPAWNFLAG_CHANGELEVEL_USE_ONLY: u32 = 2;

/// `trigger_auto`: "automatically fires its targets on map spawn".
///
/// Published behaviour (see `docs/FORMAT_SOURCES.md`, "Scripted sequences
/// and talk monsters"): it fires its `target` as soon as the map has
/// loaded, `delay` seconds later, and the `Remove On fire` spawnflag
/// removes the entity afterwards.
///
/// **`TODO(black-box)`**: the published `globalstate` key — "or as soon as
/// the specified global state becomes active" — is not modelled; a
/// `trigger_auto` here always fires on load.
///
/// **Save/load note**: [`Self::fired`] is one-shot state that lives only in
/// the entity world, so a save taken after a `trigger_auto` has fired must
/// persist it — otherwise restoring that save re-fires every auto trigger
/// on the map, replaying whatever they started (in particular, silently
/// re-toggling — and so stopping — any `func_train`/`trigger_camera` a
/// `trigger_auto` had started, exactly when a save section for *that*
/// state was about to make it resume correctly). `ohl-engine`'s M7.13
/// `SECTION_MOVER_STATE` (tag 28) now carries this flag, on this doc
/// comment's own invitation, alongside `ohl_game::logic::SimulationState`'s
/// own trigger cooldowns which already travel in a save for exactly the
/// same reason.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AutoTrigger {
    /// Seconds between map load and firing `target`.
    pub delay: f32,
    /// Whether the entity is removed once it has fired.
    pub remove_on_fire: bool,
    /// Whether it has already fired.
    pub fired: bool,
}

/// The documented `triggerstate` ("Trigger State") keyvalue of
/// `trigger_auto` and `trigger_relay`: *which* use type the entity sends
/// to the target it names, rather than what it targets.
///
/// Sven Co-op's own wiki entry for `trigger_auto`
/// (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic") documents
/// the key as "Changes state in which target will be triggered. On- turns
/// entity on; Off- Turns entity off; Toggle- turns entity On when it's Off
/// and vice versa", and Sven Co-op Manor's mirror of both entities gives
/// the three values as `0` Off, `1` On and `2` Toggle, adding for
/// `trigger_auto` that the "Trigger use-type is defined in 'Trigger State'
/// keyvalue".
///
/// A firing entity that declares no `triggerstate` keeps this project's
/// original behaviour, [`Self::Toggle`], so nothing that never named the
/// key changes. **That is knowingly not the documented default**: the same
/// Sven Co-op Manor `trigger_relay` page continues "This is set to 'Off' by
/// default, make sure to change this if you want it to be anything else."
/// Following it would turn every existing un-keyed fire in every map into
/// an *off*, which is a far larger behaviour change than this project has
/// evidence for — the cited TWHL page says most entities ignore the use
/// type and just toggle regardless, so an "Off" default would be
/// unobservable for them and a regression for the rest. Recorded as a
/// `TODO(black-box)` divergence in `docs/FORMAT_SOURCES.md` rather than
/// silently followed or silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerUse {
    /// `0` — "turns entity off".
    Off,
    /// `1` — "turns entity on".
    On,
    /// `2` — "turns entity On when it's Off and vice versa"; also what an
    /// entity that declares no `triggerstate` sends.
    #[default]
    Toggle,
}

impl TriggerUse {
    /// The documented value mapping above. Anything else — an absent key,
    /// a blank one, a number outside `0..=2` — is [`Self::Toggle`], which
    /// is both the documented "normal trigger" behaviour and what this
    /// project did before the key was read at all.
    #[must_use]
    pub fn from_keyvalue(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("0") => Self::Off,
            Some("1") => Self::On,
            _ => Self::Toggle,
        }
    }
}

/// The use type a `trigger_auto`/`trigger_relay` sends, when it declares a
/// `triggerstate` at all. Absent on every other entity, and on one that
/// leaves the key unset, so `Simulation` reads a missing component as
/// [`TriggerUse::Toggle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerUseType(pub TriggerUse);

/// Marks a `multisource`: the published "master" entity, documented as an
/// AND gate that "only triggers its target(s) if all entities targeting it
/// are in the 'ON' state".
///
/// This project's reading of "in the 'ON' state", recorded as project
/// behaviour rather than as a quotation: a targeting entity counts once it
/// has *fired* this `multisource`. See `docs/FORMAT_SOURCES.md`, "Masters
/// (`multisource`)", for the citation and for the divergences that reading
/// carries. Which entities target it is not stored here — it is the
/// `targetname` index run backwards, which `ohl_game::logic::Simulation`
/// computes from the registry when it evaluates a [`Master`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiSource;

/// An entity's `master` keyvalue: "the name of a `multisource` (or
/// `game_team_master`) entity. A master must usually be active in order
/// for the entity to work."
///
/// Attached to any entity that carries a non-empty `master`, whatever its
/// classname, since the key is documented on the whole `trigger_*`/mover
/// family rather than on one entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Master(pub String);

/// `trigger_teleport`'s published "No Clients" spawnflag bit: "Players
/// cannot activate this entity" (see `docs/FORMAT_SOURCES.md`, "Teleport
/// volumes and destinations").
pub const SPAWNFLAG_TELEPORT_NO_CLIENTS: u32 = 2;

/// Marks a `trigger_teleport`: the brush volume a player walks into to be
/// moved to wherever its `target` names.
///
/// Published behaviour (`docs/FORMAT_SOURCES.md`, "Teleport volumes and
/// destinations"): the volume "will teleport the player to the origin of
/// the target entity that was provided to it in its list of properties,
/// when the player touches it", its `target` is "the name of the
/// `info_teleport_destination` **or any other entity** to use as
/// destination", and a destination's own `angles` are "the angles at which
/// the entity will be facing upon teleportation".
///
/// The dispatch rides the ordinary [`Trigger`] machinery — the volume is
/// touched, and its `target` is fired after its own `delay` — with one
/// difference this component is what marks: the fire carries the volume
/// itself as its activator, and `ohl_game::logic::Simulation::activate`
/// treats *any* entity activated by a teleport volume as that teleport's
/// destination, whatever its classname. That is what makes "or any other
/// entity" work without the opposite error of teleporting the player every
/// time some unrelated chain happens to fire a marker entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeleportTrigger {
    /// The published "No Clients" spawnflag
    /// ([`SPAWNFLAG_TELEPORT_NO_CLIENTS`]): the player may not activate
    /// this volume.
    pub no_clients: bool,
}

/// `trigger_camera`'s documented "Start At Player" spawnflag: the sequence's
/// camera begins at the player's own current view instead of the entity's
/// placed `origin`/`angles`, when it has no `moveto` path (see
/// `docs/FORMAT_SOURCES.md`, "Camera sequences").
pub const SPAWNFLAG_CAMERA_START_AT_PLAYER: u32 = 1;

/// `trigger_camera`'s documented "Follow Player" spawnflag: the camera aims
/// at the player instead of at `target`.
pub const SPAWNFLAG_CAMERA_FOLLOW_PLAYER: u32 = 2;

/// `trigger_camera`'s documented "Freeze Player" spawnflag: player movement
/// is disabled while the sequence is active (documented as not affecting
/// mouse look).
pub const SPAWNFLAG_CAMERA_FREEZE_PLAYER: u32 = 4;

/// `trigger_camera`: a scripted view-override sequence, optionally moving
/// along a `path_corner`/`path_track` chain named by `moveto`.
///
/// Published behaviour and every keyvalue/spawnflag below are recorded in
/// `docs/FORMAT_SOURCES.md` ("Camera sequences"); the entity's own `target`
/// keyvalue (already a generic [`Target`] component every entity gets) does
/// double duty as the look-at entity and the completion target, per that
/// section.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TriggerCamera {
    /// `wait`: total seconds the sequence holds the player's view before
    /// reverting, regardless of path progress.
    ///
    /// **`TODO(black-box)`**: the default (`10.0`) is corroborated only by a
    /// search-engine summary of the published FGD, not a directly fetchable
    /// primary source; see `docs/FORMAT_SOURCES.md`.
    pub hold_seconds: f32,
    /// `moveto`: the first `path_corner`/`path_track` name, or empty for a
    /// stationary camera.
    pub move_to: String,
    /// `speed`: initial travel speed, units/second.
    pub speed: f32,
    /// `acceleration`: recorded but not applied; no public source documents
    /// how it combines with `speed` over a segment, so this project travels
    /// the path at a constant `speed`, the same choice already made for
    /// `func_tracktrain`'s undocumented `bank`/`wheels` formulas.
    pub acceleration: f32,
    /// `deceleration`: recorded but not applied; see [`Self::acceleration`].
    pub deceleration: f32,
    /// The documented "Start At Player" spawnflag.
    pub start_at_player: bool,
    /// The documented "Follow Player" spawnflag.
    pub follow_player: bool,
    /// The documented "Freeze Player" spawnflag.
    pub freeze_player: bool,
}

impl TriggerCamera {
    /// Reads the three documented spawnflags out of a raw `spawnflags`
    /// bitmask.
    #[must_use]
    pub const fn flags_from_raw(spawnflags: u32) -> (bool, bool, bool) {
        (
            spawnflags & SPAWNFLAG_CAMERA_START_AT_PLAYER != 0,
            spawnflags & SPAWNFLAG_CAMERA_FOLLOW_PLAYER != 0,
            spawnflags & SPAWNFLAG_CAMERA_FREEZE_PLAYER != 0,
        )
    }
}

/// Any classname not otherwise recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Unknown;

/// `worldspawn`'s map-wide keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Worldspawn {
    /// `skyname`.
    pub skyname: String,
    /// `wad`, split into individual package paths.
    pub wads: Vec<String>,
    /// `newunit`: the map starts a new unit, so the previous level's
    /// carried-over state is discarded rather than applied.
    pub newunit: bool,
}

/// Standard GoldSrc `angle` sentinel meaning "straight up".
const ANGLE_UP: f32 = -1.0;
/// Standard GoldSrc `angle` sentinel meaning "straight down".
const ANGLE_DOWN: f32 = -2.0;

/// Converts a mover's `angles`/`angle` keyvalue into a unit movement
/// direction, honouring the special "straight up"/"straight down" sentinel
/// values (`-1`/`-2`) that Half-Life's editors present as an "Up"/"Down"
/// choice in place of a numeric angle; any other value is a yaw in degrees,
/// counter-clockwise around `+Z` from `+X`, matching `info_player_start`'s
/// convention.
#[must_use]
pub fn movedir_from_angles(angles: Vec3) -> Vec3 {
    let yaw = angles.y;
    if (yaw - ANGLE_UP).abs() < f32::EPSILON {
        Vec3::Z
    } else if (yaw - ANGLE_DOWN).abs() < f32::EPSILON {
        -Vec3::Z
    } else {
        let radians = yaw.to_radians();
        Vec3::new(radians.cos(), radians.sin(), 0.0)
    }
}

/// `func_door_rotating`'s "Reverse Direction" spawnflag: reverses the sign
/// of [`Door::rotation_axis`]. TWHL wiki `func_door_rotating`
/// (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic";
/// search-summary citation, reviewed 2026-09-07, same HTTP 403 caveat as
/// the other TWHL citations in this crate).
pub const SPAWNFLAG_DOOR_ROTATING_REVERSE: u32 = 2;
/// `func_door_rotating`'s "One Way" spawnflag: the same TWHL page documents
/// it as "door only opens in the direction set in Distance", i.e. it
/// disables the activator-relative opening direction the same page
/// documents ("the door will always open away from the player") as the
/// default. Carried into [`RotatingDoorSwing::one_way`], which is what
/// `ohl_game::logic::Simulation::activate` gates the direction decision
/// on.
pub const SPAWNFLAG_DOOR_ROTATING_ONE_WAY: u32 = 16;
/// `func_door_rotating`'s "X Axis" spawnflag.
pub const SPAWNFLAG_DOOR_ROTATING_X_AXIS: u32 = 64;
/// `func_door_rotating`'s "Y Axis" spawnflag. Neither this nor
/// [`SPAWNFLAG_DOOR_ROTATING_X_AXIS`] set means the documented default,
/// `Z`.
pub const SPAWNFLAG_DOOR_ROTATING_Y_AXIS: u32 = 128;
/// `func_door_rotating`'s "Starts Open" spawnflag: the door spawns already
/// fully open (and swings *closed* on its first trigger) instead of
/// closed.
pub const SPAWNFLAG_DOOR_ROTATING_STARTS_OPEN: u32 = 1;

/// `func_door`/`func_door_rotating`'s "Use Only" spawnflag: the Sven
/// Co-op wiki's `Func_door` page (`https://wiki.svencoop.com/Func_door`,
/// fetched directly; see `docs/FORMAT_SOURCES.md` item 30) documents it as
/// "If set, this door can be triggered by using it but not by touching it
/// anymore. This does not outrule activation by triggering, though." — a
/// player's `use` press ([`crate::logic::find_usable_within`]/
/// `Simulation::use_entity`) and another entity's fire chain
/// (`Simulation::activate_trigger`/fan-out) still reach a door with this
/// flag set; only [`crate::logic::Simulation::touch_doors`]'s own touch
/// path excludes it. Carried on [`DoorUseOnly`].
///
/// The same page's touch rule is one sentence, not two independent ones:
/// "Func_doors are triggered on touch, unless they have a name, in
/// which's case they require to be triggered manually." A door's own
/// `targetname` is therefore *also* part of this cited rule, not a
/// separate, project-invented exclusion — see [`crate::logic::Simulation::
/// touch_doors`]'s own doc comment for where that half is implemented
/// (a [`crate::registry::TargetName`] check, not a spawnflag).
pub const SPAWNFLAG_DOOR_USE_ONLY: u32 = 256;
/// `func_door`/`func_door_rotating`'s "Passable" spawnflag: see
/// [`DoorPassable`]'s own doc comment for the cited wording. Carried on
/// [`DoorPassable`].
pub const SPAWNFLAG_DOOR_PASSABLE: u32 = 8;

/// `func_trackchange`/`func_trackautochange`'s "Auto Activate train"
/// spawnflag. Sven Co-op Manor's `func_trackautochange` entry
/// (`https://www.svenmanor.com/entity-guide/func_trackautochange`, fetched
/// directly, reviewed 2026-09-08) documents it as "train continues moving
/// instead of pausing after platform finishes movement"; TWHL's own
/// `func_trackautochange` page names the flag but leaves its description
/// blank. See [`crate::registry::TrackChange::auto_activate`] and
/// `docs/FORMAT_SOURCES.md` for what this project does with a platform
/// that does *not* set it.
pub const SPAWNFLAG_TRACK_CHANGE_AUTO_ACTIVATE: u32 = 1;
/// `func_trackchange`/`func_trackautochange`'s "Start at Bottom"
/// spawnflag: "platform starts at the bottom path_track rather than the
/// top".
pub const SPAWNFLAG_TRACK_CHANGE_START_AT_BOTTOM: u32 = 8;
/// `func_trackchange`/`func_trackautochange`'s "Rotate Only" spawnflag:
/// "platform rotates without traveling the specified altitude".
pub const SPAWNFLAG_TRACK_CHANGE_ROTATE_ONLY: u32 = 16;
/// `func_trackchange`/`func_trackautochange`'s "X Axis" spawnflag.
pub const SPAWNFLAG_TRACK_CHANGE_X_AXIS: u32 = 64;
/// `func_trackchange`/`func_trackautochange`'s "Y Axis" spawnflag. Neither
/// this nor [`SPAWNFLAG_TRACK_CHANGE_X_AXIS`] set means the documented
/// default, `Z`.
pub const SPAWNFLAG_TRACK_CHANGE_Y_AXIS: u32 = 128;

/// `func_rotating`'s "Start On" spawnflag: the brush is already spinning at
/// map spawn. TWHL wiki `func_rotating` (`docs/FORMAT_SOURCES.md`, "Entity
/// keyvalues and map logic"; same search-summary/403 caveat).
pub const SPAWNFLAG_ROTATING_START_ON: u32 = 1;
/// `func_rotating`'s "Reverse Direction" spawnflag: reverses the sign of
/// [`Rotator::axis`].
pub const SPAWNFLAG_ROTATING_REVERSE: u32 = 2;
/// `func_rotating`'s "X Axis" spawnflag.
pub const SPAWNFLAG_ROTATING_X_AXIS: u32 = 4;
/// `func_rotating`'s "Y Axis" spawnflag. Neither this nor
/// [`SPAWNFLAG_ROTATING_X_AXIS`] set means the documented default, `Z`.
pub const SPAWNFLAG_ROTATING_Y_AXIS: u32 = 8;

/// `func_rot_button`'s "Reverse direction" spawnflag: reverses the sign of
/// [`RotButton::axis`]. TWHL wiki `func_rot_button` (`docs/
/// FORMAT_SOURCES.md`, "Entity keyvalues and map logic"; search-summary
/// citation, same HTTP 403 caveat as the other TWHL citations in this
/// crate).
pub const SPAWNFLAG_ROT_BUTTON_REVERSE: u32 = 2;
/// `func_rot_button`'s "Toggle" spawnflag: "Allows button to be toggled.
/// Using the button toggles it between 'on' and 'off', each time
/// retriggering its target."
pub const SPAWNFLAG_ROT_BUTTON_TOGGLE: u32 = 32;
/// `func_rot_button`'s "X axis" spawnflag.
pub const SPAWNFLAG_ROT_BUTTON_X_AXIS: u32 = 64;
/// `func_rot_button`'s "Y axis" spawnflag. Neither this nor
/// [`SPAWNFLAG_ROT_BUTTON_X_AXIS`] set means the documented default, `Z`.
pub const SPAWNFLAG_ROT_BUTTON_Y_AXIS: u32 = 128;
/// `func_rot_button`'s "Touch activates" spawnflag: "The button can only be
/// activated by the player bumping into it (or by being shot, if Health is
/// > 0)."
pub const SPAWNFLAG_ROT_BUTTON_TOUCH: u32 = 256;

/// `momentary_rot_button`'s "Door Hack" spawnflag: see [`MomentaryRotButton::
/// door_hack`]'s own doc comment for this project's reading of its
/// documented "makes this entity solid, but un-USE-able" text. TWHL wiki
/// `momentary_rot_button` (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and
/// map logic"; same search-summary/403 caveat).
pub const SPAWNFLAG_MOMENTARY_DOOR_HACK: u32 = 1;
/// `momentary_rot_button`'s "Auto return" spawnflag: "Button returns to its
/// starting position automatically after use at the speed set in Auto-return
/// speed (returnspeed)."
pub const SPAWNFLAG_MOMENTARY_AUTO_RETURN: u32 = 16;
/// `momentary_rot_button`'s "X axis" spawnflag.
pub const SPAWNFLAG_MOMENTARY_X_AXIS: u32 = 64;
/// `momentary_rot_button`'s "Y axis" spawnflag. Neither this nor
/// [`SPAWNFLAG_MOMENTARY_X_AXIS`] set means the documented default, `Z`.
pub const SPAWNFLAG_MOMENTARY_Y_AXIS: u32 = 128;

/// `func_pendulum`'s "Start ON" spawnflag, directly documented (unlike
/// [`SPAWNFLAG_ROTATING_START_ON`], which is only this project's own
/// FGD-convention reading): TWHL wiki `func_pendulum` (`docs/
/// FORMAT_SOURCES.md`, "Entity keyvalues and map logic"; search-summary
/// citation, same HTTP 403 caveat as the other TWHL citations in this
/// crate) lists the entity's spawnflags as "1 for 'Start ON', 8 for
/// 'Passable', 16 for 'Auto-return', and 64 for 'X Axis'"; see
/// [`Pendulum::swinging`]'s own doc comment.
pub const SPAWNFLAG_PENDULUM_START_ON: u32 = 1;
/// `func_pendulum`'s "Auto return" spawnflag: "if this is enabled it will
/// cause the pendulum to return to its start position when triggered".
pub const SPAWNFLAG_PENDULUM_AUTO_RETURN: u32 = 16;
/// `func_pendulum`'s "X Axis" spawnflag.
pub const SPAWNFLAG_PENDULUM_X_AXIS: u32 = 64;
/// `func_pendulum`'s "Y Axis" spawnflag: "the Y-axis flag causes the swing
/// to be in the Y axis. The swing defaults to the Z axis if neither of the
/// axis flags is enabled." That quotation names the flag but not a bit
/// value for it; `128` is this project's own inference (the next bit after
/// [`SPAWNFLAG_PENDULUM_X_AXIS`]'s `64`, matching the `X`/`Y` pairing
/// already documented with actual numbers for `func_door_rotating`/
/// `func_rotating`/`momentary_rot_button`), the same kind of
/// not-independently-cited reading already recorded for
/// [`SPAWNFLAG_ROTATING_START_ON`]. Neither this nor
/// [`SPAWNFLAG_PENDULUM_X_AXIS`] set means the documented default, `Z`.
pub const SPAWNFLAG_PENDULUM_Y_AXIS: u32 = 128;

/// The signed unit rotation axis a `func_door_rotating`/`func_rotating`
/// spawnflag selection and a `reverse` bit describe: `x_axis`/`y_axis`
/// choose which world axis (`Z` when neither is set, the documented
/// default for both entities), and `reverse` negates it.
#[must_use]
fn rotation_axis(x_axis: bool, y_axis: bool, reverse: bool) -> Vec3 {
    let unit = if x_axis {
        Vec3::X
    } else if y_axis {
        Vec3::Y
    } else {
        Vec3::Z
    };
    if reverse { -unit } else { unit }
}

/// The [`RotatingDoorSwing::hinge_normal`] for a `func_door_rotating` whose
/// signed spawn axis is `base_axis` and whose submodel's compiled bounding
/// box is `model_bounds`' entry for its `*N` model index.
///
/// A rotating door's geometry is compiled *relative to its own origin
/// brush* — the pivot — rather than in absolute world space
/// (`docs/FORMAT_SOURCES.md` item 24, verified there against a real map's
/// `BSPMODEL::mins/maxs`), so the midpoint of those raw compiled bounds is
/// already the offset from the pivot to the door leaf's centre. The
/// direction that leaf first moves in under a small positive rotation is
/// then the rigid-body `v = axis x r`, normalized; a leaf centred on the
/// axis itself (a symmetric double leaf) leaves no side to prefer and
/// yields `Vec3::ZERO`.
#[must_use]
fn hinge_normal(
    def: &EntityDef,
    base_axis: Vec3,
    model_bounds: &BTreeMap<u32, ([f32; 3], [f32; 3])>,
) -> Vec3 {
    let Some(ModelRef::Brush(index)) = &def.model else {
        return Vec3::ZERO;
    };
    let Some((mins, maxs)) = model_bounds.get(index) else {
        return Vec3::ZERO;
    };
    let leaf = (Vec3::from_array(*mins) + Vec3::from_array(*maxs)) * 0.5;
    base_axis.cross(leaf).normalize_or_zero()
}

/// Clamps a float keyvalue field into `0..=255` for storage as a `u8`
/// (colour channels, style/sound indices), rounding towards zero the same
/// way GoldSrc's own integer keyvalue fields do.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_u8(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

fn parse_light_value(value: Option<&String>) -> (f32, [u8; 3]) {
    let Some(value) = value else {
        return (200.0, [255, 255, 255]);
    };
    let parts: Vec<f32> = value
        .split_ascii_whitespace()
        .filter_map(|part| part.parse::<f32>().ok())
        .collect();
    match parts.as_slice() {
        [brightness] => (*brightness, [255, 255, 255]),
        [r, g, b] => (200.0, [clamp_u8(*r), clamp_u8(*g), clamp_u8(*b)]),
        [r, g, b, brightness] => (*brightness, [clamp_u8(*r), clamp_u8(*g), clamp_u8(*b)]),
        _ => (200.0, [255, 255, 255]),
    }
}

fn strip_multi_manager_suffix(key: &str) -> &str {
    match key.rfind('#') {
        Some(index) => &key[..index],
        None => key,
    }
}

fn brush_travel_distance(
    def: &EntityDef,
    movedir: Vec3,
    lip: f32,
    model_bounds: &BTreeMap<u32, ([f32; 3], [f32; 3])>,
) -> f32 {
    let Some(ModelRef::Brush(index)) = &def.model else {
        return 0.0;
    };
    let Some((mins, maxs)) = model_bounds.get(index) else {
        return 0.0;
    };
    let size = Vec3::new(maxs[0] - mins[0], maxs[1] - mins[1], maxs[2] - mins[2]);
    let projected = size.x * movedir.x.abs() + size.y * movedir.y.abs() + size.z * movedir.z.abs();
    (projected - lip).max(0.0)
}

/// The entity registry: a [`hecs::World`] plus a bounded name index.
pub struct Registry {
    /// The entity-component world.
    pub world: World,
    name_index: BTreeMap<String, Vec<Entity>>,
    /// `worldspawn`'s keys, when the map had a `worldspawn` entity.
    pub worldspawn: Option<Worldspawn>,
    /// The `entity index within `defs`, matching hecs `entity` -> the
    /// spawn order, kept so callers can align with brush-model indices.
    pub entities: Vec<Entity>,
}

impl Registry {
    /// Builds a registry from parsed entity definitions.
    ///
    /// `model_bounds` maps a brush submodel index (`ohl_formats::bsp30`'s
    /// `Model` slot, the same index a `*N` `model` keyvalue names) to its
    /// `(mins, maxs)` bounding box, used to derive door/platform travel
    /// distances; pass an empty map when that data is unavailable (travel
    /// distance then falls back to `0`, a safe default that trigger and
    /// timing logic can still exercise).
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn build(
        defs: &[EntityDef],
        model_bounds: &BTreeMap<u32, ([f32; 3], [f32; 3])>,
        limits: &Limits,
    ) -> Self {
        let mut world = World::new();
        let mut name_index: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
        let mut worldspawn = None;
        let mut entities = Vec::with_capacity(defs.len());

        for def in defs {
            let transform = Transform {
                origin: Vec3::from_array(def.origin),
                angles: Vec3::from_array(def.angles),
            };
            let entity = world.spawn((
                ClassName(def.classname.clone()),
                transform,
                SpawnFlags(def.spawnflags),
                def.render,
            ));
            entities.push(entity);

            if let Some(name) = &def.targetname {
                world.insert_one(entity, TargetName(name.clone())).ok();
                let bucket = name_index.entry(name.clone()).or_default();
                if bucket.len() < MAX_ENTITIES_PER_NAME {
                    bucket.push(entity);
                }
            }
            if let Some(target) = &def.target {
                world.insert_one(entity, Target(target.clone())).ok();
            }
            if let Some(ModelRef::Brush(index)) = &def.model {
                world.insert_one(entity, BrushModel(*index)).ok();
                if let Some((mins, maxs)) = model_bounds.get(index) {
                    // Plus the `origin` keyvalue, unconditionally: see
                    // `BrushCenter`'s own doc comment for why that is
                    // right for a submodel compiled in absolute world
                    // space as well as for one compiled relative to an
                    // origin brush.
                    let center = Vec3::new(
                        f32::midpoint(mins[0], maxs[0]),
                        f32::midpoint(mins[1], maxs[1]),
                        f32::midpoint(mins[2], maxs[2]),
                    ) + Vec3::from_array(def.origin);
                    world.insert_one(entity, BrushCenter(center)).ok();
                    world
                        .insert_one(
                            entity,
                            BrushBounds {
                                mins: Vec3::from_array(*mins) + Vec3::from_array(def.origin),
                                maxs: Vec3::from_array(*maxs) + Vec3::from_array(def.origin),
                            },
                        )
                        .ok();
                }
            }
            if let Some(master) = def
                .keyvalues
                .get("master")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            {
                world.insert_one(entity, Master(master.to_string())).ok();
            }
            if let Some(global) = def
                .keyvalues
                .get("globalname")
                .filter(|value| !value.is_empty())
            {
                world.insert_one(entity, GlobalName(global.clone())).ok();
            }
            // The documented `triggerstate` of the two entities whose
            // published pages carry it: `trigger_auto` and
            // `trigger_relay` (see [`TriggerUse`]). Read here rather than
            // in either classname's own arm below because the key means
            // the same thing on both, and attached only when the entity
            // actually declares it, so an entity that does not is read as
            // the documented default by its absence.
            if matches!(def.classname.as_str(), "trigger_auto" | "trigger_relay")
                && let Some(raw) = def
                    .keyvalues
                    .get("triggerstate")
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
            {
                world
                    .insert_one(entity, TriggerUseType(TriggerUse::from_keyvalue(Some(raw))))
                    .ok();
            }

            match def.classname.as_str() {
                "worldspawn" => {
                    let wads = def
                        .keyvalues
                        .get("wad")
                        .map(|value| keyvalues::parse_wad_list(value, limits))
                        .unwrap_or_default();
                    worldspawn = Some(Worldspawn {
                        skyname: def.keyvalues.get("skyname").cloned().unwrap_or_default(),
                        wads,
                        newunit: def
                            .keyvalues
                            .get("newunit")
                            .is_some_and(|value| value.trim() != "0" && !value.trim().is_empty()),
                    });
                }
                "func_door" => {
                    let lip = def
                        .keyvalues
                        .get("lip")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let movedir = movedir_from_angles(transform.angles);
                    let travel = brush_travel_distance(def, movedir, lip, model_bounds);
                    let door = Door {
                        speed: numeric(def, "speed", 100.0),
                        wait: numeric(def, "wait", 4.0),
                        lip,
                        movedir,
                        dmg: numeric(def, "dmg", 0.0),
                        health: numeric(def, "health", 0.0),
                        delay: numeric(def, "delay", 0.0),
                        sounds: (
                            clamp_u8(numeric(def, "movesnd", 0.0)),
                            clamp_u8(numeric(def, "stopsnd", 0.0)),
                        ),
                        travel_distance: travel,
                        rotation_axis: None,
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, door).ok();
                    if def.spawnflags & SPAWNFLAG_DOOR_USE_ONLY != 0 {
                        world.insert_one(entity, DoorUseOnly).ok();
                    }
                    if def.spawnflags & SPAWNFLAG_DOOR_PASSABLE != 0 {
                        world.insert_one(entity, DoorPassable).ok();
                    }
                }
                // `func_door_rotating`: TWHL wiki `func_door_rotating`
                // (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
                // logic"). Shares `Door`'s timer/state machine (`speed`,
                // `wait`, `dmg`, `health`, `delay`, sounds) with `func_door`
                // above, but swings about its origin-keyvalue pivot instead
                // of sliding: `travel_distance` holds the `distance`
                // keyvalue itself (degrees, not units; `lip` is documented
                // "Not used" and so is left unread here), and
                // `rotation_axis` is `Some`, its sign carrying both a
                // negative `distance` and the "Reverse Direction"
                // spawnflag.
                //
                // Without "One Way" (see
                // `SPAWNFLAG_DOOR_ROTATING_ONE_WAY`'s own doc comment) the
                // same page documents the door as opening "away from the
                // player", so this arm also records a `RotatingDoorSwing`:
                // the spawnflag-chosen axis, the door leaf's own swing
                // normal, and the "One Way" bit. `ohl_game::logic::
                // Simulation::activate` flips `Door::rotation_axis`'s sign
                // from those on each open, which is also how the choice
                // persists (see `RotatingDoorSwing`'s own doc comment).
                "func_door_rotating" => {
                    let flags = def.spawnflags;
                    let reverse = flags & SPAWNFLAG_DOOR_ROTATING_REVERSE != 0;
                    let x_axis = flags & SPAWNFLAG_DOOR_ROTATING_X_AXIS != 0;
                    let y_axis = flags & SPAWNFLAG_DOOR_ROTATING_Y_AXIS != 0;
                    let starts_open = flags & SPAWNFLAG_DOOR_ROTATING_STARTS_OPEN != 0;
                    let distance = numeric(def, "distance", 90.0);
                    let axis = rotation_axis(x_axis, y_axis, reverse != (distance < 0.0));
                    let speed = numeric(def, "speed", 100.0);
                    let wait = numeric(def, "wait", 4.0);
                    let travel = distance.abs();
                    let (state, timer) = if starts_open {
                        (MoverState::Open, if wait < 0.0 { 0.0 } else { wait })
                    } else {
                        (MoverState::Closed, 0.0)
                    };
                    let door = Door {
                        speed,
                        wait,
                        lip: 0.0,
                        movedir: Vec3::ZERO,
                        dmg: numeric(def, "dmg", 0.0),
                        health: numeric(def, "health", 0.0),
                        delay: numeric(def, "delay", 0.0),
                        sounds: (
                            clamp_u8(numeric(def, "movesnd", 0.0)),
                            clamp_u8(numeric(def, "stopsnd", 0.0)),
                        ),
                        travel_distance: travel,
                        rotation_axis: Some(axis),
                        state,
                        timer,
                    };
                    world.insert_one(entity, door).ok();
                    if flags & SPAWNFLAG_DOOR_USE_ONLY != 0 {
                        world.insert_one(entity, DoorUseOnly).ok();
                    }
                    if flags & SPAWNFLAG_DOOR_PASSABLE != 0 {
                        world.insert_one(entity, DoorPassable).ok();
                    }
                    world
                        .insert_one(
                            entity,
                            RotatingDoorSwing {
                                base_axis: axis,
                                hinge_normal: hinge_normal(def, axis, model_bounds),
                                one_way: flags & SPAWNFLAG_DOOR_ROTATING_ONE_WAY != 0,
                            },
                        )
                        .ok();
                }
                // `func_rotating`: TWHL wiki `func_rotating` (`docs/
                // FORMAT_SOURCES.md`, "Entity keyvalues and map logic"): a
                // continuous spin rather than an open/close cycle, so it
                // gets its own `Rotator` component/state instead of reusing
                // `Door`.
                "func_rotating" => {
                    let flags = def.spawnflags;
                    let reverse = flags & SPAWNFLAG_ROTATING_REVERSE != 0;
                    let x_axis = flags & SPAWNFLAG_ROTATING_X_AXIS != 0;
                    let y_axis = flags & SPAWNFLAG_ROTATING_Y_AXIS != 0;
                    let start_on = flags & SPAWNFLAG_ROTATING_START_ON != 0;
                    let rotator = Rotator {
                        axis: rotation_axis(x_axis, y_axis, reverse),
                        speed: numeric(def, "speed", 100.0).abs(),
                        spinning: start_on,
                        angle_deg: 0.0,
                    };
                    world.insert_one(entity, rotator).ok();
                }
                // `func_rot_button`: TWHL wiki `func_rot_button` (`docs/
                // FORMAT_SOURCES.md`, "Entity keyvalues and map logic").
                // Shares `func_button`'s press/fire-target/auto-reset shape
                // (`wait`, `health`, `delay`, `sounds`) but rotates about an
                // origin-keyvalue pivot, so it gets its own `RotButton`
                // component instead of reusing `Button` (which has no
                // rotation axis) or `Door` (which never fires `target`).
                "func_rot_button" => {
                    let flags = def.spawnflags;
                    let reverse = flags & SPAWNFLAG_ROT_BUTTON_REVERSE != 0;
                    let x_axis = flags & SPAWNFLAG_ROT_BUTTON_X_AXIS != 0;
                    let y_axis = flags & SPAWNFLAG_ROT_BUTTON_Y_AXIS != 0;
                    let button = RotButton {
                        axis: rotation_axis(x_axis, y_axis, reverse),
                        speed: numeric(def, "speed", 100.0).abs(),
                        distance: numeric(def, "distance", 90.0).abs(),
                        wait: numeric(def, "wait", 1.0),
                        health: numeric(def, "health", 0.0),
                        delay: numeric(def, "delay", 0.0),
                        sound: clamp_u8(numeric(def, "sounds", 0.0)),
                        toggle: flags & SPAWNFLAG_ROT_BUTTON_TOGGLE != 0,
                        touch: flags & SPAWNFLAG_ROT_BUTTON_TOUCH != 0,
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, button).ok();
                }
                // `momentary_rot_button`: TWHL wiki `momentary_rot_button`
                // (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
                // logic"). No documented "Reverse direction" spawnflag (see
                // `MomentaryRotButton::axis`'s own doc comment), so only the
                // axis-selection flags feed `rotation_axis`.
                "momentary_rot_button" => {
                    let flags = def.spawnflags;
                    let x_axis = flags & SPAWNFLAG_MOMENTARY_X_AXIS != 0;
                    let y_axis = flags & SPAWNFLAG_MOMENTARY_Y_AXIS != 0;
                    let button = MomentaryRotButton {
                        axis: rotation_axis(x_axis, y_axis, false),
                        speed: numeric(def, "speed", 100.0).abs(),
                        distance: numeric(def, "distance", 90.0).abs().max(1.0),
                        return_speed: numeric(def, "returnspeed", 100.0).abs(),
                        auto_return: flags & SPAWNFLAG_MOMENTARY_AUTO_RETURN != 0,
                        door_hack: flags & SPAWNFLAG_MOMENTARY_DOOR_HACK != 0,
                        fraction: 0.0,
                        moving_forward: true,
                        returning: false,
                    };
                    world.insert_one(entity, button).ok();
                }
                // `momentary_door`: TWHL wiki `momentary_rot_button`
                // (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
                // logic", item 29): shares `func_door`'s translating
                // `speed`/`lip`/`movedir`/`travel_distance` shape (see
                // `MomentaryDoor`'s own doc comment for why it is not a
                // `Door` itself), but never gets a `state`/`timer`
                // open-close cycle: only `Simulation::
                // drive_momentary_rot_button` ever moves `fraction`.
                // `func_breakable`/`func_pushable`: TWHL wiki
                // `func_breakable`/`func_pushable` (`docs/
                // FORMAT_SOURCES.md`, item 32). Both carry a `Breakable`;
                // a `func_pushable` additionally carries a `Pushable`, and
                // is only breakable by damage when its own documented
                // "Breakable (128)" flag is set.
                "func_breakable" | "func_pushable" => {
                    let pushable = def.classname == "func_pushable";
                    let flags = def.spawnflags;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let material = numeric(def, "material", 0.0).clamp(0.0, 255.0) as u8;
                    let breakable_flag = !pushable || flags & SPAWNFLAG_PUSHABLE_BREAKABLE != 0;
                    let breakable = Breakable {
                        // A `func_pushable` without the documented
                        // "Breakable" flag keeps no hit points at all, so
                        // no damage path can ever reach it: the cited page
                        // states its own `health` applies only "If
                        // breakable".
                        health: if breakable_flag {
                            numeric(def, "health", 0.0).max(0.0)
                        } else {
                            0.0
                        },
                        material,
                        // A `func_pushable`'s "Only Trigger"/"Touch"/
                        // "Pressure" bits are not documented for it at all
                        // (its own cited flag list has one entry,
                        // "Breakable (128)"), and the same page records
                        // that a pushable "Can't be broken by impact like a
                        // func_breakable does with 'Pressure' on" — so only
                        // a real `func_breakable` reads those three bits.
                        trigger_only: !pushable && flags & SPAWNFLAG_BREAKABLE_ONLY_TRIGGER != 0,
                        break_on_touch: !pushable && flags & SPAWNFLAG_BREAKABLE_TOUCH != 0,
                        break_on_pressure: !pushable && flags & SPAWNFLAG_BREAKABLE_PRESSURE != 0,
                        instant_crowbar: breakable_flag
                            && flags & SPAWNFLAG_BREAKABLE_INSTANT_CROWBAR != 0,
                        delay: numeric(def, "delay", 0.0).max(0.0),
                        broken: false,
                    };
                    world.insert_one(entity, breakable).ok();
                    if pushable {
                        world
                            .insert_one(
                                entity,
                                Pushable {
                                    friction: numeric(def, "friction", 0.0)
                                        .clamp(0.0, PUSHABLE_MAX_FRICTION),
                                    breakable: breakable_flag,
                                    offset: Vec3::ZERO,
                                },
                            )
                            .ok();
                    }
                }
                "momentary_door" => {
                    let lip = def
                        .keyvalues
                        .get("lip")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let movedir = movedir_from_angles(transform.angles);
                    let travel = brush_travel_distance(def, movedir, lip, model_bounds);
                    let door = MomentaryDoor {
                        speed: numeric(def, "speed", 100.0).abs(),
                        lip,
                        movedir,
                        travel_distance: travel,
                        fraction: 0.0,
                    };
                    world.insert_one(entity, door).ok();
                }
                // `func_pendulum`: TWHL wiki `func_pendulum` (`docs/
                // FORMAT_SOURCES.md`, "Entity keyvalues and map logic").
                "func_pendulum" => {
                    let flags = def.spawnflags;
                    let x_axis = flags & SPAWNFLAG_PENDULUM_X_AXIS != 0;
                    let y_axis = flags & SPAWNFLAG_PENDULUM_Y_AXIS != 0;
                    let start_on = flags & SPAWNFLAG_PENDULUM_START_ON != 0;
                    let pendulum = Pendulum {
                        axis: rotation_axis(x_axis, y_axis, false),
                        distance: numeric(def, "distance", 90.0).abs(),
                        speed: numeric(def, "speed", 100.0).abs(),
                        damping: numeric(def, "damping", 0.0).clamp(0.0, 1000.0),
                        auto_return: flags & SPAWNFLAG_PENDULUM_AUTO_RETURN != 0,
                        swinging: start_on,
                        elapsed: 0.0,
                        returning: false,
                        angle_deg: 0.0,
                    };
                    world.insert_one(entity, pendulum).ok();
                }
                "func_button" => {
                    let button = Button {
                        speed: numeric(def, "speed", 40.0),
                        wait: numeric(def, "wait", 1.0),
                        health: numeric(def, "health", 0.0),
                        delay: numeric(def, "delay", 0.0),
                        sound: clamp_u8(numeric(def, "sounds", 0.0)),
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, button).ok();
                }
                "func_plat" | "func_platform" => {
                    let lip = def
                        .keyvalues
                        .get("lip")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let movedir = def
                        .keyvalues
                        .get("angles")
                        .or_else(|| def.keyvalues.get("angle"))
                        .map_or(-Vec3::Z, |_| movedir_from_angles(transform.angles));
                    let explicit_height = def
                        .keyvalues
                        .get("height")
                        .and_then(|v| v.trim().parse::<f32>().ok());
                    let travel = explicit_height
                        .unwrap_or_else(|| brush_travel_distance(def, movedir, lip, model_bounds));
                    let platform = Platform {
                        speed: numeric(def, "speed", 150.0),
                        wait: numeric(def, "wait", 3.0),
                        movedir,
                        travel_distance: travel.max(0.0),
                        sounds: (
                            clamp_u8(numeric(def, "movesnd", 0.0)),
                            clamp_u8(numeric(def, "stopsnd", 0.0)),
                        ),
                        state: MoverState::Closed,
                        timer: 0.0,
                    };
                    world.insert_one(entity, platform).ok();
                }
                "light" | "light_spot" | "light_environment" => {
                    let (brightness, color) = parse_light_value(
                        def.keyvalues
                            .get("_light")
                            .or_else(|| def.keyvalues.get("light")),
                    );
                    let light = Light {
                        brightness,
                        color,
                        style: clamp_u8(numeric(def, "style", 0.0)),
                        cone: def
                            .keyvalues
                            .get("_cone")
                            .and_then(|v| v.trim().parse::<f32>().ok()),
                    };
                    world.insert_one(entity, light).ok();
                }
                "info_player_start" => {
                    world.insert_one(entity, PlayerStart).ok();
                }
                "info_landmark" => {
                    world.insert_one(entity, Landmark).ok();
                }
                "trigger_changelevel" => {
                    let change = ChangeLevel {
                        map: def.keyvalues.get("map").cloned().unwrap_or_default(),
                        landmark: def.keyvalues.get("landmark").cloned().unwrap_or_default(),
                    };
                    world.insert_one(entity, change).ok();
                    // Also touch-capable, exactly like the generic
                    // `trigger_*` arm below, unless "USE Only" is set (see
                    // `SPAWNFLAG_CHANGELEVEL_USE_ONLY`): the fields on this
                    // `Trigger` are not read by
                    // `Simulation::touch_changelevel_triggers` (which does
                    // its own edge-triggered one-shot bookkeeping), only
                    // its presence, so `wait`/`delay` are recorded for
                    // documentation purposes even though nothing consumes
                    // them today.
                    if def.spawnflags & SPAWNFLAG_CHANGELEVEL_USE_ONLY == 0 {
                        world
                            .insert_one(
                                entity,
                                Trigger {
                                    once: false,
                                    wait: numeric(def, "wait", 0.2),
                                    delay: numeric(def, "delay", 0.0),
                                },
                            )
                            .ok();
                    }
                }
                "trigger_transition" => {
                    world.insert_one(entity, TransitionVolume).ok();
                }
                "env_global" => {
                    let env_global = EnvGlobal {
                        global_state: def
                            .keyvalues
                            .get("globalstate")
                            .cloned()
                            .unwrap_or_default(),
                        trigger_mode: global_state_value(def.keyvalues.get("triggermode")),
                        initial_state: global_state_value(def.keyvalues.get("initialstate"))
                            .unwrap_or_default(),
                        sets_initial_state: def.spawnflags & 1 != 0,
                    };
                    world.insert_one(entity, env_global).ok();
                }
                "env_message" | "game_text" => {
                    let message = Message {
                        message: def.keyvalues.get("message").cloned().unwrap_or_default(),
                        literal: def.classname == "game_text",
                        fadein: optional_numeric(def, "fadein"),
                        fadeout: optional_numeric(def, "fadeout"),
                        holdtime: optional_numeric(def, "holdtime"),
                    };
                    world.insert_one(entity, message).ok();
                }
                "path_corner" | "path_track" => {
                    let path = Path {
                        wait: numeric(def, "wait", 0.0),
                        speed: def
                            .keyvalues
                            .get("speed")
                            .and_then(|v| v.trim().parse::<f32>().ok())
                            .and_then(crate::track_train::path_speed_override),
                        stop: crate::track_train::path_stop_from_flags(def.spawnflags),
                    };
                    world.insert_one(entity, path).ok();
                    if let Some(message) = def
                        .keyvalues
                        .get("message")
                        .map(|message| message.trim())
                        .filter(|message| !message.is_empty())
                    {
                        world
                            .insert_one(entity, PathFireOnPass(message.to_string()))
                            .ok();
                    }
                    if let Some(netname) = def
                        .keyvalues
                        .get("netname")
                        .map(|netname| netname.trim())
                        .filter(|netname| !netname.is_empty())
                    {
                        world
                            .insert_one(entity, PathFireOnDeadEnd(netname.to_string()))
                            .ok();
                    }
                }
                "func_trackchange" | "func_trackautochange" => {
                    let flags = def.spawnflags;
                    let axis = if flags & SPAWNFLAG_TRACK_CHANGE_X_AXIS != 0 {
                        Vec3::X
                    } else if flags & SPAWNFLAG_TRACK_CHANGE_Y_AXIS != 0 {
                        Vec3::Y
                    } else {
                        Vec3::Z
                    };
                    let change = TrackChange {
                        height: numeric(def, "height", 0.0).abs(),
                        rotation: numeric(def, "rotation", 0.0),
                        speed: numeric(def, "speed", 0.0).abs(),
                        axis,
                        spawnflags: flags,
                        displaced: 0.0,
                        direction: 1.0,
                        moving: false,
                        carrying: false,
                        carry_from: Vec3::ZERO,
                        carry_to: Vec3::ZERO,
                    };
                    let links = TrackChangeLinks {
                        train: def.keyvalues.get("train").cloned().unwrap_or_default(),
                        toptrack: def.keyvalues.get("toptrack").cloned().unwrap_or_default(),
                        bottomtrack: def
                            .keyvalues
                            .get("bottomtrack")
                            .cloned()
                            .unwrap_or_default(),
                    };
                    world.insert_one(entity, change).ok();
                    world.insert_one(entity, links).ok();
                }
                "func_train" | "func_tracktrain" => {
                    let train = crate::track_train::TrackTrain {
                        turns_to_face: def.classname == "func_tracktrain",
                        speed: numeric(def, "speed", 100.0),
                        start_speed: numeric(def, "startspeed", 0.0),
                        height: numeric(def, "height", 4.0),
                        bank: numeric(def, "bank", 0.0),
                        dmg: numeric(def, "dmg", 0.0),
                        wheels: numeric(def, "wheels", 0.0),
                        no_user_control: crate::track_train::TrackTrain::no_user_control_from_flags(
                            def.spawnflags,
                        ),
                    };
                    world.insert_one(entity, train).ok();
                }
                "multi_manager" => {
                    let reserved = [
                        "classname",
                        "origin",
                        "angles",
                        "angle",
                        "targetname",
                        "target",
                        "spawnflags",
                        "model",
                        "rendermode",
                        "renderamt",
                        "rendercolor",
                    ];
                    let mut targets = Vec::new();
                    for (key, value) in &def.keyvalues {
                        if reserved.contains(&key.as_str()) {
                            continue;
                        }
                        if let Ok(delay) = value.trim().parse::<f32>() {
                            if targets.len() >= MAX_MULTI_MANAGER_TARGETS {
                                break;
                            }
                            targets.push((strip_multi_manager_suffix(key).to_string(), delay));
                        }
                    }
                    world.insert_one(entity, MultiManager { targets }).ok();
                }
                "func_ladder" => {
                    world.insert_one(entity, Ladder).ok();
                }
                "func_water" => {
                    world
                        .insert_one(entity, Water(Liquid::from_skin_keyvalue(def)))
                        .ok();
                }
                "trigger_auto" => {
                    world
                        .insert_one(
                            entity,
                            AutoTrigger {
                                delay: numeric(def, "delay", 0.0),
                                remove_on_fire: def.spawnflags
                                    & SPAWNFLAG_TRIGGER_AUTO_REMOVE_ON_FIRE
                                    != 0,
                                fired: false,
                            },
                        )
                        .ok();
                }
                "trigger_hurt" => {
                    world
                        .insert_one(
                            entity,
                            Trigger {
                                once: false,
                                wait: numeric(def, "wait", 0.2),
                                delay: numeric(def, "delay", 0.0),
                            },
                        )
                        .ok();
                    let hurt = TriggerHurt {
                        damage_per_second: numeric(def, "dmg", 0.0),
                        damage_type: def
                            .keyvalues
                            .get("damagetype")
                            .and_then(|value| value.trim().parse::<u32>().ok())
                            .unwrap_or(0),
                    };
                    world.insert_one(entity, hurt).ok();
                }
                "multisource" => {
                    world.insert_one(entity, MultiSource).ok();
                }
                "trigger_teleport" => {
                    world
                        .insert_one(
                            entity,
                            Trigger {
                                once: false,
                                wait: numeric(def, "wait", 0.2),
                                delay: numeric(def, "delay", 0.0),
                            },
                        )
                        .ok();
                    world
                        .insert_one(
                            entity,
                            TeleportTrigger {
                                no_clients: def.spawnflags & SPAWNFLAG_TELEPORT_NO_CLIENTS != 0,
                            },
                        )
                        .ok();
                }
                "trigger_camera" => {
                    let (start_at_player, follow_player, freeze_player) =
                        TriggerCamera::flags_from_raw(def.spawnflags);
                    let camera = TriggerCamera {
                        hold_seconds: numeric(def, "wait", 10.0),
                        move_to: text_field(def, "moveto"),
                        speed: numeric(def, "speed", 0.0),
                        acceleration: numeric(def, "acceleration", 500.0),
                        deceleration: numeric(def, "deceleration", 500.0),
                        start_at_player,
                        follow_player,
                        freeze_player,
                    };
                    world.insert_one(entity, camera).ok();
                }
                name if name.starts_with("trigger_") => {
                    let trigger = Trigger {
                        once: name == "trigger_once",
                        wait: numeric(def, "wait", 0.2),
                        delay: numeric(def, "delay", 0.0),
                    };
                    world.insert_one(entity, trigger).ok();
                }
                _ => {
                    world.insert_one(entity, Unknown).ok();
                }
            }
        }

        let mut registry = Self {
            world,
            name_index,
            worldspawn,
            entities,
        };
        // Second pass: a train's first `path_track`/`path_corner` node
        // commonly appears later in the entities lump than the train
        // itself, so the whole `targetname` index must exist first.
        crate::track_train::spawn_all(&mut registry);
        crate::camera::spawn_all(&mut registry);
        registry
    }

    /// Registers an entity spawned after [`Self::build`] (for example one
    /// carried in from the previous map across a level transition) in the
    /// name index, under the same [`MAX_ENTITIES_PER_NAME`] bound.
    ///
    /// Passing `None` records nothing: an unnamed entity is not indexed.
    pub fn index(&mut self, entity: Entity, name: Option<&str>) {
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            return;
        };
        let bucket = self.name_index.entry(name.to_string()).or_default();
        if bucket.len() < MAX_ENTITIES_PER_NAME {
            bucket.push(entity);
        }
    }

    /// Entities whose `targetname` is `name`, bounded to
    /// [`MAX_ENTITIES_PER_NAME`].
    #[must_use]
    pub fn find(&self, name: &str) -> &[Entity] {
        self.name_index
            .get(name)
            .map_or(&[] as &[Entity], Vec::as_slice)
    }
}

/// Reads a documented `env_global` mode keyvalue (`triggermode`,
/// `initialstate`): `0` off, `1` on, `2` dead, anything else (including the
/// documented `3` "toggle" `triggermode`) `None`.
fn global_state_value(value: Option<&String>) -> Option<GlobalStateValue> {
    match value?.trim() {
        "0" => Some(GlobalStateValue::Off),
        "1" => Some(GlobalStateValue::On),
        "2" => Some(GlobalStateValue::Dead),
        _ => None,
    }
}

/// A finite numeric keyvalue, or `None` when the entity does not set it.
fn optional_numeric(def: &EntityDef, key: &str) -> Option<f32> {
    def.keyvalues
        .get(key)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
}

/// `key`'s trimmed text, or `""`.
fn text_field(def: &EntityDef, key: &str) -> String {
    def.keyvalues
        .get(key)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

fn numeric(def: &EntityDef, key: &str, default: f32) -> f32 {
    def.keyvalues
        .get(key)
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvalues::parse_entities;
    use ohl_formats::bsp30::Entity as RawEntity;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn builds_door_component_and_name_index() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("targetname", "door1"),
            ("angle", "90"),
            ("speed", "50"),
            ("wait", "2"),
            ("lip", "8"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let found = registry.find("door1");
        assert_eq!(found.len(), 1);
        let door = registry.world.get::<&Door>(found[0]).expect("door");
        assert!((door.speed - 50.0).abs() < f32::EPSILON);
        assert!((door.wait - 2.0).abs() < f32::EPSILON);
        assert!((door.lip - 8.0).abs() < f32::EPSILON);
        assert!((door.movedir - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-4);
    }

    /// `func_door`'s "Use Only" spawnflag (256; `SPAWNFLAG_DOOR_USE_ONLY`)
    /// attaches [`DoorUseOnly`]; leaving it unset does not.
    #[test]
    fn use_only_spawnflag_attaches_door_use_only_marker() {
        let entities = vec![
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door1"),
                ("spawnflags", &SPAWNFLAG_DOOR_USE_ONLY.to_string()),
            ]),
            raw(&[("classname", "func_door"), ("targetname", "door2")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let use_only = registry.find("door1")[0];
        let plain = registry.find("door2")[0];
        assert!(registry.world.get::<&DoorUseOnly>(use_only).is_ok());
        assert!(registry.world.get::<&DoorUseOnly>(plain).is_err());
    }

    /// The same spawnflag, read off `func_door_rotating` instead.
    #[test]
    fn use_only_spawnflag_attaches_door_use_only_marker_on_rotating_door() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("model", "*1"),
            ("spawnflags", &SPAWNFLAG_DOOR_USE_ONLY.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("door1")[0];
        assert!(registry.world.get::<&DoorUseOnly>(entity).is_ok());
    }

    /// `func_door`'s "Passable" spawnflag (8; `SPAWNFLAG_DOOR_PASSABLE`)
    /// attaches [`DoorPassable`]; leaving it unset does not.
    #[test]
    fn passable_spawnflag_attaches_door_passable_marker() {
        let entities = vec![
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door1"),
                ("spawnflags", &SPAWNFLAG_DOOR_PASSABLE.to_string()),
            ]),
            raw(&[("classname", "func_door"), ("targetname", "door2")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let passable = registry.find("door1")[0];
        let plain = registry.find("door2")[0];
        assert!(registry.world.get::<&DoorPassable>(passable).is_ok());
        assert!(registry.world.get::<&DoorPassable>(plain).is_err());
    }

    /// The same spawnflag, read off `func_door_rotating` instead.
    #[test]
    fn passable_spawnflag_attaches_door_passable_marker_on_rotating_door() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("model", "*1"),
            ("spawnflags", &SPAWNFLAG_DOOR_PASSABLE.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("door1")[0];
        assert!(registry.world.get::<&DoorPassable>(entity).is_ok());
    }

    /// A brush entity built around an "origin brush" has its geometry
    /// compiled relative to that brush and the brush's own world position
    /// written into its `origin` keyvalue, so its raw compiled bounds sit
    /// around the submodel's local `(0, 0, 0)`. `BrushCenter` has to add
    /// `origin` back, or a proximity check against it searches near the
    /// map's world origin instead of near the entity — the bug
    /// `docs/FORMAT_SOURCES.md`'s `TODO(black-box)` item 25 recorded.
    #[test]
    fn brush_center_of_an_origin_brush_entity_is_its_placed_centre() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("model", "*1"),
            ("origin", "1000 -500 64"),
            ("distance", "90"),
            ("speed", "120"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        // Compiled relative to the origin brush: a 16x64x96 leaf hanging
        // off the local origin, nowhere near the entity's real position.
        bounds.insert(1u32, ([-8.0, 0.0, -48.0], [8.0, 64.0, 48.0]));
        let registry = Registry::build(&defs, &bounds, &Limits::default());
        let entity = registry.find("door1")[0];

        let center = registry
            .world
            .get::<&BrushCenter>(entity)
            .expect("a brush entity with known bounds has a centre");
        assert_eq!(center.0, Vec3::new(1000.0, -468.0, 64.0));

        // The bounds travel with it, and stay consistent with the centre.
        let box_ = registry
            .world
            .get::<&BrushBounds>(entity)
            .expect("a brush entity with known bounds has a box");
        assert_eq!(box_.mins, Vec3::new(992.0, -500.0, 16.0));
        assert_eq!(box_.maxs, Vec3::new(1008.0, -436.0, 112.0));
        assert_eq!((box_.mins + box_.maxs) * 0.5, center.0);
    }

    /// The ordinary case stays exactly as it was: a brush entity compiled
    /// in absolute world space leaves `origin` at `0 0 0`, so adding it
    /// unconditionally changes nothing for it.
    #[test]
    fn brush_center_of_a_world_compiled_entity_is_unchanged() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("targetname", "door1"),
            ("model", "*2"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(2u32, ([100.0, 200.0, 0.0], [140.0, 240.0, 80.0]));
        let registry = Registry::build(&defs, &bounds, &Limits::default());
        let center = registry
            .world
            .get::<&BrushCenter>(registry.find("door1")[0])
            .expect("a brush entity with known bounds has a centre");
        assert_eq!(center.0, Vec3::new(120.0, 220.0, 40.0));
    }

    #[test]
    fn movedir_handles_up_and_down_sentinels() {
        assert_eq!(movedir_from_angles(Vec3::new(0.0, -1.0, 0.0)), Vec3::Z);
        assert_eq!(movedir_from_angles(Vec3::new(0.0, -2.0, 0.0)), -Vec3::Z);
    }

    #[test]
    fn func_door_rotating_defaults_to_the_z_axis_and_carries_distance_in_degrees() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("distance", "90"),
            ("speed", "120"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let door = registry
            .world
            .get::<&Door>(registry.find("door1")[0])
            .expect("door");
        assert_eq!(door.rotation_axis, Some(Vec3::Z));
        assert!((door.travel_distance - 90.0).abs() < f32::EPSILON);
        assert!((door.speed - 120.0).abs() < f32::EPSILON);
        assert_eq!(door.movedir, Vec3::ZERO);
        assert_eq!(door.state, MoverState::Closed);
    }

    #[test]
    fn func_door_rotating_x_axis_spawnflag_selects_x() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("spawnflags", &SPAWNFLAG_DOOR_ROTATING_X_AXIS.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let door = registry
            .world
            .get::<&Door>(registry.find("door1")[0])
            .expect("door");
        assert_eq!(door.rotation_axis, Some(Vec3::X));
    }

    #[test]
    fn func_door_rotating_reverse_spawnflag_and_negative_distance_cancel_out() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("distance", "-90"),
            ("spawnflags", &SPAWNFLAG_DOOR_ROTATING_REVERSE.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let door = registry
            .world
            .get::<&Door>(registry.find("door1")[0])
            .expect("door");
        // A negative `distance` and the Reverse spawnflag each flip the
        // sign once, so together they cancel back to the un-reversed axis.
        assert_eq!(door.rotation_axis, Some(Vec3::Z));
        assert!((door.travel_distance - 90.0).abs() < f32::EPSILON);
    }

    #[test]
    fn func_door_rotating_starts_open_spawns_open() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("wait", "3"),
            (
                "spawnflags",
                &SPAWNFLAG_DOOR_ROTATING_STARTS_OPEN.to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let door = registry
            .world
            .get::<&Door>(registry.find("door1")[0])
            .expect("door");
        assert_eq!(door.state, MoverState::Open);
        assert!((door.timer - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn func_rotating_defaults_to_off_and_reads_axis_speed() {
        let entities = vec![raw(&[
            ("classname", "func_rotating"),
            ("targetname", "fan1"),
            ("speed", "180"),
            ("spawnflags", &SPAWNFLAG_ROTATING_Y_AXIS.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let rotator = registry
            .world
            .get::<&Rotator>(registry.find("fan1")[0])
            .expect("rotator");
        assert!(!rotator.spinning);
        assert_eq!(rotator.axis, Vec3::Y);
        assert!((rotator.speed - 180.0).abs() < f32::EPSILON);
        assert!((rotator.angle_deg - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn func_rotating_start_on_spawnflag_spins_immediately() {
        let entities = vec![raw(&[
            ("classname", "func_rotating"),
            ("targetname", "fan1"),
            (
                "spawnflags",
                &(SPAWNFLAG_ROTATING_START_ON | SPAWNFLAG_ROTATING_REVERSE).to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let rotator = registry
            .world
            .get::<&Rotator>(registry.find("fan1")[0])
            .expect("rotator");
        assert!(rotator.spinning);
        assert_eq!(rotator.axis, -Vec3::Z);
    }

    #[test]
    fn func_rot_button_reads_distance_speed_wait_and_defaults_to_z_axis() {
        let entities = vec![raw(&[
            ("classname", "func_rot_button"),
            ("targetname", "btn1"),
            ("distance", "60"),
            ("speed", "90"),
            ("wait", "2"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let button = registry
            .world
            .get::<&RotButton>(registry.find("btn1")[0])
            .expect("rot button");
        assert_eq!(button.axis, Vec3::Z);
        assert!((button.distance - 60.0).abs() < f32::EPSILON);
        assert!((button.speed - 90.0).abs() < f32::EPSILON);
        assert!((button.wait - 2.0).abs() < f32::EPSILON);
        assert!(!button.toggle);
        assert!(!button.touch);
        assert_eq!(button.state, MoverState::Closed);
    }

    #[test]
    fn func_rot_button_reverse_and_toggle_and_touch_spawnflags() {
        let entities = vec![raw(&[
            ("classname", "func_rot_button"),
            ("targetname", "btn1"),
            (
                "spawnflags",
                &(SPAWNFLAG_ROT_BUTTON_REVERSE
                    | SPAWNFLAG_ROT_BUTTON_TOGGLE
                    | SPAWNFLAG_ROT_BUTTON_TOUCH)
                    .to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let button = registry
            .world
            .get::<&RotButton>(registry.find("btn1")[0])
            .expect("rot button");
        assert_eq!(button.axis, -Vec3::Z);
        assert!(button.toggle);
        assert!(button.touch);
    }

    #[test]
    fn func_rot_button_x_axis_spawnflag_selects_x() {
        let entities = vec![raw(&[
            ("classname", "func_rot_button"),
            ("targetname", "btn1"),
            ("spawnflags", &SPAWNFLAG_ROT_BUTTON_X_AXIS.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let button = registry
            .world
            .get::<&RotButton>(registry.find("btn1")[0])
            .expect("rot button");
        assert_eq!(button.axis, Vec3::X);
    }

    #[test]
    fn momentary_rot_button_reads_distance_speed_returnspeed_and_flags() {
        let entities = vec![raw(&[
            ("classname", "momentary_rot_button"),
            ("targetname", "valve1"),
            ("distance", "120"),
            ("speed", "40"),
            ("returnspeed", "80"),
            (
                "spawnflags",
                &(SPAWNFLAG_MOMENTARY_AUTO_RETURN | SPAWNFLAG_MOMENTARY_Y_AXIS).to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let button = registry
            .world
            .get::<&MomentaryRotButton>(registry.find("valve1")[0])
            .expect("momentary rot button");
        assert_eq!(button.axis, Vec3::Y);
        assert!((button.distance - 120.0).abs() < f32::EPSILON);
        assert!((button.speed - 40.0).abs() < f32::EPSILON);
        assert!((button.return_speed - 80.0).abs() < f32::EPSILON);
        assert!(button.auto_return);
        assert!(!button.door_hack);
        assert!((button.fraction - 0.0).abs() < f32::EPSILON);
        assert!(button.moving_forward);
    }

    #[test]
    fn momentary_rot_button_door_hack_spawnflag() {
        let entities = vec![raw(&[
            ("classname", "momentary_rot_button"),
            ("targetname", "valve1"),
            ("spawnflags", &SPAWNFLAG_MOMENTARY_DOOR_HACK.to_string()),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let button = registry
            .world
            .get::<&MomentaryRotButton>(registry.find("valve1")[0])
            .expect("momentary rot button");
        assert!(button.door_hack);
    }

    #[test]
    fn func_pendulum_reads_distance_speed_damping_and_defaults_to_off() {
        let entities = vec![raw(&[
            ("classname", "func_pendulum"),
            ("targetname", "swing1"),
            ("distance", "45"),
            ("speed", "30"),
            ("damping", "200"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let pendulum = registry
            .world
            .get::<&Pendulum>(registry.find("swing1")[0])
            .expect("pendulum");
        assert_eq!(pendulum.axis, Vec3::Z);
        assert!((pendulum.distance - 45.0).abs() < f32::EPSILON);
        assert!((pendulum.speed - 30.0).abs() < f32::EPSILON);
        assert!((pendulum.damping - 200.0).abs() < f32::EPSILON);
        assert!(!pendulum.swinging);
        assert!(!pendulum.auto_return);
        assert!((pendulum.angle_deg - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn func_pendulum_start_on_spawnflag_swings_immediately() {
        let entities = vec![raw(&[
            ("classname", "func_pendulum"),
            ("targetname", "swing1"),
            (
                "spawnflags",
                &(SPAWNFLAG_PENDULUM_START_ON | SPAWNFLAG_PENDULUM_X_AXIS).to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let pendulum = registry
            .world
            .get::<&Pendulum>(registry.find("swing1")[0])
            .expect("pendulum");
        assert!(pendulum.swinging);
        assert_eq!(pendulum.axis, Vec3::X);
    }

    #[test]
    fn multi_manager_collects_target_delay_pairs() {
        let entities = vec![raw(&[
            ("classname", "multi_manager"),
            ("targetname", "mm1"),
            ("light1", "0.0"),
            ("door1", "1.5"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("mm1")[0];
        let mm = registry.world.get::<&MultiManager>(entity).expect("mm");
        assert_eq!(mm.targets.len(), 2);
    }

    #[test]
    fn trigger_hurt_keeps_its_damage_and_damage_type() {
        let entities = vec![raw(&[
            ("classname", "trigger_hurt"),
            ("targetname", "acid"),
            ("dmg", "20"),
            // The documented additive bit field: 8 (burn) + 16 (freeze).
            ("damagetype", "24"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("acid")[0];
        let hurt = registry.world.get::<&TriggerHurt>(entity).expect("hurt");
        assert!((hurt.damage_per_second - 20.0).abs() < f32::EPSILON);
        assert_eq!(hurt.damage_type, 24);
        // It is still a trigger, so the ordinary trigger plumbing applies.
        assert!(registry.world.get::<&Trigger>(entity).is_ok());
    }

    #[test]
    fn func_ladder_gets_a_ladder_marker() {
        let entities = vec![raw(&[
            ("classname", "func_ladder"),
            ("targetname", "ladder1"),
            ("model", "*3"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("ladder1")[0];
        assert!(registry.world.get::<&Ladder>(entity).is_ok());
        assert!(registry.world.get::<&Unknown>(entity).is_err());
    }

    #[test]
    fn func_water_gets_a_water_marker_with_the_default_liquid() {
        let entities = vec![raw(&[
            ("classname", "func_water"),
            ("targetname", "pool1"),
            ("model", "*4"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("pool1")[0];
        let water = registry.world.get::<&Water>(entity).expect("func_water");
        assert_eq!(water.0, Liquid::Water);
        assert!(registry.world.get::<&Unknown>(entity).is_err());
    }

    #[test]
    fn func_water_reads_its_liquid_from_the_skin_keyvalue() {
        for (skin, expected) in [
            ("-3", Liquid::Water),
            ("-4", Liquid::Slime),
            ("-5", Liquid::Lava),
            ("bogus", Liquid::Water),
        ] {
            let entities = vec![raw(&[
                ("classname", "func_water"),
                ("targetname", "pool1"),
                ("model", "*4"),
                ("skin", skin),
            ])];
            let defs = parse_entities(&entities, &Limits::default());
            let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
            let entity = registry.find("pool1")[0];
            let water = registry.world.get::<&Water>(entity).expect("func_water");
            assert_eq!(water.0, expected, "skin {skin}");
        }
    }

    #[test]
    fn worldspawn_wad_list_is_parsed() {
        let entities = vec![raw(&[
            ("classname", "worldspawn"),
            ("skyname", "desert"),
            ("wad", "halflife.wad;xeno.wad"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let ws = registry.worldspawn.expect("worldspawn");
        assert_eq!(ws.skyname, "desert");
        assert_eq!(ws.wads, vec!["halflife.wad", "xeno.wad"]);
    }

    #[test]
    fn name_index_is_bounded_per_name() {
        let mut entities = Vec::new();
        for _ in 0..(MAX_ENTITIES_PER_NAME + 10) {
            entities.push(raw(&[("classname", "light"), ("targetname", "many")]));
        }
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        assert_eq!(registry.find("many").len(), MAX_ENTITIES_PER_NAME);
    }
}
