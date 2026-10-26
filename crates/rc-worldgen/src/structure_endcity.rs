//! End city (`generation-decoration.md` § 8.5): starting house then recursive section
//! generators (tower, bridge, house, big tower); each subtree receives a drawn label and
//! is kept only if it does not collide with a piece of another label of the current list.

use std::sync::{Arc, Mutex};

use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece};
use crate::structure_coded::CodedPiece;
use crate::structure_place::Proc;
use crate::structure_templ::{TKind, TemplPiece};
use crate::template::{BBox, Mirror, Rot, Settings, transform};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gen {
    House,
    Tower,
    Bridge,
    Fat,
}

struct EPiece {
    name: &'static str,
    pos: [i32; 3],
    rot: Rot,
    overwrite: bool,
    depth: i32,
    bbox: BBox,
    size: [i32; 3],
}

struct City<'a, 'g> {
    g: &'a GenStart<'g>,
    arena: Vec<EPiece>,
    ship: bool,
}

impl City<'_, '_> {
    fn make(&mut self, name: &'static str, pos: [i32; 3], rot: Rot, overwrite: bool) -> usize {
        let t = rc_data::ids::with_key(&["minecraft:end_city/", name], |k| self.g.sg.templates.get(k, self.g.b));
        let size = t.as_ref().map(|t| t.size).unwrap_or([1, 1, 1]);
        let bbox = match &t {
            Some(t) => t.bbox(pos, &Settings { mirror: Mirror::None, rot, pivot: [0, 0, 0] }),
            None => BBox::new(pos, pos),
        };
        self.arena.push(EPiece { name, pos, rot, overwrite, depth: 0, bbox, size });
        self.arena.len() - 1
    }

    /// Piece connected to `parent` (offset in the parent's frame).
    fn add(
        &mut self,
        list: &mut Vec<usize>,
        parent: usize,
        off: [i32; 3],
        name: &'static str,
        rot: Rot,
        ow: bool,
    ) -> usize {
        let pr = self.arena[parent].rot;
        let a = transform(off, Mirror::None, pr, [0, 0, 0]);
        let b = transform([0, 0, 0], Mirror::None, rot, [0, 0, 0]);
        let pp = self.arena[parent].pos;
        let pos = [pp[0] + a[0] - b[0], pp[1] + a[1] - b[1], pp[2] + a[2] - b[2]];
        let i = self.make(name, pos, rot, ow);
        list.push(i);
        i
    }

    fn recurse(
        &mut self,
        sec: Gen,
        depth: i32,
        parent: usize,
        off: [i32; 3],
        list: &mut Vec<usize>,
        r: &mut DecoRng,
    ) -> bool {
        if depth > 8 {
            return false;
        }
        let mut kids: Vec<usize> = Vec::new();
        if self.generate(sec, depth, parent, off, &mut kids, r) {
            let tag = r.next_int();
            let mut collision = false;
            for &k in &kids {
                self.arena[k].depth = tag;
                let bb = self.arena[k].bbox;
                if let Some(&c) = list.iter().find(|&&c| self.arena[c].bbox.intersects(&bb))
                    && self.arena[c].depth != self.arena[parent].depth
                {
                    collision = true;
                    break;
                }
            }
            if !collision {
                list.extend(kids);
                return true;
            }
        }
        false
    }

    fn generate(
        &mut self,
        sec: Gen,
        depth: i32,
        parent: usize,
        off: [i32; 3],
        l: &mut Vec<usize>,
        r: &mut DecoRng,
    ) -> bool {
        let rot = self.arena[parent].rot;
        match sec {
            Gen::House => {
                if depth > 8 {
                    return false;
                }
                let mut last = self.add(l, parent, off, "base_floor", rot, true);
                match r.next_int_bounded(3) {
                    0 => {
                        self.add(l, last, [-1, 4, -1], "base_roof", rot, true);
                    }
                    1 => {
                        last = self.add(l, last, [-1, 0, -1], "second_floor_2", rot, false);
                        last = self.add(l, last, [-1, 8, -1], "second_roof", rot, false);
                        self.recurse(Gen::Tower, depth + 1, last, [0, 0, 0], l, r);
                    }
                    _ => {
                        last = self.add(l, last, [-1, 0, -1], "second_floor_2", rot, false);
                        last = self.add(l, last, [-1, 4, -1], "third_floor_2", rot, false);
                        last = self.add(l, last, [-1, 8, -1], "third_roof", rot, true);
                        self.recurse(Gen::Tower, depth + 1, last, [0, 0, 0], l, r);
                    }
                }
                true
            }
            Gen::Tower => {
                let (a, b) = (r.next_int_bounded(2), r.next_int_bounded(2));
                let mut last = self.add(l, parent, [3 + a, -3, 3 + b], "tower_base", rot, true);
                last = self.add(l, last, [0, 7, 0], "tower_piece", rot, true);
                let mut bridge = if r.next_int_bounded(3) == 0 { Some(last) } else { None };
                let height = 1 + r.next_int_bounded(3);
                for i in 0..height {
                    last = self.add(l, last, [0, 4, 0], "tower_piece", rot, true);
                    if i < height - 1 && r.next_bool() {
                        bridge = Some(last);
                    }
                }
                if let Some(bp) = bridge {
                    for (br, o) in [
                        (Rot::None, [1, -1, 0]),
                        (Rot::Cw90, [6, -1, 1]),
                        (Rot::Ccw90, [0, -1, 5]),
                        (Rot::Cw180, [5, -1, 6]),
                    ] {
                        if r.next_bool() {
                            let s = self.add(l, bp, o, "bridge_end", rot.then(br), true);
                            self.recurse(Gen::Bridge, depth + 1, s, [0, 0, 0], l, r);
                        }
                    }
                    self.add(l, last, [-1, 4, -1], "tower_top", rot, true);
                } else {
                    if depth != 7 {
                        return self.recurse(Gen::Fat, depth + 1, last, [0, 0, 0], l, r);
                    }
                    self.add(l, last, [-1, 4, -1], "tower_top", rot, true);
                }
                true
            }
            Gen::Bridge => {
                let len = r.next_int_bounded(4) + 1;
                let mut last = self.add(l, parent, [0, 0, -4], "bridge_piece", rot, true);
                self.arena[last].depth = -1;
                let mut ny = 0;
                for _ in 0..len {
                    if r.next_bool() {
                        last = self.add(l, last, [0, ny, -4], "bridge_piece", rot, true);
                        ny = 0;
                    } else {
                        last = if r.next_bool() {
                            self.add(l, last, [0, ny, -4], "bridge_steep_stairs", rot, true)
                        } else {
                            self.add(l, last, [0, ny, -8], "bridge_gentle_stairs", rot, true)
                        };
                        ny = 4;
                    }
                }
                if !self.ship && r.next_int_bounded(10 - depth) == 0 {
                    let (sx, sz) = (r.next_int_bounded(8), r.next_int_bounded(10));
                    self.add(l, last, [-8 + sx, ny, -70 + sz], "ship", rot, true);
                    self.ship = true;
                } else if !self.recurse(Gen::House, depth + 1, last, [-3, ny + 1, -11], l, r) {
                    return false;
                }
                let e = self.add(l, last, [4, ny, 0], "bridge_end", rot.then(Rot::Cw180), true);
                self.arena[e].depth = -1;
                true
            }
            Gen::Fat => {
                let mut last = self.add(l, parent, [-3, 4, -3], "fat_tower_base", rot, true);
                last = self.add(l, last, [0, 4, 0], "fat_tower_middle", rot, true);
                let mut i = 0;
                while i < 2 && r.next_int_bounded(3) != 0 {
                    last = self.add(l, last, [0, 8, 0], "fat_tower_middle", rot, true);
                    for (br, o) in [
                        (Rot::None, [4, -1, 0]),
                        (Rot::Cw90, [12, -1, 4]),
                        (Rot::Ccw90, [0, -1, 8]),
                        (Rot::Cw180, [8, -1, 12]),
                    ] {
                        if r.next_bool() {
                            let s = self.add(l, last, o, "bridge_end", rot.then(br), true);
                            self.recurse(Gen::Bridge, depth + 1, s, [0, 0, 0], l, r);
                        }
                    }
                    i += 1;
                }
                self.add(l, last, [-2, 8, -2], "fat_tower_top", rot, true);
                true
            }
        }
    }
}

/// A valid biome cell in the column (world bottom − 1 to top).
fn biome_in_column(g: &GenStart, x: i32, z: i32) -> bool {
    let (y0, y1) = (g.t.world_min_y - 1, g.t.world_min_y + g.t.world_height - 1);
    ((y0 >> 2)..=(y1 >> 2)).any(|qy| g.def.biomes.get(g.biome_at(x, qy << 2, z) as usize).copied().unwrap_or(false))
}

pub fn end_city(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    let (x, z) = (16 * g.cx + 7, 16 * g.cz + 7);
    if !biome_in_column(g, x, z) {
        return None;
    }
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let (ox, oz) = match rot {
        Rot::Cw90 => (-5, 5),
        Rot::Cw180 => (-5, -5),
        Rot::Ccw90 => (5, -5),
        Rot::None => (5, 5),
    };
    let h = |a: i32, b: i32| g.first_free(a, b, false) - 1;
    let y = h(x, z).min(h(x, z + oz)).min(h(x + ox, z).min(h(x + ox, z + oz)));
    if y < 60 {
        return None;
    }
    let start = [x, y, z];
    let mut rng = std::mem::replace(&mut g.rng, DecoRng::legacy(0));
    let order = {
        let mut c = City { g, arena: Vec::new(), ship: false };
        let mut list: Vec<usize> = Vec::new();
        let mut last = c.make("base_floor", start, rot, true);
        list.push(last);
        last = c.add(&mut list, last, [-1, 0, -1], "second_floor_1", rot, false);
        last = c.add(&mut list, last, [-1, 4, -1], "third_floor_1", rot, false);
        last = c.add(&mut list, last, [-1, 8, -1], "third_roof", rot, true);
        c.recurse(Gen::Tower, 1, last, [0, 0, 0], &mut list, &mut rng);
        list.into_iter()
            .map(|i| {
                let e = &c.arena[i];
                (e.name, e.pos, e.rot, e.overwrite, e.bbox, e.size)
            })
            .collect::<Vec<_>>()
    };
    g.rng = rng;
    let b = g.b;
    let pieces = order
        .into_iter()
        .map(|(name, pos, rot, ow, bbox, _)| {
            let mut ign: Vec<u16> = b.block_id("minecraft:structure_block").into_iter().collect();
            if !ow {
                ign.extend(b.block_id("minecraft:air"));
            }
            let tp = TemplPiece {
                location: format!("minecraft:end_city/{name}"),
                pos: Mutex::new(pos),
                mirror: Mirror::None,
                rot,
                pivot: [0, 0, 0],
                procs: Arc::new(vec![Proc::Ignore(ign)]),
                kind: TKind::EndCity,
            };
            Piece::Coded { id: "end_city", bbox, data: Arc::new(CodedPiece::Template(Arc::new(tp))) }
        })
        .collect();
    Some((start, pieces))
}
