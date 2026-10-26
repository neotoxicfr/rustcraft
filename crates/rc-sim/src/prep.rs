//! Preparation of the singleplayer world as seen by the loading screen: weighted progress of the
//! integrated server's stages and chunk statuses around the tracked point (screen map), like the game.
//!
//! Shared between the server thread (and the generation one), which write, and the client, which
//! reads every frame: atomics, no lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};

/// Radius of the map: 5 (at least 3) + radius of the dependencies of a full chunk (11) + 1.
pub const RADIUS: i32 = 17;
/// Side of the map, in chunks.
pub const SIDE: usize = (2 * RADIUS + 1) as usize;
/// Distance (Chebyshev) of the chunks pulled by a full chunk: beyond it, no status.
const REACH: i32 = 11;
/// Player chunks awaited by the progress: 7 × 7.
const PLAYER_CHUNKS: i32 = 49;
/// Radius of the player's square (7 × 7).
const PLAYER_RADIUS: i32 = 3;
/// Weight of the server preparation.
const PREPARE_WEIGHT: i32 = 10;

/// Status of a chunk, in the game's generation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Status {
    Empty,
    StructureStarts,
    StructureReferences,
    Biomes,
    Terrain,
    Features,
    InitializeLight,
    Light,
    Spawn,
    Full,
}

impl Status {
    const ALL: [Status; 10] = [
        Status::Empty,
        Status::StructureStarts,
        Status::StructureReferences,
        Status::Biomes,
        Status::Terrain,
        Status::Features,
        Status::InitializeLight,
        Status::Light,
        Status::Spawn,
        Status::Full,
    ];

    /// Status of a chunk pulled by a full chunk at `d` chunks (cumulated dependencies of the full
    /// status: light at 1, decorations at 1 from it, terrain, biomes, then structure starts at
    /// 8 from the biomes).
    fn around_full(d: i32) -> Option<Status> {
        match d {
            0 => Some(Status::Full),
            1 => Some(Status::InitializeLight),
            2 => Some(Status::Terrain),
            3 => Some(Status::Biomes),
            4..=REACH => Some(Status::StructureStarts),
            _ => None,
        }
    }
}

/// Stage tracked by the progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Chunks kept loaded by the world (registered tickets).
    Saved,
    /// Chunks around the player.
    AroundPlayer,
}

/// Weighted progress: preparation (10), initial chunks, 49 player chunks; a stage
/// in progress counts for its fraction of chunks ready.
#[derive(Clone, Debug, Default)]
pub struct Weights {
    total: i32,
    done: i32,
    segment: i32,
    fraction: f32,
}

impl Weights {
    pub fn start(&mut self, stage: Stage, total_chunks: i32) {
        match stage {
            Stage::Saved => {
                self.total = PREPARE_WEIGHT + total_chunks + PLAYER_CHUNKS;
                self.begin(PREPARE_WEIGHT);
                self.finish();
                self.begin(total_chunks);
            }
            Stage::AroundPlayer => self.begin(PLAYER_CHUNKS),
        }
    }

    fn begin(&mut self, weight: i32) {
        self.segment = weight;
        self.fraction = 0.0;
    }

    pub fn update(&mut self, current: i32, total: i32) {
        self.fraction = if total == 0 { 0.0 } else { current as f32 / total as f32 };
    }

    pub fn finish(&mut self) {
        self.done += self.segment;
        self.segment = 0;
    }

    pub fn get(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.done as f32 + self.fraction * self.segment as f32) / self.total as f32 }
    }
}

/// Shared state read by the loading screen.
pub struct Prep {
    /// 0: no status; otherwise rank of the status + 1. Line by line (z), `SIDE` × `SIDE`.
    cells: Box<[AtomicU8]>,
    center: [AtomicI32; 2],
    focused: AtomicBool,
    /// A stage has started (bar displayed).
    started: AtomicBool,
    progress: AtomicU32,
}

impl Default for Prep {
    fn default() -> Prep {
        Prep {
            cells: (0..SIDE * SIDE).map(|_| AtomicU8::new(0)).collect(),
            center: [AtomicI32::new(0), AtomicI32::new(0)],
            focused: AtomicBool::new(false),
            started: AtomicBool::new(false),
            progress: AtomicU32::new(0),
        }
    }
}

impl Prep {
    /// Status of the cell (`x`, `z`) of the map (0 ≤ x, z < `SIDE`), `None` without status.
    pub fn status(&self, x: usize, z: usize) -> Option<Status> {
        let v = self.cells.get(z * SIDE + x)?.load(Ordering::Relaxed);
        v.checked_sub(1).and_then(|i| Status::ALL.get(usize::from(i)).copied())
    }

    /// A tracked stage has started: the bar is displayed.
    pub fn has_progress(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// Server progress (0 to 1).
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Acquire))
    }

    fn set_progress(&self, w: &Weights) {
        self.progress.store(w.get().to_bits(), Ordering::Release);
        self.started.store(true, Ordering::Release);
    }

    /// New tracked point: map emptied.
    fn focus(&self, (x, z): (i32, i32)) {
        for c in &*self.cells {
            c.store(0, Ordering::Relaxed);
        }
        self.center[0].store(x, Ordering::Relaxed);
        self.center[1].store(z, Ordering::Relaxed);
        self.focused.store(true, Ordering::Release);
    }

    fn index(&self, (x, z): (i32, i32)) -> Option<usize> {
        if !self.focused.load(Ordering::Acquire) {
            return None;
        }
        let dx = x - self.center[0].load(Ordering::Relaxed) + RADIUS;
        let dz = z - self.center[1].load(Ordering::Relaxed) + RADIUS;
        let side = SIDE as i32;
        ((0..side).contains(&dx) && (0..side).contains(&dz)).then(|| (dz * side + dx) as usize)
    }

    /// Status reached by chunk `p` (statuses only go up).
    pub(crate) fn raise(&self, p: (i32, i32), s: Status) {
        if let Some(i) = self.index(p) {
            self.cells[i].fetch_max(s as u8 + 1, Ordering::Relaxed);
        }
    }
}

/// Side of the computation grid: map plus the chunks that can pull it from outside.
const WIDE: usize = SIDE + 2 * REACH as usize;

/// Chebyshev distances from each cell of the enlarged grid to the nearest full chunk
/// (`i32::MAX / 2`: none); two passes, diagonal neighbours counted as 1.
fn chebyshev(full: &[bool]) -> Vec<i32> {
    const FAR: i32 = i32::MAX / 2;
    let n = WIDE;
    let mut d: Vec<i32> = full.iter().map(|&f| if f { 0 } else { FAR }).collect();
    for z in 0..n {
        for x in 0..n {
            let mut v = d[z * n + x];
            if x > 0 {
                v = v.min(d[z * n + x - 1] + 1);
            }
            if z > 0 {
                v = v.min(d[(z - 1) * n + x] + 1);
                if x > 0 {
                    v = v.min(d[(z - 1) * n + x - 1] + 1);
                }
                if x + 1 < n {
                    v = v.min(d[(z - 1) * n + x + 1] + 1);
                }
            }
            d[z * n + x] = v;
        }
    }
    for z in (0..n).rev() {
        for x in (0..n).rev() {
            let mut v = d[z * n + x];
            if x + 1 < n {
                v = v.min(d[z * n + x + 1] + 1);
            }
            if z + 1 < n {
                v = v.min(d[(z + 1) * n + x] + 1);
                if x + 1 < n {
                    v = v.min(d[(z + 1) * n + x + 1] + 1);
                }
                if x > 0 {
                    v = v.min(d[(z + 1) * n + x - 1] + 1);
                }
            }
            d[z * n + x] = v;
        }
    }
    d
}

/// Statuses pulled by the full chunks (`is_full`) around `center`, placed on the map.
fn raise_rings(prep: &Prep, center: (i32, i32), is_full: impl Fn((i32, i32)) -> bool) {
    let off = RADIUS + REACH;
    let full: Vec<bool> = (0..WIDE * WIDE)
        .map(|i| is_full((center.0 - off + (i % WIDE) as i32, center.1 - off + (i / WIDE) as i32)))
        .collect();
    let d = chebyshev(&full);
    let r = REACH as usize;
    for z in 0..SIDE {
        for x in 0..SIDE {
            if let Some(s) = Status::around_full(d[(z + r) * WIDE + x + r]) {
                let p = (center.0 - RADIUS + x as i32, center.1 - RADIUS + z as i32);
                prep.raise(p, s);
            }
        }
    }
}

/// Server-side tracking, until the player enters the world.
pub(crate) struct Track {
    pub(crate) shared: Arc<Prep>,
    weights: Weights,
    center: (i32, i32),
    /// Player-square chunks not yet ready, and their number at the start of the stage.
    waiting: Vec<(i32, i32)>,
    total: i32,
    /// Fingerprint of the chunk state at the last map computation.
    seen: (usize, usize),
}

impl crate::server::Server {
    /// Open world (levels created): tracked point = player's chunk, stage of the initial chunks, then
    /// that of the player's 7 × 7 square (chunks not yet loaded); map
    /// passed to the generation.
    /// Known DIFFERENCE: the game counts in the initial stage the chunks awakened by the saved tickets of the
    /// world (forced loading, portals), all dimensions; our server does not
    /// load those chunks at startup, so the stage stays empty (bar starting at 10/59 instead
    /// of 10/(59 + N) on a world with `/forceload`).
    pub(crate) fn prep_open(&mut self, shared: Arc<Prep>) {
        let center = self.chunk_pos();
        shared.focus(center);
        let mut weights = Weights::default();
        weights.start(Stage::Saved, 0);
        weights.update(0, 0);
        weights.finish();
        let (cx, cz) = center;
        let r = PLAYER_RADIUS;
        let waiting: Vec<(i32, i32)> = (cz - r..=cz + r)
            .flat_map(|z| (cx - r..=cx + r).map(move |x| (x, z)))
            .filter(|&p| !self.chunk_ready(p))
            .collect();
        let total = waiting.len() as i32;
        weights.start(Stage::AroundPlayer, total);
        weights.update(0, total);
        if waiting.is_empty() {
            weights.finish();
        }
        shared.set_progress(&weights);
        if let Some(g) = &self.generator {
            let _ = g.prep.set(shared.clone());
        }
        self.prep = Some(Box::new(Track { shared, weights, center, waiting, total, seen: (usize::MAX, 0) }));
    }

    fn chunk_ready(&self, p: (i32, i32)) -> bool {
        self.chunks.contains_key(&p) && !self.generator.as_ref().is_some_and(|g| g.pending.contains(&p))
    }

    /// Tick: progress of the player's chunks, map redone if chunks arrived;
    /// tracking abandoned once the player has entered (loading screen closed), or after the maximum
    /// delay of that screen (30 s).
    pub(crate) fn prep_tick(&mut self) {
        let Some(mut t) = self.prep.take() else { return };
        if self.loaded || self.tick >= self.joined_tick + 600 {
            return;
        }
        if !t.waiting.is_empty() {
            // The 7 × 7 square is loaded whatever the view distance (the game prepares it
            // before entry): what the view does not request is requested here.
            let missing: Vec<(i32, i32)> = t.waiting.iter().copied().filter(|p| !self.chunks.contains_key(p)).collect();
            if !missing.is_empty() {
                self.ensure_loaded(&missing);
            }
            t.waiting.retain(|&p| !self.chunk_ready(p));
            t.weights.update(t.total - t.waiting.len() as i32, t.total);
            if t.waiting.is_empty() {
                t.weights.finish();
            }
            t.shared.set_progress(&t.weights);
        }
        let seen = (self.chunks.len(), self.generator.as_ref().map_or(0, |g| g.pending.len()));
        if seen != t.seen {
            t.seen = seen;
            if let Some(g) = &self.generator {
                for &p in &g.pending {
                    t.shared.raise(p, Status::Empty);
                }
            }
            raise_rings(&t.shared, t.center, |p| matches!(self.chunks.get(&p), Some(Some(_))));
        }
        self.prep = Some(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Game weights: 10 of preparation out of 59, then the player's 49 chunks; an old code
    /// without progress returned 0 everywhere.
    #[test]
    fn weights_follow_the_game() {
        let mut w = Weights::default();
        assert_eq!(w.get(), 0.0);
        w.start(Stage::Saved, 0);
        w.update(0, 0);
        w.finish();
        assert!((w.get() - 10.0 / 59.0).abs() < 1e-6);
        w.start(Stage::AroundPlayer, 40);
        w.update(20, 40);
        assert!((w.get() - (10.0 + 24.5) / 59.0).abs() < 1e-6);
        w.finish();
        assert!((w.get() - 1.0).abs() < 1e-6);
        // Initial chunks counted in the total.
        let mut w = Weights::default();
        w.start(Stage::Saved, 41);
        w.update(41, 41);
        assert!((w.get() - 51.0 / 100.0).abs() < 1e-6);
    }

    /// Rings around a full 7 × 7 square: light, terrain, biomes, structure starts
    /// up to 11, nothing beyond (black edge of the map).
    #[test]
    fn rings_around_the_player_square() {
        let p = Prep::default();
        p.focus((100, -40));
        raise_rings(&p, (100, -40), |(x, z)| (x - 100).abs() <= 3 && (z + 40).abs() <= 3);
        let at = |dx: i32| p.status((RADIUS + dx) as usize, RADIUS as usize);
        assert_eq!(at(0), Some(Status::Full));
        assert_eq!(at(3), Some(Status::Full));
        assert_eq!(at(4), Some(Status::InitializeLight));
        assert_eq!(at(5), Some(Status::Terrain));
        assert_eq!(at(6), Some(Status::Biomes));
        assert_eq!(at(7), Some(Status::StructureStarts));
        assert_eq!(at(14), Some(Status::StructureStarts));
        assert_eq!(at(15), None);
        assert_eq!(at(-17), None);
        // Diagonal: Chebyshev distance.
        assert_eq!(p.status((RADIUS + 5) as usize, (RADIUS + 5) as usize), Some(Status::Terrain));
        // A full chunk outside the map still pulls its neighbours inside.
        let q = Prep::default();
        q.focus((0, 0));
        raise_rings(&q, (0, 0), |p| p == (RADIUS + 2, 0));
        assert_eq!(q.status(SIDE - 1, RADIUS as usize), Some(Status::Terrain));
        // Statuses do not go back down; outside the map ignored.
        q.raise((RADIUS, 0), Status::Empty);
        assert_eq!(q.status(SIDE - 1, RADIUS as usize), Some(Status::Terrain));
        q.raise((RADIUS + 1, 0), Status::Full);
    }

    /// Real walk: new world opened by the integrated server, player never "loaded"; the
    /// preparation ends full, the player's chunk full, the corners of the map empty.
    #[test]
    fn new_world_preparation_is_reported() {
        use std::time::{Duration, Instant};
        let Some(data) = rc_testdata::version_data() else { return };
        let data = Arc::new(data);
        if crate::registries::client_jar(&data).is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rc-prep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = crate::newworld::NewWorld::new("prep");
        w.seed = 7;
        crate::newworld::create_world(&dir, &data, &w).expect("creation");
        let opts = crate::Options {
            world: dir.clone(),
            name: "AgentPrep".into(),
            uuid: rc_client::offline_uuid("AgentPrep"),
            info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
            sim_distance: 2,
        };
        crate::tests::solo_slot();
        let (session, handle) = crate::start(opts, data).expect("solo");
        let prep = handle.prep();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(25) && prep.progress() < 1.0 {
            if let Ok(rc_client::Event::Disconnected(r)) = session.events.recv_timeout(Duration::from_millis(50)) {
                panic!("{r}");
            }
        }
        assert!(prep.has_progress());
        assert!((prep.progress() - 1.0).abs() < 1e-6, "progress {}", prep.progress());
        let c = RADIUS as usize;
        assert_eq!(prep.status(c, c), Some(Status::Full));
        assert_eq!(prep.status(0, 0), None);
        assert_eq!(prep.status(SIDE - 1, SIDE - 1), None);
        session.link.disconnect();
        handle.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exact distances against a direct computation on a random grid (fixed seed).
    #[test]
    fn chebyshev_matches_brute_force() {
        let mut s = crate::rng::test_seed() as u64 | 1;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let full: Vec<bool> = (0..WIDE * WIDE).map(|_| next() % 97 == 0).collect();
        let d = chebyshev(&full);
        for i in (0..WIDE * WIDE).step_by(7) {
            let (x, z) = ((i % WIDE) as i32, (i / WIDE) as i32);
            let best = (0..WIDE * WIDE)
                .filter(|&j| full[j])
                .map(|j| ((j % WIDE) as i32 - x).abs().max(((j / WIDE) as i32 - z).abs()))
                .min()
                .unwrap_or(i32::MAX / 2);
            assert_eq!(d[i], best, "cell {i}");
        }
    }
}
