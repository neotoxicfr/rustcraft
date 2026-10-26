//! Navigation and path finding (`server-mobs.md` § 3.5): the game's A* (binary heap with
//! strict comparisons, walked distance, heuristic ×1.5), ground evaluator (cell types,
//! climbing, descending, diagonals), path following and blockage detection.

use hashbrown::HashMap;

use rc_physics::Aabb;

use super::env::Fluid;
use super::species::Kind;
use crate::server::Server;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathType {
    Blocked,
    Open,
    Walkable,
    WalkableDoor,
    Trapdoor,
    PowderSnow,
    OnTopOfPowderSnow,
    Fence,
    Lava,
    Water,
    WaterBorder,
    Rail,
    UnpassableRail,
    FireInNeighbor,
    Fire,
    DamagingInNeighbor,
    Damaging,
    DoorOpen,
    DoorWoodClosed,
    DoorIronClosed,
    Breach,
    Leaves,
    StickyHoney,
    Cocoa,
    DamageCautious,
    OnTopOfTrapdoor,
    BigMobsCloseToDanger,
}

impl PathType {
    pub fn default_malus(self) -> f32 {
        use PathType::*;
        match self {
            Blocked | PowderSnow | Fence | Lava | UnpassableRail | Damaging | DoorWoodClosed | DoorIronClosed
            | Leaves => -1.0,
            Water | WaterBorder | FireInNeighbor | DamagingInNeighbor | StickyHoney => 8.0,
            Fire => 16.0,
            Breach | BigMobsCloseToDanger => 4.0,
            _ => 0.0,
        }
    }
}

/// Penalty of a cell for a species (overrides of each mob).
/// Penalty of a mob: the one set along the way, otherwise that of
/// the species.
pub fn mob_malus(m: &super::mob::Mob, t: PathType) -> f32 {
    match m.nav.malus_override.iter().find(|(k, _)| *k == t) {
        Some(&(_, v)) => v,
        None => malus(m.kind, t),
    }
}

/// Sets a penalty for the mob.
pub fn set_malus(m: &mut super::mob::Mob, t: PathType, v: f32) {
    m.nav.malus_override.retain(|(k, _)| *k != t);
    m.nav.malus_override.push((t, v));
}

pub fn malus(kind: Kind, t: PathType) -> f32 {
    use PathType::*;
    // F11: bee (fire, water, cocoa, fences), fox (hurtful blocks allowed).
    if let Some(v) = super::wild::malus(kind, t) {
        return v;
    }
    // F12: blaze, wither skeleton, strider (penalties set by their constructors).
    if let Some(v) = super::nether::malus(kind, t) {
        return v;
    }
    // F13: piglins (fire), zombified piglin (lava).
    if let Some(v) = super::piglins::malus(kind, t) {
        return v;
    }
    match (kind, t) {
        // F09: water costs nothing to aquatic fauna or nautiluses.
        (k, Water) if super::sealife::handles(k) => 0.0,
        // F10: sniffer in water or on fire at path computation.
        (Kind::Sniffer, Water) if super::brainy::sniffer_water_ok() => 0.0,
        (k, t) if super::brainy::malus(k, t).is_some() => super::brainy::malus(k, t).unwrap_or(0.0),
        (Kind::Chicken, Water) => 0.0,
        (Kind::Wolf, PowderSnow | OnTopOfPowderSnow) => -1.0,
        (Kind::Enderman, Water) => -1.0,
        // Turtle: water costs nothing, no passable door.
        (Kind::Turtle, Water) => 0.0,
        // F08: drowned and guardians (water costs nothing).
        (Kind::Drowned | Kind::Guardian | Kind::ElderGuardian, Water) => 0.0,
        (Kind::Turtle, DoorIronClosed | DoorWoodClosed | DoorOpen) => -1.0,
        // Parrot: neither fire nor cocoa (before the generic rule for animals).
        (Kind::Parrot, FireInNeighbor | Fire | Cocoa) => -1.0,
        // F06: copper golem (fire and nearby damage 16, fire impassable).
        (Kind::CopperGolem, FireInNeighbor | DamagingInNeighbor) => 16.0,
        (Kind::CopperGolem, Fire) => -1.0,
        // F07: the ravager passes through foliage (which it tears up).
        (Kind::Ravager, Leaves) => 0.0,
        (k, FireInNeighbor) if k.is_animal() || k == Kind::Villager => 16.0,
        (k, Fire) if k.is_animal() || k == Kind::Villager => -1.0,
        _ => t.default_malus(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Node {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub heap: i32,
    pub g: f32,
    pub h: f32,
    pub f: f32,
    pub came_from: Option<usize>,
    pub closed: bool,
    pub walked: f32,
    pub malus: f32,
    pub ty: PathType,
}

impl Node {
    fn new(x: i32, y: i32, z: i32) -> Node {
        Node {
            x,
            y,
            z,
            heap: -1,
            g: 0.0,
            h: 0.0,
            f: 0.0,
            came_from: None,
            closed: false,
            walked: 0.0,
            malus: 0.0,
            ty: PathType::Blocked,
        }
    }

    fn dist(&self, o: &Node) -> f32 {
        let (dx, dy, dz) = ((o.x - self.x) as f32, (o.y - self.y) as f32, (o.z - self.z) as f32);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    fn dist_xz(&self, o: &Node) -> f32 {
        let (dx, dz) = ((o.x - self.x) as f32, (o.z - self.z) as f32);
        (dx * dx + dz * dz).sqrt()
    }

    fn dist_to(&self, p: [i32; 3]) -> f32 {
        let (dx, dy, dz) = ((p[0] - self.x) as f32, (p[1] - self.y) as f32, (p[2] - self.z) as f32);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    fn manhattan(&self, p: [i32; 3]) -> f32 {
        ((p[0] - self.x).abs() as f32) + ((p[1] - self.y).abs() as f32) + ((p[2] - self.z).abs() as f32)
    }

    pub fn pos(&self) -> [i32; 3] {
        [self.x, self.y, self.z]
    }
}

/// Path found: nodes (position, type), target, reached.
#[derive(Debug, Clone)]
pub struct Path {
    pub nodes: Vec<([i32; 3], PathType)>,
    pub next: usize,
    pub target: [i32; 3],
    pub dist_to_target: f32,
    pub reached: bool,
}

impl Path {
    pub fn is_done(&self) -> bool {
        self.next >= self.nodes.len()
    }

    pub fn end(&self) -> Option<[i32; 3]> {
        self.nodes.last().map(|n| n.0)
    }

    /// Position of the mob's foot on a node.
    pub fn entity_pos_at(&self, width: f32, i: usize) -> [f64; 3] {
        let n = self.nodes[i].0;
        let off = f64::from((width + 1.0) as i32) * 0.5;
        [f64::from(n[0]) + off, f64::from(n[1]), f64::from(n[2]) + off]
    }

    fn same_as(&self, o: &Path) -> bool {
        self.nodes.len() == o.nodes.len() && self.nodes.iter().zip(&o.nodes).all(|(a, b)| a.0 == b.0)
    }
}

/// Navigation state of a mob (ground or climbing).
#[derive(Debug, Clone)]
pub struct Nav {
    pub path: Option<Path>,
    pub speed_mod: f64,
    pub tick: i32,
    last_stuck_check: i32,
    last_stuck_pos: [f64; 3],
    timeout_node: [i32; 3],
    timeout_timer: i64,
    last_timeout_check: i64,
    timeout_limit: f64,
    pub delayed_recompute: bool,
    time_last_recompute: i64,
    pub target_pos: Option<[i32; 3]>,
    reach: i32,
    pub stuck: bool,
    pub required_length: f32,
    pub max_visited: i32,
    pub can_float: bool,
    pub can_open_doors: bool,
    /// Penalties set along the way, taking priority over those of the species.
    pub malus_override: Vec<(PathType, f32)>,
    pub avoid_sun: bool,
    /// Climber (spider): target position off the path.
    pub climber: bool,
    pub path_to: Option<[i32; 3]>,
    /// Flying (parrot) and amphibious (turtle) navigation, F02 (`surface_nav`).
    pub fly: bool,
    pub amphibious: bool,
    /// F06: targets below the surface allowed (copper golem transport).
    pub below_surface: bool,
    /// F09: swimming navigation (`swim_nav`), breach allowed (dolphin).
    pub swim: bool,
    pub breach: bool,
}

impl Nav {
    pub fn new(follow_base: f64) -> Nav {
        Nav {
            path: None,
            speed_mod: 0.0,
            tick: 0,
            last_stuck_check: 0,
            last_stuck_pos: [0.0; 3],
            timeout_node: [0; 3],
            timeout_timer: 0,
            last_timeout_check: 0,
            timeout_limit: 0.0,
            delayed_recompute: false,
            time_last_recompute: 0,
            target_pos: None,
            reach: 0,
            stuck: false,
            required_length: 16.0,
            max_visited: (follow_base * 16.0).floor() as i32,
            can_float: false,
            can_open_doors: false,
            malus_override: Vec::new(),
            avoid_sun: false,
            climber: false,
            path_to: None,
            fly: false,
            amphibious: false,
            below_surface: false,
            swim: false,
            breach: false,
        }
    }

    pub fn is_done(&self) -> bool {
        self.path.as_ref().is_none_or(Path::is_done)
    }

    pub fn stop(&mut self) {
        self.path = None;
    }

    fn reset_stuck_timeout(&mut self) {
        self.timeout_node = [0; 3];
        self.timeout_timer = 0;
        self.timeout_limit = 0.0;
        self.stuck = false;
    }
}

/// What the evaluator knows about the mob.
pub struct MobView {
    pub kind: Kind,
    pub pos: [f64; 3],
    pub block: [i32; 3],
    pub bbox: Aabb,
    pub width: f32,
    pub height: f32,
    pub step: f64,
    pub on_ground: bool,
    pub in_water: bool,
    pub max_fall: i32,
    pub can_float: bool,
    pub can_open_doors: bool,
    pub fly: bool,
    pub amphibious: bool,
    pub rng_seed: i64,
    /// F09: swimmer, breach allowed.
    pub swim: bool,
    pub breach: bool,
    /// F14: linked creaking heart (cells blocked beyond 32 blocks, own penalties).
    pub home: Option<[i32; 3]>,
    /// Penalties set along the way (owner following: water costs nothing any more).
    pub malus_override: Vec<(PathType, f32)>,
}

struct Eval<'a> {
    s: &'a Server,
    m: &'a MobView,
    nodes: Vec<Node>,
    index: HashMap<(i32, i32, i32), usize>,
    types: HashMap<(i32, i32, i32), PathType>,
    collisions: HashMap<[u64; 6], bool>,
    ew: i32,
    eh: i32,
}

/// Horizontal directions of the game (north, east, south, west) and their clockwise order.
const HORIZONTAL: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

impl<'a> Eval<'a> {
    fn new(s: &'a Server, m: &'a MobView) -> Eval<'a> {
        Eval {
            s,
            m,
            nodes: Vec::new(),
            index: HashMap::new(),
            types: HashMap::new(),
            collisions: HashMap::new(),
            ew: (m.width + 1.0).floor() as i32,
            eh: (m.height + 1.0).floor() as i32,
        }
    }

    fn node(&mut self, x: i32, y: i32, z: i32) -> usize {
        if let Some(&i) = self.index.get(&(x, y, z)) {
            return i;
        }
        self.nodes.push(Node::new(x, y, z));
        let i = self.nodes.len() - 1;
        self.index.insert((x, y, z), i);
        i
    }

    fn malus(&self, t: PathType) -> f32 {
        if let Some(&(_, v)) = self.m.malus_override.iter().find(|(k, _)| *k == t) {
            return v;
        }
        // F14: penalties set by the transient marking (creaking linked to its heart).
        if let Some(v) = super::recent::view_malus(self.m.kind, self.m.home.is_some(), t) {
            return v;
        }
        if self.m.amphibious {
            match t {
                PathType::Water => return 0.0,
                PathType::Walkable => return 6.0,
                PathType::WaterBorder => return 4.0,
                _ => {}
            }
        }
        malus(self.m.kind, t)
    }

    /// "Static" type of a single block.
    fn type_from_state(&self, p: [i32; 3]) -> PathType {
        let s = self.s;
        if s.is_air(p) {
            return PathType::Open;
        }
        let name = s.bname(p).trim_start_matches("minecraft:");
        if name.ends_with("_trapdoor") || name == "lily_pad" || name == "big_dripleaf" {
            return PathType::Trapdoor;
        }
        match name {
            "powder_snow" => return PathType::PowderSnow,
            "cactus" | "sweet_berry_bush" => return PathType::Damaging,
            "honey_block" => return PathType::StickyHoney,
            "cocoa" => return PathType::Cocoa,
            "wither_rose" => return PathType::DamageCautious,
            _ => {}
        }
        // Stalactites and sulfur spikes (speleothem tag).
        if s.btag_id(p, crate::tag_bits::TagId::Speleothems) {
            return PathType::DamageCautious;
        }
        let fluid = s.mob_fluid(p);
        if fluid == Fluid::Lava {
            return PathType::Lava;
        }
        let lit = || s.data.state_properties(s.st(p)).any(|kv| kv == ("lit", "true"));
        if name == "fire"
            || name == "soul_fire"
            || name == "lava"
            || name == "magma_block"
            || name == "lava_cauldron"
            || ((name == "campfire" || name == "soul_campfire") && lit())
        {
            return PathType::Fire;
        }
        if name.ends_with("_door") {
            let open = s.data.state_properties(s.st(p)).any(|kv| kv == ("open", "true"));
            return if open {
                PathType::DoorOpen
            } else if name == "iron_door" {
                PathType::DoorIronClosed
            } else {
                PathType::DoorWoodClosed
            };
        }
        if name.ends_with("rail") {
            return PathType::Rail;
        }
        if name.ends_with("_leaves") {
            return PathType::Leaves;
        }
        let gate_closed =
            name.ends_with("_fence_gate") && !s.data.state_properties(s.st(p)).any(|kv| kv == ("open", "true"));
        if s.btag_id(p, crate::tag_bits::TagId::Fences) || s.btag_id(p, crate::tag_bits::TagId::Walls) || gate_closed {
            return PathType::Fence;
        }
        if !s.pathfindable_land(p) {
            return PathType::Blocked;
        }
        if fluid == Fluid::Water { PathType::Water } else { PathType::Open }
    }

    /// The below and the neighbours.
    fn static_type(&self, x: i32, y: i32, z: i32) -> PathType {
        // F14: linked creaking: cell more than 32 blocks from the heart and no closer than the mob.
        if let Some(h) = self.m.home
            && super::recent::home_blocks(h, [x, y, z], self.m.block)
        {
            return PathType::Blocked;
        }
        if self.m.fly {
            return self.fly_type(x, y, z);
        }
        if self.m.amphibious
            && let Some(t) = self.amphibious_type(x, y, z)
        {
            return t;
        }
        let t = self.type_from_state([x, y, z]);
        if t == PathType::Open && y > self.s.dim.height.min_y {
            return match self.type_from_state([x, y - 1, z]) {
                PathType::Open | PathType::Water | PathType::Lava | PathType::Walkable => PathType::Open,
                PathType::Fire => PathType::Fire,
                PathType::Damaging => PathType::Damaging,
                PathType::StickyHoney => PathType::StickyHoney,
                PathType::PowderSnow => PathType::OnTopOfPowderSnow,
                PathType::DamageCautious => PathType::DamageCautious,
                PathType::Trapdoor => PathType::OnTopOfTrapdoor,
                _ => self.check_neighbours(x, y, z),
            };
        }
        t
    }

    fn check_neighbours(&self, x: i32, y: i32, z: i32) -> PathType {
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    match self.type_from_state([x + dx, y + dy, z + dz]) {
                        PathType::Damaging => return PathType::DamagingInNeighbor,
                        PathType::Fire | PathType::Lava => return PathType::FireInNeighbor,
                        PathType::Water => return PathType::WaterBorder,
                        PathType::DamageCautious => return PathType::DamageCautious,
                        _ => {}
                    }
                }
            }
        }
        PathType::Walkable
    }

    /// Type of a cell for the mob (whole footprint).
    fn mob_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        if let Some(&t) = self.types.get(&(x, y, z)) {
            return t;
        }
        let mut set: Vec<PathType> = Vec::new();
        let b = self.m.block;
        for dx in 0..self.ew {
            for dy in 0..self.eh {
                for dz in 0..self.ew {
                    let mut t = self.static_type(x + dx, y + dy, z + dz);
                    if t == PathType::DoorWoodClosed && self.m.can_open_doors {
                        t = PathType::WalkableDoor;
                    }
                    if t == PathType::Rail
                        && self.static_type(b[0], b[1], b[2]) != PathType::Rail
                        && self.static_type(b[0], b[1] - 1, b[2]) != PathType::Rail
                    {
                        t = PathType::UnpassableRail;
                    }
                    if !set.contains(&t) {
                        set.push(t);
                    }
                }
            }
        }
        set.sort();
        let r = if set.len() == 1 {
            set[0]
        } else if set.contains(&PathType::Fence) {
            PathType::Fence
        } else if set.contains(&PathType::UnpassableRail) {
            PathType::UnpassableRail
        } else {
            let mut best = PathType::Blocked;
            let mut best_m = self.malus(best);
            let mut neg = None;
            for &t in &set {
                let m = self.malus(t);
                if m < 0.0 {
                    neg = Some(t);
                    break;
                }
                if m >= best_m {
                    best_m = m;
                    best = t;
                }
            }
            match neg {
                Some(t) => t,
                None => {
                    let cur = self.static_type(x, y, z);
                    if self.ew > 1 {
                        let cheaper = self.malus(cur) < best_m;
                        if cheaper && self.malus(PathType::BigMobsCloseToDanger) < best_m {
                            PathType::BigMobsCloseToDanger
                        } else {
                            best
                        }
                    } else if cur == PathType::Open && best != PathType::Open && best_m == 0.0 {
                        PathType::Open
                    } else {
                        best
                    }
                }
            }
        };
        self.types.insert((x, y, z), r);
        r
    }

    fn floor_level(&self, p: [i32; 3]) -> f64 {
        if (self.m.can_float || self.m.amphibious) && self.s.mob_fluid(p) == Fluid::Water {
            return f64::from(p[1]) + 0.5;
        }
        floor_level(self.s, p)
    }

    fn has_collisions(&mut self, b: &Aabb) -> bool {
        let key = [
            b.min[0].to_bits(),
            b.min[1].to_bits(),
            b.min[2].to_bits(),
            b.max[0].to_bits(),
            b.max[1].to_bits(),
            b.max[2].to_bits(),
        ];
        if let Some(&c) = self.collisions.get(&key) {
            return c;
        }
        let c = !rc_physics::is_free(&self.s.world(), b);
        self.collisions.insert(key, c);
        c
    }

    fn start(&mut self) -> Option<usize> {
        if self.m.swim {
            return Some(self.swim_start());
        }
        if self.m.fly {
            return Some(self.fly_start());
        }
        if self.m.amphibious && self.m.in_water {
            let b = self.m.bbox;
            let p = [b.min[0].floor() as i32, (b.min[1] + 0.5).floor() as i32, b.min[2].floor() as i32];
            return Some(self.start_node(p[0], p[1], p[2]));
        }
        let m = self.m;
        let s = self.s;
        let mut start_y = m.block[1];
        let (bx, bz) = (m.pos[0].floor() as i32, m.pos[2].floor() as i32);
        if m.can_float && m.in_water {
            while s.mob_fluid([bx, start_y, bz]) == Fluid::Water {
                start_y += 1;
            }
            start_y -= 1;
        } else if m.on_ground {
            start_y = (m.pos[1] + 0.5).floor() as i32;
        } else {
            let mut y = (m.pos[1] + 1.0).floor() as i32;
            while y > s.dim.height.min_y {
                start_y = y;
                y -= 1;
                if !s.is_air([bx, y, bz]) && !s.pathfindable_land([bx, y, bz]) {
                    break;
                }
            }
        }
        let sp = m.block;
        if !self.can_start_at(sp[0], start_y, sp[2]) {
            let b = m.bbox;
            for (x, z) in [(b.min[0], b.min[2]), (b.min[0], b.max[2]), (b.max[0], b.min[2]), (b.max[0], b.max[2])] {
                let (x, z) = (x.floor() as i32, z.floor() as i32);
                if self.can_start_at(x, start_y, z) {
                    return Some(self.start_node(x, start_y, z));
                }
            }
        }
        Some(self.start_node(sp[0], start_y, sp[2]))
    }

    fn can_start_at(&mut self, x: i32, y: i32, z: i32) -> bool {
        let t = self.mob_type(x, y, z);
        t != PathType::Open && self.malus(t) >= 0.0
    }

    fn start_node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let t = self.mob_type(x, y, z);
        let i = self.node(x, y, z);
        self.nodes[i].ty = t;
        self.nodes[i].malus = self.malus(t);
        i
    }

    fn node_cost_max(&mut self, x: i32, y: i32, z: i32, t: PathType, cost: f32) -> usize {
        let i = self.node(x, y, z);
        self.nodes[i].ty = t;
        self.nodes[i].malus = self.nodes[i].malus.max(cost);
        i
    }

    fn blocked_node(&mut self, x: i32, y: i32, z: i32) -> usize {
        let i = self.node(x, y, z);
        self.nodes[i].ty = PathType::Blocked;
        self.nodes[i].malus = -1.0;
        i
    }

    fn closed_node(&mut self, x: i32, y: i32, z: i32, t: PathType) -> usize {
        let i = self.node(x, y, z);
        self.nodes[i].closed = true;
        self.nodes[i].ty = t;
        self.nodes[i].malus = t.default_malus();
        i
    }

    fn jump_height(&self) -> f64 {
        1.125f64.max(self.m.step)
    }

    fn partial(t: PathType) -> bool {
        matches!(t, PathType::Fence | PathType::DoorWoodClosed | PathType::DoorIronClosed)
    }

    fn can_reach_without_collision(&mut self, n: usize) -> bool {
        let mut b = self.m.bbox;
        let node = self.nodes[n];
        let size = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
        let mut d = [
            f64::from(node.x) - self.m.pos[0] + size[0] / 2.0,
            f64::from(node.y) - self.m.pos[1] + size[1] / 2.0,
            f64::from(node.z) - self.m.pos[2] + size[2] / 2.0,
        ];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let avg = (size[0] + size[1] + size[2]) / 3.0;
        let steps = (len / avg).ceil() as i32;
        let k = f64::from(1.0f32 / steps as f32);
        d = d.map(|v| v * k);
        for _ in 1..=steps {
            b = b.offset(d[0], d[1], d[2]);
            if self.has_collisions(&b) {
                return false;
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn find_accepted(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        jump: i32,
        node_h: f64,
        dir: (i32, i32),
        cur_t: PathType,
    ) -> Option<usize> {
        let max_y = self.floor_level([x, y, z]);
        if max_y - node_h > self.jump_height() {
            return None;
        }
        let t = self.mob_type(x, y, z);
        let cost = self.malus(t);
        let mut best = None;
        if cost >= 0.0 {
            best = Some(self.node_cost_max(x, y, z, t, cost));
        }
        if Self::partial(cur_t)
            && let Some(b) = best
            && self.nodes[b].malus >= 0.0
            && !self.can_reach_without_collision(b)
        {
            best = None;
        }
        if t == PathType::Walkable || (self.m.amphibious && t == PathType::Water) {
            return best;
        }
        let neg = best.is_none_or(|b| self.nodes[b].malus < 0.0);
        if neg
            && jump > 0
            && t != PathType::Fence
            && t != PathType::UnpassableRail
            && t != PathType::Trapdoor
            && t != PathType::PowderSnow
        {
            best = self.try_jump_on(x, y, z, jump, node_h, dir, cur_t);
        } else if t == PathType::Water && !self.m.can_float && !self.m.amphibious {
            best = self.first_non_water_below(x, y, z, best);
        } else if t == PathType::Open {
            best = Some(self.first_ground_below(x, y, z));
        } else if Self::partial(t) && best.is_none() {
            best = Some(self.closed_node(x, y, z, t));
        }
        best
    }

    #[allow(clippy::too_many_arguments)]
    fn try_jump_on(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        jump: i32,
        node_h: f64,
        dir: (i32, i32),
        cur_t: PathType,
    ) -> Option<usize> {
        let above = self.find_accepted(x, y + 1, z, jump - 1, node_h, dir, cur_t)?;
        if self.m.width >= 1.0 {
            return Some(above);
        }
        let n = self.nodes[above];
        if n.ty != PathType::Open && n.ty != PathType::Walkable {
            return Some(above);
        }
        let cx = f64::from(x - dir.0) + 0.5;
        let cz = f64::from(z - dir.1) + 0.5;
        let hw = f64::from(self.m.width) / 2.0;
        let lo = self.floor_level([cx.floor() as i32, y + 1, cz.floor() as i32]) + 0.001;
        let hi = f64::from(self.m.height) + self.floor_level([n.x, n.y, n.z]) - 0.002;
        let b = Aabb { min: [cx - hw, lo, cz - hw], max: [cx + hw, hi, cz + hw] };
        if self.has_collisions(&b) { None } else { Some(above) }
    }

    fn first_non_water_below(&mut self, x: i32, y: i32, z: i32, mut best: Option<usize>) -> Option<usize> {
        let mut y = y - 1;
        while y > self.s.dim.height.min_y {
            let t = self.mob_type(x, y, z);
            if t != PathType::Water {
                return best;
            }
            let c = self.malus(t);
            best = Some(self.node_cost_max(x, y, z, t, c));
            y -= 1;
        }
        best
    }

    fn first_ground_below(&mut self, x: i32, y: i32, z: i32) -> usize {
        let min = self.s.dim.height.min_y;
        let mut cy = y - 1;
        while cy >= min {
            if y - cy > self.m.max_fall {
                return self.blocked_node(x, cy, z);
            }
            let t = self.mob_type(x, cy, z);
            let c = self.malus(t);
            if t != PathType::Open {
                if c >= 0.0 {
                    return self.node_cost_max(x, cy, z, t, c);
                }
                return self.blocked_node(x, cy, z);
            }
            cy -= 1;
        }
        self.blocked_node(x, y, z)
    }

    fn neighbors(&mut self, cur: usize, out: &mut Vec<usize>) {
        out.clear();
        if self.m.swim {
            self.swim_neighbors(cur, out);
            return;
        }
        if self.m.fly {
            self.fly_neighbors(cur, out);
            return;
        }
        let c = self.nodes[cur];
        let mut jump = 0;
        let above = self.mob_type(c.x, c.y + 1, c.z);
        let here = self.mob_type(c.x, c.y, c.z);
        if self.malus(above) >= 0.0 && here != PathType::StickyHoney {
            jump = self.m.step.max(1.0).floor() as i32;
        }
        let h = self.floor_level([c.x, c.y, c.z]);
        let mut around: [Option<usize>; 4] = [None; 4];
        for (k, d) in HORIZONTAL.iter().enumerate() {
            let n = self.find_accepted(c.x + d.0, c.y, c.z + d.1, jump, h, *d, here);
            around[k] = n;
            if let Some(n) = n {
                let nn = self.nodes[n];
                if !nn.closed && (nn.malus >= 0.0 || c.malus < 0.0) {
                    out.push(n);
                }
            }
        }
        for k in 0..4 {
            let k2 = (k + 1) % 4;
            if !self.diagonal_ok(&c, around[k], around[k2]) {
                continue;
            }
            let (d1, d2) = (HORIZONTAL[k], HORIZONTAL[k2]);
            let n = self.find_accepted(c.x + d1.0 + d2.0, c.y, c.z + d1.1 + d2.1, jump, h, d1, here);
            if let Some(n) = n {
                let nn = self.nodes[n];
                if !nn.closed && nn.ty != PathType::WalkableDoor && nn.malus >= 0.0 {
                    out.push(n);
                }
            }
        }
        if self.m.amphibious {
            self.amphibious_neighbors(cur, out);
        }
    }

    fn diagonal_ok(&self, pos: &Node, ew: Option<usize>, ns: Option<usize>) -> bool {
        let (Some(ew), Some(ns)) = (ew, ns) else { return false };
        let (ew, ns) = (self.nodes[ew], self.nodes[ns]);
        if ns.y > pos.y || ew.y > pos.y {
            return false;
        }
        if ew.ty == PathType::WalkableDoor || ns.ty == PathType::WalkableDoor {
            return false;
        }
        if self.m.width > 1.0 && (ew.malus > 0.0 || ns.malus > 0.0) {
            return false;
        }
        let posts = ns.ty == PathType::Fence && ew.ty == PathType::Fence && f64::from(self.m.width) < 0.5;
        (ns.y < pos.y || ns.malus >= 0.0 || posts) && (ew.y < pos.y || ew.malus >= 0.0 || posts)
    }
}

/// The static path type of a cell (without the mob's footprint).
pub fn static_path_type(s: &Server, view: &MobView, p: [i32; 3]) -> PathType {
    Eval::new(s, view).static_type(p[0], p[1], p[2])
}

/// Ground level of a cell: top of the collision shape of the block below.
pub fn floor_level(s: &Server, p: [i32; 3]) -> f64 {
    let below = [p[0], p[1] - 1, p[2]];
    f64::from(below[1]) + s.collision_top(below).unwrap_or(0.0)
}

// ---------- Binary heap of the game ----------

#[allow(clippy::neg_cmp_op_on_partial_ord)]
struct Heap {
    items: Vec<usize>,
}

#[allow(clippy::neg_cmp_op_on_partial_ord)]
impl Heap {
    fn insert(&mut self, n: &mut [Node], i: usize) {
        self.items.push(i);
        let at = self.items.len() - 1;
        n[i].heap = at as i32;
        self.up(n, at);
    }

    fn pop(&mut self, n: &mut [Node]) -> usize {
        let top = self.items[0];
        let last = self.items.pop().unwrap_or(top);
        if !self.items.is_empty() {
            self.items[0] = last;
            n[last].heap = 0;
            self.down(n, 0);
        }
        n[top].heap = -1;
        top
    }

    fn change_cost(&mut self, n: &mut [Node], i: usize, cost: f32) {
        let old = n[i].f;
        n[i].f = cost;
        let at = n[i].heap as usize;
        if cost < old {
            self.up(n, at);
        } else {
            self.down(n, at);
        }
    }

    fn up(&mut self, n: &mut [Node], mut idx: usize) {
        let node = self.items[idx];
        let cost = n[node].f;
        while idx > 0 {
            let parent = (idx - 1) >> 1;
            let p = self.items[parent];
            if !(cost < n[p].f) {
                break;
            }
            self.items[idx] = p;
            n[p].heap = idx as i32;
            idx = parent;
        }
        self.items[idx] = node;
        n[node].heap = idx as i32;
    }

    fn down(&mut self, n: &mut [Node], mut idx: usize) {
        let node = self.items[idx];
        let cost = n[node].f;
        loop {
            let l = 1 + (idx << 1);
            let r = l + 1;
            if l >= self.items.len() {
                break;
            }
            let ln = self.items[l];
            let lc = n[ln].f;
            let (rn, rc) =
                if r >= self.items.len() { (usize::MAX, f32::INFINITY) } else { (self.items[r], n[self.items[r]].f) };
            if lc < rc {
                if !(lc < cost) {
                    break;
                }
                self.items[idx] = ln;
                n[ln].heap = idx as i32;
                idx = l;
            } else {
                if !(rc < cost) || rn == usize::MAX {
                    break;
                }
                self.items[idx] = rn;
                n[rn].heap = idx as i32;
                idx = r;
            }
        }
        self.items[idx] = node;
        n[node].heap = idx as i32;
    }
}

/// The game's A* towards a single target.
pub fn find_path(
    s: &Server,
    m: &MobView,
    target: [i32; 3],
    max_len: f32,
    reach: i32,
    max_visited: i32,
) -> Option<Path> {
    let mut ev = Eval::new(s, m);
    let from = ev.start()?;
    let tnode = Node::new(target[0], target[1], target[2]);
    let mut best: (f32, usize) = (f32::MAX, from);
    let h = |n: &Node| n.dist(&tnode);
    {
        let hv = h(&ev.nodes[from]);
        if hv < best.0 {
            best = (hv, from);
        }
        let n = &mut ev.nodes[from];
        n.g = 0.0;
        n.h = hv;
        n.f = hv;
    }
    let mut heap = Heap { items: Vec::new() };
    heap.insert(&mut ev.nodes, from);
    let mut count = 0;
    let mut reached = false;
    let mut nb = Vec::with_capacity(8);
    let origin = ev.nodes[from];
    while !heap.items.is_empty() {
        count += 1;
        if count >= max_visited {
            break;
        }
        let cur = heap.pop(&mut ev.nodes);
        ev.nodes[cur].closed = true;
        if ev.nodes[cur].manhattan(target) <= reach as f32 {
            reached = true;
            break;
        }
        if ev.nodes[cur].dist(&origin) >= max_len {
            continue;
        }
        ev.neighbors(cur, &mut nb);
        for &n in &nb {
            let c = ev.nodes[cur];
            // F14: the warden counts distances flat.
            let d = if ev.m.kind == Kind::Warden { c.dist_xz(&ev.nodes[n]) } else { c.dist(&ev.nodes[n]) };
            ev.nodes[n].walked = c.walked + d;
            let g = c.g + d + ev.nodes[n].malus;
            let nn = ev.nodes[n];
            if nn.walked < max_len && (nn.heap < 0 || g < nn.g) {
                let hv = h(&nn);
                if hv < best.0 {
                    best = (hv, n);
                }
                let node = &mut ev.nodes[n];
                node.came_from = Some(cur);
                node.g = g;
                node.h = hv * 1.5;
                if node.heap >= 0 {
                    let f = node.g + node.h;
                    heap.change_cost(&mut ev.nodes, n, f);
                } else {
                    node.f = node.g + node.h;
                    heap.insert(&mut ev.nodes, n);
                }
            }
        }
    }
    // Path towards the best node of the target.
    let mut list = Vec::new();
    let mut i = Some(best.1);
    while let Some(k) = i {
        let n = ev.nodes[k];
        list.push((n.pos(), n.ty));
        i = n.came_from;
    }
    list.reverse();
    let dist = list.last().map_or(f32::MAX, |l| Node::new(l.0[0], l.0[1], l.0[2]).manhattan(target));
    let _ = tnode.dist_to(target);
    Some(Path { nodes: list, next: 0, target, dist_to_target: dist, reached })
}

impl Server {
    /// View of the mob for the evaluator.
    pub(crate) fn mob_view(&self, m: &super::mob::Mob) -> MobView {
        MobView {
            kind: m.kind,
            pos: m.pos,
            block: m.block_pos(),
            bbox: m.bbox(),
            width: m.width(),
            height: m.height(),
            step: m.step_height(),
            on_ground: m.on_ground,
            in_water: m.in_water,
            max_fall: self.max_fall_distance(m),
            can_float: m.nav.can_float,
            can_open_doors: m.nav.can_open_doors,
            fly: m.nav.fly,
            amphibious: m.nav.amphibious,
            rng_seed: i64::from(m.id) ^ i64::from(m.tick_count),
            swim: m.nav.swim,
            breach: m.nav.breach,
            home: super::recent::path_home(m),
            malus_override: m.nav.malus_override.clone(),
        }
    }

    /// The maximum fall tolerated (§ 3.5).
    pub(crate) fn max_fall_distance(&self, m: &super::mob::Mob) -> i32 {
        let comfortable = |extra: f32| -> i32 {
            let safe = m.attrs.safe_fall.value() as f32;
            (safe + extra) as i32
        };
        if m.target.is_none() {
            return comfortable(0.0);
        }
        if m.kind == Kind::Creeper {
            return comfortable(m.health - 1.0);
        }
        let mut sacrifice = (m.health - m.max_health() * 0.33) as i32;
        sacrifice -= (3 - self.level.difficulty_id()) * 4;
        comfortable(sacrifice.max(0) as f32)
    }

    fn can_update_path(m: &super::mob::Mob) -> bool {
        // F09: swimmer: breach allowed or in a liquid.
        if m.nav.swim {
            return m.nav.breach || m.in_water || m.lava_height > 0.0;
        }
        m.nav.fly || m.nav.amphibious || m.on_ground || m.in_water || m.lava_height > 0.0 || m.mount.vehicle.is_some()
    }

    /// Surface of a ground target.
    fn surface_target(&self, mut p: [i32; 3]) -> [i32; 3] {
        let min = self.dim.height.min_y;
        let max = min + self.dim.height.sections as i32 * 16 - 1;
        if self.is_air(p) {
            let mut c = [p[0], p[1] - 1, p[2]];
            while c[1] >= min && self.is_air(c) {
                c[1] -= 1;
            }
            if c[1] >= min {
                return [c[0], c[1] + 1, c[2]];
            }
            c[1] = p[1] + 1;
            while c[1] <= max && self.is_air(c) {
                c[1] += 1;
            }
            p = c;
        }
        if !self.is_solid(p) {
            return p;
        }
        let mut c = [p[0], p[1] + 1, p[2]];
        while c[1] <= max && self.is_solid(c) {
            c[1] += 1;
        }
        c
    }

    /// Creation of the path (ground).
    pub(crate) fn create_path(&self, m: &mut super::mob::Mob, target: [i32; 3], reach: i32) -> Option<Path> {
        let max_len = (m.follow_range() as f32).max(m.nav.required_length);
        self.create_path_len(m, target, reach, max_len)
    }

    /// Creation of the path (F10: long jump, length 8).
    pub(crate) fn create_path_len(
        &self,
        m: &mut super::mob::Mob,
        target: [i32; 3],
        reach: i32,
        max_len: f32,
    ) -> Option<Path> {
        if !self.chunks.get(&(target[0] >> 4, target[2] >> 4)).is_some_and(Option::is_some) {
            return None;
        }
        let target = if m.nav.fly || m.nav.amphibious || m.nav.swim || m.nav.below_surface {
            target
        } else {
            self.surface_target(target)
        };
        if m.nav.climber {
            m.nav.path_to = Some(target);
        }
        if m.pos[1] < f64::from(self.dim.height.min_y) || !Self::can_update_path(m) {
            return None;
        }
        if let Some(p) = &m.nav.path
            && !p.is_done()
            && m.nav.target_pos == Some(target)
        {
            return Some(p.clone());
        }
        let view = self.mob_view(m);
        let path = find_path(self, &view, target, max_len, reach, m.nav.max_visited)?;
        m.nav.target_pos = Some(path.target);
        m.nav.reach = reach;
        m.nav.reset_stuck_timeout();
        Some(path)
    }

    /// Movement towards a point.
    pub(crate) fn move_to_path(&self, m: &mut super::mob::Mob, path: Option<Path>, speed: f64) -> bool {
        let Some(p) = path else {
            m.nav.path = None;
            return false;
        };
        let same = m.nav.path.as_ref().is_some_and(|q| q.same_as(&p));
        if !same {
            m.nav.path = Some(p);
        }
        if m.nav.is_done() {
            return false;
        }
        self.trim_path(m);
        if m.nav.path.as_ref().is_none_or(|p| p.nodes.is_empty()) {
            return false;
        }
        m.nav.speed_mod = speed;
        m.nav.last_stuck_check = m.nav.tick;
        m.nav.last_stuck_pos = self.temp_mob_pos(m);
        true
    }

    pub(crate) fn move_to_pos(&self, m: &mut super::mob::Mob, p: [f64; 3], speed: f64) -> bool {
        let path = self.create_path(m, super::mth::block_of(p), 1);
        self.move_to_path(m, path, speed)
    }

    pub(crate) fn move_to_entity(&self, m: &mut super::mob::Mob, target: [f64; 3], speed: f64) -> bool {
        let reach = if m.nav.climber { 0 } else { 1 };
        let path = self.create_path(m, super::mth::block_of(target), reach);
        if path.is_none() && m.nav.climber {
            m.nav.path_to = Some(super::mth::block_of(target));
            m.nav.speed_mod = speed;
            return true;
        }
        // Towards an entity, the absence of a path leaves the current path in place (the mob
        // keeps following the old route); only the call by path or position clears it.
        match path {
            Some(p) => self.move_to_path(m, Some(p), speed),
            None => false,
        }
    }

    fn trim_path(&self, m: &mut super::mob::Mob) {
        let avoid_sun = m.nav.avoid_sun;
        let pos = m.pos;
        let Some(p) = m.nav.path.as_mut() else { return };
        for i in 0..p.nodes.len() {
            let n = p.nodes[i].0;
            if self.btag_id(n, crate::tag_bits::TagId::Cauldrons) {
                p.nodes[i].0 = [n[0], n[1] + 1, n[2]];
                if i + 1 < p.nodes.len() {
                    let nx = p.nodes[i + 1].0;
                    if n[1] >= nx[1] {
                        p.nodes[i + 1].0 = [nx[0], n[1] + 1, nx[2]];
                    }
                }
            }
        }
        if avoid_sun {
            if self.can_see_sky(super::mth::block_of([pos[0], pos[1] + 0.5, pos[2]])) {
                return;
            }
            if let Some(i) = p.nodes.iter().position(|n| self.can_see_sky(n.0)) {
                p.nodes.truncate(i);
            }
        }
    }

    fn temp_mob_pos(&self, m: &super::mob::Mob) -> [f64; 3] {
        if m.nav.fly {
            return m.pos;
        }
        if m.nav.amphibious || m.nav.swim {
            return [m.pos[0], m.pos[1] + f64::from(m.height()) * 0.5, m.pos[2]];
        }
        let y = if m.in_water && m.nav.can_float {
            let mut s = m.block_pos()[1];
            let (x, z) = (m.pos[0].floor() as i32, m.pos[2].floor() as i32);
            let mut steps = 0;
            loop {
                if self.mob_fluid([x, s, z]) != Fluid::Water {
                    break f64::from(s);
                }
                s += 1;
                steps += 1;
                if steps > 16 {
                    break f64::from(m.block_pos()[1]);
                }
            }
        } else {
            (m.pos[1] + 0.5).floor()
        };
        [m.pos[0], y, m.pos[2]]
    }

    /// Navigation tick.
    pub(crate) fn nav_tick(&self, m: &mut super::mob::Mob) {
        // F11: the bee that is foraging does not advance its navigation.
        if super::wild::nav_frozen(m) {
            return;
        }
        if m.nav.climber && m.nav.is_done() {
            if let Some(t) = m.nav.path_to {
                let center = [f64::from(t[0]) + 0.5, f64::from(t[1]) + 0.5, f64::from(t[2]) + 0.5];
                let w = f64::from(m.width());
                let close = super::mth::dist2(center, m.pos) < w * w;
                let above = m.pos[1] > f64::from(t[1]) && {
                    let c2 = [f64::from(t[0]) + 0.5, m.pos[1].floor() + 0.5, f64::from(t[2]) + 0.5];
                    super::mth::dist2(c2, m.pos) < w * w
                };
                if !close && !above {
                    let s = m.nav.speed_mod;
                    set_wanted(m, [f64::from(t[0]), f64::from(t[1]), f64::from(t[2])], s);
                } else {
                    m.nav.path_to = None;
                }
            }
            return;
        }
        m.nav.tick += 1;
        if m.nav.delayed_recompute {
            self.recompute_path(m);
        }
        if m.nav.is_done() {
            return;
        }
        if Self::can_update_path(m) {
            self.follow_path(m);
        } else if let w = m.width()
            && let mp = [m.pos[0], (m.pos[1] + 0.5).floor(), m.pos[2]]
            && let ground = m.on_ground
            && let Some(p) = m.nav.path.as_mut()
            && !p.is_done()
        {
            let np = p.entity_pos_at(w, p.next);
            if mp[1] > np[1] && !ground && mp[0].floor() == np[0].floor() && mp[2].floor() == np[2].floor() {
                p.next += 1;
            }
        }
        if let Some(p) = &m.nav.path
            && !p.is_done()
        {
            let t = p.entity_pos_at(m.width(), p.next);
            let b = super::mth::block_of(t);
            let gy = if m.nav.fly || m.nav.amphibious || m.nav.swim || self.is_air([b[0], b[1] - 1, b[2]]) {
                t[1]
            } else {
                floor_level(self, b)
            };
            let s = m.nav.speed_mod;
            set_wanted(m, [t[0], gy, t[2]], s);
        }
    }

    fn recompute_path(&self, m: &mut super::mob::Mob) {
        if self.time - m.nav.time_last_recompute <= 20 || !Self::can_update_path(m) {
            m.nav.delayed_recompute = true;
        } else if let Some(t) = m.nav.target_pos {
            m.nav.path = None;
            let r = m.nav.reach;
            m.nav.path = self.create_path(m, t, r);
            m.nav.time_last_recompute = self.time;
            m.nav.delayed_recompute = false;
        }
    }

    fn follow_path(&self, m: &mut super::mob::Mob) {
        let mob_pos = self.temp_mob_pos(m);
        let w = m.width();
        let max_d = f64::from(if w > 0.75 { w / 2.0 } else { 0.75 - w / 2.0 });
        let Some(p) = m.nav.path.as_ref() else { return };
        let cur = p.nodes[p.next].0;
        let xd = (m.pos[0] - (f64::from(cur[0]) + 0.5)).abs();
        let yd = (m.pos[1] - f64::from(cur[1])).abs();
        let zd = (m.pos[2] - (f64::from(cur[2]) + 0.5)).abs();
        // F08/F09: vertical difference allowed of 0.5 for swimming navigation.
        let close = xd < max_d && zd < max_d && yd < if m.nav.swim { 0.5 } else { 1.0 };
        let ty = p.nodes[p.next].1;
        let cut = !matches!(ty, PathType::FireInNeighbor | PathType::DamagingInNeighbor | PathType::WalkableDoor)
            && (self.next_in_direction_direct(m, p, mob_pos) || self.next_in_direction(p, w, mob_pos));
        if (close || cut)
            && let Some(p) = m.nav.path.as_mut()
        {
            p.next += 1;
        }
        self.stuck_detection(m, mob_pos);
    }

    fn next_in_direction(&self, p: &Path, _w: f32, mp: [f64; 3]) -> bool {
        if p.next + 1 >= p.nodes.len() {
            return false;
        }
        let c = p.nodes[p.next].0;
        let cur = [f64::from(c[0]) + 0.5, f64::from(c[1]), f64::from(c[2]) + 0.5];
        if super::mth::dist2(mp, cur) >= 4.0 {
            return false;
        }
        let nx = p.nodes[p.next + 1].0;
        let next = [f64::from(nx[0]) + 0.5, f64::from(nx[1]), f64::from(nx[2]) + 0.5];
        let to_cur = [cur[0] - mp[0], cur[1] - mp[1], cur[2] - mp[2]];
        let to_next = [next[0] - mp[0], next[1] - mp[1], next[2] - mp[2]];
        let lc = to_cur.iter().map(|v| v * v).sum::<f64>();
        let ln = to_next.iter().map(|v| v * v).sum::<f64>();
        if ln >= lc && lc >= 0.5 {
            return false;
        }
        let norm = |v: [f64; 3]| {
            let l = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            if l < f64::from(1.0e-5f32) { [0.0; 3] } else { v.map(|x| x / l) }
        };
        let (a, b) = (norm(to_cur), norm(to_next));
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2] < 0.0
    }

    fn stuck_detection(&self, m: &mut super::mob::Mob, mp: [f64; 3]) {
        let speed = m.speed;
        if m.nav.tick - m.nav.last_stuck_check > 100 {
            let eff = if speed >= 1.0 { speed } else { speed * speed };
            let thr = eff * 100.0 * 0.25;
            if super::mth::dist2(mp, m.nav.last_stuck_pos) < f64::from(thr * thr) {
                m.nav.stuck = true;
                m.nav.stop();
            } else {
                m.nav.stuck = false;
            }
            m.nav.last_stuck_check = m.nav.tick;
            m.nav.last_stuck_pos = mp;
        }
        let now = self.time;
        if let Some(p) = &m.nav.path
            && !p.is_done()
        {
            let pos = p.nodes[p.next].0;
            if pos == m.nav.timeout_node {
                m.nav.timeout_timer += now - m.nav.last_timeout_check;
            } else {
                m.nav.timeout_node = pos;
                let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]), f64::from(pos[2]) + 0.5];
                let d = super::mth::dist2(mp, c).sqrt();
                m.nav.timeout_limit = if speed > 0.0 { d / f64::from(speed) * 20.0 } else { 0.0 };
            }
            if m.nav.timeout_limit > 0.0 && m.nav.timeout_timer as f64 > m.nav.timeout_limit * 3.0 {
                m.nav.reset_stuck_timeout();
                m.nav.stop();
            }
            m.nav.last_timeout_check = now;
        }
    }

    /// Block below solid for rendering.
    pub(crate) fn stable_destination(&self, p: [i32; 3]) -> bool {
        self.solid_render([p[0], p[1] - 1, p[2]])
    }
}

/// The position instruction of the movement control.
pub fn set_wanted(m: &mut super::mob::Mob, p: [f64; 3], speed: f64) {
    if m.kind == Kind::Rabbit {
        // Movement control specific to the rabbit (speed of the next hop).
        super::surface::rabbit::set_wanted(m, p, speed);
        return;
    }
    m.move_ctl.wanted = p;
    m.move_ctl.speed_mod = speed;
    if m.move_ctl.op != super::mob::MoveOp::Jumping {
        m.move_ctl.op = super::mob::MoveOp::MoveTo;
    }
}

// Flying and amphibious evaluators, stability of destinations (F02).
mod surface_nav;
// Swimming evaluator (F08 guardians, F09 fauna).
mod swim_nav;

// ---------- Path towards several targets (F05: villager POIs) ----------

/// The game's A* towards a set of targets: stops as soon as a target is in range; path towards the
/// best node of the shortest reached target, otherwise of the nearest target.
pub fn find_path_multi(
    s: &Server,
    m: &MobView,
    targets: &[[i32; 3]],
    max_len: f32,
    reach: i32,
    max_visited: i32,
) -> Option<Path> {
    if targets.is_empty() {
        return None;
    }
    let mut ev = Eval::new(s, m);
    let from = ev.start()?;
    // Best node (distance, index) and reached, per target.
    let mut best: Vec<(f32, usize)> = vec![(f32::MAX, from); targets.len()];
    let mut reached = vec![false; targets.len()];
    let best_h = |nodes: &[Node], n: usize, best: &mut Vec<(f32, usize)>| -> f32 {
        let mut b = f32::MAX;
        for (i, t) in targets.iter().enumerate() {
            let h = nodes[n].dist_to(*t);
            if h < best[i].0 {
                best[i] = (h, n);
            }
            b = b.min(h);
        }
        b
    };
    let hv = best_h(&ev.nodes, from, &mut best);
    {
        let n = &mut ev.nodes[from];
        n.g = 0.0;
        n.h = hv;
        n.f = hv;
    }
    let mut heap = Heap { items: Vec::new() };
    heap.insert(&mut ev.nodes, from);
    let mut count = 0;
    let mut nb = Vec::with_capacity(8);
    let origin = ev.nodes[from];
    while !heap.items.is_empty() {
        count += 1;
        if count >= max_visited {
            break;
        }
        let cur = heap.pop(&mut ev.nodes);
        ev.nodes[cur].closed = true;
        let mut any = false;
        for (i, t) in targets.iter().enumerate() {
            if ev.nodes[cur].manhattan(*t) <= reach as f32 {
                reached[i] = true;
                any = true;
            }
        }
        if any {
            break;
        }
        if ev.nodes[cur].dist(&origin) >= max_len {
            continue;
        }
        ev.neighbors(cur, &mut nb);
        for &n in &nb {
            let c = ev.nodes[cur];
            // F14: the warden counts distances flat.
            let d = if ev.m.kind == Kind::Warden { c.dist_xz(&ev.nodes[n]) } else { c.dist(&ev.nodes[n]) };
            ev.nodes[n].walked = c.walked + d;
            let g = c.g + d + ev.nodes[n].malus;
            let nn = ev.nodes[n];
            if nn.walked < max_len && (nn.heap < 0 || g < nn.g) {
                let hv = best_h(&ev.nodes, n, &mut best);
                let node = &mut ev.nodes[n];
                node.came_from = Some(cur);
                node.g = g;
                node.h = hv * 1.5;
                if node.heap >= 0 {
                    let f = node.g + node.h;
                    heap.change_cost(&mut ev.nodes, n, f);
                } else {
                    node.f = node.g + node.h;
                    heap.insert(&mut ev.nodes, n);
                }
            }
        }
    }
    let build = |i: usize, ok: bool| -> Path {
        let mut list = Vec::new();
        let mut k = Some(best[i].1);
        while let Some(j) = k {
            let n = ev.nodes[j];
            list.push((n.pos(), n.ty));
            k = n.came_from;
        }
        list.reverse();
        let dist = list.last().map_or(f32::MAX, |l| Node::new(l.0[0], l.0[1], l.0[2]).manhattan(targets[i]));
        Path { nodes: list, next: 0, target: targets[i], dist_to_target: dist, reached: ok }
    };
    let any = reached.iter().any(|&r| r);
    let mut out: Option<Path> = None;
    for (i, &r) in reached.iter().enumerate() {
        if any && !r {
            continue;
        }
        let p = build(i, any);
        let better = match &out {
            None => true,
            Some(o) if any => p.nodes.len() < o.nodes.len(),
            Some(o) => {
                p.dist_to_target < o.dist_to_target
                    || (p.dist_to_target == o.dist_to_target && p.nodes.len() < o.nodes.len())
            }
        };
        if better {
            out = Some(p);
        }
    }
    out
}

impl Server {
    /// Without surface search; current path kept
    /// if it already targets one of the targets.
    pub(crate) fn create_path_multi(&self, m: &mut super::mob::Mob, targets: &[[i32; 3]], reach: i32) -> Option<Path> {
        if targets.is_empty() || m.pos[1] < f64::from(self.dim.height.min_y) || !Self::can_update_path(m) {
            return None;
        }
        if let Some(p) = &m.nav.path
            && !p.is_done()
            && m.nav.target_pos.is_some_and(|t| targets.contains(&t))
        {
            return Some(p.clone());
        }
        let max_len = (m.follow_range() as f32).max(m.nav.required_length);
        let view = self.mob_view(m);
        let path = find_path_multi(self, &view, targets, max_len, reach, m.nav.max_visited)?;
        m.nav.target_pos = Some(path.target);
        m.nav.reach = reach;
        m.nav.reset_stuck_timeout();
        Some(path)
    }
}
