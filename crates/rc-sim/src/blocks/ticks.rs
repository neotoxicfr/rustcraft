//! World tick engine (§ 1.2 to 1.7): scheduled-tick queues per chunk and collection
//! in the game's order, random ticks, block events, ticking block entities.
//! Saving and resuming pending ticks (`block_ticks`, `fluid_ticks` of the chunk).

use hashbrown::{HashMap, HashSet};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use rc_proto::nbt::Nbt;

use super::{Dir, P};
use crate::server::Server;

/// Scheduled tick: due time, priority, sub-order, position, type (block or fluid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Tick {
    pub at: i64,
    pub priority: i8,
    pub sub: i64,
    pub pos: P,
    pub kind: u32,
}

#[derive(Default)]
struct Container {
    heap: BinaryHeap<Reverse<Tick>>,
    set: HashSet<(P, u32)>,
}

impl Container {
    fn peek(&self) -> Option<Tick> {
        self.heap.peek().map(|r| r.0)
    }

    fn poll(&mut self) -> Option<Tick> {
        let t = self.heap.pop()?.0;
        self.set.remove(&(t.pos, t.kind));
        Some(t)
    }
}

/// Queue of scheduled ticks of one type (blocks or fluids), § 1.3.
#[derive(Default)]
pub(crate) struct TickQueue {
    containers: HashMap<(i32, i32), Container>,
    next: HashMap<(i32, i32), i64>,
    to_run: VecDeque<Tick>,
    to_run_set: HashSet<(P, u32)>,
}

fn chunk_of(p: P) -> (i32, i32) {
    (p[0] >> 4, p[2] >> 4)
}

/// Head of a chunk in the heap: (priority, sub-order), chunk.
type Head = ((i8, i64), (i32, i32));

/// "In-tick" order: priority then sub-order.
fn intra(t: &Tick) -> (i8, i64) {
    (t.priority, t.sub)
}

impl TickQueue {
    /// Schedules a tick; false if (position, type) is already pending.
    pub fn schedule(&mut self, pos: P, kind: u32, at: i64, priority: i8, sub: i64) -> bool {
        let ch = chunk_of(pos);
        let c = self.containers.entry(ch).or_default();
        if !c.set.insert((pos, kind)) {
            return false;
        }
        let t = Tick { at, priority, sub, pos, kind };
        c.heap.push(Reverse(t));
        if c.peek() == Some(t) {
            self.next.insert(ch, at);
        }
        true
    }

    pub fn has(&self, pos: P, kind: u32) -> bool {
        self.containers.get(&chunk_of(pos)).is_some_and(|c| c.set.contains(&(pos, kind)))
    }

    pub fn will_tick_this_tick(&self, pos: P, kind: u32) -> bool {
        self.to_run_set.contains(&(pos, kind))
    }

    /// Collection of due ticks (§ 1.3, collection at tick T).
    pub fn collect(&mut self, now: i64, max: usize, ticking: impl Fn((i32, i32)) -> bool) {
        // Chunks whose head is due, within tick range.
        let mut pq: BinaryHeap<Reverse<Head>> = BinaryHeap::new();
        let keys: Vec<((i32, i32), i64)> = self.next.iter().map(|(k, v)| (*k, *v)).collect();
        for (ch, next) in keys {
            if next > now {
                continue;
            }
            let Some(c) = self.containers.get(&ch) else {
                self.next.remove(&ch);
                continue;
            };
            match c.peek() {
                None => {
                    self.next.remove(&ch);
                }
                Some(t) if t.at > now => {
                    self.next.insert(ch, t.at);
                }
                Some(t) => {
                    if ticking(ch) {
                        self.next.remove(&ch);
                        pq.push(Reverse((intra(&t), ch)));
                    }
                }
            }
        }
        // Heap drain on the heads.
        while self.to_run.len() < max {
            let Some(Reverse((_, ch))) = pq.pop() else { break };
            let next_best = pq.peek().map(|r| r.0.0);
            let Some(c) = self.containers.get_mut(&ch) else { continue };
            if let Some(t) = c.poll() {
                self.to_run_set.insert((t.pos, t.kind));
                self.to_run.push_back(t);
            }
            if self.to_run.len() < max {
                while self.to_run.len() < max {
                    let Some(cur) = c.peek() else { break };
                    if cur.at > now || next_best.is_some_and(|nb| intra(&cur) > nb) {
                        break;
                    }
                    c.poll();
                    self.to_run_set.insert((cur.pos, cur.kind));
                    self.to_run.push_back(cur);
                }
            }
            if let Some(nt) = c.peek() {
                if nt.at <= now && self.to_run.len() < max {
                    pq.push(Reverse((intra(&nt), ch)));
                } else {
                    self.next.insert(ch, nt.at);
                }
            }
        }
        for Reverse((_, ch)) in pq {
            if let Some(t) = self.containers.get(&ch).and_then(Container::peek) {
                self.next.insert(ch, t.at);
            }
        }
    }

    pub fn pop_next(&mut self) -> Option<Tick> {
        let t = self.to_run.pop_front()?;
        self.to_run_set.remove(&(t.pos, t.kind));
        Some(t)
    }

    pub fn cleanup(&mut self) {
        self.to_run.clear();
        self.to_run_set.clear();
    }

    /// Pending ticks of a chunk (save).
    pub fn chunk_ticks(&self, ch: (i32, i32)) -> Vec<Tick> {
        let mut v: Vec<Tick> =
            self.containers.get(&ch).map(|c| c.heap.iter().map(|r| r.0).collect()).unwrap_or_default();
        v.sort();
        v
    }
}

/// Names of the fluid types (indices of `Fluid::type_id`).
pub(crate) const FLUID_NAMES: [&str; 5] =
    ["minecraft:empty", "minecraft:water", "minecraft:flowing_water", "minecraft:lava", "minecraft:flowing_lava"];

impl Server {
    fn rule_int(&self, name: &str, default: i64) -> i64 {
        self.rules.get(name).and_then(Nbt::as_i64).unwrap_or(default)
    }

    /// "Block" phases of a world tick (§ 1.2, steps 7 to 11) then block entities, without the
    /// other entities (block probes).
    #[cfg(test)]
    pub(crate) fn tick_world_blocks(&mut self) {
        self.tick_level_blocks();
        self.tick_block_entities();
    }

    /// "Block" phases of a world tick (§ 1.2, steps 7 to 11a), after time advances;
    /// block entities tick after all entities (`tick_block_entities`, called by the
    /// server tick after mobs, items and projectiles, like the game).
    pub(crate) fn tick_level_blocks(&mut self) {
        self.bs.handling_tick = true;
        let now = self.time;
        // 7. Scheduled ticks of blocks then of fluids.
        let chunks: HashSet<(i32, i32)> = self.chunks.iter().filter(|(_, c)| c.is_some()).map(|(k, _)| *k).collect();
        // Ticking chunks: loaded neighbours AND within a player's simulation range (or
        // force-loaded); chunks visited then left no longer grow.
        let centers = self.sim_centers();
        let simulated: HashSet<(i32, i32)> =
            chunks.iter().copied().filter(|&c| self.chunk_simulated(c, &centers)).collect();
        let ticking = |ch: (i32, i32)| {
            simulated.contains(&ch) && (-1..=1).all(|dx| (-1..=1).all(|dz| chunks.contains(&(ch.0 + dx, ch.1 + dz))))
        };
        self.bs.block_ticks.collect(now, 65536, ticking);
        while let Some(t) = self.bs.block_ticks.pop_next() {
            let s = self.get(t.pos);
            if u32::from(self.v().block(s)) == t.kind {
                self.tick_block(s, t.pos);
            }
        }
        self.bs.block_ticks.cleanup();
        self.bs.fluid_ticks.collect(now, 65536, ticking);
        while let Some(t) = self.bs.fluid_ticks.pop_next() {
            let s = self.get(t.pos);
            if u32::from(self.fluid_of(s).type_id()) == t.kind {
                self.fluid_tick(s, t.pos);
            }
        }
        self.bs.fluid_ticks.cleanup();
        // 9. Random ticks of the "ticking entities" chunks (simulation distance),
        // loaded neighbours.
        let speed = self.rule_int("minecraft:random_tick_speed", 3).max(0) as usize;
        let mut list: Vec<(i32, i32)> =
            chunks.iter().copied().filter(|&c| ticking(c) && self.chunk_entity_ticking(c, &centers)).collect();
        list.sort_unstable();
        for ch in list {
            self.tick_chunk(ch, speed);
        }
        // 10. Block events.
        self.run_block_events();
        self.bs.handling_tick = false;
        // 11a. Entities (falling blocks, primed TNT); block entities later.
        self.tick_sim_entities();
    }

    /// Position drawn at random in a section (world LCG, § 1.5).
    fn random_pos(&mut self, x: i32, y: i32, z: i32) -> P {
        self.bs.lcg = self.bs.lcg.wrapping_mul(3).wrapping_add(1_013_904_223);
        let w = self.bs.lcg >> 2;
        [x + (w & 15), y + ((w >> 16) & 15), z + ((w >> 8) & 15)]
    }

    fn tick_chunk(&mut self, (cx, cz): (i32, i32), speed: usize) {
        let (x0, z0) = (cx << 4, cz << 4);
        // Thunderstorms are drawn in the spawning chunks (`natural_spawn_tick`), not here.
        for _ in 0..speed {
            if self.bs.rng.next_int_bound(48) == 0 {
                let p = self.random_pos(x0, 0, z0);
                self.tick_precipitation(p);
            }
        }
        if speed == 0 {
            return;
        }
        let n = self.dim.height.sections;
        for si in 0..n {
            if !self.section_random_ticks(cx, cz, si) {
                continue;
            }
            let y0 = self.dim.height.min_y + si as i32 * 16;
            for _ in 0..speed {
                let p = self.random_pos(x0, y0, z0);
                let s = self.get(p);
                if self.v().random_ticks(s) {
                    self.random_tick(s, p);
                }
                let f = self.fluid_of(s);
                if f.kind == 2 {
                    self.lava_random_tick(p);
                }
            }
        }
    }

    /// The section contains at least one random-tick state (block or fluid).
    fn section_random_ticks(&mut self, cx: i32, cz: i32, si: usize) -> bool {
        if let Some(&b) = self.bs.rt.get(&(cx, cz, si)) {
            return b;
        }
        let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { return false };
        let Some(sec) = c.sections.get(si) else { return false };
        let v = self.v();
        let b = sec.non_empty != 0
            && (0..4096).any(|i| {
                let s = sec.blocks.get(i);
                v.random_ticks(s) || self.fluid_of(s).kind == 2
            });
        self.bs.rt.insert((cx, cz, si), b);
        b
    }

    fn run_block_events(&mut self) {
        let mut later = Vec::new();
        while let Some(e) = self.bs.events.pop_front() {
            self.bs.event_set.remove(&e);
            if self.ticking_at(e.pos) {
                let s = self.get(e.pos);
                if self.v().block(s) == e.block && self.trigger_event(s, e.pos, e.a, e.b) {
                    let _ = self.send(rc_client::Event::BlockEvent {
                        pos: e.pos,
                        b0: e.a as u8,
                        b1: e.b as u8,
                        block: i32::from(e.block),
                    });
                }
            } else {
                later.push(e);
            }
        }
        for e in later {
            if self.bs.event_set.insert(e) {
                self.bs.events.push_back(e);
            }
        }
    }

    /// Adds a ticking block entity (pending if the walk is in progress, § 1.7).
    pub(crate) fn add_ticker(&mut self, p: P) -> u64 {
        let id = self.bs.next_be;
        self.bs.next_be += 1;
        if self.bs.ticking_bes {
            self.bs.pending_tickers.push((id, p));
        } else {
            self.bs.tickers.push((id, p));
        }
        id
    }

    pub(crate) fn tick_block_entities(&mut self) {
        self.bs.ticking_bes = true;
        let pending = std::mem::take(&mut self.bs.pending_tickers);
        self.bs.tickers.extend(pending);
        let mut i = 0;
        while i < self.bs.tickers.len() {
            let (id, p) = self.bs.tickers[i];
            if !self.ticker_alive(id, p) {
                self.bs.tickers.remove(i);
                continue;
            }
            if self.be_ticking(p) {
                self.tick_ticker(id, p);
            }
            i += 1;
        }
        self.bs.ticking_bes = false;
    }

    fn ticker_alive(&mut self, id: u64, p: P) -> bool {
        if let Some(m) = self.bs.movers.get(&p) {
            return m.id == id;
        }
        // F14: creaking hearts, shriekers.
        if let Some(alive) = self.rec_ticker_alive(id, p) {
            return alive;
        }
        // G6-F01: jukebox, campfires.
        if let Some(alive) = self.be_tick_alive(id, p) {
            return alive;
        }
        // F11: beehives and nests.
        if self.hive_is_ticker(id, p) {
            return self.hive_ticker_alive(id, p);
        }
        // Shulker box lid; box removed: state forgotten.
        if let Some(alive) = self.shulker_ticker(id, p) {
            if !alive {
                self.bs.shulkers.remove(&p);
            }
            return alive;
        }
        self.hopper_ticker_alive(id, p)
    }

    fn tick_ticker(&mut self, id: u64, p: P) {
        if self.bs.movers.get(&p).is_some_and(|m| m.id == id) {
            let s = self.get(p);
            if self.v().kind(s) == super::Kind::MovingPiston {
                self.mover_tick(p);
            }
        } else if let Some(&(i, t)) = self.bs.be_ticks.get(&p)
            && i == id
        {
            // G6-F01: jukebox, campfires (tick skipped without food or started cooking).
            match t {
                super::level::BeTick::Jukebox(song) => self.jukebox_tick(p, song),
                super::level::BeTick::Campfire { food, warm } => self.campfire_tick(p, food, warm),
                super::level::BeTick::Gateway => self.gateway_be_tick(p), // G5-F10
                super::level::BeTick::Beacon(b) => self.beacon_tick(p, b), // G5-F11
                super::level::BeTick::Conduit(c) => self.conduit_tick(p, c), // G6-F12
            }
        } else if self.hive_is_ticker(id, p) {
            self.hive_tick(p); // F11
        } else if self.shulker_ticker(id, p).is_some() {
            self.shulker_tick(p);
        } else if self.rec_tick_ticker(id, p) {
            // F14: creaking heart, shrieker vibrations.
        } else {
            self.hopper_tick(p);
        }
    }

    /// Resumption of the saved ticks of a chunk that has just been loaded.
    pub(crate) fn import_chunk_ticks(&mut self, ch: (i32, i32)) {
        if !self.bs.imported.insert(ch) {
            return;
        }
        let Some(Some(c)) = self.chunks.get(&ch) else { return };
        let mut todo: Vec<(bool, Nbt)> = Vec::new();
        for (key, fluid) in [("block_ticks", false), ("fluid_ticks", true)] {
            if let Some(Nbt::List(l)) = c.root.get(key) {
                todo.extend(l.iter().map(|t| (fluid, t.clone())));
            }
        }
        for (fluid, t) in todo {
            let int = |k: &str| crate::nbtio::int(&t, k).unwrap_or(0) as i32;
            let Some(name) = t.get("i").and_then(Nbt::as_str) else { continue };
            let p = [int("x"), int("y"), int("z")];
            let (delay, priority) = (int("t"), int("p") as i8);
            let sub = self.bs.sub_tick;
            self.bs.sub_tick += 1;
            let at = self.time + i64::from(delay);
            if fluid {
                if let Some(k) = FLUID_NAMES.iter().position(|n| *n == name) {
                    self.bs.fluid_ticks.schedule(p, k as u32, at, 0, sub);
                }
            } else {
                let b = self.bs.t.block_id(name);
                if b != u16::MAX {
                    self.bs.block_ticks.schedule(p, u32::from(b), at, priority, sub);
                }
            }
        }
        self.register_chunk_tickers(ch);
        self.import_movers(ch);
    }

    /// Writes the pending ticks of a chunk into its compound (before the save).
    pub(crate) fn export_chunk_ticks(&mut self, ch: (i32, i32)) {
        let now = self.time;
        let names: Vec<String> = self.data.blocks.iter().map(|b| b.name.clone()).collect();
        let enc = |t: &Tick, name: &str| {
            Nbt::Compound(vec![
                ("i".into(), Nbt::String(name.to_owned())),
                ("x".into(), Nbt::Int(t.pos[0])),
                ("y".into(), Nbt::Int(t.pos[1])),
                ("z".into(), Nbt::Int(t.pos[2])),
                ("t".into(), Nbt::Int((t.at - now) as i32)),
                ("p".into(), Nbt::Int(i32::from(t.priority))),
            ])
        };
        let blocks: Vec<Nbt> = self
            .bs
            .block_ticks
            .chunk_ticks(ch)
            .iter()
            .filter_map(|t| names.get(t.kind as usize).map(|n| enc(t, n)))
            .collect();
        let fluids: Vec<Nbt> = self
            .bs
            .fluid_ticks
            .chunk_ticks(ch)
            .iter()
            .filter_map(|t| FLUID_NAMES.get(t.kind as usize).map(|n| enc(t, n)))
            .collect();
        // Moving blocks: `minecraft:piston` block entities (state, direction, progress).
        let movers: Vec<Nbt> = self
            .bs
            .movers
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == ch)
            .map(|(p, m)| {
                Nbt::Compound(vec![
                    ("id".into(), Nbt::String("minecraft:piston".into())),
                    ("x".into(), Nbt::Int(p[0])),
                    ("y".into(), Nbt::Int(p[1])),
                    ("z".into(), Nbt::Int(p[2])),
                    ("blockState".into(), self.states.palette_entry(&self.data, m.moved)),
                    ("facing".into(), Nbt::Int(m.dir as i32)),
                    ("progress".into(), Nbt::Float(m.progress_o)),
                    ("extending".into(), Nbt::Byte(i8::from(m.extending))),
                    ("source".into(), Nbt::Byte(i8::from(m.source))),
                ])
            })
            .collect();
        if let Some(Some(c)) = self.chunks.get_mut(&ch) {
            crate::nbtio::set(&mut c.root, "block_ticks", Nbt::List(blocks));
            crate::nbtio::set(&mut c.root, "fluid_ticks", Nbt::List(fluids));
            c.block_entities.retain(|b| b.get("id").and_then(Nbt::as_str) != Some("minecraft:piston"));
            c.block_entities.extend(movers);
        }
    }

    /// Saved moving blocks of a loaded chunk.
    fn import_movers(&mut self, ch: (i32, i32)) {
        let Some(Some(c)) = self.chunks.get(&ch) else { return };
        let list: Vec<Nbt> = c
            .block_entities
            .iter()
            .filter(|b| b.get("id").and_then(Nbt::as_str) == Some("minecraft:piston"))
            .cloned()
            .collect();
        for b in list {
            let i = |k: &str| crate::nbtio::int(&b, k).unwrap_or(0) as i32;
            let p = [i("x"), i("y"), i("z")];
            let Some(moved) = b.get("blockState").and_then(|e| self.states.state_of(&self.data, e)) else { continue };
            let progress = match b.get("progress") {
                Some(Nbt::Float(f)) => *f,
                _ => 0.0,
            };
            let id = self.add_ticker(p);
            self.bs.movers.insert(
                p,
                super::level::Mover {
                    id,
                    moved,
                    dir: Dir::from_index(i("facing").clamp(0, 5) as usize),
                    extending: i("extending") != 0,
                    source: i("source") != 0,
                    progress,
                    progress_o: progress,
                    last_ticked: i64::MIN,
                },
            );
        }
    }
}
