//! Swimming path evaluator (F09, `server-mobs.md` § 4.F09): water cells only
//! (integer footprint), surface breach for the dolphin, six faces then four horizontal
//! diagonals; the penalty of a cell without fluid (breach) climbs by 8.

use super::super::env::Fluid;
use super::{Eval, PathType};

impl Eval<'_> {
    /// The path type of a swimming mob (cache by position).
    pub(super) fn swim_type(&mut self, x: i32, y: i32, z: i32) -> PathType {
        if let Some(&t) = self.types.get(&(x, y, z)) {
            return t;
        }
        let s = self.s;
        let mut last = [x, y, z];
        let t = 'scan: {
            for xx in x..x + self.ew {
                for yy in y..y + self.eh {
                    for zz in z..z + self.ew {
                        last = [xx, yy, zz];
                        let fluid = s.mob_fluid(last);
                        let below_water = s.pathfindable_water([xx, yy - 1, zz]);
                        // Breach: air cell without fluid above a swim-traversable cell.
                        if fluid == Fluid::None && below_water && s.is_air(last) {
                            break 'scan PathType::Breach;
                        }
                        if fluid != Fluid::Water {
                            break 'scan PathType::Blocked;
                        }
                    }
                }
            }
            // Last cell walked: swim-traversable.
            if s.pathfindable_water(last) { PathType::Water } else { PathType::Blocked }
        };
        self.types.insert((x, y, z), t);
        t
    }

    /// Low corner of the box, mid-height rounded.
    pub(super) fn swim_start(&mut self) -> usize {
        let b = self.m.bbox;
        let (x, y, z) = (b.min[0].floor() as i32, (b.min[1] + 0.5).floor() as i32, b.min[2].floor() as i32);
        self.node(x, y, z)
    }

    /// Water (or breach allowed), maximum penalty, +8 outside fluid.
    fn swim_accepted(&mut self, x: i32, y: i32, z: i32) -> Option<usize> {
        let t = self.swim_type(x, y, z);
        if !((self.m.breach && t == PathType::Breach) || t == PathType::Water) {
            return None;
        }
        let cost = self.malus(t);
        if cost < 0.0 {
            return None;
        }
        let i = self.node(x, y, z);
        self.nodes[i].ty = t;
        self.nodes[i].malus = self.nodes[i].malus.max(cost);
        if self.s.mob_fluid([x, y, z]) == Fluid::None {
            self.nodes[i].malus += 8.0;
        }
        Some(i)
    }

    /// Neighbours: down, up, north, south, west, east (the game's direction order), then the
    /// horizontal diagonals whose two sides have a positive penalty.
    pub(super) fn swim_neighbors(&mut self, cur: usize, out: &mut Vec<usize>) {
        let c = self.nodes[cur];
        let dirs = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
        let mut found: [Option<usize>; 6] = [None; 6];
        for (k, (dx, dy, dz)) in dirs.iter().enumerate() {
            let n = self.swim_accepted(c.x + dx, c.y + dy, c.z + dz);
            found[k] = n;
            if n.is_some_and(|i| !self.nodes[i].closed) {
                out.push(n.unwrap_or_default());
            }
        }
        // Horizontal plane: north, east, south, west; each with its clockwise neighbour.
        let horiz = [(2usize, (0, -1)), (5, (1, 0)), (3, (0, 1)), (4, (-1, 0))];
        for k in 0..4 {
            let (a, da) = horiz[k];
            let (b, db) = horiz[(k + 1) % 4];
            let has = |n: Option<usize>| n.is_some_and(|i| self.nodes[i].malus >= 0.0);
            if has(found[a]) && has(found[b]) {
                let n = self.swim_accepted(c.x + da.0 + db.0, c.y, c.z + da.1 + db.1);
                if n.is_some_and(|i| !self.nodes[i].closed) {
                    out.push(n.unwrap_or_default());
                }
            }
        }
    }
}
