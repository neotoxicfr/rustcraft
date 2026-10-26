//! Technical blocks (`docs/specs/special-screens.md` § 6): data of the structure,
//! jigsaw, test and test instance blocks received from the server (chunks and `block_entity_data`), structure
//! boxes drawn in game, F3+I with the server's data (`tag_query`) and SNBT text
//! copied in the game's format. Test instances: error markers and `/test pos` positions
//! (the state beam goes through `block_entities.rs`).

use hashbrown::HashMap;
use std::time::{Duration, Instant};

use rc_proto::nbt::Nbt;

use crate::debug_render::{DebugLines, Layer};

/// Display duration of a `/test pos` position.
const HIGHLIGHT_TIME: Duration = Duration::from_secs(10);

/// Type of technical block tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TechKind {
    Structure,
    Jigsaw,
    Test,
    TestInstance,
}

impl TechKind {
    fn registry_name(self) -> &'static str {
        match self {
            TechKind::Structure => "minecraft:structure_block",
            TechKind::Jigsaw => "minecraft:jigsaw",
            TechKind::Test => "minecraft:test_block",
            TechKind::TestInstance => "minecraft:test_instance_block",
        }
    }
    const ALL: [TechKind; 4] = [TechKind::Structure, TechKind::Jigsaw, TechKind::Test, TechKind::TestInstance];
}

/// Settings of a structure block (default values and bounds of the game).
#[derive(Debug, Clone, PartialEq)]
pub struct Structure {
    pub name: String,
    pub metadata: String,
    /// Offset (−48..48) and size (0..48).
    pub pos: [i32; 3],
    pub size: [i32; 3],
    /// 0 none, 1 90°, 2 180°, 3 270° (counter-clockwise 90).
    pub rotation: i32,
    /// 0 none, 1 left-right, 2 front-back.
    pub mirror: i32,
    /// 0 save, 1 load, 2 corner, 3 data.
    pub mode: i32,
    pub ignore_entities: bool,
    pub strict: bool,
    pub show_air: bool,
    pub show_bounding_box: bool,
    pub integrity: f32,
    pub seed: i64,
}

impl Structure {
    pub fn new(mode: i32) -> Structure {
        Structure {
            name: String::new(),
            metadata: String::new(),
            pos: [0, 1, 0],
            size: [0, 0, 0],
            rotation: 0,
            mirror: 0,
            mode,
            ignore_entities: true,
            strict: false,
            show_air: false,
            show_bounding_box: true,
            integrity: 1.0,
            seed: 0,
        }
    }

    fn from_nbt(n: &Nbt) -> Structure {
        let int = |k: &str, d: i32| n.get(k).and_then(Nbt::as_i64).map_or(d, |v| v as i32);
        let flag = |k: &str, d: bool| n.get(k).and_then(Nbt::as_i64).map_or(d, |v| v != 0);
        let s = |k: &str| n.get(k).and_then(Nbt::as_str).unwrap_or("");
        let pick = |k: &str, names: &[&str], d: i32| {
            let v = s(k);
            names.iter().position(|x| x.eq_ignore_ascii_case(v)).map_or(d, |i| i as i32)
        };
        Structure {
            name: s("name").to_owned(),
            metadata: s("metadata").to_owned(),
            pos: [int("posX", 0).clamp(-48, 48), int("posY", 1).clamp(-48, 48), int("posZ", 0).clamp(-48, 48)],
            size: [int("sizeX", 0).clamp(0, 48), int("sizeY", 0).clamp(0, 48), int("sizeZ", 0).clamp(0, 48)],
            rotation: pick("rotation", &["NONE", "CLOCKWISE_90", "CLOCKWISE_180", "COUNTERCLOCKWISE_90"], 0),
            mirror: pick("mirror", &["NONE", "LEFT_RIGHT", "FRONT_BACK"], 0),
            mode: pick("mode", &["SAVE", "LOAD", "CORNER", "DATA"], 3),
            ignore_entities: flag("ignoreEntities", true),
            strict: flag("strict", false),
            show_air: flag("showair", false),
            show_bounding_box: flag("showboundingbox", true),
            integrity: match n.get("integrity") {
                Some(Nbt::Float(f)) => *f,
                Some(Nbt::Double(d)) => *d as f32,
                _ => 1.0,
            },
            seed: n.get("seed").and_then(Nbt::as_i64).unwrap_or(0),
        }
    }

    /// Render mode: 0 nothing, 1 box, 2 box and invisible blocks.
    pub fn render_mode(&self) -> u8 {
        match self.mode {
            0 if self.show_air => 2,
            0 => 1,
            1 if self.show_bounding_box => 1,
            _ => 0,
        }
    }

    /// Box relative to the block: (corner, size), from offset, mirror and rotation.
    pub fn renderable_box(&self) -> ([i32; 3], [i32; 3]) {
        let [xo, y0, zo] = self.pos;
        let y1 = y0 + self.size[1];
        let (xd, zd) = match self.mirror {
            1 => (self.size[0], -self.size[2]),
            2 => (-self.size[0], self.size[2]),
            _ => (self.size[0], self.size[2]),
        };
        let (x0, z0, x1, z1);
        match self.rotation {
            1 => {
                x0 = if zd < 0 { xo } else { xo + 1 };
                z0 = if xd < 0 { zo + 1 } else { zo };
                x1 = x0 - zd;
                z1 = z0 + xd;
            }
            2 => {
                x0 = if xd < 0 { xo } else { xo + 1 };
                z0 = if zd < 0 { zo } else { zo + 1 };
                x1 = x0 - xd;
                z1 = z0 - zd;
            }
            3 => {
                x0 = if zd < 0 { xo + 1 } else { xo };
                z0 = if xd < 0 { zo } else { zo + 1 };
                x1 = x0 + zd;
                z1 = z0 - xd;
            }
            _ => {
                x0 = if xd < 0 { xo + 1 } else { xo };
                z0 = if zd < 0 { zo + 1 } else { zo };
                x1 = x0 + xd;
                z1 = z0 + zd;
            }
        }
        from_corners([x0, y0, z0], [x1, y1, z1])
    }
}

fn from_corners(a: [i32; 3], b: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    let lo = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
    (lo, [a[0].max(b[0]) - lo[0], a[1].max(b[1]) - lo[1], a[2].max(b[2]) - lo[2]])
}

/// Jigsaw block.
#[derive(Debug, Clone, PartialEq)]
pub struct Jigsaw {
    pub name: String,
    pub target: String,
    pub pool: String,
    pub final_state: String,
    /// `None`: default joint according to the orientation.
    pub aligned: Option<bool>,
    pub selection_priority: i32,
    pub placement_priority: i32,
}

impl Jigsaw {
    fn from_nbt(n: &Nbt) -> Jigsaw {
        let s = |k: &str, d: &str| n.get(k).and_then(Nbt::as_str).unwrap_or(d).to_owned();
        let int = |k: &str| n.get(k).and_then(Nbt::as_i64).map_or(0, |v| v as i32);
        Jigsaw {
            name: s("name", "minecraft:empty"),
            target: s("target", "minecraft:empty"),
            pool: s("pool", "minecraft:empty"),
            final_state: s("final_state", "minecraft:air"),
            aligned: n.get("joint").and_then(Nbt::as_str).map(|j| j == "aligned"),
            selection_priority: int("selection_priority"),
            placement_priority: int("placement_priority"),
        }
    }
}

impl Default for Jigsaw {
    fn default() -> Self {
        Jigsaw::from_nbt(&Nbt::Compound(Vec::new()))
    }
}

/// Test instance block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TestInstance {
    pub test: Option<String>,
    pub size: [i32; 3],
    pub rotation: i32,
    pub ignore_entities: bool,
    /// State of the last run: 0 cleared, 1 running, 2 finished.
    pub status: u8,
    /// Error message of the last run (plain text of the component).
    pub error: Option<rc_text::Component>,
    /// Error markers (absolute position, text).
    pub errors: Vec<([i32; 3], rc_text::Component)>,
}

/// Client settings of a test of the synchronised `test_instance` registry: required test,
/// margin around the structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestDef {
    pub required: bool,
    pub padding: i32,
    /// Rotation specific to the test (0 none, 1 clockwise 90, 2 180, 3 counter-clockwise 90).
    pub rotation: i32,
}

/// `test_instance` registry received at configuration (id → settings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestDefs(HashMap<String, TestDef>);

impl TestDefs {
    /// Registry entries; content omitted (base pack known): settings of the base pack's only
    /// test (`always_pass`, optional, no margin), otherwise default values of the format.
    pub fn from_entries(entries: &[(String, Option<Nbt>)]) -> TestDefs {
        let def = |name: &str, n: Option<&Nbt>| match n {
            Some(n) => TestDef {
                required: n.get("required").and_then(Nbt::as_i64).is_none_or(|v| v != 0),
                padding: n.get("padding").and_then(Nbt::as_i64).map_or(0, |v| v.clamp(0, 128) as i32),
                rotation: match n.get("rotation").and_then(Nbt::as_str) {
                    Some("clockwise_90") => 1,
                    Some("180") => 2,
                    Some("counterclockwise_90") => 3,
                    _ => 0,
                },
            },
            None => TestDef { required: name != "minecraft:always_pass", padding: 0, rotation: 0 },
        };
        TestDefs(entries.iter().map(|(name, n)| (name.clone(), def(name, n.as_ref()))).collect())
    }

    /// Settings of a test; test absent or unknown: required, no margin.
    pub fn get(&self, test: Option<&str>) -> TestDef {
        test.and_then(|t| self.0.get(t)).copied().unwrap_or(TestDef { required: true, padding: 0, rotation: 0 })
    }
}

impl TestInstance {
    pub(crate) fn from_nbt(n: &Nbt) -> TestInstance {
        let errors = match n.get("errors") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|e| {
                    let pos = match e.get("pos")? {
                        Nbt::IntArray(v) if v.len() == 3 => [v[0], v[1], v[2]],
                        _ => return None,
                    };
                    Some((pos, rc_text::from_nbt(e.get("text")?)?))
                })
                .collect(),
            _ => Vec::new(),
        };
        let Some(d) = n.get("data") else { return TestInstance { errors, ..TestInstance::default() } };
        let size = match d.get("size") {
            Some(Nbt::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
            Some(Nbt::List(v)) if v.len() == 3 => std::array::from_fn(|i| v[i].as_i64().map_or(0, |x| x as i32)),
            _ => [0, 0, 0],
        };
        let rotation = match d.get("rotation").and_then(Nbt::as_str) {
            Some("clockwise_90") => 1,
            Some("180") => 2,
            Some("counterclockwise_90") => 3,
            _ => 0,
        };
        TestInstance {
            test: d.get("test").and_then(Nbt::as_str).map(str::to_owned),
            size,
            rotation,
            ignore_entities: d.get("ignore_entities").and_then(Nbt::as_i64).is_some_and(|v| v != 0),
            status: match d.get("status").and_then(Nbt::as_str) {
                Some("running") => 1,
                Some("finished") => 2,
                _ => 0,
            },
            error: d.get("error_message").and_then(rc_text::from_nbt),
            errors,
        }
    }

    /// Effective rotation: the test's composed with the block's.
    pub fn combined_rotation(&self, def: TestDef) -> i32 {
        (def.rotation + self.rotation).rem_euclid(4)
    }

    /// Box relative to the block: offset (0, 1, 1) plus the test's margin on each axis,
    /// size rotated by the effective rotation.
    pub fn renderable_box(&self, def: TestDef) -> ([i32; 3], [i32; 3]) {
        let s = self.size;
        let padding = def.padding;
        let rot = self.combined_rotation(def);
        let swap = rot == 1 || rot == 3;
        ([padding, 1 + padding, 1 + padding], if swap { [s[2], s[1], s[0]] } else { s })
    }

    /// State beam colour: nothing if cleared, grey running, green passed, red failure
    /// of a required (or unknown) test, orange failure of an optional test.
    pub fn beam(&self, defs: &TestDefs) -> Option<[u8; 3]> {
        match self.status {
            1 => Some([128, 128, 128]),
            2 if self.error.is_none() => Some([0, 255, 0]),
            2 if defs.get(self.test.as_deref()).required => Some([255, 0, 0]),
            2 => Some([255, 128, 0]),
            _ => None,
        }
    }
}

/// Known data of a technical block.
#[derive(Debug, Clone, PartialEq)]
pub enum Tech {
    Structure(Structure),
    Jigsaw(Jigsaw),
    /// Mode (0 start, 1 log, 2 fail, 3 accept) and message.
    Test(i32, String),
    TestInstance(TestInstance),
}

/// F3+I request pending: what to copy on the reply.
#[derive(Debug, Clone)]
enum Pending {
    Block { pos: [i32; 3], state: String },
    Entity { id: String, pos: [f64; 3] },
}

#[derive(Default)]
pub struct TechBlocks {
    /// Block entity type (registry index) → type tracked.
    types: Vec<Option<TechKind>>,
    pub map: HashMap<[i32; 3], Tech>,
    /// Raw data received (F3+I on the client side, Shift).
    pub raw: HashMap<[i32; 3], Nbt>,
    transaction: i32,
    pending: Option<(i32, Pending)>,
    /// Tests registry (margins, required tests).
    pub tests: TestDefs,
    /// `/test pos` positions: absolute block → (relative coordinates, end of display).
    highlights: HashMap<[i32; 3], (String, Instant)>,
}

impl TechBlocks {
    pub fn new(data: &rc_data::VersionData) -> TechBlocks {
        let types = data
            .registry("minecraft:block_entity_type")
            .map(|r| r.entries.iter().map(|n| TechKind::ALL.into_iter().find(|k| k.registry_name() == n)).collect())
            .unwrap_or_default();
        TechBlocks { types, transaction: -1, ..Default::default() }
    }

    fn kind(&self, k: i32) -> Option<TechKind> {
        usize::try_from(k).ok().and_then(|k| self.types.get(k)).copied().flatten()
    }

    /// World change: block data and reported positions forgotten.
    pub fn clear(&mut self) {
        self.map.clear();
        self.raw.clear();
        self.highlights.clear();
    }

    /// `game_test_highlight_pos`: position shown for 10 s (replaces a mark on the same block).
    pub fn highlight(&mut self, absolute: [i32; 3], relative: [i32; 3], now: Instant) {
        let text = format!("{}, {}, {}", relative[0], relative[1], relative[2]);
        self.highlights.insert(absolute, (text, now + HIGHLIGHT_TIME));
    }

    /// Positions whose delay has passed removed (displayed up to and including the end instant).
    fn expire_highlights(&mut self, now: Instant) {
        self.highlights.retain(|_, (_, end)| now <= *end);
    }

    fn put(&mut self, pos: [i32; 3], kind: TechKind, nbt: Option<&Nbt>) {
        let empty = Nbt::Compound(Vec::new());
        let n = nbt.unwrap_or(&empty);
        let t = match kind {
            TechKind::Structure => Tech::Structure(Structure::from_nbt(n)),
            TechKind::Jigsaw => Tech::Jigsaw(Jigsaw::from_nbt(n)),
            TechKind::Test => {
                let mode = match n.get("mode").and_then(Nbt::as_str) {
                    Some("start") => 0,
                    Some("log") => 1,
                    Some("accept") => 3,
                    _ => 2,
                };
                Tech::Test(mode, n.get("message").and_then(Nbt::as_str).unwrap_or("").to_owned())
            }
            TechKind::TestInstance => Tech::TestInstance(TestInstance::from_nbt(n)),
        };
        self.map.insert(pos, t);
        if let Some(n) = nbt {
            self.raw.insert(pos, n.clone());
        }
    }

    pub fn load_chunk(&mut self, cx: i32, cz: i32, list: &[rc_proto_v777::chunk::BlockEntity]) {
        self.forget_chunk(cx, cz);
        for be in list {
            if let Some(k) = self.kind(be.kind) {
                let pos = [cx * 16 + i32::from(be.x), i32::from(be.y), cz * 16 + i32::from(be.z)];
                self.put(pos, k, be.data.as_ref());
            }
        }
    }

    pub fn on_data(&mut self, pos: [i32; 3], kind: i32, nbt: Option<&Nbt>) {
        if let Some(k) = self.kind(kind) {
            self.put(pos, k, nbt);
        }
    }

    pub fn forget_chunk(&mut self, cx: i32, cz: i32) {
        let inside = |p: &[i32; 3]| p[0] >> 4 == cx && p[2] >> 4 == cz;
        self.map.retain(|p, _| !inside(p));
        self.raw.retain(|p, _| !inside(p));
    }

    /// Block replaced: its data leaves if the block changes type.
    pub fn on_block_change(&mut self, pos: [i32; 3], same_block: bool) {
        if !same_block {
            self.map.remove(&pos);
            self.raw.remove(&pos);
        }
    }

    /// Structure at this position, otherwise default settings of the state's mode.
    pub fn structure(&self, pos: [i32; 3], state_mode: i32) -> Structure {
        match self.map.get(&pos) {
            Some(Tech::Structure(s)) => s.clone(),
            _ => Structure::new(state_mode),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Boxes in game.

const BOX: u32 = 0xFFE5_E5E5;

/// Colour multiplied component by component (`a·b/255`).
fn multiply(a: u32, b: u32) -> u32 {
    let ch = |s: u32| ((a >> s & 0xFF) * (b >> s & 0xFF) / 255) << s;
    ch(24) | ch(16) | ch(8) | ch(0)
}

/// Rendering of the technical block at `pos` takes place: structure block within 96 blocks of the
/// camera; test instance too as long as its beam is in range (flat distance under the
/// render distance × 16).
fn in_range(t: &Tech, pos: [i32; 3], eye: [f64; 3], render_distance: i32) -> bool {
    let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    let flat = (c[0] - eye[0]).powi(2) + (c[2] - eye[2]).powi(2);
    if flat + (c[1] - eye[1]).powi(2) < 96.0 * 96.0 {
        return true;
    }
    let beam = f64::from(render_distance * 16);
    matches!(t, Tech::TestInstance(_)) && flat < beam * beam
}

/// Boxes of the structure blocks (save, load) and test instance, visible to
/// a creative operator or a spectator.
pub fn draw_boxes(
    lines: &mut DebugLines,
    tech: &TechBlocks,
    eye: [f64; 3],
    visible: bool,
    render_distance: i32,
    state: &dyn Fn(i32, i32, i32) -> u32,
    data: &rc_data::VersionData,
) {
    if !visible || tech.map.is_empty() {
        return;
    }
    let origin = [eye[0].floor() as i32, eye[2].floor() as i32];
    let rel = |x: f64, y: f64, z: f64| [(x - f64::from(origin[0])) as f32, y as f32, (z - f64::from(origin[1])) as f32];
    for (pos, t) in &tech.map {
        if !in_range(t, *pos, eye, render_distance) {
            continue;
        }
        let (mode, (lp, size)) = match t {
            Tech::Structure(s) => (s.render_mode(), s.renderable_box()),
            Tech::TestInstance(i) => (1, i.renderable_box(tech.tests.get(i.test.as_deref()))),
            _ => continue,
        };
        if mode == 0 || size.iter().any(|&v| v < 1) {
            continue;
        }
        let p = [pos[0] + lp[0], pos[1] + lp[1], pos[2] + lp[2]];
        let f = |i: usize, o: i32| f64::from(p[i] + o);
        let a = rel(f(0, 0), f(1, 0), f(2, 0));
        let b = rel(f(0, size[0]), f(1, size[1]), f(2, size[2]));
        // Edges of the origin corner tinted (x red, y green, z blue), then the other nine.
        lines.line(Layer::Depth, a, [b[0], a[1], a[2]], multiply(BOX, 0xFFFF_7777), 2.5);
        lines.line(Layer::Depth, a, [a[0], b[1], a[2]], multiply(BOX, 0xFF77_FF77), 2.5);
        lines.line(Layer::Depth, a, [a[0], a[1], b[2]], multiply(BOX, 0xFF77_77FF), 2.5);
        let edges = [
            ([b[0], a[1], a[2]], [b[0], b[1], a[2]]),
            ([b[0], b[1], a[2]], [a[0], b[1], a[2]]),
            ([a[0], b[1], a[2]], [a[0], b[1], b[2]]),
            ([a[0], b[1], b[2]], [a[0], a[1], b[2]]),
            ([a[0], a[1], b[2]], [b[0], a[1], b[2]]),
            ([b[0], a[1], b[2]], [b[0], a[1], a[2]]),
            ([a[0], b[1], b[2]], [b[0], b[1], b[2]]),
            ([b[0], a[1], b[2]], [b[0], b[1], b[2]]),
            ([b[0], b[1], a[2]], [b[0], b[1], b[2]]),
        ];
        for (u, v) in edges {
            lines.line(Layer::Depth, u, v, BOX, 2.5);
        }
        if mode == 2 {
            invisible_blocks(lines, p, size, state, data, &rel);
        }
    }
}

/// Markers of the invisible blocks of a save area (air, structure void, barrier,
/// light).
fn invisible_blocks(
    lines: &mut DebugLines,
    start: [i32; 3],
    size: [i32; 3],
    state: &dyn Fn(i32, i32, i32) -> u32,
    data: &rc_data::VersionData,
    rel: &dyn Fn(f64, f64, f64) -> [f32; 3],
) {
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..size[2] {
                let (bx, by, bz) = (start[0] + x, start[1] + y, start[2] + z);
                let Some(b) = data.block_of(state(bx, by, bz)) else { continue };
                let (color, scale) = match b.name.as_str() {
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => (0xFF7F_7FFF, 0.05),
                    "minecraft:structure_void" => (0xFFFF_BFBF, 0.0),
                    "minecraft:barrier" => (0xFFFF_0000, 0.0),
                    "minecraft:light" => (0xFFFF_FF00, 0.0),
                    _ => continue,
                };
                let lo = 0.45f32 - scale;
                let hi = 0.55f32 + scale;
                let min =
                    rel(f64::from(bx) + f64::from(lo), f64::from(by) + f64::from(lo), f64::from(bz) + f64::from(lo));
                let max =
                    rel(f64::from(bx) + f64::from(hi), f64::from(by) + f64::from(hi), f64::from(bz) + f64::from(hi));
                lines.cuboid(Layer::Depth, min, max, color, 2.5);
            }
        }
    }
}

/// Error marker of a test instance: translucent red cube (alpha ⌊0.375 × 255⌋).
const ERROR_FILL: u32 = 0x5FFF_0000;
/// `/test pos` position: translucent green cube.
const HIGHLIGHT_FILL: u32 = 0x6000_FF00;
/// Margin of the cubes around the block; scale and height of the text above.
const MARK_PADDING: f32 = 0.02;
const MARK_TEXT_SCALE: f32 = 0.16;
const MARK_TEXT_Y: f64 = 1.2;

/// Solid cube around block `p` and white text centred above, always in front.
fn mark(
    lines: &mut DebugLines,
    gui: &mut crate::gui::Gui,
    queue: &wgpu::Queue,
    origin: [i32; 2],
    p: [i32; 3],
    fill: u32,
    text: &str,
) {
    let (x, y, z) = ((p[0] - origin[0]) as f32, p[1] as f32, (p[2] - origin[1]) as f32);
    let m = MARK_PADDING;
    lines.filled_cuboid([x - m, y - m, z - m], [x + 1.0 + m, y + 1.0 + m, z + 1.0 + m], fill);
    if !text.is_empty() {
        let at = [x + 0.5, (f64::from(p[1]) + MARK_TEXT_Y) as f32, z + 0.5];
        lines.front_text(gui, queue, at, text, 0xFFFF_FFFF, MARK_TEXT_SCALE);
    }
}

/// Error markers of the test instances in range (for all players) and `/test pos` positions
/// still valid (the expired ones are removed).
pub fn draw_markers(
    lines: &mut DebugLines,
    gui: &mut crate::gui::Gui,
    queue: &wgpu::Queue,
    tech: &mut TechBlocks,
    eye: [f64; 3],
    render_distance: i32,
    now: Instant,
) {
    tech.expire_highlights(now);
    if tech.highlights.is_empty() && tech.map.is_empty() {
        return;
    }
    let origin = [eye[0].floor() as i32, eye[2].floor() as i32];
    for (pos, t) in &tech.map {
        let Tech::TestInstance(i) = t else { continue };
        if i.errors.is_empty() || !in_range(t, *pos, eye, render_distance) {
            continue;
        }
        for (p, text) in &i.errors {
            let s = rc_text::plain_string(text, &gui.lang);
            mark(lines, gui, queue, origin, *p, ERROR_FILL, &s);
        }
    }
    for (p, (text, _)) in &tech.highlights {
        mark(lines, gui, queue, origin, *p, HIGHLIGHT_FILL, text);
    }
}

impl crate::App {
    /// `test_instance` registry received: margins and required tests for the boxes and the
    /// beams.
    pub(crate) fn set_test_registry(&mut self, entries: &[(String, Option<Nbt>)]) {
        let defs = TestDefs::from_entries(entries);
        self.block_entities.set_test_defs(defs.clone());
        self.tech.tests = defs;
    }
}

// ---------------------------------------------------------------------------------------------
// F3+I.

impl crate::App {
    /// F3+I (§ 6.4): command to recreate the target. Operator: server data (without
    /// Shift) or client data (Shift); otherwise without data.
    pub(crate) fn debug_copy_recreate(&mut self, shift: bool) {
        let add_nbt = self.debug.op_level >= 2;
        let pull = !shift;
        match self.pick() {
            crate::pick::Hit::Block { pos, .. } => {
                let view = crate::environ::WorldView {
                    grid: &self.world.grid,
                    min_y: self.world.min_y,
                    height: self.world.height,
                };
                let state = view.state(pos[0], pos[1], pos[2]);
                let name = self.data.block_of(state).map_or_else(|| "minecraft:air".into(), |b| b.name.clone());
                let props: Vec<String> = self.data.state_properties(state).map(|(k, v)| format!("{k}={v}")).collect();
                let state = if props.is_empty() { name } else { format!("{name}[{}]", props.join(",")) };
                if add_nbt && pull {
                    let t = self.next_query(Pending::Block { pos, state });
                    let _ = self.session.link.send_block_entity_tag_query(t, pos);
                    return;
                }
                let tag = if add_nbt { self.tech.raw.get(&pos).cloned() } else { None };
                self.copy_block_command(pos, &state, tag.as_ref());
                self.feedback("debug.inspect.client.block");
            }
            crate::pick::Hit::Entity { id, .. } => {
                let Some(e) = self.entities.map.get(&id) else { return };
                let (kind, pos) = (format!("minecraft:{}", e.kind), e.pos);
                if add_nbt && pull {
                    let t = self.next_query(Pending::Entity { id: kind, pos });
                    let _ = self.session.link.send_entity_tag_query(t, id);
                    return;
                }
                // Client-side entity data not reconstructed: command without data.
                copy_entity_command(&kind, pos, None);
                self.feedback("debug.inspect.client.entity");
            }
            crate::pick::Hit::Miss => {}
        }
    }

    fn next_query(&mut self, p: Pending) -> i32 {
        self.tech.transaction += 1;
        self.tech.pending = Some((self.tech.transaction, p));
        self.tech.transaction
    }

    fn copy_block_command(&self, pos: [i32; 3], state: &str, tag: Option<&Nbt>) {
        let mut d = state.to_owned();
        if let Some(t) = tag {
            d.push_str(&snbt_compact(t));
        }
        crate::debug_sys::set_clipboard(&format!("/setblock {} {} {} {d}", pos[0], pos[1], pos[2]));
    }

    /// `tag_query`: reply to the request in progress (the others are ignored).
    pub(crate) fn tag_query(&mut self, id: i32, nbt: Option<Nbt>) {
        let Some((t, p)) = self.tech.pending.take() else { return };
        if t != id {
            self.tech.pending = Some((t, p));
            return;
        }
        match p {
            Pending::Block { pos, state } => {
                self.copy_block_command(pos, &state, nbt.as_ref());
                self.feedback("debug.inspect.server.block");
            }
            Pending::Entity { id, pos } => {
                copy_entity_command(&id, pos, nbt.as_ref());
                self.feedback("debug.inspect.server.entity");
            }
        }
    }
}

fn copy_entity_command(id: &str, pos: [f64; 3], tag: Option<&Nbt>) {
    let p = format!("{} {} {}", java_fixed2(pos[0]), java_fixed2(pos[1]), java_fixed2(pos[2]));
    let cmd = match tag {
        Some(t) => format!("/summon {id} {p} {}", snbt_pretty_entity(t)),
        None => format!("/summon {id} {p}"),
    };
    crate::debug_sys::set_clipboard(&cmd);
}

// ---------------------------------------------------------------------------------------------
// SNBT text in the game's format.

/// Shortest decimal digits: (negative, digits, position of the point).
fn shortest(e: &str) -> (bool, Vec<u8>, i32) {
    let (neg, e) = e.strip_prefix('-').map_or((false, e), |r| (true, r));
    let (m, x) = e.split_once('e').unwrap_or((e, "0"));
    let digits: Vec<u8> = m.bytes().filter(u8::is_ascii_digit).collect();
    (neg, digits, x.parse::<i32>().unwrap_or(0) + 1)
}

/// Conversion of a float to text, the Java way, from Rust's `{:e}` form.
fn java_fp(v: f64, e: String) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let (neg, d, point) = shortest(&e);
    let sign = if neg { "-" } else { "" };
    let a = v.abs();
    if a == 0.0 {
        return format!("{sign}0.0");
    }
    let ds: String = d.iter().map(|&b| b as char).collect();
    if (1e-3..1e7).contains(&a) {
        if point <= 0 {
            format!("{sign}0.{}{ds}", "0".repeat((-point) as usize))
        } else if point as usize >= ds.len() {
            format!("{sign}{ds}{}.0", "0".repeat(point as usize - ds.len()))
        } else {
            format!("{sign}{}.{}", &ds[..point as usize], &ds[point as usize..])
        }
    } else {
        let frac = if ds.len() > 1 { &ds[1..] } else { "0" };
        format!("{sign}{}.{frac}E{}", &ds[..1], point - 1)
    }
}

fn java_float(v: f32) -> String {
    java_fp(f64::from(v), format!("{v:e}"))
}

fn java_double(v: f64) -> String {
    java_fp(v, format!("{v:e}"))
}

/// Java's `%.2f`: "half up" rounding applied to the shortest digits.
pub fn java_fixed2(v: f64) -> String {
    if !v.is_finite() {
        return java_double(v);
    }
    let (neg, mut d, mut point) = shortest(&format!("{v:e}"));
    if point < 0 {
        let mut z = vec![b'0'; (-point) as usize];
        z.extend(d);
        d = z;
        point = 0;
    }
    let keep = point as usize + 2;
    if d.len() > keep {
        let up = d[keep] >= b'5';
        d.truncate(keep);
        if up {
            let mut i = keep;
            loop {
                if i == 0 {
                    d.insert(0, b'1');
                    point += 1;
                    break;
                }
                i -= 1;
                if d[i] == b'9' {
                    d[i] = b'0';
                } else {
                    d[i] += 1;
                    break;
                }
            }
        }
    }
    let keep = point as usize + 2;
    d.resize(keep, b'0');
    let p = point as usize;
    let int: String = if p == 0 { "0".into() } else { d[..p].iter().map(|&b| b as char).collect() };
    let frac: String = d[p..].iter().map(|&b| b as char).collect();
    format!("{}{int}.{frac}", if neg { "-" } else { "" })
}

/// Quotes and escapes of the SNBT strings (the quote that does not appear first).
fn quote(s: &str, out: &mut String) {
    let at = out.len();
    out.push(' ');
    let mut q: Option<char> = None;
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' | '\'' => {
                let qq = *q.get_or_insert(if c == '"' { '\'' } else { '"' });
                if qq == c {
                    out.push('\\');
                }
                out.push(c);
            }
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    let q = q.unwrap_or('"');
    out.replace_range(at..at + 1, &q.to_string());
    out.push(q);
}

/// Compact SNBT (block tag): keys sorted, no spaces.
pub fn snbt_compact(n: &Nbt) -> String {
    let mut s = String::new();
    compact(n, &mut s);
    s
}

fn compact(n: &Nbt, o: &mut String) {
    let join = |o: &mut String, items: Vec<String>, open: &str| {
        o.push_str(open);
        o.push_str(&items.join(","));
        o.push(']');
    };
    match n {
        Nbt::Byte(v) => o.push_str(&format!("{v}b")),
        Nbt::Short(v) => o.push_str(&format!("{v}s")),
        Nbt::Int(v) => o.push_str(&v.to_string()),
        Nbt::Long(v) => o.push_str(&format!("{v}L")),
        Nbt::Float(v) => o.push_str(&format!("{}f", java_float(*v))),
        Nbt::Double(v) => o.push_str(&format!("{}d", java_double(*v))),
        Nbt::String(v) => quote(v, o),
        Nbt::ByteArray(v) => join(o, v.iter().map(|&b| format!("{}B", b as i8)).collect(), "[B;"),
        Nbt::IntArray(v) => join(o, v.iter().map(i32::to_string).collect(), "[I;"),
        Nbt::LongArray(v) => join(o, v.iter().map(|x| format!("{x}L")).collect(), "[L;"),
        Nbt::List(v) => join(o, v.iter().map(snbt_compact).collect(), "["),
        Nbt::Compound(v) => {
            let mut e: Vec<&(String, Nbt)> = v.iter().collect();
            e.sort_by(|a, b| java_cmp(&a.0, &b.0));
            o.push('{');
            for (i, (k, val)) in e.into_iter().enumerate() {
                if i != 0 {
                    o.push(',');
                }
                let plain = !k.eq_ignore_ascii_case("true")
                    && !k.eq_ignore_ascii_case("false")
                    && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '.' || c == '_')
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
                if plain {
                    o.push_str(k);
                } else {
                    quote(k, o);
                }
                o.push(':');
                compact(val, o);
            }
            o.push('}');
        }
    }
}

/// Comparison order of Java strings (UTF-16 units).
fn java_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// String hash the Java way.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}

/// Iteration order of a Java hash table filled in the order `keys`: ascending buckets,
/// insertion order within a bucket (capacity doubled beyond 3/4).
fn hash_order(keys: &[&str]) -> Vec<usize> {
    let mut cap = 16usize;
    while keys.len() > cap * 3 / 4 {
        cap *= 2;
    }
    let bucket = |k: &str| {
        let h = java_hash(k) as u32;
        ((h ^ (h >> 16)) as usize) & (cap - 1)
    };
    let mut idx: Vec<usize> = (0..keys.len()).collect();
    idx.sort_by_key(|&i| (bucket(keys[i]), i));
    idx
}

/// "Pretty" SNBT of `/summon`: `UUID` and `Pos` removed, keys in the order of the game's table.
pub fn snbt_pretty_entity(n: &Nbt) -> String {
    let mut o = String::new();
    pretty(n, &mut o, 0, &["UUID", "Pos"]);
    o
}

fn pretty(n: &Nbt, o: &mut String, depth: u32, removed: &[&str]) {
    let arr = |o: &mut String, items: Vec<String>, prefix: &str| {
        o.push('[');
        o.push_str(prefix);
        o.push(';');
        let len = items.len();
        for (i, s) in items.into_iter().take(128).enumerate() {
            o.push(' ');
            o.push_str(&s);
            if i != len - 1 {
                o.push(',');
            }
        }
        if len > 128 {
            o.push_str("<...>");
        }
        o.push(']');
    };
    match n {
        Nbt::ByteArray(v) => arr(o, v.iter().map(|&b| format!("{}b", b as i8)).collect(), "B"),
        Nbt::IntArray(v) => arr(o, v.iter().map(i32::to_string).collect(), "I"),
        Nbt::LongArray(v) => arr(o, v.iter().map(|x| format!("{x}L")).collect(), "L"),
        Nbt::List(v) => {
            if v.is_empty() {
                o.push_str("[]");
            } else if depth >= 64 {
                o.push_str("[<...>]");
            } else {
                let wrap = v.len() < 8
                    && v.iter().any(|e| {
                        !matches!(
                            e,
                            Nbt::Byte(_) | Nbt::Short(_) | Nbt::Int(_) | Nbt::Long(_) | Nbt::Float(_) | Nbt::Double(_)
                        )
                    });
                o.push('[');
                let limit = if wrap { 128 } else { usize::MAX };
                for (i, e) in v.iter().take(limit).enumerate() {
                    if i != 0 {
                        o.push_str(", ");
                    }
                    pretty(e, o, depth + 1, &[]);
                }
                if wrap && v.len() > 128 {
                    o.push_str(", <...>");
                }
                o.push(']');
            }
        }
        Nbt::Compound(v) => {
            let keys: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
            let order: Vec<usize> = hash_order(&keys).into_iter().filter(|&i| !removed.contains(&keys[i])).collect();
            if order.is_empty() {
                o.push_str("{}");
            } else if depth >= 64 {
                o.push_str("{<...>}");
            } else {
                o.push('{');
                for (j, &i) in order.iter().enumerate() {
                    let k = keys[i];
                    if !k.is_empty()
                        && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
                    {
                        o.push_str(k);
                    } else {
                        quote(k, o);
                    }
                    o.push_str(": ");
                    pretty(&v[i].1, o, depth + 1, &[]);
                    if j + 1 != order.len() {
                        o.push_str(", ");
                    }
                }
                o.push('}');
            }
        }
        other => compact(other, o),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_numbers() {
        assert_eq!(java_float(1.0), "1.0");
        assert_eq!(java_float(0.1), "0.1");
        assert_eq!(java_float(1e7), "1.0E7");
        assert_eq!(java_float(1.5e-4), "1.5E-4");
        assert_eq!(java_double(-0.0), "-0.0");
        assert_eq!(java_double(123456.75), "123456.75");
        assert_eq!(java_fixed2(0.125), "0.13");
        assert_eq!(java_fixed2(-59.0), "-59.00");
        assert_eq!(java_fixed2(1899.995), "1900.00");
        assert_eq!(java_fixed2(0.004), "0.00");
    }

    #[test]
    fn compact_sorted_and_quoted() {
        let n = Nbt::Compound(vec![
            ("b".into(), Nbt::String("it's".into())),
            ("A".into(), Nbt::Byte(1)),
            ("x y".into(), Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])),
        ]);
        assert_eq!(snbt_compact(&n), "{A:1b,b:\"it's\",\"x y\":[1,2]}");
    }

    #[test]
    fn structure_box_rotated() {
        let mut s = Structure::new(0);
        s.size = [3, 2, 5];
        assert_eq!(s.renderable_box(), ([0, 1, 0], [3, 2, 5]));
        s.rotation = 1;
        assert_eq!(s.renderable_box(), ([-4, 1, 0], [5, 2, 3]));
    }

    #[test]
    fn hash_order_buckets() {
        // "Pos" (hash 80,247) falls before "Air" (66,095) in a table of 16.
        let keys = ["Air", "Pos"];
        let o = hash_order(&keys);
        let b = |k: &str| {
            let h = java_hash(k) as u32;
            (h ^ (h >> 16)) & 15
        };
        assert_eq!(o[0], if b("Air") <= b("Pos") { 0 } else { 1 });
    }

    fn instance_nbt(status: &str, error: bool, test: &str) -> Nbt {
        let mut data = vec![
            ("test".into(), Nbt::String(test.into())),
            ("size".into(), Nbt::IntArray(vec![3, 2, 5])),
            ("rotation".into(), Nbt::String("none".into())),
            ("ignore_entities".into(), Nbt::Byte(0)),
            ("status".into(), Nbt::String(status.into())),
        ];
        if error {
            data.push(("error_message".into(), Nbt::String("Boom".into())));
        }
        let errors = Nbt::List(vec![Nbt::Compound(vec![
            ("pos".into(), Nbt::IntArray(vec![10, -60, 7])),
            ("text".into(), Nbt::String("Expected stone".into())),
        ])]);
        Nbt::Compound(vec![("data".into(), Nbt::Compound(data)), ("errors".into(), errors)])
    }

    #[test]
    fn test_instance_reads_status_and_error_markers() {
        let t = TestInstance::from_nbt(&instance_nbt("running", false, "minecraft:always_pass"));
        assert_eq!(t.status, 1);
        assert_eq!(t.errors.len(), 1);
        assert_eq!(t.errors[0].0, [10, -60, 7]);
        // Markers alone, without a data block.
        let only = Nbt::Compound(vec![("errors".into(), Nbt::List(Vec::new()))]);
        assert_eq!(TestInstance::from_nbt(&only), TestInstance::default());
    }

    #[test]
    fn beam_color_follows_status_and_required_flag() {
        let defs = TestDefs::from_entries(&[
            ("minecraft:always_pass".into(), None),
            ("pack:strict".into(), Some(Nbt::Compound(vec![("padding".into(), Nbt::Int(2))]))),
            ("pack:loose".into(), Some(Nbt::Compound(vec![("required".into(), Nbt::Byte(0))]))),
        ]);
        let beam = |s: &str, e: bool, t: &str| TestInstance::from_nbt(&instance_nbt(s, e, t)).beam(&defs);
        assert_eq!(beam("cleared", true, "pack:strict"), None);
        assert_eq!(beam("running", false, "pack:strict"), Some([128, 128, 128]));
        assert_eq!(beam("finished", false, "pack:strict"), Some([0, 255, 0]));
        assert_eq!(beam("finished", true, "pack:strict"), Some([255, 0, 0]));
        assert_eq!(beam("finished", true, "pack:loose"), Some([255, 128, 0]));
        // Base pack test (content omitted): optional; unknown test: required.
        assert_eq!(beam("finished", true, "minecraft:always_pass"), Some([255, 128, 0]));
        assert_eq!(beam("finished", true, "pack:missing"), Some([255, 0, 0]));
        assert_eq!(defs.get(Some("pack:strict")), TestDef { required: true, padding: 2, rotation: 0 });
    }

    #[test]
    fn test_instance_box_offset_by_padding() {
        let mut t = TestInstance::from_nbt(&instance_nbt("cleared", false, "x:y"));
        let d = |padding, rotation| TestDef { required: true, padding, rotation };
        assert_eq!(t.renderable_box(d(0, 0)), ([0, 1, 1], [3, 2, 5]));
        assert_eq!(t.renderable_box(d(2, 0)), ([2, 3, 3], [3, 2, 5]));
        t.rotation = 3;
        assert_eq!(t.renderable_box(d(1, 0)), ([1, 2, 2], [5, 2, 3]));
        // Test rotation composed with the block's: 3 + 1 = 0, no swap.
        assert_eq!(t.renderable_box(d(1, 1)), ([1, 2, 2], [3, 2, 5]));
        assert_eq!(t.combined_rotation(d(0, 1)), 0);
        t.rotation = 0;
        assert_eq!(t.renderable_box(d(0, 1)), ([0, 1, 1], [5, 2, 3]));
        let defs = TestDefs::from_entries(&[(
            "pack:r".into(),
            Some(Nbt::Compound(vec![("rotation".into(), Nbt::String("counterclockwise_90".into()))])),
        )]);
        assert_eq!(defs.get(Some("pack:r")).rotation, 3);
    }

    #[test]
    fn instance_box_stays_visible_while_its_beam_is() {
        let inst = Tech::TestInstance(TestInstance::default());
        let st = Tech::Structure(Structure::new(0));
        let eye = [200.5, 0.5, 0.5];
        // 200 flat blocks: outside the 96 of the structure block, inside the beam at 16 chunks.
        assert!(in_range(&inst, [0, 0, 0], eye, 16));
        assert!(!in_range(&st, [0, 0, 0], eye, 16));
        assert!(!in_range(&inst, [0, 0, 0], eye, 12));
        // Height ignored for the beam, counted for the box.
        assert!(in_range(&inst, [0, 0, 0], [0.5, 150.0, 0.5], 2));
        assert!(!in_range(&st, [0, 0, 0], [0.5, 150.0, 0.5], 2));
    }

    #[test]
    fn highlights_last_ten_seconds_and_clear_with_the_world() {
        let mut t = TechBlocks::default();
        let now = Instant::now();
        t.highlight([5, -60, 9], [-1, 0, 2], now);
        assert_eq!(t.highlights[&[5, -60, 9]].0, "-1, 0, 2");
        t.expire_highlights(now + Duration::from_secs(10));
        assert_eq!(t.highlights.len(), 1);
        t.expire_highlights(now + Duration::from_millis(10_001));
        assert!(t.highlights.is_empty());
        t.highlight([5, -60, 9], [0, 0, 0], now);
        t.clear();
        assert!(t.highlights.is_empty());
    }
}
