//! F3+L recording (`debug-f3.md` § 6.4): ten seconds of profiler and per-frame readings,
//! then archive `debug/profiling/<date>-<world or server>-<version>.zip` in the
//! game folder, like the game: `system.txt`, `client/options.txt`, `client/metrics/*.csv`
//! (one table per category, one row per frame: ticking, chunk rendering, gpu, jvm, cpu),
//! `client/deviations/ticktime/<frame>@<date>.txt` (profile of each frame three times slower
//! than the previous one), `client/profiling.txt` (zone tree and counters); in singleplayer
//! the same under `server/` for the integrated server's ticks.

use std::io::Write;
use std::time::Instant;

use rc_text::Component;

use crate::profiler::{self, Collector, SEP};

/// Conversion of a double to text, Java style: decimal notation between 1e-3 and 1e7, scientific otherwise
/// (`1.6123456E7`), minimal digits in both cases.
pub fn java_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let a = v.abs();
    if (1e-3..1e7).contains(&a) {
        let s = format!("{v}");
        if s.contains('.') { s } else { format!("{s}.0") }
    } else {
        let s = format!("{v:e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
        format!("{m}E{e}")
    }
}

/// Java's `String.format("%.0f")` (rounding to nearest, half up).
fn java_0f(v: f64) -> String {
    format!("{}", (v + 0.5).floor() as i64)
}

/// `profiling.txt`: header, durations, then the zone tree (`[nn] |   name(n/per tick) - % local/% global`).
pub fn profile_text(c: &Collector, nanos: u64, ticks: u64, version: &str) -> String {
    let mut out = crate::crash_report::header(crate::crash_report::Kind::Profile, crate::crash_report::nanos());
    out.push_str(&format!("Version: {version}\n"));
    out.push_str(&format!("Time span: {} ms\n", nanos / 1_000_000));
    out.push_str(&format!("Tick span: {ticks} ticks\n"));
    let tps = ticks as f32 / (nanos as f32 / 1.0e9);
    out.push_str(&format!("// This is approximately {tps:.2} ticks per second. It should be 20 ticks per second\n\n"));
    out.push_str("--- BEGIN PROFILE DUMP ---\n\n");
    dump(c, 0, "root", ticks.max(1), &mut out);
    out.push_str("--- END PROFILE DUMP ---\n\n");
    counter_dump(c, ticks.max(1), &mut out);
    out
}

fn indent(depth: usize, out: &mut String) {
    out.push_str(&format!("[{depth:02}] "));
    for _ in 0..depth {
        out.push_str("|   ");
    }
}

fn dump(c: &Collector, depth: usize, path: &str, ticks: u64, out: &mut String) {
    // Counters of the node, before its children: `#name total/per tick` (whole division).
    for (name, v) in c.counters.get(path).into_iter().flatten() {
        indent(depth, out);
        out.push_str(&format!("#{name} {v}/{}\n", v / ticks));
    }
    let results = c.times(path);
    if results.len() < 3 {
        return;
    }
    for r in &results[1..] {
        indent(depth, out);
        out.push_str(&format!(
            "{}({}/{}) - {:.2}%/{:.2}%\n",
            r.name,
            r.count,
            java_0f(f64::from(r.count as f32 / ticks as f32)),
            r.percentage,
            r.global
        ));
        if r.name != "unspecified" {
            dump(c, depth + 1, &format!("{path}{SEP}{}", r.name), ticks, out);
        }
    }
}

/// Tree of one counter: own value and value of the whole subtree, children by zone name.
#[derive(Default)]
struct CounterNode {
    own: u64,
    total: u64,
    children: Vec<(String, CounterNode)>,
}

impl CounterNode {
    fn add<'a>(&mut self, mut path: impl Iterator<Item = &'a str>, v: u64) {
        self.total += v;
        match path.next() {
            None => self.own += v,
            Some(seg) => {
                let i = match self.children.iter().position(|(n, _)| n == seg) {
                    Some(i) => i,
                    None => {
                        self.children.push((seg.to_owned(), CounterNode::default()));
                        self.children.len() - 1
                    }
                };
                self.children[i].1.add(path, v);
            }
        }
    }

    /// `[dd] name total:own/subtree average: own/subtree` per tick, children by decreasing subtree.
    fn write(&self, depth: usize, name: &str, ticks: u64, out: &mut String) {
        indent(depth, out);
        let (o, t) = (self.own, self.total);
        out.push_str(&format!("{name} total:{o}/{t} average: {}/{}\n", o / ticks, t / ticks));
        let mut kids: Vec<&(String, CounterNode)> = self.children.iter().collect();
        kids.sort_by_key(|k| std::cmp::Reverse(k.1.total));
        for (n, k) in kids {
            k.write(depth + 1, n, ticks, out);
        }
    }
}

/// Counter dump (after the zone tree, only if a counter was increased): one tree per counter
/// spread over the zones where it was increased.
fn counter_dump(c: &Collector, ticks: u64, out: &mut String) {
    let mut trees: Vec<(&str, CounterNode)> = Vec::new();
    let mut paths: Vec<&String> = c.counters.keys().collect();
    paths.sort();
    for path in paths {
        for &(name, v) in &c.counters[path] {
            let i = match trees.iter().position(|(n, _)| *n == name) {
                Some(i) => i,
                None => {
                    trees.push((name, CounterNode::default()));
                    trees.len() - 1
                }
            };
            trees[i].1.add(path.split(SEP), v);
        }
    }
    if trees.is_empty() {
        return;
    }
    out.push_str("--- BEGIN COUNTER DUMP ---\n\n");
    for (name, tree) in &trees {
        out.push_str(&format!("-- Counter: {name} --\n"));
        if let Some((_, root)) = tree.children.iter().find(|(n, _)| n == "root") {
            root.write(0, "root", ticks, out);
        }
        out.push_str("\n\n");
    }
    out.push_str("--- END COUNTER DUMP ---\n\n");
}

/// Table of a readings category (`@tick` then one column per reading; CRLF line endings).
fn csv(columns: &[&str], rows: &[(usize, Vec<f64>)]) -> String {
    let mut out = format!("@tick,{}\r\n", columns.join(","));
    for (tick, vals) in rows {
        out.push_str(&tick.to_string());
        for v in vals {
            out.push(',');
            out.push_str(&java_double(*v));
        }
        out.push_str("\r\n");
    }
    out
}

/// Options for the report: `name: value`, sorted, without the keys or the volumes.
fn options_dump(text: &str) -> String {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| !k.starts_with("key_") && !k.starts_with("soundCategory_") && *k != "version")
        .map(|(k, val)| format!("{k}: {val}"))
        .collect();
    v.sort();
    v.join("\r\n")
}

/// Load of each logical processor (`cpu#n`), read again at most every half second like the
/// game's sampler: share of non-idle time since the previous reading, in percent.
pub struct CpuStats {
    prev: Vec<(u64, u64)>,
    load: Vec<f64>,
    polled: Option<Instant>,
}

impl CpuStats {
    pub fn new() -> Option<CpuStats> {
        let prev = crate::debug_sys::cpu_times();
        (!prev.is_empty()).then(|| CpuStats { load: vec![0.0; prev.len()], prev, polled: None })
    }

    fn sample(&mut self) -> Vec<f64> {
        if self.polled.is_none_or(|t| t.elapsed().as_millis() > 501) {
            let now = crate::debug_sys::cpu_times();
            self.load = busy_share(&self.prev, &now).unwrap_or_else(|| self.load.clone());
            self.prev = now;
            self.polled = Some(Instant::now());
        }
        self.load.clone()
    }
}

/// Busy share (%) of each processor between two readings; `None` if they do not match.
fn busy_share(prev: &[(u64, u64)], now: &[(u64, u64)]) -> Option<Vec<f64>> {
    (prev.len() == now.len()).then(|| {
        now.iter()
            .zip(prev)
            .map(|(n, p)| {
                let total = n.1.saturating_sub(p.1);
                if total == 0 { 0.0 } else { n.0.saturating_sub(p.0) as f64 / total as f64 * 100.0 }
            })
            .collect()
    })
}

/// Threshold test of the tick time: an increase of 200 % or more over the previous value.
fn deviates(prev: &mut Option<f64>, v: f64) -> bool {
    let hit = prev.is_some_and(|p| v > p && (v - p) / p >= 2.0);
    *prev = Some(v);
    hit
}

/// Date of a deviation file: `yyyy-MM-dd_HH.mm.ss.SSS`, local time.
fn deviation_stamp(ms: i64) -> String {
    let (y, mo, d, h, mi, s) = rc_sim::storage::local_datetime(ms);
    format!("{y:04}-{mo:02}-{d:02}_{h:02}.{mi:02}.{s:02}.{:03}", ms.rem_euclid(1000))
}

/// Files of one side of the archive (`client` or `server`): metrics tables, profiles of the
/// slow ticks, whole profile; in the order of a sorted folder walk.
fn side_files(
    side: &str,
    tables: Vec<(&str, String)>,
    deviations: &[(usize, i64, String)],
    profile: String,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = tables.into_iter().map(|(n, t)| (format!("{side}/metrics/{n}.csv"), t)).collect();
    for (tick, ms, text) in deviations {
        out.push((format!("{side}/deviations/ticktime/{tick}@{}.txt", deviation_stamp(*ms)), text.clone()));
    }
    out.push((format!("{side}/profiling.txt"), profile));
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `server/` of a singleplayer archive, from the integrated server's full ticks (duration ns,
/// date ms): tick times, slow ticks and profile (time and tick spans; the integrated server
/// keeps no zone tree).
fn server_files(ticks: &[(i64, i64)], version: &str) -> Vec<(String, String)> {
    let rows: Vec<(usize, Vec<f64>)> = ticks.iter().enumerate().map(|(i, t)| (i + 1, vec![t.0 as f64])).collect();
    let empty = Collector::default();
    let mut prev = None;
    let deviations: Vec<(usize, i64, String)> = ticks
        .iter()
        .enumerate()
        .filter(|(_, t)| deviates(&mut prev, t.0 as f64))
        .map(|(i, t)| (i + 1, t.1, profile_text(&empty, t.0.max(0) as u64, 1, version)))
        .collect();
    let span = ticks.iter().map(|t| t.0.max(0) as u64).sum();
    let profile = profile_text(&empty, span, ticks.len() as u64, version);
    side_files("server", vec![("ticking", csv(&["ticktime"], &rows))], &deviations, profile)
}

impl crate::App {
    /// Start of frame during the recording: frame duration timer.
    pub(crate) fn profiling_frame_start(&mut self) {
        if let Some(p) = &mut self.debug.profiling {
            p.frame_start = Some(Instant::now());
        }
    }

    /// End of frame: readings of the frame, then end of the recording after 10 s or on request.
    pub(crate) fn profiling_frame_end(&mut self) {
        let gpu = self.debug.gpu_percent;
        let view = f64::from(self.options.i("renderDistance"));
        let (total, drawn) = self.renderer.as_ref().map_or((0.0, 0.0), |r| (r.stats().0 as f64, r.drawn.0 as f64));
        let Some(p) = &mut self.debug.profiling else { return };
        // The frame alone, collected since the end of the previous one.
        let single = profiler::take(profiler::SINGLE);
        // Starting frame (begun before F3+L): no reading, like the game (tick 0).
        if let Some(t) = p.frame_start {
            let ticktime = t.elapsed().as_nanos() as f64;
            let heap = (crate::debug_sys::process_memory().1 / (1024 * 1024)) as f64;
            p.rows.push([ticktime, total, drawn, view, gpu, heap]);
            if let Some(c) = &mut p.cpu {
                p.cpu_rows.push(c.sample());
            }
            if deviates(&mut p.last_frame, ticktime)
                && let Some(s) = &single
            {
                let nanos = s.start.map_or(0, |t0| t0.elapsed().as_nanos() as u64);
                let text = profile_text(s, nanos, s.frames, &self.data.version);
                p.deviations.push((p.rows.len(), rc_sim::progress::now_ms(), text));
            }
        }
        if p.stop || p.start.elapsed().as_secs_f64() > 10.0 {
            self.profiling_finish();
        } else {
            profiler::set(profiler::SINGLE, true);
        }
    }

    /// Disconnection during the recording: cancelled with no message or archive (empty results).
    pub(crate) fn profiling_cancel(&mut self) {
        if self.debug.profiling.take().is_some() {
            let _ = profiler::take(profiler::RECORD);
            let _ = profiler::take(profiler::SINGLE);
        }
    }

    /// End of the recording: stop message, archive written, link to its folder.
    fn profiling_finish(&mut self) {
        let Some(p) = self.debug.profiling.take() else { return };
        let Some(col) = profiler::take(profiler::RECORD) else { return };
        let end = col.end.unwrap_or_else(Instant::now);
        let nanos = col.start.map_or(0, |s| end.duration_since(s).as_nanos() as u64);
        let ticks = col.frames;
        let secs = nanos as f64 / 1e9;
        let tps = ticks as f64 / secs;
        self.debug_feedback(
            false,
            Component::translate(
                "commands.debug.stopped",
                vec![
                    rc_text::Arg::Text(format!("{secs:.2}")),
                    rc_text::Arg::Text(ticks.to_string()),
                    rc_text::Arg::Text(format!("{tps:.2}")),
                ],
            ),
        );
        let version = self.data.version.clone();
        let place = if self.debug.logs.place.is_empty() { "unknown".to_owned() } else { self.debug.logs.place.clone() };
        let game = crate::servers::game_dir();
        let dir = game.join("debug").join("profiling");
        let _ = std::fs::create_dir_all(&dir);
        let base = format!("{}-{place}-{version}", crate::debug_keys::file_stamp());
        let name = rc_sim::storage::find_available_name(&dir, &base, ".zip");
        let rows: Vec<(usize, [f64; 6])> = p.rows.iter().enumerate().map(|(i, r)| (i + 1, *r)).collect();
        let pick = |cols: &[usize]| -> Vec<(usize, Vec<f64>)> {
            rows.iter().map(|(t, r)| (*t, cols.iter().map(|&c| r[c]).collect())).collect()
        };
        let mut tables = vec![
            ("ticking", csv(&["ticktime"], &pick(&[0]))),
            ("chunk rendering", csv(&["totalChunks", "renderedChunks", "lastViewDistance"], &pick(&[1, 2, 3]))),
            ("gpu", csv(&["gpuUtilization"], &pick(&[4]))),
            ("jvm", csv(&["heap MiB"], &pick(&[5]))),
        ];
        if let Some(n) = p.cpu_rows.first().map(Vec::len) {
            let names: Vec<String> = (0..n).map(|i| format!("cpu#{i}")).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let cpu: Vec<(usize, Vec<f64>)> = p.cpu_rows.iter().enumerate().map(|(i, r)| (i + 1, r.clone())).collect();
            tables.push(("cpu", csv(&names, &cpu)));
        }
        let mut files: Vec<(String, String)> = vec![
            ("system.txt".into(), self.system_report()),
            ("client/options.txt".into(), options_dump(&self.options.to_text())),
        ];
        let profile = profile_text(&col, nanos, ticks, &version);
        files.extend(side_files("client", tables, &p.deviations, profile));
        if self.solo.is_some() {
            files.extend(server_files(&p.server_ticks, &version));
        }
        let path = dir.join(&name);
        if write_zip(&path, &files).is_err() {
            return;
        }
        // Path relative to the game folder, system separator; click: opens the folder.
        let shown = std::path::Path::new("debug").join("profiling").join(&name);
        let mut link = Component::text(shown.to_string_lossy().into_owned());
        link.style.underlined = Some(true);
        let abs = std::path::absolute(&dir).unwrap_or(dir);
        link.style.click = Some(std::sync::Arc::new(rc_text::ClickEvent {
            action: "open_file".into(),
            value: abs.to_string_lossy().into_owned(),
            nbt: None,
        }));
        self.debug_feedback(false, Component::translate("debug.profiling.stop", vec![rc_text::Arg::Component(link)]));
    }

    /// `system.txt`: system details known to the client (`key: value` per line).
    fn system_report(&mut self) -> String {
        self.system_lines().iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\r\n")
    }

    /// System details (`system.txt`, section of the crash reports).
    pub(crate) fn system_lines(&mut self) -> Vec<(String, String)> {
        let version = self.data.version.clone();
        let mut lines = vec![
            ("Minecraft Version".to_owned(), version.clone()),
            ("Minecraft Version ID".to_owned(), version.clone()),
            ("Operating System".to_owned(), format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)),
            ("CPUs".to_owned(), std::thread::available_parallelism().map_or(1, std::num::NonZero::get).to_string()),
            ("Launched Version".to_owned(), version),
        ];
        if let Some(r) = &self.renderer {
            let (w, h) = r.size();
            let info = &r.adapter_info;
            lines.push(("Backend library".into(), format!("wgpu ({:?})", info.backend)));
            lines.push(("Window size".into(), format!("{w}x{h}")));
            lines.push(("Graphics Device".into(), info.name.clone()));
            let t = &r.gpu_text;
            lines.push(("Graphics Backend".into(), t.backend.into()));
            lines.push(("Graphics Vendor".into(), t.vendor.clone()));
            lines.push(("Graphics Drivers".into(), t.driver.clone()));
        }
        lines.push(("Type".into(), "Client".into()));
        let rd = self.options.i("renderDistance");
        lines.push(("Render Distance".into(), format!("{rd}/{rd} chunks")));
        lines.push(("Current Language".into(), self.options.s("lang").to_owned()));
        lines.push(("CPU".into(), self.debug.cpu().to_owned()));
        lines
    }
}

/// Archive ("deflate" compression), entries in the given order.
fn write_zip(path: &std::path::Path, files: &[(String, String)]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in files {
        zip.start_file(name.as_str(), opts).map_err(std::io::Error::other)?;
        zip.write_all(text.as_bytes())?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_doubles() {
        assert_eq!(java_double(16_712_300.0), "1.67123E7");
        assert_eq!(java_double(123.0), "123.0");
        assert_eq!(java_double(0.5), "0.5");
        assert_eq!(java_double(0.0001), "1.0E-4");
        assert_eq!(java_double(9_999_999.0), "9999999.0");
    }

    #[test]
    fn profile_dump_tree() {
        let mut c = Collector::default();
        let e = |n, k| profiler::Entry { nanos: n, count: k, max: n, min: n };
        c.entries.insert("root".into(), e(1000, 10));
        c.entries.insert(format!("root{SEP}frame"), e(600, 10));
        c.entries.insert(format!("root{SEP}tick"), e(300, 15));
        let t = profile_text(&c, 2_000_000_000, 10, "26.3");
        assert!(t.contains("Time span: 2000 ms\nTick span: 10 ticks\n"));
        assert!(t.contains("approximately 5.00 ticks per second"));
        assert!(t.contains("[00] frame(10/1) - 60.00%/60.00%\n"), "{t}");
        assert!(t.contains("[00] tick(15/2) - 30.00%/30.00%\n"), "{t}");
        assert!(t.contains("[00] unspecified(10/1) - 10.00%/10.00%\n"), "{t}");
        assert!(!t.contains("COUNTER"), "no counter: no counter dump");
    }

    /// Counters: listed under their zone before its children, then one tree per counter.
    #[test]
    fn profile_counters_and_counter_dump() {
        let mut c = Collector::default();
        let e = |n, k| profiler::Entry { nanos: n, count: k, max: n, min: n };
        c.entries.insert("root".into(), e(1000, 10));
        c.entries.insert(format!("root{SEP}tick"), e(300, 10));
        c.entries.insert(format!("root{SEP}frame"), e(600, 10));
        c.counters.insert(format!("root{SEP}tick"), vec![("clientTick", 25)]);
        let t = profile_text(&c, 2_000_000_000, 10, "26.3");
        assert!(t.contains("[00] tick(10/1) - 30.00%/30.00%\n[01] |   #clientTick 25/2\n"), "{t}");
        let dump = "--- END PROFILE DUMP ---\n\n--- BEGIN COUNTER DUMP ---\n\n-- Counter: clientTick --\n\
                    [00] root total:0/25 average: 0/2\n[01] |   tick total:25/25 average: 2/2\n\n\n\
                    --- END COUNTER DUMP ---\n\n";
        assert!(t.ends_with(dump), "{t}");
    }

    /// Slow tick: 200 % more than the previous one or beyond; the first never counts.
    #[test]
    fn tick_time_threshold() {
        let mut prev = None;
        let hits: Vec<bool> = [10.0, 29.0, 90.0, 90.0, 5.0, 15.0, 0.0, 1.0].iter().map(|&v| deviates(&mut prev, v)).collect();
        assert_eq!(hits, [false, false, true, false, false, true, false, true]);
    }

    /// Processor load between two readings; the server side of a singleplayer archive.
    #[test]
    fn cpu_share_and_server_side() {
        assert_eq!(busy_share(&[(10, 100), (0, 0)], &[(60, 200), (0, 0)]), Some(vec![50.0, 0.0]));
        assert_eq!(busy_share(&[(1, 2)], &[]), None);
        let files = server_files(&[(50_000_000, 0), (50_000_000, 0), (150_000_000, 1_234)], "26.3");
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        let slow = format!("server/deviations/ticktime/3@{}.txt", deviation_stamp(1_234));
        assert_eq!(names, [slow.as_str(), "server/metrics/ticking.csv", "server/profiling.txt"]);
        assert_eq!(files[1].1, "@tick,ticktime\r\n1,5.0E7\r\n2,5.0E7\r\n3,1.5E8\r\n");
        assert!(files[2].1.contains("Time span: 250 ms\nTick span: 3 ticks\n"), "{}", files[2].1);
        assert!(files[0].1.contains("Time span: 150 ms\nTick span: 1 ticks\n"));
    }
}
