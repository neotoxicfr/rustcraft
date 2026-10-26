//! Structure templates (`generation-decoration.md` § 10.1–10.2): `structure/*.nbt` files of the
//! jar, palettes sorted like the game, junction blocks, transformations (mirror, rotation) of
//! positions, boxes and states.

use hashbrown::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rc_proto::nbt::Nbt;

use crate::Error;
use crate::decor_blocks::{Blocks, Dir, State};

/// Rotation of a template (game order: none, 90° clockwise, 180°, 90° counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Rot {
    #[default]
    None = 0,
    Cw90 = 1,
    Cw180 = 2,
    Ccw90 = 3,
}

impl Rot {
    pub const ALL: [Rot; 4] = [Rot::None, Rot::Cw90, Rot::Cw180, Rot::Ccw90];

    pub fn from_index(i: i32) -> Rot {
        Rot::ALL[i.rem_euclid(4) as usize]
    }

    pub fn name(self) -> &'static str {
        ["NONE", "CLOCKWISE_90", "CLOCKWISE_180", "COUNTERCLOCKWISE_90"][self as usize]
    }

    /// Composition (rotations added).
    pub fn then(self, o: Rot) -> Rot {
        Rot::from_index(self as i32 + o as i32)
    }

    /// Rotated horizontal direction (up and down unchanged).
    pub fn dir(self, d: Dir) -> Dir {
        if !d.is_horizontal() {
            return d;
        }
        let mut d = d;
        for _ in 0..self as u8 {
            d = d.cw();
        }
        d
    }
}

/// Mirror: `LeftRight` = z ↦ −z (north ↔ south), `FrontBack` = x ↦ −x (east ↔ west).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mirror {
    #[default]
    None,
    LeftRight,
    FrontBack,
}

impl Mirror {
    pub fn dir(self, d: Dir) -> Dir {
        match (self, d) {
            (Mirror::LeftRight, Dir::North | Dir::South) => d.opposite(),
            (Mirror::FrontBack, Dir::East | Dir::West) => d.opposite(),
            _ => d,
        }
    }
}

/// Inclusive integer box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl BBox {
    pub fn new(a: [i32; 3], b: [i32; 3]) -> BBox {
        BBox {
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }

    pub fn intersects(&self, o: &BBox) -> bool {
        (0..3).all(|i| self.max[i] >= o.min[i] && self.min[i] <= o.max[i])
    }

    /// Intersects the columns [x0, x1] × [z0, z1].
    pub fn intersects_xz(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> bool {
        self.max[0] >= x0 && self.min[0] <= x1 && self.max[2] >= z0 && self.min[2] <= z1
    }

    pub fn contains(&self, p: [i32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    pub fn encapsulate(&mut self, o: &BBox) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(o.min[i]);
            self.max[i] = self.max[i].max(o.max[i]);
        }
    }

    pub fn moved(&self, d: [i32; 3]) -> BBox {
        BBox {
            min: [self.min[0] + d[0], self.min[1] + d[1], self.min[2] + d[2]],
            max: [self.max[0] + d[0], self.max[1] + d[1], self.max[2] + d[2]],
        }
    }

    pub fn inflated(&self, n: i32) -> BBox {
        BBox {
            min: [self.min[0] - n, self.min[1] - n, self.min[2] - n],
            max: [self.max[0] + n, self.max[1] + n, self.max[2] + n],
        }
    }

    pub fn span(&self, i: usize) -> i32 {
        self.max[i] - self.min[i] + 1
    }

    /// "Object" centre (§ 9.1): `min + (max − min + 1) / 2`.
    pub fn center(&self) -> [i32; 3] {
        [
            self.min[0] + (self.max[0] - self.min[0] + 1) / 2,
            self.min[1] + (self.max[1] - self.min[1] + 1) / 2,
            self.min[2] + (self.max[2] - self.min[2] + 1) / 2,
        ]
    }
}

/// Block of a template.
#[derive(Debug, Clone)]
pub struct TBlock {
    pub pos: [i32; 3],
    pub state: State,
    pub nbt: Option<Nbt>,
}

/// Junction block (data read from its NBT).
#[derive(Debug, Clone)]
pub struct Jigsaw {
    pub pos: [i32; 3],
    pub state: State,
    pub name: Arc<str>,
    pub pool: Arc<str>,
    pub target: Arc<str>,
    /// Front and top faces (in the template, before transformation).
    pub front: Dir,
    pub top: Dir,
    pub rollable: bool,
    pub placement_priority: i32,
    pub selection_priority: i32,
    pub final_state: String,
}

/// Entity of a template.
#[derive(Debug, Clone)]
pub struct TEntity {
    pub pos: [f64; 3],
    pub block_pos: [i32; 3],
    pub nbt: Nbt,
}

#[derive(Debug)]
pub struct Template {
    pub size: [i32; 3],
    /// Palettes: blocks in placement order (full, others, with data).
    pub palettes: Vec<Vec<TBlock>>,
    /// Ends of the first two categories of each palette (full, then others); each
    /// category is sorted by (y, x, z).
    pub cats: Vec<[usize; 2]>,
    /// Junction blocks of each palette (same order).
    pub jigsaws: Vec<Vec<Jigsaw>>,
    pub entities: Vec<TEntity>,
}

fn parse_dir(s: &str) -> Option<Dir> {
    Dir::from_name(s)
}

impl Template {
    pub fn parse(root: &Nbt, b: &Blocks) -> Result<Template, Error> {
        let ints = |v: Option<&Nbt>| -> Option<[i32; 3]> {
            let Some(Nbt::List(l)) = v else { return None };
            if l.len() != 3 {
                return None;
            }
            let g = |n: &Nbt| match n {
                Nbt::Int(i) => Some(*i),
                _ => None,
            };
            Some([g(&l[0])?, g(&l[1])?, g(&l[2])?])
        };
        let size = ints(root.get("size")).ok_or_else(|| Error::Format("template without size".into()))?;
        // Palette entry: bare string, or `{id | Name, properties | Properties}` (26.3 keys).
        let state_of = |e: &Nbt| -> State {
            let e = match e {
                Nbt::Compound(f) if f.len() == 1 && f[0].0.is_empty() => &f[0].1,
                e => e,
            };
            let name = match e {
                Nbt::String(s) => s.as_str(),
                _ => e.get("id").or_else(|| e.get("Name")).and_then(Nbt::as_str).unwrap_or("minecraft:air"),
            };
            let mut text = name.to_owned();
            if let Some(Nbt::Compound(props)) = e.get("properties").or_else(|| e.get("Properties")) {
                text.push('[');
                for (i, (k, v)) in props.iter().enumerate() {
                    if i > 0 {
                        text.push(',');
                    }
                    text.push_str(k);
                    text.push('=');
                    text.push_str(v.as_str().unwrap_or(""));
                }
                text.push(']');
            }
            b.state_from_str(&text).unwrap_or_else(|| b.state_from_str(name).unwrap_or(0))
        };
        let palettes_nbt: Vec<Vec<State>> = match (root.get("palettes"), root.get("palette")) {
            (Some(Nbt::List(ps)), _) => ps
                .iter()
                .map(|p| match p {
                    Nbt::List(l) => l.iter().map(state_of).collect(),
                    _ => Vec::new(),
                })
                .collect(),
            (_, Some(Nbt::List(l))) => vec![l.iter().map(state_of).collect()],
            _ => vec![Vec::new()],
        };
        let raw: Vec<([i32; 3], usize, Option<Nbt>)> = match root.get("blocks") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|e| {
                    let pos = ints(e.get("pos"))?;
                    let st = match e.get("state")? {
                        Nbt::Int(i) => *i as usize,
                        _ => return None,
                    };
                    Some((pos, st, e.get("nbt").cloned()))
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut palettes = Vec::new();
        let mut cats = Vec::new();
        let mut jigsaws = Vec::new();
        let jigsaw_block = b.block_id("minecraft:jigsaw");
        let orientation = b.prop("orientation");
        for pal in &palettes_nbt {
            let (mut full, mut other, mut with) = (Vec::new(), Vec::new(), Vec::new());
            for (pos, si, nbt) in &raw {
                let state = pal.get(*si).copied().unwrap_or(0);
                let t = TBlock { pos: *pos, state, nbt: nbt.clone() };
                if t.nbt.is_some() {
                    with.push(t);
                } else if b.has_flag(state, rc_data::flags::FULL_COLLISION) {
                    full.push(t);
                } else {
                    other.push(t);
                }
            }
            let key = |t: &TBlock| (t.pos[1], t.pos[0], t.pos[2]);
            full.sort_by_key(key);
            other.sort_by_key(key);
            with.sort_by_key(key);
            cats.push([full.len(), full.len() + other.len()]);
            let mut list = full;
            list.extend(other);
            list.extend(with);
            let js: Vec<Jigsaw> = list
                .iter()
                .filter(|t| Some(b.block(t.state)) == jigsaw_block)
                .map(|t| {
                    let n = t.nbt.as_ref();
                    let s = |k: &str, d: &str| n.and_then(|n| n.get(k)).and_then(Nbt::as_str).unwrap_or(d).to_owned();
                    let a = |k: &str, d: &str| Arc::<str>::from(n.and_then(|n| n.get(k)).and_then(Nbt::as_str).unwrap_or(d));
                    let i = |k: &str| n.and_then(|n| n.get(k)).and_then(Nbt::as_i64).unwrap_or(0) as i32;
                    let o = orientation.and_then(|p| b.get_str(t.state, p)).unwrap_or("north_up");
                    let (f, u) = o.split_once('_').unwrap_or(("north", "up"));
                    let front = parse_dir(f).unwrap_or(Dir::North);
                    let top = parse_dir(u).unwrap_or(Dir::Up);
                    let joint = n.and_then(|n| n.get("joint")).and_then(Nbt::as_str);
                    let rollable = match joint {
                        Some("rollable") => true,
                        Some("aligned") => false,
                        _ => !front.is_horizontal(),
                    };
                    Jigsaw {
                        pos: t.pos,
                        state: t.state,
                        name: a("name", "minecraft:empty"),
                        pool: a("pool", "minecraft:empty"),
                        target: a("target", "minecraft:empty"),
                        front,
                        top,
                        rollable,
                        placement_priority: i("placement_priority"),
                        selection_priority: i("selection_priority"),
                        final_state: s("final_state", "minecraft:air"),
                    }
                })
                .collect();
            palettes.push(list);
            jigsaws.push(js);
        }
        let entities = match root.get("entities") {
            Some(Nbt::List(l)) => l
                .iter()
                .filter_map(|e| {
                    let Some(Nbt::List(p)) = e.get("pos") else { return None };
                    let d = |n: &Nbt| match n {
                        Nbt::Double(v) => *v,
                        _ => 0.0,
                    };
                    Some(TEntity {
                        pos: [d(p.first()?), d(p.get(1)?), d(p.get(2)?)],
                        block_pos: ints(e.get("blockPos"))?,
                        nbt: e.get("nbt").cloned().unwrap_or(Nbt::Compound(Vec::new())),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(Template { size, palettes, cats, jigsaws, entities })
    }

    /// Size after rotation.
    pub fn size_rot(&self, r: Rot) -> [i32; 3] {
        match r {
            Rot::Cw90 | Rot::Ccw90 => [self.size[2], self.size[1], self.size[0]],
            _ => self.size,
        }
    }

    /// Box of the template placed at `pos` with these settings.
    pub fn bbox(&self, pos: [i32; 3], s: &Settings) -> BBox {
        let a = transform([0, 0, 0], s.mirror, s.rot, s.pivot);
        let b = transform([self.size[0] - 1, self.size[1] - 1, self.size[2] - 1], s.mirror, s.rot, s.pivot);
        BBox::new(a, b).moved(pos)
    }
}

/// Placement settings (subset useful to geometry).
#[derive(Debug, Clone, Copy, Default)]
pub struct Settings {
    pub mirror: Mirror,
    pub rot: Rot,
    pub pivot: [i32; 3],
}

impl Settings {
    pub fn rot(rot: Rot) -> Settings {
        Settings { rot, ..Default::default() }
    }
}

/// Position of a template block after mirror then rotation around the pivot (x, z).
pub fn transform(p: [i32; 3], m: Mirror, r: Rot, pivot: [i32; 3]) -> [i32; 3] {
    let (mut x, y, mut z) = (p[0], p[1], p[2]);
    match m {
        Mirror::LeftRight => z = -z,
        Mirror::FrontBack => x = -x,
        Mirror::None => {}
    }
    let (px, pz) = (pivot[0], pivot[2]);
    match r {
        Rot::None => [x, y, z],
        Rot::Cw90 => [px + pz - z, y, pz - px + x],
        Rot::Cw180 => [px + px - x, y, pz + pz - z],
        Rot::Ccw90 => [px - pz + z, y, px + pz - x],
    }
}

/// Position of a template entity (real coordinates).
pub fn transform_f(p: [f64; 3], m: Mirror, r: Rot, pivot: [i32; 3]) -> [f64; 3] {
    let (mut x, y, mut z) = (p[0], p[1], p[2]);
    match m {
        Mirror::LeftRight => z = 1.0 - z,
        Mirror::FrontBack => x = 1.0 - x,
        Mirror::None => {}
    }
    let (px, pz) = (f64::from(pivot[0]), f64::from(pivot[2]));
    match r {
        Rot::None => [x, y, z],
        Rot::Cw90 => [px + pz + 1.0 - z, y, pz - px + x],
        Rot::Cw180 => [px + px + 1.0 - x, y, pz + pz + 1.0 - z],
        Rot::Ccw90 => [px - pz + z, y, px + pz + 1.0 - x],
    }
}

/// Origin ("zero") corner of a rotated and mirrored template for the same footprint (§ 10.2).
pub fn zero_position(pos: [i32; 3], size: [i32; 3], m: Mirror, r: Rot) -> [i32; 3] {
    let (a, b) = (size[0] - 1, size[2] - 1);
    let mx = if m == Mirror::FrontBack { a } else { 0 };
    let mz = if m == Mirror::LeftRight { b } else { 0 };
    let d = match r {
        Rot::Ccw90 => [mz, 0, a - mx],
        Rot::Cw90 => [b - mz, 0, mx],
        Rot::Cw180 => [a - mx, 0, b - mz],
        Rot::None => [mx, 0, mz],
    };
    [pos[0] + d[0], pos[1], pos[2] + d[2]]
}

/// Position seed (`position_seed`, § 10.2).
pub fn pos_seed(x: i32, y: i32, z: i32) -> i64 {
    crate::rng::position_seed(x, y, z)
}

/// State transformation tables (mirror then rotation), built once.
pub struct StateTransforms {
    /// `rot[r][state]` for r = 1..3 (index r − 1).
    rot: [Vec<State>; 3],
    /// `mirror[m][state]`: 0 left-right, 1 front-back.
    mirror: [Vec<State>; 2],
    /// Template processor tables, resolved with the same blocks.
    pub procs: crate::structure_procs::ProcTables,
}

/// Properties that rotation or mirror can change (north/east/south/west faces included):
/// a block that has none keeps all its states.
const ORIENTED_PROPS: [&str; 11] =
    ["north", "east", "south", "west", "facing", "axis", "rotation", "shape", "orientation", "hinge", "type"];

/// Name of a transformed rail shape (directions of the name transformed, name normalised).
fn rail_shape(v: &str, f: &dyn Fn(Dir) -> Dir) -> Option<String> {
    const SHAPES: [&str; 10] = [
        "north_south",
        "east_west",
        "ascending_east",
        "ascending_west",
        "ascending_north",
        "ascending_south",
        "south_east",
        "south_west",
        "north_west",
        "north_east",
    ];
    if let Some(d) = v.strip_prefix("ascending_") {
        return Some(format!("ascending_{}", f(Dir::from_name(d)?).name()));
    }
    let (a, c) = v.split_once('_')?;
    let (a, c) = (f(Dir::from_name(a)?), f(Dir::from_name(c)?));
    SHAPES
        .iter()
        .find(|s| {
            !s.starts_with("ascending")
                && s.split_once('_')
                    .is_some_and(|(x, y)| (x == a.name() && y == c.name()) || (x == c.name() && y == a.name()))
        })
        .map(|s| (*s).to_owned())
}

impl StateTransforms {
    pub fn new(b: &Blocks) -> StateTransforms {
        Self::new_mode(b, crate::opt::statetrans_fast())
    }

    /// `fast`: blocks without an oriented property copied as is (`RUSTCRAFT_OPT_STATETRANS_FAST`).
    pub(crate) fn new_mode(b: &Blocks, fast: bool) -> StateTransforms {
        let n = b.state_count();
        let fixed: Vec<bool> = if fast {
            (0..b.block_count())
                .map(|bl| {
                    b.props_of(b.default_state(bl as crate::decor_blocks::BlockId))
                        .iter()
                        .all(|(_, name, _)| !ORIENTED_PROPS.contains(name))
                })
                .collect()
        } else {
            Vec::new()
        };
        let mk = |f: &dyn Fn(State) -> State| {
            (0..n as State)
                .map(|s| if fast && fixed[b.block(s) as usize] { s } else { f(s) })
                .collect::<Vec<_>>()
        };
        let rot = [
            mk(&|s| Self::rotate_one(b, s, Rot::Cw90)),
            mk(&|s| Self::rotate_one(b, s, Rot::Cw180)),
            mk(&|s| Self::rotate_one(b, s, Rot::Ccw90)),
        ];
        let mirror =
            [mk(&|s| Self::mirror_one(b, s, Mirror::LeftRight)), mk(&|s| Self::mirror_one(b, s, Mirror::FrontBack))];
        StateTransforms { rot, mirror, procs: crate::structure_procs::ProcTables::new(b) }
    }

    /// Fingerprint of the tables (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for t in self.rot.iter().chain(&self.mirror) {
            for s in t {
                for x in s.to_le_bytes() {
                    h ^= u64::from(x);
                    h = h.wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
        h
    }

    /// `state.mirror(m).rotate(r)`.
    pub fn apply(&self, s: State, m: Mirror, r: Rot) -> State {
        let s = match m {
            Mirror::None => s,
            Mirror::LeftRight => self.mirror[0][s as usize],
            Mirror::FrontBack => self.mirror[1][s as usize],
        };
        match r {
            Rot::None => s,
            r => self.rot[r as usize - 1][s as usize],
        }
    }

    pub fn rotate(&self, s: State, r: Rot) -> State {
        self.apply(s, Mirror::None, r)
    }

    fn rewrite(b: &Blocks, s: State, f: &dyn Fn(&str, &str) -> Option<String>) -> State {
        let mut out = s;
        let props: Vec<(crate::decor_blocks::Prop, String, String)> =
            b.props_of(s).into_iter().map(|(p, n, v)| (p, n.to_owned(), v.to_owned())).collect();
        // North/east/south/west properties: values permuted together.
        let mut side: HashMap<Dir, String> = HashMap::new();
        for (_, n, v) in &props {
            if let Some(d) = Dir::from_name(n).filter(|d| d.is_horizontal()) {
                side.insert(d, v.clone());
            }
        }
        for (p, n, v) in &props {
            if let Some(d) = Dir::from_name(n).filter(|d| d.is_horizontal()) {
                // New value for face d = old value of the face that arrives there.
                let src = Dir::NESW.iter().copied().find(|o| f("__dir", o.name()).as_deref() == Some(d.name()));
                if let Some(src) = src
                    && let Some(val) = side.get(&src)
                    && let Some(vi) = b.value_index(out, *p, val)
                {
                    out = b.with(out, *p, vi);
                }
                let _ = v;
                continue;
            }
            if let Some(nv) = f(n, v)
                && let Some(vi) = b.value_index(out, *p, &nv)
            {
                out = b.with(out, *p, vi);
            }
        }
        out
    }

    fn rotate_one(b: &Blocks, s: State, r: Rot) -> State {
        let name = b.block_name(b.block(s)).to_owned();
        let turn = |d: Dir| r.dir(d);
        Self::rewrite(b, s, &|prop, v| match prop {
            "__dir" => Some(turn(Dir::from_name(v)?).name().to_owned()),
            "facing" => Some(turn(Dir::from_name(v)?).name().to_owned()),
            "axis" if matches!(r, Rot::Cw90 | Rot::Ccw90) => match v {
                "x" => Some("z".into()),
                "z" => Some("x".into()),
                _ => None,
            },
            "rotation" => Some(((v.parse::<i32>().ok()? + 4 * r as i32) % 16).to_string()),
            "shape" if name.ends_with("rail") => rail_shape(v, &turn),
            "orientation" => {
                let (a, c) = v.split_once('_')?;
                Some(format!("{}_{}", turn(Dir::from_name(a)?).name(), turn(Dir::from_name(c)?).name()))
            }
            _ => None,
        })
    }

    fn mirror_one(b: &Blocks, s: State, m: Mirror) -> State {
        let name = b.block_name(b.block(s)).to_owned();
        let flip = |d: Dir| m.dir(d);
        // Stairs: the game's own rules (including front-back which keeps "inner right").
        if name.ends_with("_stairs")
            && let (Some(fp), Some(sp)) = (b.prop("facing"), b.prop("shape"))
            && let (Some(facing), Some(shape)) = (b.get_str(s, fp).and_then(Dir::from_name), b.get_str(s, sp))
        {
            let z_axis = matches!(facing, Dir::North | Dir::South);
            let applies = match m {
                Mirror::LeftRight => z_axis,
                Mirror::FrontBack => !z_axis,
                Mirror::None => false,
            };
            if !applies {
                return s;
            }
            let turned = Self::rotate_one(b, s, Rot::Cw180);
            let new_shape = match (m, shape) {
                (_, "outer_left") => "outer_right",
                (_, "outer_right") => "outer_left",
                (Mirror::LeftRight, "inner_right") => "inner_left",
                (Mirror::LeftRight, "inner_left") => "inner_right",
                (_, other) => other,
            };
            return b.with_str(turned, sp, new_shape);
        }
        let door = name.ends_with("_door");
        let chest = name.ends_with("chest");
        let out = Self::rewrite(b, s, &|prop, v| match prop {
            "__dir" => Some(flip(Dir::from_name(v)?).name().to_owned()),
            "facing" => Some(flip(Dir::from_name(v)?).name().to_owned()),
            "rotation" => {
                let r = v.parse::<i32>().ok()?;
                Some(
                    match m {
                        Mirror::LeftRight => (16 - r).rem_euclid(16),
                        Mirror::FrontBack => (8 - r).rem_euclid(16),
                        Mirror::None => r,
                    }
                    .to_string(),
                )
            }
            "shape" if name.ends_with("rail") => rail_shape(v, &flip),
            "orientation" => {
                let (a, c) = v.split_once('_')?;
                Some(format!("{}_{}", flip(Dir::from_name(a)?).name(), flip(Dir::from_name(c)?).name()))
            }
            _ => None,
        });
        let mut out = out;
        if door
            && m != Mirror::None
            && let Some(h) = b.prop("hinge")
            && let Some(v) = b.get_str(out, h)
        {
            let nv = if v == "left" { "right" } else { "left" };
            out = b.with_str(out, h, nv);
        }
        if chest
            && m != Mirror::None
            && let Some(t) = b.prop("type")
            && let Some(v) = b.get_str(out, t)
        {
            let nv = match v {
                "left" => "right",
                "right" => "left",
                o => o,
            };
            out = b.with_str(out, t, nv);
        }
        out
    }
}

type TemplCell = OnceLock<Option<Arc<Template>>>;

/// Templates of the jar, read on demand (gzip NBT) and kept.
pub struct Templates {
    /// Table frozen at construction: bytes of the file and template decoded at first need.
    files: HashMap<String, (Arc<[u8]>, TemplCell)>,
    /// Old path (`RUSTCRAFT_OPT_TEMPL_TABLE=0`): cache under lock, absent identifiers included.
    cache: Mutex<HashMap<String, Option<Arc<Template>>>>,
    /// Decodings performed (proofs: one per distinct template on the new path).
    decodes: AtomicUsize,
    pub transforms: StateTransforms,
}

impl std::fmt::Debug for Templates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Templates({} files)", self.files.len())
    }
}

impl Templates {
    /// `files`: `ns:path` → gzip bytes of the `.nbt`.
    pub fn new(files: HashMap<String, Arc<[u8]>>, b: &Blocks) -> Templates {
        Templates {
            files: files.into_iter().map(|(k, v)| (k, (v, OnceLock::new()))).collect(),
            cache: Mutex::new(HashMap::new()),
            decodes: AtomicUsize::new(0),
            transforms: StateTransforms::new(b),
        }
    }

    pub fn get(&self, id: &str, b: &Blocks) -> Option<Arc<Template>> {
        if crate::opt::templ_table() {
            let (bytes, cell) = crate::data::with_full_id(id, |k| self.files.get(k))?;
            return cell.get_or_init(|| self.decode(bytes, b)).clone();
        }
        let id = crate::data::full_id(id);
        if let Some(t) = self.cache.lock().ok()?.get(&id) {
            return t.clone();
        }
        let t = self.files.get(&id).and_then(|(bytes, _)| self.decode(bytes, b));
        if let Ok(mut c) = self.cache.lock() {
            c.insert(id, t.clone());
        }
        t
    }

    fn decode(&self, bytes: &[u8], b: &Blocks) -> Option<Arc<Template>> {
        self.decodes.fetch_add(1, Ordering::Relaxed);
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(bytes).read_to_end(&mut raw).ok()?;
        let mut r = rc_proto::Reader::new(&raw);
        let (_, root) = rc_proto::nbt::read_named(&mut r).ok()??;
        Template::parse(&root, b).ok().map(Arc::new)
    }

    /// Number of decodings performed since construction.
    pub fn decode_count(&self) -> usize {
        self.decodes.load(Ordering::Relaxed)
    }

    /// Identifiers of all the known templates (benches and proofs).
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}
