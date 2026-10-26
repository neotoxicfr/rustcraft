//! Graphs of the debug screen (`debug-f3.md` § 7): frame times (F3+2) and server tick,
//! bandwidth and ping (F3+3). Drawn at the bottom of the screen, at the F3 text scale.

use crate::debug_screen::{Chart, DebugScreen, RING, Ring};
use crate::gui::Gui;

const BG: u32 = 0x9050_5050;
const TEXT: u32 = 0xFFE0_E0E0;

/// Three-point gradient, linear per channel in sRGB (`c0 + floor(t·(c1 − c0))`).
fn gradient(v: f64, min: f64, c0: u32, mid: f64, c1: u32, max: f64, c2: u32) -> u32 {
    let v = v.clamp(min, max);
    let (t, a, b) = if v < mid { ((v - min) / (mid - min), c0, c1) } else { ((v - mid) / (max - mid), c1, c2) };
    let t = t as f32;
    let mut out = 0u32;
    for s in [24, 16, 8, 0] {
        let (x, y) = (((a >> s) & 0xff) as i32, ((b >> s) & 0xff) as i32);
        let c = x + (t * (y - x) as f32).floor() as i32;
        out |= (c.clamp(0, 255) as u32) << s;
    }
    out
}

fn width_of(gui: &Gui, s: &str) -> f32 {
    rc_text::width(gui, &crate::gui::plain(s)) as f32
}

/// Label on a background (text offset by 1, no shadow).
fn label(gui: &mut Gui, q: &wgpu::Queue, s: &str, x: f32, y: f32) {
    let chars = crate::gui::plain(s);
    let w = rc_text::width(gui, &chars) as f32;
    gui.fill(x, y, x + w + 1.0, y + 9.0, BG);
    gui.text(q, &chars, x + 1.0, y + 1.0, TEXT, false, 0);
}

fn hline(gui: &mut Gui, x0: f32, x1: f32, y: f32, color: u32) {
    gui.fill(x0, y, x1 + 1.0, y + 1.0, color);
}

/// A graph: background, bars (`bars(i)` = list of (bottom, top, color) in pixels above the
/// bottom), frame, min / average / max (`agg(i)`, formatted by `fmt`).
#[allow(clippy::too_many_arguments)]
fn chart(
    gui: &mut Gui,
    q: &wgpu::Queue,
    left: f32,
    width: f32,
    bottom: f32,
    len: usize,
    bars: &dyn Fn(usize) -> Vec<(i32, i32, u32)>,
    agg: &dyn Fn(usize) -> i64,
    fmt: &dyn Fn(f64) -> String,
) {
    gui.fill(left, bottom - 60.0, left + width, bottom, BG);
    let start = RING.saturating_sub(width as usize - 2);
    let count = len.saturating_sub(start);
    let (mut min, mut max, mut sum) = (i64::MAX, i64::MIN, 0i64);
    for i in 0..count {
        let x = left + i as f32 + 1.0;
        let v = agg(start + i);
        min = min.min(v);
        max = max.max(v);
        sum += v;
        for (lo, hi, color) in bars(start + i) {
            // Corners put back in order like the game's fill: a negative height (the
            // "other" part < 0 of the ticks graph) is drawn downward.
            if hi != lo {
                gui.fill(x, bottom - hi.max(lo) as f32, x + 1.0, bottom - hi.min(lo) as f32, color);
            }
        }
    }
    hline(gui, left, left + width - 1.0, bottom - 60.0, 0xFFFF_FFFF);
    hline(gui, left, left + width - 1.0, bottom - 1.0, 0xFFFF_FFFF);
    gui.fill(left, bottom - 60.0, left + 1.0, bottom, 0xFFFF_FFFF);
    gui.fill(left + width - 1.0, bottom - 60.0, left + width, bottom, 0xFFFF_FFFF);
    if count > 0 {
        let y = bottom - 69.0;
        let lo = format!("{} min", fmt(min as f64));
        let avg = format!("{} avg", fmt(sum as f64 / count as f64));
        let hi = format!("{} max", fmt(max as f64));
        gui.text(q, &crate::gui::plain(&lo), left + 2.0, y, TEXT, true, 0);
        let aw = width_of(gui, &avg);
        gui.text(q, &crate::gui::plain(&avg), left + (width / 2.0).floor() - (aw / 2.0).floor(), y, TEXT, true, 0);
        let hw = width_of(gui, &hi);
        gui.text(q, &crate::gui::plain(&hi), left + width - hw - 2.0, y, TEXT, true, 0);
    }
}

/// Logarithmic height of the bandwidth (top = 1 MiB/s).
fn bw_height(bps: f64) -> i32 {
    ((bps + 1.0).ln() * 60.0 / 1_048_576f64.ln()).round() as i32
}

fn bw_text(bps: f64) -> String {
    if bps >= 1_048_576.0 {
        format!("{:.1} MiB/s", bps / 1_048_576.0)
    } else if bps >= 1024.0 {
        format!("{:.1} KiB/s", bps / 1024.0)
    } else {
        format!("{} B/s", bps.floor() as i64)
    }
}

fn ms_text(ms: f64) -> String {
    format!("{} ms", ms.round() as i64)
}

/// Samples of the network and server graphs, kept per tick.
#[derive(Default)]
pub struct ChartLogs {
    pub ping: Ring<i64>,
    pub bandwidth: Ring<i64>,
    pub tick: Ring<[i64; 4]>,
    pub bytes_prev: Option<u64>,
    /// Subscription to the server tick times in progress.
    pub subscribed: bool,
    /// Milliseconds per tick of the current rate.
    pub mspt: f32,
    /// Integrated server (singleplayer, open to the LAN or not): no bandwidth graph.
    pub local: bool,
    /// Last light map sent to the GPU (16 × 16 RGBA, row = sky), for F3+4.
    pub lightmap: Option<Vec<u8>>,
    /// Pie chart node shown (profiler path, `root` at the start), and results ready
    /// (collection turned on at the previous frame, like the game).
    pub pie_path: String,
    pub pie_ready: bool,
    /// World name (singleplayer) or server entry, for the F3+L archive name.
    pub place: String,
    /// Integrated server: smoothed tick time (ms, `0.8 × old + 0.2 × method`), F3 line.
    pub smoothed_tick_ms: f32,
}

/// Pie chart percentage (`##0.00`, NaN and infinities as in Java).
fn pct_text(v: f64) -> String {
    if v.is_nan() {
        "NaN%".into()
    } else if v.is_infinite() {
        if v > 0.0 { "\u{221e}%".into() } else { "-\u{221e}%".into() }
    } else {
        format!("{v:.2}%")
    }
}

/// Splitting of the node name on dots: first line at most `first`,
/// following ones at most `rest` pixels; a piece that is too long alone takes its line.
fn split_node_name(gui: &Gui, name: &str, first: f32, rest: f32) -> Vec<String> {
    let parts: Vec<&str> = name.split('.').collect();
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < parts.len() {
        let piece = if i != 0 { format!(".{}", parts[i]) } else { parts[i].to_owned() };
        let candidate = format!("{cur}{piece}");
        let max = if lines.is_empty() { first } else { rest };
        if width_of(gui, &candidate) > max {
            if cur.is_empty() {
                lines.push(piece);
                i += 1;
            } else {
                lines.push(std::mem::take(&mut cur));
            }
        } else {
            cur = candidate;
            i += 1;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Channels × 128 / 255.
fn shade(c: u32) -> u32 {
    let ch = |s: u32| ((c >> s) & 0xff) * (if s == 24 { 255 } else { 128 }) / 255;
    ch(24) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// Fills a quadrilateral clipped to the vertical band `[ymin, ymax]` (the game draws the disc
/// into a texture of this height: what overflows is lost).
fn fill_quad_banded(gui: &mut Gui, p: [[f32; 2]; 4], ymin: f32, ymax: f32, color: u32) {
    let mut poly: Vec<[f32; 2]> = p.to_vec();
    for (keep, edge) in [(true, ymin), (false, ymax)] {
        let inside = |q: [f32; 2]| if keep { q[1] >= edge } else { q[1] <= edge };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if inside(a) {
                out.push(a);
            }
            if inside(a) != inside(b) {
                let t = (edge - a[1]) / (b[1] - a[1]);
                out.push([a[0] + (b[0] - a[0]) * t, edge]);
            }
        }
        poly = out;
    }
    for i in 1..poly.len().saturating_sub(1) {
        gui.fill_quad([poly[0], poly[i], poly[i + 1], poly[i + 1]], color);
    }
}

/// Pie chart disc in the band `[top, bottom]`: centered at mid-height of the band, parts in
/// a fan (radius 105, flattened by half, one subdivision per 4% slice), then a 10 px slice
/// darkened on the front half, clipped at the bottom of the band.
fn draw_pie_disc(gui: &mut Gui, fields: &[crate::profiler::Field], cx: f32, top: f32, bottom: f32) {
    let cy = (top + bottom) / 2.0;
    let mut total = 0.0f64;
    for f in fields {
        let (pct, cur) = (f.percentage, total);
        total += pct;
        let steps = (pct / 4.0).floor() as i32 + 1;
        let color = f.color() | 0xFF00_0000;
        let dark = shade(color);
        let at = |j: i32| {
            let dir = ((cur + pct * f64::from(j) / f64::from(steps)) * f64::from(std::f32::consts::TAU) / 100.0) as f32;
            (dir.sin() * 105.0, dir.cos() * 105.0 * 0.5)
        };
        for j in (1..=steps).rev() {
            let (x0, y0) = at(j);
            let (x1, y1) = at(j - 1);
            let q = [[cx, cy], [cx + x0, cy + y0], [cx + x1, cy + y1], [cx + x1, cy + y1]];
            fill_quad_banded(gui, q, top, bottom, color);
        }
        for j in (1..=steps).rev() {
            let (x0, y0) = at(j);
            let (x1, y1) = at(j - 1);
            if (y0 + y1) / 2.0 >= 0.0 {
                let q = [[cx + x0, cy + y0], [cx + x0, cy + y0 + 10.0], [cx + x1, cy + y1 + 10.0], [cx + x1, cy + y1]];
                fill_quad_banded(gui, q, top, bottom, dark);
            }
        }
    }
}

impl ChartLogs {
    /// Disconnection: tick, ping and bandwidth logs cleared (the frames graph
    /// is kept, like the game); no subscription any more.
    pub fn reset(&mut self) {
        self.ping = Ring::default();
        self.bandwidth = Ring::default();
        self.tick = Ring::default();
        self.bytes_prev = None;
        self.subscribed = false;
        self.smoothed_tick_ms = 0.0;
    }
}

impl DebugScreen {
    /// Graphs shown, at the bottom of the virtual surface `(w, h)` (§ 7.1).
    pub fn draw_charts(&self, gui: &mut Gui, q: &wgpu::Queue, l: f32, h: f32, max_fps: i32) {
        let width = (242.0f32).min((l / 2.0).floor());
        if self.chart(Chart::Fps) {
            let r = &self.frame_ns;
            let ms = |i: usize| r.get(i) as f64 / 1e6;
            let height = |ms: f64| (ms * 60.0 / (100.0 / 3.0)).round() as i32;
            chart(
                gui,
                q,
                0.0,
                width,
                h,
                r.len(),
                &|i| {
                    let v = ms(i);
                    vec![(0, height(v), gradient(v, 0.0, 0xFF00_FF00, 28.0, 0xFFFF_FF00, 56.0, 0xFFFF_0000))]
                },
                &|i| r.get(i) as i64,
                &|ns| ms_text(ns / 1e6),
            );
            label(gui, q, "30 fps", 1.0, h - 60.0 + 1.0);
            label(gui, q, "60 fps", 1.0, h - 30.0 + 1.0);
            hline(gui, 0.0, width - 1.0, h - 30.0, 0xFFFF_FFFF);
            if (1..=250).contains(&max_fps) {
                let y = h - height(1000.0 / f64::from(max_fps)) as f32 - 1.0;
                hline(gui, 0.0, width - 1.0, y, 0xFF00_FFFF);
            }
            let t = &self.logs.tick;
            if t.len() > 0 {
                let mspt = f64::from(self.logs.mspt.max(0.001));
                let hgt = |ns: i64| (ns as f64 / 1e6 * 60.0 / mspt).round() as i32;
                let x = l - width;
                chart(
                    gui,
                    q,
                    x,
                    width,
                    h,
                    t.len(),
                    &|i| {
                        let s = t.get(i);
                        let full = s[0] as f64 / 1e6;
                        let c = gradient(full, mspt, 0xFF00_FF00, mspt * 1.125, 0xFFFF_FF00, mspt * 1.25, 0xFFFF_0000);
                        let (m, k) = (hgt(s[1]), hgt(s[2]));
                        let o = hgt(s[0] - s[3] - s[1] - s[2]);
                        vec![
                            (0, hgt(s[0]), c),
                            (0, m, 0xFF99_1111),
                            (m, m + k, 0xFFBA_995F),
                            (m + k, m + k + o, 0xFF5F_0E8C),
                        ]
                    },
                    &|i| {
                        let s = t.get(i);
                        s[0] - s[3]
                    },
                    &|ns| ms_text(ns / 1e6),
                );
                label(gui, q, &format!("{:.1} tps", 1000.0 / mspt), x + 1.0, h - 60.0 + 1.0);
            }
        }
        if self.chart(Chart::Network) {
            if !self.logs.local {
                self.draw_bandwidth(gui, q, width, h);
            }
            let p = &self.logs.ping;
            let x = l - width;
            chart(
                gui,
                q,
                x,
                width,
                h,
                p.len(),
                &|i| {
                    let ms = p.get(i) as f64;
                    vec![(
                        0,
                        (ms * 60.0 / 500.0).round() as i32,
                        gradient(ms, 0.0, 0xFF00_FF00, 250.0, 0xFFFF_FF00, 500.0, 0xFFFF_0000),
                    )]
                },
                &|i| p.get(i),
                &ms_text,
            );
            label(gui, q, "500 ms", x + 1.0, h - 60.0 + 1.0);
        }
        if self.chart(Chart::Lightmap) {
            self.draw_lightmap(gui, l, h);
        }
        if self.chart(Chart::Profiler) && self.logs.pie_ready {
            // Block placed above the bottom graphs (full height of a graph: 69).
            let offset = if self.chart(Chart::Fps) || self.chart(Chart::Network) { 69.0 } else { 10.0 };
            self.draw_pie(gui, q, l, h, offset);
        }
    }

    /// Profiler pie chart (F3+1): current node, its children as parts and as lines
    /// "[n] name  local %  global %", at the right of the screen.
    fn draw_pie(&self, gui: &mut Gui, q: &wgpu::Queue, l: f32, h: f32, offset: f32) {
        let path = if self.logs.pie_path.is_empty() { "root" } else { self.logs.pie_path.as_str() };
        let Some(mut list) = crate::profiler::with(crate::profiler::PIE, |c| c.times(path)) else { return };
        let cur = list.remove(0);
        let cx = l - 130.0 - 10.0;
        let (left, right) = (cx - 130.0, cx + 130.0);
        let bottom = h - offset - 5.0;
        let text_y = bottom - list.len() as f32 * 9.0;
        let cy = text_y - 62.0 - 5.0;
        let global = pct_text(cur.global);
        let gw = width_of(gui, &global);
        let zw = width_of(gui, "[0] ");
        let top_max = right - gw - 5.0 - left - zw;
        let name = cur.name.replace(crate::profiler::SEP, ".");
        let lines = split_node_name(gui, &name, top_max, top_max - 10.0);
        let top = cy - 62.0 - (lines.len().max(1) - 1) as f32 * 9.0;
        gui.fill(left - 5.0, top - 5.0, right + 5.0, bottom + 5.0, BG);
        draw_pie_disc(gui, &list, cx, cy - 52.0, cy + 62.0);
        let prefix = if name != "unspecified" && name != "root" { "[0] " } else { "" };
        let first = format!("{prefix}{}", lines.first().map_or("", String::as_str));
        let text = |gui: &mut Gui, s: &str, x: f32, y: f32, color: u32| {
            gui.text(q, &crate::gui::plain(s), x, y, color, true, 0);
        };
        text(gui, &first, left, top, 0xFFFF_FFFF);
        for (i, line) in lines.iter().enumerate().skip(1) {
            text(gui, line, left + 10.0 + zw, top + i as f32 * 9.0, 0xFFFF_FFFF);
        }
        text(gui, &global, right - gw, top, 0xFFFF_FFFF);
        for (i, f) in list.iter().enumerate() {
            let y = text_y + i as f32 * 9.0;
            let color = f.color();
            let label =
                if f.name == "unspecified" { format!("[?] {}", f.name) } else { format!("[{}] {}", i + 1, f.name) };
            text(gui, &label, left, y, color);
            let local = pct_text(f.percentage);
            text(gui, &local, right - 50.0 - width_of(gui, &local), y, color);
            let g = pct_text(f.global);
            text(gui, &g, right - width_of(gui, &g), y, color);
        }
    }

    /// Light map (F3+4): 16 × 16 texels enlarged to 64 × 64 nearest, black frame
    /// of one pixel, bottom right, flipped (sky 15 at the top).
    fn draw_lightmap(&self, gui: &mut Gui, l: f32, h: f32) {
        let (x, y) = (l - 64.0 - 2.0, h - 64.0 - 2.0);
        gui.fill(x - 1.0, y - 1.0, x + 65.0, y + 65.0, 0xFF00_0000);
        let Some(lm) = self.logs.lightmap.as_deref() else { return };
        for row in 0..16 {
            let sky = 15 - row;
            for block in 0..16 {
                let i = (sky * 16 + block) * 4;
                let Some(px) = lm.get(i..i + 4) else { return };
                let c = 0xFF00_0000 | u32::from(px[0]) << 16 | u32::from(px[1]) << 8 | u32::from(px[2]);
                let (cx, cy) = (x + 4.0 * block as f32, y + 4.0 * row as f32);
                gui.fill(cx, cy, cx + 4.0, cy + 4.0, c);
            }
        }
    }

    fn draw_bandwidth(&self, gui: &mut Gui, q: &wgpu::Queue, width: f32, h: f32) {
        {
            let b = &self.logs.bandwidth;
            chart(
                gui,
                q,
                0.0,
                width,
                h,
                b.len(),
                &|i| {
                    let bps = b.get(i) as f64 * 20.0;
                    vec![(
                        0,
                        bw_height(bps),
                        gradient(bps, 0.0, 0xFF00_FFFF, 8192.0, 0xFFA0_A0FF, 10_485_760.0, 0xFFFF_0000),
                    )]
                },
                &|i| b.get(i),
                &|bpt| bw_text(bpt * 20.0),
            );
            for v in [64.0, 1024.0, 16384.0] {
                let y = h - bw_height(v) as f32;
                label(gui, q, &bw_text(v), 1.0, y + 1.0);
                hline(gui, 0.0, width - 1.0, y, 0xFFFF_FFFF);
            }
            label(gui, q, &bw_text(1_048_576.0), 1.0, h - bw_height(1_048_576.0) as f32 + 1.0);
        }
    }
}

impl crate::App {
    /// Start of frame: profiler collections turned on according to F3+1 and the F3+L recording,
    /// `root` zone opened.
    pub(crate) fn profiler_frame_start(&mut self) {
        crate::profiler::set(crate::profiler::PIE, self.debug.chart(Chart::Profiler));
        crate::profiler::start_tick();
        self.profiling_frame_start();
    }

    /// End of frame: zones closed again; pie chart results shown at the next frame
    /// only if the collection covered it.
    pub(crate) fn profiler_frame_end(&mut self) {
        crate::profiler::end_tick();
        self.debug.logs.pie_ready = self.debug.chart(Chart::Profiler);
        self.profiling_frame_end();
    }

    /// Digit pressed, pie chart shown: 0 goes up to the parent, n goes down into the n-th child
    /// (never into "unspecified").
    pub(crate) fn pie_key(&mut self, digit: u32) {
        if !self.debug.logs.pie_ready {
            return;
        }
        let logs = &mut self.debug.logs;
        if logs.pie_path.is_empty() {
            logs.pie_path = "root".into();
        }
        let path = logs.pie_path.clone();
        let Some(mut list) = crate::profiler::with(crate::profiler::PIE, |c| c.times(&path)) else { return };
        if list.is_empty() {
            return;
        }
        let node = list.remove(0);
        if digit == 0 {
            if !node.name.is_empty()
                && let Some(pos) = logs.pie_path.rfind(crate::profiler::SEP)
            {
                logs.pie_path.truncate(pos);
            }
        } else if let Some(child) = list.get(digit as usize - 1)
            && child.name != "unspecified"
        {
            if !logs.pie_path.is_empty() {
                logs.pie_path.push(crate::profiler::SEP);
            }
            logs.pie_path.push_str(&child.name);
        }
    }

    /// Graphs tick: bytes received, ping requested as long as the network graph is shown,
    /// subscription to the server tick times as long as the frames graph is shown.
    pub(crate) fn debug_charts_tick(&mut self) {
        let st = &self.session.link.stats;
        let bytes = st.bytes_in.load(std::sync::atomic::Ordering::Relaxed);
        let logs = &mut self.debug.logs;
        if let Some(prev) = logs.bytes_prev {
            logs.bandwidth.push(bytes.saturating_sub(prev) as i64);
        }
        logs.bytes_prev = Some(bytes);
        // World rate of the client (`ticking_state`, singleplayer included).
        logs.mspt = 1000.0 / self.exact.rate.rate.max(1.0);
        for s in st.take_samples() {
            if s.len() >= 4 {
                logs.tick.push([s[0], s[1], s[2], s[3]]);
                logs.smoothed_tick_ms = logs.smoothed_tick_ms * 0.8 + s[1] as f32 / 1e6 * 0.199_999_99;
            }
        }
        // Like the game: every tick of the play phase, from entry (no waiting for the
        // position); the subscription follows the frames graph.
        if !self.session.link.in_play() {
            return;
        }
        if self.debug.chart(Chart::Network) {
            let _ = self.session.link.send_ping_request(rc_client::netstats::millis());
        }
        let want = self.debug.chart(Chart::Fps);
        if want != self.debug.logs.subscribed {
            self.debug.logs.subscribed = want;
            let _ = self.session.link.send_debug_subscriptions(if want { &[0] } else { &[] });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_ends_and_middle() {
        assert_eq!(gradient(0.0, 0.0, 0xFF00_FF00, 28.0, 0xFFFF_FF00, 56.0, 0xFFFF_0000), 0xFF00_FF00);
        assert_eq!(gradient(28.0, 0.0, 0xFF00_FF00, 28.0, 0xFFFF_FF00, 56.0, 0xFFFF_0000), 0xFFFF_FF00);
        assert_eq!(gradient(99.0, 0.0, 0xFF00_FF00, 28.0, 0xFFFF_FF00, 56.0, 0xFFFF_0000), 0xFFFF_0000);
        assert_eq!(gradient(14.0, 0.0, 0xFF00_FF00, 28.0, 0xFFFF_FF00, 56.0, 0xFFFF_0000), 0xFF7F_FF00);
    }

    #[test]
    fn bandwidth_text() {
        assert_eq!(bw_text(64.0), "64 B/s");
        assert_eq!(bw_text(1024.0), "1.0 KiB/s");
        assert_eq!(bw_text(1_048_576.0), "1.0 MiB/s");
        assert_eq!(bw_height(1_048_576.0), 60);
    }
}
