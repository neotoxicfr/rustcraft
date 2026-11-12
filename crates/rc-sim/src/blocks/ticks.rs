//! World tick engine (§ 1.2 to 1.7): scheduled-tick queues per chunk and collection
//! in the game's order, random ticks, block events, ticking block entities.
//! Saving and resuming pending ticks (`block_ticks`, `fluid_ticks` of the chunk).

use hashbrown::{HashMap, HashSet};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use rc_proto::nbt::Nbt;

use super::{Dir, P};
use crate::server::Server;

/// Optimised paths of the world tick engine (`RUSTCRAFT_OPT_*`, on by default, `=0` for the
/// former path, read once); a test picks its own per thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockTickPaths {
    /// Ticking chunks listed from what simulates them (players, forced chunks, pearl tickets,
    /// dragon fight) instead of testing every loaded chunk (`RUSTCRAFT_OPT_SIMULATED_CANDIDATES`).
    pub simulated_candidates: bool,
    /// With the listed chunks, the loaded flags of their box read once into a grid instead of 9
    /// table lookups per candidate (`RUSTCRAFT_OPT_TICKING_LOADED_GRID`).
    pub loaded_grid: bool,
    /// Due scheduled ticks found from the shorter of the ticking chunks and the chunks with
    /// pending ticks, without copying the keys, the heap of chunk heads kept between ticks
    /// (`RUSTCRAFT_OPT_TICK_QUEUE_CANDIDATES`).
    pub tick_queue_candidates: bool,
    /// Random-tick test of a section answered by its palette when it can
    /// (`RUSTCRAFT_OPT_RANDOM_TICK_PALETTE`).
    pub random_tick_palette: bool,
    /// Ticker walk compacted in one pass, registrations looked up in a set instead of a scan of
    /// the list (`RUSTCRAFT_OPT_TICKER_COMPACTION`).
    pub ticker_compaction: bool,
    /// Ticker walk checking only the family recorded at registration
    /// (`RUSTCRAFT_OPT_TICKER_FAMILY`).
    pub ticker_family: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests: paths used by this thread (equivalence proofs), without touching the environment.
    static FORCED_BLOCK_TICK_PATHS: std::cell::Cell<Option<BlockTickPaths>> = const { std::cell::Cell::new(None) };
}

impl BlockTickPaths {
    pub(crate) fn current() -> BlockTickPaths {
        #[cfg(test)]
        if let Some(paths) = FORCED_BLOCK_TICK_PATHS.with(std::cell::Cell::get) {
            return paths;
        }
        BlockTickPaths {
            simulated_candidates: rc_data::opt_switch!("RUSTCRAFT_OPT_SIMULATED_CANDIDATES"),
            loaded_grid: rc_data::opt_switch!("RUSTCRAFT_OPT_TICKING_LOADED_GRID"),
            tick_queue_candidates: rc_data::opt_switch!("RUSTCRAFT_OPT_TICK_QUEUE_CANDIDATES"),
            random_tick_palette: rc_data::opt_switch!("RUSTCRAFT_OPT_RANDOM_TICK_PALETTE"),
            ticker_compaction: rc_data::opt_switch!("RUSTCRAFT_OPT_TICKER_COMPACTION"),
            ticker_family: rc_data::opt_switch!("RUSTCRAFT_OPT_TICKER_FAMILY"),
        }
    }

    /// Every new path.
    #[cfg(test)]
    pub(crate) const NEW: BlockTickPaths = BlockTickPaths {
        simulated_candidates: true,
        loaded_grid: true,
        tick_queue_candidates: true,
        random_tick_palette: true,
        ticker_compaction: true,
        ticker_family: true,
    };

    /// Every former path.
    #[cfg(test)]
    pub(crate) const OLD: BlockTickPaths = BlockTickPaths {
        simulated_candidates: false,
        loaded_grid: false,
        tick_queue_candidates: false,
        random_tick_palette: false,
        ticker_compaction: false,
        ticker_family: false,
    };

    /// Runs `f` with these paths on the current thread.
    #[cfg(test)]
    pub(crate) fn run<R>(self, f: impl FnOnce() -> R) -> R {
        let before = FORCED_BLOCK_TICK_PATHS.with(|c| c.replace(Some(self)));
        let out = f();
        FORCED_BLOCK_TICK_PATHS.with(|c| c.set(before));
        out
    }
}

/// The 3 × 3 chunks centred on `ch`.
fn chunk_and_neighbours(ch: (i32, i32)) -> impl Iterator<Item = (i32, i32)> {
    (-1..=1).flat_map(move |dx| (-1..=1).map(move |dz| (ch.0 + dx, ch.1 + dz)))
}

/// Loaded flags of a box of chunks, each read once from the chunk table: neighbouring candidates
/// share most of their 3 × 3 tests, which cost 9 table lookups each without it.
struct LoadedGrid {
    min: (i32, i32),
    depth: usize,
    loaded: Vec<bool>,
}

impl LoadedGrid {
    /// The box of the sorted `cells` grown by one chunk, when it stays close to their count (one
    /// square or overlapping ones); `None` for scattered sources (a far pearl, a distant guest).
    fn around(cells: &[(i32, i32)], loaded: impl Fn((i32, i32)) -> bool) -> Option<LoadedGrid> {
        let (x0, x1) = (cells.first()?.0 - 1, cells.last()?.0 + 1);
        let (z0, z1) = cells.iter().fold((i32::MAX, i32::MIN), |(lo, hi), c| (lo.min(c.1 - 1), hi.max(c.1 + 1)));
        let (width, depth) = ((x1 - x0 + 1) as usize, (z1 - z0 + 1) as usize);
        if width.saturating_mul(depth) > 2 * cells.len() + 256 {
            return None;
        }
        let loaded = (x0..=x1).flat_map(|x| (z0..=z1).map(move |z| (x, z))).map(loaded).collect();
        Some(LoadedGrid { min: (x0, z0), depth, loaded })
    }

    /// Only for the cells given to `around` and their neighbours.
    fn get(&self, c: (i32, i32)) -> bool {
        self.loaded[(c.0 - self.min.0) as usize * self.depth + (c.1 - self.min.1) as usize]
    }
}

/// Chunks of a square of Chebyshev radius `r` (none below 0).
fn square_cells(r: i32) -> usize {
    let side = usize::try_from(r).map_or(0, |r| 2 * r + 1);
    side.saturating_mul(side)
}

/// Time spent in the block phases of a world tick (scheduled ticks, random ticks, block events),
/// added to `server::BLOCKS_NS` for the benches; nothing is measured outside tests.
#[derive(Default)]
struct BlockPhaseTime {
    #[cfg(test)]
    spent: std::time::Duration,
}

impl BlockPhaseTime {
    /// Runs one block phase, counting its time.
    #[inline]
    fn count<R>(&mut self, phase: impl FnOnce() -> R) -> R {
        #[cfg(test)]
        let start = std::time::Instant::now();
        let out = phase();
        #[cfg(test)]
        {
            self.spent += start.elapsed();
        }
        out
    }

    fn publish(self) {
        #[cfg(test)]
        crate::server::BLOCKS_NS.with(|n| n.set(n.get() + self.spent.as_nanos() as u64));
    }
}

/// Chunks whose blocks tick this tick (§ 1.4): simulated ("ticking blocks"), loaded, their 8
/// neighbours loaded; fixed before the scheduled ticks and kept for the random ticks.
pub(crate) enum TickingChunks {
    /// Former path: the loaded chunks and the simulated ones among them.
    Sets { loaded: HashSet<(i32, i32)>, simulated: HashSet<(i32, i32)> },
    /// The ticking chunks themselves, in increasing order.
    Sorted(Vec<(i32, i32)>),
}

impl TickingChunks {
    pub(crate) fn contains(&self, ch: (i32, i32)) -> bool {
        match self {
            TickingChunks::Sets { loaded, simulated } => {
                simulated.contains(&ch) && chunk_and_neighbours(ch).all(|c| loaded.contains(&c))
            }
            TickingChunks::Sorted(list) => list.binary_search(&ch).is_ok(),
        }
    }

    /// The ticking chunks in increasing order, when they are listed.
    fn sorted(&self) -> Option<&[(i32, i32)]> {
        match self {
            TickingChunks::Sets { .. } => None,
            TickingChunks::Sorted(list) => Some(list),
        }
    }
}

/// Which registration made a ticker (§ 1.7): the walk then checks that family alone
/// (`RUSTCRAFT_OPT_TICKER_FAMILY`) instead of asking each family's map in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickerFamily {
    /// State kept in the shared map `be_ticks`: jukebox, campfire, end gateway, beacon, conduit,
    /// potent sulfur, furnaces, brewing stand, bell, spawner blocks.
    Shared,
    /// Moving piston block (`movers`).
    Mover,
    /// Creaking heart.
    Heart,
    /// Sculk shrieker or sensor.
    Vibration,
    /// Beehive or bee nest.
    Hive,
    /// Shulker box lid.
    ShulkerLid,
    /// Hopper or daylight detector (`hoppers`).
    Hopper,
}

/// A ticking block entity: identifier (never reused), position, family; 24 bytes, the size of
/// the former (identifier, position) pair, the family sitting in its padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ticker {
    pub id: u64,
    pub pos: P,
    pub family: TickerFamily,
}

impl Ticker {
    /// Hole left by a dead ticker during a compacting walk: identifiers count up from 1, so
    /// no lookup of a real ticker matches it.
    const GONE: Ticker = Ticker { id: u64::MAX, pos: [i32::MIN; 3], family: TickerFamily::Hopper };
}

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
    /// Heads of the due ticking chunks during a collection, kept between ticks for its capacity.
    due_heads: BinaryHeap<Reverse<Head>>,
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

    /// Collection of due ticks (§ 1.3, collection at tick T) from the chunks of `ticking`;
    /// `by_candidates` false keeps the former walk over a copy of every chunk key.
    pub fn collect(&mut self, now: i64, max: usize, ticking: &TickingChunks, by_candidates: bool) {
        if !by_candidates {
            self.collect_by_key_copy(now, max, |ch| ticking.contains(ch));
            return;
        }
        let mut heads = std::mem::take(&mut self.due_heads);
        heads.clear();
        match ticking.sorted() {
            // A due chunk that does not tick keeps its ticks untouched until it ticks again: when
            // the ticking chunks are fewer, only they are looked up (chunks left behind with due
            // fluid ticks no longer cost a visit per tick).
            Some(list) if list.len() < self.next.len() => self.due_heads_of(now, list, &mut heads),
            _ => self.due_heads_by_retain(now, ticking, &mut heads),
        }
        self.drain_due_heads(now, max, &mut heads);
        self.due_heads = heads;
    }

    /// Heads of the due chunks among the ticking chunks `list`, taken out of `next`.
    fn due_heads_of(&mut self, now: i64, list: &[(i32, i32)], heads: &mut BinaryHeap<Reverse<Head>>) {
        for &ch in list {
            if !self.next.get(&ch).is_some_and(|&at| at <= now) {
                continue;
            }
            match self.containers.get(&ch).and_then(Container::peek) {
                None => {
                    self.next.remove(&ch);
                }
                Some(t) if t.at > now => {
                    self.next.insert(ch, t.at);
                }
                Some(t) => {
                    self.next.remove(&ch);
                    heads.push(Reverse((intra(&t), ch)));
                }
            }
        }
    }

    /// Heads of the due ticking chunks, found by one pass over `next` (no copy of its keys).
    fn due_heads_by_retain(&mut self, now: i64, ticking: &TickingChunks, heads: &mut BinaryHeap<Reverse<Head>>) {
        let containers = &self.containers;
        self.next.retain(|&ch, at| {
            if *at > now {
                return true;
            }
            match containers.get(&ch).and_then(Container::peek) {
                None => false,
                Some(t) if t.at > now => {
                    *at = t.at;
                    true
                }
                Some(t) if ticking.contains(ch) => {
                    heads.push(Reverse((intra(&t), ch)));
                    false
                }
                Some(_) => true,
            }
        });
    }

    /// Takes the due ticks in the game's order from the chunk heads (same drain as the former
    /// path), then gives the chunks left in `heads` their next due time back.
    fn drain_due_heads(&mut self, now: i64, max: usize, heads: &mut BinaryHeap<Reverse<Head>>) {
        while self.to_run.len() < max {
            let Some(Reverse((_, ch))) = heads.pop() else { break };
            let next_best = heads.peek().map(|r| r.0.0);
            let Some(c) = self.containers.get_mut(&ch) else { continue };
            // The chunk's head always goes, then its next ticks while they stay due and ahead
            // of the best other chunk.
            if let Some(t) = c.poll() {
                self.to_run_set.insert((t.pos, t.kind));
                self.to_run.push_back(t);
            }
            while self.to_run.len() < max {
                let Some(cur) = c.peek() else { break };
                if cur.at > now || next_best.is_some_and(|nb| intra(&cur) > nb) {
                    break;
                }
                c.poll();
                self.to_run_set.insert((cur.pos, cur.kind));
                self.to_run.push_back(cur);
            }
            if let Some(nt) = c.peek() {
                if nt.at <= now && self.to_run.len() < max {
                    heads.push(Reverse((intra(&nt), ch)));
                } else {
                    self.next.insert(ch, nt.at);
                }
            }
        }
        for Reverse((_, ch)) in heads.drain() {
            if let Some(t) = self.containers.get(&ch).and_then(Container::peek) {
                self.next.insert(ch, t.at);
            }
        }
    }

    /// Former collection: every chunk key copied, then each due one tested.
    fn collect_by_key_copy(&mut self, now: i64, max: usize, ticking: impl Fn((i32, i32)) -> bool) {
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

    /// Pending ticks inside the box `min..=max` (those queued for the current tick included),
    /// for an area copy (`clone`); only the chunks of the box are visited.
    pub fn in_box(&self, min: P, max: P) -> Vec<Tick> {
        let inside = |p: &P| (0..3).all(|i| p[i] >= min[i] && p[i] <= max[i]);
        let mut v: Vec<Tick> = self.to_run.iter().filter(|t| inside(&t.pos)).copied().collect();
        for cx in (min[0] >> 4)..=(max[0] >> 4) {
            for cz in (min[2] >> 4)..=(max[2] >> 4) {
                if let Some(c) = self.containers.get(&(cx, cz)) {
                    v.extend(c.heap.iter().map(|r| r.0).filter(|t| inside(&t.pos)));
                }
            }
        }
        v
    }

    /// Drops the pending ticks inside the box `min..=max` (those queued for the current tick
    /// included); only the chunks of the box are visited, a chunk keeps its next due time.
    pub fn clear_box(&mut self, min: P, max: P) {
        let inside = |p: &P| (0..3).all(|i| p[i] >= min[i] && p[i] <= max[i]);
        for cx in (min[0] >> 4)..=(max[0] >> 4) {
            for cz in (min[2] >> 4)..=(max[2] >> 4) {
                let Some(c) = self.containers.get_mut(&(cx, cz)) else { continue };
                if !c.set.iter().any(|(p, _)| inside(p)) {
                    continue;
                }
                c.heap.retain(|r| !inside(&r.0.pos));
                c.set.retain(|(p, _)| !inside(p));
                match c.peek() {
                    Some(t) => {
                        self.next.insert((cx, cz), t.at);
                    }
                    None => {
                        self.containers.remove(&(cx, cz));
                        self.next.remove(&(cx, cz));
                    }
                }
            }
        }
        if self.to_run_set.iter().any(|(p, _)| inside(p)) {
            self.to_run.retain(|t| !inside(&t.pos));
            self.to_run_set.retain(|(p, _)| !inside(p));
        }
    }

    /// Pending ticks of a chunk, sorted.
    #[cfg(test)]
    pub fn chunk_ticks(&self, ch: (i32, i32)) -> Vec<Tick> {
        let mut v = Vec::new();
        self.chunk_ticks_into(ch, &mut v);
        v
    }

    /// Pending ticks of a chunk in the game's save order, written into `out` (emptied first):
    /// by sub-order, the order they were scheduled in (ties, only from area copies, by the
    /// rest of the tick, so the order is always the same).
    pub fn chunk_ticks_into(&self, ch: (i32, i32), out: &mut Vec<Tick>) {
        out.clear();
        if let Some(c) = self.containers.get(&ch) {
            out.extend(c.heap.iter().map(|r| r.0));
        }
        // Compared in place: sub-orders are unique within a chunk but for area copies, so one
        // integer comparison decides almost every time.
        out.sort_unstable_by(|a, b| a.sub.cmp(&b.sub).then_with(|| a.cmp(b)));
    }
}

/// A tick read from a chunk's `block_ticks` or `fluid_ticks`.
struct StoredTick {
    pos: P,
    kind: u32,
    delay: i32,
    priority: i8,
}

/// The saved ticks of chunk `ch` the game keeps, in the file's order: every field there (`i` a
/// known block or fluid, `x` `y` `z` `t` `p` numbers) and the position inside the chunk; the
/// priority brought into -3..=3 like the game's.
fn saved_ticks(list: Option<&Nbt>, ch: (i32, i32), kind_of: impl Fn(&str) -> Option<u32>) -> Vec<StoredTick> {
    let Some(Nbt::List(list)) = list else { return Vec::new() };
    let read = |t: &Nbt| {
        let int = |k: &str| crate::nbtio::int(t, k).map(|v| v as i32);
        let kind = kind_of(t.get("i")?.as_str()?)?;
        let pos = [int("x")?, int("y")?, int("z")?];
        let (delay, priority) = (int("t")?, int("p")?.clamp(-3, 3) as i8);
        (chunk_of(pos) == ch).then_some(StoredTick { pos, kind, delay, priority })
    };
    list.iter().filter_map(read).collect()
}

/// Saved ticks back in their queue like the game: sub-orders -n..-1 in the file's order, so at
/// the same due tick and priority they run before every tick scheduled since the server started,
/// in the order they had when saved.
fn resume_saved_ticks(queue: &mut TickQueue, ticks: &[StoredTick], now: i64) {
    for (t, sub) in ticks.iter().zip(-(ticks.len() as i64)..) {
        queue.schedule(t.pos, t.kind, now + i64::from(t.delay), t.priority, sub);
    }
}

#[cfg(test)]
#[path = "saved_ticks_tests.rs"]
mod saved_ticks_tests;

/// A non-empty section holds a state for which `random_ticks` answers yes. `palette` false reads
/// every cell (former path); true asks the palette first: no palette entry answering yes means
/// none of the cells does, and a single-state section answers with its one state, so the 4,096
/// cells are only read when the palette says "maybe" (it may keep states no cell uses any more).
pub(crate) fn section_has_random_ticks(
    sec: &rc_world::Section,
    palette: bool,
    random_ticks: impl Fn(u32) -> bool,
) -> bool {
    if sec.non_empty == 0 {
        return false;
    }
    if palette {
        if !sec.blocks.may_contain(&random_ticks) {
            return false;
        }
        if let rc_world::Container::Single(_) = sec.blocks {
            return true;
        }
    }
    (0..4096).any(|i| random_ticks(sec.blocks.get(i)))
}

/// Names of the fluid types (indices of `Fluid::type_id`).
pub(crate) const FLUID_NAMES: [&str; 5] =
    ["minecraft:empty", "minecraft:water", "minecraft:flowing_water", "minecraft:lava", "minecraft:flowing_lava"];

impl Server {
    fn rule_int(&self, name: &str, default: i64) -> i64 {
        self.rules.get(name).and_then(Nbt::as_i64).unwrap_or(default)
    }

    /// "Block" phases of a world tick (§ 1.2, steps 7 to 10), falling blocks and primed TNT, then
    /// block entities, without raids, spawns or the other entities (block probes).
    #[cfg(test)]
    pub(crate) fn tick_world_blocks(&mut self) {
        self.tick_level_blocks(false);
        self.tick_sim_entities();
        self.tick_block_entities();
    }

    /// Phases of a world tick from the scheduled ticks to the block events (§ 1.2, steps 7 to
    /// 10), after time advances: scheduled ticks of blocks then of fluids, raids, chunk source
    /// (spawning chunks: thunder and natural spawns; random ticks; special spawners), block
    /// events. `world` false (block probes): without raids and spawns. The entities then the
    /// block entities follow in the server tick.
    pub(crate) fn tick_level_blocks(&mut self, world: bool) {
        #[cfg(test)]
        crate::server::phase("scheduled");
        let paths = BlockTickPaths::current();
        let mut clock = BlockPhaseTime::default();
        self.bs.handling_tick = true;
        let now = self.time;
        // L9-TICK: the pearl tickets created during the previous tick apply (between two ticks).
        self.proj.pearl_tickets_apply();
        // 7. Scheduled ticks of blocks then of fluids, in the chunks ticking at the start of the
        // tick: loaded neighbours AND within a player's simulation range (or force-loaded, or a
        // pearl's or the dragon fight's ticket); chunks visited then left no longer grow. The
        // players' centres are read once, here.
        let (centers, ticking) = clock.count(|| {
            let centers = self.sim_centers();
            let ticking = self.ticking_chunks(&centers, paths.simulated_candidates);
            self.run_scheduled_ticks(now, &ticking, paths.tick_queue_candidates);
            (centers, ticking)
        });
        if world {
            // 8. Raids.
            self.raids_tick();
        }
        // 9. Chunk source: purge of the expired tickets first (L9-TICK: a pearl's ticket
        // stops counting from here, its chunks no longer simulated for the rest of the tick).
        self.proj.pearl_tickets_purge(now);
        // The chunks' new states: entities whose chunk ticks again join the entity list.
        self.walk_rejoin();
        if world {
            // Then the spawning chunks (thunder, natural spawns).
            self.natural_spawn_tick();
        }
        #[cfg(test)]
        crate::server::phase("random");
        clock.count(|| self.run_random_ticks(&ticking, &centers));
        if world {
            // 9. Then the special spawners (phantoms, patrols, cats, siege, trader).
            self.custom_spawners_tick();
        }
        // End of the chunk source: the blocks changed so far go to the players, before the
        // block events (what those change leaves on the next tick, like the game).
        if !Server::blocks_sent_at_tick_end() {
            self.broadcast_changed_blocks();
        }
        // 10. Block events.
        clock.count(|| self.run_block_events());
        self.bs.handling_tick = false;
        clock.publish();
    }

    /// Chunks whose blocks tick this tick (simulated, loaded, 8 neighbours loaded); `listed`
    /// false keeps the former test of every loaded chunk.
    pub(crate) fn ticking_chunks(&self, centers: &[(i32, i32)], listed: bool) -> TickingChunks {
        if !listed {
            let loaded: HashSet<(i32, i32)> =
                self.chunks.iter().filter(|(_, c)| c.is_some()).map(|(k, _)| *k).collect();
            let simulated = loaded.iter().copied().filter(|&c| self.chunk_simulated(c, centers)).collect();
            return TickingChunks::Sets { loaded, simulated };
        }
        TickingChunks::Sorted(self.ticking_chunks_in(&self.simulated_squares(centers), centers))
    }

    /// Ticking chunks among the cells of `squares`, in increasing order (the random ticks walk
    /// them in that order: same draws of the world LCG as the former sort).
    pub(crate) fn ticking_chunks_in(&self, squares: &[((i32, i32), i32)], centers: &[(i32, i32)]) -> Vec<(i32, i32)> {
        let area = squares.iter().map(|&(_, r)| square_cells(r)).fold(0, usize::saturating_add);
        // The squares cover a few hundred chunks around each source; thousands of forced chunks
        // or far-apart guests would cover more than the loaded chunks, which are then walked.
        let mut list: Vec<(i32, i32)> = if area > self.chunks.len() {
            self.chunks.iter().filter(|(_, c)| c.is_some()).map(|(k, _)| *k).collect()
        } else {
            let mut cells = Vec::with_capacity(area);
            for &((cx, cz), r) in squares {
                cells.extend((cx - r..=cx + r).flat_map(|x| (cz - r..=cz + r).map(move |z| (x, z))));
            }
            cells
        };
        list.sort_unstable();
        list.dedup();
        let in_table = |c: (i32, i32)| matches!(self.chunks.get(&c), Some(Some(_)));
        let grid = if BlockTickPaths::current().loaded_grid { LoadedGrid::around(&list, in_table) } else { None };
        let loaded = |c: (i32, i32)| grid.as_ref().map_or_else(|| in_table(c), |g| g.get(c));
        list.retain(|&c| self.chunk_simulated(c, centers) && chunk_and_neighbours(c).all(loaded));
        list
    }

    /// Step 7: due scheduled ticks of the blocks, then of the fluids, in the chunks of `ticking`.
    fn run_scheduled_ticks(&mut self, now: i64, ticking: &TickingChunks, by_candidates: bool) {
        self.bs.block_ticks.collect(now, 65536, ticking, by_candidates);
        while let Some(t) = self.bs.block_ticks.pop_next() {
            let s = self.get(t.pos);
            if u32::from(self.view().block(s)) == t.kind {
                self.tick_block(s, t.pos);
            }
        }
        self.bs.block_ticks.cleanup();
        self.bs.fluid_ticks.collect(now, 65536, ticking, by_candidates);
        while let Some(t) = self.bs.fluid_ticks.pop_next() {
            let s = self.get(t.pos);
            if u32::from(self.fluid_of(s).type_id()) == t.kind {
                self.fluid_tick(s, t.pos);
            }
        }
        self.bs.fluid_ticks.cleanup();
    }

    /// Step 9: random ticks of the ticking chunks that are also "ticking entities" (simulation
    /// distance, forced chunk, pearl or fight ticket, read after the purge), in increasing order.
    fn run_random_ticks(&mut self, ticking: &TickingChunks, centers: &[(i32, i32)]) {
        let speed = self.rule_int("minecraft:random_tick_speed", 3).max(0) as usize;
        for ch in self.random_tick_chunks(ticking, centers) {
            self.tick_chunk(ch, speed);
        }
    }

    /// Chunks of the random ticks, in increasing order.
    pub(crate) fn random_tick_chunks(&self, ticking: &TickingChunks, centers: &[(i32, i32)]) -> Vec<(i32, i32)> {
        match ticking {
            TickingChunks::Sets { loaded, .. } => {
                let mut list: Vec<(i32, i32)> = loaded
                    .iter()
                    .copied()
                    .filter(|&c| ticking.contains(c) && self.chunk_entity_ticking(c, centers))
                    .collect();
                list.sort_unstable();
                list
            }
            TickingChunks::Sorted(sorted) => {
                sorted.iter().copied().filter(|&c| self.chunk_entity_ticking(c, centers)).collect()
            }
        }
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
                if self.view().random_ticks(s) {
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
        if let Some(&b) = self.bs.random_ticking_sections.get(&(cx, cz, si)) {
            return b;
        }
        let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { return false };
        let Some(sec) = c.sections.get(si) else { return false };
        let v = self.view();
        let palette = BlockTickPaths::current().random_tick_palette;
        let b = section_has_random_ticks(sec, palette, |s| v.random_ticks(s) || self.fluid_of(s).kind == 2);
        self.bs.random_ticking_sections.insert((cx, cz, si), b);
        b
    }

    fn run_block_events(&mut self) {
        #[cfg(test)]
        crate::server::phase("events");
        let mut later = Vec::new();
        while let Some(e) = self.bs.events.pop_front() {
            self.bs.event_set.remove(&e);
            if self.ticking_at(e.pos) {
                let s = self.get(e.pos);
                if self.view().block(s) == e.block && self.trigger_event(s, e.pos, e.a, e.b) {
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

    /// Adds a ticking block entity of `family` (pending if the walk is in progress, § 1.7).
    pub(crate) fn add_ticker(&mut self, p: P, family: TickerFamily) -> u64 {
        let id = self.bs.next_be;
        self.bs.next_be += 1;
        if BlockTickPaths::current().ticker_compaction {
            self.bs.registered_tickers.insert((id, p));
        }
        let ticker = Ticker { id, pos: p, family };
        if self.bs.ticking_bes {
            self.bs.pending_tickers.push(ticker);
        } else {
            self.bs.tickers.push(ticker);
        }
        id
    }

    /// The ticker `id` made at `p` is still in the list (or waiting to join it).
    pub(crate) fn ticker_registered(&self, id: u64, p: P) -> bool {
        if BlockTickPaths::current().ticker_compaction {
            return self.bs.registered_tickers.contains(&(id, p));
        }
        self.bs.tickers.iter().chain(self.bs.pending_tickers.iter()).any(|t| (t.id, t.pos) == (id, p))
    }

    /// Ticker of the family `t` at `p` (§ 1.7), registered once: a re-read of its data keeps its
    /// place in the list (G3-F13: furnaces, brewing stands, bells, spawner blocks).
    pub(crate) fn ticker_register(&mut self, p: P, t: super::level::BeTick) {
        if self.bs.be_ticks.get(&p).is_some_and(|(_, o)| std::mem::discriminant(o) == std::mem::discriminant(&t)) {
            return;
        }
        let id = self.add_ticker(p, TickerFamily::Shared);
        self.bs.be_ticks.insert(p, (id, t));
    }

    /// State kept outside the block entity by a furnace, a brewing stand, a bell or a vault whose
    /// block entity is removed: forgotten with it.
    pub(crate) fn dead_ticker_state(&mut self, p: P, t: super::level::BeTick) {
        use super::level::BeTick;
        match t {
            BeTick::Furnace => {
                self.life.furnaces.remove(&p);
            }
            BeTick::Brewing => {
                self.life.brewers.remove(&p);
            }
            BeTick::Bell => self.mobs.bells.retain(|b| b.pos != p),
            BeTick::Spawner => self.spawner_block_forgotten(p),
            _ => {}
        }
    }

    /// Block entity phase (§ 1.2 step 11c, § 1.7), after all the entities: one list in
    /// insertion order for every family (a furnace placed before a hopper ticks before it).
    pub(crate) fn tick_block_entities(&mut self) {
        #[cfg(test)]
        crate::server::phase("block_entities");
        let paths = BlockTickPaths::current();
        self.bs.ticking_bes = true;
        let pending = std::mem::take(&mut self.bs.pending_tickers);
        self.bs.tickers.extend(pending);
        // Living players seen by the monster spawners, read once per walk (at the first one).
        let mut players = None;
        // `be_ticking` split: the border read once, the chunk test (9 table lookups) reused while
        // consecutive tickers stay in the same chunk (neither changes during the walk).
        let border = self.border_bounds();
        let mut last: Option<((i32, i32), bool)> = None;
        // Compacting path: the living tickers slide down to `kept` as the walk goes, a dead one
        // leaves a hole that no lookup matches, and the list is cut once at the end (the former
        // path removed each dead one at once, moving the whole tail each time). The list stays
        // in place: a ticker's tick may look for another ticker in it.
        let mut kept = 0;
        let mut i = 0;
        while i < self.bs.tickers.len() {
            let ticker = self.bs.tickers[i];
            let (id, p) = (ticker.id, ticker.pos);
            // Families of the shared map first: one lookup gives the family and its kept state.
            let shared = if paths.ticker_family && ticker.family != TickerFamily::Shared {
                None
            } else {
                self.bs.be_ticks.get(&p).filter(|e| e.0 == id).map(|e| e.1)
            };
            let alive = match shared {
                Some(t) => self.be_tick_alive(p, t),
                None if paths.ticker_family => self.family_ticker_alive(ticker),
                None => self.ticker_alive(id, p),
            };
            if !alive {
                if paths.ticker_compaction {
                    self.bs.tickers[i] = Ticker::GONE;
                    self.bs.registered_tickers.remove(&(id, p));
                    i += 1;
                } else {
                    self.bs.tickers.remove(i);
                }
                continue;
            }
            if paths.ticker_compaction {
                if kept < i {
                    self.bs.tickers[kept] = ticker;
                    self.bs.tickers[i] = Ticker::GONE;
                }
                kept += 1;
            }
            let c = (p[0] >> 4, p[2] >> 4);
            let chunk_ticks = match last {
                Some((k, v)) if k == c => v,
                _ => {
                    let v = self.ticking_at(p);
                    last = Some((c, v));
                    v
                }
            };
            if chunk_ticks && Self::block_in_border(border, p) {
                match shared {
                    Some(t) => self.be_tick_run(p, t, &mut players),
                    None if paths.ticker_family => self.family_ticker_tick(ticker),
                    None => self.tick_ticker(id, p),
                }
            }
            i += 1;
        }
        if paths.ticker_compaction {
            self.bs.tickers.truncate(kept);
        }
        self.bs.ticking_bes = false;
        self.station_menu_data(); // open furnace or brewing stand menu: its data after the tick
    }

    /// `ticker_alive` asking only the ticker's own family. A moving block at its place still
    /// decides first, as on the former path (a block entity pushed away leaves a mover there).
    fn family_ticker_alive(&mut self, t: Ticker) -> bool {
        let (id, p) = (t.id, t.pos);
        if t.family == TickerFamily::Shared {
            // Its kept state left the shared map.
            return false;
        }
        if let Some(m) = self.bs.movers.get(&p) {
            return m.id == id;
        }
        match t.family {
            TickerFamily::Shared | TickerFamily::Mover => false,
            TickerFamily::Heart => self.heart_ticker_alive(id, p),
            TickerFamily::Vibration => self.vib_block_ticker_alive(id, p),
            TickerFamily::Hive => self.hive_ticker_alive(id, p),
            TickerFamily::ShulkerLid => match self.shulker_ticker(id, p) {
                Some(alive) => {
                    if !alive {
                        self.bs.shulkers.remove(&p);
                    }
                    alive
                }
                None => false,
            },
            TickerFamily::Hopper => self.hopper_ticker_alive(id, p),
        }
    }

    /// `tick_ticker` for a living ticker of a family outside the shared map.
    fn family_ticker_tick(&mut self, t: Ticker) {
        let p = t.pos;
        match t.family {
            TickerFamily::Mover => {
                let s = self.get(p);
                if self.view().kind(s) == super::Kind::MovingPiston {
                    self.mover_tick(p);
                }
            }
            TickerFamily::Hive => self.hive_tick(p),
            TickerFamily::ShulkerLid => self.shulker_tick(p),
            TickerFamily::Heart => self.heart_tick(p),
            TickerFamily::Vibration => self.vib_tick_block(p),
            TickerFamily::Hopper => self.hopper_tick(p),
            TickerFamily::Shared => {}
        }
    }

    fn ticker_alive(&mut self, id: u64, p: P) -> bool {
        if let Some(m) = self.bs.movers.get(&p) {
            return m.id == id;
        }
        // F14: creaking hearts, shriekers.
        if let Some(alive) = self.rec_ticker_alive(id, p) {
            return alive;
        }
        // (The families of the shared map, G6-F01 and after, are tested by the walk itself.)
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

    /// Tick of a ticker of the shared map, with its kept state `t`.
    fn be_tick_run(&mut self, p: P, t: super::level::BeTick, players: &mut Option<Vec<[f64; 3]>>) {
        use super::level::BeTick;
        match t {
            // G6-F01: jukebox, campfires (tick skipped without food or started cooking).
            BeTick::Jukebox(song) => self.jukebox_tick(p, song),
            BeTick::Campfire { food, warm } => self.campfire_tick(p, food, warm),
            BeTick::Gateway => self.gateway_be_tick(p),         // G5-F10
            BeTick::Beacon(b) => self.beacon_tick(p, b),        // G5-F11
            BeTick::Conduit(c) => self.conduit_tick(p, c),      // G6-F12
            BeTick::PotentSulfur => self.potent_sulfur_tick(p), // G6-F09a
            // G3-F13: furnaces, brewing stands, bells and spawner blocks in the same list.
            BeTick::Furnace => self.furnace_tick(p),
            BeTick::Brewing => self.brewer_tick(p),
            BeTick::Bell => self.bell_tick(p),
            BeTick::Spawner => {
                let players = players.get_or_insert_with(|| self.alive_players());
                self.spawner_block_tick(p, players);
            }
        }
    }

    fn tick_ticker(&mut self, id: u64, p: P) {
        if self.bs.movers.get(&p).is_some_and(|m| m.id == id) {
            let s = self.get(p);
            if self.view().kind(s) == super::Kind::MovingPiston {
                self.mover_tick(p);
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
        // L9-TICK: marked once the chunk is there. Before, a chunk missing from the disk was
        // marked when it was put into generation, so the generated chunk never resumed its ticks
        // nor registered its block entities' tickers (a dungeon's spawner, a village's bell).
        let Some(Some(c)) = self.chunks.get(&ch) else { return };
        if !self.bs.imported.insert(ch) {
            return;
        }
        let kind_of_block = |name: &str| Some(self.bs.t.block_id(name)).filter(|&b| b != u16::MAX).map(u32::from);
        let kind_of_fluid = |name: &str| FLUID_NAMES.iter().position(|n| *n == name).map(|k| k as u32);
        let blocks = saved_ticks(c.root.get("block_ticks"), ch, kind_of_block);
        let fluids = saved_ticks(c.root.get("fluid_ticks"), ch, kind_of_fluid);
        let now = self.time;
        resume_saved_ticks(&mut self.bs.block_ticks, &blocks, now);
        resume_saved_ticks(&mut self.bs.fluid_ticks, &fluids, now);
        self.register_chunk_tickers(ch);
        self.import_movers(ch);
    }

    /// Writes the pending ticks of a chunk into its compound (before the save).
    pub(crate) fn export_chunk_ticks(&mut self, ch: (i32, i32)) {
        let now = self.time;
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
        // One sorting buffer for both queues; block names borrowed from the data (they were
        // copied, all 1,286 of them, for every chunk saved).
        let mut sorted = Vec::new();
        self.bs.block_ticks.chunk_ticks_into(ch, &mut sorted);
        let block_name = |kind: u32| self.data.blocks.get(kind as usize).map(|b| b.name.as_str());
        let blocks: Vec<Nbt> = sorted.iter().filter_map(|t| block_name(t.kind).map(|n| enc(t, n))).collect();
        self.bs.fluid_ticks.chunk_ticks_into(ch, &mut sorted);
        let fluids: Vec<Nbt> =
            sorted.iter().filter_map(|t| FLUID_NAMES.get(t.kind as usize).map(|n| enc(t, n))).collect();
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
            let id = self.add_ticker(p, TickerFamily::Mover);
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
