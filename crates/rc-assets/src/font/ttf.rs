//! TrueType font provider of the packs (`"type": "ttf"`): `file` (in `font/`), `size`
//! (11), `oversample` (1), `shift` ([0, 0]), `skip` (string or list of strings).
//!
//! Like the game: pixel size per em = round(size × oversample), outlines FITTED to the
//! grid (font instructions, or automatic fitting if it has none, "normal" grayscale
//! target), `shift` offset applied after fitting, pixel box
//! = envelope of the control points rounded outwards, grayscale coverage.
//! Glyphs produced on demand then kept.

use hashbrown::HashMap;
use std::sync::{Arc, Mutex};

use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, HintingOptions, OutlinePen};
use skrifa::raw::TableProvider;

use super::{Bitmap, Glyph};

/// Settings of a `ttf` provider (kept for the resource cache).
#[derive(Debug, Clone, PartialEq)]
pub struct TtfParams {
    pub size: f32,
    pub oversample: f32,
    pub shift: (f32, f32),
    pub skip: String,
}

/// Loaded TrueType font: code points served and glyphs already produced.
pub struct TtfFace {
    pub data: Arc<[u8]>,
    pub params: TtfParams,
    /// (code point, glyph of the font), increasing.
    pub cps: Vec<(u32, u32)>,
    ppem: u32,
    /// Offset in 1/64 pixel (x, y upwards).
    delta: (i32, i32),
    hinting: Mutex<Option<HintingInstance>>,
    done: Mutex<HashMap<u32, Glyph>>,
}

impl std::fmt::Debug for TtfFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtfFace").field("params", &self.params).field("glyphs", &self.cps.len()).finish()
    }
}

/// "Half towards +∞" rounding of a float, like the game's integer rounding.
fn round_half_up(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

impl TtfFace {
    /// Font read; `None` if it is not in TrueType format (`glyf` outlines).
    pub fn new(data: Arc<[u8]>, params: TtfParams) -> Option<TtfFace> {
        let font = skrifa::FontRef::new(&data).ok()?;
        if font.glyf().is_err() {
            eprintln!("font: non-TrueType file ignored");
            return None;
        }
        let skip: Vec<u32> = params.skip.chars().map(u32::from).collect();
        let mut cps: Vec<(u32, u32)> = font
            .charmap()
            .mappings()
            .map(|(c, g)| (c, g.to_u32()))
            .filter(|&(c, g)| g != 0 && !skip.contains(&c))
            .collect();
        cps.sort_unstable_by_key(|e| e.0);
        cps.dedup_by_key(|e| e.0);
        let ppem = round_half_up(params.size * params.oversample).max(1) as u32;
        let delta = (
            round_half_up(params.shift.0 * params.oversample * 64.0),
            round_half_up(-params.shift.1 * params.oversample * 64.0),
        );
        Some(TtfFace { data, params, cps, ppem, delta, hinting: Mutex::new(None), done: Mutex::new(HashMap::new()) })
    }

    pub fn contains(&self, c: u32) -> bool {
        self.cps.binary_search_by_key(&c, |e| e.0).is_ok()
    }

    pub fn glyph(&self, c: u32) -> Option<Glyph> {
        let gid = self.cps[self.cps.binary_search_by_key(&c, |e| e.0).ok()?].1;
        if let Some(g) = self.done.lock().ok().and_then(|d| d.get(&c).cloned()) {
            return Some(g);
        }
        let g = self.make(gid)?;
        if let Ok(mut d) = self.done.lock() {
            d.insert(c, g.clone());
        }
        Some(g)
    }

    /// Glyph fitted, measured and rendered.
    fn make(&self, gid: u32) -> Option<Glyph> {
        let font = skrifa::FontRef::new(&self.data).ok()?;
        let outlines = font.outline_glyphs();
        let outline = outlines.get(skrifa::GlyphId::new(gid))?;
        let mut hint = self.hinting.lock().ok()?;
        if hint.is_none() {
            *hint = HintingInstance::new(
                &outlines,
                Size::new(self.ppem as f32),
                LocationRef::default(),
                HintingOptions::default(),
            )
            .ok();
        }
        let mut pen = Collect::default();
        let metrics = match hint.as_ref() {
            Some(h) => outline.draw(DrawSettings::hinted(h, false), &mut pen).ok()?,
            None => outline
                .draw(DrawSettings::unhinted(Size::new(self.ppem as f32), LocationRef::default()), &mut pen)
                .ok()?,
        };
        drop(hint);
        let os = self.params.oversample;
        let advance = metrics.advance_width.unwrap_or(0.0) / os;
        // Points in 1/64 pixel, shifted.
        let pts: Vec<Seg<(i32, i32)>> =
            pen.segs.iter().map(|s| s.map(|(x, y)| (to64(x) + self.delta.0, to64(y) + self.delta.1))).collect();
        let mut all = pts.iter().flat_map(Seg::points);
        let Some(first) = all.next() else { return Some(empty(advance)) };
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for (x, y) in all {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        // Pixel box: minimum by default, maximum by excess.
        let (left, bottom) = (x0 >> 6, y0 >> 6);
        let right = (x1 >> 6) + i32::from(x1 & 63 != 0);
        let top = (y1 >> 6) + i32::from(y1 & 63 != 0);
        let (w, h) = ((right - left) as u32, (top - bottom) as u32);
        if w == 0 || h == 0 {
            return Some(empty(advance));
        }
        let mut r = ab_glyph_rasterizer::Rasterizer::new(w as usize, h as usize);
        let p = |(x, y): (i32, i32)| {
            ab_glyph_rasterizer::point(x as f32 / 64.0 - left as f32, top as f32 - y as f32 / 64.0)
        };
        for s in &pts {
            match *s {
                Seg::Line(a, b) => r.draw_line(p(a), p(b)),
                Seg::Quad(a, b, c) => flatten(&mut r, &[p(a), p(b), p(c)]),
                Seg::Cubic(a, b, c, d) => flatten(&mut r, &[p(a), p(b), p(c), p(d)]),
            }
        }
        let mut gray = vec![0u8; (w * h) as usize];
        r.for_each_pixel(|i, a| gray[i] = ((a * 256.0) as u32).min(255) as u8);
        let (bl, bt) = (left as f32 / os, top as f32 / os);
        let gtop = 7.0 - bt;
        Some(Glyph {
            advance,
            offset: 1.0,
            left: bl,
            top: gtop,
            right: bl + w as f32 / os,
            bottom: gtop + h as f32 / os,
            bitmap: Some(Bitmap::Gray { w, h, data: gray.into() }),
        })
    }
}

/// Curve split like the game's grayscale rasteriser (coordinates in 1/256 of a
/// pixel): a conic into 2^k segments equal in parameter, k such that its second difference
/// falls under 1/4 pixel (divided by 4 at each doubling); a cubic cut in two
/// as long as a control point strays more than 1/2 pixel from the chord.
fn flatten(r: &mut ab_glyph_rasterizer::Rasterizer, c: &[ab_glyph_rasterizer::Point]) {
    use ab_glyph_rasterizer::{Point, point};
    const ONE: f32 = 256.0;
    if c.len() == 3 {
        let d = ((c[2].x + c[0].x - 2.0 * c[1].x).abs().max((c[2].y + c[0].y - 2.0 * c[1].y).abs()) * ONE) as i64;
        let mut d = d;
        let mut n = 1usize;
        while d > (ONE as i64) / 4 {
            d >>= 2;
            n <<= 1;
        }
        let mut prev = c[0];
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let next = if i == n {
                c[2]
            } else {
                point(
                    u * u * c[0].x + 2.0 * u * t * c[1].x + t * t * c[2].x,
                    u * u * c[0].y + 2.0 * u * t * c[1].y + t * t * c[2].y,
                )
            };
            r.draw_line(prev, next);
            prev = next;
        }
        return;
    }
    fn cubic(r: &mut ab_glyph_rasterizer::Rasterizer, a: [Point; 4], depth: u32) {
        let far = |p: f32| p * ONE > ONE / 2.0;
        let flat = !(far((2.0 * a[0].x - 3.0 * a[1].x + a[3].x).abs())
            || far((2.0 * a[0].y - 3.0 * a[1].y + a[3].y).abs())
            || far((a[0].x - 3.0 * a[2].x + 2.0 * a[3].x).abs())
            || far((a[0].y - 3.0 * a[2].y + 2.0 * a[3].y).abs()));
        if flat || depth >= 16 {
            r.draw_line(a[0], a[3]);
            return;
        }
        let mid = |p: Point, q: Point| point((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
        let (ab, bc, cd) = (mid(a[0], a[1]), mid(a[1], a[2]), mid(a[2], a[3]));
        let (abc, bcd) = (mid(ab, bc), mid(bc, cd));
        let m = mid(abc, bcd);
        cubic(r, [a[0], ab, abc, m], depth + 1);
        cubic(r, [m, bcd, cd, a[3]], depth + 1);
    }
    cubic(r, [c[0], c[1], c[2], c[3]], 0);
}

fn to64(v: f32) -> i32 {
    (v * 64.0).round() as i32
}

fn empty(advance: f32) -> Glyph {
    Glyph { advance, offset: 1.0, left: 0.0, top: 0.0, right: 0.0, bottom: 0.0, bitmap: None }
}

/// Segment of an outline (control points included).
#[derive(Clone, Copy)]
enum Seg<T> {
    Line(T, T),
    Quad(T, T, T),
    Cubic(T, T, T, T),
}

impl<T: Copy> Seg<T> {
    fn map<U>(&self, f: impl Fn(T) -> U) -> Seg<U> {
        match *self {
            Seg::Line(a, b) => Seg::Line(f(a), f(b)),
            Seg::Quad(a, b, c) => Seg::Quad(f(a), f(b), f(c)),
            Seg::Cubic(a, b, c, d) => Seg::Cubic(f(a), f(b), f(c), f(d)),
        }
    }

    fn points(&self) -> impl Iterator<Item = T> {
        let v: Vec<T> = match *self {
            Seg::Line(a, b) => vec![a, b],
            Seg::Quad(a, b, c) => vec![a, b, c],
            Seg::Cubic(a, b, c, d) => vec![a, b, c, d],
        };
        v.into_iter()
    }
}

/// Outlines received from the font, in pixels (y upwards), closed outlines.
#[derive(Default)]
struct Collect {
    segs: Vec<Seg<(f32, f32)>>,
    start: (f32, f32),
    at: (f32, f32),
}

impl OutlinePen for Collect {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.start = (x, y);
        self.at = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Line(self.at, (x, y)));
        self.at = (x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.segs.push(Seg::Quad(self.at, (cx, cy), (x, y)));
        self.at = (x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.segs.push(Seg::Cubic(self.at, (cx0, cy0), (cx1, cy1), (x, y)));
        self.at = (x, y);
    }

    fn close(&mut self) {
        if self.at != self.start {
            self.segs.push(Seg::Line(self.at, self.start));
            self.at = self.start;
        }
    }
}

/// Settings read from the JSON provider.
pub fn params(p: &serde_json::Value) -> TtfParams {
    let f = |v: &serde_json::Value, d: f32| v.as_f64().map_or(d, |x| x as f32);
    let shift = match p["shift"].as_array() {
        Some(a) if a.len() == 2 => (f(&a[0], 0.0), f(&a[1], 0.0)),
        _ => (0.0, 0.0),
    };
    let skip = match &p["skip"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().filter_map(serde_json::Value::as_str).collect(),
        _ => String::new(),
    };
    TtfParams { size: f(&p["size"], 11.0), oversample: f(&p["oversample"], 1.0), shift, skip }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_defaults_and_skip_list() {
        let p = params(&serde_json::json!({"file": "x.ttf", "skip": ["ab", "c"], "shift": [0.5, 1]}));
        assert_eq!(p, TtfParams { size: 11.0, oversample: 1.0, shift: (0.5, 1.0), skip: "abc".into() });
        assert_eq!(round_half_up(2.5), 3);
        assert_eq!(round_half_up(-2.5), -2);
    }

    /// Comparison with the official game's FreeType probe. `RUSTCRAFT_TTF_PROBE`:
    /// "font|size|oversample|shiftx|shifty|expected file" (lines of `ft_probe.py`).
    /// Measurements (advance, corner, size) exact; coverage within ±`tol`.
    #[test]
    fn matches_freetype_probe() {
        let Some(spec) = rc_data::env_str!("RUSTCRAFT_TTF_PROBE") else { return };
        let f: Vec<&str> = spec.split('|').collect();
        let num = |i: usize| f[i].parse::<f32>().expect("number");
        let data: Arc<[u8]> = std::fs::read(f[0]).expect("font").into();
        let params = TtfParams { size: num(1), oversample: num(2), shift: (num(3), num(4)), skip: String::new() };
        let os = params.oversample;
        let face = TtfFace::new(data, params).expect("TrueType");
        let want = std::fs::read_to_string(f[5]).expect("probe");
        let (mut glyphs, mut px, mut worst, mut off) = (0, 0usize, 0i32, 0usize);
        let mut bad: Vec<String> = Vec::new();
        for line in want.lines() {
            let w: Vec<&str> = line.split(' ').collect();
            let cp: u32 = w[0].parse().expect("cp");
            let g = face.glyph(cp).expect("glyph");
            let (adv, left, top, gw, gh): (f32, i32, i32, u32, u32) = (
                w[1].parse().unwrap(),
                w[2].parse().unwrap(),
                w[3].parse().unwrap(),
                w[4].parse().unwrap(),
                w[5].parse().unwrap(),
            );
            if g.advance != adv {
                bad.push(format!("advance U+{cp:04X} {} / {adv}", g.advance));
            }
            glyphs += 1;
            match &g.bitmap {
                Some(Bitmap::Gray { w: bw, h: bh, data }) => {
                    let corner = ((g.left * os).round() as i32, ((7.0 - g.top) * os).round() as i32);
                    if (*bw, *bh) != (gw, gh) || corner != (left, top) {
                        bad.push(format!("box U+{cp:04X} {bw}x{bh} {corner:?} / {gw}x{gh} ({left}, {top})"));
                        continue;
                    }
                    let hex = w.get(6).copied().unwrap_or("");
                    for (i, &v) in data.iter().enumerate() {
                        let e = i32::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
                        let d = (i32::from(v) - e).abs();
                        worst = worst.max(d);
                        off += usize::from(d > 0);
                        px += 1;
                    }
                }
                _ if gw == 0 || gh == 0 => {}
                _ => bad.push(format!("empty U+{cp:04X} / {gw}x{gh}")),
            }
        }
        eprintln!("{glyphs} glyphs, {px} pixels, {off} different, max difference {worst}");
        for b in &bad {
            eprintln!("{b}");
        }
        assert!(bad.is_empty(), "{} glyphs with different measurements", bad.len());
        let tol: i32 = rc_data::env_str!("RUSTCRAFT_TTF_TOL").and_then(|v| v.parse().ok()).unwrap_or(8);
        assert!(worst <= tol, "coverage difference {worst} > {tol}");
    }
}
