//! Structure templates (`generation-decoration.md` § 10.1–10.2): `structure/*.nbt` files of the
//! jar, palettes sorted like the game, junction blocks, transformations (mirror, rotation) of
//! positions, boxes and states.

use hashbrown::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use rc_proto::nbt::Nbt;

use crate::Error;
use crate::decor_blocks::{BlockId, Blocks, Dir, Prop, State};
use crate::opt::TablesThread;

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

/// Angle brought back into [−180, 180) (float remainder, like the game).
pub fn wrap_degrees(a: f32) -> f32 {
    let mut w = a % 360.0;
    if w >= 180.0 {
        w -= 360.0;
    }
    if w < -180.0 {
        w += 360.0;
    }
    w
}

/// Yaw of a template entity once placed: the rotation and the mirror are both applied to the
/// wrapped angle, then the stored angle is removed once (game formula; front-back flips x so the
/// angle changes sign, left-right flips z so it becomes 180° minus the angle).
pub fn entity_yaw(yaw: f32, m: Mirror, r: Rot) -> f32 {
    let w = wrap_degrees(yaw);
    let rotated = match r {
        Rot::Cw180 => w + 180.0,
        Rot::Ccw90 => w + 270.0,
        Rot::Cw90 => w + 90.0,
        Rot::None => w,
    };
    let mirrored = match m {
        Mirror::FrontBack => -w,
        Mirror::LeftRight => 180.0 - w,
        Mirror::None => w,
    };
    rotated + mirrored - yaw
}

/// Entity compound of a template made ready for the world: position, look, attached block
/// and facing of the hanging entities (paintings, item frames) transformed.
pub fn place_entity_nbt(f: &mut Vec<(String, Nbt)>, wp: [f64; 3], bp: [i32; 3], m: Mirror, r: Rot) {
    f.retain(|(k, _)| k != "UUID" && k != "Pos");
    f.push(("Pos".into(), Nbt::List(wp.iter().map(|v| Nbt::Double(*v)).collect())));
    if let Some(i) = f.iter().position(|(k, _)| k == "block_pos") {
        f[i].1 = Nbt::IntArray(bp.to_vec());
    }
    let (yaw, pitch) = match f.iter().find(|(k, _)| k == "Rotation").map(|(_, v)| v) {
        Some(Nbt::List(l)) if l.len() == 2 => match (&l[0], &l[1]) {
            (Nbt::Float(a), Nbt::Float(b)) => (*a, *b),
            _ => (0.0, 0.0),
        },
        _ => (0.0, 0.0),
    };
    let id = f.iter().find(|(k, _)| k == "id").and_then(|(_, v)| v.as_str()).unwrap_or("");
    let id = id.strip_prefix("minecraft:").unwrap_or(id);
    // Hanging entities turn their facing (horizontal only): rotation, then the mirror reverses a
    // facing that lies on the flipped axis.
    let facing = match id {
        "painting" => Some(("facing", false)),
        "item_frame" | "glow_item_frame" => Some(("Facing", true)),
        _ => None,
    };
    if let Some((key, three_d)) = facing
        && let Some(i) = f.iter().position(|(k, _)| k == key)
        && let Some(v) = f[i].1.as_i64()
    {
        let d = if three_d { Dir::ALL.get(v as usize).copied() } else { Dir::H2D.get((v & 3) as usize).copied() };
        if let Some(d) = d.filter(|d| d.is_horizontal()) {
            let nd = m.dir(r.dir(d));
            let table: &[Dir] = if three_d { &Dir::ALL } else { &Dir::H2D };
            let out = table.iter().position(|&x| x == nd);
            if let Some(o) = out {
                f[i].1 = Nbt::Byte(o as i8);
            }
        }
    }
    f.retain(|(k, _)| k != "Rotation");
    f.push(("Rotation".into(), Nbt::List(vec![Nbt::Float(entity_yaw(yaw, m, r)), Nbt::Float(pitch)])));
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

/// How the five tables are built: one field per lead, so that the proofs compare every
/// combination with the old build (all false).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TableBuild {
    /// Blocks without an oriented property copied as is (`RUSTCRAFT_OPT_STATETRANS_FAST`).
    pub skip_unoriented: bool,
    /// Half and three-quarter turns composed from the quarter turn table, mirrored stairs read
    /// from the half-turn table (`RUSTCRAFT_OPT_COMPOSE_ROTATIONS`).
    pub compose_rotations: bool,
    /// Property rules worked out once per block, then applied to its states by value index
    /// (`RUSTCRAFT_OPT_TRANSFORMS_PER_BLOCK`).
    pub per_block: bool,
    /// Blocks and states split across the rayon pool (`RUSTCRAFT_OPT_PARALLEL_TRANSFORMS`).
    pub parallel: bool,
}

impl TableBuild {
    /// The build chosen by the switches.
    fn from_switches() -> TableBuild {
        TableBuild {
            skip_unoriented: crate::opt::statetrans_fast(),
            compose_rotations: crate::opt::compose_rotations(),
            per_block: crate::opt::transforms_per_block(),
            parallel: crate::opt::parallel_transforms(),
        }
    }
}

/// One of the five changes the tables hold.
#[derive(Debug, Clone, Copy)]
enum StateChange {
    Rotate(Rot),
    Mirror(Mirror),
}

impl StateChange {
    fn dir(self, d: Dir) -> Dir {
        match self {
            StateChange::Rotate(r) => r.dir(d),
            StateChange::Mirror(m) => m.dir(d),
        }
    }

    /// A mirror that flips an axis.
    fn flips(self) -> bool {
        matches!(self, StateChange::Mirror(Mirror::LeftRight | Mirror::FrontBack))
    }
}

/// What a block's name says about how its states change.
#[derive(Debug, Clone, Copy)]
struct BlockFamily {
    /// Rails: the directions written in the shape turn.
    rail: bool,
    /// Stairs: mirrored by the game's own rule ([`mirror_stairs`]).
    stairs: bool,
    /// Doors: a mirror swaps the hinge.
    door: bool,
    /// Chests: a mirror swaps the left and right halves.
    chest: bool,
}

impl BlockFamily {
    fn of(name: &str) -> BlockFamily {
        BlockFamily {
            rail: name.ends_with("rail"),
            stairs: name.ends_with("_stairs"),
            door: name.ends_with("_door"),
            chest: name.ends_with("chest"),
        }
    }
}

/// Side named by a property (north, east, south, west): the four values turn together.
fn side_of(prop: &str) -> Option<Dir> {
    Dir::from_name(prop).filter(|d| d.is_horizontal())
}

/// Side whose value arrives on side `d` (the one the change turns onto it).
fn side_source(change: StateChange, d: Dir) -> Option<Dir> {
    Dir::NESW.into_iter().find(|&o| change.dir(o) == d)
}

/// New text value of a property that is not a side (`None` = unchanged).
fn changed_value(change: StateChange, family: BlockFamily, prop: &str, value: &str) -> Option<String> {
    let turn = |name: &str| Dir::from_name(name).map(|d| change.dir(d).name());
    match (change, prop) {
        (_, "facing") => turn(value).map(str::to_owned),
        (StateChange::Rotate(Rot::Cw90 | Rot::Ccw90), "axis") => match value {
            "x" => Some("z".into()),
            "z" => Some("x".into()),
            _ => None,
        },
        (StateChange::Rotate(r), "rotation") => Some(((value.parse::<i32>().ok()? + 4 * r as i32) % 16).to_string()),
        (StateChange::Mirror(m), "rotation") => {
            let r = value.parse::<i32>().ok()?;
            let mirrored = match m {
                // Left-right flips the z axis: north (8) and south (0) swap, east and west stay;
                // front-back flips x: east (4) and west (12) swap.
                Mirror::LeftRight => (8 - r).rem_euclid(16),
                Mirror::FrontBack => (16 - r).rem_euclid(16),
                Mirror::None => r,
            };
            Some(mirrored.to_string())
        }
        (_, "shape") if family.rail => rail_shape(value, &|d| change.dir(d)),
        (_, "orientation") => {
            let (front, top) = value.split_once('_')?;
            Some(format!("{}_{}", turn(front)?, turn(top)?))
        }
        (_, "hinge") if family.door && change.flips() => Some(if value == "left" { "right" } else { "left" }.to_owned()),
        (_, "type") if family.chest && change.flips() => match value {
            "left" => Some("right".into()),
            "right" => Some("left".into()),
            _ => None,
        },
        _ => None,
    }
}

/// The stairs face along the axis the mirror flips.
fn faces_flipped_axis(m: Mirror, facing: Dir) -> bool {
    let z_axis = matches!(facing, Dir::North | Dir::South);
    match m {
        Mirror::LeftRight => z_axis,
        Mirror::FrontBack => !z_axis,
        Mirror::None => false,
    }
}

/// Shape of mirrored stairs: the outer shapes swap left and right; the inner ones only in a
/// left-right mirror (front-back keeps them, like the game).
fn mirrored_stairs_shape(m: Mirror, shape: &str) -> &str {
    match (m, shape) {
        (_, "outer_left") => "outer_right",
        (_, "outer_right") => "outer_left",
        (Mirror::LeftRight, "inner_right") => "inner_left",
        (Mirror::LeftRight, "inner_left") => "inner_right",
        (_, other) => other,
    }
}

/// The state turned half a turn, read from the finished table when there is one.
fn half_turned(b: &Blocks, s: State, half_turn: Option<&[State]>) -> State {
    match half_turn {
        Some(t) => t[s as usize],
        None => transform_state(b, s, StateChange::Rotate(Rot::Cw180), None),
    }
}

/// Stairs in a mirror, the game's own rule: only stairs facing along the flipped axis change;
/// they turn half a turn and take the mirrored shape. `None`: not shaped stairs (generic rewrite).
fn mirror_stairs(b: &Blocks, s: State, m: Mirror, half_turn: Option<&[State]>) -> Option<State> {
    let (facing_prop, shape_prop) = (b.prop("facing")?, b.prop("shape")?);
    let facing = b.get_str(s, facing_prop).and_then(Dir::from_name)?;
    let shape = b.get_str(s, shape_prop)?;
    if !faces_flipped_axis(m, facing) {
        return Some(s);
    }
    Some(b.with_str(half_turned(b, s, half_turn), shape_prop, mirrored_stairs_shape(m, shape)))
}

/// One state through one change, its properties rewritten as text (old path, per state).
fn transform_state(b: &Blocks, s: State, change: StateChange, half_turn: Option<&[State]>) -> State {
    let family = BlockFamily::of(b.block_name(b.block(s)));
    if let StateChange::Mirror(m) = change
        && family.stairs
        && let Some(out) = mirror_stairs(b, s, m, half_turn)
    {
        return out;
    }
    rewrite(b, s, change, family)
}

/// Properties of a state rewritten as text, all read from the original state: each side takes
/// the old value of the side that turns onto it, the others go through [`changed_value`].
fn rewrite(b: &Blocks, s: State, change: StateChange, family: BlockFamily) -> State {
    let props: Vec<(Prop, String, String)> =
        b.props_of(s).into_iter().map(|(p, n, v)| (p, n.to_owned(), v.to_owned())).collect();
    let sides: HashMap<Dir, String> = props.iter().filter_map(|(_, n, v)| Some((side_of(n)?, v.clone()))).collect();
    let mut out = s;
    for (p, name, value) in &props {
        let new = match side_of(name) {
            Some(d) => side_source(change, d).and_then(|from| sides.get(&from).cloned()),
            None => changed_value(change, family, name, value),
        };
        if let Some(new) = new
            && let Some(vi) = b.value_index(out, *p, &new)
        {
            out = b.with(out, *p, vi);
        }
    }
    out
}

/// Index of a text value in a property's list.
fn index_of(values: &[String], value: &str) -> Option<u8> {
    values.iter().position(|v| v == value).map(|i| i as u8)
}

/// How one property of a block changes: the new value index for each value index of `source`
/// (the property itself, or the side that turns onto it); `None` keeps the value.
#[derive(Debug)]
struct PropertyRule {
    prop: Prop,
    source: Prop,
    new_value: Vec<Option<u8>>,
}

/// Rule of one property of a block (`any`: one of its states), `None` when the change keeps
/// every value.
fn property_rule(
    b: &Blocks,
    any: State,
    props: &[(Prop, &str, &str)],
    change: StateChange,
    family: BlockFamily,
    prop: Prop,
    name: &str,
) -> Option<PropertyRule> {
    let values = b.values_of(any, prop)?;
    let (source, new_value): (Prop, Vec<Option<u8>>) = match side_of(name) {
        Some(d) => {
            let from = side_source(change, d)?;
            let &(source, _, _) = props.iter().find(|(_, n, _)| *n == from.name())?;
            (source, b.values_of(any, source)?.iter().map(|v| index_of(values, v)).collect())
        }
        None => {
            let changed = |v: &String| changed_value(change, family, name, v).and_then(|new| index_of(values, &new));
            (prop, values.iter().map(changed).collect())
        }
    };
    let keeps_all = source == prop && new_value.iter().enumerate().all(|(i, v)| v.is_none_or(|v| v as usize == i));
    (!keeps_all).then_some(PropertyRule { prop, source, new_value })
}

/// One change worked out once for one block, then applied to each of its states by value index.
#[derive(Debug)]
enum BlockPlan {
    /// No property changes: every state stays.
    Unchanged,
    /// Properties rewritten, all read from the original state.
    Rules(Vec<PropertyRule>),
    /// Stairs in a mirror ([`mirror_stairs`]): `flips` and `new_shape` by value index of the
    /// facing and of the shape.
    MirroredStairs { facing: Prop, flips: Vec<bool>, shape: Prop, new_shape: Vec<Option<u8>> },
}

impl BlockPlan {
    fn new(b: &Blocks, block: BlockId, change: StateChange) -> BlockPlan {
        let any = b.default_state(block);
        let family = BlockFamily::of(b.block_name(block));
        if let StateChange::Mirror(m) = change
            && family.stairs
            && let Some(plan) = Self::mirrored_stairs(b, any, m)
        {
            return plan;
        }
        let props = b.props_of(any);
        let rules: Vec<PropertyRule> =
            props.iter().filter_map(|&(p, name, _)| property_rule(b, any, &props, change, family, p, name)).collect();
        if rules.is_empty() { BlockPlan::Unchanged } else { BlockPlan::Rules(rules) }
    }

    /// `None`: not shaped stairs, or a facing value that is not a direction (generic rules).
    fn mirrored_stairs(b: &Blocks, any: State, m: Mirror) -> Option<BlockPlan> {
        let (facing, shape) = (b.prop("facing")?, b.prop("shape")?);
        let facings = b.values_of(any, facing)?;
        let flips = facings.iter().map(|v| Dir::from_name(v).map(|d| faces_flipped_axis(m, d))).collect::<Option<_>>()?;
        let shapes = b.values_of(any, shape)?;
        let new_shape = shapes.iter().map(|v| index_of(shapes, mirrored_stairs_shape(m, v))).collect();
        Some(BlockPlan::MirroredStairs { facing, flips, shape, new_shape })
    }

    fn apply(&self, b: &Blocks, s: State, half_turn: Option<&[State]>) -> State {
        match self {
            BlockPlan::Unchanged => s,
            BlockPlan::Rules(rules) => {
                let mut out = s;
                for rule in rules {
                    if let Some(old) = b.get(s, rule.source)
                        && let Some(v) = rule.new_value[old as usize]
                    {
                        out = b.with(out, rule.prop, v);
                    }
                }
                out
            }
            BlockPlan::MirroredStairs { facing, flips, shape, new_shape } => {
                if !b.get(s, *facing).is_some_and(|f| flips[f as usize]) {
                    return s;
                }
                let turned = half_turned(b, s, half_turn);
                match b.get(s, *shape).and_then(|v| new_shape[v as usize]) {
                    Some(v) => b.with(turned, *shape, v),
                    None => turned,
                }
            }
        }
    }
}

/// `f` over `0..n`, in order, across the rayon pool when `parallel`.
fn map_range<T: Send>(n: u32, parallel: bool, f: impl Fn(u32) -> T + Sync + Send) -> Vec<T> {
    use rayon::prelude::*;
    if parallel { (0..n).into_par_iter().map(f).collect() } else { (0..n).map(f).collect() }
}

/// `outer` after `inner`, state by state.
fn compose(outer: &[State], inner: &[State]) -> Vec<State> {
    inner.iter().map(|&s| outer[s as usize]).collect()
}

/// Builds the tables one change at a time.
struct TableBuilder<'a> {
    b: &'a Blocks,
    how: TableBuild,
    /// Per block: no oriented property (filled only on the `skip_unoriented` text path).
    unoriented: Vec<bool>,
}

impl<'a> TableBuilder<'a> {
    fn new(b: &'a Blocks, how: TableBuild) -> TableBuilder<'a> {
        let unoriented = if how.skip_unoriented && !how.per_block {
            map_range(b.block_count() as u32, how.parallel, |bl| {
                let any = b.default_state(bl as BlockId);
                b.props_of(any).iter().all(|(_, name, _)| !ORIENTED_PROPS.contains(name))
            })
        } else {
            Vec::new()
        };
        TableBuilder { b, how, unoriented }
    }

    /// The state each state becomes through `change`.
    fn table(&self, change: StateChange, half_turn: Option<&[State]>) -> Vec<State> {
        let (b, how) = (self.b, self.how);
        let states = b.state_count() as u32;
        if how.per_block {
            let plans = map_range(b.block_count() as u32, how.parallel, |bl| BlockPlan::new(b, bl as BlockId, change));
            return map_range(states, how.parallel, |s| plans[b.block(s) as usize].apply(b, s, half_turn));
        }
        map_range(states, how.parallel, |s| {
            if !self.unoriented.is_empty() && self.unoriented[b.block(s) as usize] {
                s
            } else {
                transform_state(b, s, change, half_turn)
            }
        })
    }
}

impl StateTransforms {
    pub fn new(b: &Blocks) -> StateTransforms {
        Self::build(b, TableBuild::from_switches())
    }

    pub(crate) fn build(b: &Blocks, how: TableBuild) -> StateTransforms {
        let builder = TableBuilder::new(b, how);
        let quarter = builder.table(StateChange::Rotate(Rot::Cw90), None);
        let (half, three_quarters) = if how.compose_rotations {
            // Two quarter turns make a half turn, a quarter after a half makes three quarters
            // (the proofs check it on every state): one table rewritten instead of three.
            let half = compose(&quarter, &quarter);
            let three_quarters = compose(&quarter, &half);
            (half, three_quarters)
        } else {
            (
                builder.table(StateChange::Rotate(Rot::Cw180), None),
                builder.table(StateChange::Rotate(Rot::Ccw90), None),
            )
        };
        // Mirrored stairs turn half a turn: the new paths read it from the finished table.
        let half_turn = (how.compose_rotations || how.per_block).then_some(half.as_slice());
        let mirror = [
            builder.table(StateChange::Mirror(Mirror::LeftRight), half_turn),
            builder.table(StateChange::Mirror(Mirror::FrontBack), half_turn),
        ];
        StateTransforms {
            rot: [quarter, half, three_quarters],
            mirror,
            procs: crate::structure_procs::ProcTables::new(b),
        }
    }

    /// The five tables: quarter, half and three-quarter turns, left-right and front-back
    /// mirrors (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn tables(&self) -> [&[State]; 5] {
        [&self.rot[0], &self.rot[1], &self.rot[2], &self.mirror[0], &self.mirror[1]]
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

    /// The state mirrored by `m`, then turned by `r`.
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
}

/// Tables built in this process, by generation data key (`RUSTCRAFT_OPT_KEEP_TRANSFORMS`):
/// ≈ 0.6 MB per game version, kept after the world closes so that opening it again skips the
/// build.
static KEPT_TABLES: Mutex<Vec<(String, Arc<StateTransforms>)>> = Mutex::new(Vec::new());

fn kept_tables(key: &str) -> Option<Arc<StateTransforms>> {
    let kept = KEPT_TABLES.lock().unwrap_or_else(PoisonError::into_inner);
    kept.iter().find(|(k, _)| k == key).map(|(_, t)| t.clone())
}

fn keep_tables(key: &str, t: &Arc<StateTransforms>) {
    let mut kept = KEPT_TABLES.lock().unwrap_or_else(PoisonError::into_inner);
    if !kept.iter().any(|(k, _)| k == key) {
        kept.push((key.to_owned(), t.clone()));
    }
}

/// State transformation tables of a set of templates, possibly still being built off the
/// critical path (`RUSTCRAFT_OPT_TRANSFORMS_IN_BACKGROUND`): the first use waits for that build,
/// or does it itself if it has not started yet. Reads like a [`StateTransforms`].
pub struct TransformTables(Arc<PendingTables>);

struct PendingTables {
    tables: OnceLock<Arc<StateTransforms>>,
    /// Blocks to build from and key to keep the result under (`None` when built at construction).
    source: Option<(Arc<Blocks>, Option<String>)>,
}

impl PendingTables {
    fn get(&self) -> &StateTransforms {
        self.tables.get_or_init(|| {
            let (blocks, keep_key) = self.source.as_ref().expect("tables without a source are built at construction");
            // Sequential on purpose: waiters block inside this cell, so a build that needed other
            // workers of the rayon pool could wait forever on workers that wait for it.
            let t = Arc::new(StateTransforms::build(blocks, TableBuild { parallel: false, ..TableBuild::from_switches() }));
            if let Some(key) = keep_key {
                keep_tables(key, &t);
            }
            t
        })
    }
}

impl std::ops::Deref for TransformTables {
    type Target = StateTransforms;

    fn deref(&self) -> &StateTransforms {
        self.0.get()
    }
}

impl TransformTables {
    /// Tables already built.
    fn ready(t: Arc<StateTransforms>) -> TransformTables {
        TransformTables(Arc::new(PendingTables { tables: OnceLock::from(t), source: None }))
    }

    /// Tables of a world's generation data: those kept from an earlier opening of the same data
    /// (`RUSTCRAFT_OPT_KEEP_TRANSFORMS`), otherwise built where the switch says.
    fn for_world(blocks: &Arc<Blocks>, data_key: String) -> TransformTables {
        let keep_key = crate::opt::keep_transforms().then_some(data_key);
        Self::start(blocks, keep_key, crate::opt::transforms_in_background())
    }

    pub(crate) fn start(blocks: &Arc<Blocks>, keep_key: Option<String>, thread: TablesThread) -> TransformTables {
        if let Some(kept) = keep_key.as_deref().and_then(kept_tables) {
            return Self::ready(kept);
        }
        if thread == TablesThread::Inline {
            let t = Arc::new(StateTransforms::new(blocks));
            if let Some(key) = &keep_key {
                keep_tables(key, &t);
            }
            return Self::ready(t);
        }
        let pending = Arc::new(PendingTables { tables: OnceLock::new(), source: Some((blocks.clone(), keep_key)) });
        let job = pending.clone();
        let build = move || {
            job.get();
        };
        if thread == TablesThread::RayonPool {
            rayon::spawn(build);
        } else {
            // A thread that cannot start leaves the build to the first use.
            let _ = std::thread::Builder::new().name("state tables".into()).spawn(build);
        }
        TransformTables(pending)
    }

    /// The tables are built (proofs and benches).
    #[cfg(test)]
    pub(crate) fn is_built(&self) -> bool {
        self.0.tables.get().is_some()
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
    pub transforms: TransformTables,
}

impl std::fmt::Debug for Templates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Templates({} files)", self.files.len())
    }
}

impl Templates {
    /// `files`: `ns:path` → gzip bytes of the `.nbt`; state tables built on the spot.
    pub fn new(files: HashMap<String, Arc<[u8]>>, b: &Blocks) -> Templates {
        Self::with_transforms(files, TransformTables::ready(Arc::new(StateTransforms::new(b))))
    }

    /// Templates of a world's generation data: state tables reused or built off the critical
    /// path ([`TransformTables`]). `data_key` names the jar and game version of the blocks.
    pub fn for_world(files: HashMap<String, Arc<[u8]>>, blocks: &Arc<Blocks>, data_key: String) -> Templates {
        Self::with_transforms(files, TransformTables::for_world(blocks, data_key))
    }

    fn with_transforms(files: HashMap<String, Arc<[u8]>>, transforms: TransformTables) -> Templates {
        Templates {
            files: files.into_iter().map(|(k, v)| (k, (v, OnceLock::new()))).collect(),
            cache: Mutex::new(HashMap::new()),
            decodes: AtomicUsize::new(0),
            transforms,
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
