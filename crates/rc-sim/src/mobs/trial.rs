//! Singleplayer spawner blocks, ticked as block entities after the entities: monster spawner,
//! trial spawner (states inactive → waiting → active → waiting for reward → ejecting →
//! cooldown, ominous version) and vault (key, rewarded players, ejection one by one);
//! plus the ominous item spawner that the ominous trial spawner places above the fighters.

use hashbrown::{HashMap, HashSet};

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{AddEntity, Meta};

use super::mob::{Ent, f64_of};
use super::spawn::{Group, Reason};
use super::species::Kind;
use crate::nbtio;
use crate::player::Stack;
use crate::projectiles::{PKind, Pickup, Proj};
use crate::server::Server;

type P = [i32; 3];

const CAGE: &str = "minecraft:mob_spawner";
const TRIAL: &str = "minecraft:trial_spawner";
const VAULT: &str = "minecraft:vault";

/// Known spawner blocks (add order = order of the game's tickers) and ominous spawners.
#[derive(Debug, Clone, Default)]
pub struct SpawnerBlocks {
    list: Vec<P>,
    set: HashSet<P>,
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
}

/// Settings of a trial spawner (normal or ominous).
#[derive(Debug, Clone)]
struct Cfg {
    range: i32,
    total: f32,
    simul: f32,
    total_add: f32,
    simul_add: f32,
    between: i32,
    /// `[{data, weight}]`.
    potentials: Vec<Nbt>,
    eject: Vec<(String, i32)>,
    ominous_items: String,
}

impl Default for Cfg {
    fn default() -> Cfg {
        Cfg {
            range: 4,
            total: 6.0,
            simul: 2.0,
            total_add: 2.0,
            simul_add: 1.0,
            between: 40,
            potentials: Vec::new(),
            eject: vec![
                ("minecraft:spawners/trial_chamber/consumables".into(), 1),
                ("minecraft:spawners/trial_chamber/key".into(), 1),
            ],
            ominous_items: "minecraft:spawners/trial_chamber/items_to_drop_when_ominous".into(),
        }
    }
}

impl Cfg {
    fn target_total(&self, extra: i32) -> i32 {
        (self.total + self.total_add * extra as f32).floor() as i32
    }

    fn target_simul(&self, extra: i32) -> i32 {
        (self.simul + self.simul_add * extra as f32).floor() as i32
    }
}

/// Saved state of a trial spawner.
#[derive(Debug, Clone, Default)]
struct TrialData {
    players: Vec<u128>,
    mobs: Vec<u128>,
    cooldown_ends_at: i64,
    next_mob_spawns_at: i64,
    total: i32,
    spawn_data: Option<Nbt>,
    ejecting: Option<String>,
}

/// Player seen by a spawner (host or guest).
#[derive(Debug, Clone, Copy)]
struct Seen {
    guest: Option<usize>,
    uuid: u128,
    pos: [f64; 3],
    eye: [f64; 3],
    creative: bool,
    spectator: bool,
    alive: bool,
}

fn num(v: Option<&Nbt>) -> Option<f64> {
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
        let id = self.be_get(p).and_then(|b| b.get("id")).and_then(Nbt::as_str);
        if !matches!(id, Some(CAGE | TRIAL | VAULT)) {
            return;
        }
        let b = &mut self.mobs.spawners.blocks;
        if b.set.insert(p) {
            b.list.push(p);
        }
    }

    pub(crate) fn spawner_blocks_register_chunk(&mut self, ch: (i32, i32)) {
        let Some(Some(c)) = self.chunks.get(&ch) else { return };
        let found: Vec<P> = c
            .block_entities
            .iter()
            .filter(|b| matches!(b.get("id").and_then(Nbt::as_str), Some(CAGE | TRIAL | VAULT)))
            .map(|b| {
                let i = |k: &str| nbtio::int(b, k).unwrap_or(0) as i32;
                [i("x"), i("y"), i("z")]
            })
            .collect();
        for p in found {
            self.spawner_blocks_register(p);
        }
    }

    /// Tick of the ominous spawners (entities) then of the spawner blocks (block entities), after
    /// the mobs: a mob born here ticks on the next tick, like in the game.
    pub(crate) fn spawner_blocks_tick(&mut self) {
        self.ominous_items_tick();
        let list = std::mem::take(&mut self.mobs.spawners.blocks.list);
        let players = self.alive_players();
        let mut keep = Vec::with_capacity(list.len());
        for p in list {
            let id = self.be_get(p).and_then(|b| b.get("id")).and_then(Nbt::as_str).map(str::to_owned);
            let Some(id) = id.filter(|i| matches!(i.as_str(), CAGE | TRIAL | VAULT)) else {
                self.mobs.spawners.blocks.set.remove(&p);
                self.mobs.spawners.blocks.vault_fail.remove(&p);
                continue;
            };
            keep.push(p);
            if !self.be_ticking(p) {
                continue;
            }
            match id.as_str() {
                CAGE => {
                    if self.rule("minecraft:spawner_blocks_work", true) {
                        self.spawner_tick(p, &players);
                    }
                }
                TRIAL => self.trial_tick(p),
                _ => self.vault_tick(p),
            }
        }
        // Registered during the traversal: after the old ones.
        let added = std::mem::take(&mut self.mobs.spawners.blocks.list);
        keep.extend(added);
        self.mobs.spawners.blocks.list = keep;
    }

    // ---------- Players ----------

    fn seen_players(&self) -> Vec<Seen> {
        let mut out = vec![Seen {
            guest: None,
            uuid: self.uuid,
            pos: self.pos,
            eye: [self.pos[0], self.player_eye_y(), self.pos[2]],
            creative: self.game_mode == 1,
            spectator: self.game_mode == 3,
            alive: !self.life.vit.dead,
        }];
        if let Some(l) = &self.lan {
            for (i, g) in l.guests.iter().enumerate() {
                if let Some(c) = g.ctx.as_ref().filter(|_| g.joined && !g.gone) {
                    let eye = c.pos[1] + if c.sneaking { 1.27 } else { 1.62 };
                    out.push(Seen {
                        guest: Some(i),
                        uuid: c.uuid,
                        pos: c.pos,
                        eye: [c.pos[0], eye, c.pos[2]],
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

    /// Setting: id of the `trial_spawner` registry or inline setting.
    fn trial_cfg(&self, raw: Option<&Nbt>) -> Cfg {
        let owned;
        let v = match raw {
            // Pack settings: converted to NBT once (reread on every spawner tick).
            Some(Nbt::String(id)) => match self.life.pack.get_nbt("trial_spawner", id) {
                Some(j) => {
                    owned = j;
                    &*owned
                }
                None => return Cfg::default(),
            },
            Some(c @ Nbt::Compound(_)) => c,
            _ => return Cfg::default(),
        };
        let d = Cfg::default();
        let f = |k: &str, def: f32| num(v.get(k)).map_or(def, |x| x as f32);
        let i = |k: &str, def: i32| num(v.get(k)).map_or(def, |x| x as i32);
        let eject = match v.get("loot_tables_to_eject") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|e| {
                    let id = e.get("data").and_then(Nbt::as_str)?.to_owned();
                    Some((id, num(e.get("weight")).map_or(1, |w| w as i32)))
                })
                .collect(),
            _ => d.eject.clone(),
        };
        Cfg {
            range: i("spawn_range", d.range),
            total: f("total_mobs", d.total),
            simul: f("simultaneous_mobs", d.simul),
            total_add: f("total_mobs_added_per_player", d.total_add),
            simul_add: f("simultaneous_mobs_added_per_player", d.simul_add),
            between: i("ticks_between_spawn", d.between),
            potentials: nbtio::list(v.get("spawn_potentials")).to_vec(),
            eject,
            ominous_items: v
                .get("items_to_drop_when_ominous")
                .and_then(Nbt::as_str)
                .map_or(d.ominous_items, str::to_owned),
        }
    }

    fn trial_load(be: &Nbt) -> TrialData {
        TrialData {
            players: uuids(be.get("registered_players")),
            mobs: uuids(be.get("current_mobs")),
            cooldown_ends_at: nbtio::int(be, "cooldown_ends_at").unwrap_or(0),
            next_mob_spawns_at: nbtio::int(be, "next_mob_spawns_at").unwrap_or(0),
            total: nbtio::int(be, "total_mobs_spawned").unwrap_or(0).max(0) as i32,
            spawn_data: be.get("spawn_data").filter(|d| d.get("entity").is_some()).cloned(),
            ejecting: be.get("ejecting_loot_table").and_then(Nbt::as_str).map(str::to_owned),
        }
    }

    fn trial_store(&mut self, p: P, t: &TrialData) {
        let Some(be) = self.be_get_mut(p) else { return };
        let put = |be: &mut Nbt, k: &str, v: Option<Nbt>| match v {
            Some(v) => nbtio::set(be, k, v),
            None => nbtio::remove(be, k),
        };
        put(be, "registered_players", (!t.players.is_empty()).then(|| uuid_list(&t.players)));
        put(be, "current_mobs", (!t.mobs.is_empty()).then(|| uuid_list(&t.mobs)));
        put(be, "cooldown_ends_at", (t.cooldown_ends_at != 0).then_some(Nbt::Long(t.cooldown_ends_at)));
        put(be, "next_mob_spawns_at", (t.next_mob_spawns_at != 0).then_some(Nbt::Long(t.next_mob_spawns_at)));
        put(be, "total_mobs_spawned", (t.total != 0).then_some(Nbt::Int(t.total)));
        put(be, "spawn_data", t.spawn_data.clone());
        put(be, "ejecting_loot_table", t.ejecting.clone().map(Nbt::String));
    }

    /// Data sent to the client: next spawn (active state), spawn data.
    fn trial_send(&self, p: P) {
        let Some(be) = self.be_get(p) else { return };
        let mut tag = Vec::new();
        if self.prop(p, "trial_spawner_state").as_deref() == Some("active") {
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

    fn trial_tick(&mut self, p: P) {
        let Some(be) = self.be_get(p).cloned() else { return };
        let mut t = Self::trial_load(&be);
        let normal = self.trial_cfg(be.get("normal_config"));
        let ominous_cfg = self.trial_cfg(be.get("ominous_config"));
        let target_cd = nbtio::int(&be, "target_cooldown_length").map_or(36000, |v| v.max(0));
        let range = nbtio::int(&be, "required_player_range").map_or(14, |v| v.clamp(1, 128) as i32);
        let s = self.get(p);
        let mut ominous = self.v().get_bool(s, "ominous");
        let state = self.prop(p, "trial_spawner_state").unwrap_or_else(|| "inactive".into());
        let mut updated = false;
        let now = self.time;
        // Mobs lost (dead, removed, more than 47 blocks away): next spawn pushed back.
        let before = t.mobs.len();
        t.mobs.retain(|&u| {
            self.mobs.get_uuid(u).is_some_and(|m| {
                let b = m.block_pos();
                let d = [b[0] - p[0], b[1] - p[1], b[2] - p[2]].map(i64::from);
                m.alive() && !m.removed && d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= 47 * 47
            })
        });
        if t.mobs.len() != before {
            let cfg = if ominous { &ominous_cfg } else { &normal };
            t.next_mob_spawns_at = now + i64::from(cfg.between);
        }
        let ctx = TrialCtx { normal, ominous_cfg, target_cd, range, p };
        let next = match state.as_str() {
            "inactive" => {
                let d = self.trial_next_data(&mut t, &ctx, ominous, &mut updated);
                let id = d.get("entity").and_then(|e| e.get("id")).and_then(Nbt::as_str);
                let known = id.is_some_and(|i| {
                    self.data.registry("minecraft:entity_type").is_some_and(|r| r.entries.iter().any(|e| e == i))
                });
                if known { "waiting_for_players" } else { "inactive" }
            }
            "waiting_for_players" => {
                if !self.trial_can_spawn() {
                    Self::trial_reset_stats(&mut t);
                    "waiting_for_players"
                } else if !self.trial_has_mob(&mut t, &ctx, ominous, &mut updated) {
                    "inactive"
                } else {
                    self.trial_detect(&mut t, &ctx, &state, &mut ominous, &mut updated);
                    if t.players.is_empty() { "waiting_for_players" } else { "active" }
                }
            }
            "active" => {
                if !self.trial_can_spawn() {
                    Self::trial_reset_stats(&mut t);
                    "waiting_for_players"
                } else if !self.trial_has_mob(&mut t, &ctx, ominous, &mut updated) {
                    "inactive"
                } else {
                    let cfg = if ominous { ctx.ominous_cfg.clone() } else { ctx.normal.clone() };
                    let extra = (t.players.len() as i32 - 1).max(0);
                    self.trial_detect(&mut t, &ctx, &state, &mut ominous, &mut updated);
                    if ominous {
                        self.trial_item_spawner(&mut t, &ctx);
                    }
                    if t.total >= cfg.target_total(extra) {
                        if t.mobs.is_empty() {
                            t.cooldown_ends_at = self.time + ctx.target_cd;
                            t.total = 0;
                            t.next_mob_spawns_at = 0;
                            "waiting_for_reward_ejection"
                        } else {
                            "active"
                        }
                    } else {
                        if self.time >= t.next_mob_spawns_at
                            && (t.mobs.len() as i32) < cfg.target_simul(extra)
                            && let Some(u) = self.trial_spawn_mob(&mut t, &ctx, ominous, &mut updated)
                        {
                            t.mobs.push(u);
                            t.total += 1;
                            t.next_mob_spawns_at = self.time + i64::from(cfg.between);
                            if let Some(d) = super::spawn::pick_weighted(&cfg.potentials, &mut self.mobs.rng) {
                                t.spawn_data = Some(d);
                                updated = true;
                            }
                        }
                        "active"
                    }
                }
            }
            "waiting_for_reward_ejection" => {
                let started = t.cooldown_ends_at - ctx.target_cd;
                if self.time as f32 >= started as f32 + 40.0 {
                    self.block_sound(p, "minecraft:block.trial_spawner.open_shutter", 1.0, 1.0);
                    "ejecting_reward"
                } else {
                    "waiting_for_reward_ejection"
                }
            }
            "ejecting_reward" => {
                let started = t.cooldown_ends_at - ctx.target_cd;
                if (self.time - started) as f32 % 30.0 != 0.0 {
                    "ejecting_reward"
                } else if t.players.is_empty() {
                    self.block_sound(p, "minecraft:block.trial_spawner.close_shutter", 1.0, 1.0);
                    t.ejecting = None;
                    "cooldown"
                } else {
                    if t.ejecting.is_none() {
                        let cfg = if ominous { &ctx.ominous_cfg } else { &ctx.normal };
                        let list: Vec<Nbt> = cfg
                            .eject
                            .iter()
                            .map(|(id, w)| {
                                Nbt::Compound(vec![
                                    ("data".into(), Nbt::String(id.clone())),
                                    ("weight".into(), Nbt::Int(*w)),
                                ])
                            })
                            .collect();
                        t.ejecting = super::spawn::pick_weighted(&list, &mut self.mobs.rng)
                            .and_then(|d| d.as_str().map(str::to_owned));
                    }
                    if let Some(table) = t.ejecting.clone() {
                        self.trial_eject(p, &table);
                    }
                    t.players.remove(0);
                    "ejecting_reward"
                }
            }
            _ => {
                // Recharge.
                self.trial_detect(&mut t, &ctx, "cooldown", &mut ominous, &mut updated);
                if !t.players.is_empty() {
                    t.total = 0;
                    t.next_mob_spawns_at = 0;
                    "active"
                } else if self.time >= t.cooldown_ends_at {
                    let s = self.get(p);
                    let ns = self.v().set(s, "ominous", "false");
                    self.set_update(p, ns);
                    ominous = false;
                    t.mobs.clear();
                    t.spawn_data = None;
                    Self::trial_reset_stats(&mut t);
                    "waiting_for_players"
                } else {
                    "cooldown"
                }
            }
        };
        let changed = next != state;
        if changed {
            let s = self.get(p);
            let ns = self.v().set(s, "trial_spawner_state", next);
            self.set_update(p, ns);
        }
        let _ = ominous;
        self.trial_store(p, &t);
        if changed || updated {
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

    /// The next spawn data.
    fn trial_next_data(&mut self, t: &mut TrialData, ctx: &TrialCtx, ominous: bool, updated: &mut bool) -> Nbt {
        if let Some(d) = &t.spawn_data {
            return d.clone();
        }
        let cfg = if ominous { &ctx.ominous_cfg } else { &ctx.normal };
        let picked = if cfg.potentials.is_empty() {
            None
        } else {
            super::spawn::pick_weighted(&cfg.potentials, &mut self.mobs.rng)
        };
        let d = picked.unwrap_or_else(|| Nbt::Compound(vec![("entity".into(), Nbt::Compound(Vec::new()))]));
        t.spawn_data = Some(d.clone());
        *updated = true;
        d
    }

    fn trial_has_mob(&mut self, t: &mut TrialData, ctx: &TrialCtx, ominous: bool, updated: &mut bool) -> bool {
        let d = self.trial_next_data(t, ctx, ominous, updated);
        let has_id = d.get("entity").and_then(|e| e.get("id")).and_then(Nbt::as_str).is_some();
        let cfg = if ominous { &ctx.ominous_cfg } else { &ctx.normal };
        has_id || !cfg.potentials.is_empty()
    }

    /// Players detected (not creative, not spectators, in range, clear sight when needed).
    fn trial_find(&self, p: P, range: i32, sight: bool) -> Vec<Seen> {
        let c = center(p);
        self.seen_players()
            .into_iter()
            .filter(|s| !s.creative && !s.spectator && closer_than(block_of(s.pos), p, f64::from(range)))
            .filter(|s| !sight || self.sight_to(s.eye, c, p))
            .collect()
    }

    /// Player detection (every 20 ticks depending on the position): ominous omen converted,
    /// players added, next spawn at least 40 ticks later.
    fn trial_detect(&mut self, t: &mut TrialData, ctx: &TrialCtx, state: &str, ominous: &mut bool, updated: &mut bool) {
        let p = ctx.p;
        if pos_long(p).wrapping_add(self.time) % 20 != 0 {
            return;
        }
        if state == "cooldown" && *ominous {
            return;
        }
        let in_sight = self.trial_find(p, ctx.range, true);
        let mut became = false;
        if !*ominous && !in_sight.is_empty() {
            let mut omen: Option<(Seen, bool)> = None;
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
                self.trial_apply_ominous(t, ctx, updated);
                *ominous = true;
                became = true;
            }
        }
        if state != "cooldown" || became {
            let found = if t.players.is_empty() { in_sight } else { self.trial_find(p, ctx.range, false) };
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
                    let ev = if *ominous { 3019 } else { 3013 };
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
    fn trial_apply_ominous(&mut self, t: &mut TrialData, ctx: &TrialCtx, updated: &mut bool) {
        let p = ctx.p;
        let s = self.get(p);
        let ns = self.v().set(s, "ominous", "true");
        self.set_update(p, ns);
        self.level_event(3020, p, 1);
        for u in std::mem::take(&mut t.mobs) {
            let Some(i) = self.mobs.index_of_uuid(u) else {
                continue;
            };
            let Some(mut m) = self.mobs.list[i].take() else { continue };
            self.level_event(3012, m.block_pos(), 0);
            self.drop_preserved_equipment(&mut m);
            m.removed = true;
            self.mobs.list[i] = Some(m);
        }
        if !ctx.ominous_cfg.potentials.is_empty() {
            t.spawn_data = None;
        }
        t.total = 0;
        t.next_mob_spawns_at = self.time + i64::from(ctx.ominous_cfg.between);
        *updated = true;
        t.cooldown_ends_at = self.time + 160;
    }

    /// Drawn position (or `Pos`), free spot, clear sight, spawn rules
    /// (light ignored), own rules, finalization if only the id is given, persistent
    /// and equipped mob; flames 3011/3012, `entity_place` event.
    fn trial_spawn_mob(
        &mut self,
        t: &mut TrialData,
        ctx: &TrialCtx,
        ominous: bool,
        updated: &mut bool,
    ) -> Option<u128> {
        let p = ctx.p;
        let next = self.trial_next_data(t, ctx, ominous, updated);
        let ent = next.get("entity").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        let kind = ent.get("id").and_then(Nbt::as_str).and_then(Kind::from_name)?;
        let pos = match nbtio::list(ent.get("Pos")) {
            [Nbt::Double(x), Nbt::Double(y), Nbt::Double(z)] => [*x, *y, *z],
            _ => {
                let range = f64::from(if ominous { ctx.ominous_cfg.range } else { ctx.normal.range });
                let r = &mut self.mobs.rng;
                let x = f64::from(p[0]) + (r.next_double() - r.next_double()) * range + 0.5;
                let y = f64::from(p[1] + r.next_int_bound(3) - 1);
                let z = f64::from(p[2]) + (r.next_double() - r.next_double()) * range + 0.5;
                [x, y, z]
            }
        };
        let i = kind.info();
        if !rc_physics::is_free(&self.world(), &super::mob::Mob::bbox_at(pos, i.width, i.height)) {
            return None;
        }
        if !self.sight_to(pos, center(p), p) {
            return None;
        }
        let bp = block_of(pos);
        if !self.spawn_rules_for(kind, bp, Reason::TrialSpawner) {
            return None;
        }
        if let Some(rules) = next.get("custom_spawn_rules")
            && !self.spawn_custom_rules_ok(rules, bp)
        {
            return None;
        }
        let only_id = matches!(&ent, Nbt::Compound(f) if f.len() == 1);
        let yaw = self.mobs.rng.next_float() * 360.0;
        let mut m = self.new_mob(kind, pos, yaw);
        if !only_id {
            let mut full = ent.clone();
            nbtio::set(&mut full, "Pos", Nbt::List(pos.iter().map(|&v| Nbt::Double(v)).collect()));
            self.load_mob(&mut m, &full);
            m.pos = pos;
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
        let flame = i32::from(ominous);
        self.level_event(3011, p, flame);
        self.level_event(3012, bp, flame);
        let src = super::recent::vibration::Src::Living(Ent::Mob(id));
        self.game_event_src("minecraft:entity_place", bp.map(|v| f64::from(v) + 0.5), Some(src), None);
        Some(uuid)
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
            let tags = &self.block_tags;
            let data = &self.data;
            let tag_fn = |tag: &str, n: &str| {
                let id = data.blocks.iter().position(|b| b.name == n).map(|i| i as u32);
                id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
            };
            let l = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let mut rng = super::jrand::JRand::new(seed);
            l.drops(&ctx.ominous_cfg.ominous_items, &crate::loot::Ctx::default(), &mut rng)
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
        let near: Vec<Seen> = self
            .seen_players()
            .into_iter()
            .filter(|s| t.players.contains(&s.uuid) && !s.creative && !s.spectator && s.alive && dist2(s.pos, c) <= r2)
            .collect();
        if near.is_empty() {
            return;
        }
        let mobs: Vec<([f64; 3], f64)> = t
            .mobs
            .iter()
            .filter_map(|u| self.mobs.get_uuid(*u))
            .filter(|m| m.alive() && dist2(m.pos, c) <= r2)
            .map(|m| (m.pos, f64::from(m.kind.info().height)))
            .collect();
        let use_mobs = self.mobs.rng.next_bool();
        let pool: Vec<([f64; 3], f64)> = if use_mobs { mobs } else { near.iter().map(|s| (s.pos, 1.8)).collect() };
        if pool.is_empty() {
            return;
        }
        let (at, h) =
            if pool.len() == 1 { pool[0] } else { pool[self.mobs.rng.next_int_bound(pool.len() as i32) as usize] };
        let up = h + 2.0 + f64::from(self.mobs.rng.next_int_bound(4));
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
        self.mobs.spawners.blocks.items.push(OminousItem { id, uuid, pos, item, after, age: 0 });
    }

    fn ominous_items_tick(&mut self) {
        if self.mobs.spawners.blocks.items.is_empty() {
            return;
        }
        let mut list = std::mem::take(&mut self.mobs.spawners.blocks.items);
        let mut gone = Vec::new();
        list.retain_mut(|o| {
            o.age += 1;
            if o.age == o.after - 36 {
                self.world_sound(
                    "minecraft:block.trial_spawner.about_to_spawn_item",
                    6,
                    center(block_of(o.pos)),
                    1.0,
                    1.0,
                );
            }
            if o.age >= o.after {
                self.ominous_item_release(o);
                gone.push(o.id);
                return false;
            }
            true
        });
        list.append(&mut self.mobs.spawners.blocks.items);
        self.mobs.spawners.blocks.items = list;
        if !gone.is_empty() {
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

    fn vault_tick(&mut self, p: P) {
        let Some(be) = self.be_get(p).cloned() else { return };
        let mut v = VaultData::load(&be);
        let state = self.prop(p, "vault_state").unwrap_or_else(|| "inactive".into());
        let s = self.get(p);
        let ominous = self.v().get_bool(s, "ominous");
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
        let ns = self.v().set(s, "vault_state", to);
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
            let tags = &self.block_tags;
            let data = &self.data;
            let tag_fn = |tag: &str, n: &str| {
                let id = data.blocks.iter().position(|b| b.name == n).map(|i| i as u32);
                id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
            };
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
        if self.prop(p, "vault_state").as_deref() != Some("active") {
            return None;
        }
        let be = self.be_get(p).cloned()?;
        let mut v = VaultData::load(&be);
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
        let ominous = self.v().get_bool(self.get(p), "ominous");
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

    /// Spawn egg on a monster spawner or a trial spawner: the mob to spawn
    /// changes; true if the egg was used that way.
    pub(crate) fn spawn_egg_on_spawner(&mut self, p: P, entity: &str) -> Option<bool> {
        let id = self.be_get(p).and_then(|b| b.get("id")).and_then(Nbt::as_str).map(str::to_owned)?;
        if id != CAGE && id != TRIAL {
            return None;
        }
        if !self.rule("minecraft:spawner_blocks_work", true) {
            let msg = rc_text::Component::translate("advMode.notEnabled.spawner", Vec::new());
            self.chat_out(rc_client::Event::Chat { component: msg, tag: rc_client::ChatTag::System });
            return Some(false);
        }
        let only = |e: &str| {
            Nbt::Compound(vec![("entity".into(), Nbt::Compound(vec![("id".into(), Nbt::String(e.to_owned()))]))])
        };
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
            let mut ent = data.get("entity").cloned().unwrap_or(Nbt::Compound(Vec::new()));
            nbtio::set(&mut ent, "id", Nbt::String(entity.to_owned()));
            nbtio::set(&mut data, "entity", ent);
            if let Some(b) = self.be_get_mut(p) {
                nbtio::set(b, "SpawnData", data);
            }
        } else {
            // Trial spawner: settings replaced by the single mob, state reset.
            let be = self.be_get(p).cloned()?;
            let put = |key: &str, raw: Option<&Nbt>, s: &mut Server| {
                let base = s.trial_cfg(raw);
                let mut c = vec![
                    ("spawn_range".into(), Nbt::Int(base.range)),
                    ("total_mobs".into(), Nbt::Float(base.total)),
                    ("simultaneous_mobs".into(), Nbt::Float(base.simul)),
                    ("total_mobs_added_per_player".into(), Nbt::Float(base.total_add)),
                    ("simultaneous_mobs_added_per_player".into(), Nbt::Float(base.simul_add)),
                    ("ticks_between_spawn".into(), Nbt::Int(base.between)),
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
                        base.eject
                            .iter()
                            .map(|(t, w)| {
                                Nbt::Compound(vec![
                                    ("data".into(), Nbt::String(t.clone())),
                                    ("weight".into(), Nbt::Int(*w)),
                                ])
                            })
                            .collect(),
                    ),
                ));
                c.push(("items_to_drop_when_ominous".into(), Nbt::String(base.ominous_items)));
                if let Some(b) = s.be_get_mut(p) {
                    nbtio::set(b, key, Nbt::Compound(c));
                }
            };
            put("normal_config", be.get("normal_config"), self);
            put("ominous_config", be.get("ominous_config"), self);
            let t = TrialData::default();
            self.trial_store(p, &t);
            let s = self.get(p);
            let ns = self.v().set(s, "trial_spawner_state", "inactive");
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

/// Common settings of a trial spawner tick.
struct TrialCtx {
    normal: Cfg,
    ominous_cfg: Cfg,
    target_cd: i64,
    range: i32,
    p: P,
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
            activation: num(g("activation_range")).unwrap_or(4.0),
            deactivation: num(g("deactivation_range")).unwrap_or(4.5),
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
            particles_range: num(sh.and_then(|s| s.get("connected_particles_range"))).unwrap_or(4.5),
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
            p.s.spawner_blocks_tick();
        }
    }

    fn state(p: &Probe, key: &str) -> String {
        p.s.prop(AT, key).unwrap_or_default()
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
        p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Zombie && !m.removed).map(|m| m.uuid).collect()
    }

    fn trial_with_zombies(p: &mut Probe) {
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
                            Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:zombie".into()))]),
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
        assert!(p.s.mobs.list.iter().flatten().any(|m| m.uuid == first && m.persistent));
        run(&mut p, 60);
        assert_eq!(zombies(&p).len(), 1, "only one at a time");
        for m in p.s.mobs.list.iter_mut().flatten() {
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
        for m in p.s.mobs.list.iter_mut().flatten() {
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

    /// Egg on a monster spawner: the mob to spawn changes, no mob is born.
    #[test]
    fn spawn_egg_sets_cage_entity() {
        let Some((mut p, dir)) = probe("w9egg") else { return };
        p.run("setblock 216 -59 212 minecraft:spawner\n");
        let before = p.s.mobs.list.len();
        assert!(p.s.use_spawn_egg("minecraft:skeleton_spawn_egg", AT, 1));
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
}
