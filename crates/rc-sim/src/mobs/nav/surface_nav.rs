//! Flying path evaluators (parrot) and amphibious (turtle) grafted onto the land
//! evaluator (`server-mobs.md` § 3.5), free straight line, stability of destinations.

use super::super::env::Fluid;
use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::{Eval, Path, PathType};
use crate::server::Server;

impl Eval<'_> {
    /// Type of a cell for a flyer.
    pub(super) fn fly_type(&self, x: i32, y: i32, z: i32) -> PathType {
        let mut t = self.type_from_state([x, y, z]);
        if t == PathType::Open && y > self.s.dim.height.min_y {
            let below = self.type_from_state([x, y - 1, z]);
            t = match below {
                PathType::Fire | PathType::Lava => PathType::Fire,
                PathType::Damaging => PathType::Damaging,
                PathType::Cocoa => PathType::Cocoa,
                PathType::Fence => {
                    if [x, y - 1, z] != self.m.block {
                        PathType::Fence
                    } else {
                        PathType::Open
                    }
                }
                PathType::Walkable | PathType::Open | PathType::Water => PathType::Open,
                _ => PathType::Walkable,
            };
        }
        if t == PathType::Walkable || t == PathType::Open {
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
        }
        t
    }

    /// Water seen by the amphibious evaluator: border if a direct neighbour is blocking.
    pub(super) fn amphibious_type(&self, x: i32, y: i32, z: i32) -> Option<PathType> {
        if self.type_from_state([x, y, z]) != PathType::Water {
            return None;
        }
        let dirs = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
        for (dx, dy, dz) in dirs {
            if self.type_from_state([x + dx, y + dy, z + dz]) == PathType::Blocked {
                return Some(PathType::WaterBorder);
            }
        }
        Some(PathType::Water)
    }

    fn can_start_fly(&mut self, x: i32, y: i32, z: i32) -> bool {
        let t = self.mob_type(x, y, z);
        self.malus(t) >= 0.0
    }

    /// Start of a flying path.
    pub(super) fn fly_start(&mut self) -> usize {
        let m = self.m;
        let s = self.s;
        let (bx, bz) = (m.pos[0].floor() as i32, m.pos[2].floor() as i32);
        let start_y = if m.can_float && m.in_water {
            let mut y = m.block[1];
            while s.mob_fluid([bx, y, bz]) == Fluid::Water && s.bname([bx, y, bz]) == "minecraft:water" {
                y += 1;
            }
            y
        } else {
            (m.pos[1] + 0.5).floor() as i32
        };
        if !self.can_start_fly(bx, start_y, bz) {
            let b = m.bbox;
            let size = ((b.max[0] - b.min[0]) + (b.max[1] - b.min[1]) + (b.max[2] - b.min[2])) / 3.0;
            let cands: Vec<[i32; 3]> = if size >= 1.0 {
                [(b.min[0], b.min[2]), (b.min[0], b.max[2]), (b.max[0], b.min[2]), (b.max[0], b.max[2])]
                    .iter()
                    .map(|&(x, z)| [x.floor() as i32, m.block[1], z.floor() as i32])
                    .collect()
            } else {
                // Draws on the mob's random in the game; here an own sequence (difference noted).
                let pad = |len: f64| (f64::from(1.1f32) - len).max(0.0);
                let (px, py, pz) = (pad(b.max[0] - b.min[0]), pad(b.max[1] - b.min[1]), pad(b.max[2] - b.min[2]));
                let lo =
                    [(b.min[0] - px).floor() as i32, (b.min[1] - py).floor() as i32, (b.min[2] - pz).floor() as i32];
                let hi =
                    [(b.max[0] + px).floor() as i32, (b.max[1] + py).floor() as i32, (b.max[2] + pz).floor() as i32];
                let mut r = JRand::new(m.rng_seed);
                (0..10)
                    .map(|_| {
                        [
                            lo[0] + r.next_int_bound(hi[0] - lo[0] + 1),
                            lo[1] + r.next_int_bound(hi[1] - lo[1] + 1),
                            lo[2] + r.next_int_bound(hi[2] - lo[2] + 1),
                        ]
                    })
                    .collect()
            };
            for c in cands {
                if self.can_start_fly(c[0], c[1], c[2]) {
                    return self.start_node(c[0], c[1], c[2]);
                }
            }
        }
        self.start_node(bx, start_y, bz)
    }

    /// The penalty of a walkable cell climbs by 1 at each
    /// examination (node cached), as in the game.
    fn fly_accepted(&mut self, x: i32, y: i32, z: i32) -> Option<usize> {
        let t = self.mob_type(x, y, z);
        let cost = self.malus(t);
        if cost < 0.0 {
            return None;
        }
        let i = self.node(x, y, z);
        self.nodes[i].ty = t;
        self.nodes[i].malus = self.nodes[i].malus.max(cost);
        if t == PathType::Walkable {
            self.nodes[i].malus += 1.0;
        }
        Some(i)
    }

    fn open(&self, n: Option<usize>) -> bool {
        n.is_some_and(|i| !self.nodes[i].closed)
    }

    fn has_m(&self, n: Option<usize>) -> bool {
        n.is_some_and(|i| self.nodes[i].malus >= 0.0)
    }

    /// Neighbours of a flyer: 6 faces, 12 edges, 8 corners (the game's order).
    pub(super) fn fly_neighbors(&mut self, cur: usize, out: &mut Vec<usize>) {
        let c = self.nodes[cur];
        let (x, y, z) = (c.x, c.y, c.z);
        let f = |e: &mut Self, dx: i32, dy: i32, dz: i32| e.fly_accepted(x + dx, y + dy, z + dz);
        let south = f(self, 0, 0, 1);
        let west = f(self, -1, 0, 0);
        let east = f(self, 1, 0, 0);
        let north = f(self, 0, 0, -1);
        let up = f(self, 0, 1, 0);
        let down = f(self, 0, -1, 0);
        for n in [south, west, east, north, up, down] {
            if self.open(n) {
                out.push(n.unwrap_or_default());
            }
        }
        let mut edge = |e: &mut Self, dx: i32, dy: i32, dz: i32, a: Option<usize>, b: Option<usize>| {
            let n = f(e, dx, dy, dz);
            if e.open(n) && e.has_m(a) && e.has_m(b) {
                out.push(n.unwrap_or_default());
            }
            n
        };
        let south_up = edge(self, 0, 1, 1, south, up);
        let west_up = edge(self, -1, 1, 0, west, up);
        let east_up = edge(self, 1, 1, 0, east, up);
        let north_up = edge(self, 0, 1, -1, north, up);
        let south_down = edge(self, 0, -1, 1, south, down);
        let west_down = edge(self, -1, -1, 0, west, down);
        let east_down = edge(self, 1, -1, 0, east, down);
        let north_down = edge(self, 0, -1, -1, north, down);
        let north_east = edge(self, 1, 0, -1, north, east);
        let south_east = edge(self, 1, 0, 1, south, east);
        let north_west = edge(self, -1, 0, -1, north, west);
        let south_west = edge(self, -1, 0, 1, south, west);
        let corners = [
            ((1, 1, -1), [north_east, north, east, up, north_up, east_up]),
            ((1, 1, 1), [south_east, south, east, up, south_up, east_up]),
            ((-1, 1, -1), [north_west, north, west, up, north_up, west_up]),
            ((-1, 1, 1), [south_west, south, west, up, south_up, west_up]),
            ((1, -1, -1), [north_east, north, east, down, north_down, east_down]),
            ((1, -1, 1), [south_east, south, east, down, south_down, east_down]),
            ((-1, -1, -1), [north_west, north, west, down, north_down, west_down]),
            ((-1, -1, 1), [south_west, south, west, down, south_down, west_down]),
        ];
        for ((dx, dy, dz), need) in corners {
            let n = f(self, dx, dy, dz);
            if self.open(n) && need.iter().all(|&k| self.has_m(k)) {
                out.push(n.unwrap_or_default());
            }
        }
    }

    /// Vertical neighbours in the water of the amphibious evaluator.
    pub(super) fn amphibious_neighbors(&mut self, cur: usize, out: &mut Vec<usize>) {
        let c = self.nodes[cur];
        let above = self.mob_type(c.x, c.y + 1, c.z);
        let here = self.mob_type(c.x, c.y, c.z);
        let jump = if self.malus(above) >= 0.0 && here != PathType::StickyHoney {
            self.m.step.max(1.0).floor() as i32
        } else {
            0
        };
        let h = self.floor_level([c.x, c.y, c.z]);
        let up = self.find_accepted(c.x, c.y + 1, c.z, (jump - 1).max(0), h, (0, 0), here);
        let down = self.find_accepted(c.x, c.y - 1, c.z, jump, h, (0, 0), here);
        let valid = |e: &Self, n: Option<usize>| {
            n.is_some_and(|i| {
                let nn = e.nodes[i];
                !nn.closed && (nn.malus >= 0.0 || c.malus < 0.0) && nn.ty == PathType::Water
            })
        };
        if valid(self, up) {
            out.push(up.unwrap_or_default());
        }
        if valid(self, down) && here != PathType::Trapdoor {
            out.push(down.unwrap_or_default());
        }
    }
}

impl Server {
    /// Cuts to the next node in a free straight line: flyers, and
    /// amphibians in a liquid.
    pub(super) fn next_in_direction_direct(&self, m: &Mob, p: &Path, mp: [f64; 3]) -> bool {
        let fluids = if m.nav.fly {
            true
        } else if m.nav.swim || (m.nav.amphibious && (m.in_water || m.lava_height > 0.0)) {
            false
        } else {
            return false;
        };
        if p.next + 1 >= p.nodes.len() {
            return false;
        }
        let c = p.nodes[p.next].0;
        let cur = [f64::from(c[0]) + 0.5, f64::from(c[1]), f64::from(c[2]) + 0.5];
        if super::super::mth::dist2(mp, cur) >= 4.0 {
            return false;
        }
        let t = p.entity_pos_at(m.width(), p.next);
        let to = [t[0], t[1] + f64::from(m.height()) * 0.5, t[2]];
        self.clear_between(mp, to, fluids)
    }

    /// No block (and, if needed, no fluid) on the segment.
    pub(crate) fn clear_between(&self, from: [f64; 3], to: [f64; 3], fluids: bool) -> bool {
        if self.clip_blocks(from, to) {
            return false;
        }
        if fluids {
            let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let n = (len / 0.25).ceil().max(1.0) as i32;
            for i in 0..=n {
                let k = f64::from(i) / f64::from(n);
                let q = [from[0] + d[0] * k, from[1] + d[1] * k, from[2] + d[2] * k];
                if self.mob_fluid(super::super::mth::block_of(q)) != Fluid::None {
                    return false;
                }
            }
        }
        true
    }

    /// The stable destination test according to the mob's navigation.
    pub(crate) fn nav_stable(&self, m: &Mob, p: [i32; 3]) -> bool {
        // F11: bee, block below not empty.
        if let Some(v) = super::super::wild::nav_stable(self, m, p) {
            return v;
        }
        // F12: lava is a stable destination for the strider.
        if super::super::nether::stable_destination(self, m, p) {
            return true;
        }
        // F08/F09: swimmer: everything that is not full when rendered.
        if m.nav.swim {
            return !self.solid_render(p);
        }
        if m.nav.fly {
            return self.data.state(self.st(p)).is_some_and(|i| i.sturdy & 2 != 0);
        }
        if m.nav.amphibious {
            if m.surf.travel.is_some() {
                return self.bname(p) == "minecraft:water";
            }
            return !self.is_air([p[0], p[1] - 1, p[2]]);
        }
        self.stable_destination(p)
    }
}
