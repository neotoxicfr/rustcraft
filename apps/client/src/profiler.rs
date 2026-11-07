//! Hierarchical profiler of the frame (`debug-f3.md` § 7.4): nested named zones
//! (`root`, `tick`, `frame`, …), cumulated duration and number of passes per path. It only measures
//! when it is on (F3+1 pie chart, F3+L recording); when off, each call boils down
//! to reading a thread boolean.
//!
//! Like the game: paths separated by the 0x1E character, cumulation since turning on (no reset
//! per frame), "live" reading of the results during the current frame.
//!
//! A collection apart, ours, besides the three text collections (`PIE`, `RECORD`, `SINGLE`), serves
//! the perf log (`RUSTCRAFT_PERFLOG_STAGES`, `frame_stages.rs`): the zones right under `root`, the
//! end of the frame and the driver stages, by index, read once per frame.

use hashbrown::HashMap;
use std::cell::{Cell, RefCell};
use std::time::Instant;

use crate::frame_stages::{FrameStage, FrameStageClock, StageTimes};

/// Separator of the zone paths.
pub const SEP: char = '\u{1e}';

/// Cumulation of a path.
#[derive(Clone, Copy, Debug, Default)]
pub struct Entry {
    pub nanos: u64,
    pub count: u64,
    pub max: u64,
    pub min: u64,
}

/// Collection (pie chart or recording): open paths and cumulations.
#[derive(Default)]
pub struct Collector {
    started: bool,
    path: String,
    /// Length of `path` before each open zone, and opening instant.
    stack: Vec<(usize, Instant)>,
    pub entries: HashMap<String, Entry>,
    /// Counters of each path (`clientTick`…), in order of first increment.
    pub counters: HashMap<String, Vec<(&'static str, u64)>>,
    pub start: Option<Instant>,
    pub frames: u64,
    pub end: Option<Instant>,
}

impl Collector {
    fn new() -> Collector {
        Collector { start: Some(Instant::now()), ..Default::default() }
    }

    fn start_tick(&mut self) {
        self.started = true;
        self.path.clear();
        self.stack.clear();
        self.frames += 1;
        self.push("root");
    }

    fn end_tick(&mut self) {
        while !self.stack.is_empty() {
            self.pop();
        }
        self.started = false;
        self.end = Some(Instant::now());
    }

    fn push(&mut self, name: &str) {
        if !self.started {
            return;
        }
        let len = self.path.len();
        if !self.path.is_empty() {
            self.path.push(SEP);
        }
        self.path.push_str(name);
        self.stack.push((len, Instant::now()));
    }

    fn pop(&mut self) {
        let Some((len, t0)) = self.stack.pop() else { return };
        let ns = t0.elapsed().as_nanos() as u64;
        let e = match self.entries.get_mut(self.path.as_str()) {
            Some(e) => e,
            None => {
                self.entries.entry(self.path.clone()).or_insert_with(|| Entry { min: u64::MAX, ..Default::default() })
            }
        };
        e.nanos += ns;
        e.count += 1;
        e.max = e.max.max(ns);
        e.min = e.min.min(ns);
        self.path.truncate(len);
    }

    /// One more `name` on the open path.
    fn increment(&mut self, name: &'static str) {
        if !self.started {
            return;
        }
        let list = match self.counters.get_mut(self.path.as_str()) {
            Some(l) => l,
            None => self.counters.entry(self.path.clone()).or_default(),
        };
        match list.iter_mut().find(|(n, _)| *n == name) {
            Some(c) => c.1 += 1,
            None => list.push((name, 1)),
        }
    }

    fn duration(&self, path: &str) -> u64 {
        self.entries.get(path).map_or(0, |e| e.nanos)
    }

    fn count(&self, path: &str) -> u64 {
        self.entries.get(path).map_or(0, |e| e.count)
    }

    /// Direct children of `path` with their shares: the node itself first (100 %),
    /// then the children sorted by decreasing share (decreasing name on ties), with an
    /// "unspecified" share when the node lasts longer than its children.
    pub fn times(&self, path: &str) -> Vec<Field> {
        let mut global = self.duration("root");
        let own = self.duration(path);
        let own_count = self.count(path);
        let prefix = if path.is_empty() { String::new() } else { format!("{path}{SEP}") };
        let child = |k: &str| k.len() > prefix.len() && k.starts_with(&prefix) && !k[prefix.len()..].contains(SEP);
        let mut total: u64 = self.entries.iter().filter(|(k, _)| child(k)).map(|(_, e)| e.nanos).sum();
        let old = total as f32;
        total = total.max(own);
        global = global.max(total);
        let mut out: Vec<Field> = self
            .entries
            .iter()
            .filter(|(k, _)| child(k))
            .map(|(k, e)| Field {
                name: k[prefix.len()..].to_owned(),
                percentage: e.nanos as f64 * 100.0 / total as f64,
                global: e.nanos as f64 * 100.0 / global as f64,
                count: e.count,
            })
            .collect();
        if total as f32 > old {
            let rest = f64::from(total as f32 - old);
            out.push(Field {
                name: "unspecified".into(),
                percentage: rest * 100.0 / total as f64,
                global: rest * 100.0 / global as f64,
                count: own_count,
            });
        }
        out.sort_by(|a, b| b.percentage.total_cmp(&a.percentage).then_with(|| b.name.cmp(&a.name)));
        out.insert(
            0,
            Field {
                name: path.to_owned(),
                percentage: 100.0,
                global: total as f64 * 100.0 / global as f64,
                count: own_count,
            },
        );
        out
    }
}

/// Share of a node.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub percentage: f64,
    pub global: f64,
    pub count: u64,
}

impl Field {
    /// Node color: `(hash of the name & 0xAAAAAA) + 0xFF444444` (Java string hash).
    pub fn color(&self) -> u32 {
        let h = self.name.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)));
        ((h & 0x00AA_AAAA) as u32).wrapping_add(0xFF44_4444)
    }
}

/// Collections of the main thread: 0 = pie chart, 1 = F3+L recording, 2 = the recording's
/// current frame alone (kept when the frame is much slower than the previous one).
pub const PIE: usize = 0;
pub const RECORD: usize = 1;
pub const SINGLE: usize = 2;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static COLLECTORS: RefCell<[Option<Collector>; 3]> = const { RefCell::new([None, None, None]) };
    /// Collection apart from the three above, for the perf log: the top zones, the end of the
    /// frame and the driver stages by index
    /// (`frame_stages.rs`), created once the composition is wanted.
    static FRAME_STAGES: RefCell<Option<FrameStageClock>> = const { RefCell::new(None) };
}

fn each(f: impl Fn(&mut Collector)) {
    COLLECTORS.with(|c| {
        for col in c.borrow_mut().iter_mut().flatten() {
            f(col);
        }
    });
}

/// Runs `f` on the frame composition clock of this thread, if it exists.
fn with_frame_stages<T>(f: impl FnOnce(&mut FrameStageClock) -> T) -> Option<T> {
    FRAME_STAGES.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Any collection on: the string ones, or the frame composition.
fn any_on(c: &[Option<Collector>; 3]) -> bool {
    c.iter().any(Option::is_some) || FRAME_STAGES.with(|s| s.borrow().is_some())
}

/// Turns a collection on or off (a collection turned back on restarts from zero). Also starts the
/// frame composition the first time it is wanted (`frame_stages::enabled`, read once).
pub fn set(which: usize, on: bool) {
    if crate::frame_stages::enabled() {
        start_frame_stages();
    }
    COLLECTORS.with(|c| {
        let mut c = c.borrow_mut();
        match (on, c[which].is_some()) {
            (true, false) => c[which] = Some(Collector::new()),
            (false, true) => c[which] = None,
            _ => {}
        }
        ON.set(any_on(&c));
    });
}

/// Frame composition clock started on this thread (kept for good).
fn start_frame_stages() {
    FRAME_STAGES.with(|s| {
        s.borrow_mut().get_or_insert_with(|| FrameStageClock::new(Instant::now()));
    });
    ON.set(true);
}

/// Removes a collection (end of a recording) and returns it.
pub fn take(which: usize) -> Option<Collector> {
    COLLECTORS.with(|c| {
        let mut c = c.borrow_mut();
        let out = c[which].take();
        ON.set(any_on(&c));
        out
    })
}

/// Stage times of the frame composition since the last call, up to `now` (`None` when it is off).
pub fn take_frame_stages(now: Instant) -> Option<StageTimes> {
    if !ON.get() {
        return None;
    }
    with_frame_stages(|c| c.take(now))
}

/// Runs a driver call (`present_watch.rs`) in its own stage of the frame composition.
pub fn during_driver_stage<T>(stage: crate::present_watch::Stage, f: impl FnOnce() -> T) -> T {
    let Some(stage) = ON.get().then(|| FrameStage::of_driver(stage)).flatten() else { return f() };
    let started = with_frame_stages(|c| c.begin_driver(stage, Instant::now())).is_some();
    let out = f();
    if started {
        with_frame_stages(|c| c.end_driver(Instant::now()));
    }
    out
}

/// Reading of a collection that is on.
pub fn with<T>(which: usize, f: impl FnOnce(&Collector) -> T) -> Option<T> {
    COLLECTORS.with(|c| c.borrow()[which].as_ref().map(f))
}

/// Start of frame: `root` zone opened.
pub fn start_tick() {
    if ON.get() {
        each(Collector::start_tick);
        with_frame_stages(|c| c.start_frame(Instant::now()));
    }
}

/// End of frame: zones left open closed again.
pub fn end_tick() {
    if ON.get() {
        each(Collector::end_tick);
        with_frame_stages(|c| c.end_frame(Instant::now()));
    }
}

pub fn push(name: &str) {
    if ON.get() {
        each(|c| c.push(name));
        with_frame_stages(|c| c.push(name, Instant::now));
    }
}

pub fn pop() {
    if ON.get() {
        each(Collector::pop);
        with_frame_stages(|c| c.pop(Instant::now));
    }
}

/// Counter `name` of the open zone increased by one.
pub fn increment(name: &'static str) {
    if ON.get() {
        each(|c| c.increment(name));
    }
}

pub fn pop_push(name: &str) {
    if ON.get() {
        each(|c| {
            c.pop();
            c.push(name);
        });
        with_frame_stages(|c| {
            c.pop(Instant::now);
            c.push(name, Instant::now);
        });
    }
}

/// Depth of the open zones (to close again after a call with multiple exits).
pub fn depth() -> usize {
    if !ON.get() {
        return 0;
    }
    let collected = COLLECTORS.with(|c| c.borrow().iter().flatten().map(|c| c.stack.len()).max().unwrap_or(0));
    collected.max(with_frame_stages(|c| c.depth).unwrap_or(0))
}

/// Closes the open zones beyond `d`.
pub fn unwind(d: usize) {
    if ON.get() {
        each(|c| {
            while c.stack.len() > d {
                c.pop();
            }
        });
        with_frame_stages(|c| {
            while c.depth > d {
                c.pop(Instant::now);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_like_the_game() {
        let mut c = Collector::default();
        let e = |n| Entry { nanos: n, count: 1, max: n, min: n };
        c.entries.insert("root".into(), e(100));
        c.entries.insert(format!("root{SEP}tick"), e(30));
        c.entries.insert(format!("root{SEP}frame"), e(60));
        c.entries.insert(format!("root{SEP}frame{SEP}render"), e(40));
        let t = c.times("root");
        let names: Vec<&str> = t.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["root", "frame", "tick", "unspecified"]);
        assert!((t[1].percentage - 60.0).abs() < 1e-9 && (t[3].percentage - 10.0).abs() < 1e-6);
        let f = c.times(&format!("root{SEP}frame"));
        assert_eq!(f[0].global, 60.0);
        assert_eq!(f[1].name, "render");
        assert!((f[1].percentage - 66.666_666).abs() < 1e-4 && (f[1].global - 40.0).abs() < 1e-9);
        assert_eq!(f[2].name, "unspecified");
    }

    #[test]
    fn java_string_hash_color() {
        // Java-style text hash of "root" = 3506402 = 0x3580E2; & 0xAAAAAA = 0x2080A2; + 0xFF444444.
        let f = Field { name: "root".into(), percentage: 0.0, global: 0.0, count: 0 };
        assert_eq!(f.color(), 0xFF64_C4E6);
    }

    #[test]
    fn zones_nest_and_unwind() {
        set(PIE, true);
        start_tick();
        push("tick");
        let d = depth();
        push("a");
        push("b");
        unwind(d);
        pop();
        push("frame");
        end_tick();
        let names = with(PIE, |c| {
            let mut k: Vec<String> = c.entries.keys().map(|k| k.replace(SEP, ".")).collect();
            k.sort();
            k
        })
        .unwrap();
        assert_eq!(names, ["root", "root.frame", "root.tick", "root.tick.a", "root.tick.a.b"]);
        set(PIE, false);
        assert_eq!(depth(), 0);
    }

    /// A frame through the zone calls of `app_frame.rs`, in their order and at their depth (the
    /// end of the frame is a child of `frame`, the presentation runs inside `render`), and the
    /// presentation guard: each stage keeps its own time, the driver call and the end of the frame
    /// are taken out of the `frame` zone, the gap after the frame is outside it, and the stages add
    /// up to the time between two readings within 5 %. Fails if a driver stage stays charged to its
    /// zone, if the end of the frame is not found under `frame`, or if a zone switch is lost.
    #[test]
    fn frame_stages_follow_the_zones_and_the_driver_calls() {
        use crate::frame_stages::FrameStage;
        use std::time::Duration;
        let sleep = |ms| std::thread::sleep(Duration::from_millis(ms));
        start_frame_stages();
        let started = Instant::now();
        let _ = take_frame_stages(started);
        start_tick();
        push("scheduledPacketProcessing");
        sleep(3);
        pop_push("scheduledExecutables");
        pop_push("mouse");
        pop();
        push("tick");
        pop();
        push("sound");
        pop_push("frame");
        push("update");
        pop_push("extract");
        sleep(2);
        pop_push("render");
        crate::present_watch::during(crate::present_watch::Stage::Present, || sleep(4));
        pop_push("fpsUpdate");
        sleep(2);
        end_tick();
        sleep(1);
        let now = Instant::now();
        let ns = take_frame_stages(now).expect("frame stages on");
        let ms = |s: FrameStage| ns[s as usize] as f64 / 1e6;
        let interval = now.duration_since(started).as_secs_f64() * 1e3;
        let sum: f64 = FrameStage::ALL.iter().map(|&s| ms(s)).sum();
        assert!((sum - interval).abs() <= 0.05 * interval, "stages {sum:.3} ms for a frame of {interval:.3} ms");
        // Lower bounds: the sleeps, enough to catch a stage charged to another one. The upper
        // margin is wide: a loaded machine or a shared runner wakes a sleeper late.
        let within = |s: FrameStage, low: f64| (low..low + 20.0).contains(&ms(s));
        assert!(within(FrameStage::Packets, 3.0), "packets {:.3} ms", ms(FrameStage::Packets));
        assert!(within(FrameStage::Frame, 2.0), "frame {:.3} ms", ms(FrameStage::Frame));
        assert!(within(FrameStage::Present, 4.0), "present {:.3} ms", ms(FrameStage::Present));
        assert!(within(FrameStage::FpsUpdate, 2.0), "end of frame {:.3} ms", ms(FrameStage::FpsUpdate));
        assert!(within(FrameStage::OutsideFrame, 1.0), "outside {:.3} ms", ms(FrameStage::OutsideFrame));
        // Depth and unwinding with no string collection on (only the frame stages).
        start_tick();
        push("tick");
        let d = depth();
        assert_eq!(d, 2);
        push("a");
        push("b");
        unwind(d);
        assert_eq!(depth(), 2);
        end_tick();
    }

    /// Cost of the frame composition for a frame of 8 top zones, 30 nested zones and 4 driver
    /// calls, on and off: `cargo test --release -p rc-app --bin rustcraft -- --ignored
    /// frame_stage_cost --nocapture`.
    #[test]
    #[ignore]
    fn frame_stage_cost() {
        const FRAMES: u32 = 100_000;
        let frame = || {
            start_tick();
            for top in ["scheduledPacketProcessing", "scheduledExecutables", "mouse", "tick", "sound", "frame"] {
                push(top);
                for _ in 0..5 {
                    push("nested");
                    pop();
                }
                pop();
            }
            push("frame");
            push("render");
            for stage in [crate::present_watch::Stage::Acquire, crate::present_watch::Stage::Submit] {
                std::hint::black_box(during_driver_stage(stage, || 1));
            }
            pop_push("fpsUpdate");
            std::hint::black_box(take_frame_stages(Instant::now()));
            end_tick();
        };
        let time = || {
            let started = Instant::now();
            for _ in 0..FRAMES {
                frame();
            }
            started.elapsed().as_nanos() as f64 / f64::from(FRAMES)
        };
        let off = time();
        start_frame_stages();
        let on = time();
        eprintln!("[bench] frame_stages per_frame_ns off {off:.1}");
        eprintln!("[bench] frame_stages per_frame_ns on {on:.1}");
    }
}
