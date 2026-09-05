//! Sight and hearing.
//!
//! [`look`] walks a candidate list, keeps the ones inside the look distance,
//! the potentially visible set and the view cone, confirms line of sight with
//! a point-hull trace from the viewer's eye to the candidate's eye, and picks
//! an enemy by relationship first and distance second. [`listen`] scans a
//! bounded sound list and reports what was audible.
//!
//! Only the *rules* are documented public behaviour (see
//! `docs/FORMAT_SOURCES.md`, "Monster AI behaviour"): sight originates at
//! `origin + view_ofs`, a view cone limits it, an enemy is chosen by
//! relationship then distance, an occluded enemy inside 256 units stays
//! tracked, and hearing has a per-monster sensitivity multiplier. Every
//! numeric default in [`Senses`] is a placeholder to be black-box observed.

use glam::Vec3;
use hecs::Entity;
use ohl_physics::{CollisionModel, Hull};
use ohl_world::WorldModel;

use crate::state::{Classification, Conditions, Relationship, RelationshipTable};

/// The largest number of sound events [`SoundList`] retains. Bounded so a
/// misbehaving emitter cannot grow the AI's per-tick work without limit.
pub const MAX_SOUNDS: usize = 128;

/// The distance inside which an occluded enemy stays tracked instead of
/// being forgotten (published behaviour).
pub const OCCLUDED_MEMORY_DISTANCE: f32 = 256.0;

/// The hearing sensitivity multiplier documented for the tentacle.
pub const TENTACLE_HEARING_SENSITIVITY: f32 = 2.0;

/// What kind of thing made a sound.
///
/// The published sound vocabulary; `Carcass`, `Meat` and `Garbage` are
/// scents rather than sounds and are carried in the same list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SoundKind {
    /// Weapons fire and impacts.
    Combat,
    /// Something about to hurt whoever is nearby (a live grenade).
    Danger,
    /// Doors, machinery, breakables.
    World,
    /// The player moving about.
    Player,
    /// A dead body.
    Carcass,
    /// Food.
    Meat,
    /// Refuse.
    Garbage,
}

impl SoundKind {
    /// A stable byte tag for determinism hashes and save files.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Combat => 0,
            Self::Danger => 1,
            Self::World => 2,
            Self::Player => 3,
            Self::Carcass => 4,
            Self::Meat => 5,
            Self::Garbage => 6,
        }
    }

    /// Whether this entry is smelled rather than heard.
    #[must_use]
    pub const fn is_scent(self) -> bool {
        matches!(self, Self::Carcass | Self::Meat | Self::Garbage)
    }

    /// The condition an audible entry of this kind sets, in addition to
    /// [`Conditions::HEAR_SOUND`] (or [`Conditions::SMELL`] for scents).
    #[must_use]
    pub const fn condition(self) -> Conditions {
        match self {
            Self::Combat => Conditions::HEAR_COMBAT,
            Self::Danger => Conditions::HEAR_DANGER,
            Self::World | Self::Player => Conditions::EMPTY,
            Self::Carcass | Self::Meat | Self::Garbage => Conditions::SMELL,
        }
    }
}

/// One entry in the world's sound/scent list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundEvent {
    /// What made it.
    pub kind: SoundKind,
    /// Where it is.
    pub position: Vec3,
    /// The base radius inside which a listener of sensitivity `1.0` hears it.
    pub radius: f32,
    /// Who made it, when that is known.
    pub emitter: Option<Entity>,
    /// How much longer the entry stays in the list, in seconds.
    pub lifetime: f32,
}

impl SoundEvent {
    /// A sound with no known emitter that lasts one tenth of a second.
    #[must_use]
    pub fn new(kind: SoundKind, position: Vec3, radius: f32) -> Self {
        Self {
            kind,
            position,
            radius,
            emitter: None,
            lifetime: 0.1,
        }
    }

    /// The same sound with an emitter attached.
    #[must_use]
    pub fn from(mut self, emitter: Entity) -> Self {
        self.emitter = Some(emitter);
        self
    }

    /// The same sound with an explicit lifetime in seconds.
    #[must_use]
    pub fn lasting(mut self, seconds: f32) -> Self {
        self.lifetime = seconds;
        self
    }
}

/// A bounded, deterministic list of live sound and scent events.
#[derive(Debug, Clone, Default)]
pub struct SoundList {
    events: Vec<SoundEvent>,
}

impl SoundList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an event, dropping it when the list is already at
    /// [`MAX_SOUNDS`] or its geometry is not finite.
    ///
    /// Returns whether the event was kept.
    pub fn push(&mut self, event: SoundEvent) -> bool {
        let usable = event.position.is_finite() && event.radius.is_finite() && event.radius > 0.0;
        if !usable || self.events.len() >= MAX_SOUNDS {
            return false;
        }
        self.events.push(event);
        true
    }

    /// The live events, oldest first.
    #[must_use]
    pub fn events(&self) -> &[SoundEvent] {
        &self.events
    }

    /// Whether the list holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The number of live events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Ages every event by `dt` and drops the expired ones, preserving
    /// order.
    pub fn expire(&mut self, dt: f32) {
        for event in &mut self.events {
            event.lifetime -= dt;
        }
        self.events.retain(|event| event.lifetime > 0.0);
    }

    /// Drops every event.
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// Per-monster sensory parameters.
///
/// **Every default here is provisional** and marked to be black-box observed
/// against legally obtained retail software; no published source gives view
/// cone angles or look distances (`docs/FORMAT_SOURCES.md`, "Monster AI
/// behaviour").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Senses {
    /// How far sight reaches, in world units.
    pub look_distance: f32,
    /// The cosine of the view cone's half angle. `1.0` is a needle, `0.0` a
    /// half space, `-1.0` full 360-degree vision (turrets).
    pub fov_cos: f32,
    /// Multiplies every heard sound's radius. Published: `2.0` for the
    /// tentacle, `1.0` for everything else.
    pub hearing_sensitivity: f32,
}

impl Senses {
    /// Sight in every direction, for turret-style entities.
    #[must_use]
    pub fn omnidirectional(look_distance: f32) -> Self {
        Self {
            look_distance,
            fov_cos: -1.0,
            ..Self::default()
        }
    }

    /// Whether `to_target`, a vector from the eye, lies inside the cone
    /// around `forward`.
    #[must_use]
    pub fn in_view_cone(&self, forward: Vec3, to_target: Vec3) -> bool {
        let length = to_target.length();
        if length <= f32::EPSILON {
            return true;
        }
        let forward = if forward.length_squared() > f32::EPSILON {
            forward.normalize()
        } else {
            Vec3::X
        };
        forward.dot(to_target / length) >= self.fov_cos
    }
}

impl Default for Senses {
    /// **Provisional:** 2048 units of sight through a 120-degree cone, with
    /// unmodified hearing.
    fn default() -> Self {
        Self {
            look_distance: 2_048.0,
            fov_cos: 0.5,
            hearing_sensitivity: 1.0,
        }
    }
}

/// The entity doing the looking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewer {
    /// The looker itself, so it never sees itself.
    pub entity: Entity,
    /// Its world-space origin.
    pub origin: Vec3,
    /// The eye offset above the origin.
    pub view_ofs: Vec3,
    /// The direction it faces, as a unit vector.
    pub forward: Vec3,
    /// Its faction.
    pub classification: Classification,
}

impl Viewer {
    /// The point sight originates from.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.origin + self.view_ofs
    }
}

/// One entity that might be seen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// The entity.
    pub entity: Entity,
    /// Its faction.
    pub classification: Classification,
    /// Its world-space origin.
    pub origin: Vec3,
    /// The eye offset used as the trace endpoint.
    pub view_ofs: Vec3,
    /// The direction it faces, used for `ENEMY_FACING_ME`.
    pub forward: Vec3,
    /// Whether it is still alive.
    pub alive: bool,
    /// Whether it is the player.
    pub is_client: bool,
}

impl Candidate {
    /// The point a line-of-sight trace aims at.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.origin + self.view_ofs
    }
}

/// The collision and visibility data sight is resolved against.
///
/// Both are optional: with neither, sight degrades to distance plus view
/// cone, which is always the permissive answer.
#[derive(Clone, Copy, Default)]
pub struct SightContext<'a> {
    /// The world's clip hulls, used for the line-of-sight trace.
    pub collision: Option<&'a CollisionModel>,
    /// The world model, used for the potentially-visible-set pre-filter.
    pub world: Option<&'a WorldModel>,
}

impl core::fmt::Debug for SightContext<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SightContext")
            .field("collision", &self.collision.is_some())
            .field("world", &self.world.is_some())
            .finish()
    }
}

impl<'a> SightContext<'a> {
    /// A context with no world data at all.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A context that only traces.
    #[must_use]
    pub fn tracing(collision: &'a CollisionModel) -> Self {
        Self {
            collision: Some(collision),
            world: None,
        }
    }

    /// Whether `to` is in `from`'s potentially visible set. Returns `true`
    /// when no world model is available or either point falls outside the
    /// tree, so the pre-filter never hides something the trace would see.
    #[must_use]
    pub fn potentially_visible(&self, from: Vec3, to: Vec3) -> bool {
        let Some(world) = self.world else {
            return true;
        };
        let (Some(from_leaf), Some(to_leaf)) =
            (world.leaf_at(from.to_array()), world.leaf_at(to.to_array()))
        else {
            return true;
        };
        let vis = world.visibility();
        if from_leaf >= vis.leaf_count() || to_leaf >= vis.leaf_count() {
            return true;
        }
        vis.is_visible(from_leaf, to_leaf)
    }

    /// Whether an unobstructed straight line runs from `from` to `to`.
    ///
    /// Uses the point hull (hull 0), the hull sight is resolved against.
    #[must_use]
    pub fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        let Some(collision) = self.collision else {
            return true;
        };
        let trace = collision.trace(Hull::Point, from, to);
        trace.fraction >= 1.0 && !trace.start_solid
    }
}

/// One candidate that survived every sight test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sighting {
    /// The entity seen.
    pub entity: Entity,
    /// Its faction.
    pub classification: Classification,
    /// How the viewer regards it.
    pub relationship: Relationship,
    /// Its origin.
    pub origin: Vec3,
    /// The point that was actually traced to.
    pub eye: Vec3,
    /// Distance from the viewer's eye to `eye`.
    pub distance: f32,
    /// Whether it is still alive.
    pub alive: bool,
    /// Whether it is facing the viewer.
    pub facing_viewer: bool,
}

/// What [`look`] found.
#[derive(Debug, Clone, Default)]
pub struct LookResult {
    /// The sight conditions this tick.
    pub conditions: Conditions,
    /// Everything visible, in candidate order.
    pub visible: Vec<Sighting>,
    /// The best enemy among [`Self::visible`], by relationship then
    /// distance.
    pub enemy: Option<Sighting>,
}

/// Runs the full sight pass for one viewer.
///
/// Candidates are filtered by, in order: not the viewer itself, finite
/// geometry, the look distance, the potentially visible set, the view cone
/// and finally a point-hull trace from eye to eye. Enemy selection then
/// prefers the worse relationship and, among equals, the nearer candidate;
/// ties beyond that are broken by entity id so the result never depends on
/// candidate ordering noise.
#[must_use]
pub fn look(
    viewer: &Viewer,
    senses: &Senses,
    candidates: &[Candidate],
    relationships: &RelationshipTable,
    context: &SightContext<'_>,
) -> LookResult {
    let mut result = LookResult::default();
    let eye = viewer.eye();
    if !eye.is_finite() || !senses.look_distance.is_finite() || senses.look_distance <= 0.0 {
        return result;
    }

    for candidate in candidates {
        if candidate.entity == viewer.entity || !candidate.origin.is_finite() {
            continue;
        }
        let target = candidate.eye();
        if !target.is_finite() {
            continue;
        }
        let to_target = target - eye;
        let distance = to_target.length();
        if !distance.is_finite() || distance > senses.look_distance {
            continue;
        }
        if !senses.in_view_cone(viewer.forward, to_target) {
            continue;
        }
        if !context.potentially_visible(eye, target) || !context.line_of_sight(eye, target) {
            continue;
        }

        let relationship = relationships.get(viewer.classification, candidate.classification);
        let facing_viewer = facing(candidate.forward, eye - target);
        result.conditions |= relationship.sighting_condition();
        if candidate.is_client {
            result.conditions |= Conditions::SEE_CLIENT;
        }
        result.visible.push(Sighting {
            entity: candidate.entity,
            classification: candidate.classification,
            relationship,
            origin: candidate.origin,
            eye: target,
            distance,
            alive: candidate.alive,
            facing_viewer,
        });
    }

    result.enemy = select_enemy(&result.visible);
    if result.enemy.is_some() {
        result.conditions |= Conditions::SEE_ENEMY;
    }
    result
}

/// The best enemy among `visible`: worst relationship first, then nearest,
/// then lowest entity id.
#[must_use]
pub fn select_enemy(visible: &[Sighting]) -> Option<Sighting> {
    visible
        .iter()
        .filter(|sighting| sighting.relationship.is_hostile() && sighting.alive)
        .copied()
        .reduce(|best, candidate| {
            let better = match candidate.relationship.cmp(&best.relationship) {
                core::cmp::Ordering::Greater => true,
                core::cmp::Ordering::Less => false,
                core::cmp::Ordering::Equal => {
                    if (candidate.distance - best.distance).abs() > f32::EPSILON {
                        candidate.distance < best.distance
                    } else {
                        candidate.entity.id() < best.entity.id()
                    }
                }
            };
            if better { candidate } else { best }
        })
}

/// Whether `forward` points within 45 degrees of `toward`.
fn facing(forward: Vec3, toward: Vec3) -> bool {
    let length = toward.length();
    if length <= f32::EPSILON || forward.length_squared() <= f32::EPSILON {
        return false;
    }
    forward.normalize().dot(toward / length) >= core::f32::consts::FRAC_1_SQRT_2
}

/// What [`listen`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ListenResult {
    /// The hearing conditions this tick.
    pub conditions: Conditions,
    /// The most interesting audible entry: a danger sound if any was
    /// audible, otherwise the nearest audible entry.
    pub best: Option<SoundEvent>,
}

/// Runs the hearing pass for one listener at `ears`.
///
/// An entry is audible when the listener is inside `radius *
/// hearing_sensitivity` of it. Scents set [`Conditions::SMELL`] rather than
/// [`Conditions::HEAR_SOUND`].
#[must_use]
pub fn listen(ears: Vec3, senses: &Senses, sounds: &SoundList) -> ListenResult {
    let mut result = ListenResult::default();
    if !ears.is_finite() {
        return result;
    }
    let sensitivity = if senses.hearing_sensitivity.is_finite() && senses.hearing_sensitivity > 0.0
    {
        senses.hearing_sensitivity
    } else {
        0.0
    };

    let mut best_distance = f32::INFINITY;
    let mut best_is_danger = false;
    for event in sounds.events() {
        let distance = (event.position - ears).length();
        if !distance.is_finite() || distance > event.radius * sensitivity {
            continue;
        }
        if event.kind.is_scent() {
            result.conditions |= Conditions::SMELL;
        } else {
            result.conditions |= Conditions::HEAR_SOUND;
        }
        result.conditions |= event.kind.condition();

        let is_danger = event.kind == SoundKind::Danger;
        let better = match (is_danger, best_is_danger) {
            (true, false) => true,
            (false, true) => false,
            _ => distance < best_distance,
        };
        if better {
            best_distance = distance;
            best_is_danger = is_danger;
            result.best = Some(*event);
        }
    }
    result
}

/// What a monster remembers about the enemy it acquired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyMemory {
    /// The enemy.
    pub entity: Entity,
    /// Where it was last actually seen.
    pub last_known_position: Vec3,
    /// Seconds since it was last seen; zero while visible.
    pub time_since_seen: f32,
    /// Whether line of sight is currently blocked.
    pub occluded: bool,
    /// The distance at the last update, used by the occlusion rule.
    pub last_known_distance: f32,
}

impl EnemyMemory {
    /// A fresh memory from a sighting.
    #[must_use]
    pub fn seen(sighting: &Sighting) -> Self {
        Self {
            entity: sighting.entity,
            last_known_position: sighting.origin,
            time_since_seen: 0.0,
            occluded: false,
            last_known_distance: sighting.distance,
        }
    }

    /// Refreshes the memory from a new sighting of the same enemy.
    pub fn refresh(&mut self, sighting: &Sighting) {
        self.last_known_position = sighting.origin;
        self.time_since_seen = 0.0;
        self.occluded = false;
        self.last_known_distance = sighting.distance;
    }

    /// Ages the memory by `dt` with the enemy out of sight.
    ///
    /// Returns whether the enemy is still tracked: the published rule is
    /// that an occluded enemy inside [`OCCLUDED_MEMORY_DISTANCE`] stays
    /// known, so beyond that the caller should forget it.
    pub fn occlude(&mut self, dt: f32, current_distance: Option<f32>) -> bool {
        self.occluded = true;
        self.time_since_seen += dt;
        if let Some(distance) = current_distance {
            self.last_known_distance = distance;
        }
        self.last_known_distance <= OCCLUDED_MEMORY_DISTANCE
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Candidate, Senses, SoundEvent, SoundKind, SoundList, Viewer, listen, look, select_enemy,
    };
    use crate::state::{Classification, Conditions, Relationship, RelationshipTable};
    use glam::Vec3;
    use hecs::World;

    fn two_entities() -> (hecs::Entity, hecs::Entity) {
        let mut world = World::new();
        (world.spawn((1u32,)), world.spawn((2u32,)))
    }

    fn viewer(entity: hecs::Entity) -> Viewer {
        Viewer {
            entity,
            origin: Vec3::ZERO,
            view_ofs: Vec3::new(0.0, 0.0, 28.0),
            forward: Vec3::X,
            classification: Classification::HumanMilitary,
        }
    }

    fn candidate(entity: hecs::Entity, origin: Vec3) -> Candidate {
        Candidate {
            entity,
            classification: Classification::Player,
            origin,
            view_ofs: Vec3::new(0.0, 0.0, 28.0),
            forward: -Vec3::X,
            alive: true,
            is_client: true,
        }
    }

    #[test]
    fn a_hostile_in_front_becomes_the_enemy() {
        let (me, them) = two_entities();
        let result = look(
            &viewer(me),
            &Senses::default(),
            &[candidate(them, Vec3::new(256.0, 0.0, 0.0))],
            &RelationshipTable::provisional(),
            &super::SightContext::empty(),
        );
        assert!(result.conditions.contains(Conditions::SEE_ENEMY));
        assert!(result.conditions.contains(Conditions::SEE_HATE));
        assert!(result.conditions.contains(Conditions::SEE_CLIENT));
        let enemy = result.enemy.expect("a hostile candidate was in view");
        assert_eq!(enemy.entity, them);
        assert_eq!(enemy.relationship, Relationship::Hate);
        assert!(enemy.facing_viewer);
    }

    #[test]
    fn candidates_behind_and_beyond_the_look_distance_are_not_seen() {
        let (me, them) = two_entities();
        let table = RelationshipTable::provisional();
        let behind = look(
            &viewer(me),
            &Senses::default(),
            &[candidate(them, Vec3::new(-256.0, 0.0, 0.0))],
            &table,
            &super::SightContext::empty(),
        );
        assert!(behind.enemy.is_none());

        let senses = Senses {
            look_distance: 64.0,
            ..Senses::default()
        };
        let far = look(
            &viewer(me),
            &senses,
            &[candidate(them, Vec3::new(256.0, 0.0, 0.0))],
            &table,
            &super::SightContext::empty(),
        );
        assert!(far.enemy.is_none());
        assert!(far.conditions.is_empty());
    }

    #[test]
    fn an_omnidirectional_viewer_sees_behind_itself() {
        let (me, them) = two_entities();
        let result = look(
            &viewer(me),
            &Senses::omnidirectional(2_048.0),
            &[candidate(them, Vec3::new(-256.0, 0.0, 0.0))],
            &RelationshipTable::provisional(),
            &super::SightContext::empty(),
        );
        assert!(result.enemy.is_some());
    }

    #[test]
    fn the_viewer_never_sees_itself() {
        let (me, _) = two_entities();
        let result = look(
            &viewer(me),
            &Senses::default(),
            &[candidate(me, Vec3::new(256.0, 0.0, 0.0))],
            &RelationshipTable::provisional(),
            &super::SightContext::empty(),
        );
        assert!(result.visible.is_empty());
    }

    #[test]
    fn enemy_selection_prefers_relationship_then_distance() {
        let mut world = World::new();
        let near_dislike = world.spawn((0u8,));
        let far_hate = world.spawn((0u8,));
        let nearer_hate = world.spawn((0u8,));
        let make = |entity, relationship, distance| super::Sighting {
            entity,
            classification: Classification::Player,
            relationship,
            origin: Vec3::X * distance,
            eye: Vec3::X * distance,
            distance,
            alive: true,
            facing_viewer: false,
        };
        let visible = [
            make(near_dislike, Relationship::Dislike, 10.0),
            make(far_hate, Relationship::Hate, 900.0),
            make(nearer_hate, Relationship::Hate, 100.0),
        ];
        let chosen = select_enemy(&visible).expect("hostiles present");
        assert_eq!(chosen.entity, nearer_hate);

        let dead_only = [super::Sighting {
            alive: false,
            ..make(far_hate, Relationship::Nemesis, 5.0)
        }];
        assert!(select_enemy(&dead_only).is_none());
    }

    #[test]
    fn hearing_scales_with_sensitivity_and_prefers_danger() {
        let mut sounds = SoundList::new();
        assert!(sounds.push(SoundEvent::new(
            SoundKind::World,
            Vec3::new(150.0, 0.0, 0.0),
            200.0
        )));
        assert!(sounds.push(SoundEvent::new(
            SoundKind::Danger,
            Vec3::new(300.0, 0.0, 0.0),
            200.0
        )));
        assert_eq!(sounds.len(), 2);

        let plain = listen(Vec3::ZERO, &Senses::default(), &sounds);
        assert!(plain.conditions.contains(Conditions::HEAR_SOUND));
        assert!(!plain.conditions.contains(Conditions::HEAR_DANGER));

        let sensitive = Senses {
            hearing_sensitivity: super::TENTACLE_HEARING_SENSITIVITY,
            ..Senses::default()
        };
        let keen = listen(Vec3::ZERO, &sensitive, &sounds);
        assert!(keen.conditions.contains(Conditions::HEAR_DANGER));
        assert_eq!(
            keen.best.expect("danger is audible at sensitivity 2").kind,
            SoundKind::Danger
        );
    }

    #[test]
    fn scents_set_smell_not_hearing() {
        let mut sounds = SoundList::new();
        sounds.push(SoundEvent::new(SoundKind::Meat, Vec3::ZERO, 100.0));
        let result = listen(Vec3::ZERO, &Senses::default(), &sounds);
        assert!(result.conditions.contains(Conditions::SMELL));
        assert!(!result.conditions.contains(Conditions::HEAR_SOUND));
    }

    #[test]
    fn the_sound_list_is_bounded_and_expires() {
        let mut sounds = SoundList::new();
        for _ in 0..(super::MAX_SOUNDS + 8) {
            sounds.push(SoundEvent::new(SoundKind::World, Vec3::ZERO, 10.0));
        }
        assert_eq!(sounds.len(), super::MAX_SOUNDS);
        assert!(!sounds.push(SoundEvent::new(SoundKind::World, Vec3::ZERO, 10.0)));
        sounds.expire(1.0);
        assert!(sounds.is_empty());

        let mut rejecting = SoundList::new();
        assert!(!rejecting.push(SoundEvent::new(SoundKind::World, Vec3::NAN, 10.0)));
        assert!(!rejecting.push(SoundEvent::new(SoundKind::World, Vec3::ZERO, -1.0)));
        rejecting.clear();
        assert!(rejecting.is_empty());
    }

    #[test]
    fn occlusion_keeps_a_near_enemy_and_forgets_a_far_one() {
        let (_, them) = two_entities();
        let sighting = super::Sighting {
            entity: them,
            classification: Classification::Player,
            relationship: Relationship::Hate,
            origin: Vec3::new(100.0, 0.0, 0.0),
            eye: Vec3::new(100.0, 0.0, 28.0),
            distance: 100.0,
            alive: true,
            facing_viewer: true,
        };
        let mut memory = super::EnemyMemory::seen(&sighting);
        assert!(memory.occlude(0.01, None));
        assert!(memory.occluded);
        assert_eq!(memory.last_known_position, Vec3::new(100.0, 0.0, 0.0));
        assert!(!memory.occlude(0.01, Some(1_000.0)));

        let mut refreshed = super::EnemyMemory::seen(&sighting);
        refreshed.occlude(0.5, None);
        refreshed.refresh(&sighting);
        assert!(!refreshed.occluded);
        assert!(refreshed.time_since_seen.abs() < f32::EPSILON);
    }
}
