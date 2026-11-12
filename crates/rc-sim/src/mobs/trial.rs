//! Singleplayer spawner blocks, ticked as block entities after the entities: monster spawner,
//! trial spawner (states inactive → waiting → active → waiting for reward → ejecting →
//! cooldown, ominous version) and vault (key, rewarded players, ejection one by one);
//! plus the ominous item spawner that the ominous trial spawner places above the fighters.

use std::sync::Arc;

use hashbrown::HashMap;
use rc_data::PropKey;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{AddEntity, Meta};

use super::mob::{Ent, f64_of};
use super::recent::vibration::Src;
use super::spawn::{Group, Reason, SpawnPaths};
use super::species::Kind;
use crate::nbtio;
use crate::player::Stack;
use crate::projectiles::{PKind, Pickup, Proj};
use crate::server::Server;

type P = [i32; 3];

const CAGE: &str = "minecraft:mob_spawner";
const TRIAL: &str = "minecraft:trial_spawner";
const VAULT: &str = "minecraft:vault";

/// Block entity identifier of a spawner block (monster spawner, trial spawner, vault).
pub(crate) fn spawner_block_id(be: Option<&Nbt>) -> Option<&str> {
    be?.get("id").and_then(Nbt::as_str).filter(|i| matches!(*i, CAGE | TRIAL | VAULT))
}

/// State of the spawner blocks outside their block entity, and ominous spawners. The spawner
/// blocks themselves tick in the common block-entity list (G3-F13).
#[derive(Debug, Clone, Default)]
pub struct SpawnerBlocks {
    /// Vaults: last denied sound (not saved).
    vault_fail: HashMap<P, i64>,
    items: Vec<OminousItem>,
}

/// Ominous item spawner (entity without physics): drops or shoots its item at the end of the delay.
#[derive(Debug, Clone)]
struct OminousItem {
    id: i32,
    uuid: u128,
    pos: [f64; 3],
    item: Stack,
    after: i64,
    age: i64,
    /// Rank in the entity walk (`walk::next_rank`); released (removed after the walk).
    rank: u64,
    removed: bool,
}

/// Loot tables ejected by a configuration that names none (weight 1 each).
const DEFAULT_LOOT_TABLES_TO_EJECT: [&str; 2] =
    ["minecraft:spawners/trial_chamber/consumables", "minecraft:spawners/trial_chamber/key"];
const DEFAULT_ITEMS_TO_DROP_WHEN_OMINOUS: &str = "minecraft:spawners/trial_chamber/items_to_drop_when_ominous";

/// Settings of a trial spawner (normal or ominous): the numbers decoded, the lists read in place
/// from the configuration compound.
#[derive(Debug, Clone)]
struct TrialSpawnSettings {
    /// Configuration compound: pack entry (shared with the pack's cache), inline compound, or
    /// empty for the defaults.
    source: Arc<Nbt>,
    spawn_range: i32,
    total_mobs: f32,
    simultaneous_mobs: f32,
    total_mobs_added_per_player: f32,
    simultaneous_mobs_added_per_player: f32,
    ticks_between_spawn: i32,
}

impl TrialSpawnSettings {
    fn read(source: Arc<Nbt>) -> TrialSpawnSettings {
        let float = |k: &str, default: f32| nbt_number(source.get(k)).map_or(default, |x| x as f32);
        let int = |k: &str, default: i32| nbt_number(source.get(k)).map_or(default, |x| x as i32);
        TrialSpawnSettings {
            spawn_range: int("spawn_range", 4),
            total_mobs: float("total_mobs", 6.0),
            simultaneous_mobs: float("simultaneous_mobs", 2.0),
            total_mobs_added_per_player: float("total_mobs_added_per_player", 2.0),
            simultaneous_mobs_added_per_player: float("simultaneous_mobs_added_per_player", 1.0),
            ticks_between_spawn: int("ticks_between_spawn", 40),
            source,
        }
    }

    /// `[{data, weight}]`.
    fn spawn_potentials(&self) -> &[Nbt] {
        nbtio::list(self.source.get("spawn_potentials"))
    }

    /// Loot tables to eject and their weights (only read when a reward is ejected).
    fn loot_tables_to_eject(&self) -> Vec<(String, i32)> {
        match self.source.get("loot_tables_to_eject") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|e| {
                    let id = e.get("data").and_then(Nbt::as_str)?.to_owned();
                    Some((id, nbt_number(e.get("weight")).map_or(1, |w| w as i32)))
                })
                .collect(),
            _ => DEFAULT_LOOT_TABLES_TO_EJECT.iter().map(|t| ((*t).to_owned(), 1)).collect(),
        }
    }

    fn items_to_drop_when_ominous(&self) -> &str {
        self.source
            .get("items_to_drop_when_ominous")
            .and_then(Nbt::as_str)
            .unwrap_or(DEFAULT_ITEMS_TO_DROP_WHEN_OMINOUS)
    }

    fn target_total(&self, extra: i32) -> i32 {
        (self.total_mobs + self.total_mobs_added_per_player * extra as f32).floor() as i32
    }

    fn target_simultaneous(&self, extra: i32) -> i32 {
        (self.simultaneous_mobs + self.simultaneous_mobs_added_per_player * extra as f32).floor() as i32
    }
}

/// Former path (`RUSTCRAFT_OPT_TRIAL_BORROW=0`): the copies the former decode made on every tick
/// and then dropped, so that the switch off costs what the former code cost. It built the
/// defaults first (eject list and ominous loot table) and, for a configuration found, copied its
/// spawn potentials, eject list and ominous loot table; the configuration itself was borrowed.
fn replay_former_config_copies(settings: &TrialSpawnSettings, found: bool) {
    let defaults: (Vec<(String, i32)>, String) = (
        DEFAULT_LOOT_TABLES_TO_EJECT.iter().map(|t| ((*t).to_owned(), 1)).collect(),
        DEFAULT_ITEMS_TO_DROP_WHEN_OMINOUS.to_owned(),
    );
    std::hint::black_box(&defaults);
    if found {
        let copies = (
            settings.spawn_potentials().to_vec(),
            settings.loot_tables_to_eject(),
            // A missing field took the default string over, without a copy.
            settings.source.get("items_to_drop_when_ominous").and_then(Nbt::as_str).map(str::to_owned),
        );
        std::hint::black_box(&copies);
    }
}

/// Configuration compound of the defaults (no allocation per tick).
fn empty_config() -> Arc<Nbt> {
    static EMPTY: std::sync::OnceLock<Arc<Nbt>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(Nbt::Compound(Vec::new()))).clone()
}

/// Empty compound, lent where a missing field reads as `{}`.
static EMPTY_COMPOUND: Nbt = Nbt::Compound(Vec::new());

/// Spawn data of a spawner with nothing to spawn: `{entity: {}}`.
fn empty_spawn_data() -> Nbt {
    Nbt::Compound(vec![("entity".into(), Nbt::Compound(Vec::new()))])
}

/// Saved state of a trial spawner.
#[derive(Debug, Clone, Default)]
struct TrialData {
    players: Vec<u128>,
    tracked_entities: Vec<u128>,
    cooldown_ends_at: i64,
    next_mob_spawns_at: i64,
    total: i32,
    spawn_data: Option<Nbt>,
    ejecting: Option<String>,
}

/// States of a trial spawner (block property `trial_spawner_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrialState {
    Inactive,
    WaitingForPlayers,
    Active,
    WaitingForRewardEjection,
    EjectingReward,
    Cooldown,
}

impl TrialState {
    /// State of a property value (absent: inactive; the property only takes the six values).
    fn of(value: Option<&str>) -> TrialState {
        match value.unwrap_or("inactive") {
            "inactive" => TrialState::Inactive,
            "waiting_for_players" => TrialState::WaitingForPlayers,
            "active" => TrialState::Active,
            "waiting_for_reward_ejection" => TrialState::WaitingForRewardEjection,
            "ejecting_reward" => TrialState::EjectingReward,
            _ => TrialState::Cooldown,
        }
    }

    fn name(self) -> &'static str {
        match self {
            TrialState::Inactive => "inactive",
            TrialState::WaitingForPlayers => "waiting_for_players",
            TrialState::Active => "active",
            TrialState::WaitingForRewardEjection => "waiting_for_reward_ejection",
            TrialState::EjectingReward => "ejecting_reward",
            TrialState::Cooldown => "cooldown",
        }
    }
}

/// What a trial spawner tick reads and changes: saved data, ominous flag, state at the start,
/// whether the client must receive the block entity again.
struct TrialTick {
    data: TrialData,
    ominous: bool,
    state: TrialState,
    updated: bool,
}

impl TrialTick {
    /// Spawn data (after `trial_draw_next_data`).
    fn next_data(&self) -> &Nbt {
        self.data.spawn_data.as_ref().unwrap_or(&EMPTY_COMPOUND)
    }
}

/// Player seen by a spawner (host or guest).
#[derive(Debug, Clone, Copy)]
struct DetectedPlayer {
    guest: Option<usize>,
    uuid: u128,
    pos: [f64; 3],
    eye: [f64; 3],
    /// Height of the player's box in its current pose.
    height: f64,
    creative: bool,
    spectator: bool,
    alive: bool,
}

fn nbt_number(v: Option<&Nbt>) -> Option<f64> {
    v.and_then(f64_of)
}

fn uuids(v: Option<&Nbt>) -> Vec<u128> {
    let mut out: Vec<u128> = Vec::new();
    for u in nbtio::list(v).iter().filter_map(|n| nbtio::uuid(Some(n))) {
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

fn uuid_list(v: &[u128]) -> Nbt {
    Nbt::List(v.iter().map(|&u| nbtio::uuid_nbt(u)).collect())
}

/// Packing of a position into a 64-bit integer.
fn pos_long(p: P) -> i64 {
    ((i64::from(p[0]) & 0x3FF_FFFF) << 38) | ((i64::from(p[2]) & 0x3FF_FFFF) << 12) | (i64::from(p[1]) & 0xFFF)
}

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn block_of(v: [f64; 3]) -> P {
    v.map(|c| c.floor() as i32)
}

/// How far above the feet of its target an ominous item spawner aims: the height of the
/// target's box, 2 blocks and `drawn` more (0 to 3), summed in single precision like the game
/// (a box height is a float there): above a swimming player, 2.6 comes out just under 2.6.
fn ominous_item_rise(box_height: f64, drawn: i32) -> f64 {
    f64::from(box_height as f32 + 2.0 + drawn as f32)
}

/// Distance between whole cells strictly under the range.
fn closer_than(a: P, b: P, range: f64) -> bool {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]].map(f64::from);
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < range * range
}

fn same_stack(a: &Stack, b: &Stack) -> bool {
    let comps = |s: &Stack| match &s.components {
        Some(Nbt::Compound(c)) if !c.is_empty() => Some(s.components.clone()),
        _ => None,
    };
    a.id == b.id && comps(a) == comps(b)
}

impl Server {
    // ---------- Registration ----------

    /// Spawner or vault block entity added (load or placement).
    pub(crate) fn spawner_blocks_register(&mut self, p: P) {
        if spawner_block_id(self.be_get(p)).is_some() {
            self.ticker_register(p, crate::blocks::level::BeTick::Spawner);
        }
    }

    /// Spawner block gone (its ticker died): its state outside the block entity forgotten.
    pub(crate) fn spawner_block_forgotten(&mut self, p: P) {
        self.mobs.spawners.blocks.vault_fail.remove(&p);
    }

    /// Tick of a spawner block (block entity phase, after the entities: a mob born here ticks on
    /// the next tick, like in the game). `players`: living players, read once per walk.
    pub(crate) fn spawner_block_tick(&mut self, p: P, players: &[[f64; 3]]) {
        // The block entity's rank in its chunk list, found once and handed to the tick.
        let Some(rank) = self.be_index(p) else { return };
        match spawner_block_id(self.chunk_be(p, rank)) {
            Some(CAGE) => {
                if self.rule("minecraft:spawner_blocks_work", true) {
                    self.spawner_tick(p, players);
                }
            }
            Some(TRIAL) => self.trial_tick(p, rank),
            Some(_) => self.vault_tick(p, rank),
            None => {}
        }
    }

    /// Block tag test of the loot conditions: the block found by name through the data's index
    /// (the former copies walked the whole block list for each test).
    fn loot_block_in_tag(&self, tag: &str, name: &str) -> bool {
        self.data
            .block_id(name)
            .is_some_and(|id| self.block_tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
    }

    /// Block entity of rank `rank` in the list of the chunk of `p`.
    fn chunk_be(&self, p: P, rank: usize) -> Option<&Nbt> {
        self.chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?.block_entities.get(rank)
    }

    /// Like `chunk_be`, to modify it (the chunk is then marked for saving, like `be_get_mut`).
    fn chunk_be_mut(&mut self, p: P, rank: usize) -> Option<&mut Nbt> {
        let c = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))?.as_mut()?;
        c.dirty = true;
        c.block_entities.get_mut(rank)
    }

    /// Rank of the block entity at `p`: `hint` if it still holds it (a block update during the
    /// tick may have shifted the chunk's list), otherwise looked up again.
    fn be_rank(&self, p: P, hint: usize) -> Option<usize> {
        let at_p = |be: &Nbt| {
            [("x", p[0]), ("y", p[1]), ("z", p[2])].iter().all(|&(k, v)| nbtio::int(be, k) == Some(i64::from(v)))
        };
        if self.chunk_be(p, hint).is_some_and(at_p) { Some(hint) } else { self.be_index(p) }
    }

    // ---------- Players ----------

    fn seen_players(&self) -> Vec<DetectedPlayer> {
        let mut out = vec![DetectedPlayer {
            guest: None,
            uuid: self.uuid,
            pos: self.pos,
            eye: [self.pos[0], self.player_eye_y(), self.pos[2]],
            height: self.player_dims().1,
            creative: self.game_mode == 1,
            spectator: self.game_mode == 3,
            alive: !self.life.vit.dead,
        }];
        if let Some(l) = &self.lan {
            for (i, g) in l.guests.iter().enumerate() {
                if let Some(c) = g.ctx.as_ref().filter(|_| g.joined && !g.gone) {
                    // The guest's own pose (asleep, swimming or gliding, crouching), like the host's.
                    let (_, height, eye) = super::body::guest_dims(c);
                    out.push(DetectedPlayer {
                        guest: Some(i),
                        uuid: c.uuid,
                        pos: c.pos,
                        eye: [c.pos[0], c.pos[1] + eye, c.pos[2]],
                        height,
                        creative: c.game_mode == 1,
                        spectator: c.game_mode == 3,
                        alive: !c.vit.dead,
                    });
                }
            }
        }
        out
    }

    fn seen_effect(&self, who: Option<usize>, id: &str) -> Option<i32> {
        let list = match who {
            None => &self.life.fx.list,
            Some(i) => &self.lan.as_ref()?.guests.get(i)?.ctx.as_ref()?.fx.list,
        };
        list.iter().find(|e| e.id == id).map(|e| e.amp)
    }

    /// Is the segment free of visual shapes, or does it bump into the cell `target`?
    fn sight_to(&self, from: [f64; 3], to: [f64; 3], target: P) -> bool {
        match self.clip_visual_cell(from, to) {
            None => true,
            Some(c) => c == target,
        }
    }

    /// Cell hit by the segment on the visual shapes.
    fn clip_visual_cell(&self, from: [f64; 3], to: [f64; 3]) -> Option<P> {
        if from == to {
            return None;
        }
        let lerp = |k: f64, a: f64, b: f64| a + k * (b - a);
        let te: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, to[i], from[i]));
        let fe: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, from[i], to[i]));
        let mut cell = fe.map(|v| v.floor() as i32);
        if self.visual_cell_hit(cell, from, to) {
            return Some(cell);
        }
        let d: [f64; 3] = std::array::from_fn(|i| te[i] - fe[i]);
        let sign = d.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
        let frac = |v: f64| v - v.floor();
        let mut t: [f64; 3] =
            std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(fe[i]) } else { frac(fe[i]) });
        while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
            let a = if t[0] < t[1] {
                if t[0] < t[2] { 0 } else { 2 }
            } else if t[1] < t[2] {
                1
            } else {
                2
            };
            cell[a] += sign[a];
            t[a] += delta[a];
            if self.visual_cell_hit(cell, from, to) {
                return Some(cell);
            }
        }
        None
    }

    // ---------- Trial spawner ----------

    /// Configuration: id of the `trial_spawner` registry (the pack's NBT, shared) or inline
    /// compound; anything else: the defaults. `shared` false: the former path, which made on
    /// every tick the copies `replay_former_config_copies` repeats.
    fn trial_config(&self, raw: Option<&Nbt>, shared: bool) -> TrialSpawnSettings {
        let (source, found) = match raw {
            Some(Nbt::String(id)) => match self.life.pack.get_nbt("trial_spawner", id) {
                Some(entry) => (entry, true),
                None => (empty_config(), false),
            },
            Some(c @ Nbt::Compound(_)) => (Arc::new(c.clone()), true),
            _ => (empty_config(), false),
        };
        let settings = TrialSpawnSettings::read(source);
        if !shared {
            replay_former_config_copies(&settings, found);
        }
        settings
    }

    fn trial_load(be: &Nbt) -> TrialData {
        TrialData {
            players: uuids(be.get("registered_players")),
            tracked_entities: uuids(be.get("current_mobs")),
            cooldown_ends_at: nbtio::int(be, "cooldown_ends_at").unwrap_or(0),
            next_mob_spawns_at: nbtio::int(be, "next_mob_spawns_at").unwrap_or(0),
            total: nbtio::int(be, "total_mobs_spawned").unwrap_or(0).max(0) as i32,
            spawn_data: be.get("spawn_data").filter(|d| d.get("entity").is_some()).cloned(),
            ejecting: be.get("ejecting_loot_table").and_then(Nbt::as_str).map(str::to_owned),
        }
    }

    /// Data written into the block entity (found by its rank when known).
    fn trial_store(&mut self, p: P, rank: Option<usize>, t: &TrialData) {
        let be = match rank {
            Some(i) => self.chunk_be_mut(p, i),
            None => self.be_get_mut(p),
        };
        if let Some(be) = be {
            Self::trial_write(be, t);
        }
    }

    /// Data written into a trial spawner's compound, default values removed (in place: the other
    /// fields keep their order).
    fn trial_write(be: &mut Nbt, t: &TrialData) {
        let put = |be: &mut Nbt, k: &str, v: Option<Nbt>| match v {
            Some(v) => nbtio::set(be, k, v),
            None => nbtio::remove(be, k),
        };
        put(be, "registered_players", (!t.players.is_empty()).then(|| uuid_list(&t.players)));
        put(be, "current_mobs", (!t.tracked_entities.is_empty()).then(|| uuid_list(&t.tracked_entities)));
        put(be, "cooldown_ends_at", (t.cooldown_ends_at != 0).then_some(Nbt::Long(t.cooldown_ends_at)));
        put(be, "next_mob_spawns_at", (t.next_mob_spawns_at != 0).then_some(Nbt::Long(t.next_mob_spawns_at)));
        put(be, "total_mobs_spawned", (t.total != 0).then_some(Nbt::Int(t.total)));
        put(be, "spawn_data", t.spawn_data.clone());
        put(be, "ejecting_loot_table", t.ejecting.clone().map(Nbt::String));
    }

    /// Would `trial_write` leave `be` as it is? Compared field by field without building the
    /// fields: most ticks change nothing, and a write marks the chunk for saving. A field kept in
    /// another form (an `Int` time, a duplicated UUID, an empty list…) is not "as it is": the
    /// write normalizes it, like before.
    fn trial_stored_as(be: &Nbt, t: &TrialData) -> bool {
        let uuids_as = |key: &str, list: &[u128]| match be.get(key) {
            None => list.is_empty(),
            Some(Nbt::List(stored)) => {
                !list.is_empty()
                    && stored.len() == list.len()
                    && stored.iter().zip(list).all(|(n, &u)| nbtio::uuid(Some(n)) == Some(u))
            }
            Some(_) => false,
        };
        let long_as = |key: &str, v: i64| match be.get(key) {
            None => v == 0,
            Some(Nbt::Long(stored)) => v != 0 && *stored == v,
            Some(_) => false,
        };
        let total_as = match be.get("total_mobs_spawned") {
            None => t.total == 0,
            Some(Nbt::Int(stored)) => t.total != 0 && *stored == t.total,
            Some(_) => false,
        };
        let ejecting_as = match be.get("ejecting_loot_table") {
            None => t.ejecting.is_none(),
            Some(Nbt::String(stored)) => t.ejecting.as_deref() == Some(stored.as_str()),
            Some(_) => false,
        };
        uuids_as("registered_players", &t.players)
            && uuids_as("current_mobs", &t.tracked_entities)
            && long_as("cooldown_ends_at", t.cooldown_ends_at)
            && long_as("next_mob_spawns_at", t.next_mob_spawns_at)
            && total_as
            && be.get("spawn_data") == t.spawn_data.as_ref()
            && ejecting_as
    }

    /// Data sent to the client: next spawn (active state), spawn data.
    fn trial_send(&self, p: P) {
        let Some(be) = self.be_get(p) else { return };
        let mut tag = Vec::new();
        if self.prop(p, PropKey::TrialSpawnerState) == Some("active") {
            tag.push(("next_mob_spawns_at".to_owned(), Nbt::Long(nbtio::int(be, "next_mob_spawns_at").unwrap_or(0))));
        }
        if let Some(d) = be.get("spawn_data") {
            tag.push(("spawn_data".to_owned(), d.clone()));
        }
        self.send_be_tag(p, TRIAL, Nbt::Compound(tag));
    }

    fn send_be_tag(&self, p: P, ty: &str, nbt: Nbt) {
        if let Some(kind) =
            self.data.registry("minecraft:block_entity_type").and_then(|r| r.entries.iter().position(|e| e == ty))
        {
            let _ = self.send(rc_client::Event::BlockEntityData { pos: p, kind: kind as i32, nbt: Some(nbt) });
        }
    }

    /// Trial spawner tick: lost mobs forgotten, then the step of its state; the block entity is
    /// rewritten and sent back to the client when needed.
    fn trial_tick(&mut self, p: P, rank: usize) {
        let paths = SpawnPaths::current();
        let Some((mut tick, ctx)) = self.trial_tick_read(p, rank, paths.trial_borrow) else { return };
        self.trial_forget_lost_entities(&mut tick, &ctx);
        let next = match tick.state {
            TrialState::Inactive => self.trial_inactive_step(&mut tick, &ctx),
            TrialState::WaitingForPlayers => self.trial_waiting_step(&mut tick, &ctx),
            TrialState::Active => self.trial_active_step(&mut tick, &ctx),
            TrialState::WaitingForRewardEjection => self.trial_reward_wait_step(&tick, &ctx),
            TrialState::EjectingReward => self.trial_ejecting_step(&mut tick, &ctx),
            TrialState::Cooldown => self.trial_cooldown_step(&mut tick, &ctx),
        };
        self.trial_tick_write(p, rank, &tick, next, paths);
    }

    /// The tick's inputs: saved data, configurations, block state. `in_place` false: the former
    /// path, which first copied the whole block entity and read the state as a string.
    fn trial_tick_read(&self, p: P, rank: usize, in_place: bool) -> Option<(TrialTick, TrialCtx)> {
        let copy;
        let be = if in_place {
            self.chunk_be(p, rank)?
        } else {
            copy = self.be_get(p).cloned()?;
            &copy
        };
        let ctx = TrialCtx {
            normal_config: self.trial_config(be.get("normal_config"), in_place),
            ominous_config: self.trial_config(be.get("ominous_config"), in_place),
            target_cooldown: nbtio::int(be, "target_cooldown_length").map_or(36000, |v| v.max(0)),
            range: nbtio::int(be, "required_player_range").map_or(14, |v| v.clamp(1, 128) as i32),
            p,
        };
        let data = Self::trial_load(be);
        let s = self.get(p);
        let state = if in_place {
            TrialState::of(self.view().get(s, PropKey::TrialSpawnerState))
        } else {
            TrialState::of(self.prop(p, PropKey::TrialSpawnerState))
        };
        let ominous = self.view().get_bool(s, PropKey::Ominous);
        Some((TrialTick { data, ominous, state, updated: false }, ctx))
    }

    /// Entities lost (dead, removed, more than 47 blocks away): next spawn pushed back.
    fn trial_forget_lost_entities(&self, tick: &mut TrialTick, ctx: &TrialCtx) {
        let p = ctx.p;
        let before = tick.data.tracked_entities.len();
        tick.data.tracked_entities.retain(|&u| {
            self.tracked_entity_block(u).is_some_and(|b| {
                let d = [b[0] - p[0], b[1] - p[1], b[2] - p[2]].map(i64::from);
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= 47 * 47
            })
        });
        if tick.data.tracked_entities.len() != before {
            tick.data.next_mob_spawns_at = self.time + i64::from(ctx.config(tick.ominous).ticks_between_spawn);
        }
    }

    /// Inactive: waits for spawn data naming a known entity type.
    fn trial_inactive_step(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> TrialState {
        self.trial_draw_next_data(tick, ctx);
        let id = tick.next_data().get("entity").and_then(|e| e.get("id")).and_then(Nbt::as_str);
        // The registry, not `Kind`: a known type we do not spawn still wakes the spawner up.
        let known = id.is_some_and(|i| {
            self.data.registry("minecraft:entity_type").is_some_and(|r| r.entries.iter().any(|e| e == i))
        });
        if known { TrialState::WaitingForPlayers } else { TrialState::Inactive }
    }

    /// Waiting for players: detection (spawns allowed and a mob to spawn).
    fn trial_waiting_step(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> TrialState {
        if !self.trial_can_spawn() {
            Self::trial_reset_stats(&mut tick.data);
            TrialState::WaitingForPlayers
        } else if !self.trial_has_mob(tick, ctx) {
            TrialState::Inactive
        } else {
            self.trial_detect(tick, ctx);
            if tick.data.players.is_empty() { TrialState::WaitingForPlayers } else { TrialState::Active }
        }
    }

    /// Active: detection, ominous items, then a mob when the delay and the cap allow; the fight
    /// is over once the total is reached and every mob is gone.
    fn trial_active_step(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> TrialState {
        if !self.trial_can_spawn() {
            Self::trial_reset_stats(&mut tick.data);
            return TrialState::WaitingForPlayers;
        }
        if !self.trial_has_mob(tick, ctx) {
            return TrialState::Inactive;
        }
        // Configuration and player count of the start of the tick, like the game: a detection
        // that turns the spawner ominous changes them from the next tick on.
        let cfg = ctx.config(tick.ominous);
        let extra = (tick.data.players.len() as i32 - 1).max(0);
        self.trial_detect(tick, ctx);
        if tick.ominous {
            self.trial_item_spawner(&mut tick.data, ctx);
        }
        if tick.data.total >= cfg.target_total(extra) {
            if !tick.data.tracked_entities.is_empty() {
                return TrialState::Active;
            }
            let t = &mut tick.data;
            t.cooldown_ends_at = self.time + ctx.target_cooldown;
            t.total = 0;
            t.next_mob_spawns_at = 0;
            return TrialState::WaitingForRewardEjection;
        }
        if self.time >= tick.data.next_mob_spawns_at
            && (tick.data.tracked_entities.len() as i32) < cfg.target_simultaneous(extra)
            && let Some(u) = self.trial_spawn_entity(tick, ctx)
        {
            let t = &mut tick.data;
            t.tracked_entities.push(u);
            t.total += 1;
            t.next_mob_spawns_at = self.time + i64::from(cfg.ticks_between_spawn);
            if let Some(d) = super::spawn::pick_weighted(cfg.spawn_potentials(), &mut self.mobs.rng) {
                t.spawn_data = Some(d);
                tick.updated = true;
            }
        }
        TrialState::Active
    }

    /// Reward wait: the shutter opens 40 ticks after the end of the fight.
    fn trial_reward_wait_step(&mut self, tick: &TrialTick, ctx: &TrialCtx) -> TrialState {
        let started = tick.data.cooldown_ends_at - ctx.target_cooldown;
        if self.time as f32 >= started as f32 + 40.0 {
            self.block_sound(ctx.p, "minecraft:block.trial_spawner.open_shutter", 1.0, 1.0);
            TrialState::EjectingReward
        } else {
            TrialState::WaitingForRewardEjection
        }
    }

    /// Ejection: every 30 ticks, one reward for one registered player; none left: cooldown.
    fn trial_ejecting_step(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> TrialState {
        let t = &mut tick.data;
        let started = t.cooldown_ends_at - ctx.target_cooldown;
        if (self.time - started) as f32 % 30.0 != 0.0 {
            return TrialState::EjectingReward;
        }
        if t.players.is_empty() {
            self.block_sound(ctx.p, "minecraft:block.trial_spawner.close_shutter", 1.0, 1.0);
            t.ejecting = None;
            return TrialState::Cooldown;
        }
        if t.ejecting.is_none() {
            let tables: Vec<Nbt> = ctx
                .config(tick.ominous)
                .loot_tables_to_eject()
                .into_iter()
                .map(|(id, w)| Nbt::Compound(vec![("data".into(), Nbt::String(id)), ("weight".into(), Nbt::Int(w))]))
                .collect();
            t.ejecting =
                super::spawn::pick_weighted(&tables, &mut self.mobs.rng).and_then(|d| d.as_str().map(str::to_owned));
        }
        if let Some(table) = t.ejecting.clone() {
            self.trial_eject(ctx.p, &table);
        }
        t.players.remove(0);
        TrialState::EjectingReward
    }

    /// Cooldown: a player detected again restarts the fight; at its end, back to waiting (an
    /// ominous spawner becomes normal again).
    fn trial_cooldown_step(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> TrialState {
        self.trial_detect(tick, ctx);
        let t = &mut tick.data;
        if !t.players.is_empty() {
            t.total = 0;
            t.next_mob_spawns_at = 0;
            return TrialState::Active;
        }
        if self.time < t.cooldown_ends_at {
            return TrialState::Cooldown;
        }
        let s = self.get(ctx.p);
        let ns = self.view().set(s, PropKey::Ominous, "false");
        self.set_update(ctx.p, ns);
        tick.ominous = false;
        let t = &mut tick.data;
        t.tracked_entities.clear();
        t.spawn_data = None;
        Self::trial_reset_stats(t);
        TrialState::WaitingForPlayers
    }

    /// End of tick: new state set, data rewritten (with the switch, only when that changes the
    /// block entity), block entity sent back when the state or the spawn data changed.
    fn trial_tick_write(&mut self, p: P, rank: usize, tick: &TrialTick, next: TrialState, paths: SpawnPaths) {
        let changed = next != tick.state;
        if changed {
            let s = self.get(p);
            let ns = self.view().set(s, PropKey::TrialSpawnerState, next.name());
            self.set_update(p, ns);
        }
        let rank = if paths.trial_borrow { self.be_rank(p, rank) } else { None };
        let stored_as_is = paths.trial_write_on_change && {
            let be = match rank {
                Some(i) => self.chunk_be(p, i),
                None => self.be_get(p),
            };
            be.is_some_and(|be| Self::trial_stored_as(be, &tick.data))
        };
        if !stored_as_is {
            self.trial_store(p, rank, &tick.data);
        }
        if changed || tick.updated {
            self.trial_send(p);
        }
    }

    fn trial_can_spawn(&self) -> bool {
        self.rule("minecraft:spawner_blocks_work", true)
            && self.level.difficulty_id() != 0
            && self.rule("minecraft:spawn_mobs", true)
    }

    fn trial_reset_stats(t: &mut TrialData) {
        t.players.clear();
        t.total = 0;
        t.next_mob_spawns_at = 0;
        t.cooldown_ends_at = 0;
    }

    /// Spawn data drawn from the potentials when the spawner has none (`{entity: {}}` without
    /// potentials); the client then receives the block entity again.
    fn trial_draw_next_data(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) {
        if tick.data.spawn_data.is_some() {
            return;
        }
        // An empty list draws nothing (no random number either).
        let picked = super::spawn::pick_weighted(ctx.config(tick.ominous).spawn_potentials(), &mut self.mobs.rng);
        tick.data.spawn_data = Some(picked.unwrap_or_else(empty_spawn_data));
        tick.updated = true;
    }

    fn trial_has_mob(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> bool {
        self.trial_draw_next_data(tick, ctx);
        let has_id = tick.next_data().get("entity").and_then(|e| e.get("id")).and_then(Nbt::as_str).is_some();
        has_id || !ctx.config(tick.ominous).spawn_potentials().is_empty()
    }

    /// Players detected (not creative, not spectators, in range, clear sight when needed).
    fn trial_find(&self, p: P, range: i32, sight: bool) -> Vec<DetectedPlayer> {
        let c = center(p);
        self.seen_players()
            .into_iter()
            .filter(|s| !s.creative && !s.spectator && closer_than(block_of(s.pos), p, f64::from(range)))
            .filter(|s| !sight || self.sight_to(s.eye, c, p))
            .collect()
    }

    /// Player detection (every 20 ticks depending on the position): ominous omen converted,
    /// players added, next spawn at least 40 ticks later.
    fn trial_detect(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) {
        let p = ctx.p;
        if pos_long(p).wrapping_add(self.time) % 20 != 0 {
            return;
        }
        let cooldown = tick.state == TrialState::Cooldown;
        if cooldown && tick.ominous {
            return;
        }
        let in_sight = self.trial_find(p, ctx.range, true);
        let mut became = false;
        if !tick.ominous && !in_sight.is_empty() {
            let mut omen: Option<(DetectedPlayer, bool)> = None;
            for s in &in_sight {
                if self.seen_effect(s.guest, "minecraft:trial_omen").is_some() {
                    omen = Some((*s, false));
                    break;
                }
                if self.seen_effect(s.guest, "minecraft:bad_omen").is_some() {
                    omen = Some((*s, true));
                }
            }
            if let Some((s, bad)) = omen {
                if bad {
                    self.trial_omen_from_bad(s.guest);
                }
                self.level_event(3020, block_of(s.eye), 0);
                self.trial_apply_ominous(tick, ctx);
                tick.ominous = true;
                became = true;
            }
        }
        if !cooldown || became {
            let found = if tick.data.players.is_empty() { in_sight } else { self.trial_find(p, ctx.range, false) };
            let t = &mut tick.data;
            let mut added = false;
            for s in found {
                if !t.players.contains(&s.uuid) {
                    t.players.push(s.uuid);
                    added = true;
                }
            }
            if added {
                t.next_mob_spawns_at = (self.time + 40).max(t.next_mob_spawns_at);
                if !became {
                    let ev = if tick.ominous { 3019 } else { 3013 };
                    self.level_event(ev, p, t.players.len() as i32);
                }
            }
        }
    }

    /// Bad Omen → Trial Omen (18,000 ticks per level, level 0).
    fn trial_omen_from_bad(&mut self, who: Option<usize>) {
        let run = |s: &mut Server| {
            let Some(amp) = s.life.fx.list.iter().find(|e| e.id == "minecraft:bad_omen").map(|e| e.amp) else {
                return;
            };
            let level = amp + 1;
            s.remove_effect("minecraft:bad_omen");
            s.add_effect(crate::effects::Inst::new("minecraft:trial_omen", 18000 * level, 0));
        };
        match who {
            None => run(self),
            Some(i) => self.as_guest(i, run),
        }
    }

    /// Ominous block, event 3020, mobs in progress removed (with their
    /// guaranteed-drop items), counters reset, spawns and spawners pushed back.
    fn trial_apply_ominous(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) {
        let p = ctx.p;
        let s = self.get(p);
        let ns = self.view().set(s, PropKey::Ominous, "true");
        self.set_update(p, ns);
        self.level_event(3020, p, 1);
        let t = &mut tick.data;
        for u in std::mem::take(&mut t.tracked_entities) {
            let Some(i) = self.mobs.index_of_uuid(u) else {
                continue;
            };
            let Some(mut m) = self.mobs.list.take(i) else { continue };
            self.level_event(3012, m.block_pos(), 0);
            self.drop_preserved_equipment(&mut m);
            m.removed = true;
            self.mobs.list.put(i, m);
        }
        if !ctx.ominous_config.spawn_potentials().is_empty() {
            t.spawn_data = None;
        }
        t.total = 0;
        t.next_mob_spawns_at = self.time + i64::from(ctx.ominous_config.ticks_between_spawn);
        t.cooldown_ends_at = self.time + 160;
        tick.updated = true;
    }

    /// Drawn position (or `Pos`), free spot for the box of the entity type, clear sight, spawn
    /// rules (light ignored), own rules, then the entity: a mob (finalised if only its id is
    /// given, persistent, equipped) or any other entity read from its compound; flames
    /// 3011/3012, `entity_place` event. Returns the UUID the spawner tracks.
    fn trial_spawn_entity(&mut self, tick: &mut TrialTick, ctx: &TrialCtx) -> Option<u128> {
        let p = ctx.p;
        let ominous = tick.ominous;
        self.trial_draw_next_data(tick, ctx);
        // Read in place: the spawn data is only copied into the entity that loads it.
        let next = tick.next_data();
        let ent = next.get("entity").unwrap_or(&EMPTY_COMPOUND);
        let type_name = ent.get("id").and_then(Nbt::as_str)?;
        let kind = Kind::from_name(type_name);
        let [width, height] = match kind {
            Some(kind) => [kind.info().width, kind.info().height],
            None => self.type_dims(type_name).map(|d| [d[0], d[1]])?,
        };
        let pos = match nbtio::list(ent.get("Pos")) {
            [Nbt::Double(x), Nbt::Double(y), Nbt::Double(z)] => [*x, *y, *z],
            _ => {
                let range = f64::from(ctx.config(ominous).spawn_range);
                let r = &mut self.mobs.rng;
                let x = f64::from(p[0]) + (r.next_double() - r.next_double()) * range + 0.5;
                let y = f64::from(p[1] + r.next_int_bound(3) - 1);
                let z = f64::from(p[2]) + (r.next_double() - r.next_double()) * range + 0.5;
                [x, y, z]
            }
        };
        if !rc_physics::is_free(&self.world(), &super::mob::Mob::bbox_at(pos, width, height)) {
            return None;
        }
        if !self.sight_to(pos, center(p), p) {
            return None;
        }
        let bp = block_of(pos);
        // Only mobs have spawn rules; any other type passes.
        if kind.is_some_and(|kind| !self.spawn_rules_for(kind, bp, Reason::TrialSpawner)) {
            return None;
        }
        if let Some(rules) = next.get("custom_spawn_rules")
            && !self.spawn_custom_rules_ok(rules, bp)
        {
            return None;
        }
        let (uuid, source) = match kind {
            Some(kind) => self.trial_place_mob(kind, ent, next, pos)?,
            None => self.trial_place_other_entity(type_name, ent, pos)?,
        };
        let flame = i32::from(ominous);
        self.level_event(3011, p, flame);
        self.level_event(3012, bp, flame);
        self.game_event_src("minecraft:entity_place", bp.map(|v| f64::from(v) + 0.5), Some(source), None);
        Some(uuid)
    }

    /// The mob of a trial spawner's spawn data, placed at `pos` facing a random way: read from
    /// its compound (or finalised when only its id is given), a spot free of other mobs and
    /// liquids, persistent, equipped. Its UUID and the source of its placement event.
    fn trial_place_mob(&mut self, kind: Kind, ent: &Nbt, next: &Nbt, pos: [f64; 3]) -> Option<(u128, Src)> {
        let only_id = matches!(ent, Nbt::Compound(f) if f.len() == 1);
        let yaw = self.mobs.rng.next_float() * 360.0;
        let mut m = self.new_mob(kind, pos, yaw);
        if !only_id {
            let mut full = ent.clone();
            nbtio::set(&mut full, "Pos", Nbt::List(pos.iter().map(|&v| Nbt::Double(v)).collect()));
            self.load_mob(&mut m, &full);
            m.set_pos(pos);
            m.old = pos;
            m.yaw = super::mth::wrap(yaw);
            m.pitch = 0.0;
            m.sync_data();
        }
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        if !self.spawn_obstruction(&mut m) {
            return None;
        }
        if only_id {
            let mut g = Group::default();
            self.finalize_spawn(&mut m, Reason::TrialSpawner, &mut g);
        }
        m.persistent = true;
        if let Some(eq) = next.get("equipment") {
            self.equip_from_table(&mut m, eq);
        }
        let (id, uuid) = (m.id, m.uuid);
        self.add_mob(m);
        Some((uuid, Src::Living(Ent::Mob(id))))
    }

    /// An entity that is not a mob (primed TNT, armor stand, arrow…) read from a trial spawner's
    /// spawn data like a summon, facing a random way. Its UUID (0 when it has none here: primed
    /// TNT and falling blocks, then forgotten at the next tick) and its placement event source.
    fn trial_place_other_entity(&mut self, type_name: &str, ent: &Nbt, pos: [f64; 3]) -> Option<(u128, Src)> {
        let yaw = self.mobs.rng.next_float() * 360.0;
        let mut compound = ent.clone();
        nbtio::set(&mut compound, "Rotation", Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)]));
        let source = self.server_source();
        let placed = self.summon_entity(&source, type_name, pos, Some(compound)).ok()?;
        Some((self.placed_entity_uuid(placed), Src::Other))
    }

    /// UUID of an entity just placed: a projectile's own, otherwise the one of its family.
    fn placed_entity_uuid(&self, placed: crate::commands::ents::E) -> u128 {
        if let crate::commands::ents::E::Other(id) = placed
            && let Some(projectile) = self.proj.list.iter().find(|p| p.id == id)
        {
            return projectile.uuid;
        }
        self.ent_uuid(placed)
    }

    /// Block a trial spawner's tracked entity stands in, found by its UUID among the mobs, the
    /// projectiles in flight and the other entities; `None` once it is dead or gone.
    fn tracked_entity_block(&self, uuid: u128) -> Option<P> {
        if let Some(m) = self.mobs.get_uuid(uuid) {
            return (m.alive() && !m.removed).then(|| m.block_pos());
        }
        // Many kept compounds carry no UUID (read as 0): nothing to follow.
        if uuid == 0 {
            return None;
        }
        if let Some(projectile) = self.proj.list.iter().find(|p| p.uuid == uuid && !p.removed) {
            return Some(block_of(projectile.pos));
        }
        self.ent_by_uuid(uuid).map(|e| block_of(self.e_pos(e)))
    }

    /// Equipment table, slot of each item (`equippable`
    /// component, otherwise hand), drop chances (one number for all or per slot).
    pub(crate) fn equip_from_table(&mut self, m: &mut super::mob::Mob, eq: &Nbt) {
        let (table, chances) = match eq {
            Nbt::String(t) => (t.clone(), None),
            _ => match eq.get("loot_table").and_then(Nbt::as_str) {
                Some(t) => (t.to_owned(), eq.get("slot_drop_chances").cloned()),
                None => return,
            },
        };
        let items = super::surface::loot(self, &table, &crate::loot::Ctx::default());
        let mut used: Vec<usize> = Vec::new();
        for it in items {
            let slot = match self.equippable(&it) {
                Some(e) => (!used.contains(&e.slot)).then_some(e.slot),
                None => (!used.contains(&0)).then_some(0),
            };
            let Some(slot) = slot else { continue };
            let mut it = it;
            if slot >= 2 {
                it.count = it.count.min(1);
            }
            m.equipment[slot] = Some(it);
            let chance = match &chances {
                Some(Nbt::Compound(map)) => {
                    map.iter().find(|(k, _)| super::mob::slot_index(k) == Some(slot)).and_then(|(_, v)| f64_of(v))
                }
                Some(v) => f64_of(v),
                None => None,
            };
            if let Some(c) = chance {
                m.drop_chance[slot] = c as f32;
            }
            used.push(slot);
        }
        m.sync_data();
    }

    /// Loot dropped upward from 1.2 above the block, event 3014.
    fn trial_eject(&mut self, p: P, table: &str) {
        let items = super::surface::loot(self, table, &crate::loot::Ctx::default());
        if items.is_empty() {
            return;
        }
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 1.2, f64::from(p[2]) + 0.5];
        for it in items {
            self.spawn_item_upward(it, 2, at);
        }
        self.level_event(3014, p, 0);
    }

    /// The output of an item in front of the dispenser upward.
    fn spawn_item_upward(&mut self, it: Stack, accuracy: i32, at: [f64; 3]) {
        let r = &mut self.mobs.rng;
        // Initial movement of the item (drawn then replaced, like the game).
        let _ = (r.next_double(), r.next_double());
        let pow = r.next_double() * 0.1 + 0.2;
        let spread = 0.017_227_5 * f64::from(accuracy);
        let v = [r.triangle(0.0, spread), r.triangle(0.2, spread), r.triangle(0.0, spread)];
        let _ = pow;
        self.spawn_item([at[0], at[1] - 0.125, at[2]], v, it, 0);
    }

    /// Ominous spawner above a fighter or a spawner mob (every
    /// 160 ticks), item drawn from the ominous table (fixed seed per 30 × 20 × 30 zone).
    fn trial_item_spawner(&mut self, t: &mut TrialData, ctx: &TrialCtx) {
        let p = ctx.p;
        let low = [
            (p[0] as f32 / 30.0).floor() as i32,
            (p[1] as f32 / 20.0).floor() as i32,
            (p[2] as f32 / 30.0).floor() as i32,
        ];
        let seed = self.seed().wrapping_add(pos_long(low));
        let drops = {
            let tag_fn = |tag: &str, n: &str| self.loot_block_in_tag(tag, n);
            let l = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let mut rng = super::jrand::JRand::new(seed);
            l.drops(ctx.ominous_config.items_to_drop_when_ominous(), &crate::loot::Ctx::default(), &mut rng)
        };
        let total: i32 = drops.iter().map(|d| d.count).sum();
        if total <= 0 {
            return;
        }
        let mut r = self.mobs.rng.next_int_bound(total);
        let Some(item) = drops.iter().find(|d| {
            r -= d.count;
            r < 0
        }) else {
            return;
        };
        let item = Stack { count: 1, ..item.clone() };
        if self.time < t.cooldown_ends_at {
            return;
        }
        let c = center(p);
        let r2 = f64::from(ctx.range) * f64::from(ctx.range);
        let near: Vec<DetectedPlayer> = self
            .seen_players()
            .into_iter()
            .filter(|s| t.players.contains(&s.uuid) && !s.creative && !s.spectator && s.alive && dist2(s.pos, c) <= r2)
            .collect();
        if near.is_empty() {
            return;
        }
        let mobs: Vec<([f64; 3], f64)> = t
            .tracked_entities
            .iter()
            .filter_map(|u| self.mobs.get_uuid(*u))
            .filter(|m| m.alive() && dist2(m.pos(), c) <= r2)
            .map(|m| (m.pos(), f64::from(m.kind.info().height)))
            .collect();
        let use_mobs = self.mobs.rng.next_bool();
        let pool: Vec<([f64; 3], f64)> = if use_mobs { mobs } else { near.iter().map(|s| (s.pos, s.height)).collect() };
        if pool.is_empty() {
            return;
        }
        let (at, h) =
            if pool.len() == 1 { pool[0] } else { pool[self.mobs.rng.next_int_bound(pool.len() as i32) as usize] };
        let up = ominous_item_rise(h, self.mobs.rng.next_int_bound(4));
        let to = [at[0], at[1] + up, at[2]];
        let hit = self.clip_visual_cell(at, to).unwrap_or_else(|| block_of(to));
        let down = [f64::from(hit[0]) + 0.5, f64::from(hit[1]) + 0.5 - 1.0, f64::from(hit[2]) + 0.5];
        if self.has_collision(block_of(down)) {
            return;
        }
        self.ominous_item_add(down, item);
        let r = &mut self.mobs.rng;
        let pitch = (r.next_float() - r.next_float()) * 0.2 + 1.0;
        self.block_sound(block_of(down), "minecraft:block.trial_spawner.spawn_item_begin", 1.0, pitch);
        t.cooldown_ends_at = self.time + 160;
    }

    // ---------- Ominous item spawner ----------

    fn ominous_item_add(&mut self, pos: [f64; 3], item: Stack) {
        let after = i64::from(self.mobs.rng.next_int_bound(61) + 60);
        self.next_entity += 1;
        let id = self.next_entity;
        let mut rng = super::jrand::JRand::from_time(i64::from(id));
        let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
        let add = AddEntity {
            id,
            uuid,
            kind: self.entity_kind_id("minecraft:ominous_item_spawner"),
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Add(Box::new(add))));
        let net = self.net(Some(&item));
        let _ = self
            .send(rc_client::Event::Entity(rc_client::EntityEvent::Data { id, values: vec![(8, Meta::Item(net))] }));
        let rank = crate::walk::next_rank();
        self.mobs.spawners.blocks.items.push(OminousItem { id, uuid, pos, item, after, age: 0, rank, removed: false });
    }

    /// The ominous item spawners as their own walk (tests).
    #[cfg(test)]
    pub(crate) fn ominous_items_tick(&mut self) {
        self.walk_alone(crate::walk::bit(crate::walk::Fam::Ominous), |_| {}, Self::ominous_items_post);
    }

    pub(crate) fn ominous_len(&self) -> usize {
        self.mobs.spawners.blocks.items.len()
    }

    pub(crate) fn ominous_rank(&self, i: usize) -> Option<u64> {
        self.mobs.spawners.blocks.items.get(i).map(|o| o.rank)
    }

    pub(crate) fn ominous_pos(&self, i: usize) -> Option<[f64; 3]> {
        self.mobs.spawners.blocks.items.get(i).map(|o| o.pos)
    }

    pub(crate) fn ominous_rank_mut(&mut self, i: usize) -> Option<&mut u64> {
        self.mobs.spawners.blocks.items.get_mut(i).map(|o| &mut o.rank)
    }

    pub(crate) fn ominous_resort(&mut self) {
        crate::walk::sort_by_rank(&mut self.mobs.spawners.blocks.items, |o| o.rank);
    }

    /// Ominous item spawner of slot `i`, at its rank in the entity walk. The list is taken out
    /// during its tick (newcomers appended after).
    pub(crate) fn ominous_visit(&mut self, i: usize) {
        let mut list = std::mem::take(&mut self.mobs.spawners.blocks.items);
        if let Some(o) = list.get_mut(i).filter(|o| !o.removed) {
            // Outside a "ticking entities" chunk, it waits (G3-F13).
            let ticking = self.entity_ticks_at(o.pos);
            if self.meet(&mut o.rank, ticking, crate::walk::Fam::Ominous) == crate::walk::Meet::Tick {
                o.age += 1;
                if o.age == o.after - 36 {
                    let at = center(block_of(o.pos));
                    self.world_sound("minecraft:block.trial_spawner.about_to_spawn_item", 6, at, 1.0, 1.0);
                }
                if o.age >= o.after {
                    self.ominous_item_release(o);
                    o.removed = true;
                }
            }
        }
        list.append(&mut self.mobs.spawners.blocks.items);
        self.mobs.spawners.blocks.items = list;
    }

    /// Released ominous item spawners, after the entity walk.
    pub(crate) fn ominous_items_post(&mut self) {
        if !self.mobs.spawners.blocks.items.iter().any(|o| o.removed) {
            return;
        }
        let gone: Vec<i32> = self.mobs.spawners.blocks.items.iter().filter(|o| o.removed).map(|o| o.id).collect();
        self.mobs.spawners.blocks.items.retain(|o| !o.removed);
        {
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Remove(gone)));
        }
    }

    /// Item dropped (item entity) or shot downward (arrows, lingering potions, charges).
    fn ominous_item_release(&mut self, o: &OminousItem) {
        let short = o.item.id.trim_start_matches("minecraft:").to_owned();
        let one = o.item.clone();
        let bp = block_of(o.pos);
        let projectile = matches!(
            short.as_str(),
            "arrow"
                | "tipped_arrow"
                | "spectral_arrow"
                | "snowball"
                | "egg"
                | "splash_potion"
                | "lingering_potion"
                | "experience_bottle"
                | "trident"
                | "fire_charge"
                | "wind_charge"
        );
        if !projectile {
            self.spawn_item(o.pos, [0.0; 3], one, 0);
        } else {
            let (kind, pow, unc, ev) = match short.as_str() {
                "arrow" | "tipped_arrow" => (PKind::Arrow, 1.1f32, 6.0f32, None),
                "spectral_arrow" => (PKind::SpectralArrow, 1.1, 6.0, None),
                "snowball" => (PKind::Snowball, 1.1, 6.0, None),
                "egg" => (PKind::Egg, 1.1, 6.0, None),
                "splash_potion" => (PKind::SplashPotion, 1.1 * 1.25, 6.0 * 0.5, None),
                "lingering_potion" => (PKind::LingeringPotion, 1.1 * 1.25, 6.0 * 0.5, None),
                "experience_bottle" => (PKind::XpBottle, 1.1 * 1.25, 6.0 * 0.5, None),
                "trident" => (PKind::Trident, 1.1, 6.0, None),
                "fire_charge" => (PKind::SmallFireball, 1.0, 6.666_666_5, Some(1018)),
                _ => (PKind::WindCharge, 1.0, 6.666_666_5, Some(1051)),
            };
            if let Some(e) = ev {
                self.level_event(e, bp, 0);
            }
            let id = self.proj_new_id();
            let mut pr = Proj::new(kind, id, o.pos, one.clone());
            pr.owner = Some(o.uuid);
            if matches!(kind, PKind::SmallFireball | PKind::WindCharge) {
                // Noisy direction (triangular 0.11485) before the shot.
                let r = &mut self.mobs.rng;
                let _ = (r.triangle(0.0, 0.114_85), r.triangle(-1.0, 0.114_85), r.triangle(0.0, 0.114_85));
                if kind == PKind::WindCharge {
                    pr.accel = 0.0;
                }
            }
            if kind.arrow_like() {
                pr.pickup = Pickup::Allowed;
                if kind == PKind::Arrow {
                    let color = if crate::items::component(&one, "minecraft:potion_contents").is_some() {
                        crate::projectiles::hits::potion_color(&one)
                    } else {
                        -1
                    };
                    pr.set_meta(11, Meta::Int(color));
                }
            } else {
                let net = self.net(Some(&one));
                pr.set_meta(8, Meta::Item(net));
            }
            pr.shoot([0.0, -1.0, 0.0], pow, unc);
            self.proj_add(pr);
        }
        self.level_event(3021, bp, 1);
        self.game_event_src("minecraft:entity_place", o.pos, None, None);
    }

    // ---------- Vault ----------

    fn vault_tick(&mut self, p: P, rank: usize) {
        let paths = SpawnPaths::current();
        if paths.vault_early_exit && self.vault_waits(p, rank) {
            return;
        }
        let Some(mut v) = self.vault_load(p, Some(rank), paths.vault_borrow) else { return };
        let state = self.prop(p, PropKey::VaultState).unwrap_or("inactive").to_owned();
        let s = self.get(p);
        let ominous = self.view().get_bool(s, PropKey::Ominous);
        if self.time % 20 == 0 && state == "active" {
            self.vault_cycle_display(&mut v, &state, p);
        }
        if self.time >= v.resumes_at {
            let next = self.vault_next(&mut v, &state, p);
            if next != state {
                self.vault_set_state(&mut v, p, &state, next, ominous);
            }
        }
        v.finish(self, p);
    }

    /// Paused vault (before `state_updating_resumes_at`) that does not cycle its displayed item
    /// this tick (every 20 ticks when active): its tick would change nothing, so nothing is
    /// decoded. Same readers as `VaultData::load` (any integer width, default 0 and inactive).
    fn vault_waits(&self, p: P, rank: usize) -> bool {
        let Some(be) = self.chunk_be(p, rank) else { return true };
        let resumes_at = be.get("server_data").and_then(|s| nbtio::int(s, "state_updating_resumes_at")).unwrap_or(0);
        let active = self.view().get(self.get(p), PropKey::VaultState) == Some("active");
        self.time < resumes_at && !(self.time % 20 == 0 && active)
    }

    /// Vault data of the block entity at `p` (its rank when known). `in_place` false: the former
    /// path, which decoded a copy of the whole block entity.
    fn vault_load(&self, p: P, rank: Option<usize>, in_place: bool) -> Option<VaultData> {
        if !in_place {
            return self.be_get(p).cloned().map(|be| VaultData::load(&be));
        }
        let be = match rank {
            Some(i) => self.chunk_be(p, i),
            None => self.be_get(p),
        };
        be.map(VaultData::load)
    }

    fn vault_next(&mut self, v: &mut VaultData, state: &str, p: P) -> &'static str {
        match state {
            "inactive" => self.vault_connected(v, p, v.activation),
            "active" => self.vault_connected(v, p, v.deactivation),
            "unlocking" => {
                v.pause(self.time + 20);
                "ejecting"
            }
            _ => {
                if v.to_eject.is_empty() {
                    v.total_needed = 0;
                    v.server_dirty = true;
                    self.vault_connected(v, p, v.deactivation)
                } else {
                    let progress = if v.total_needed == 1 {
                        1.0
                    } else {
                        1.0 - (v.to_eject.len() as f32 - 1.0) / (v.total_needed as f32 - 1.0)
                    };
                    let it = v.to_eject.pop();
                    v.server_dirty = true;
                    if let Some(it) = it {
                        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 1.2, f64::from(p[2]) + 0.5];
                        self.spawn_item_upward(it, 2, at);
                    }
                    self.level_event(3017, p, 0);
                    self.block_sound(p, "minecraft:block.vault.eject_item", 1.0, 0.8 + 0.4 * progress);
                    let next = v.to_eject.last().cloned();
                    v.set_display(next);
                    v.pause(self.time + 20);
                    "ejecting"
                }
            }
        }
    }

    /// Linked players (not spectators, not rewarded) in range; pause of 20 ticks.
    fn vault_connected(&mut self, v: &mut VaultData, p: P, range: f64) -> &'static str {
        self.vault_update_connected(v, p, range);
        v.pause(self.time + 20);
        if v.connected.is_empty() { "inactive" } else { "active" }
    }

    fn vault_update_connected(&self, v: &mut VaultData, p: P, range: f64) {
        let now: Vec<u128> = self
            .seen_players()
            .into_iter()
            .filter(|s| !s.spectator && closer_than(block_of(s.pos), p, range) && !v.rewarded.contains(&s.uuid))
            .map(|s| s.uuid)
            .collect();
        let same = now.len() == v.connected.len() && now.iter().all(|u| v.connected.contains(u));
        if !same {
            v.connected = now;
            v.shared_dirty = true;
        }
    }

    fn vault_set_state(&mut self, v: &mut VaultData, p: P, from: &str, to: &str, ominous: bool) {
        let s = self.get(p);
        let ns = self.view().set(s, PropKey::VaultState, to);
        self.set_update(p, ns);
        if from == "ejecting" {
            self.block_sound(p, "minecraft:block.vault.close_shutter", 1.0, 1.0);
        }
        match to {
            "inactive" => {
                v.set_display(None);
                self.level_event(3016, p, i32::from(ominous));
            }
            "active" => {
                if v.display.is_none() {
                    self.vault_cycle_display(v, to, p);
                }
                self.level_event(3015, p, i32::from(ominous));
            }
            "unlocking" => self.block_sound(p, "minecraft:block.vault.insert_item", 1.0, 1.0),
            "ejecting" => self.block_sound(p, "minecraft:block.vault.open_shutter", 1.0, 1.0),
            _ => {}
        }
        // State change: the block entity goes back to the client with the block.
        v.state_changed = true;
    }

    fn vault_cycle_display(&mut self, v: &mut VaultData, state: &str, p: P) {
        if v.key.is_none() || state == "inactive" {
            v.set_display(None);
            return;
        }
        let table = v.display_table.clone().unwrap_or_else(|| v.loot_table.clone());
        let ctx = crate::loot::Ctx::default();
        let items = {
            let tag_fn = |tag: &str, n: &str| self.loot_block_in_tag(tag, n);
            let l = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let mut rng = self.mobs.rng;
            let out = l.drops(&table, &ctx, &mut rng);
            self.mobs.rng = rng;
            out
        };
        let _ = p;
        let it = if items.is_empty() {
            None
        } else {
            let i = self.mobs.rng.next_int_bound(items.len() as i32) as usize;
            Some(items[i].clone())
        };
        v.set_display(it);
    }

    fn vault_send(&self, p: P) {
        let Some(be) = self.be_get(p) else { return };
        let shared = be.get("shared_data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        self.send_be_tag(p, VAULT, Nbt::Compound(vec![("shared_data".into(), shared)]));
    }

    /// Item held on an active vault; `None`: the empty hand tries.
    pub(crate) fn vault_use_item(&mut self, p: P, slot: usize) -> Option<()> {
        let held = self.slots[slot].clone()?;
        if self.prop(p, PropKey::VaultState) != Some("active") {
            return None;
        }
        let mut v = self.vault_load(p, None, SpawnPaths::current().vault_borrow)?;
        let key = v.key.clone()?;
        let now = self.time;
        let fail = |s: &mut Server, sound: &str| {
            let last = s.mobs.spawners.blocks.vault_fail.get(&p).copied().unwrap_or(i64::MIN / 2);
            if now >= last + 15 {
                s.block_sound(p, sound, 1.0, 1.0);
                s.mobs.spawners.blocks.vault_fail.insert(p, now);
            }
        };
        if !same_stack(&held, &key) || held.count < key.count {
            fail(self, "minecraft:block.vault.insert_item_fail");
            return Some(());
        }
        if v.rewarded.contains(&self.uuid) {
            fail(self, "minecraft:block.vault.reject_rewarded_player");
            return Some(());
        }
        let ctx = crate::loot::Ctx {
            tool: Some(Some(&held)),
            luck: self.player_attribute("minecraft:luck") as f32,
            ..Default::default()
        };
        let table = v.loot_table.clone();
        let items = super::surface::loot(self, &table, &ctx);
        if items.is_empty() {
            return Some(());
        }
        self.award_stat("minecraft:used", &held.id, 1);
        if !self.infinite_materials()
            && let Some(st) = &mut self.slots[slot]
        {
            st.count -= key.count;
            if st.count <= 0 {
                self.slots[slot] = None;
            }
        }
        self.broadcast_menus();
        // Unlocking: items to eject (the last first), display, pause of 14 ticks.
        v.total_needed = items.len() as i32;
        v.to_eject = items;
        let next = v.to_eject.last().cloned();
        v.set_display(next);
        v.pause(now + 14);
        let ominous = self.view().get_bool(self.get(p), PropKey::Ominous);
        self.vault_set_state(&mut v, p, "active", "unlocking", ominous);
        v.rewarded.push(self.uuid);
        if v.rewarded.len() > 128 {
            v.rewarded.remove(0);
        }
        v.server_dirty = true;
        let range = v.deactivation;
        self.vault_update_connected(&mut v, p, range);
        v.finish(self, p);
        Some(())
    }

    // ---------- Egg on a spawner ----------

    /// Spawn egg on a monster spawner or a trial spawner: the mob to spawn changes; true if the
    /// egg was used that way. `entity`: the egg's species (`id`), with the rest of its entity
    /// data when an operator uses it.
    pub(crate) fn spawn_egg_on_spawner(&mut self, p: P, entity: &Nbt) -> Option<bool> {
        let id = self.be_get(p).and_then(|b| b.get("id")).and_then(Nbt::as_str).map(str::to_owned)?;
        if id != CAGE && id != TRIAL {
            return None;
        }
        if !self.rule("minecraft:spawner_blocks_work", true) {
            let msg = rc_text::Component::translate("advMode.notEnabled.spawner", Vec::new());
            self.chat_out(rc_client::Event::Chat { component: Box::new(msg), tag: rc_client::ChatTag::System });
            return Some(false);
        }
        let only = |e: &Nbt| Nbt::Compound(vec![("entity".into(), e.clone())]);
        if id == CAGE {
            let be = self.be_get(p).cloned()?;
            let mut data = match be.get("SpawnData") {
                Some(d) => d.clone(),
                None => {
                    let pots = nbtio::list(be.get("SpawnPotentials")).to_vec();
                    super::spawn::pick_weighted(&pots, &mut self.mobs.rng)
                        .unwrap_or_else(|| Nbt::Compound(vec![("entity".into(), Nbt::Compound(Vec::new()))]))
                }
            };
            // The spawner's own entity keeps its other fields: the egg's are merged over them.
            let mut ent = data.get("entity").cloned().unwrap_or(Nbt::Compound(Vec::new()));
            crate::commands::cmds_data::merge(&mut ent, entity);
            nbtio::set(&mut data, "entity", ent);
            if let Some(b) = self.be_get_mut(p) {
                nbtio::set(b, "SpawnData", data);
            }
        } else {
            // Trial spawner: settings replaced by the single mob, state reset.
            let be = self.be_get(p).cloned()?;
            let put = |key: &str, raw: Option<&Nbt>, s: &mut Server| {
                let base = s.trial_config(raw, true);
                let mut c = vec![
                    ("spawn_range".into(), Nbt::Int(base.spawn_range)),
                    ("total_mobs".into(), Nbt::Float(base.total_mobs)),
                    ("simultaneous_mobs".into(), Nbt::Float(base.simultaneous_mobs)),
                    ("total_mobs_added_per_player".into(), Nbt::Float(base.total_mobs_added_per_player)),
                    ("simultaneous_mobs_added_per_player".into(), Nbt::Float(base.simultaneous_mobs_added_per_player)),
                    ("ticks_between_spawn".into(), Nbt::Int(base.ticks_between_spawn)),
                    (
                        "spawn_potentials".into(),
                        Nbt::List(vec![Nbt::Compound(vec![
                            ("data".into(), only(entity)),
                            ("weight".into(), Nbt::Int(1)),
                        ])]),
                    ),
                ];
                c.push((
                    "loot_tables_to_eject".into(),
                    Nbt::List(
                        base.loot_tables_to_eject()
                            .into_iter()
                            .map(|(t, w)| {
                                Nbt::Compound(vec![("data".into(), Nbt::String(t)), ("weight".into(), Nbt::Int(w))])
                            })
                            .collect(),
                    ),
                ));
                c.push((
                    "items_to_drop_when_ominous".into(),
                    Nbt::String(base.items_to_drop_when_ominous().to_owned()),
                ));
                if let Some(b) = s.be_get_mut(p) {
                    nbtio::set(b, key, Nbt::Compound(c));
                }
            };
            put("normal_config", be.get("normal_config"), self);
            put("ominous_config", be.get("ominous_config"), self);
            let t = TrialData::default();
            self.trial_store(p, None, &t);
            let s = self.get(p);
            let ns = self.view().set(s, PropKey::TrialSpawnerState, "inactive");
            self.set_update(p, ns);
        }
        if id == TRIAL {
            self.trial_send(p);
        } else if let Some(be) = self.be_get(p) {
            let mut nbt = be.clone();
            for k in ["id", "x", "y", "z", "keepPacked", "SpawnPotentials"] {
                nbtio::remove(&mut nbt, k);
            }
            self.send_be_tag(p, CAGE, nbt);
        }
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
        Some(true)
    }
}

/// Settings read once per trial spawner tick.
struct TrialCtx {
    normal_config: TrialSpawnSettings,
    ominous_config: TrialSpawnSettings,
    target_cooldown: i64,
    range: i32,
    p: P,
}

impl TrialCtx {
    fn config(&self, ominous: bool) -> &TrialSpawnSettings {
        if ominous { &self.ominous_config } else { &self.normal_config }
    }
}

/// Vault: setting, server and shared data (read from the block entity, rewritten).
#[derive(Debug, Clone)]
struct VaultData {
    loot_table: String,
    activation: f64,
    deactivation: f64,
    key: Option<Stack>,
    display_table: Option<String>,
    rewarded: Vec<u128>,
    resumes_at: i64,
    to_eject: Vec<Stack>,
    total_needed: i32,
    display: Option<Stack>,
    connected: Vec<u128>,
    particles_range: f64,
    server_dirty: bool,
    shared_dirty: bool,
    state_changed: bool,
}

impl VaultData {
    fn load(be: &Nbt) -> VaultData {
        let cfg = be.get("config");
        let g = |k: &str| cfg.and_then(|c| c.get(k));
        let sd = be.get("server_data");
        let sh = be.get("shared_data");
        let key = match g("key_item") {
            Some(k) => Stack::from_nbt(k),
            None => Some(Stack { id: "minecraft:trial_key".into(), count: 1, components: None }),
        };
        VaultData {
            loot_table: g("loot_table")
                .and_then(Nbt::as_str)
                .unwrap_or("minecraft:chests/trial_chambers/reward")
                .to_owned(),
            activation: nbt_number(g("activation_range")).unwrap_or(4.0),
            deactivation: nbt_number(g("deactivation_range")).unwrap_or(4.5),
            key,
            display_table: g("override_loot_table_to_display").and_then(Nbt::as_str).map(str::to_owned),
            rewarded: uuids(sd.and_then(|s| s.get("rewarded_players"))),
            resumes_at: sd.and_then(|s| nbtio::int(s, "state_updating_resumes_at")).unwrap_or(0),
            to_eject: nbtio::list(sd.and_then(|s| s.get("items_to_eject")))
                .iter()
                .filter_map(Stack::from_nbt)
                .collect(),
            total_needed: sd.and_then(|s| nbtio::int(s, "total_ejections_needed")).unwrap_or(0) as i32,
            display: sh.and_then(|s| s.get("display_item")).and_then(Stack::from_nbt),
            connected: uuids(sh.and_then(|s| s.get("connected_players"))),
            particles_range: nbt_number(sh.and_then(|s| s.get("connected_particles_range"))).unwrap_or(4.5),
            server_dirty: false,
            shared_dirty: false,
            state_changed: false,
        }
    }

    fn pause(&mut self, until: i64) {
        self.resumes_at = until;
        self.server_dirty = true;
    }

    fn set_display(&mut self, it: Option<Stack>) {
        let same = match (&self.display, &it) {
            (None, None) => true,
            (Some(a), Some(b)) => a.count == b.count && same_stack(a, b),
            _ => false,
        };
        if !same {
            self.display = it;
            self.shared_dirty = true;
        }
    }

    /// End of tick: data rewritten if it changed; shared data (or changed block)
    /// sent back to the client.
    fn finish(&mut self, s: &mut Server, p: P) {
        if self.server_dirty || self.shared_dirty || self.state_changed {
            self.store(s, p);
        }
        if self.shared_dirty || self.state_changed {
            s.vault_send(p);
        }
        self.server_dirty = false;
        self.shared_dirty = false;
        self.state_changed = false;
    }

    /// Writing (default values omitted, like the game's codecs).
    fn store(&self, s: &mut Server, p: P) {
        let mut server = Vec::new();
        if !self.rewarded.is_empty() {
            server.push(("rewarded_players".to_owned(), uuid_list(&self.rewarded)));
        }
        if self.resumes_at != 0 {
            server.push(("state_updating_resumes_at".to_owned(), Nbt::Long(self.resumes_at)));
        }
        if !self.to_eject.is_empty() {
            server
                .push(("items_to_eject".to_owned(), Nbt::List(self.to_eject.iter().map(|i| i.to_nbt(None)).collect())));
        }
        if self.total_needed != 0 {
            server.push(("total_ejections_needed".to_owned(), Nbt::Int(self.total_needed)));
        }
        let mut shared = Vec::new();
        if let Some(d) = &self.display {
            shared.push(("display_item".to_owned(), d.to_nbt(None)));
        }
        if !self.connected.is_empty() {
            shared.push(("connected_players".to_owned(), uuid_list(&self.connected)));
        }
        if self.particles_range != 4.5 {
            shared.push(("connected_particles_range".to_owned(), Nbt::Double(self.particles_range)));
        }
        if let Some(be) = s.be_get_mut(p) {
            nbtio::set(be, "server_data", Nbt::Compound(server));
            nbtio::set(be, "shared_data", Nbt::Compound(shared));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::probe::{Probe, state_string};

    const AT: P = [216, -59, 212];

    fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
        let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
        let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
        let mut p = Probe::open(&dir, std::sync::Arc::new(data));
        p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 206.5 0 0\n");
        if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
            eprintln!("area 200..231 missing from the copy");
            return None;
        }
        p.s.game_mode = 0;
        p.s.loaded = true;
        p.s.view = 8;
        Some((p, dir))
    }

    fn finish(p: Probe, dir: std::path::PathBuf) {
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn run(p: &mut Probe, n: i32) {
        for _ in 0..n {
            p.tick();
            p.s.ominous_items_tick(); // the spawner blocks tick in `p.tick()` (block entities)
        }
    }

    fn state(p: &Probe, key: &str) -> String {
        p.s.state_at(AT).and_then(|s| p.s.view().get_named(s, key)).unwrap_or_default().to_owned()
    }

    /// Arrow released by an ominous item spawner (an entity, ticked during the entity walk):
    /// first tick on the next server tick, not on the one it was born in. Complete server ticks.
    #[test]
    fn ominous_arrow_first_tick_on_next_server_tick() {
        let Some((mut p, dir)) = probe("g4f07b-ominous") else { return };
        let item = Stack { id: "minecraft:arrow".into(), count: 1, components: None };
        let o = OminousItem {
            id: 9_000_100,
            uuid: 9_000_100,
            pos: [216.5, -55.0, 214.5],
            item,
            after: 1,
            age: 0,
            rank: crate::walk::next_rank(),
            removed: false,
        };
        p.s.mobs.spawners.blocks.items.push(o);
        p.s.game_tick();
        p.s.tick += 1;
        let a = p.s.proj.list.iter().find(|q| q.owner == Some(9_000_100)).expect("arrow released");
        assert_eq!(a.tick_count, 0, "spawner's arrow advanced on the tick of its birth");
        let aid = a.id;
        p.s.game_tick();
        assert_eq!(p.s.proj.list.iter().find(|q| q.id == aid).map(|q| q.tick_count), Some(1));
        finish(p, dir);
    }

    /// Advances to the wanted state (at most `max` ticks); returns the number of ticks.
    fn until(p: &mut Probe, key: &str, want: &str, max: i32) -> i32 {
        for i in 0..max {
            if state(p, key) == want {
                return i;
            }
            run(p, 1);
        }
        panic!("{key}: {} instead of {want} after {max} ticks", state(p, key));
    }

    fn zombies(p: &Probe) -> Vec<u128> {
        p.s.mobs.list.iter().filter(|m| m.kind == Kind::Zombie && !m.removed).map(|m| m.uuid).collect()
    }

    fn trial_with_zombies(p: &mut Probe) {
        trial_spawning(p, "minecraft:zombie");
    }

    /// A trial spawner at `AT` whose two configurations spawn `entity_type`, two in all, one at a
    /// time.
    fn trial_spawning(p: &mut Probe, entity_type: &str) {
        // The test map cuts spawns; the trial spawner requires them.
        nbtio::set(&mut p.s.rules, "minecraft:spawn_mobs", Nbt::Byte(1));
        p.run("setblock 216 -59 212 minecraft:trial_spawner\n");
        let cfg = Nbt::Compound(vec![
            (
                "spawn_potentials".into(),
                Nbt::List(vec![Nbt::Compound(vec![
                    (
                        "data".into(),
                        Nbt::Compound(vec![(
                            "entity".into(),
                            Nbt::Compound(vec![("id".into(), Nbt::String(entity_type.into()))]),
                        )]),
                    ),
                    ("weight".into(), Nbt::Int(1)),
                ])]),
            ),
            ("total_mobs".into(), Nbt::Float(2.0)),
            ("simultaneous_mobs".into(), Nbt::Float(1.0)),
            ("ticks_between_spawn".into(), Nbt::Int(20)),
        ]);
        let be = p.s.be_get_mut(AT).expect("trial spawner block entity");
        nbtio::set(be, "normal_config", cfg.clone());
        nbtio::set(be, "ominous_config", cfg);
    }

    /// Full cycle in broad daylight (light ignored): waiting, player detected (3013), two
    /// zombies one after the other (the second when the first dies), wait of 40 ticks, loot
    /// ejected (3014) for the single player, then cooldown.
    #[test]
    fn trial_spawner_full_cycle() {
        let Some((mut p, dir)) = probe("w9trial") else { return };
        trial_with_zombies(&mut p);
        run(&mut p, 1);
        assert_eq!(state(&p, "trial_spawner_state"), "waiting_for_players");
        until(&mut p, "trial_spawner_state", "active", 25);
        let be = p.s.be_get(AT).unwrap();
        assert_eq!(uuids(be.get("registered_players")), vec![p.s.uuid], "player registered");
        // First zombie at most 40 ticks after detection, persistent, flames 3011/3012.
        let mut first = None;
        for _ in 0..60 {
            run(&mut p, 1);
            if let Some(&z) = zombies(&p).first() {
                first = Some(z);
                break;
            }
        }
        let first = first.expect("first zombie (broad daylight: light ignored)");
        assert!(p.s.mobs.list.iter().any(|m| m.uuid == first && m.persistent));
        run(&mut p, 60);
        assert_eq!(zombies(&p).len(), 1, "only one at a time");
        for m in p.s.mobs.list.iter_mut() {
            if m.uuid == first {
                m.removed = true;
            }
        }
        let mut second = None;
        for _ in 0..60 {
            run(&mut p, 1);
            if let Some(&z) = zombies(&p).first() {
                second = Some(z);
                break;
            }
        }
        let second = second.expect("second zombie");
        assert_ne!(second, first);
        assert_eq!(nbtio::int(p.s.be_get(AT).unwrap(), "total_mobs_spawned"), Some(2));
        for m in p.s.mobs.list.iter_mut() {
            if m.uuid == second {
                m.removed = true;
            }
        }
        run(&mut p, 1);
        assert_eq!(state(&p, "trial_spawner_state"), "waiting_for_reward_ejection");
        let drops = p.s.life.drops.len();
        let n = until(&mut p, "trial_spawner_state", "ejecting_reward", 45);
        assert!((38..=41).contains(&n), "shutter open after {n} ticks");
        until(&mut p, "trial_spawner_state", "cooldown", 70);
        assert!(p.s.life.drops.len() > drops, "loot ejected");
        // Cooldown: the player still there does not restart the spawner.
        run(&mut p, 100);
        assert_eq!(state(&p, "trial_spawner_state"), "cooldown");
        finish(p, dir);
    }

    /// UUIDs of the armor stands in the world.
    fn armor_stands(p: &Probe) -> Vec<u128> {
        let kept = p.s.entities.values().flatten().map(|(_, entity)| entity);
        let stands = kept.filter(|entity| entity.get("id").and_then(Nbt::as_str) == Some("minecraft:armor_stand"));
        stands.filter_map(|entity| nbtio::uuid(entity.get("UUID"))).collect()
    }

    /// A trial spawner whose spawn data names an entity that is not a mob (an armor stand)
    /// spawns it like a mob, counts it, and follows it by its UUID: no second one while it stays.
    #[test]
    fn trial_spawner_spawns_and_follows_an_entity_that_is_not_a_mob() {
        let Some((mut p, dir)) = probe("trial-armor-stand") else { return };
        trial_spawning(&mut p, "minecraft:armor_stand");
        run(&mut p, 1);
        until(&mut p, "trial_spawner_state", "active", 25);
        let mut first = None;
        for _ in 0..60 {
            run(&mut p, 1);
            if let Some(&stand) = armor_stands(&p).first() {
                first = Some(stand);
                break;
            }
        }
        let first = first.expect("an armor stand spawned");
        let be = p.s.be_get(AT).expect("trial spawner");
        assert_eq!(uuids(be.get("current_mobs")), vec![first], "followed by its UUID");
        assert_eq!(nbtio::int(be, "total_mobs_spawned"), Some(1));
        run(&mut p, 60);
        assert_eq!(armor_stands(&p), vec![first], "only one at a time: still followed");
        finish(p, dir);
    }

    /// Bad Omen II: on detection, Trial Omen of 36,000 ticks, ominous spawner.
    #[test]
    fn trial_spawner_bad_omen_turns_ominous() {
        let Some((mut p, dir)) = probe("w9omen") else { return };
        trial_with_zombies(&mut p);
        p.s.add_effect(crate::effects::Inst::new("minecraft:bad_omen", 2000, 1));
        run(&mut p, 1);
        until(&mut p, "ominous", "true", 25);
        let omen = p.s.life.fx.list.iter().find(|e| e.id == "minecraft:trial_omen").cloned();
        assert_eq!(omen.map(|e| (e.dur, e.amp)), Some((36000, 0)));
        assert!(!p.s.life.fx.list.iter().any(|e| e.id == "minecraft:bad_omen"));
        finish(p, dir);
    }

    /// Vault: active at 4 blocks (item displayed), key inserted → unlocking, key
    /// consumed, loot ejected one item after another, rewarded player refused afterwards.
    #[test]
    fn vault_unlocks_once_per_player() {
        let Some((mut p, dir)) = probe("w9vault") else { return };
        p.run("setblock 216 -59 212 minecraft:vault\nplayer 216.5 -59 209.5 0 0\n");
        assert!(p.s.be_get(AT).is_some(), "vault block entity");
        until(&mut p, "vault_state", "active", 25);
        let shared = p.s.be_get(AT).unwrap().get("shared_data").cloned().unwrap();
        assert!(shared.get("display_item").is_some(), "item displayed");
        let slot = 36 + p.s.held as usize;
        p.s.slots[slot] = Some(Stack { id: "minecraft:trial_key".into(), count: 2, components: None });
        assert_eq!(p.s.vault_use_item(AT, slot), Some(()));
        assert_eq!(state(&p, "vault_state"), "unlocking");
        assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1), "key consumed");
        let to_eject =
            nbtio::list(p.s.be_get(AT).unwrap().get("server_data").and_then(|s| s.get("items_to_eject"))).len();
        assert!(to_eject > 0);
        let drops = p.s.life.drops.len();
        until(&mut p, "vault_state", "ejecting", 20);
        run(&mut p, 20 * (to_eject as i32 + 2));
        assert_eq!(p.s.life.drops.len() - drops, to_eject, "one item per ejection");
        // Rewarded: no longer linked, the vault goes out; a second key is refused.
        until(&mut p, "vault_state", "inactive", 30);
        p.s.slots[slot] = Some(Stack { id: "minecraft:trial_key".into(), count: 1, components: None });
        assert_eq!(p.s.vault_use_item(AT, slot), None, "inactive vault: the empty hand tries");
        assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1));
        finish(p, dir);
    }

    /// Spawn caps: a zombie in view + 3 chunks (entities unloaded in the game) does not
    /// count; it counts if its chunk is force-loaded (radius 2).
    #[test]
    fn far_mobs_leave_the_mob_cap() {
        let Some((mut p, dir)) = probe("w9cap") else { return };
        let monster = super::super::species::Category::slot(super::super::species::Category::Monster);
        let base = p.s.spawn_state_counts().0[monster];
        let near = p.s.new_mob(Kind::Zombie, [216.5, -59.0, 210.5], 0.0);
        p.s.add_mob(near);
        // View 8: player's chunk 13; at 11 chunks → out of view + 2.
        let far_x = f64::from((13 + 11) * 16) + 8.5;
        let far = p.s.new_mob(Kind::Zombie, [far_x, -59.0, 210.5], 0.0);
        p.s.add_mob(far);
        assert_eq!(p.s.spawn_state_counts().0[monster], base + 1, "only the nearby one counts");
        p.s.cmd.forced.insert((13 + 11 - 2, 13));
        assert_eq!(p.s.spawn_state_counts().0[monster], base + 2, "chunk force-loaded at 2");
        finish(p, dir);
    }

    /// Ticket ranges (view 8, simulation 5): mobs at min(view, simulation), random
    /// ticks at the simulation, scheduled ticks at simulation + 1; simulation 12: the view
    /// caps the mobs.
    #[test]
    fn simulation_distance_radii() {
        let Some((mut p, dir)) = probe("w9sim") else { return };
        p.s.view = 8;
        p.s.sim_dist = 5;
        let pc = p.s.chunk_pos();
        let at = |d: i32| (pc.0 + d, pc.1);
        let centers = p.s.sim_centers();
        assert!(p.s.mob_chunk_ticking(at(5)) && !p.s.mob_chunk_ticking(at(6)));
        assert!(p.s.chunk_entity_ticking(at(5), &centers) && !p.s.chunk_entity_ticking(at(6), &centers));
        assert!(p.s.chunk_simulated(at(6), &centers) && !p.s.chunk_simulated(at(7), &centers));
        p.s.sim_dist = 12;
        assert!(p.s.mob_chunk_ticking(at(8)) && !p.s.mob_chunk_ticking(at(9)));
        finish(p, dir);
    }

    /// The trial spawner's write is skipped only when it changes nothing: a field in another form
    /// (an `Int` time, a duplicated player, an empty list, a `Long` total, a zero time, a
    /// non-text table, spawn data without entity) is still normalized, like before; the
    /// normalized form is then left alone.
    #[test]
    fn trial_write_skipped_only_when_unchanged() {
        let u = nbtio::uuid_nbt(7);
        let base = Nbt::Compound(vec![
            ("id".into(), Nbt::String(TRIAL.into())),
            ("registered_players".into(), Nbt::List(vec![nbtio::uuid_nbt(3)])),
            ("next_mob_spawns_at".into(), Nbt::Long(40)),
            ("spawn_data".into(), Nbt::Compound(vec![("entity".into(), Nbt::Compound(Vec::new()))])),
        ]);
        let data = Server::trial_load(&base);
        assert!(Server::trial_stored_as(&base, &data), "canonical block entity: no write");
        for (field, value) in [
            ("next_mob_spawns_at", Nbt::Int(40)),
            ("registered_players", Nbt::List(vec![u.clone(), u])),
            ("current_mobs", Nbt::List(Vec::new())),
            ("total_mobs_spawned", Nbt::Long(1)),
            ("cooldown_ends_at", Nbt::Long(0)),
            ("ejecting_loot_table", Nbt::Int(1)),
            ("spawn_data", Nbt::Compound(Vec::new())),
        ] {
            let mut be = base.clone();
            nbtio::set(&mut be, field, value);
            let t = Server::trial_load(&be);
            let mut written = be.clone();
            Server::trial_write(&mut written, &t);
            assert_ne!(written, be, "{field}: the write normalizes");
            assert!(!Server::trial_stored_as(&be, &t), "{field}: not skipped");
            assert!(Server::trial_stored_as(&written, &t), "{field}: normalized form left alone");
        }
        // Data changed by the tick: written.
        let mut t = data.clone();
        t.total = 2;
        assert!(!Server::trial_stored_as(&base, &t));
        t = data;
        t.players.push(9);
        assert!(!Server::trial_stored_as(&base, &t));
    }

    /// Egg on a monster spawner: the mob to spawn changes, no mob is born.
    #[test]
    fn spawn_egg_sets_cage_entity() {
        let Some((mut p, dir)) = probe("w9egg") else { return };
        p.run("setblock 216 -59 212 minecraft:spawner\n");
        let before = p.s.mobs.list.len();
        let egg = Stack { id: "minecraft:skeleton_spawn_egg".into(), count: 1, components: None };
        assert!(p.s.use_spawn_egg(&egg, AT, 1));
        assert_eq!(p.s.mobs.list.len(), before, "no mob placed");
        let id =
            p.s.be_get(AT)
                .and_then(|b| b.get("SpawnData"))
                .and_then(|d| d.get("entity"))
                .and_then(|e| e.get("id"))
                .and_then(Nbt::as_str)
                .map(str::to_owned);
        assert_eq!(id.as_deref(), Some("minecraft:skeleton"));
        finish(p, dir);
    }

    /// The ominous item spawner is placed above the player's box in its current pose, not above
    /// a standing player: the height seen follows sneaking, swimming and sleeping.
    #[test]
    fn detected_player_height_follows_the_pose() {
        use super::super::PlayerPose;
        use super::super::body::PoseWish;
        let height = |asleep, swimming, shift_held, flying| {
            PlayerPose::wanted(PoseWish { asleep, swimming, shift_held, flying, ..PoseWish::default() }).dims().1
        };
        assert_eq!(height(false, false, false, false), 1.8);
        assert_eq!(height(false, false, true, false), 1.5);
        assert_eq!(height(false, false, true, true), 1.8, "shift held while flying: standing");
        assert_eq!(height(false, true, true, false), 0.6);
        assert_eq!(height(true, true, true, false), f64::from(0.2f32));
        let Some((mut p, dir)) = probe("trial-player-height") else { return };
        assert_eq!(p.s.seen_players()[0].height, 1.8, "standing");
        p.s.sneaking = true;
        assert_eq!(p.s.seen_players()[0].height, 1.5, "sneaking");
        p.s.mobs.sea.player_swimming = true;
        assert_eq!(p.s.seen_players()[0].height, 0.6, "swimming");
        // A guest is seen in its own pose too: box height and eye.
        let feet = [218.5, -59.0, 206.5];
        let _link = super::super::tests_spawn_paths::add_guest(&mut p.s, "Guest", feet);
        let guest_pose = |p: &mut Probe, sneaking: bool, swimming: bool| {
            let guest = p.s.lan.as_mut().expect("LAN open").guests[0].ctx.as_mut().expect("guest state");
            guest.sneaking = sneaking;
            guest.swimming = swimming;
            let seen = p.s.seen_players();
            (seen[1].height, seen[1].eye[1])
        };
        assert_eq!(guest_pose(&mut p, false, false), (1.8, feet[1] + 1.62), "guest standing");
        assert_eq!(guest_pose(&mut p, true, false), (1.5, feet[1] + 1.27), "guest sneaking");
        assert_eq!(guest_pose(&mut p, true, true), (0.6, feet[1] + f64::from(0.4f32)), "guest swimming");
        finish(p, dir);
    }

    /// The rise above the target is a single precision sum: exact above a standing or sneaking
    /// player, one float step under 2.6 above a swimming one, which lowers the block aimed at
    /// when the feet are at a height ending in .4.
    #[test]
    fn ominous_item_rise_is_summed_in_single_precision() {
        assert_eq!(ominous_item_rise(1.5, 3), 6.5);
        assert_eq!(ominous_item_rise(1.8, 0), f64::from(3.8f32));
        assert_eq!(ominous_item_rise(0.6, 0), f64::from(2.6f32));
        assert!(ominous_item_rise(0.6, 0) < 2.6);
        assert_eq!((-59.6f64 + 2.6).floor(), -57.0, "in double precision");
        assert_eq!((-59.6 + ominous_item_rise(0.6, 0)).floor(), -58.0, "as the game sums it");
    }
}
