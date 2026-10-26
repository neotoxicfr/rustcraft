//! Hierarchical profiler of the frame (`debug-f3.md` § 7.4): nested named zones
//! (`root`, `tick`, `frame`, …), cumulated duration and number of passes per path. It only measures
//! when it is on (F3+1 pie chart, F3+L recording); when off, each call boils down
//! to reading a thread boolean.
//!
//! Like the game: paths separated by the 0x1E character, cumulation since turning on (no reset
//! per frame), "live" reading of the results during the current frame.

use hashbrown::HashMap;
use std::cell::{Cell, RefCell};
use std::time::Instant;

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

/// Collections of the main thread: 0 = pie chart, 1 = F3+L recording.
pub const PIE: usize = 0;
pub const RECORD: usize = 1;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static COLLECTORS: RefCell<[Option<Collector>; 2]> = const { RefCell::new([None, None]) };
}

fn each(f: impl Fn(&mut Collector)) {
    COLLECTORS.with(|c| {
        for col in c.borrow_mut().iter_mut().flatten() {
            f(col);
        }
    });
}

/// Turns a collection on or off (a collection turned back on restarts from zero).
pub fn set(which: usize, on: bool) {
    COLLECTORS.with(|c| {
        let mut c = c.borrow_mut();
        match (on, c[which].is_some()) {
            (true, false) => c[which] = Some(Collector::new()),
            (false, true) => c[which] = None,
            _ => {}
        }
        ON.set(c.iter().any(Option::is_some));
    });
}

/// Removes a collection (end of a recording) and returns it.
pub fn take(which: usize) -> Option<Collector> {
    COLLECTORS.with(|c| {
        let mut c = c.borrow_mut();
        let out = c[which].take();
        ON.set(c.iter().any(Option::is_some));
        out
    })
}

/// Reading of a collection that is on.
pub fn with<T>(which: usize, f: impl FnOnce(&Collector) -> T) -> Option<T> {
    COLLECTORS.with(|c| c.borrow()[which].as_ref().map(f))
}

/// Start of frame: `root` zone opened.
pub fn start_tick() {
    if ON.get() {
        each(Collector::start_tick);
    }
}

/// End of frame: zones left open closed again.
pub fn end_tick() {
    if ON.get() {
        each(Collector::end_tick);
    }
}

pub fn push(name: &str) {
    if ON.get() {
        each(|c| c.push(name));
    }
}

pub fn pop() {
    if ON.get() {
        each(Collector::pop);
    }
}

pub fn pop_push(name: &str) {
    if ON.get() {
        each(|c| {
            c.pop();
            c.push(name);
        });
    }
}

/// Depth of the open zones (to close again after a call with multiple exits).
pub fn depth() -> usize {
    if !ON.get() {
        return 0;
    }
    COLLECTORS.with(|c| c.borrow().iter().flatten().map(|c| c.stack.len()).max().unwrap_or(0))
}

/// Closes the open zones beyond `d`.
pub fn unwind(d: usize) {
    if ON.get() {
        each(|c| {
            while c.stack.len() > d {
                c.pop();
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
}
