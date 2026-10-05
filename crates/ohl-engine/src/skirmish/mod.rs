//! Local skirmish: a deathmatch played offline against bots.
//!
//! A skirmish turns an already-loaded [`crate::Game`] on a deathmatch map
//! into a free-for-all match: the human and every bot spawn at the map's
//! `info_player_deathmatch` points, carry the deathmatch starting
//! equipment, fight with the same weapon code the human's own weapons run
//! through, score frags, respawn after dying, and the match ends at a frag
//! or time limit. Nothing here talks to a network: "multiplayer" is the
//! rules and the opponents, all simulated inside the one fixed step every
//! other system already shares.
//!
//! # What a bot is
//!
//! A bot is a second walking player, not a monster. It owns its own
//! [`ohl_physics::PlayerController`] (the same collision, step-up, gravity
//! and jump the human moves with), its own [`ohl_player::Player`] (health,
//! HEV armour, fall damage) and its own `crate::combat::CombatState` (the
//! weapon inventory, ammo ledger and firing state machine the human's own
//! weapons run through), and a [`bot::Brain`] that turns what it can see
//! into the same kind of held input a keyboard produces. Its world entity
//! carries a [`BotBody`], a `Transform`, `ohl_combat::Health`/`Armor`
//! mirrors (so a blast finds it) and, when the payload publishes the player
//! model, a `StudioAnim` that draws it and gives it hitboxes.
//!
//! # Sources
//!
//! Every gameplay rule a published page states is cited in
//! `docs/FORMAT_SOURCES.md`, "Local skirmish (deathmatch against bots)":
//! spawn points (TWHL `info_player_deathmatch`), the starting equipment
//! (TWHL "Tutorial: Half-Life Deathmatch mapping"), the weapon, ammo and
//! item respawn delays (TWHL `weapon_9mmhandgun`, `ammo_9mmclip`,
//! `item_healthkit`; VDC `weapon_crowbar (GoldSrc)`, `item_battery
//! (GoldSrc)`), the "Not In Deathmatch" spawnflag (VDC `weapon_crowbar
//! (GoldSrc)`), the frag and time limits and the intermission length (VDC
//! "List of Half-Life console commands and variables": `mp_fraglimit`,
//! `mp_timelimit`, `mp_chattime`, `mp_forcerespawn`), and the player model
//! path (TWHL "Reference: Entities and their models"). Everything else —
//! the bot's behaviour, its skill tuning, the respawn delays a bot and a
//! clicking human wait, the starting ammunition count, the suicide frag
//! penalty — is project-authored and labelled so where it is defined.
//!
//! Nothing here logs. Positions and classnames are map-derived; bot names
//! are project-authored.

mod bot;
pub(crate) mod nav;

use glam::Vec3;
use ohl_combat::{DamageType, EntityId, WeaponId};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;

use crate::components::{Pickup, StudioAnim};
use crate::ids::entity_id;
use crate::level::Level;
use crate::systems::QueuedDamage;

pub(crate) use bot::{Bot, give_deathmatch_loadout};
use nav::NavGraph;

/// The most bots one skirmish runs. Project-authored: enough for a full
/// arena, few enough that every bot's per-step traces stay cheap.
pub const MAX_BOTS: u8 = 15;

/// The studio model a bot is drawn with: TWHL's "Reference: Entities and
/// their models" lists it against `monster_hevsuit_dead`, the dead HEV
/// scientist placed in maps, which is the same player model the client
/// draws (`docs/FORMAT_SOURCES.md`, "Local skirmish").
pub const PLAYER_MODEL_PATH: &str = "models/player.mdl";

/// The multiplayer spawn point classname (TWHL `info_player_deathmatch`:
/// "Defines player spawn positions for multiplayer games").
pub const DEATHMATCH_SPAWN_CLASSNAME: &str = "info_player_deathmatch";

/// The single-player start, which the same TWHL page documents as the
/// "backup" a map with no `info_player_deathmatch` respawns players at.
const BACKUP_SPAWN_CLASSNAME: &str = "info_player_start";

/// "In a deathmatch game, the weapon respawns 20 seconds after being
/// collected" (TWHL `weapon_9mmhandgun`; VDC `weapon_crowbar (GoldSrc)`).
pub const WEAPON_RESPAWN_SECONDS: f32 = 20.0;

/// "In a deathmatch game, ammo respawns 20 seconds after being collected"
/// (TWHL `ammo_9mmclip`).
pub const AMMO_RESPAWN_SECONDS: f32 = 20.0;

/// "In a deathmatch game, the item respawns 30 seconds after being
/// collected" (TWHL `item_healthkit`; VDC `item_battery (GoldSrc)`). Also
/// applied to the long jump module and the suit, which no reviewed page
/// gives a delay for — project-authored, the same as the items that do.
pub const ITEM_RESPAWN_SECONDS: f32 = 30.0;

/// The "Not In Deathmatch" spawnflag: "Prevent this entity from attempting
/// to spawn when `deathmatch` (multiplayer) is enabled" (VDC
/// `weapon_crowbar (GoldSrc)`). A pickup carrying it never appears in a
/// skirmish.
pub const NOT_IN_DEATHMATCH_SPAWNFLAG: u32 = 2048;

/// How long the scoreboard holds once the match is over before the host
/// goes back to the menu: `mp_chattime`'s published default, `10` (VDC
/// "List of Half-Life console commands and variables").
pub const INTERMISSION_SECONDS: f32 = 10.0;

/// How long a dead bot waits before it respawns. Project-authored.
pub const BOT_RESPAWN_SECONDS: f32 = 2.0;

/// How long a dead human must wait before a click respawns them, so the
/// shot that was still held when they died does not respawn them at once.
/// Project-authored.
pub const RESPAWN_CLICK_DELAY_SECONDS: f32 = 1.0;

/// How long a dead human waits before [`SkirmishConfig::force_respawn`]
/// respawns them without a click (`mp_forcerespawn`: "Automatically
/// respawn players without waiting for them to click"). The delay itself
/// is project-authored.
pub const FORCE_RESPAWN_SECONDS: f32 = 5.0;

/// The name the human is listed under on the scoreboard.
pub const HUMAN_NAME: &str = "Player";

/// Bot names, assigned in order. Project-authored: the NATO phonetic
/// alphabet, one per [`MAX_BOTS`] slot.
const BOT_NAMES: [&str; MAX_BOTS as usize] = [
    "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliett",
    "Kilo", "Lima", "Mike", "November", "Oscar",
];

/// How far apart two combatants must be for a spawn point to count as
/// free, so a respawn never lands inside someone. Project-authored: two
/// standing hull widths.
const SPAWN_CLEARANCE: f32 = 64.0;

/// How hard bots are to beat. Every number each level selects lives in
/// `bot::SkillParams` and is project-authored tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotSkill {
    /// Slow to react, slow to turn, and inaccurate.
    Easy,
    /// The default.
    #[default]
    Normal,
    /// Quick, accurate and evasive.
    Hard,
}

/// How a skirmish is set up: everything the host chooses rather than the
/// map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkirmishConfig {
    /// How many bots join, clamped to `0..=`[`MAX_BOTS`].
    pub bots: u8,
    /// How hard every bot plays.
    pub bot_skill: BotSkill,
    /// The frag count that ends the match; `0` for none (`mp_fraglimit`,
    /// "the number of kills at which the map ends").
    pub frag_limit: u32,
    /// How long the match lasts, in seconds; `0.0` for no limit
    /// (`mp_timelimit`, published in minutes; the host converts).
    pub time_limit_seconds: f32,
    /// Respawn the human automatically after [`FORCE_RESPAWN_SECONDS`]
    /// instead of waiting for a click (`mp_forcerespawn`).
    pub force_respawn: bool,
    /// Seeds the bots' own random stream. Two skirmishes on the same map
    /// with the same seed and the same human input play out identically.
    pub seed: u64,
}

impl Default for SkirmishConfig {
    fn default() -> Self {
        Self {
            bots: 3,
            bot_skill: BotSkill::Normal,
            frag_limit: 10,
            time_limit_seconds: 600.0,
            force_respawn: false,
            seed: 0x534B_4952_4D49_5348,
        }
    }
}

/// One scoreboard line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRow {
    /// Who: [`HUMAN_NAME`] or a bot's project-authored name.
    pub name: String,
    /// Frags scored, less one per suicide (so it can go negative).
    pub frags: i32,
    /// Times killed, by anyone or anything.
    pub deaths: u32,
    /// Whether this row is the human at this keyboard.
    pub is_human: bool,
    /// Whether this combatant is currently alive.
    pub alive: bool,
}

/// The match as the host shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SkirmishStatus {
    /// Every combatant, best first: most frags, then fewest deaths, then
    /// the human ahead of a tied bot.
    pub scoreboard: Vec<ScoreRow>,
    /// [`SkirmishConfig::frag_limit`]; `0` for none.
    pub frag_limit: u32,
    /// Seconds left on the clock, when the match has a time limit.
    pub seconds_left: Option<f32>,
    /// Who won, once the match is over.
    pub winner: Option<String>,
    /// Seconds of intermission left, once the match is over.
    pub intermission_left: Option<f32>,
    /// Whether the human is dead and waiting to respawn.
    pub human_dead: bool,
    /// Whether a click would respawn the human right now.
    pub respawn_ready: bool,
    /// The human's own frag count, for the HUD.
    pub human_frags: i32,
}

/// Something a skirmish step produced for the host to announce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkirmishEvent {
    /// Someone died. `killer` is `None` for a death nobody is credited
    /// with (a fall, a hazard, a crushing door) and equals `victim` for a
    /// suicide by one's own weapon.
    Frag {
        /// Who scored, when anyone did.
        killer: Option<String>,
        /// Who died.
        victim: String,
        /// What did it, as a short project-authored label.
        weapon: Option<&'static str>,
        /// Whether the human was the killer or the victim.
        involves_human: bool,
    },
    /// The frag or time limit was reached.
    MatchOver {
        /// Whoever tops the scoreboard.
        winner: String,
    },
    /// The human respawned.
    HumanRespawned,
}

/// Marks a bot's world entity, and whether it is currently alive (a dead
/// bot's entity stays in the world as its corpse until it respawns, but
/// is no longer shootable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotBody {
    /// Index into the skirmish's own bot list.
    pub index: u8,
    /// Whether the bot is alive.
    pub alive: bool,
}

/// One place a combatant can (re)spawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpawnPoint {
    pub(crate) origin: Vec3,
    pub(crate) yaw: f32,
}

/// Who is in the match, by scoreboard slot: `0` is the human, `1..` the
/// bots in [`SkirmishState::bots`] order.
type Slot = usize;

const HUMAN_SLOT: Slot = 0;

/// One scoreboard slot's tally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    frags: i32,
    deaths: u32,
}

/// The human, as the skirmish phases see them each step.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HumanView {
    pub(crate) entity: Entity,
    pub(crate) origin: Vec3,
    pub(crate) eye: Vec3,
    pub(crate) alive: bool,
    /// Whether the human's fire button is down this step (bots hear it).
    pub(crate) firing: bool,
}

/// The skirmish's own state, owned by [`crate::systems::Systems`].
pub(crate) struct SkirmishState {
    config: SkirmishConfig,
    spawns: Vec<SpawnPoint>,
    nav: NavGraph,
    pub(crate) bots: Vec<Bot>,
    tallies: Vec<Tally>,
    /// Seconds since the human died, while they are dead.
    human_dead_for: Option<f32>,
    /// Whether every respawn button has been let go since the human died,
    /// so only a fresh click respawns them.
    human_released: bool,
    /// Seconds until each taken pickup reappears.
    pickup_timers: std::collections::BTreeMap<Entity, f32>,
    /// Pickups that never appear in a skirmish ("Not In Deathmatch").
    excluded_pickups: std::collections::BTreeSet<Entity>,
    /// Seconds of match played.
    clock: f32,
    /// Seconds of intermission played, once the match is over.
    intermission: Option<f32>,
    winner: Option<Slot>,
    events: Vec<SkirmishEvent>,
    rng: ohl_ai::Pcg32,
    player_model: Option<usize>,
}

/// How many deathmatch spawn points `map_bytes`' entity lump declares, or
/// `None` when the bytes are not a readable map. A map with at least one
/// is offered as a skirmish arena.
#[must_use]
pub fn deathmatch_spawn_count(map_bytes: &[u8]) -> Option<usize> {
    let limits = ohl_formats::bsp30::Limits::default();
    let bsp = ohl_formats::bsp30::Bsp::parse(map_bytes, &limits).ok()?;
    let entities = bsp.entities(&limits).ok()?;
    let defs =
        ohl_game::keyvalues::parse_entities(&entities, &ohl_game::keyvalues::Limits::default());
    Some(
        defs.iter()
            .filter(|def| def.classname == DEATHMATCH_SPAWN_CLASSNAME)
            .count(),
    )
}

/// The spawn points `level` declares: every `info_player_deathmatch`, or
/// every `info_player_start` when it has none.
pub(crate) fn spawn_points(level: &Level) -> Vec<SpawnPoint> {
    let collect = |classname: &str| -> Vec<SpawnPoint> {
        level
            .defs
            .iter()
            .filter(|def| def.classname == classname)
            .map(|def| SpawnPoint {
                origin: Vec3::from_array(def.origin),
                yaw: def.angles[1],
            })
            .filter(|spawn| spawn.origin.is_finite() && spawn.yaw.is_finite())
            .collect()
    };
    let deathmatch = collect(DEATHMATCH_SPAWN_CLASSNAME);
    if deathmatch.is_empty() {
        collect(BACKUP_SPAWN_CLASSNAME)
    } else {
        deathmatch
    }
}

/// A short project-authored label for what killed someone, for the kill
/// feed.
fn weapon_label(weapon: WeaponId) -> &'static str {
    match weapon {
        WeaponId::Crowbar => "crowbar",
        WeaponId::Glock => "9mm pistol",
        WeaponId::Python => ".357",
        WeaponId::Mp5 => "MP5",
        WeaponId::Shotgun => "shotgun",
        WeaponId::Crossbow => "crossbow",
        WeaponId::Rpg => "RPG",
        WeaponId::Gauss => "gauss",
        WeaponId::Egon => "egon",
        WeaponId::HornetGun => "hivehand",
        WeaponId::HandGrenade => "grenade",
        WeaponId::Satchel => "satchel",
        WeaponId::Tripmine => "tripmine",
        WeaponId::Snark => "snark",
    }
}

/// The kill feed label for a hit of `kind` by an attacker currently
/// holding `weapon`: a blast is an explosion whatever is in hand now (the
/// rocket may have been fired before a switch), a club is a crowbar.
fn kill_label(kind: DamageType, weapon: Option<WeaponId>) -> Option<&'static str> {
    if kind.contains(DamageType::BLAST) {
        return Some(match weapon {
            Some(WeaponId::Rpg) => "RPG",
            _ => "explosion",
        });
    }
    if kind.contains(DamageType::CLUB) {
        return Some("crowbar");
    }
    weapon.map(weapon_label)
}

impl SkirmishState {
    /// A fresh match over `level`: spawn points read, the bots' walkable
    /// graph flood-filled from every spawn point and pickup, and `config`
    /// clamped. Bots are created by [`Self::add_bots`] once the level's
    /// player model slot is known.
    pub(crate) fn new(level: &Level, config: SkirmishConfig, player_model: Option<usize>) -> Self {
        let config = SkirmishConfig {
            bots: config.bots.min(MAX_BOTS),
            frag_limit: config.frag_limit,
            time_limit_seconds: if config.time_limit_seconds.is_finite() {
                config.time_limit_seconds.max(0.0)
            } else {
                0.0
            },
            ..config
        };
        let spawns = spawn_points(level);
        let nav = level
            .collision
            .as_ref()
            .map_or_else(NavGraph::default, |collision| {
                let mut seeds: Vec<Vec3> = spawns.iter().map(|spawn| spawn.origin).collect();
                for (_, transform) in &mut level.registry.world.query::<(&Pickup, &Transform)>() {
                    // A pickup's own origin sits on (or a little above) its
                    // floor; lift it to a standing hull's centre so the
                    // settle below starts in open space.
                    seeds.push(transform.origin + Vec3::Z * 36.0);
                }
                NavGraph::build(collision, &ohl_physics::MoveConfig::default(), &seeds)
            });
        let mut tallies = Vec::with_capacity(usize::from(config.bots) + 1);
        tallies.push(Tally::default());
        Self {
            rng: ohl_ai::Pcg32::new(config.seed),
            config,
            spawns,
            nav,
            bots: Vec::new(),
            tallies,
            human_dead_for: None,
            human_released: true,
            pickup_timers: std::collections::BTreeMap::new(),
            excluded_pickups: std::collections::BTreeSet::new(),
            clock: 0.0,
            intermission: None,
            winner: None,
            events: Vec::new(),
            player_model,
        }
    }

    /// How many nodes the bots' walkable graph holds. Data, never a log
    /// line.
    pub(crate) fn nav_node_count(&self) -> usize {
        self.nav.len()
    }

    /// Hides every pickup flagged "Not In Deathmatch" for the whole match.
    pub(crate) fn exclude_non_deathmatch_pickups(&mut self, level: &mut Level) {
        let flagged: Vec<Entity> = level
            .defs
            .iter()
            .zip(level.registry.entities.iter())
            .filter(|(def, _)| def.spawnflags & NOT_IN_DEATHMATCH_SPAWNFLAG != 0)
            .map(|(_, entity)| *entity)
            .collect();
        for entity in flagged {
            if let Ok(mut pickup) = level.registry.world.get::<&mut Pickup>(entity) {
                pickup.taken = true;
                self.excluded_pickups.insert(entity);
            }
        }
    }

    /// Creates [`SkirmishConfig::bots`] bots and spawns each one.
    pub(crate) fn add_bots(&mut self, level: &mut Level, human: HumanView) {
        for index in 0..self.config.bots {
            let seed = (u64::from(self.rng.next_u32()) << 32) | u64::from(self.rng.next_u32());
            let bot = Bot::new(
                level,
                index,
                BOT_NAMES[usize::from(index)],
                self.config.bot_skill,
                seed,
                self.player_model,
            );
            self.bots.push(bot);
            self.tallies.push(Tally::default());
            let slot = self.bots.len();
            let spawn = self.pick_spawn(level, human, Some(slot));
            self.bots[slot - 1].respawn(level, spawn);
        }
    }

    /// Chooses where combatant `slot` (re)spawns: a random point among the
    /// third farthest from every living opponent, never one another
    /// combatant is standing on. Project-authored: no reviewed page says
    /// how GoldSrc picks among several `info_player_deathmatch` points.
    pub(crate) fn pick_spawn(
        &mut self,
        level: &Level,
        human: HumanView,
        slot: Option<Slot>,
    ) -> SpawnPoint {
        let fallback = SpawnPoint {
            origin: level
                .spawn
                .map_or(Vec3::ZERO, |spawn| Vec3::from_array(spawn.origin)),
            yaw: level.spawn.map_or(0.0, |spawn| spawn.yaw),
        };
        if self.spawns.is_empty() {
            return fallback;
        }
        let others: Vec<Vec3> = self
            .living_positions(human)
            .filter(|(other, _)| Some(*other) != slot)
            .map(|(_, position)| position)
            .collect();
        let mut scored: Vec<(f32, usize)> = self
            .spawns
            .iter()
            .enumerate()
            .map(|(index, spawn)| {
                let nearest = others
                    .iter()
                    .map(|position| position.distance(spawn.origin))
                    .fold(f32::INFINITY, f32::min);
                (nearest, index)
            })
            .collect();
        let clear: Vec<(f32, usize)> = scored
            .iter()
            .copied()
            .filter(|(nearest, _)| *nearest > SPAWN_CLEARANCE)
            .collect();
        if !clear.is_empty() {
            scored = clear;
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let pool = scored.len().div_ceil(3).max(1);
        let pick = self.rng.below(u32::try_from(pool).unwrap_or(1)) as usize;
        self.spawns[scored[pick].1]
    }

    /// Every living combatant's slot and hull origin.
    fn living_positions(&self, human: HumanView) -> impl Iterator<Item = (Slot, Vec3)> + '_ {
        let human = human.alive.then_some((HUMAN_SLOT, human.origin));
        human.into_iter().chain(
            self.bots
                .iter()
                .enumerate()
                .filter(|(_, bot)| bot.alive())
                .map(|(index, bot)| (index + 1, bot.origin())),
        )
    }

    /// Whether the match has ended.
    pub(crate) fn is_over(&self) -> bool {
        self.winner.is_some()
    }

    /// Which scoreboard slot `id` (an attacker's combat id) belongs to.
    fn slot_of(&self, id: EntityId, human: Entity) -> Option<Slot> {
        if id == entity_id(human) {
            return Some(HUMAN_SLOT);
        }
        self.bots
            .iter()
            .position(|bot| entity_id(bot.entity) == id)
            .map(|index| index + 1)
    }

    fn name_of(&self, slot: Slot) -> String {
        if slot == HUMAN_SLOT {
            HUMAN_NAME.to_string()
        } else {
            self.bots
                .get(slot - 1)
                .map_or_else(String::new, |bot| bot.name.to_string())
        }
    }

    /// Records one death: the victim's tally, the killer's frag (or the
    /// victim's suicide penalty), the kill feed event, and the frag limit.
    fn record_death(&mut self, victim: Slot, killer: Option<Slot>, weapon: Option<&'static str>) {
        // Nothing scores once the match is over.
        if self.winner.is_some() {
            return;
        }
        if let Some(tally) = self.tallies.get_mut(victim) {
            tally.deaths = tally.deaths.saturating_add(1);
        }
        match killer {
            Some(killer) if killer != victim => {
                if let Some(tally) = self.tallies.get_mut(killer) {
                    tally.frags = tally.frags.saturating_add(1);
                }
            }
            // A suicide, or a death nobody is credited with, costs the
            // victim a frag. Project-authored: no reviewed page states the
            // penalty.
            _ => {
                if let Some(tally) = self.tallies.get_mut(victim) {
                    tally.frags = tally.frags.saturating_sub(1);
                }
            }
        }
        self.events.push(SkirmishEvent::Frag {
            killer: killer.map(|slot| self.name_of(slot)),
            victim: self.name_of(victim),
            weapon,
            involves_human: victim == HUMAN_SLOT || killer == Some(HUMAN_SLOT),
        });
        if self.config.frag_limit > 0
            && let Some(killer) = killer
            && self
                .tallies
                .get(killer)
                .is_some_and(|tally| i64::from(tally.frags) >= i64::from(self.config.frag_limit))
        {
            self.end_match();
        }
    }

    fn end_match(&mut self) {
        if self.winner.is_some() {
            return;
        }
        let winner = self.ranking().first().copied().unwrap_or(HUMAN_SLOT);
        self.winner = Some(winner);
        self.intermission = Some(0.0);
        self.events.push(SkirmishEvent::MatchOver {
            winner: self.name_of(winner),
        });
    }

    /// Every slot, best first: most frags, then fewest deaths, then the
    /// human ahead of a tied bot, then slot order.
    fn ranking(&self) -> Vec<Slot> {
        let mut slots: Vec<Slot> = (0..self.tallies.len()).collect();
        slots.sort_by(|a, b| {
            let (ta, tb) = (self.tallies[*a], self.tallies[*b]);
            tb.frags
                .cmp(&ta.frags)
                .then(ta.deaths.cmp(&tb.deaths))
                .then(a.cmp(b))
        });
        slots
    }

    /// The match as the host shows it.
    pub(crate) fn status(&self, human_alive: bool) -> SkirmishStatus {
        let scoreboard = self
            .ranking()
            .into_iter()
            .map(|slot| ScoreRow {
                name: self.name_of(slot),
                frags: self.tallies[slot].frags,
                deaths: self.tallies[slot].deaths,
                is_human: slot == HUMAN_SLOT,
                alive: if slot == HUMAN_SLOT {
                    human_alive
                } else {
                    self.bots[slot - 1].alive()
                },
            })
            .collect();
        SkirmishStatus {
            scoreboard,
            frag_limit: self.config.frag_limit,
            seconds_left: (self.config.time_limit_seconds > 0.0)
                .then(|| (self.config.time_limit_seconds - self.clock).max(0.0)),
            winner: self.winner.map(|slot| self.name_of(slot)),
            intermission_left: self
                .intermission
                .map(|played| (INTERMISSION_SECONDS - played).max(0.0)),
            human_dead: self.human_dead_for.is_some(),
            respawn_ready: self.winner.is_none()
                && self
                    .human_dead_for
                    .is_some_and(|dead| dead >= RESPAWN_CLICK_DELAY_SECONDS),
            human_frags: self.tallies[HUMAN_SLOT].frags,
        }
    }

    /// Takes every event collected since the last call.
    pub(crate) fn drain_events(&mut self) -> Vec<SkirmishEvent> {
        std::mem::take(&mut self.events)
    }

    /// Phase 9, before the human's own damage resolves: applies (and
    /// removes from `queue`) every hit aimed at a bot, recording any death
    /// it causes as it happens.
    pub(crate) fn resolve_bot_damage(
        &mut self,
        level: &mut Level,
        queue: &mut Vec<QueuedDamage>,
        human_weapon: Option<WeaponId>,
    ) {
        let human = level.player;
        let mut kept = Vec::with_capacity(queue.len());
        for queued in queue.drain(..) {
            let Some(index) = self.bots.iter().position(|bot| bot.entity == queued.target) else {
                kept.push(queued);
                continue;
            };
            if self.winner.is_some() || !self.bots[index].alive() {
                continue;
            }
            let dead = self.bots[index].take_damage(level, &queued.info);
            if dead {
                let killer = queued
                    .info
                    .attacker
                    .and_then(|attacker| self.slot_of(attacker, human));
                let weapon = match killer {
                    Some(HUMAN_SLOT) => kill_label(queued.info.kind, human_weapon),
                    Some(slot) => kill_label(
                        queued.info.kind,
                        self.bots.get(slot - 1).and_then(Bot::selected_weapon),
                    ),
                    None => None,
                };
                self.bots[index].die(level);
                // Recorded at once, so a frag that ends the match stops
                // every later hit in this batch from scoring.
                self.record_death(index + 1, killer, weapon);
            }
        }
        *queue = kept;
    }

    /// Records the human's death once: `attacker` and `kind` are the hit
    /// that killed them (phase 9's report), or `None`/generic for a death
    /// nobody is credited with (a fall, a hazard, a crusher).
    pub(crate) fn note_human_death(
        &mut self,
        human: Entity,
        human_weapon: Option<WeaponId>,
        attacker: Option<EntityId>,
        kind: DamageType,
    ) {
        if self.human_dead_for.is_some() {
            return;
        }
        self.human_dead_for = Some(0.0);
        self.human_released = false;
        let killer = attacker.and_then(|attacker| self.slot_of(attacker, human));
        let weapon = match killer {
            Some(HUMAN_SLOT) => kill_label(kind, human_weapon),
            Some(slot) => kill_label(kind, self.bots.get(slot - 1).and_then(Bot::selected_weapon)),
            None => None,
        };
        self.record_death(HUMAN_SLOT, killer, weapon);
    }

    /// Phase 11b: every living bot touches the pickups within reach, and
    /// every taken pickup counts down to its respawn.
    pub(crate) fn pickups(&mut self, level: &mut Level, dt: f32) {
        if self.winner.is_none() {
            for bot in &mut self.bots {
                if bot.alive() {
                    bot.touch_pickups(level);
                }
            }
        }
        let taken: Vec<(Entity, ohl_combat::PickupKind)> = level
            .registry
            .world
            .query::<(Entity, &Pickup)>()
            .iter()
            .filter(|(entity, pickup)| pickup.taken && !self.excluded_pickups.contains(entity))
            .map(|(entity, pickup)| (entity, pickup.kind))
            .collect();
        for (entity, kind) in taken {
            let Some(delay) = respawn_delay(kind) else {
                continue;
            };
            let timer = self.pickup_timers.entry(entity).or_insert(delay);
            *timer -= dt;
            if *timer <= 0.0 {
                self.pickup_timers.remove(&entity);
                if let Ok(mut pickup) = level.registry.world.get::<&mut Pickup>(entity) {
                    pickup.taken = false;
                }
            }
        }
    }

    /// Phase 12b: every living bot's hull opens the touch-opened doors it
    /// walks into, the way a monster's does, and is carried by any
    /// `trigger_teleport` it steps into.
    pub(crate) fn touch_world(&mut self, level: &mut Level) {
        for bot in &mut self.bots {
            if bot.alive() {
                bot.touch_world(level);
            }
        }
    }

    /// Phase 13c: the match clock, the time limit, bot respawns and the
    /// intermission. Returns whether the human should respawn now (the
    /// caller owns the human's state and does the respawn itself, then
    /// calls [`Self::human_respawned`]).
    pub(crate) fn rules(
        &mut self,
        level: &mut Level,
        human: HumanView,
        respawn_pressed: bool,
        dt: f32,
    ) -> bool {
        if let Some(intermission) = self.intermission.as_mut() {
            *intermission += dt;
            return false;
        }
        self.clock += dt;
        if self.config.time_limit_seconds > 0.0 && self.clock >= self.config.time_limit_seconds {
            self.end_match();
            return false;
        }
        for index in 0..self.bots.len() {
            if self.bots[index].ready_to_respawn(dt) {
                let spawn = self.pick_spawn(level, human, Some(index + 1));
                self.bots[index].respawn(level, spawn);
            }
        }
        self.separate(level, human);
        let Some(dead_for) = self.human_dead_for.as_mut() else {
            return false;
        };
        *dead_for += dt;
        let dead_for = *dead_for;
        if !respawn_pressed {
            self.human_released = true;
        }
        (self.human_released && respawn_pressed && dead_for >= RESPAWN_CLICK_DELAY_SECONDS)
            || (self.config.force_respawn && dead_for >= FORCE_RESPAWN_SECONDS)
    }

    /// The human has respawned; stop counting their death.
    pub(crate) fn human_respawned(&mut self) {
        self.human_dead_for = None;
        self.events.push(SkirmishEvent::HumanRespawned);
    }

    /// Nudges every living bot out of every other combatant it overlaps.
    /// Players do not collide with each other in the collision model (it
    /// holds only the world and brush entities), so without this two bots
    /// chasing the same item stand inside one another. Only bots move; the
    /// human is never pushed. Project-authored.
    fn separate(&mut self, level: &mut Level, human: HumanView) {
        let positions: Vec<(Slot, Vec3)> = self.living_positions(human).collect();
        for index in 0..self.bots.len() {
            if !self.bots[index].alive() {
                continue;
            }
            let own = self.bots[index].origin();
            let mut push = Vec3::ZERO;
            for (slot, other) in &positions {
                if *slot == index + 1 {
                    continue;
                }
                let offset = (own - *other).truncate();
                let distance = offset.length();
                if distance < 32.0 && (own.z - other.z).abs() < 72.0 {
                    let away = if distance > 0.01 {
                        offset / distance
                    } else {
                        glam::Vec2::new(1.0, 0.0)
                    };
                    push += (away * (32.0 - distance)).extend(0.0);
                }
            }
            if push != Vec3::ZERO {
                self.bots[index].nudge(level, push.clamp_length_max(8.0));
            }
        }
    }

    /// Phase 6b: every living bot senses, decides, moves and fires.
    /// Returns the projectile commands its weapons produced, paired with
    /// the bot index, for the caller to dispatch (it owns the projectile
    /// system) and report back through [`Bot::finish_projectile_command`].
    pub(crate) fn think_and_act(
        &mut self,
        level: &mut Level,
        human: HumanView,
        hitboxes: &ohl_combat::HitboxIndex,
        damage_queue: &mut Vec<QueuedDamage>,
        dt: f32,
    ) -> Vec<(usize, crate::combat::PlayerProjectileCommand)> {
        let mut commands = Vec::new();
        if self.winner.is_some() {
            for bot in &mut self.bots {
                bot.idle(level, dt);
            }
            return commands;
        }
        let opponents: Vec<bot::Opponent> = self
            .living_positions(human)
            .map(|(slot, origin)| bot::Opponent {
                slot,
                entity: if slot == HUMAN_SLOT {
                    human.entity
                } else {
                    self.bots[slot - 1].entity
                },
                origin,
                eye: if slot == HUMAN_SLOT {
                    human.eye
                } else {
                    self.bots[slot - 1].eye()
                },
                firing: if slot == HUMAN_SLOT {
                    human.firing
                } else {
                    self.bots[slot - 1].firing()
                },
            })
            .collect();
        for index in 0..self.bots.len() {
            if !self.bots[index].alive() {
                self.bots[index].idle(level, dt);
                continue;
            }
            let bot = &mut self.bots[index];
            bot.think(level, &self.nav, &opponents, index + 1, dt);
            bot.move_step(level, dt);
            if bot.died_from_world() {
                // A fall or a hazard killed it mid-step: it does not get to
                // fire on its way down.
                bot.die(level);
                self.record_death(index + 1, None, None);
                continue;
            }
            if let Some(command) = bot.fire(level, hitboxes, damage_queue, dt) {
                commands.push((index, command));
            }
        }
        commands
    }

    /// The bot entities, for tests and dev tools.
    pub(crate) fn bot_entities(&self) -> Vec<Entity> {
        self.bots.iter().map(|bot| bot.entity).collect()
    }
}

/// How long a taken pickup of `kind` stays gone, or `None` for one that
/// never comes back (a dropped `weaponbox`, a charger).
fn respawn_delay(kind: ohl_combat::PickupKind) -> Option<f32> {
    use ohl_combat::PickupKind as P;
    match kind {
        P::Weapon(_) => Some(WEAPON_RESPAWN_SECONDS),
        P::Ammo(_) => Some(AMMO_RESPAWN_SECONDS),
        P::HealthKit | P::Battery | P::LongJump | P::Suit => Some(ITEM_RESPAWN_SECONDS),
        _ => None,
    }
}

/// Whether `entity` is a dead bot's corpse, which neither draws hitboxes
/// nor counts as a target. Used by the hitbox rebuild.
pub(crate) fn is_dead_bot(level: &Level, entity: Entity) -> bool {
    level
        .registry
        .world
        .get::<&BotBody>(entity)
        .is_ok_and(|body| !body.alive)
}

/// Whether `entity`'s studio model is hidden this frame: a pickup that has
/// been taken and not yet respawned.
pub(crate) fn studio_hidden(world: &ohl_game::hecs::World, entity: Entity) -> bool {
    world
        .get::<&Pickup>(entity)
        .is_ok_and(|pickup| pickup.taken)
}

/// The sequence of `anim`'s model named `name` (or, with `prefix`, the
/// first one whose name starts with it), the same name lookup
/// `crate::ai` resolves a monster's activities through.
pub(crate) fn sequence_named(
    level: &Level,
    anim: &StudioAnim,
    name: &str,
    prefix: bool,
) -> Option<usize> {
    let model = level.studio_models.get(anim.model)?;
    if let Some(index) = model.sequence_by_name(name) {
        return Some(index);
    }
    if !prefix {
        return None;
    }
    model.sequence_names.iter().position(|candidate| {
        candidate
            .get(..name.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(name))
    })
}
