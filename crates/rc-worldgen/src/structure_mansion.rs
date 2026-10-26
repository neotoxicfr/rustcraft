//! Woodland mansion (`generation-decoration.md` § 8.10): 11 × 11 base grid carved with
//! recursive corridors, rooms identified per floor (1 × 1, 1 × 2, 2 × 2, doors), third floor around
//! a stairs room, then template pieces (entrance, outer walls, roofs, corridors,
//! partitions, rooms). After placement: stone foundations under the pieces.

use std::sync::{Arc, Mutex};

use crate::decor_blocks::Dir;
use crate::decor_region::Region;
use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece};
use crate::structure_coded::CodedPiece;
use crate::structure_place::Proc;
use crate::structure_templ::{TKind, TemplPiece};
use crate::template::{BBox, Mirror, Rot, Settings, zero_position};

const CORRIDOR: i32 = 1;
const ROOM: i32 = 2;
const START: i32 = 3;
const BLOCKED: i32 = 5;
const T1X1: i32 = 65536;
const T1X2: i32 = 131072;
const T2X2: i32 = 262144;
const ORIGIN: i32 = 1048576;
const DOOR: i32 = 2097152;
const STAIRS: i32 = 4194304;
const CORR_FLAG: i32 = 8388608;
const TYPE_MASK: i32 = 983040;
const ID_MASK: i32 = 65535;

/// The game's horizontal order (north, east, south, west).
const PLANE: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

fn step(d: Dir) -> (i32, i32) {
    let o = d.offset();
    (o[0], o[2])
}

/// Direction of the game's 2D value (south, west, north, east).
fn from_2d(i: i32) -> Dir {
    [Dir::South, Dir::West, Dir::North, Dir::East][i as usize & 3]
}

#[derive(Clone)]
struct Grid {
    w: i32,
    h: i32,
    out: i32,
    g: Vec<i32>,
}

impl Grid {
    fn new(w: i32, h: i32, out: i32) -> Grid {
        Grid { w, h, out, g: vec![0; (w * h) as usize] }
    }
    fn set(&mut self, x: i32, y: i32, v: i32) {
        if x >= 0 && x < self.w && y >= 0 && y < self.h {
            self.g[(x * self.h + y) as usize] = v;
        }
    }
    fn set_box(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, v: i32) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.set(x, y, v);
            }
        }
    }
    fn get(&self, x: i32, y: i32) -> i32 {
        if x >= 0 && x < self.w && y >= 0 && y < self.h { self.g[(x * self.h + y) as usize] } else { self.out }
    }
    fn set_if(&mut self, x: i32, y: i32, if_v: i32, v: i32) {
        if self.get(x, y) == if_v {
            self.set(x, y, v);
        }
    }
    fn edges_to(&self, x: i32, y: i32, v: i32) -> bool {
        self.get(x - 1, y) == v || self.get(x + 1, y) == v || self.get(x, y + 1) == v || self.get(x, y - 1) == v
    }
}

fn is_house(g: &Grid, x: i32, y: i32) -> bool {
    matches!(g.get(x, y), 1..=4)
}

struct Plan {
    base: Grid,
    third: Grid,
    rooms: [Grid; 3],
    ex: i32,
    ey: i32,
}

impl Plan {
    fn is_room_id(&self, x: i32, y: i32, floor: usize, id: i32) -> bool {
        (self.rooms[floor].get(x, y) & ID_MASK) == id
    }

    fn room_dir_1x2(&self, x: i32, y: i32, floor: usize, id: i32) -> Option<Dir> {
        PLANE.into_iter().find(|&d| {
            let (sx, sz) = step(d);
            self.is_room_id(x + sx, y + sz, floor, id)
        })
    }
}

fn corridor(g: &mut Grid, r: &mut DecoRng, x: i32, y: i32, heading: Dir, depth: i32) {
    if depth <= 0 {
        return;
    }
    let (hx, hz) = step(heading);
    g.set(x, y, CORRIDOR);
    g.set_if(x + hx, y + hz, 0, CORRIDOR);
    for _ in 0..8 {
        let nd = from_2d(r.next_int_bounded(4));
        if nd != heading.opposite() && (nd != Dir::East || !r.next_bool()) {
            let (nx, ny) = (x + hx, y + hz);
            let (dx, dz) = step(nd);
            if g.get(nx + dx, ny + dz) == 0 && g.get(nx + dx * 2, ny + dz * 2) == 0 {
                corridor(g, r, x + hx + dx, y + hz + dz, nd, depth - 1);
                break;
            }
        }
    }
    let (cx, cz) = step(heading.cw());
    let (ax, az) = step(heading.ccw());
    g.set_if(x + cx, y + cz, 0, ROOM);
    g.set_if(x + ax, y + az, 0, ROOM);
    g.set_if(x + hx + cx, y + hz + cz, 0, ROOM);
    g.set_if(x + hx + ax, y + hz + az, 0, ROOM);
    g.set_if(x + hx * 2, y + hz * 2, 0, ROOM);
    g.set_if(x + cx * 2, y + cz * 2, 0, ROOM);
    g.set_if(x + ax * 2, y + az * 2, 0, ROOM);
}

fn clean_edges(g: &mut Grid) -> bool {
    let mut touched = false;
    for y in 0..g.h {
        for x in 0..g.w {
            if g.get(x, y) != 0 {
                continue;
            }
            let h = |a: i32, b: i32| i32::from(is_house(g, a, b));
            let direct = h(x + 1, y) + h(x - 1, y) + h(x, y + 1) + h(x, y - 1);
            if direct >= 3 {
                g.set(x, y, ROOM);
                touched = true;
            } else if direct == 2 {
                let diag = h(x + 1, y + 1) + h(x - 1, y + 1) + h(x + 1, y - 1) + h(x - 1, y - 1);
                if diag <= 1 {
                    g.set(x, y, ROOM);
                    touched = true;
                }
            }
        }
    }
    touched
}

fn identify_rooms(from: &Grid, rooms: &mut Grid, r: &mut DecoRng) {
    let mut pos: Vec<(i32, i32)> = Vec::new();
    for y in 0..from.h {
        for x in 0..from.w {
            if from.get(x, y) == ROOM {
                pos.push((x, y));
            }
        }
    }
    r.shuffle(&mut pos);
    let mut id = 10;
    for (x, y) in pos {
        if rooms.get(x, y) != 0 {
            continue;
        }
        let (mut x0, mut x1, mut y0, mut y1) = (x, x, y, y);
        let free = |rooms: &Grid, a: i32, b: i32| rooms.get(a, b) == 0 && from.get(a, b) == ROOM;
        let mut ty = T1X1;
        if free(rooms, x + 1, y) && free(rooms, x, y + 1) && free(rooms, x + 1, y + 1) {
            x1 += 1;
            y1 += 1;
            ty = T2X2;
        } else if free(rooms, x - 1, y) && free(rooms, x, y + 1) && free(rooms, x - 1, y + 1) {
            x0 -= 1;
            y1 += 1;
            ty = T2X2;
        } else if free(rooms, x - 1, y) && free(rooms, x, y - 1) && free(rooms, x - 1, y - 1) {
            x0 -= 1;
            y0 -= 1;
            ty = T2X2;
        } else if free(rooms, x + 1, y) {
            x1 += 1;
            ty = T1X2;
        } else if free(rooms, x, y + 1) {
            y1 += 1;
            ty = T1X2;
        } else if free(rooms, x - 1, y) {
            x0 -= 1;
            ty = T1X2;
        } else if free(rooms, x, y - 1) {
            y0 -= 1;
            ty = T1X2;
        }
        let mut dx = if r.next_bool() { x0 } else { x1 };
        let mut dy = if r.next_bool() { y0 } else { y1 };
        let mut flag = DOOR;
        if !from.edges_to(dx, dy, CORRIDOR) {
            dx = if dx == x0 { x1 } else { x0 };
            dy = if dy == y0 { y1 } else { y0 };
            if !from.edges_to(dx, dy, CORRIDOR) {
                dy = if dy == y0 { y1 } else { y0 };
                if !from.edges_to(dx, dy, CORRIDOR) {
                    dx = if dx == x0 { x1 } else { x0 };
                    dy = if dy == y0 { y1 } else { y0 };
                    if !from.edges_to(dx, dy, CORRIDOR) {
                        flag = 0;
                        dx = x0;
                        dy = y0;
                    }
                }
            }
        }
        for ry in y0..=y1 {
            for rx in x0..=x1 {
                let v = if rx == dx && ry == dy { ORIGIN | flag | ty | id } else { ty | id };
                rooms.set(rx, ry, v);
            }
        }
        id += 1;
    }
}

fn plan(r: &mut DecoRng) -> Plan {
    let (ex, ey) = (7, 4);
    let mut base = Grid::new(11, 11, BLOCKED);
    base.set_box(ex, ey, ex + 1, ey + 1, START);
    base.set_box(ex - 1, ey, ex - 1, ey + 1, ROOM);
    base.set_box(ex + 2, ey - 2, ex + 3, ey + 3, BLOCKED);
    base.set_box(ex + 1, ey - 2, ex + 1, ey - 1, CORRIDOR);
    base.set_box(ex + 1, ey + 2, ex + 1, ey + 3, CORRIDOR);
    base.set(ex - 1, ey - 1, CORRIDOR);
    base.set(ex - 1, ey + 2, CORRIDOR);
    base.set_box(0, 0, 11, 1, BLOCKED);
    base.set_box(0, 9, 11, 11, BLOCKED);
    corridor(&mut base, r, ex, ey - 2, Dir::West, 6);
    corridor(&mut base, r, ex, ey + 3, Dir::West, 6);
    corridor(&mut base, r, ex - 2, ey - 1, Dir::West, 3);
    corridor(&mut base, r, ex - 2, ey + 2, Dir::West, 3);
    while clean_edges(&mut base) {}
    let mut rooms = [Grid::new(11, 11, BLOCKED), Grid::new(11, 11, BLOCKED), Grid::new(11, 11, BLOCKED)];
    identify_rooms(&base, &mut rooms[0], r);
    identify_rooms(&base, &mut rooms[1], r);
    rooms[0].set_box(ex + 1, ey, ex + 1, ey + 1, CORR_FLAG);
    rooms[1].set_box(ex + 1, ey, ex + 1, ey + 1, CORR_FLAG);
    let mut p = Plan { third: Grid::new(base.w, base.h, BLOCKED), base, rooms, ex, ey };
    third_floor(&mut p, r);
    let third = p.third.clone();
    identify_rooms(&third, &mut p.rooms[2], r);
    p
}

fn third_floor(p: &mut Plan, r: &mut DecoRng) {
    let mut cands: Vec<(i32, i32)> = Vec::new();
    for y in 0..p.third.h {
        for x in 0..p.third.w {
            let d = p.rooms[1].get(x, y);
            if d & TYPE_MASK == T1X2 && d & DOOR == DOOR {
                cands.push((x, y));
            }
        }
    }
    let (w, h) = (p.third.w, p.third.h);
    if cands.is_empty() {
        p.third.set_box(0, 0, w, h, BLOCKED);
        return;
    }
    let (rx, ry) = cands[r.next_int_bounded(cands.len() as i32) as usize];
    let data = p.rooms[1].get(rx, ry);
    p.rooms[1].set(rx, ry, data | STAIRS);
    let Some(rd) = p.room_dir_1x2(rx, ry, 1, data & ID_MASK) else { return };
    let (sx, sz) = step(rd);
    let (ex, ey) = (rx + sx, ry + sz);
    for y in 0..h {
        for x in 0..w {
            if !is_house(&p.base, x, y) {
                p.third.set(x, y, BLOCKED);
            } else if x == rx && y == ry {
                p.third.set(x, y, START);
            } else if x == ex && y == ey {
                p.third.set(x, y, START);
                p.rooms[2].set(x, y, CORR_FLAG);
            }
        }
    }
    let dirs: Vec<Dir> = PLANE
        .into_iter()
        .filter(|&d| {
            let (dx, dz) = step(d);
            p.third.get(ex + dx, ey + dz) == 0
        })
        .collect();
    if dirs.is_empty() {
        p.third.set_box(0, 0, w, h, BLOCKED);
        p.rooms[1].set(rx, ry, data);
    } else {
        let d = dirs[r.next_int_bounded(dirs.len() as i32) as usize];
        let (dx, dz) = step(d);
        corridor(&mut p.third, r, ex + dx, ey + dz, d, 4);
        while clean_edges(&mut p.third) {}
    }
}

// ---------------------------------------------------------------- Pieces

struct MPiece {
    name: String,
    pos: [i32; 3],
    rot: Rot,
    mirror: Mirror,
}

fn rel(p: [i32; 3], d: Dir, n: i32) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0] * n, p[1] + o[1] * n, p[2] + o[2] * n]
}

fn up(p: [i32; 3], n: i32) -> [i32; 3] {
    [p[0], p[1] + n, p[2]]
}

/// Rotated position.
fn rotate_pos(p: [i32; 3], r: Rot) -> [i32; 3] {
    match r {
        Rot::None => p,
        Rot::Cw90 => [-p[2], p[1], p[0]],
        Rot::Cw180 => [-p[0], p[1], -p[2]],
        Rot::Ccw90 => [p[2], p[1], -p[0]],
    }
}

struct Data {
    rot: Rot,
    pos: [i32; 3],
    wall: &'static str,
}

struct Placer<'r> {
    r: &'r mut DecoRng,
    out: Vec<MPiece>,
    sx: i32,
    sy: i32,
}

impl Placer<'_> {
    fn add(&mut self, name: impl Into<String>, pos: [i32; 3], rot: Rot) {
        self.add_m(name, pos, rot, Mirror::None);
    }
    fn add_m(&mut self, name: impl Into<String>, pos: [i32; 3], rot: Rot, mirror: Mirror) {
        self.out.push(MPiece { name: name.into(), pos, rot, mirror });
    }

    fn wall_piece(&mut self, d: &mut Data) {
        let p = rel(d.pos, d.rot.dir(Dir::East), 7);
        self.add(d.wall, p, d.rot);
        d.pos = rel(d.pos, d.rot.dir(Dir::South), 8);
    }

    fn turn(&mut self, d: &mut Data) {
        d.pos = rel(d.pos, d.rot.dir(Dir::South), -1);
        self.add("wall_corner", d.pos, d.rot);
        d.pos = rel(d.pos, d.rot.dir(Dir::South), -7);
        d.pos = rel(d.pos, d.rot.dir(Dir::West), -6);
        d.rot = d.rot.then(Rot::Cw90);
    }

    fn inner_turn(d: &mut Data) {
        d.pos = rel(d.pos, d.rot.dir(Dir::South), 6);
        d.pos = rel(d.pos, d.rot.dir(Dir::East), 8);
        d.rot = d.rot.then(Rot::Ccw90);
    }

    #[allow(clippy::too_many_arguments)]
    fn outer_walls(&mut self, d: &mut Data, g: &Grid, dir: Dir, sx: i32, sy: i32, ex: i32, ey: i32) {
        let (mut gx, mut gy, mut gd) = (sx, sy, dir);
        let start = dir;
        loop {
            let (dx, dz) = step(gd);
            let (cx, cz) = step(gd.ccw());
            if !is_house(g, gx + dx, gy + dz) {
                self.turn(d);
                gd = gd.cw();
                if gx != ex || gy != ey || start != gd {
                    self.wall_piece(d);
                }
            } else if is_house(g, gx + dx + cx, gy + dz + cz) {
                Self::inner_turn(d);
                gx += dx;
                gy += dz;
                gd = gd.ccw();
            } else {
                gx += dx;
                gy += dz;
                if gx != ex || gy != ey || start != gd {
                    self.wall_piece(d);
                }
            }
            if gx == ex && gy == ey && start == gd {
                break;
            }
        }
    }

    fn cell(&self, origin: [i32; 3], rot: Rot, x: i32, y: i32) -> [i32; 3] {
        let p = rel(origin, rot.dir(Dir::South), 8 + (y - self.sy) * 8);
        rel(p, rot.dir(Dir::East), (x - self.sx) * 8)
    }

    fn roof(&mut self, origin: [i32; 3], rot: Rot, g: &Grid, above: Option<&Grid>) {
        let e = rot.dir(Dir::East);
        let s = rot.dir(Dir::South);
        let w = rot.dir(Dir::West);
        let n = rot.dir(Dir::North);
        for y in 0..g.h {
            for x in 0..g.w {
                let p = self.cell(origin, rot, x, y);
                let is_above = above.is_some_and(|a| is_house(a, x, y));
                if is_house(g, x, y) && !is_above {
                    self.add("roof", up(p, 3), rot);
                    if !is_house(g, x + 1, y) {
                        self.add("roof_front", rel(p, e, 6), rot);
                    }
                    if !is_house(g, x - 1, y) {
                        self.add("roof_front", rel(rel(p, e, 0), s, 7), rot.then(Rot::Cw180));
                    }
                    if !is_house(g, x, y - 1) {
                        self.add("roof_front", rel(p, w, 1), rot.then(Rot::Ccw90));
                    }
                    if !is_house(g, x, y + 1) {
                        self.add("roof_front", rel(rel(p, e, 6), s, 6), rot.then(Rot::Cw90));
                    }
                }
            }
        }
        if let Some(a) = above {
            for y in 0..g.h {
                for x in 0..g.w {
                    let p = self.cell(origin, rot, x, y);
                    if !(is_house(g, x, y) && is_house(a, x, y)) {
                        continue;
                    }
                    if !is_house(g, x + 1, y) {
                        self.add("small_wall", rel(p, e, 7), rot);
                    }
                    if !is_house(g, x - 1, y) {
                        self.add("small_wall", rel(rel(p, w, 1), s, 6), rot.then(Rot::Cw180));
                    }
                    if !is_house(g, x, y - 1) {
                        self.add("small_wall", rel(rel(p, w, 0), n, 1), rot.then(Rot::Ccw90));
                    }
                    if !is_house(g, x, y + 1) {
                        self.add("small_wall", rel(rel(p, e, 6), s, 7), rot.then(Rot::Cw90));
                    }
                    if !is_house(g, x + 1, y) {
                        if !is_house(g, x, y - 1) {
                            self.add("small_wall_corner", rel(rel(p, e, 7), n, 2), rot);
                        }
                        if !is_house(g, x, y + 1) {
                            self.add("small_wall_corner", rel(rel(p, e, 8), s, 7), rot.then(Rot::Cw90));
                        }
                    }
                    if !is_house(g, x - 1, y) {
                        if !is_house(g, x, y - 1) {
                            self.add("small_wall_corner", rel(rel(p, w, 2), n, 1), rot.then(Rot::Ccw90));
                        }
                        if !is_house(g, x, y + 1) {
                            self.add("small_wall_corner", rel(rel(p, w, 1), s, 8), rot.then(Rot::Cw180));
                        }
                    }
                }
            }
        }
        for y in 0..g.h {
            for x in 0..g.w {
                let p = self.cell(origin, rot, x, y);
                let is_above = above.is_some_and(|a| is_house(a, x, y));
                if !(is_house(g, x, y) && !is_above) {
                    continue;
                }
                if !is_house(g, x + 1, y) {
                    let p2 = rel(p, e, 6);
                    if !is_house(g, x, y + 1) {
                        self.add("roof_corner", rel(p2, s, 6), rot);
                    } else if is_house(g, x + 1, y + 1) {
                        self.add("roof_inner_corner", rel(p2, s, 5), rot);
                    }
                    if !is_house(g, x, y - 1) {
                        self.add("roof_corner", p2, rot.then(Rot::Ccw90));
                    } else if is_house(g, x + 1, y - 1) {
                        self.add("roof_inner_corner", rel(rel(p, e, 9), n, 2), rot.then(Rot::Cw90));
                    }
                }
                if !is_house(g, x - 1, y) {
                    let p2 = rel(rel(p, e, 0), s, 0);
                    if !is_house(g, x, y + 1) {
                        self.add("roof_corner", rel(p2, s, 6), rot.then(Rot::Cw90));
                    } else if is_house(g, x - 1, y + 1) {
                        self.add("roof_inner_corner", rel(rel(p2, s, 8), w, 3), rot.then(Rot::Ccw90));
                    }
                    if !is_house(g, x, y - 1) {
                        self.add("roof_corner", p2, rot.then(Rot::Cw180));
                    } else if is_house(g, x - 1, y - 1) {
                        self.add("roof_inner_corner", rel(p2, s, 1), rot.then(Rot::Cw180));
                    }
                }
            }
        }
    }

    fn room_1x1(&mut self, room: [i32; 3], rot: Rot, door: Option<Dir>, floor: usize) {
        let mut pr = Rot::None;
        let mut name = format!("1x1_{}{}", if floor == 0 { "a" } else { "b" }, self.r.next_int_bounded(5) + 1);
        if door != Some(Dir::East) {
            match door {
                Some(Dir::North) => pr = pr.then(Rot::Ccw90),
                Some(Dir::West) => pr = pr.then(Rot::Cw180),
                Some(Dir::South) => pr = pr.then(Rot::Cw90),
                _ => name = format!("1x1_as{}", self.r.next_int_bounded(4) + 1),
            }
        }
        let o = zero_position([1, 0, 0], [7, 1, 7], Mirror::None, pr);
        let pr = pr.then(rot);
        let o = rotate_pos(o, rot);
        self.add(name, [room[0] + o[0], room[1], room[2] + o[2]], pr);
    }

    fn side_1x2(&mut self, floor: usize, stairs: bool) -> String {
        if floor == 0 {
            format!("1x2_a{}", self.r.next_int_bounded(9) + 1)
        } else if stairs {
            "1x2_c_stairs".into()
        } else {
            format!("1x2_c{}", self.r.next_int_bounded(4) + 1)
        }
    }

    fn front_1x2(&mut self, floor: usize, stairs: bool) -> String {
        if floor == 0 {
            format!("1x2_b{}", self.r.next_int_bounded(5) + 1)
        } else if stairs {
            "1x2_d_stairs".into()
        } else {
            format!("1x2_d{}", self.r.next_int_bounded(5) + 1)
        }
    }

    fn secret_1x2(&mut self, floor: usize) -> String {
        if floor == 0 {
            format!("1x2_s{}", self.r.next_int_bounded(2) + 1)
        } else {
            format!("1x2_se{}", self.r.next_int_bounded(1) + 1)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn room_1x2(&mut self, room: [i32; 3], rot: Rot, rd: Option<Dir>, door: Dir, floor: usize, stairs: bool) {
        let e = rot.dir(Dir::East);
        let s = rot.dir(Dir::South);
        let w = rot.dir(Dir::West);
        let n = rot.dir(Dir::North);
        use Dir::*;
        match (door, rd) {
            (East, Some(South)) => {
                let name = self.side_1x2(floor, stairs);
                self.add(name, rel(room, e, 1), rot);
            }
            (East, Some(North)) => {
                let name = self.side_1x2(floor, stairs);
                self.add_m(name, rel(rel(room, e, 1), s, 6), rot, Mirror::LeftRight);
            }
            (West, Some(North)) => {
                let name = self.side_1x2(floor, stairs);
                self.add(name, rel(rel(room, e, 7), s, 6), rot.then(Rot::Cw180));
            }
            (West, Some(South)) => {
                let name = self.side_1x2(floor, stairs);
                self.add_m(name, rel(room, e, 7), rot, Mirror::FrontBack);
            }
            (South, Some(East)) => {
                let name = self.side_1x2(floor, stairs);
                self.add_m(name, rel(room, e, 1), rot.then(Rot::Cw90), Mirror::LeftRight);
            }
            (South, Some(West)) => {
                let name = self.side_1x2(floor, stairs);
                self.add(name, rel(room, e, 7), rot.then(Rot::Cw90));
            }
            (North, Some(West)) => {
                let name = self.side_1x2(floor, stairs);
                self.add_m(name, rel(rel(room, e, 7), s, 6), rot.then(Rot::Cw90), Mirror::FrontBack);
            }
            (North, Some(East)) => {
                let name = self.side_1x2(floor, stairs);
                self.add(name, rel(rel(room, e, 1), s, 6), rot.then(Rot::Ccw90));
            }
            (South, Some(North)) => {
                let name = self.front_1x2(floor, stairs);
                self.add(name, rel(rel(room, e, 1), n, 8), rot);
            }
            (North, Some(South)) => {
                let name = self.front_1x2(floor, stairs);
                self.add(name, rel(rel(room, e, 7), s, 14), rot.then(Rot::Cw180));
            }
            (West, Some(East)) => {
                let name = self.front_1x2(floor, stairs);
                self.add(name, rel(room, e, 15), rot.then(Rot::Cw90));
            }
            (East, Some(West)) => {
                let name = self.front_1x2(floor, stairs);
                self.add(name, rel(rel(room, w, 7), s, 6), rot.then(Rot::Ccw90));
            }
            (Up, Some(East)) => {
                let name = self.secret_1x2(floor);
                self.add(name, rel(room, e, 15), rot.then(Rot::Cw90));
            }
            (Up, Some(South)) => {
                let name = self.secret_1x2(floor);
                self.add(name, rel(rel(room, e, 1), n, 0), rot);
            }
            _ => {}
        }
    }

    fn room_2x2(&mut self, room: [i32; 3], rot: Rot, rd: Dir, door: Dir, floor: usize) {
        use Dir::*;
        let (mut east, mut south, mut r2, mut m) = (0, 0, rot, Mirror::None);
        match (door, rd) {
            (East, South) => east = -7,
            (East, North) => {
                east = -7;
                south = 6;
                m = Mirror::LeftRight;
            }
            (North, East) => {
                east = 1;
                south = 14;
                r2 = rot.then(Rot::Ccw90);
            }
            (North, West) => {
                east = 7;
                south = 14;
                r2 = rot.then(Rot::Ccw90);
                m = Mirror::LeftRight;
            }
            (South, West) => {
                east = 7;
                south = -8;
                r2 = rot.then(Rot::Cw90);
            }
            (South, East) => {
                east = 1;
                south = -8;
                r2 = rot.then(Rot::Cw90);
                m = Mirror::LeftRight;
            }
            (West, North) => {
                east = 15;
                south = 6;
                r2 = rot.then(Rot::Cw180);
            }
            (West, South) => {
                east = 15;
                m = Mirror::FrontBack;
            }
            _ => {}
        }
        let p = rel(rel(room, rot.dir(Dir::East), east), rot.dir(Dir::South), south);
        let name = if floor == 0 {
            format!("2x2_a{}", self.r.next_int_bounded(4) + 1)
        } else {
            format!("2x2_b{}", self.r.next_int_bounded(5) + 1)
        };
        self.add_m(name, p, r2, m);
    }

    fn build(&mut self, origin: [i32; 3], rot: Rot, p: &Plan) {
        let mut data = Data { rot, pos: origin, wall: "wall_flat" };
        let west = rot.dir(Dir::West);
        self.add("entrance", rel(data.pos, west, 9), rot);
        data.pos = rel(data.pos, rot.dir(Dir::South), 16);
        let mut second = Data { rot, pos: up(data.pos, 8), wall: "wall_window" };
        self.sx = p.ex + 1;
        self.sy = p.ey + 1;
        let (ex, ey) = (p.ex + 1, p.ey);
        self.outer_walls(&mut data, &p.base, Dir::South, self.sx, self.sy, ex, ey);
        self.outer_walls(&mut second, &p.base, Dir::South, self.sx, self.sy, ex, ey);
        let mut third = Data { rot, pos: up(data.pos, 19), wall: "wall_window" };
        'outer: for y in 0..p.third.h {
            for x in (0..p.third.w).rev() {
                if is_house(&p.third, x, y) {
                    third.pos = rel(third.pos, rot.dir(Dir::South), 8 + (y - self.sy) * 8);
                    third.pos = rel(third.pos, rot.dir(Dir::East), (x - self.sx) * 8);
                    self.wall_piece(&mut third);
                    self.outer_walls(&mut third, &p.third, Dir::South, x, y, x, y);
                    break 'outer;
                }
            }
        }
        self.roof(up(origin, 16), rot, &p.base, Some(&p.third));
        self.roof(up(origin, 27), rot, &p.third, None);
        for floor in 0..3usize {
            let fo = up(origin, 8 * floor as i32 + if floor == 2 { 3 } else { 0 });
            let rooms = &p.rooms[floor];
            let g = if floor == 2 { &p.third } else { &p.base };
            let south_piece = if floor == 0 { "carpet_south_1" } else { "carpet_south_2" };
            let west_piece = if floor == 0 { "carpet_west_1" } else { "carpet_west_2" };
            let e = rot.dir(Dir::East);
            let s = rot.dir(Dir::South);
            let w = rot.dir(Dir::West);
            let n = rot.dir(Dir::North);
            let corr = |a: i32, b: i32| g.get(a, b) == CORRIDOR || (rooms.get(a, b) & CORR_FLAG) == CORR_FLAG;
            for y in 0..g.h {
                for x in 0..g.w {
                    if g.get(x, y) != CORRIDOR {
                        continue;
                    }
                    let pos = self.cell(fo, rot, x, y);
                    self.add("corridor_floor", pos, rot);
                    if corr(x, y - 1) {
                        self.add("carpet_north", up(rel(pos, e, 1), 1), rot);
                    }
                    if corr(x + 1, y) {
                        self.add("carpet_east", up(rel(rel(pos, s, 1), e, 5), 1), rot);
                    }
                    if corr(x, y + 1) {
                        self.add(south_piece, rel(rel(pos, s, 5), w, 1), rot);
                    }
                    if corr(x - 1, y) {
                        self.add(west_piece, rel(rel(pos, w, 1), n, 1), rot);
                    }
                }
            }
            let wall_piece = if floor == 0 { "indoors_wall_1" } else { "indoors_wall_2" };
            let door_piece = if floor == 0 { "indoors_door_1" } else { "indoors_door_2" };
            for y in 0..g.h {
                for x in 0..g.w {
                    let mut third_start = floor == 2 && g.get(x, y) == START;
                    if !(g.get(x, y) == ROOM || third_start) {
                        continue;
                    }
                    let data = rooms.get(x, y);
                    let ty = data & TYPE_MASK;
                    let id = data & ID_MASK;
                    third_start = third_start && (data & CORR_FLAG) == CORR_FLAG;
                    let mut doors: Vec<Dir> = Vec::new();
                    if data & DOOR == DOOR {
                        for d in PLANE {
                            let (dx, dz) = step(d);
                            if g.get(x + dx, y + dz) == CORRIDOR {
                                doors.push(d);
                            }
                        }
                    }
                    let door = if !doors.is_empty() {
                        Some(doors[self.r.next_int_bounded(doors.len() as i32) as usize])
                    } else if data & ORIGIN == ORIGIN {
                        Some(Dir::Up)
                    } else {
                        None
                    };
                    let room = rel(rel(fo, s, 8 + (y - self.sy) * 8), e, -1 + (x - self.sx) * 8);
                    let pick = |d: Dir| if door == Some(d) { door_piece } else { wall_piece };
                    if is_house(g, x - 1, y) && !p.is_room_id(x - 1, y, floor, id) {
                        self.add(pick(Dir::West), room, rot);
                    }
                    if g.get(x + 1, y) == CORRIDOR && !third_start {
                        self.add(pick(Dir::East), rel(room, e, 8), rot);
                    }
                    if is_house(g, x, y + 1) && !p.is_room_id(x, y + 1, floor, id) {
                        self.add(pick(Dir::South), rel(rel(room, s, 7), e, 7), rot.then(Rot::Cw90));
                    }
                    if g.get(x, y - 1) == CORRIDOR && !third_start {
                        self.add(pick(Dir::North), rel(rel(room, n, 1), e, 7), rot.then(Rot::Cw90));
                    }
                    if ty == T1X1 {
                        self.room_1x1(room, rot, door, floor);
                    } else if ty == T1X2
                        && let Some(d) = door
                    {
                        let rd = p.room_dir_1x2(x, y, floor, id);
                        self.room_1x2(room, rot, rd, d, floor, data & STAIRS == STAIRS);
                    } else if ty == T2X2 && door.is_some() && door != Some(Dir::Up) {
                        let d = door.unwrap_or(Dir::North);
                        let mut rd = d.cw();
                        let (dx, dz) = step(rd);
                        if !p.is_room_id(x + dx, y + dz, floor, id) {
                            rd = rd.opposite();
                        }
                        self.room_2x2(room, rot, rd, d, floor);
                    } else if ty == T2X2 && door == Some(Dir::Up) {
                        self.add("2x2_s1", rel(room, e, 1), rot);
                    }
                }
            }
        }
    }
}

/// A valid biome cell in the column (bottom − 1 to world top).
fn biome_in_column(g: &GenStart, x: i32, z: i32) -> bool {
    let (y0, y1) = (g.t.world_min_y - 1, g.t.world_min_y + g.t.world_height - 1);
    ((y0 >> 2)..=(y1 >> 2)).any(|qy| g.def.biomes.get(g.biome_at(x, qy << 2, z) as usize).copied().unwrap_or(false))
}

pub fn mansion(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
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
    let p = plan(&mut g.rng);
    let mut pl = Placer { r: &mut g.rng, out: Vec::new(), sx: 0, sy: 0 };
    pl.build(start, rot, &p);
    let out = std::mem::take(&mut pl.out);
    let b = g.b;
    let ign: Vec<u16> = b.block_id("minecraft:structure_block").into_iter().collect();
    let mut pieces = Vec::with_capacity(out.len());
    for mp in out {
        let loc = format!("minecraft:woodland_mansion/{}", mp.name);
        let Some(t) = g.sg.templates.get(&loc, b) else { continue };
        let bbox = t.bbox(mp.pos, &Settings { mirror: mp.mirror, rot: mp.rot, pivot: [0, 0, 0] });
        let tp = TemplPiece {
            location: loc,
            pos: Mutex::new(mp.pos),
            mirror: mp.mirror,
            rot: mp.rot,
            pivot: [0, 0, 0],
            procs: Arc::new(vec![Proc::Ignore(ign.clone())]),
            kind: TKind::Mansion,
        };
        pieces.push(Piece::Coded { id: "mansion", bbox, data: Arc::new(CodedPiece::Template(Arc::new(tp))) });
    }
    Some((start, pieces))
}

/// Foundations after placement in a chunk: stone under the pieces down to the ground.
pub fn after(r: &mut Region, pieces: &[Piece], zone: BBox) {
    let Some(first) = pieces.first() else { return };
    let mut all = first.bbox();
    for p in &pieces[1..] {
        all.encapsulate(&p.bbox());
    }
    let b = r.b;
    let Some(cobble) = b.block_id("cobblestone").map(|i| b.default_state(i)) else { return };
    let y0 = all.min[1];
    for x in zone.min[0]..=zone.max[0] {
        for z in zone.min[2]..=zone.max[2] {
            let p = [x, y0, z];
            if b.is_air(r.get_p(p)) || !all.contains(p) || !pieces.iter().any(|pc| pc.bbox().contains(p)) {
                continue;
            }
            let mut y = y0 - 1;
            while y > r.min_y {
                let s = r.get(x, y, z);
                if !b.is_air(s) && !b.has_flag(s, rc_data::flags::LIQUID) {
                    break;
                }
                r.set(x, y, z, cobble, 2);
                y -= 1;
            }
        }
    }
}
