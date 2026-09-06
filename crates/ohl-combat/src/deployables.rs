//! Placed explosives: satchel charges and tripmines.
//!
//! A [`DeployableSet`] holds one player's placed charges. Satchels sit where
//! they were dropped until their owner sets them all off at once; tripmines
//! stick to the surface a trace found, arm after a delay, and then watch a
//! beam cast along their own normal, detonating when something crosses it.
//!
//! Like [`crate::projectile`], nothing here applies damage: a detonation
//! reports a [`DeployableEvent::Detonated`] and the caller turns it into a
//! [`crate::explosion::radius_damage`] call at the published 150 damage each
//! weapon does (`crate::weapons::spec`).
//!
//! # Published versus placeholder
//!
//! Published (see `docs/FORMAT_SOURCES.md`): the tripmine's roughly three
//! second arming cue ([`TRIPMINE_ARM_SECONDS`]) and the five-of-each carry
//! maximum ([`MAX_SATCHELS`], [`MAX_TRIPMINES`]), which is the published
//! maximum ammunition count for both weapons. Everything else — blast radii,
//! how long the beam reaches, how a satchel behaves when its owner dies — is
//! **to be black-box observed** and lives in [`DeployableTuning`] behind
//! [`BlackBox`] with a `// TODO(black-box)` marker.

use glam::Vec3;
use ohl_physics::{CollisionModel, Hull};

use crate::trace::{EntityId, HitboxIndex, TraceMask, trace_attack};
use crate::weapons::BlackBox;

/// How long a tripmine takes to arm, in seconds.
///
/// Published: Combine OverWiki, "Tripmine" — the mine plays an arming cue
/// for about three seconds after being placed, and only then projects its
/// beam (`docs/FORMAT_SOURCES.md`).
pub const TRIPMINE_ARM_SECONDS: f32 = 3.0;

/// The most satchel charges one owner may have placed at once.
///
/// Published: Combine OverWiki, "Satchel Charge" — a maximum of five carried
/// (`crate::ammo::AmmoType::Satchels`).
pub const MAX_SATCHELS: usize = 5;

/// The most tripmines one owner may have placed at once.
///
/// Published: Combine OverWiki, "Tripmine" — a maximum of five carried
/// (`crate::ammo::AmmoType::Tripmines`).
pub const MAX_TRIPMINES: usize = 5;

/// Which kind of placed explosive an event or handle refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeployableKind {
    /// A satchel charge, detonated by its owner's radio.
    Satchel,
    /// A tripmine, detonated by something crossing its beam.
    Tripmine,
}

/// A handle to one placed explosive, unique within its [`DeployableSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeployableId(pub u32);

/// The unpublished numbers a placed explosive needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeployableTuning {
    /// How far a tripmine's beam reaches, in world units.
    pub beam_length: BlackBox<f32>,
    /// How far in front of the surface a tripmine's beam starts, in world
    /// units, so the beam does not immediately hit the wall it is stuck to.
    pub beam_offset: BlackBox<f32>,
    /// The blast radius of a satchel charge, in world units.
    pub satchel_radius: BlackBox<f32>,
    /// The blast radius of a tripmine, in world units.
    pub tripmine_radius: BlackBox<f32>,
    /// How far a placement trace may reach for a tripmine to stick, in world
    /// units.
    pub place_range: BlackBox<f32>,
}

impl Default for DeployableTuning {
    fn default() -> Self {
        Self {
            // TODO(black-box): the tripmine's beam length is not published.
            beam_length: BlackBox::new(512.0),
            // TODO(black-box): unpublished; one unit clears the surface.
            beam_offset: BlackBox::new(1.0),
            // TODO(black-box): no blast radius is published for any
            // Half-Life explosive.
            satchel_radius: BlackBox::new(200.0),
            // TODO(black-box): as above.
            tripmine_radius: BlackBox::new(200.0),
            // TODO(black-box): the placement reach is not published.
            place_range: BlackBox::new(64.0),
        }
    }
}

/// One placed satchel charge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Satchel {
    /// This charge's handle.
    pub id: DeployableId,
    /// Who placed it.
    pub owner: Option<EntityId>,
    /// Where it sits, in world units.
    pub position: Vec3,
    /// Seconds since it was placed.
    pub age: f32,
}

/// One placed tripmine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tripmine {
    /// This mine's handle.
    pub id: DeployableId,
    /// Who placed it.
    pub owner: Option<EntityId>,
    /// Where it is stuck, in world units: the point the placement trace hit.
    pub position: Vec3,
    /// The unit normal of the surface it is stuck to, which its beam runs
    /// along.
    pub normal: Vec3,
    /// Seconds since it was placed.
    pub age: f32,
    /// Whether the arming delay has elapsed and the beam is live.
    pub armed: bool,
}

impl Tripmine {
    /// Where the beam starts: just clear of the surface.
    #[must_use]
    pub fn beam_start(&self, tuning: &DeployableTuning) -> Vec3 {
        self.position + self.normal * tuning.beam_offset.value.max(0.0)
    }
}

/// Something that happened to a placed explosive.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum DeployableEvent {
    /// A charge was placed.
    Placed {
        /// Which one.
        id: DeployableId,
        /// What kind.
        kind: DeployableKind,
        /// Where.
        position: Vec3,
    },
    /// A tripmine finished arming and its beam is now live.
    Armed {
        /// Which one.
        id: DeployableId,
        /// Where.
        position: Vec3,
    },
    /// A charge went off; the caller applies radius damage here.
    Detonated {
        /// Which one.
        id: DeployableId,
        /// What kind.
        kind: DeployableKind,
        /// Where.
        position: Vec3,
        /// Who placed it, for damage attribution.
        owner: Option<EntityId>,
        /// The blast radius from [`DeployableTuning`], so the caller does
        /// not have to re-derive it from the kind.
        radius: f32,
    },
}

/// One owner's placed explosives.
///
/// Both lists are bounded by the published carry maxima, and both are kept in
/// placement order, so ticking the set is deterministic.
#[derive(Debug, Clone, Default)]
pub struct DeployableSet {
    satchels: Vec<Satchel>,
    tripmines: Vec<Tripmine>,
    next_id: u32,
}

impl DeployableSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The placed satchel charges, in placement order.
    #[must_use]
    pub fn satchels(&self) -> &[Satchel] {
        &self.satchels
    }

    /// The placed tripmines, in placement order.
    #[must_use]
    pub fn tripmines(&self) -> &[Tripmine] {
        &self.tripmines
    }

    /// Removes everything without emitting events.
    pub fn clear(&mut self) {
        self.satchels.clear();
        self.tripmines.clear();
    }

    /// This set's own next-handle counter, for a save file. Additive, for
    /// `.plan/m79-design.md` §6/§8 P4b's `SECTION_PROJECTILES`; paired with
    /// [`Self::restore_from_parts`].
    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// Rebuilds a set from exactly the state [`Self::satchels`],
    /// [`Self::tripmines`] and [`Self::next_id`] describe, so a restored
    /// deployable keeps its original [`DeployableId`] and the next one
    /// placed continues the same sequence a continuously-run set would
    /// have produced. Additive, for save-file restore.
    ///
    /// Bounded to [`MAX_SATCHELS`]/[`MAX_TRIPMINES`], dropping from the
    /// tail, the same as [`Self::place_satchel`]/[`Self::place_tripmine`]
    /// refusing past that point.
    #[must_use]
    pub fn restore_from_parts(
        mut satchels: Vec<Satchel>,
        mut tripmines: Vec<Tripmine>,
        next_id: u32,
    ) -> Self {
        satchels.truncate(MAX_SATCHELS);
        tripmines.truncate(MAX_TRIPMINES);
        Self {
            satchels,
            tripmines,
            next_id,
        }
    }

    /// The next handle.
    fn allocate(&mut self) -> DeployableId {
        let id = DeployableId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Places a satchel charge at `position`.
    ///
    /// Returns `None` when [`MAX_SATCHELS`] are already placed or the
    /// position is not finite.
    pub fn place_satchel(
        &mut self,
        owner: Option<EntityId>,
        position: Vec3,
        events: &mut Vec<DeployableEvent>,
    ) -> Option<DeployableId> {
        if self.satchels.len() >= MAX_SATCHELS || !position.is_finite() {
            return None;
        }
        let id = self.allocate();
        self.satchels.push(Satchel {
            id,
            owner,
            position,
            age: 0.0,
        });
        events.push(DeployableEvent::Placed {
            id,
            kind: DeployableKind::Satchel,
            position,
        });
        Some(id)
    }

    /// Sets off every placed satchel charge at once, oldest first, and
    /// removes them.
    ///
    /// Returns how many went off. This is the satchel's published behaviour:
    /// the charges are remote-detonated, all of them together.
    pub fn detonate_all_satchels(
        &mut self,
        tuning: &DeployableTuning,
        events: &mut Vec<DeployableEvent>,
    ) -> usize {
        let count = self.satchels.len();
        for satchel in self.satchels.drain(..) {
            events.push(DeployableEvent::Detonated {
                id: satchel.id,
                kind: DeployableKind::Satchel,
                position: satchel.position,
                owner: satchel.owner,
                radius: tuning.satchel_radius.value.max(0.0),
            });
        }
        count
    }

    /// Places a tripmine on whatever a trace from `from` toward `direction`
    /// runs into.
    ///
    /// The trace uses hull 0 against world geometry only, so a mine sticks to
    /// the map and not to a passing monster. Returns `None` when the trace
    /// hits nothing within [`DeployableTuning::place_range`], when the
    /// surface has no usable normal, or when [`MAX_TRIPMINES`] are already
    /// placed. A freshly placed mine is not armed: it arms after
    /// [`TRIPMINE_ARM_SECONDS`] of [`tick`](Self::tick).
    pub fn place_tripmine(
        &mut self,
        owner: Option<EntityId>,
        from: Vec3,
        direction: Vec3,
        world: &CollisionModel,
        tuning: &DeployableTuning,
        events: &mut Vec<DeployableEvent>,
    ) -> Option<DeployableId> {
        if self.tripmines.len() >= MAX_TRIPMINES || !from.is_finite() {
            return None;
        }
        let direction = direction.normalize_or_zero();
        if direction == Vec3::ZERO {
            return None;
        }
        let end = from + direction * tuning.place_range.value.max(0.0);
        let trace = world.trace(Hull::Point, from, end);
        if trace.fraction >= 1.0 || trace.start_solid {
            return None;
        }
        let normal = trace.plane_normal.normalize_or_zero();
        if normal == Vec3::ZERO {
            return None;
        }
        let id = self.allocate();
        self.tripmines.push(Tripmine {
            id,
            owner,
            position: trace.end_pos,
            normal,
            age: 0.0,
            armed: false,
        });
        events.push(DeployableEvent::Placed {
            id,
            kind: DeployableKind::Tripmine,
            position: trace.end_pos,
        });
        Some(id)
    }

    /// Detonates one charge by handle, whichever list it is in.
    ///
    /// Returns `false` when no charge has that handle.
    pub fn detonate(
        &mut self,
        id: DeployableId,
        tuning: &DeployableTuning,
        events: &mut Vec<DeployableEvent>,
    ) -> bool {
        if let Some(index) = self.satchels.iter().position(|entry| entry.id == id) {
            let satchel = self.satchels.remove(index);
            events.push(DeployableEvent::Detonated {
                id,
                kind: DeployableKind::Satchel,
                position: satchel.position,
                owner: satchel.owner,
                radius: tuning.satchel_radius.value.max(0.0),
            });
            return true;
        }
        if let Some(index) = self.tripmines.iter().position(|entry| entry.id == id) {
            let mine = self.tripmines.remove(index);
            events.push(DeployableEvent::Detonated {
                id,
                kind: DeployableKind::Tripmine,
                position: mine.position,
                owner: mine.owner,
                radius: tuning.tripmine_radius.value.max(0.0),
            });
            return true;
        }
        false
    }

    /// Advances every placed charge by `dt` seconds.
    ///
    /// Satchels only age. Each tripmine ages too; the tick it passes
    /// [`TRIPMINE_ARM_SECONDS`] it emits [`DeployableEvent::Armed`], and
    /// every tick after that its beam is cast — a trace from
    /// [`Tripmine::beam_start`] along the mine's normal, out to
    /// [`DeployableTuning::beam_length`], against the world *and* the
    /// caller's hitbox index. If the beam stops on an entity hitbox, the
    /// mine detonates; if it stops on world geometry, that is simply where
    /// the beam ends.
    ///
    /// Events are appended to `events` in placement order, satchels first.
    pub fn tick(
        &mut self,
        dt: f32,
        world: &CollisionModel,
        entities: &HitboxIndex,
        tuning: &DeployableTuning,
        events: &mut Vec<DeployableEvent>,
    ) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        for satchel in &mut self.satchels {
            satchel.age += dt;
        }

        let mut index = 0;
        while index < self.tripmines.len() {
            let mine = &mut self.tripmines[index];
            mine.age += dt;
            if !mine.armed && mine.age >= TRIPMINE_ARM_SECONDS {
                mine.armed = true;
                events.push(DeployableEvent::Armed {
                    id: mine.id,
                    position: mine.position,
                });
            }
            let tripped = mine.armed && beam_blocked(*mine, world, entities, tuning);
            if tripped {
                let mine = self.tripmines.remove(index);
                events.push(DeployableEvent::Detonated {
                    id: mine.id,
                    kind: DeployableKind::Tripmine,
                    position: mine.position,
                    owner: mine.owner,
                    radius: tuning.tripmine_radius.value.max(0.0),
                });
            } else {
                index += 1;
            }
        }
    }

    /// Where an armed mine's beam currently ends, for drawing it.
    ///
    /// `None` for a mine that is not armed or has no handle in this set.
    #[must_use]
    pub fn beam_end(
        &self,
        id: DeployableId,
        world: &CollisionModel,
        entities: &HitboxIndex,
        tuning: &DeployableTuning,
    ) -> Option<Vec3> {
        let mine = self.tripmines.iter().find(|entry| entry.id == id)?;
        if !mine.armed {
            return None;
        }
        Some(beam_trace(*mine, world, entities, tuning).1)
    }
}

/// Traces one mine's beam. Returns the entity it stopped on (if any) and
/// where it ended.
fn beam_trace(
    mine: Tripmine,
    world: &CollisionModel,
    entities: &HitboxIndex,
    tuning: &DeployableTuning,
) -> (Option<EntityId>, Vec3) {
    let start = mine.beam_start(tuning);
    let end = start + mine.normal * tuning.beam_length.value.max(0.0);
    let trace = trace_attack(world, entities, start, end, TraceMask::SHOT);
    (trace.entity, trace.end)
}

/// Whether something crossed a mine's beam this tick.
fn beam_blocked(
    mine: Tripmine,
    world: &CollisionModel,
    entities: &HitboxIndex,
    tuning: &DeployableTuning,
) -> bool {
    beam_trace(mine, world, entities, tuning).0.is_some()
}
