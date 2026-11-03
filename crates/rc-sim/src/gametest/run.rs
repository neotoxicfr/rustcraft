//! Runner of the in-game tests, in the game's order: the batches of a run (one environment and
//! one dimension each, at most 50 tests) are laid out then started one after the other; each test
//! waits for its setup ticks, runs, then passes or fails (time limit), its report goes to every
//! player and its block shows the result; retries and flaky tests run again at the end, in place.
//! The runner ticks after the world and the players, while the game runs normally.

// Failures carry text components (rare: once per failing test).
#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::time::Instant;

use rc_proto::nbt::Nbt;
use rc_text::Component;
use rc_worldgen::template::{Mirror, Rot, transform};

use super::{Data, EnvDef, EnvRef, Kind, TestDef, Weather};
use crate::blocks::{Kind as BKind, P, flag};
use crate::commands::Src;
use crate::commands::ents::E;
use crate::commands::src::KeptSource;
use crate::commands::text::{GOLD, GREEN, RED, YELLOW, brackets, colored, hover_text, lit, suggest_click};
use crate::server::Server;
use crate::tr;

/// Tries of a run: fewer than 1 = unlimited; `halt`: stop at the first failure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Retry {
    pub tries: i32,
    pub halt: bool,
}

impl Retry {
    pub(crate) const NONE: Retry = Retry { tries: 1, halt: true };

    fn unlimited(self) -> bool {
        self.tries < 1
    }

    fn tries_left(self, attempts: i32, successes: i32) -> bool {
        let failures = attempts != successes;
        (self.unlimited() || attempts < self.tries) && (!failures || !self.halt)
    }

    fn has_retries(self) -> bool {
        self.tries != 1
    }
}

/// Failure of a test, like the game's exceptions: an assertion (at a block or not), a time
/// limit, an internal error (unknown function); `Exhausted` wraps the failure of the last try of
/// a flaky test.
#[derive(Debug, Clone)]
pub(crate) enum TestError {
    Assert { msg: Component, tick: i32 },
    AssertAt { msg: Component, abs: P, rel: P, tick: i32 },
    Timeout(Component),
    Unknown(String),
    Exhausted { text: String, cause: Box<TestError> },
}

impl TestError {
    /// Description of an assertion (kept by the block).
    fn assert_description(&self) -> Option<Component> {
        match self {
            TestError::Assert { msg, tick } => Some(tr!("test.error.tick", msg.clone(), *tick)),
            TestError::AssertAt { msg, abs, rel, tick } => Some(tr!(
                "test.error.position",
                msg.clone(),
                abs[0],
                abs[1],
                abs[2],
                rel[0],
                rel[1],
                rel[2],
                *tick
            )),
            _ => None,
        }
    }

    /// Text of the failure (server language).
    fn message(&self) -> String {
        match self {
            TestError::Assert { .. } | TestError::AssertAt { .. } => {
                self.assert_description().map(|c| crate::commands::cmds_ents::plain(&c)).unwrap_or_default()
            }
            TestError::Timeout(c) => crate::commands::cmds_ents::plain(c),
            TestError::Unknown(s) => s.clone(),
            TestError::Exhausted { text, .. } => text.clone(),
        }
    }

    /// Innermost cause's text (the game's error description walks the causes).
    fn deepest_message(&self) -> String {
        match self {
            TestError::Exhausted { cause, .. } => cause.deepest_message(),
            e => e.message(),
        }
    }

    /// Error kept by the test instance block.
    fn block_text(&self) -> Component {
        self.assert_description().unwrap_or_else(|| lit(self.deepest_message()))
    }

    /// Marker at the block of a positioned assertion (innermost cause).
    fn marker(&self) -> Option<(P, Component)> {
        match self {
            TestError::AssertAt { msg, abs, .. } => Some((*abs, msg.clone())),
            TestError::Exhausted { cause, .. } => cause.marker(),
            _ => None,
        }
    }
}

/// A test of a run (the game's test info).
pub(crate) struct Info {
    pub(crate) test: Arc<TestDef>,
    pub(crate) pos: Option<P>,
    pub(crate) dim: String,
    retry: Retry,
    extra_rot: Rot,
    placed: bool,
    chunks_loaded: bool,
    tick: i32,
    started: bool,
    done: bool,
    started_at: Option<Instant>,
    run_ms: u128,
    error: Option<TestError>,
    /// Test instance block known (the last data seen stays usable if the block disappears).
    be: Option<Data>,
    /// Block-based tests: their check runs on every tick of `[from, to)`, `next` the first not run.
    each_tick: Option<(i32, i32)>,
    /// Report shared with the reruns of the same test: (attempts, successes).
    report: usize,
    /// Tracker of the batch the test was laid out in.
    batch: Option<usize>,
}

impl Info {
    fn new(test: Arc<TestDef>, extra_rot: Rot, dim: String, retry: Retry, report: usize) -> Info {
        Info {
            test,
            pos: None,
            dim,
            retry,
            extra_rot,
            placed: false,
            chunks_loaded: false,
            tick: 0,
            started: false,
            done: false,
            started_at: None,
            run_ms: 0,
            error: None,
            be: None,
            each_tick: None,
            report,
            batch: None,
        }
    }

    fn rotation(&self) -> Rot {
        self.test.rotation.then(self.extra_rot)
    }

    fn fail(&mut self, e: TestError) {
        self.error = Some(e);
    }

    fn fail_msg(&mut self, msg: Component) {
        let tick = self.tick;
        self.fail(TestError::Assert { msg, tick });
    }

    fn finish(&mut self) {
        if !self.done {
            self.done = true;
            if let Some(t) = self.started_at.take() {
                self.run_ms = t.elapsed().as_millis();
            }
        }
    }

    fn run_time(&self) -> u128 {
        self.started_at.map_or(self.run_ms, |t| t.elapsed().as_millis())
    }

    fn start_execution(&mut self, delay: i32) {
        self.tick = -(self.test.setup_ticks + delay + 1);
    }
}

/// Environment set up for the current batch, with what its teardown restores.
struct Activation {
    def: Arc<EnvDef>,
    dim: String,
    saved: Saved,
}

enum Saved {
    All(Vec<Activation>),
    Clock(String, i64),
    Difficulty(i32),
    Rules(Vec<(String, Option<Nbt>)>),
    Weather(Weather),
    Nothing,
}

struct Batch {
    index: i32,
    infos: Vec<usize>,
    env: EnvRef,
    dim: String,
}

/// Layout of the new structures: rows of `per_row` tests, 5 blocks between the columns and 6
/// between the rows, from 3 blocks south of the source (surface).
struct Grid {
    per_row: i32,
    clear_on_batch: bool,
    dims: Vec<(String, DimGrid)>,
}

struct DimGrid {
    first: P,
    next: P,
    row: ([f64; 3], [f64; 3]),
    count: i32,
    last_batch: Vec<usize>,
}

fn unit_box(p: P) -> ([f64; 3], [f64; 3]) {
    let a = p.map(f64::from);
    (a, [a[0] + 1.0, a[1] + 1.0, a[2] + 1.0])
}

/// Who receives the reports meant for the source of the run.
pub(crate) type RunSource = KeptSource;

pub(crate) struct Runner {
    pub(crate) infos: Vec<Info>,
    batches: Vec<Batch>,
    trackers: Vec<(i32, Vec<usize>)>,
    scheduled: Vec<usize>,
    stopped: bool,
    env: Option<Activation>,
    grid: Option<Grid>,
    /// The tests already placed are laid out by the grid too (`verify`), otherwise rebuilt in place.
    existing_grid: bool,
    halt_on_error: bool,
    clear_between: bool,
    batch_size: usize,
    reports: Vec<(i32, i32)>,
    source: RunSource,
    summary_dims: Vec<String>,
    /// Tests being ticked, in order of start.
    ticked: Vec<usize>,
}

#[cfg(test)]
impl Runner {
    /// Tests still ticking.
    pub(crate) fn ticked_len(&self) -> usize {
        self.ticked.len()
    }
}

/// A test to run: definition, rotation added, dimension, tries, test instance block if it exists.
pub(crate) struct Planned {
    pub test: Arc<TestDef>,
    pub rot: Rot,
    pub dim: String,
    pub retry: Retry,
    pub pos: Option<P>,
}

/// Batch given to `verify`: its tests, environment, index (the rotation) and dimension.
pub(crate) type VerifyBatch = (Vec<usize>, EnvRef, i32, String);

/// Settings of a run.
pub(crate) struct RunSpec {
    pub per_row: i32,
    pub clear_on_batch: bool,
    /// `verify`: given batches, laid out by the grid even when placed, stop at the first failure,
    /// cleared between batches, 100 tests per batch.
    pub verify: Option<Vec<VerifyBatch>>,
}

/// Geometry of a test instance block: position, data, effective rotation, padding, sky.
#[derive(Clone)]
pub(crate) struct Geo {
    pub(crate) pos: P,
    pub(crate) data: Data,
    pub(crate) rot: Rot,
    pub(crate) padding: i32,
    pub(crate) sky: bool,
}

impl Geo {
    fn tsize(&self) -> [i32; 3] {
        let s = self.data.size;
        if matches!(self.rot, Rot::Cw90 | Rot::Ccw90) { [s[2], s[1], s[0]] } else { s }
    }

    pub(crate) fn structure_pos(&self) -> P {
        let d = self.padding;
        [self.pos[0] + d, self.pos[1] + d + 1, self.pos[2] + d + 1]
    }

    /// Box of the structure (corners ordered like the game's, an empty size spans two cells).
    pub(crate) fn structure_box(&self) -> (P, P) {
        let a = self.structure_pos();
        let t = self.tsize();
        let b = [a[0] + t[0] - 1, a[1] + t[1] - 1, a[2] + t[2] - 1];
        ([a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])], [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])])
    }

    pub(crate) fn test_box(&self) -> (P, P) {
        let (lo, hi) = self.structure_box();
        let d = self.padding;
        ([lo[0] - d, lo[1] - d, lo[2] - d], [hi[0] + d, hi[1] + d, hi[2] + d])
    }

    fn structure_aabb(&self) -> ([f64; 3], [f64; 3]) {
        aabb_of(self.structure_box())
    }

    fn test_aabb(&self) -> ([f64; 3], [f64; 3]) {
        let (lo, hi) = self.structure_aabb();
        let d = f64::from(self.padding);
        ([lo[0] - d, lo[1] - d, lo[2] - d], [hi[0] + d, hi[1] + d, hi[2] + d])
    }

    fn start_corner(&self) -> P {
        let s = self.data.size;
        let c = self.structure_pos();
        match self.rot {
            Rot::None => c,
            Rot::Cw90 => [c[0] + s[2] - 1, c[1], c[2]],
            Rot::Cw180 => [c[0] + s[0] - 1, c[1], c[2] + s[2] - 1],
            Rot::Ccw90 => [c[0], c[1], c[2] + s[0] - 1],
        }
    }
}

fn aabb_of((lo, hi): (P, P)) -> ([f64; 3], [f64; 3]) {
    (lo.map(f64::from), [f64::from(hi[0]) + 1.0, f64::from(hi[1]) + 1.0, f64::from(hi[2]) + 1.0])
}

/// Chunks crossed by a box.
fn chunks_of((lo, hi): (P, P)) -> Vec<(i32, i32)> {
    let mut v = Vec::new();
    for cz in (lo[2] >> 4)..=(hi[2] >> 4) {
        for cx in (lo[0] >> 4)..=(hi[0] >> 4) {
            v.push((cx, cz));
        }
    }
    v
}

/// Mode of a test block from its state.
const MODES: [&str; 4] = ["start", "log", "fail", "accept"];

impl Server {
    // ---------- Test instance blocks ----------

    /// Data of the test instance block at `p`, if it is one.
    pub(crate) fn ti_data(&self, p: P) -> Option<Data> {
        let be = self.be_get(p)?;
        (be.get("id").and_then(Nbt::as_str) == Some("minecraft:test_instance_block")).then(|| Data::read(be))
    }

    fn geo(&self, p: P, data: Data) -> Geo {
        let test = data.test.as_deref().and_then(|t| self.cmd.tests.test(t)).cloned();
        let rot = test.as_ref().map_or(Rot::None, |t| t.rotation).then(data.rot);
        Geo {
            pos: p,
            rot,
            padding: test.as_ref().map_or(0, |t| t.padding),
            sky: test.as_ref().is_some_and(|t| t.sky_access),
            data,
        }
    }

    pub(crate) fn ti_geo(&self, p: P) -> Option<Geo> {
        self.ti_data(p).map(|d| self.geo(p, d))
    }

    /// New data of a test instance block, sent to the clients.
    pub(crate) fn ti_set(&mut self, p: P, d: &Data) {
        if self.ti_data(p).is_none() {
            return;
        }
        if let Some(be) = self.be_get_mut(p) {
            d.write(be);
        }
        self.send_block_entity(p);
    }

    /// Box of the structure of the test instance block at `p`.
    pub(crate) fn ti_structure_box(&self, p: P) -> Option<(P, P)> {
        self.ti_geo(p).map(|g| g.structure_box())
    }

    /// Size of a test's structure (its template).
    pub(crate) fn test_structure_size(&mut self, test: &str) -> Option<[i32; 3]> {
        let structure = self.cmd.tests.test(test)?.structure.clone();
        self.template_get(&structure).map(|t| t.t.size)
    }

    fn ti_markers_clear(&mut self, p: P) {
        let has = self.be_get(p).is_some_and(|b| crate::nbtio::list(b.get("errors")).iter().next().is_some());
        if has {
            if let Some(be) = self.be_get_mut(p) {
                crate::nbtio::remove(be, "errors");
            }
            self.send_block_entity(p);
        }
    }

    fn ti_mark_error(&mut self, p: P, at: P, text: &Component) {
        if self.ti_data(p).is_none() {
            return;
        }
        if let Some(be) = self.be_get_mut(p) {
            let mut list = crate::nbtio::list(be.get("errors")).to_vec();
            list.push(Nbt::Compound(vec![
                ("pos".into(), Nbt::IntArray(at.to_vec())),
                ("text".into(), crate::commands::text::to_nbt(text)),
            ]));
            crate::nbtio::set(be, "errors", Nbt::List(list));
        }
        self.send_block_entity(p);
    }

    /// Cells of the box around the structure: the walls and the floor, the ceiling unless the
    /// test opens to the sky; in the game's order (x, then y, then z).
    fn ti_boundary(&self, g: &Geo) -> Vec<P> {
        let (lo, hi) = g.structure_box();
        let (low, high) = ([lo[0] - 1, lo[1] - 1, lo[2] - 1], [hi[0] + 1, hi[1] + 1, hi[2] + 1]);
        let ceiling = !g.sky;
        let mut v = Vec::new();
        for z in low[2]..=high[2] {
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    let edge = x == low[0] || x == high[0] || z == low[2] || z == high[2] || y == low[1];
                    if edge || (y == high[1] && ceiling) {
                        v.push([x, y, z]);
                    }
                }
            }
        }
        v
    }

    fn ti_encase(&mut self, g: &Geo) {
        let barrier = self.default_state("minecraft:barrier");
        for q in self.ti_boundary(g) {
            if self.view().kind(self.get(q)) != BKind::TestInstance {
                self.set_block(q, barrier, flag::ALL);
            }
        }
    }

    pub(crate) fn ti_remove_barriers(&mut self, p: P) {
        let Some(g) = self.ti_geo(p) else { return };
        self.ti_remove_barriers_of(&g);
    }

    fn ti_remove_barriers_of(&mut self, g: &Geo) {
        let barrier = self.table().block_id("minecraft:barrier");
        let air = self.air();
        for q in self.ti_boundary(g) {
            if self.view().block(self.get(q)) == barrier {
                self.set_block(q, air, flag::ALL);
            }
        }
    }

    /// Room for a structure: every cell of the box becomes air (the game's stone below the floor
    /// never applies, its floor is the box's own bottom), neighbours warned; the pending ticks
    /// and block events of the box are dropped, its entities other than players removed.
    pub(crate) fn clear_space(&mut self, (lo, hi): (P, P)) {
        self.load_now(&chunks_of((lo, hi)));
        let air = self.air();
        let air_block = self.view().block(air);
        let flags = flag::CLIENTS | flag::KNOWN_SHAPE | flag::NO_DROPS | flag::SKIP_BE_SIDE_EFFECTS | flag::SKIP_ON_PLACE;
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let q = [x, y, z];
                    self.set_block(q, air, flags);
                    self.update_neighbors_at(q, air_block);
                }
            }
        }
        self.bs.block_ticks.clear_box(lo, hi);
        self.clear_block_events(lo, hi);
        self.discard_entities_in(aabb_of((lo, hi)));
    }

    fn clear_block_events(&mut self, lo: P, hi: P) {
        let inside = |p: &P| (0..3).all(|i| p[i] >= lo[i] && p[i] <= hi[i]);
        let gone: Vec<_> = self.bs.events.iter().filter(|e| inside(&e.pos)).copied().collect();
        if gone.is_empty() {
            return;
        }
        self.bs.events.retain(|e| !inside(&e.pos));
        for e in gone {
            self.bs.event_set.remove(&e);
        }
    }

    /// Entities other than players whose box meets `b`, removed without dying.
    pub(crate) fn discard_entities_in(&mut self, b: ([f64; 3], [f64; 3])) {
        let list: Vec<E> = self
            .all_ents()
            .into_iter()
            .filter(|&e| e != E::Player)
            .filter(|&e| {
                let x = self.e_bbox(e);
                (0..3).all(|i| x.min[i] < b.1[i] && x.max[i] > b.0[i])
            })
            .collect();
        for e in list {
            self.ent_discard(e);
        }
    }

    /// Removal without death (no loot, no animation).
    pub(crate) fn ent_discard(&mut self, e: E) {
        match e {
            E::Player => {}
            E::Mob(i) => {
                if let Some(m) = self.mobs.index_of(i).and_then(|k| self.mobs.list[k].as_deref_mut()) {
                    m.removed = true;
                }
            }
            E::Other(i) if self.cart_ref(i).is_some() => {
                if let Some((k, mut c)) = self.cart_take(i) {
                    self.cart_discard(&mut c);
                    self.cart_put(k, c);
                }
            }
            _ => self.ent_kill(e),
        }
    }

    /// Placement of the test's structure by its block: chunks forced, room cleared, entities
    /// removed, template placed at the start corner (known shapes); `false` without a test or
    /// a template.
    fn ti_place(&mut self, g: &Geo) -> bool {
        let Some(test) = g.data.test.as_deref().and_then(|t| self.cmd.tests.test(t)).cloned() else { return false };
        let Some(t) = self.template_get(&test.structure) else { return false };
        let start = g.start_corner();
        let mut changed = false;
        for c in chunks_of(g.structure_box()) {
            changed |= self.cmd.forced.insert(c);
        }
        self.cmd.forced_dirty |= changed;
        self.clear_space(g.test_box());
        self.discard_entities_in(g.test_aabb());
        let set = crate::tech_blocks::LivePlace {
            mirror: Mirror::None,
            rot: g.rot,
            ignore_entities: g.data.ignore_entities,
            known_shape: true,
            integrity: 1.0,
            seed: 0,
        };
        let flags = flag::CLIENTS | flag::KNOWN_SHAPE | flag::NO_DROPS | flag::SKIP_BE_SIDE_EFFECTS | flag::SKIP_ON_PLACE;
        self.place_template_live(&t.t, start, &set, flags);
        true
    }

    /// Reset from the block's screen or `test reset…`: barriers and markers removed, structure
    /// placed again, status cleared.
    pub(crate) fn ti_reset(&mut self, p: P, feedback: &mut dyn FnMut(&mut Server, Component)) {
        let Some(g) = self.ti_geo(p) else { return };
        self.ti_remove_barriers_of(&g);
        self.ti_markers_clear(p);
        if self.ti_place(&g) {
            let name = self.ti_name(&g.data);
            feedback(self, colored(tr!("test_instance_block.reset_success", name), GREEN));
        }
        let d = self.ti_data(p).unwrap_or(g.data).with_status(0);
        self.ti_set(p, &d);
    }

    /// Test name shown by the block (its identifier, or "invalid").
    pub(crate) fn ti_name(&self, d: &Data) -> Component {
        d.test.as_ref().map_or_else(|| tr!("test_instance_block.invalid_test"), |t| lit(t.clone()))
    }

    /// Structure saved to the world's folder (air left out); its identifier, `None` without test.
    pub(crate) fn ti_save(&mut self, p: P, feedback: &mut dyn FnMut(&mut Server, Component)) -> Option<String> {
        let g = self.ti_geo(p)?;
        let id = match g.data.test.as_deref() {
            Some(t) => Some(self.cmd.tests.test(t).map_or_else(|| t.to_owned(), |d| d.structure.clone())),
            None => None,
        };
        let Some(id) = id else {
            feedback(self, colored(tr!("test_instance_block.error.unable_to_save", p[0], p[1], p[2]), RED));
            return None;
        };
        self.template_capture(&id, g.structure_pos(), g.data.size, !g.data.ignore_entities, "", true, true);
        Some(id)
    }

    /// Run from the block's screen: one test in place, no retry, reported to the operators by
    /// the server.
    pub(crate) fn ti_run(&mut self, p: P, feedback: &mut dyn FnMut(&mut Server, Component)) {
        let Some(g) = self.ti_geo(p) else { return };
        let test = g.data.test.as_deref().and_then(|t| self.cmd.tests.test(t)).cloned();
        let Some(test) = test else {
            feedback(self, colored(tr!("test_instance_block.error.no_test", p[0], p[1], p[2]), RED));
            return;
        };
        if !self.ti_place(&g) {
            feedback(self, colored(tr!("test_instance_block.error.no_test_structure", p[0], p[1], p[2]), RED));
            return;
        }
        self.ti_markers_clear(p);
        self.tests_stop();
        self.cmd.tests.forget_failed();
        feedback(self, tr!("test_instance_block.starting", test.id.clone()));
        let plan = Planned { test, rot: g.data.rot, dim: self.dim.name.clone(), retry: Retry::NONE, pos: Some(p) };
        let src = self.keep_source(&self.server_reply_source());
        self.tests_start(src, vec![plan], RunSpec { per_row: 8, clear_on_batch: false, verify: None });
    }

    // ---------- Ticker ----------

    /// `test stop` and every new run: the runner stops (at the end of its tick if it is ticking).
    pub(crate) fn tests_stop(&mut self) {
        self.cmd.tests.stops = self.cmd.tests.stops.wrapping_add(1);
        if self.cmd.tests.ticking {
            self.cmd.tests.halting = true;
            return;
        }
        if let Some(mut rn) = self.cmd.tests.runner.take() {
            rn.ticked.clear();
            self.runner_stop(&mut rn);
        }
    }

    fn runner_stop(&mut self, rn: &mut Runner) {
        rn.stopped = true;
        self.end_env(rn);
    }

    /// Ticks the tests, after the world and the players, while the game runs normally.
    pub(crate) fn gametest_tick(&mut self) {
        let Some(mut rn) = self.cmd.tests.runner.take() else { return };
        self.cmd.tests.ticking = true;
        let list = rn.ticked.clone();
        for i in list {
            let dim = rn.infos[i].dim.clone();
            self.in_dimension(&dim, |sv| sv.info_tick(&mut rn, i));
        }
        rn.ticked.retain(|&i| !rn.infos[i].done);
        self.cmd.tests.ticking = false;
        if std::mem::take(&mut self.cmd.tests.halting) {
            rn.ticked.clear();
            self.runner_stop(&mut rn);
            if let Some(mut other) = self.cmd.tests.runner.take() {
                self.runner_stop(&mut other);
            }
        } else if self.cmd.tests.runner.is_none() {
            self.cmd.tests.runner = Some(rn);
        }
    }

    /// New run (the previous one is stopped by the caller): batches built, first batch started.
    pub(crate) fn tests_start(&mut self, source: RunSource, plans: Vec<Planned>, spec: RunSpec) {
        let mut rn = Runner {
            infos: Vec::with_capacity(plans.len()),
            batches: Vec::new(),
            trackers: Vec::new(),
            scheduled: Vec::new(),
            stopped: true,
            env: None,
            grid: Some(Grid { per_row: spec.per_row, clear_on_batch: spec.clear_on_batch, dims: Vec::new() }),
            existing_grid: spec.verify.is_some(),
            halt_on_error: spec.verify.is_some(),
            clear_between: spec.verify.is_some(),
            batch_size: if spec.verify.is_some() { 100 } else { 50 },
            reports: Vec::new(),
            source,
            summary_dims: Vec::new(),
            ticked: Vec::new(),
        };
        for pl in plans {
            let r = rn.reports.len();
            rn.reports.push((0, 0));
            let mut info = Info::new(pl.test, pl.rot, pl.dim, pl.retry, r);
            info.pos = pl.pos;
            rn.infos.push(info);
        }
        rn.batches = match spec.verify {
            Some(groups) => groups
                .into_iter()
                .map(|(infos, env, index, dim)| Batch { index, infos, env, dim })
                .collect(),
            None => batches_of(&rn, (0..rn.infos.len()).collect(), 50),
        };
        rn.stopped = false;
        let stops = self.cmd.tests.stops;
        self.run_batch(&mut rn, 0);
        // The first batch starts inside the command; what follows comes after its end.
        rn.source.command_ended();
        // Stopped while starting (a function of the environment ran `test stop`): the game's
        // ticker forgets it at once; a runner started meanwhile wins.
        if self.cmd.tests.stops != stops {
            self.runner_stop(&mut rn);
        } else if self.cmd.tests.runner.is_none() {
            self.cmd.tests.runner = Some(Box::new(rn));
        }
    }

    fn run_batch(&mut self, rn: &mut Runner, index: usize) {
        if index >= rn.batches.len() {
            self.end_env(rn);
            self.rerun_scheduled(rn);
            return;
        }
        if index > 0 && rn.clear_between {
            let last = rn.batches[index - 1].infos.clone();
            for i in last {
                let (dim, pos) = (rn.infos[i].dim.clone(), rn.infos[i].pos);
                let ghost = rn.infos[i].be.clone();
                self.in_dimension(&dim, |sv| {
                    if let (Some(p), Some(g)) = (pos, pos.and_then(|p| sv.info_geo(p, ghost))) {
                        sv.clear_space(g.test_box());
                        sv.destroy_block(p, false, 512);
                    }
                });
            }
        }
        self.grid_batch_start(rn);
        let batch_infos = rn.batches[index].infos.clone();
        let mut spawned = Vec::new();
        for i in batch_infos {
            if self.spawn(rn, i) {
                spawned.push(i);
            }
        }
        self.end_env(rn);
        let (env, dim, bindex) = (rn.batches[index].env.clone(), rn.batches[index].dim.clone(), rn.batches[index].index);
        if let Some(def) = self.cmd.tests.env(&env) {
            let saved = self.in_dimension(&dim, |sv| sv.env_setup(&def, &dim.clone()));
            rn.env = Some(Activation { def, dim: dim.clone(), saved });
        }
        let src = rn.source.clone();
        self.as_source(&src, |sv, s| sv.ok(s, tr!("commands.test.batch.starting", env.name(), bindex)));
        let tracker = rn.trackers.len();
        rn.trackers.push((index as i32, spawned.clone()));
        for &i in &spawned {
            rn.infos[i].batch = Some(tracker);
        }
        rn.ticked.extend(spawned);
    }

    fn rerun_scheduled(&mut self, rn: &mut Runner) {
        if rn.scheduled.is_empty() {
            rn.batches.clear();
            rn.stopped = true;
            return;
        }
        let list = std::mem::take(&mut rn.scheduled);
        let size = rn.batch_size;
        rn.batches = batches_of(rn, list, size);
        rn.stopped = false;
        self.run_batch(rn, 0);
    }

    fn rerun(&mut self, rn: &mut Runner, i: usize) {
        let o = &rn.infos[i];
        let mut copy = Info::new(o.test.clone(), o.extra_rot, o.dim.clone(), o.retry, o.report);
        copy.pos = o.pos;
        rn.infos.push(copy);
        let c = rn.infos.len() - 1;
        rn.scheduled.push(c);
        if rn.stopped {
            self.rerun_scheduled(rn);
        }
    }

    /// Lays a test out: the grid for a new one (or always, `verify`), in place otherwise.
    fn spawn(&mut self, rn: &mut Runner, i: usize) -> bool {
        if rn.infos[i].pos.is_none() || rn.existing_grid {
            return self.grid_spawn(rn, i);
        }
        let dim = rn.infos[i].dim.clone();
        self.in_dimension(&dim, |sv| sv.prepare(rn, i)).then(|| rn.infos[i].start_execution(1)).is_some()
    }

    /// Test instance block (re)created at the test's position with its test, the size of its
    /// template and its rotation, then the structure placed and closed.
    fn prepare(&mut self, rn: &mut Runner, i: usize) -> bool {
        let Some(p) = rn.infos[i].pos else { return false };
        let block = self.default_state("minecraft:test_instance_block");
        self.load_now(&[(p[0] >> 4, p[2] >> 4)]);
        self.set_block(p, block, flag::ALL);
        if self.ti_data(p).is_none() {
            return false;
        }
        let test = rn.infos[i].test.id.clone();
        let size = self.test_structure_size(&test).unwrap_or([1, 1, 1]);
        let d = Data::new(Some(test), size, rn.infos[i].extra_rot);
        self.ti_set(p, &d);
        rn.infos[i].be = Some(d);
        self.place_structure(rn, i);
        true
    }

    fn info_geo(&self, p: P, ghost: Option<Data>) -> Option<Geo> {
        self.ti_geo(p).or_else(|| ghost.map(|d| self.geo(p, d)))
    }

    fn place_structure(&mut self, rn: &mut Runner, i: usize) {
        if rn.infos[i].placed {
            return;
        }
        let Some(p) = rn.infos[i].pos else { return };
        let Some(g) = self.info_geo(p, rn.infos[i].be.clone()) else { return };
        if !self.ti_place(&g) {
            let name = crate::commands::cmds_ents::plain(&self.ti_name(&g.data));
            rn.infos[i].fail_msg(tr!("test.error.structure.failure", name));
        }
        rn.infos[i].placed = true;
        self.ti_encase(&g);
        let (lo, hi) = g.test_box();
        self.bs.block_ticks.clear_box(lo, hi);
        self.clear_block_events(lo, hi);
        // Report: one more attempt.
        rn.reports[rn.infos[i].report].0 += 1;
    }

    fn info_tick(&mut self, rn: &mut Runner, i: usize) {
        if rn.infos[i].done {
            return;
        }
        if !rn.infos[i].placed {
            rn.infos[i].fail_msg(tr!("test.error.ticking_without_structure"));
        }
        if rn.infos[i].be.is_none() {
            rn.infos[i].fail_msg(tr!("test.error.missing_block_entity"));
        }
        if rn.infos[i].error.is_some() {
            rn.infos[i].finish();
        } else if !rn.infos[i].chunks_loaded {
            let p = rn.infos[i].pos;
            let g = p.and_then(|p| self.info_geo(p, rn.infos[i].be.clone()));
            let loaded = g.is_some_and(|g| {
                chunks_of(g.structure_box()).into_iter().all(|c| self.mob_chunk_ticking(c) && self.chunks.get(&c).is_some_and(Option::is_some))
            });
            if !loaded {
                return;
            }
            rn.infos[i].chunks_loaded = true;
        }
        self.tick_internal(rn, i);
        if rn.infos[i].done {
            if rn.infos[i].error.is_some() {
                self.on_failed(rn, i);
            } else {
                self.on_passed(rn, i);
            }
        }
    }

    fn tick_internal(&mut self, rn: &mut Runner, i: usize) {
        rn.infos[i].tick += 1;
        let tick = rn.infos[i].tick;
        if tick < 0 {
            return;
        }
        if !rn.infos[i].started {
            self.start_test(rn, i);
        }
        while let Some((next, to)) = rn.infos[i].each_tick {
            if next > tick || next >= to {
                break;
            }
            rn.infos[i].each_tick = Some((next + 1, to));
            if let Err(e) = self.block_based_check(rn, i) {
                rn.infos[i].fail(e);
            }
        }
        if tick > rn.infos[i].test.max_ticks {
            let max = rn.infos[i].test.max_ticks;
            rn.infos[i].fail(TestError::Timeout(tr!("test.error.timeout.no_result", max)));
        }
    }

    fn start_test(&mut self, rn: &mut Runner, i: usize) {
        rn.infos[i].started = true;
        rn.infos[i].started_at = Some(Instant::now());
        if let Some(p) = rn.infos[i].pos {
            let d = self.ti_data(p).or_else(|| rn.infos[i].be.clone());
            if let Some(d) = d {
                let d = d.with_status(1);
                self.ti_set(p, &d);
                rn.infos[i].be = Some(d);
            }
        }
        let kind = rn.infos[i].test.kind.clone();
        match kind {
            Kind::Function(f) if f == "minecraft:always_pass" => self.info_succeed(rn, i),
            Kind::Function(f) => rn.infos[i].fail(TestError::Unknown(format!("Trying to access missing test function: {f}"))),
            Kind::TestBlocks => {
                if let Err(e) = self.block_based_start(rn, i) {
                    rn.infos[i].fail(e);
                }
            }
        }
    }

    fn info_succeed(&mut self, rn: &mut Runner, i: usize) {
        if rn.infos[i].error.is_some() {
            return;
        }
        rn.infos[i].finish();
        if let Some(g) = rn.infos[i].pos.and_then(|p| self.info_geo(p, rn.infos[i].be.clone())) {
            let (lo, hi) = g.structure_aabb();
            self.discard_entities_in(([lo[0] - 1.0, lo[1] - 1.0, lo[2] - 1.0], [hi[0] + 1.0, hi[1] + 1.0, hi[2] + 1.0]));
        }
    }

    // ---------- Block-based tests ----------

    /// Origin, rotation and relative size of a test (the game's helper).
    fn helper(&self, rn: &Runner, i: usize) -> Option<(P, Rot, [i32; 3])> {
        let g = self.info_geo(rn.infos[i].pos?, rn.infos[i].be.clone())?;
        let (lo, hi) = g.structure_box();
        let abs = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
        let rot = rn.infos[i].rotation();
        let rel = if matches!(rot, Rot::Cw90 | Rot::Ccw90) { [abs[2], abs[1], abs[0]] } else { abs };
        Some((g.start_corner(), rot, rel))
    }

    /// Test blocks of the structure by mode (start, log, fail, accept), relative positions in
    /// the game's order (x, then y, then z). One pass for the four modes (nothing changes the
    /// blocks between the game's passes); `RUSTCRAFT_OPT_GT_SCAN=0`: every cell read, otherwise
    /// the sections without a test block are skipped.
    fn find_test_blocks(&self, rn: &Runner, i: usize) -> [Vec<P>; 4] {
        let Some((origin, rot, rel)) = self.helper(rn, i) else { return Default::default() };
        if rc_data::env_str!("RUSTCRAFT_OPT_GT_SCAN") == Some("0") {
            self.test_blocks_cellwise(origin, rot, rel)
        } else {
            self.test_blocks_by_section(origin, rot, rel)
        }
    }

    fn test_block_mode(&self, st: u32, test_block: u16) -> Option<usize> {
        if self.view().block(st) != test_block {
            return None;
        }
        self.view().get(st, "mode").and_then(|m| MODES.iter().position(|x| *x == m))
    }

    /// Every cell of the structure, in the game's order.
    pub(crate) fn test_blocks_cellwise(&self, origin: P, rot: Rot, rel: [i32; 3]) -> [Vec<P>; 4] {
        let mut out: [Vec<P>; 4] = Default::default();
        let test_block = self.table().block_id("minecraft:test_block");
        for z in 0..rel[2] {
            for y in 0..rel[1] {
                for x in 0..rel[0] {
                    let q = transform([origin[0] + x, origin[1] + y, origin[2] + z], Mirror::None, rot, origin);
                    if let Some(m) = self.test_block_mode(self.get(q), test_block) {
                        out[m].push([x, y, z]);
                    }
                }
            }
        }
        out
    }

    /// The sections of the structure that may hold a test block (palette), their cells inside
    /// the structure only, sorted back into the game's order.
    pub(crate) fn test_blocks_by_section(&self, origin: P, rot: Rot, rel: [i32; 3]) -> [Vec<P>; 4] {
        let mut out: [Vec<P>; 4] = Default::default();
        if rel.iter().any(|&v| v < 1) {
            return out;
        }
        let test_block = self.table().block_id("minecraft:test_block");
        let far = transform([origin[0] + rel[0] - 1, origin[1] + rel[1] - 1, origin[2] + rel[2] - 1], Mirror::None, rot, origin);
        let (lo, hi) = (
            [origin[0].min(far[0]), origin[1].min(far[1]), origin[2].min(far[2])],
            [origin[0].max(far[0]), origin[1].max(far[1]), origin[2].max(far[2])],
        );
        // Exact inverse of the layout rotation (absolute cell back to the relative cell the
        // game visits).
        let inv = Rot::from_index(-(rot as i32));
        let (min_y, sections) = (self.dim.height.min_y, self.dim.height.sections as i32);
        let mut found: Vec<((i32, i32, i32), usize, P)> = Vec::new();
        for cx in (lo[0] >> 4)..=(hi[0] >> 4) {
            for cz in (lo[2] >> 4)..=(hi[2] >> 4) {
                let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { continue };
                for sy in ((lo[1] - min_y).max(0) >> 4)..=((hi[1] - min_y) >> 4).min(sections - 1) {
                    let Some(sec) = c.sections.get(sy as usize) else { continue };
                    if !sec.blocks.may_contain(|st| self.view().block(st) == test_block) {
                        continue;
                    }
                    let base = [cx * 16, min_y + sy * 16, cz * 16];
                    let span = |k: usize| (lo[k].max(base[k]) - base[k])..=(hi[k].min(base[k] + 15) - base[k]);
                    for ly in span(1) {
                        for lz in span(2) {
                            for lx in span(0) {
                                let st = sec.blocks.get(rc_world::block_index(lx as usize, ly as usize, lz as usize));
                                let Some(m) = self.test_block_mode(st, test_block) else { continue };
                                let q = [base[0] + lx, base[1] + ly, base[2] + lz];
                                let r0 = transform(q, Mirror::None, inv, origin);
                                let r = [r0[0] - origin[0], r0[1] - origin[1], r0[2] - origin[2]];
                                found.push(((r[2], r[1], r[0]), m, r));
                            }
                        }
                    }
                }
            }
        }
        found.sort_unstable_by_key(|f| f.0);
        for (_, m, r) in found {
            out[m].push(r);
        }
        out
    }

    fn abs_pos(&self, rn: &Runner, i: usize, rel: P) -> Option<P> {
        let (origin, rot, _) = self.helper(rn, i)?;
        Some(transform([origin[0] + rel[0], origin[1] + rel[1], origin[2] + rel[2]], Mirror::None, rot, origin))
    }

    /// Test block at a relative position: absolute position, or the game's assertion.
    fn test_block_at(&self, rn: &Runner, i: usize, rel: P) -> Result<P, TestError> {
        let tick = rn.infos[i].tick;
        let abs = self.abs_pos(rn, i, rel).unwrap_or(rel);
        let err = |msg| TestError::AssertAt { msg, abs, rel, tick };
        match self.be_get(abs).and_then(|b| b.get("id")).and_then(Nbt::as_str) {
            None => Err(err(tr!("test.error.missing_block_entity"))),
            Some("minecraft:test_block") => Ok(abs),
            Some(other) => Err(err(tr!("test.error.wrong_block_entity", other.to_owned()))),
        }
    }

    fn test_mode_name(mode: usize) -> Component {
        tr!(&format!("test_block.mode.{}", MODES[mode]))
    }

    /// Start: the single start block is triggered, then the check runs on every remaining tick.
    fn block_based_start(&mut self, rn: &mut Runner, i: usize) -> Result<(), TestError> {
        let tick = rn.infos[i].tick;
        let blocks = self.find_test_blocks(rn, i);
        if blocks[0].is_empty() {
            return Err(TestError::Assert { msg: tr!("test_block.error.missing", Self::test_mode_name(0)), tick });
        }
        if blocks[0].len() != 1 {
            return Err(TestError::Assert { msg: tr!("test_block.error.too_many", Self::test_mode_name(0)), tick });
        }
        let at = self.test_block_at(rn, i, blocks[0][0])?;
        self.test_block_trigger(at);
        rn.infos[i].each_tick = Some((tick, rn.infos[i].test.max_ticks));
        Ok(())
    }

    /// Check of a tick: an accept block triggered passes; otherwise a triggered fail block fails
    /// with its message (it stays triggered), the triggered log blocks are reset.
    fn block_based_check(&mut self, rn: &mut Runner, i: usize) -> Result<(), TestError> {
        let tick = rn.infos[i].tick;
        let blocks = self.find_test_blocks(rn, i);
        if blocks[3].is_empty() {
            return Err(TestError::Assert { msg: tr!("test_block.error.missing", Self::test_mode_name(3)), tick });
        }
        for &r in &blocks[3] {
            let at = self.test_block_at(rn, i, r)?;
            if self.test_block_triggered(at) {
                self.info_succeed(rn, i);
                return Ok(());
            }
        }
        for &r in &blocks[2] {
            let at = self.test_block_at(rn, i, r)?;
            if self.test_block_triggered(at) {
                let msg = self.be_get(at).and_then(|b| b.get("message")).and_then(Nbt::as_str).unwrap_or("").to_owned();
                return Err(TestError::Assert { msg: lit(msg), tick });
            }
        }
        for &r in &blocks[1] {
            let at = self.test_block_at(rn, i, r)?;
            if self.test_block_triggered(at) {
                self.test_block_trigger(at);
                self.test_block_reset(at);
            }
        }
        Ok(())
    }

    // ---------- Listeners ----------

    fn on_passed(&mut self, rn: &mut Runner, i: usize) {
        self.report_passed(rn, i);
        self.summary_check(rn, i);
        if let Some(t) = rn.infos[i].batch {
            if let Some(p) = rn.infos[i].pos
                && let Some(g) = self.info_geo(p, rn.infos[i].be.clone())
            {
                self.ti_remove_barriers_of(&g);
            }
            self.batch_completed(rn, t, i);
        }
    }

    fn on_failed(&mut self, rn: &mut Runner, i: usize) {
        self.report_failed(rn, i);
        self.summary_check(rn, i);
        let id = rn.infos[i].test.id.clone();
        self.cmd.tests.remember_failed(&id);
        if let Some(t) = rn.infos[i].batch {
            if rn.halt_on_error {
                self.end_env(rn);
                self.unforce_all();
                self.tests_stop();
            } else {
                self.batch_completed(rn, t, i);
            }
        }
    }

    /// The batch's last test done: the level's forced chunks are all released (those of
    /// `forceload` too, like the game), then the next batch starts.
    fn batch_completed(&mut self, rn: &mut Runner, t: usize, _i: usize) {
        let done = rn.trackers[t].1.iter().all(|&k| rn.infos[k].done);
        if done {
            self.unforce_all();
            let next = rn.trackers[t].0 as usize + 1;
            self.run_batch(rn, next);
        }
    }

    fn unforce_all(&mut self) {
        if !self.cmd.forced.is_empty() {
            self.cmd.forced.clear();
            self.cmd.forced_dirty = true;
        }
    }

    /// Message to every player (the game's test reports).
    fn say(&self, color: u32, text: String) {
        let c = colored(lit(text), color);
        self.send_all_players(rc_client::Event::Chat { component: Box::new(c), tag: rc_client::ChatTag::System });
    }

    fn report_passed(&mut self, rn: &mut Runner, i: usize) {
        let r = rn.infos[i].report;
        rn.reports[r].1 += 1;
        let (attempts, successes) = rn.reports[r];
        let info = &rn.infos[i];
        let id = info.test.id.clone();
        if info.retry.has_retries() {
            self.handle_retry(rn, i, true);
        } else if !info.test.flaky() {
            let text = format!("{id} passed! ({}ms / {}gameticks)", info.run_time(), info.tick);
            self.report_success(rn, i, text);
        } else if successes >= info.test.required_successes {
            let text = format!("{id} passed {successes} times of {attempts} attempts.");
            self.report_success(rn, i, text);
        } else {
            self.say(GREEN, format!("Flaky test {id} succeeded, attempt: {attempts} successes: {successes}"));
            self.rerun(rn, i);
        }
    }

    fn report_failed(&mut self, rn: &mut Runner, i: usize) {
        let (attempts, successes) = rn.reports[rn.infos[i].report];
        let test = rn.infos[i].test.clone();
        if !test.flaky() {
            let e = rn.infos[i].error.clone().unwrap_or(TestError::Unknown(String::new()));
            self.report_failure(rn, i, &e);
            if rn.infos[i].retry.has_retries() {
                self.handle_retry(rn, i, false);
            }
        } else {
            let mut text = format!("Flaky test {} failed, attempt: {attempts}/{}", test.id, test.max_attempts);
            if test.required_successes > 1 {
                text += &format!(", successes: {successes} ({} required)", test.required_successes);
            }
            self.say(YELLOW, text);
            if test.max_attempts - attempts + successes >= test.required_successes {
                self.rerun(rn, i);
            } else {
                let cause = rn.infos[i].error.clone().unwrap_or(TestError::Unknown(String::new()));
                let text = format!(
                    "Not enough successes: {successes} out of {attempts} attempts. Required successes: {}. max attempts: {}.",
                    test.required_successes, test.max_attempts
                );
                self.report_failure(rn, i, &TestError::Exhausted { text, cause: Box::new(cause) });
            }
        }
    }

    fn handle_retry(&mut self, rn: &mut Runner, i: usize, passed: bool) {
        let (attempts, successes) = rn.reports[rn.infos[i].report];
        let retry = rn.infos[i].retry;
        let mut head = format!("[Run: {attempts:4}, Ok: {successes:4}, Fail: {:4}", attempts - successes);
        if !retry.unlimited() {
            head += &format!(", Left: {:4}", retry.tries - attempts);
        }
        head.push(']');
        let name = format!(
            "{} {}! {}ms",
            rn.infos[i].test.id,
            if passed { "passed" } else { "failed" },
            rn.infos[i].run_time()
        );
        let text = format!("{head:<53}{name}");
        if passed {
            self.report_success(rn, i, text);
        } else {
            self.say(RED, text);
        }
        if retry.tries_left(attempts, successes) {
            self.rerun(rn, i);
        }
    }

    fn report_success(&mut self, rn: &mut Runner, i: usize, text: String) {
        if let Some(p) = rn.infos[i].pos
            && let Some(d) = self.ti_data(p)
        {
            self.ti_set(p, &d.with_status(2));
        }
        self.say(GREEN, text);
    }

    fn report_failure(&mut self, rn: &mut Runner, i: usize, e: &TestError) {
        let info = &rn.infos[i];
        if let Some(p) = info.pos
            && let Some(d) = self.ti_data(p)
        {
            let mut d = d.with_status(2);
            d.error = Some(e.block_text());
            self.ti_set(p, &d);
        }
        let mut message = e.message();
        if let TestError::Exhausted { cause, .. } = e {
            message += &format!(" cause: {}", cause.deepest_message());
        }
        let required = info.test.required;
        let text = format!("{}{} failed! {message}", if required { "" } else { "(optional) " }, info.test.id);
        self.say(if required { RED } else { YELLOW }, text);
        if let (Some((at, msg)), Some(p)) = (e.marker(), info.pos) {
            self.ti_mark_error(p, at, &msg);
        }
    }

    /// The run's summary to its source once every test is done.
    fn summary_check(&mut self, rn: &mut Runner, i: usize) {
        let dim = rn.infos[i].test.dimension.clone();
        if !rn.summary_dims.contains(&dim) {
            rn.summary_dims.push(dim);
        }
        if !rn.infos.iter().all(|x| x.done) {
            return;
        }
        let total = rn.infos.len();
        let failed = |req: bool| rn.infos.iter().filter(|x| x.error.is_some() && x.test.required == req).count();
        let (failed_required, failed_optional) = (failed(true), failed(false));
        let src = rn.source.clone();
        let dims = rn.summary_dims.clone();
        self.as_source(&src, |sv, s| {
            let mut c = tr!("commands.test.summary", total);
            c.style.color = Some(0xFFFFFF);
            sv.ok(s, c);
            if failed_required > 0 {
                sv.fail(s, tr!("commands.test.summary.failed", failed_required));
            } else {
                sv.ok(s, colored(tr!("commands.test.summary.all_required_passed"), GREEN));
            }
            if failed_optional > 0 {
                sv.tell_source(s, tr!("commands.test.summary.optional_failed", failed_optional));
            }
            sv.output_player_coordinates(s, &dims);
            sv.output_test_coordinates(s, &dims);
        });
    }

    /// Runs `f` as the source of the run: its dimension and its player installed.
    pub(crate) fn as_source(&mut self, rs: &RunSource, f: impl FnOnce(&mut Server, &Src)) {
        self.as_kept_source(rs, f);
    }

    // ---------- Grid ----------

    fn grid_batch_start(&mut self, rn: &mut Runner) {
        let Some(grid) = rn.grid.as_mut() else { return };
        if !grid.clear_on_batch {
            return;
        }
        let mut work = Vec::new();
        for (_, g) in &mut grid.dims {
            work.extend(std::mem::take(&mut g.last_batch));
            g.next = g.first;
            g.row = unit_box(g.first);
            g.count = 0;
        }
        for i in work {
            let (dim, pos, ghost) = (rn.infos[i].dim.clone(), rn.infos[i].pos, rn.infos[i].be.clone());
            self.in_dimension(&dim, |sv| {
                if let Some(g) = pos.and_then(|p| sv.info_geo(p, ghost)) {
                    sv.clear_space(g.test_box());
                }
            });
        }
    }

    fn grid_spawn(&mut self, rn: &mut Runner, i: usize) -> bool {
        let dim = rn.infos[i].test.dimension.clone();
        let have = rn.grid.as_ref().is_some_and(|g| g.dims.iter().any(|(d, _)| *d == dim));
        if rn.grid.is_none() {
            return false;
        }
        if !have {
            let first = self.test_position_around(&mut rn.source, &dim);
            if let Some(g) = rn.grid.as_mut() {
                g.dims.push((dim.clone(), DimGrid { first, next: first, row: unit_box(first), count: 0, last_batch: Vec::new() }));
            }
        }
        let Some(corner) = rn.grid.as_ref().and_then(|g| g.dims.iter().find(|(d, _)| *d == dim)).map(|(_, g)| g.next) else {
            return false;
        };
        rn.infos[i].pos = Some(corner);
        let idim = rn.infos[i].dim.clone();
        let ok = self.in_dimension(&idim, |sv| sv.prepare(rn, i));
        if !ok {
            return false;
        }
        rn.infos[i].start_execution(1);
        let bounds = self.in_dimension(&idim, |sv| sv.info_geo(corner, rn.infos[i].be.clone()).map(|g| g.test_aabb()));
        let Some(b) = bounds else { return true };
        let Some(grid) = rn.grid.as_mut() else { return true };
        let per_row = grid.per_row;
        let Some((_, g)) = grid.dims.iter_mut().find(|(d, _)| *d == dim) else { return true };
        g.row = (
            [g.row.0[0].min(b.0[0]), g.row.0[1].min(b.0[1]), g.row.0[2].min(b.0[2])],
            [g.row.1[0].max(b.1[0]), g.row.1[1].max(b.1[1]), g.row.1[2].max(b.1[2])],
        );
        g.next[0] += (b.1[0] - b.0[0]) as i32 + 5;
        g.count += 1;
        if g.count >= per_row {
            g.count = 0;
            g.next[2] += (g.row.1[2] - g.row.0[2]) as i32 + 6;
            g.next[0] = g.first[0];
            g.row = unit_box(g.next);
        }
        g.last_batch.push(i);
        true
    }

    /// Position of the source's player when its executing entity is a player (whoever runs the
    /// command: `execute as @p` from a command block included; their last known position once
    /// they left), otherwise of the source; the surface of its column in `dim`.
    fn player_and_surface(&mut self, rs: &mut RunSource, dim: &str) -> (P, i32) {
        let pos = if rs.src.entity == Some(E::Player) {
            if let Some(seat) = rs.player.and_then(|u| self.seat_of_uuid(u)) {
                rs.last = self.as_player(seat, |sv| sv.pos);
            }
            rs.last
        } else {
            rs.src.pos
        };
        let p = pos.map(|v| v.floor() as i32);
        let y = self.in_dimension(dim, |sv| {
            sv.load_now(&[(p[0] >> 4, p[2] >> 4)]);
            sv.surface_y(p[0], p[2], 1)
        });
        (p, y)
    }

    /// A player of the run left: their last position is kept for the grid.
    pub(crate) fn tests_player_left(&mut self, uuid: u128, pos: [f64; 3]) {
        if let Some(rs) = self.cmd.tests.runner.as_mut().map(|r| &mut r.source).filter(|s| s.player == Some(uuid)) {
            rs.last = pos;
        }
    }

    pub(crate) fn test_position_around(&mut self, rs: &mut RunSource, dim: &str) -> P {
        let (p, y) = self.player_and_surface(rs, dim);
        [p[0], y, p[2] + 3]
    }

    // ---------- Environments ----------

    fn end_env(&mut self, rn: &mut Runner) {
        if let Some(a) = rn.env.take() {
            let dim = a.dim.clone();
            self.in_dimension(&dim, |sv| sv.env_teardown(a));
        }
    }

    fn env_setup(&mut self, def: &EnvDef, dim: &str) -> Saved {
        match def {
            EnvDef::Group(list) => {
                let mut v = Vec::new();
                for r in list {
                    if let Some(d) = self.cmd.tests.env(r) {
                        let saved = self.env_setup(&d, dim);
                        v.push(Activation { def: d, dim: dim.to_owned(), saved });
                    }
                }
                Saved::All(v)
            }
            EnvDef::Clock { clock, time } => {
                let prev = self.clocks.iter().find(|c| c.0 == *clock).map_or(0, |c| c.1);
                self.set_clock(clock, *time);
                Saved::Clock(clock.clone(), prev)
            }
            EnvDef::Difficulty(d) => {
                let prev = self.level.difficulty_id();
                self.set_difficulty(*d, true);
                Saved::Difficulty(prev)
            }
            EnvDef::Function { setup, .. } => {
                if let Some(f) = setup {
                    self.env_function(f);
                }
                Saved::Nothing
            }
            EnvDef::Rules(rules) => {
                let prev = rules.iter().map(|(k, _)| (k.clone(), self.rules.get(k).cloned())).collect();
                for (k, v) in rules {
                    self.set_rule_value(k, Some(v.clone()));
                }
                Saved::Rules(prev)
            }
            EnvDef::TimelineLayers(_) => Saved::Nothing,
            EnvDef::Weather(w) => {
                let wt = &self.life.weather;
                let prev = if wt.thundering {
                    Weather::Thunder
                } else if wt.raining {
                    Weather::Rain
                } else {
                    Weather::Clear
                };
                self.apply_weather(*w);
                Saved::Weather(prev)
            }
        }
    }

    fn env_teardown(&mut self, a: Activation) {
        match (&*a.def, a.saved) {
            (_, Saved::All(list)) => {
                for x in list.into_iter().rev() {
                    self.env_teardown(x);
                }
            }
            (_, Saved::Clock(c, t)) => self.set_clock(&c, t),
            (_, Saved::Difficulty(d)) => self.set_difficulty(d, true),
            (_, Saved::Rules(prev)) => {
                for (k, v) in prev {
                    self.set_rule_value(&k, v);
                }
            }
            (_, Saved::Weather(w)) => {
                let wt = &mut self.life.weather;
                wt.rain_time = 0;
                wt.raining = false;
                wt.thunder_time = 0;
                wt.thundering = false;
                self.apply_weather(w);
            }
            (EnvDef::Function { teardown: Some(f), .. }, Saved::Nothing) => {
                let f = f.clone();
                self.env_function(&f);
            }
            (_, Saved::Nothing) => {}
        }
    }

    fn apply_weather(&mut self, w: Weather) {
        let (clear, rain, raining, thundering) = match w {
            Weather::Clear => (100_000, 0, false, false),
            Weather::Rain => (0, 100_000, true, false),
            Weather::Thunder => (0, 100_000, true, true),
        };
        let wt = &mut self.life.weather;
        wt.clear_time = clear;
        wt.rain_time = rain;
        wt.thunder_time = rain;
        wt.raining = raining;
        wt.thundering = thundering;
    }

    /// Function of an environment: the server's source at the game master level, output
    /// suppressed, in the batch's dimension (a missing function only reaches the server log).
    fn env_function(&mut self, id: &str) {
        if !self.cmd.functions.contains_key(id) {
            return;
        }
        let mut src = self.server_source();
        src.level = 2;
        src.dim = self.dim.name.clone();
        let _ = self.run_functions(&src, id, false);
    }

    /// A rule set (or back to its default) with its immediate effects.
    fn set_rule_value(&mut self, key: &str, v: Option<Nbt>) {
        match v {
            Some(v) => crate::nbtio::set(&mut self.rules, key, v),
            None => crate::nbtio::remove(&mut self.rules, key),
        }
        self.cmd.rules_dirty = true;
        self.after_rule_change(key);
    }

    // ---------- Coordinates of the summary ----------

    /// The player's own world counts as the tests' world only when it is the overworld and the
    /// only one of the run.
    fn player_in_test_dimension(&self, src: &Src, dims: &[String]) -> bool {
        src.entity == Some(E::Player)
            && dims.len() <= 1
            && dims.contains(&src.dim)
            && dims.iter().any(|d| d == "minecraft:overworld")
    }

    fn coordinates_click(&self, text: Component, color: u32, cmd: String) -> Component {
        let mut c = brackets(text);
        c.style.color = Some(color);
        suggest_click(&mut c, cmd);
        hover_text(&mut c, tr!("chat.coordinates.tooltip"));
        c
    }

    pub(crate) fn output_player_coordinates(&mut self, s: &Src, dims: &[String]) {
        let mut rs = self.keep_source(s);
        let (p, _) = self.player_and_surface(&mut rs, &s.dim.clone());
        if self.player_in_test_dimension(s, dims) || s.entity != Some(E::Player) {
            return;
        }
        let dim = self.dim.name.clone();
        let path = dim.split_once(':').map_or(dim.as_str(), |x| x.1).to_owned();
        let (yaw, pitch) = (self.yaw.floor() as i32, self.pitch.floor() as i32);
        let cmd = format!("/execute in {dim} run tp @s {} {} {} {yaw} {pitch}", p[0], p[1], p[2]);
        let c = self.coordinates_click(tr!("test.player.coordinates", p[0], p[1], p[2], path), GOLD, cmd);
        self.ok_quiet(s, c);
    }

    fn output_test_coordinates(&mut self, s: &Src, dims: &[String]) {
        let mut rs = self.keep_source(s);
        for d in dims {
            let (p, y) = self.player_and_surface(&mut rs, d);
            let _ = self.player_and_surface(&mut rs, &s.dim.clone());
            if self.player_in_test_dimension(s, dims) {
                return;
            }
            let path = d.split_once(':').map_or(d.as_str(), |x| x.1).to_owned();
            let cmd = format!("/execute in {d} run tp @s {} {y} {} 0 0", p[0], p[2]);
            let c = self.coordinates_click(tr!("test.run.coordinates", p[0], y, p[2], path), YELLOW, cmd);
            self.ok_quiet(s, c);
        }
    }

    // ---------- Test blocks ----------

    pub(crate) fn test_block_triggered(&self, p: P) -> bool {
        self.cmd.tests.triggered.contains(&(self.dim.name.clone(), p))
    }

    /// Reset of a test block: no longer triggered; a start block stops emitting.
    pub(crate) fn test_block_reset(&mut self, p: P) {
        let key = (self.dim.name.clone(), p);
        self.cmd.tests.triggered.remove(&key);
        let start = self.be_get(p).and_then(|b| b.get("mode")).and_then(Nbt::as_str) == Some("start");
        if start {
            if let Some(be) = self.be_get_mut(p) {
                crate::nbtio::set(be, "powered", Nbt::Byte(0));
            }
            let b = self.view().block(self.get(p));
            self.update_neighbors_at(p, b);
        }
    }

    pub(crate) fn test_block_mark(&mut self, p: P, on: bool) {
        let key = (self.dim.name.clone(), p);
        if on {
            self.cmd.tests.triggered.insert(key);
        } else {
            self.cmd.tests.triggered.remove(&key);
        }
    }
}

/// Batches of a list of tests: one per environment and dimension (first appearance order; the
/// game's order between environments follows identity hashes), split by `size`.
fn batches_of(rn: &Runner, list: Vec<usize>, size: usize) -> Vec<Batch> {
    let mut groups: Vec<(EnvRef, String, Vec<usize>)> = Vec::new();
    for i in list {
        let t = &rn.infos[i].test;
        match groups.iter_mut().find(|g| g.0 == t.env && g.1 == t.dimension) {
            Some(g) => g.2.push(i),
            None => groups.push((t.env.clone(), t.dimension.clone(), vec![i])),
        }
    }
    let mut out = Vec::new();
    for (env, dim, infos) in groups {
        for (k, chunk) in infos.chunks(size.max(1)).enumerate() {
            out.push(Batch { index: k as i32, infos: chunk.to_vec(), env: env.clone(), dim: dim.clone() });
        }
    }
    out
}
