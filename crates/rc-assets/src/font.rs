//! Fonts (`docs/specs/text.md` § 2): bitmap, space, unihex and reference providers,
//! TrueType (`ttf.rs`), option filters, search order for a character, glyph
//! metrics.

mod ttf;

use hashbrown::HashMap;
use std::io::Read;
use std::sync::Arc;

use serde_json::Value;

use crate::Assets;
use crate::texture::Image;

/// Image of a glyph, kept compact until it is sent to the atlas.
#[derive(Debug, Clone)]
pub enum Bitmap {
    /// Cell of a sheet: corner (texels) and size.
    Sheet {
        sheet: Arc<Image>,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    /// Unihex matrix: 16 left-aligned rows (bit 31 = column 0), columns `left..left+width`.
    Hex {
        rows: [u32; 16],
        left: u32,
        width: u32,
    },
    /// Grayscale coverage (TrueType font), row by row.
    Gray {
        w: u32,
        h: u32,
        data: Arc<[u8]>,
    },
    /// 5 × 8 frame of the missing glyph, or white 5 × 8 block.
    Missing,
    White,
}

impl Bitmap {
    /// Size in texels.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Bitmap::Sheet { w, h, .. } => (*w, *h),
            Bitmap::Hex { width, .. } => (*width, 16),
            Bitmap::Gray { w, h, .. } => (*w, *h),
            Bitmap::Missing | Bitmap::White => (5, 8),
        }
    }

    /// RGBA pixels (row by row).
    pub fn rgba(&self) -> Vec<u8> {
        let (w, h) = self.size();
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let px: [u8; 4] = match self {
                    Bitmap::Sheet { sheet, x: sx, y: sy, .. } => {
                        let i = (((sy + y) * sheet.width + sx + x) * 4) as usize;
                        [sheet.rgba[i], sheet.rgba[i + 1], sheet.rgba[i + 2], sheet.rgba[i + 3]]
                    }
                    Bitmap::Hex { rows, left, .. } => {
                        let col = left + x;
                        if col < 32 && rows[y as usize] >> (31 - col) & 1 == 1 { [255; 4] } else { [0; 4] }
                    }
                    // The game's intensity texture: coverage is used for all 4 components.
                    Bitmap::Gray { data, .. } => [data[(y * w + x) as usize]; 4],
                    Bitmap::Missing => {
                        if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                            [255; 4]
                        } else {
                            [0; 4]
                        }
                    }
                    Bitmap::White => [255; 4],
                };
                let i = ((y * w + x) * 4) as usize;
                out[i..i + 4].copy_from_slice(&px);
            }
        }
        out
    }
}

/// Glyph: advance, shadow and bold offset, quad (px relative to the cursor) and image.
#[derive(Debug, Clone)]
pub struct Glyph {
    pub advance: f32,
    pub offset: f32,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// `None`: invisible glyph (space).
    pub bitmap: Option<Bitmap>,
}

impl Glyph {
    pub fn missing() -> Glyph {
        Glyph { advance: 6.0, offset: 1.0, left: 0.0, top: 0.0, right: 5.0, bottom: 8.0, bitmap: Some(Bitmap::Missing) }
    }

    pub fn white() -> Glyph {
        Glyph { bitmap: Some(Bitmap::White), ..Glyph::missing() }
    }

    /// Same glyph bit for bit: metrics and pixels (equivalence proofs).
    pub fn same_as(&self, o: &Glyph) -> bool {
        let m = |g: &Glyph| [g.advance, g.offset, g.left, g.top, g.right, g.bottom].map(f32::to_bits);
        m(self) == m(o)
            && match (&self.bitmap, &o.bitmap) {
                (None, None) => true,
                (Some(a), Some(b)) => a.size() == b.size() && a.rgba() == b.rgba(),
                _ => false,
            }
    }
}

/// Glyphs of one or more consecutive providers.
type Glyphs = HashMap<u32, Glyph>;

/// Compact unihex table: sorted code points, and for each one its rows (16 × `u16`, or
/// 32 for glyphs wider than 16 columns: upper then lower half) in `rows`, with
/// the ink (first column, width). The glyph is rebuilt on reading.
#[derive(Debug, Default)]
struct HexTable {
    cps: Vec<u32>,
    /// (start in `rows`, wide, first column, width).
    glyphs: Vec<(u32, bool, u8, u8)>,
    rows: Vec<u16>,
}

impl HexTable {
    fn push(&mut self, cp: u32, rows: &[u32; 16], left: u32, width: u32) {
        let wide = rows.iter().any(|r| r & 0xffff != 0);
        self.cps.push(cp);
        self.glyphs.push((self.rows.len() as u32, wide, left as u8, width as u8));
        self.rows.extend(rows.iter().map(|r| (r >> 16) as u16));
        if wide {
            self.rows.extend(rows.iter().map(|&r| r as u16));
        }
    }

    /// Sorted by code point (the last one read wins, like an insertion into a table).
    fn finish(mut self) -> Self {
        let mut order: Vec<usize> = (0..self.cps.len()).collect();
        order.sort_by_key(|&i| (self.cps[i], std::cmp::Reverse(i)));
        order.dedup_by_key(|i| self.cps[*i]);
        self.cps = order.iter().map(|&i| self.cps[i]).collect();
        self.glyphs = order.iter().map(|&i| self.glyphs[i]).collect();
        self
    }

    fn contains(&self, c: u32) -> bool {
        self.cps.binary_search(&c).is_ok()
    }

    fn get(&self, c: u32) -> Option<Glyph> {
        let (start, wide, left, width) = self.glyphs[self.cps.binary_search(&c).ok()?];
        let s = start as usize;
        let rows: [u32; 16] = std::array::from_fn(|y| {
            let hi = u32::from(self.rows[s + y]) << 16;
            if wide { hi | u32::from(self.rows[s + 16 + y]) } else { hi }
        });
        let (left, width) = (u32::from(left), u32::from(width));
        Some(Glyph {
            advance: (width / 2 + 1) as f32,
            offset: 0.5,
            left: 0.0,
            top: 0.0,
            right: width as f32 / 2.0,
            bottom: 8.0,
            bitmap: Some(Bitmap::Hex { rows, left, width }),
        })
    }
}

/// Layer of a font: own glyphs, or those of a unihex provider shared between fonts
/// (the same Unicode table serves `default` and `uniform`).
/// `Alt`: glyphs hidden by a "fishy" glyph of an earlier layer, kept for the
/// search without fishy glyphs only (invisible to the ordinary search).
#[derive(Debug)]
enum Layer {
    Own(Glyphs),
    Hex(Arc<HexTable>),
    Ttf(Arc<ttf::TtfFace>),
    Alt(Glyphs),
}

impl Layer {
    fn get(&self, c: u32) -> Option<Glyph> {
        match self {
            Layer::Own(m) => m.get(&c).cloned(),
            Layer::Hex(t) => t.get(c),
            Layer::Ttf(f) => f.glyph(c),
            Layer::Alt(_) => None,
        }
    }

    fn contains(&self, c: u32) -> bool {
        match self {
            Layer::Own(m) => m.contains_key(&c),
            Layer::Hex(t) => t.contains(c),
            Layer::Ttf(f) => f.contains(c),
            Layer::Alt(_) => false,
        }
    }

    fn keys(&self) -> Box<dyn Iterator<Item = u32> + '_> {
        match self {
            Layer::Own(m) => Box::new(m.keys().copied()),
            Layer::Hex(t) => Box::new(t.cps.iter().copied()),
            Layer::Ttf(f) => Box::new(f.cps.iter().map(|e| e.0)),
            Layer::Alt(_) => Box::new(std::iter::empty()),
        }
    }
}

/// "Fishy" glyph: advance, normal or bold, outside [0, 32] (the chat field and the obfuscated
/// text take another one).
fn fishy(advance: f32, offset: f32) -> bool {
    let bold = advance + offset;
    advance < 0.0 || advance > 32.0 || bold < 0.0 || bold > 32.0
}

/// Loaded font: first active provider that knows a code point.
#[derive(Debug, Default)]
pub struct Font {
    layers: Vec<Layer>,
    /// Code points by advance rounded up (obfuscated text), built on first
    /// need.
    by_width: std::sync::OnceLock<HashMap<u32, Vec<u32>>>,
    /// Flat table of the code points `0..LOW` (`RUSTCRAFT_OPT_FONTTABLE`): first glyph of
    /// each code point, resolved on first request (a TrueType font only renders what is
    /// used), then lent without copy or search through the layers.
    low: std::sync::OnceLock<Box<[std::sync::OnceLock<Option<Glyph>>]>>,
}

/// Code points covered by the flat table: Latin, extended Latin, IPA, modifier letters
/// (almost all interface text).
pub const LOW: u32 = 0x300;

impl Font {
    pub fn glyph(&self, c: u32) -> Option<Glyph> {
        self.layers.iter().find_map(|l| l.get(c))
    }

    /// Entry of the flat table (`None`: outside the table). The layers no longer change after
    /// loading: the entry equals [`Font::glyph`] forever.
    fn low(&self, c: u32) -> Option<&Option<Glyph>> {
        if c >= LOW {
            return None;
        }
        let t = self.low.get_or_init(|| (0..LOW).map(|_| std::sync::OnceLock::new()).collect());
        Some(t[c as usize].get_or_init(|| self.glyph(c)))
    }

    /// Like [`Font::glyph`], lent when possible (flat table, own layer): no
    /// atomic increment of the sheet image nor unihex rebuild in the table.
    pub fn glyph_ref(&self, c: u32) -> Option<std::borrow::Cow<'_, Glyph>> {
        use std::borrow::Cow;
        if let Some(g) = self.low(c) {
            return g.as_ref().map(Cow::Borrowed);
        }
        self.layers.iter().find_map(|l| match l {
            Layer::Own(m) => m.get(&c).map(Cow::Borrowed),
            l => l.get(c).map(Cow::Owned),
        })
    }

    /// Like [`Font::glyph_non_fishy`], lent by the flat table when the first glyph is not
    /// fishy (it is then that one); otherwise the full search.
    pub fn glyph_non_fishy_ref(&self, c: u32) -> Option<std::borrow::Cow<'_, Glyph>> {
        match self.low(c) {
            Some(None) => None,
            Some(Some(g)) if !fishy(g.advance, g.offset) => Some(std::borrow::Cow::Borrowed(g)),
            _ => self.glyph_non_fishy(c).map(std::borrow::Cow::Owned),
        }
    }

    /// Like [`Font::advance`], through the flat table within its range.
    pub fn advance_ref(&self, c: u32) -> Option<(f32, f32)> {
        match self.low(c) {
            Some(g) => g.as_ref().map(|g| (g.advance, g.offset)),
            None => self.advance(c),
        }
    }

    /// Like [`Font::first_is_fishy`], through the flat table within its range.
    pub fn first_is_fishy_ref(&self, c: u32) -> bool {
        self.advance_ref(c).is_some_and(|(a, o)| fishy(a, o))
    }

    /// Advance and shadow offset only (text measurement), without rebuilding the glyph.
    pub fn advance(&self, c: u32) -> Option<(f32, f32)> {
        self.layers.iter().find_map(|l| match l {
            Layer::Own(m) => m.get(&c).map(|g| (g.advance, g.offset)),
            Layer::Hex(t) => {
                let i = t.cps.binary_search(&c).ok()?;
                Some(((u32::from(t.glyphs[i].3) / 2 + 1) as f32, 0.5))
            }
            Layer::Ttf(f) => f.glyph(c).map(|g| (g.advance, g.offset)),
            Layer::Alt(_) => None,
        })
    }

    /// First non-fishy glyph (chat input field, obfuscated text); `None`: unknown code
    /// point, or all its glyphs fishy (the game then shows the missing glyph).
    pub fn glyph_non_fishy(&self, c: u32) -> Option<Glyph> {
        self.layers.iter().find_map(|l| {
            let g = match l {
                Layer::Alt(m) => m.get(&c).cloned(),
                l => l.get(c),
            };
            g.filter(|g| !fishy(g.advance, g.offset))
        })
    }

    /// The first glyph of `c` is fishy (the choice without fishy glyphs differs from it).
    pub fn first_is_fishy(&self, c: u32) -> bool {
        self.advance(c).is_some_and(|(a, o)| fishy(a, o))
    }

    /// Number of known code points.
    pub fn len(&self) -> usize {
        self.codepoints().count()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.keys().next().is_none())
    }

    /// Known code points, each once (that of the first layer).
    fn codepoints(&self) -> impl Iterator<Item = u32> + '_ {
        self.layers
            .iter()
            .enumerate()
            .flat_map(move |(i, l)| l.keys().filter(move |&c| !self.layers[..i].iter().any(|e| e.contains(c))))
    }

    /// Code points whose advance rounded up equals `width`.
    pub fn with_width(&self, width: u32) -> &[u32] {
        let map = self.by_width.get_or_init(|| {
            let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
            for c in self.codepoints() {
                if let Some(g) = self.glyph(c) {
                    m.entry(g.advance.ceil() as u32).or_default().push(c);
                }
            }
            for v in m.values_mut() {
                v.sort_unstable();
            }
            m
        });
        map.get(&width).map_or(&[], Vec::as_slice)
    }

    /// Current own layer (created after a shared layer).
    fn own(&mut self) -> &mut Glyphs {
        if !matches!(self.layers.last(), Some(Layer::Own(_))) {
            self.layers.push(Layer::Own(HashMap::new()));
        }
        match self.layers.last_mut() {
            Some(Layer::Own(m)) => m,
            _ => unreachable!("own layer pushed above"),
        }
    }

    /// Current layer of hidden glyphs (created as needed).
    fn alt(&mut self) -> &mut Glyphs {
        if !matches!(self.layers.last(), Some(Layer::Alt(_))) {
            self.layers.push(Layer::Alt(HashMap::new()));
        }
        match self.layers.last_mut() {
            Some(Layer::Alt(m)) => m,
            _ => unreachable!("layer pushed above"),
        }
    }

    /// Adds a glyph if no earlier layer knows it; if already known, it is kept
    /// (separately) only if it is the first non-fishy one behind a fishy glyph.
    fn add(&mut self, c: u32, g: Glyph) {
        if !self.layers.iter().any(|l| l.contains(c)) {
            self.own().insert(c, g);
        } else if !fishy(g.advance, g.offset) && self.first_is_fishy(c) && self.glyph_non_fishy(c).is_none() {
            self.alt().insert(c, g);
        }
    }

    /// All these code points already have a first glyph (in an earlier layer).
    fn shadowed(&self, mut cps: impl Iterator<Item = u32>) -> bool {
        cps.all(|c| self.layers.iter().any(|l| l.contains(c)))
    }

    /// Adds a provider kept by the options. Like the game, a provider that gives the
    /// first glyph of no code point is discarded, including from the search without fishy
    /// glyphs (chat field, obfuscated text).
    fn add_provider(&mut self, p: Provider) {
        match p {
            Provider::Glyphs(m) => {
                if !self.shadowed(m.keys().copied()) {
                    for (c, g) in m {
                        self.add(c, g);
                    }
                }
            }
            Provider::Hex(t) => {
                if !self.shadowed(t.cps.iter().copied()) {
                    self.layers.push(Layer::Hex(t));
                }
            }
            Provider::Ttf(f) => {
                if !self.shadowed(f.cps.iter().map(|e| e.0)) {
                    self.layers.push(Layer::Ttf(f));
                }
            }
        }
    }
}

/// Loaded provider, before the option filter.
enum Provider {
    Glyphs(Glyphs),
    Hex(Arc<HexTable>),
    Ttf(Arc<ttf::TtfFace>),
}

/// Options that filter the providers.
#[derive(Debug, Clone, Copy, Default)]
pub struct FontOptions {
    pub uniform: bool,
    pub jp: bool,
}

/// Number read like the game: a boolean is 1 or 0.
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// Integer read like the game: truncated number.
fn int(v: &Value) -> Option<i32> {
    num(v).map(|x| x as i32)
}

/// Boolean read like the game: a number is true if its low byte is not zero.
fn boolean(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x as i64 as i8 != 0),
        _ => None,
    }
}

/// Valid identifier: `[namespace:]path`, lowercase, digits, `_-.`, and `/` in the path.
fn valid_id(s: &str) -> bool {
    let (ns, path) = s.split_once(':').unwrap_or(("minecraft", s));
    let ok = |c: char, slash: bool| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-' | '.') || (slash && c == '/');
    ns.chars().all(|c| ok(c, false)) && path.chars().all(|c| ok(c, true))
}

/// String of exactly one code point.
fn one_codepoint(v: Option<&Value>) -> Option<char> {
    let s = v?.as_str()?;
    let mut it = s.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

/// Provider conforming to the game's format (required fields, types, constraints). A single
/// non-conforming provider rejects the whole file of its pack, like the game.
fn valid_provider(p: &Value) -> bool {
    let Some(o) = p.as_object() else { return false };
    let filter_ok = o.get("filter").is_none_or(|f| {
        f.as_object()
            .is_some_and(|f| f.iter().all(|(k, v)| matches!(k.as_str(), "uniform" | "jp") && boolean(v).is_some()))
    });
    let id = |k: &str| o.get(k).and_then(Value::as_str).is_some_and(valid_id);
    let opt_num = |k: &str| o.get(k).is_none_or(|v| num(v).is_some());
    filter_ok
        && match o.get("type").and_then(Value::as_str) {
            Some("bitmap") => {
                let height = o.get("height").map_or(Some(8), int);
                let ascent = o.get("ascent").and_then(int);
                let grid = o.get("chars").and_then(Value::as_array).is_some_and(|rows| {
                    let lens: Option<Vec<usize>> = rows.iter().map(|r| r.as_str().map(|s| s.chars().count())).collect();
                    lens.is_some_and(|l| l.first().is_some_and(|&w| w > 0 && l.iter().all(|&x| x == w)))
                });
                id("file") && grid && matches!((height, ascent), (Some(h), Some(a)) if a <= h)
            }
            Some("space") => o
                .get("advances")
                .and_then(Value::as_object)
                .is_some_and(|m| m.iter().all(|(k, v)| k.chars().count() == 1 && num(v).is_some())),
            Some("ttf") => {
                let shift = o.get("shift").is_none_or(|s| {
                    s.as_array().is_some_and(|a| {
                        a.len() == 2 && a.iter().all(|x| num(x).is_some_and(|x| (-512.0..=512.0).contains(&x)))
                    })
                });
                let skip = o
                    .get("skip")
                    .is_none_or(|s| s.is_string() || s.as_array().is_some_and(|a| a.iter().all(Value::is_string)));
                id("file") && opt_num("size") && opt_num("oversample") && shift && skip
            }
            Some("unihex") => {
                let range_ok = |r: &Value| {
                    matches!((one_codepoint(r.get("from")), one_codepoint(r.get("to"))), (Some(a), Some(b)) if a < b)
                        && r.get("left").and_then(int).is_some()
                        && r.get("right").and_then(int).is_some()
                };
                id("hex_file")
                    && o.get("size_overrides").is_none_or(|s| s.as_array().is_some_and(|a| a.iter().all(range_ok)))
            }
            Some("reference") => id("id"),
            _ => false,
        }
}

fn filter_passes(filter: Option<&serde_json::Map<String, Value>>, o: FontOptions) -> bool {
    filter.is_none_or(|f| {
        f.iter().all(|(k, v)| {
            let want = boolean(v).unwrap_or(false);
            match k.as_str() {
                "uniform" => o.uniform == want,
                "jp" => o.jp == want,
                _ => true,
            }
        })
    })
}

fn id_path(id: &str, prefix: &str, ext: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    let ns = if ns.is_empty() { "minecraft" } else { ns };
    format!("assets/{ns}/{prefix}{path}{ext}")
}

/// Providers of a font, references expanded in place, with their merged filter
/// (that of the reference wins over a common key). Stacked definitions: the one of the
/// highest pack first, then those of the lower packs; a file that is unreadable or one of whose
/// providers is not conforming only removes the share of its pack. Font absent from all
/// packs, or reference not found: `false` (except `lenient` for a reference: ignored).
fn expand(
    assets: &Assets,
    id: &str,
    filter: serde_json::Map<String, Value>,
    depth: u32,
    lenient: bool,
    out: &mut Vec<Value>,
) -> bool {
    if depth > 8 {
        return false;
    }
    let files = assets.read_all(&id_path(id, "font/", ".json"));
    if files.is_empty() {
        return false;
    }
    for bytes in &files {
        let providers = serde_json::from_slice::<Value>(bytes)
            .ok()
            .and_then(|j| j.get("providers")?.as_array().cloned())
            .filter(|ps| ps.iter().all(valid_provider));
        let Some(providers) = providers else {
            eprintln!("font {id}: non-conforming definition in a pack, ignored");
            continue;
        };
        for p in &providers {
            let mut merged = p["filter"].as_object().cloned().unwrap_or_default();
            for (k, v) in &filter {
                merged.insert(k.clone(), v.clone());
            }
            if p["type"].as_str() == Some("reference") {
                let target = p["id"].as_str().unwrap_or_default();
                if !expand(assets, target, merged, depth + 1, lenient, out) {
                    if !lenient {
                        return false;
                    }
                    eprintln!("font {id}: reference {target} not found, ignored");
                }
            } else {
                let mut p = p.clone();
                p["filter"] = Value::Object(merged);
                out.push(p);
            }
        }
    }
    true
}

impl Font {
    /// Loads the font `id` (e.g. `minecraft:default`); on failure: empty (everything missing).
    pub fn load(assets: &Assets, id: &str, o: FontOptions) -> Font {
        Self::try_load(assets, id, o, false).unwrap_or_default()
    }

    /// Loads the font `id`. `None`: it fails as in the game (absent, reference
    /// not found, file of a provider absent or unreadable). All the providers are
    /// loaded, then filtered by the options: a missing file makes the font fail whatever
    /// the options. `lenient`: a failing provider or reference is only
    /// ignored (stack without a pack whose asset index is missing: ASCII rather than no text).
    pub fn try_load(assets: &Assets, id: &str, o: FontOptions, lenient: bool) -> Option<Font> {
        let mut defs = Vec::new();
        if !expand(assets, id, serde_json::Map::new(), 0, lenient, &mut defs) {
            eprintln!("font {id}: not found");
            return None;
        }
        let mut loaded = Vec::with_capacity(defs.len());
        for p in defs {
            match load_provider(assets, &p) {
                Ok(pr) => loaded.push((p, pr)),
                Err(e) if lenient => eprintln!("font {id}: {e}, provider ignored"),
                Err(e) => {
                    eprintln!("font {id}: {e}, font failed");
                    return None;
                }
            }
        }
        let mut font = Font::default();
        for (p, pr) in loaded {
            if filter_passes(p["filter"].as_object(), o) {
                font.add_provider(pr);
            }
        }
        Some(font)
    }
}

/// Loads a conforming provider (`valid_provider`); `Err`: it makes the font fail.
fn load_provider(assets: &Assets, p: &Value) -> Result<Provider, String> {
    match p["type"].as_str() {
        Some("bitmap") => load_bitmap(assets, p).map(Provider::Glyphs),
        Some("unihex") => load_unihex(assets, p).map(Provider::Hex),
        Some("ttf") => load_ttf(assets, p),
        _ => {
            let mut m = HashMap::new();
            for (k, v) in p["advances"].as_object().into_iter().flatten() {
                let Some(c) = k.chars().next() else { continue };
                let advance = num(v).unwrap_or(0.0) as f32;
                m.insert(
                    c as u32,
                    Glyph { advance, offset: 1.0, left: 0.0, top: 0.0, right: 0.0, bottom: 0.0, bitmap: None },
                );
            }
            Ok(Provider::Glyphs(m))
        }
    }
}

/// `Err`: sheet absent, unreadable or without luminance or alpha (RGB).
fn load_bitmap(assets: &Assets, p: &Value) -> Result<Glyphs, String> {
    let file = p["file"].as_str().unwrap_or_default();
    let height = p.get("height").map_or(Some(8), int).unwrap_or(8) as f32;
    let ascent = int(&p["ascent"]).unwrap_or(0) as f32;
    let rows: Vec<Vec<u32>> = p["chars"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| s.chars().map(|c| c as u32).collect())
        .collect();
    let cols = rows.first().map_or(0, Vec::len);
    // Raw file: the pixel kind picks the byte that gives the width (luminance for
    // grays, alpha for RGBA; RGB has neither and makes the font fail).
    let path = id_path(file, "textures/", "");
    let bytes = assets.read(&path).ok_or_else(|| format!("{path} not found"))?;
    let (img, kind) = Image::decode_png_typed(&bytes).ok_or_else(|| format!("{path} unreadable"))?;
    let channel = match kind {
        png::ColorType::Rgba => 3,
        png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => 0,
        png::ColorType::Rgb | png::ColorType::Indexed => return Err(format!("{path} has neither luminance nor alpha")),
    };
    let sheet = Arc::new(img);
    let mut mine: Glyphs = HashMap::new();
    if cols == 0 {
        return Ok(mine);
    }
    let (cw, ch) = (sheet.width / cols as u32, sheet.height / rows.len() as u32);
    if cw == 0 || ch == 0 {
        return Ok(mine);
    }
    let scale = height / ch as f32;
    for (r, row) in rows.iter().enumerate() {
        for (c, &cp) in row.iter().enumerate() {
            if cp == 0 {
                continue;
            }
            let (x0, y0) = (c as u32 * cw, r as u32 * ch);
            // Real width: rightmost column that has a non-zero pixel, plus 1.
            let w = (0..cw)
                .rev()
                .find(|&x| {
                    (0..ch).any(|y| sheet.rgba[(((y0 + y) * sheet.width + x0 + x) * 4) as usize + channel] != 0)
                })
                .map_or(0, |x| x + 1);
            let top = 7.0 - ascent;
            mine.insert(
                cp,
                Glyph {
                    advance: ((0.5 + w as f32 * scale) as i32 + 1) as f32,
                    offset: 1.0,
                    left: 0.0,
                    top,
                    right: cw as f32 * scale,
                    bottom: top + ch as f32 * scale,
                    bitmap: Some(Bitmap::Sheet { sheet: sheet.clone(), x: x0, y: y0, w: cw, h: ch }),
                },
            );
        }
    }
    Ok(mine)
}

/// TrueType provider: file `font/<file>` read once, glyphs produced on demand.
/// `Err`: file not found or unreadable. OpenType font with CFF outlines (read by the game, not
/// by us): empty provider (difference noted).
fn load_ttf(assets: &Assets, p: &Value) -> Result<Provider, String> {
    let file = p["file"].as_str().unwrap_or_default();
    let path = id_path(file, "font/", "");
    let bytes = assets.read(&path).ok_or_else(|| format!("{path} not found"))?;
    if skrifa::FontRef::new(&bytes).is_err() {
        return Err(format!("{path} unreadable"));
    }
    Ok(match ttf::TtfFace::new(bytes.into(), ttf::params(p)) {
        Some(face) => Provider::Ttf(Arc::new(face)),
        None => Provider::Glyphs(HashMap::new()),
    })
}

/// Unihex providers already read (file and settings → glyphs), kept as long as a font
/// uses them.
static UNIHEX: std::sync::Mutex<Vec<(String, std::sync::Weak<HexTable>)>> = std::sync::Mutex::new(Vec::new());

/// `Err`: file not found or non-conforming line.
fn load_unihex(assets: &Assets, p: &Value) -> Result<Arc<HexTable>, String> {
    let key = format!("{}|{}", p["hex_file"], p["size_overrides"]);
    let cached = UNIHEX.lock().ok().and_then(|c| c.iter().find(|(k, _)| *k == key).and_then(|(_, w)| w.upgrade()));
    if let Some(g) = cached {
        return Ok(g);
    }
    let file = p["hex_file"].as_str().unwrap_or_default();
    let path = id_path(file, "", "");
    let bytes = assets.read(&path).ok_or_else(|| format!("{path} not found"))?;
    let g = Arc::new(parse_unihex(bytes, p).map_err(|e| format!("{path} : {e}"))?);
    if let Ok(mut c) = UNIHEX.lock() {
        c.retain(|(_, w)| w.strong_count() > 0);
        c.push((key, Arc::downgrade(&g)));
    }
    Ok(g)
}

fn hex_digit(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some(u32::from(b - b'0')),
        b'A'..=b'F' => Some(u32::from(b - b'A' + 10)),
        _ => None,
    }
}

/// Uppercase hexadecimal digits (at most 8).
fn hex_value(s: &[u8]) -> Option<u32> {
    s.iter().try_fold(0u32, |a, &b| Some(a << 4 | hex_digit(b)?))
}

/// Lines of a `.hex` file, read like the game: 4 to 6 uppercase hexadecimal digits, a
/// colon, 32, 64, 96 or 128 digits, then `\n` or the end. Any other shape (empty line
/// in the middle, lowercase, `\r`) is an error. `f` receives (code point, left-aligned
/// rows, columns).
fn hex_lines(mut rest: &[u8], mut f: impl FnMut(u32, [u32; 16], u32)) -> Result<(), String> {
    let mut line = 0;
    loop {
        let colon = rest.iter().position(|&b| b == b':');
        let head = &rest[..colon.unwrap_or(rest.len())];
        let bad = || format!("line {line} non-conforming");
        let Some(colon) = colon else {
            return if head.is_empty() { Ok(()) } else { Err(bad()) };
        };
        if !(4..=6).contains(&head.len()) {
            return Err(bad());
        }
        let cp = hex_value(head).ok_or_else(bad)?;
        rest = &rest[colon + 1..];
        let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        let data = &rest[..end];
        rest = rest.get(end + 1..).unwrap_or_default();
        let cols = match data.len() {
            32 => 8,
            64 => 16,
            96 => 24,
            128 => 32,
            _ => return Err(bad()),
        };
        let digits = cols / 4;
        let mut rows = [0u32; 16];
        for (r, row) in rows.iter_mut().enumerate() {
            *row = hex_value(&data[r * digits..(r + 1) * digits]).ok_or_else(bad)? << (32 - cols);
        }
        f(cp, rows, cols as u32);
        line += 1;
    }
}

/// Unreadable archive: empty table (the game finds no entry in it); non-conforming line: `Err`.
fn parse_unihex(bytes: Vec<u8>, p: &Value) -> Result<HexTable, String> {
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else { return Ok(HexTable::default()) };
    let overrides: Vec<(u32, u32, u32, u32)> = p["size_overrides"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let ch = |k: &str| one_codepoint(o.get(k)).map(|c| c as u32);
            let dim = |k: &str| int(&o[k]).and_then(|v| u32::try_from(v).ok());
            Some((ch("from")?, ch("to")?, dim("left")?, dim("right")?))
        })
        .collect();
    let mut mine = HexTable::default();
    for i in 0..zip.len() {
        let Ok(mut e) = zip.by_index(i) else { continue };
        if !e.name().is_ok_and(|n| n.ends_with(".hex")) {
            continue;
        }
        let mut raw = Vec::new();
        e.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        hex_lines(&raw, |cp, rows, cols| {
            let (left, right) = match overrides.iter().find(|o| (o.0..=o.1).contains(&cp)) {
                Some(o) => (o.2, o.3),
                None => {
                    let all = rows.iter().fold(0, |a, &r| a | r);
                    if all == 0 { (0, cols) } else { (all.leading_zeros(), 31 - all.trailing_zeros()) }
                }
            };
            let width = (right + 1).saturating_sub(left);
            mine.push(cp, &rows, left, width);
        })?;
    }
    Ok(mine.finish())
}

/// Fonts of a launch, by identifier, for the resource cache: sheets and unihex tables
/// shared between fonts written only once, sharing restored on reading.
pub struct Fonts(pub Vec<(String, Font)>);

/// Glyphs by increasing code point (deterministic encoding).
fn sorted(m: &Glyphs) -> Vec<(&u32, &Glyph)> {
    let mut v: Vec<_> = m.iter().collect();
    v.sort_unstable_by_key(|e| *e.0);
    v
}

/// Glyphs of a layer; sheets by rank in `sheets`.
fn enc_glyphs(m: &Glyphs, sheets: &[&Arc<Image>], w: &mut crate::cache::W) {
    use crate::cache::Codec;
    m.len().enc(w);
    for (c, g) in sorted(m) {
        c.enc(w);
        (g.advance, g.offset, g.left, g.top, g.right).enc(w);
        g.bottom.enc(w);
        match &g.bitmap {
            None => 0u8.enc(w),
            Some(Bitmap::Sheet { sheet, x, y, w: bw, h }) => {
                1u8.enc(w);
                sheets.iter().position(|s| Arc::ptr_eq(s, sheet)).unwrap_or(0).enc(w);
                (*x, *y, *bw, *h).enc(w);
            }
            Some(Bitmap::Hex { rows, left, width }) => {
                2u8.enc(w);
                rows.enc(w);
                (*left, *width).enc(w);
            }
            Some(Bitmap::Gray { w: bw, h, data }) => {
                5u8.enc(w);
                (*bw, *h).enc(w);
                data.to_vec().enc(w);
            }
            Some(Bitmap::Missing) => 3u8.enc(w),
            Some(Bitmap::White) => 4u8.enc(w),
        }
    }
}

fn dec_glyphs(r: &mut crate::cache::R<'_>, sheets: &[Arc<Image>]) -> Option<Glyphs> {
    use crate::cache::Codec;
    let n = r.len()?;
    let mut m = HashMap::with_capacity(n);
    for _ in 0..n {
        let c = u32::dec(r)?;
        let (advance, offset, left, top, right) = Codec::dec(r)?;
        let bottom = f32::dec(r)?;
        let bitmap = match u8::dec(r)? {
            0 => None,
            1 => {
                let sheet = sheets.get(usize::dec(r)?)?.clone();
                let (x, y, w, h) = Codec::dec(r)?;
                Some(Bitmap::Sheet { sheet, x, y, w, h })
            }
            2 => {
                let rows = Codec::dec(r)?;
                let (left, width) = Codec::dec(r)?;
                Some(Bitmap::Hex { rows, left, width })
            }
            3 => Some(Bitmap::Missing),
            4 => Some(Bitmap::White),
            5 => {
                let (w, h) = Codec::dec(r)?;
                let data: Vec<u8> = Codec::dec(r)?;
                Some(Bitmap::Gray { w, h, data: data.into() })
            }
            _ => return None,
        };
        m.insert(c, Glyph { advance, offset, left, top, right, bottom, bitmap });
    }
    Some(m)
}

impl crate::cache::Codec for Fonts {
    fn enc(&self, w: &mut crate::cache::W) {
        let mut sheets: Vec<&Arc<Image>> = Vec::new();
        let mut hexes: Vec<&Arc<HexTable>> = Vec::new();
        for (_, f) in &self.0 {
            for l in &f.layers {
                match l {
                    Layer::Hex(t) => {
                        if !hexes.iter().any(|h| Arc::ptr_eq(h, t)) {
                            hexes.push(t);
                        }
                    }
                    Layer::Ttf(_) => {}
                    Layer::Own(m) | Layer::Alt(m) => {
                        for (_, g) in sorted(m) {
                            if let Some(Bitmap::Sheet { sheet, .. }) = &g.bitmap
                                && !sheets.iter().any(|s| Arc::ptr_eq(s, sheet))
                            {
                                sheets.push(sheet);
                            }
                        }
                    }
                }
            }
        }
        sheets.len().enc(w);
        for s in &sheets {
            (***s).enc(w);
        }
        hexes.len().enc(w);
        for h in &hexes {
            h.cps.enc(w);
            h.glyphs.enc(w);
            h.rows.enc(w);
        }
        self.0.len().enc(w);
        for (id, f) in &self.0 {
            id.enc(w);
            f.layers.len().enc(w);
            for l in &f.layers {
                match l {
                    Layer::Hex(t) => {
                        0u8.enc(w);
                        hexes.iter().position(|h| Arc::ptr_eq(h, t)).unwrap_or(0).enc(w);
                    }
                    Layer::Ttf(f) => {
                        2u8.enc(w);
                        f.data.to_vec().enc(w);
                        (f.params.size, f.params.oversample, f.params.shift.0, f.params.shift.1).enc(w);
                        f.params.skip.enc(w);
                    }
                    Layer::Own(m) => {
                        1u8.enc(w);
                        enc_glyphs(m, &sheets, w);
                    }
                    Layer::Alt(m) => {
                        3u8.enc(w);
                        enc_glyphs(m, &sheets, w);
                    }
                }
            }
        }
    }

    fn dec(r: &mut crate::cache::R<'_>) -> Option<Self> {
        use crate::cache::Codec;
        let sheets: Vec<Arc<Image>> = Codec::dec(r)?;
        let n = r.len()?;
        let mut hexes = Vec::with_capacity(n);
        for _ in 0..n {
            hexes.push(Arc::new(HexTable { cps: Codec::dec(r)?, glyphs: Codec::dec(r)?, rows: Codec::dec(r)? }));
        }
        let n = r.len()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let id = String::dec(r)?;
            let mut font = Font::default();
            for _ in 0..r.len()? {
                match u8::dec(r)? {
                    0 => font.layers.push(Layer::Hex(hexes.get(usize::dec(r)?)?.clone())),
                    1 => font.layers.push(Layer::Own(dec_glyphs(r, &sheets)?)),
                    3 => font.layers.push(Layer::Alt(dec_glyphs(r, &sheets)?)),
                    2 => {
                        let data: Vec<u8> = Codec::dec(r)?;
                        let (size, oversample, sx, sy) = Codec::dec(r)?;
                        let skip = String::dec(r)?;
                        let params = ttf::TtfParams { size, oversample, shift: (sx, sy), skip };
                        font.layers.push(Layer::Ttf(Arc::new(ttf::TtfFace::new(data.into(), params)?)));
                    }
                    _ => return None,
                }
            }
            out.push((id, font));
        }
        Some(Fonts(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::testkit::{dir, stack};

    const DEFAULT: &str = "assets/minecraft/font/default.json";

    fn space(advances: &str) -> String {
        format!(r#"{{"providers":[{{"type":"space","advances":{{{advances}}}}}]}}"#)
    }

    /// 16 × 8 PNG (two 8 × 8 cells), a single non-zero pixel in column 2 of the first.
    fn sheet(color: png::ColorType) -> Vec<u8> {
        let comps = match color {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            _ => 4,
        };
        let mut px = vec![0u8; 16 * 8 * comps];
        if comps == 4 {
            // Full alpha everywhere: the alpha width would be the whole cell.
            px.chunks_mut(4).for_each(|p| p[3] = 255);
        }
        px[(3 * 16 + 2) * comps] = 200;
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 16, 8);
        e.set_color(color);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().expect("header").write_image_data(&px).expect("pixels");
        out
    }

    fn bitmap_font(name: &str, color: png::ColorType) -> Font {
        let def = r#"{"providers":[{"type":"bitmap","file":"minecraft:font/g.png","ascent":7,"chars":["ab"]}]}"#;
        let png = sheet(color);
        let pack = dir(name, &[(DEFAULT, def.as_bytes()), ("assets/minecraft/textures/font/g.png", &png)]);
        let base = dir(&format!("{name}-base"), &[]);
        Font::load(&stack(&[&pack], &base), "minecraft:default", FontOptions::default())
    }

    #[test]
    fn font_definitions_stack_over_lower_packs() {
        let base = dir("fnt-base", &[(DEFAULT, space(r#""a":3,"b":4"#).as_bytes())]);
        let pack = dir("fnt-pack", &[(DEFAULT, space(r#""a":9"#).as_bytes())]);
        let broken = dir("fnt-broken", &[(DEFAULT, b"{ not json")]);
        let f = Font::load(&stack(&[&pack, &broken], &base), "minecraft:default", FontOptions::default());
        // The top pack first, without losing the glyphs of the lower packs.
        assert_eq!(f.advance('a' as u32), Some((9.0, 1.0)));
        assert_eq!(f.advance('b' as u32), Some((4.0, 1.0)));
    }

    #[test]
    fn missing_provider_file_fails_the_whole_font() {
        let def = r#"{"providers":[{"type":"space","advances":{"a":3}},
            {"type":"bitmap","file":"minecraft:font/absent.png","ascent":7,"chars":["b"]}]}"#;
        let pack = dir("fnt-miss", &[(DEFAULT, def.as_bytes())]);
        let base = dir("fnt-miss-base", &[]);
        let assets = stack(&[&pack], &base);
        assert!(Font::load(&assets, "minecraft:default", FontOptions::default()).is_empty());
        assert!(Font::try_load(&assets, "minecraft:default", FontOptions::default(), false).is_none());
    }

    #[test]
    fn fishy_glyphs_are_skipped_for_the_chat_field() {
        let def = r#"{"providers":[{"type":"space","advances":{"x":40,"y":-2,"z":31.5}},
            {"type":"space","advances":{"x":5,"y":40}}]}"#;
        let pack = dir("fnt-fishy", &[(DEFAULT, def.as_bytes())]);
        let base = dir("fnt-fishy-base", &[]);
        let f = Font::load(&stack(&[&pack], &base), "minecraft:default", FontOptions::default());
        assert_eq!(f.glyph('x' as u32).map(|g| g.advance), Some(40.0));
        assert!(f.first_is_fishy('x' as u32));
        // The second provider gives the first glyph of no code point: the game discards it,
        // the chat field shows the missing glyph.
        assert!(f.glyph_non_fishy('x' as u32).is_none());
        // All fishy: missing glyph.
        assert!(f.glyph_non_fishy('y' as u32).is_none());
        // 31.5 + 1 of bold exceeds 32: fishy too.
        assert!(f.first_is_fishy('z' as u32));

        // The same provider, used for "w": it also serves as fallback for "x".
        let def = r#"{"providers":[{"type":"space","advances":{"x":40,"y":-2}},
            {"type":"space","advances":{"x":5,"y":40,"w":3}}]}"#;
        let pack = dir("fnt-fishy-used", &[(DEFAULT, def.as_bytes())]);
        let f = Font::load(&stack(&[&pack], &base), "minecraft:default", FontOptions::default());
        assert_eq!(f.glyph_non_fishy('x' as u32).map(|g| g.advance), Some(5.0));
        assert!(f.glyph_non_fishy('y' as u32).is_none());
        // The choice goes through the resource cache.
        let bytes = crate::cache::encode(&Fonts(vec![("d".into(), f)]));
        let back: Fonts = crate::cache::decode(&bytes).expect("read back");
        assert_eq!(back.0[0].1.glyph_non_fishy('x' as u32).map(|g| g.advance), Some(5.0));
    }

    /// Flat table and borrowing (`RUSTCRAFT_OPT_FONTTABLE`) against the search through the layers:
    /// glyph, non-fishy glyph, advance and fishy flag identical for each tested
    /// code point (in the table, at its limit and beyond).
    fn assert_table_matches(f: &Font, label: &str, cps: impl Iterator<Item = u32>) -> usize {
        let same = |a: Option<&Glyph>, b: Option<&Glyph>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_as(b),
            _ => false,
        };
        let mut known = 0;
        for c in cps {
            let (old, new) = (f.glyph(c), f.glyph_ref(c));
            assert!(same(old.as_ref(), new.as_deref()), "{label} U+{c:04X} : glyph");
            let (old, new) = (f.glyph_non_fishy(c), f.glyph_non_fishy_ref(c));
            assert!(same(old.as_ref(), new.as_deref()), "{label} U+{c:04X} : non-fishy glyph");
            assert_eq!(f.advance(c), f.advance_ref(c), "{label} U+{c:04X} : advance");
            assert_eq!(f.first_is_fishy(c), f.first_is_fishy_ref(c), "{label} U+{c:04X} : fishy");
            known += usize::from(f.glyph(c).is_some());
        }
        known
    }

    #[test]
    fn flat_table_matches_layer_lookup() {
        // Fishy ones, fallback behind a fishy one, shared unihex, own layer behind unihex.
        let def = r#"{"providers":[{"type":"space","advances":{"x":40,"y":-2,"z":31.5}},
            {"type":"space","advances":{"x":5,"y":40,"w":3,"̀":7}}]}"#;
        let pack = dir("fnt-table", &[(DEFAULT, def.as_bytes())]);
        let base = dir("fnt-table-base", &[]);
        let f = Font::load(&stack(&[&pack], &base), "minecraft:default", FontOptions::default());
        assert!(assert_table_matches(&f, "spaces", 0..0x400) >= 5);
        let mut f = Font::default();
        let mut t = HexTable::default();
        for c in [0x41, 0x2ff, 0x300, 0x4e00] {
            t.push(c, &[0x8000_0001; 16], 0, 20);
        }
        f.add_provider(Provider::Hex(Arc::new(t.finish())));
        let g = Glyph { advance: 3.0, offset: 1.0, left: 0.0, top: 0.0, right: 0.0, bottom: 0.0, bitmap: None };
        f.add_provider(Provider::Glyphs([(0x42, g.clone()), (0x41, g.clone()), (0x4e01, g)].into_iter().collect()));
        assert_eq!(assert_table_matches(&f, "unihex", (0..0x400).chain(0x4e00..0x4e02)), 6);
        // Two passes: the second reads the already resolved entries.
        assert_eq!(assert_table_matches(&f, "unihex (2e)", (0..0x400).chain(0x4e00..0x4e02)), 6);
        let bitmap = bitmap_font("fnt-table-bmp", png::ColorType::Rgba);
        assert_eq!(assert_table_matches(&bitmap, "sheet", 0..0x400), 2);
    }

    /// Fonts of the installed game (all option variants): flat table == layers for the whole
    /// Basic Multilingual Plane. Without an installed game, skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
    #[test]
    fn flat_table_matches_game_fonts() {
        // Launcher asset index: Unicode fonts (unihex) included.
        let found = rc_data::locate("26.3").ok().and_then(|i| {
            let index = i.asset_index()?;
            crate::Assets::for_install(&i.client_jar, &[], Some((&index.0, &index.1))).ok()
        });
        let Some(a) = rc_testdata::need(found, "installed game 26.3 with its asset index") else { return };
        let ids = ["minecraft:default", "minecraft:uniform", "minecraft:alt", "minecraft:illageralt"];
        let mut total = 0;
        for uniform in [false, true] {
            for japanese in [false, true] {
                let o = FontOptions { uniform, jp: japanese };
                for id in ids {
                    let f = Font::load(&a, id, o);
                    total += assert_table_matches(&f, &format!("{id} {o:?}"), 0..0x1_0000);
                }
            }
        }
        assert!(total > 100_000, "game fonts almost empty ({total} glyphs)");
    }

    #[test]
    fn fully_shadowed_shared_layers_are_dropped() {
        let space = |m: &[(char, f32)]| {
            Provider::Glyphs(
                m.iter()
                    .map(|&(c, advance)| {
                        let g = Glyph { advance, offset: 1.0, left: 0.0, top: 0.0, right: 0.0, bottom: 0.0, bitmap: None };
                        (c as u32, g)
                    })
                    .collect(),
            )
        };
        let hex = |cps: &[char]| {
            let mut t = HexTable::default();
            for &c in cps {
                t.push(c as u32, &[0; 16], 0, 8);
            }
            Provider::Hex(Arc::new(t.finish()))
        };
        // "a" fishy first; the unihex table gives the first glyph of nothing: discarded.
        let mut f = Font::default();
        f.add_provider(space(&[('a', 40.0)]));
        f.add_provider(hex(&['a']));
        assert!(f.glyph_non_fishy('a' as u32).is_none());
        // Used for "b": it stays, and serves as fallback for "a".
        let mut f = Font::default();
        f.add_provider(space(&[('a', 40.0)]));
        f.add_provider(hex(&['a', 'b']));
        assert_eq!(f.glyph_non_fishy('a' as u32).map(|g| g.advance), Some(5.0));
    }

    #[test]
    fn a_non_conforming_provider_drops_its_whole_file() {
        let base = dir("fnt-codec-base", &[(DEFAULT, space(r#""a":3,"b":4"#).as_bytes())]);
        for (name, bad) in [
            ("ascent", r#"{"type":"bitmap","file":"minecraft:font/g.png","height":8,"ascent":9,"chars":["b"]}"#),
            ("chars", r#"{"type":"bitmap","file":"minecraft:font/g.png","ascent":7,"chars":["bc","d"]}"#),
            ("type", r#"{"type":"unknown"}"#),
            ("filter", r#"{"type":"space","advances":{"b":1},"filter":{"other":true}}"#),
            ("codepoint", r#"{"type":"space","advances":{"bc":1}}"#),
            ("id", r#"{"type":"reference","id":"Uppercase"}"#),
        ] {
            let def = format!(r#"{{"providers":[{{"type":"space","advances":{{"a":9}}}},{bad}]}}"#);
            let pack = dir(&format!("fnt-codec-{name}"), &[(DEFAULT, def.as_bytes())]);
            let f = Font::load(&stack(&[&pack], &base), "minecraft:default", FontOptions::default());
            // The whole pack file is discarded, including the conforming provider.
            assert_eq!(f.advance('a' as u32), Some((3.0, 1.0)), "{name}");
            assert_eq!(f.advance('b' as u32), Some((4.0, 1.0)), "{name}");
        }
    }

    #[test]
    fn providers_filtered_out_by_options_still_load() {
        let def = r#"{"providers":[{"type":"space","advances":{"a":3}},
            {"type":"bitmap","file":"minecraft:font/absent.png","ascent":7,"chars":["b"],"filter":{"jp":true}}]}"#;
        let pack = dir("fnt-filtered-miss", &[(DEFAULT, def.as_bytes())]);
        let base = dir("fnt-filtered-miss-base", &[]);
        let assets = stack(&[&pack], &base);
        // Japanese option disabled: the missing file still makes the font fail.
        assert!(Font::try_load(&assets, "minecraft:default", FontOptions::default(), false).is_none());
        // Tolerated fallback (stack without a pack): the failing provider is only ignored.
        let f = Font::try_load(&assets, "minecraft:default", FontOptions::default(), true).expect("fallback");
        assert_eq!(f.advance('a' as u32), Some((3.0, 1.0)));
    }

    #[test]
    fn unreadable_ttf_fails_the_font() {
        let def = r#"{"providers":[{"type":"space","advances":{"a":3}},{"type":"ttf","file":"minecraft:x.ttf"}]}"#;
        let pack = dir("fnt-badttf", &[(DEFAULT, def.as_bytes()), ("assets/minecraft/font/x.ttf", b"not a font")]);
        let base = dir("fnt-badttf-base", &[]);
        assert!(Font::try_load(&stack(&[&pack], &base), "minecraft:default", FontOptions::default(), false).is_none());
    }

    #[test]
    fn hex_lines_are_strict_like_the_game() {
        let row32 = "00".repeat(16);
        let ok = format!("0041:{row32}\n00E9:{row32}");
        let mut seen = Vec::new();
        assert!(hex_lines(ok.as_bytes(), |cp, _, cols| seen.push((cp, cols))).is_ok());
        assert_eq!(seen, [(0x41, 8), (0xE9, 8)]);
        for bad in [
            format!("0041:{row32}\r\n"),
            format!("0041:{row32}\n\n0042:{row32}"),
            format!("00e9:{row32}"),
            format!("041:{row32}"),
            "0041:00".to_owned(),
            format!("0041{row32}"),
        ] {
            assert!(hex_lines(bad.as_bytes(), |_, _, _| {}).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bitmap_width_reads_luminance_of_gray_sheets() {
        // Gray: width from luminance (column 2 → 3, advance 4).
        for c in [png::ColorType::Grayscale, png::ColorType::GrayscaleAlpha] {
            let f = bitmap_font(&format!("fnt-gray{c:?}"), c);
            assert_eq!(f.advance('a' as u32), Some((4.0, 1.0)), "{c:?}");
            assert_eq!(f.advance('b' as u32), Some((1.0, 1.0)), "{c:?}");
        }
        // RGBA: from alpha (full everywhere here → whole cell).
        let f = bitmap_font("fnt-rgba", png::ColorType::Rgba);
        assert_eq!(f.advance('b' as u32), Some((9.0, 1.0)));
        // RGB: neither luminance nor alpha, the font fails.
        assert!(bitmap_font("fnt-rgb", png::ColorType::Rgb).is_empty());
    }
}
