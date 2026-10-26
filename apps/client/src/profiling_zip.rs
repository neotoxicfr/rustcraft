//! F3+L recording (`debug-f3.md` § 6.4): ten seconds of profiler and per-frame readings,
//! then archive `debug/profiling/<date>-<world or server>-<version>.zip` in the
//! game folder, like the game: `system.txt`, `client/options.txt`, `client/metrics/*.csv`
//! (one table per category, one row per frame) and `client/profiling.txt` (zone tree).

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
    out
}

fn dump(c: &Collector, depth: usize, path: &str, ticks: u64, out: &mut String) {
    let results = c.times(path);
    if results.len() < 3 {
        return;
    }
    for r in &results[1..] {
        out.push_str(&format!("[{depth:02}] "));
        for _ in 0..depth {
            out.push_str("|   ");
        }
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
        // Starting frame (begun before F3+L): no reading, like the game (tick 0).
        if let Some(t) = p.frame_start {
            p.rows.push([t.elapsed().as_nanos() as f64, total, drawn, view, gpu]);
        }
        if p.stop || p.start.elapsed().as_secs_f64() > 10.0 {
            self.profiling_finish();
        }
    }

    /// Disconnection during the recording: cancelled with no message or archive (empty results).
    pub(crate) fn profiling_cancel(&mut self) {
        if self.debug.profiling.take().is_some() {
            let _ = profiler::take(profiler::RECORD);
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
        let rows: Vec<(usize, [f64; 5])> = p.rows.iter().enumerate().map(|(i, r)| (i + 1, *r)).collect();
        let pick = |cols: &[usize]| -> Vec<(usize, Vec<f64>)> {
            rows.iter().map(|(t, r)| (*t, cols.iter().map(|&c| r[c]).collect())).collect()
        };
        let mut files: Vec<(String, String)> = vec![
            ("system.txt".into(), self.system_report()),
            ("client/options.txt".into(), options_dump(&self.options.to_text())),
            ("client/metrics/ticking.csv".into(), csv(&["ticktime"], &pick(&[0]))),
            (
                "client/metrics/chunk rendering.csv".into(),
                csv(&["totalChunks", "renderedChunks", "lastViewDistance"], &pick(&[1, 2, 3])),
            ),
            ("client/metrics/gpu.csv".into(), csv(&["gpuUtilization"], &pick(&[4]))),
        ];
        files.push(("client/profiling.txt".into(), profile_text(&col, nanos, ticks, &version)));
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
            lines.push(("Graphics Backend".into(), format!("{:?}", info.backend)));
            lines.push(("Graphics Vendor".into(), format!("0x{:04x}", info.vendor)));
            lines.push(("Graphics Drivers".into(), format!("{} {}", info.driver, info.driver_info)));
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
    }
}
