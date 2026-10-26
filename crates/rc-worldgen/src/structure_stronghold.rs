//! Stronghold (`generation-decoration.md` § 8.8): assembly by weighted pieces from the starting stairs
//! (queue of children drawn at random), restarted as long as the portal room is missing;
//! placement of the pieces (brick selector, doors, portal room).

use std::sync::{Arc, Mutex};

use crate::decor::Ctx;
use crate::decor_blocks::{Dir, State};
use crate::decor_region::BeData;
use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece, StructureGen};
use crate::structure_coded::CodedPiece;
use crate::structure_piece::P;
use crate::template::BBox;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Opening,
    Wood,
    Grates,
    Iron,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShKind {
    Straight { left: bool, right: bool },
    PrisonHall,
    LeftTurn,
    RightTurn,
    RoomCrossing(i32),
    StraightStairsDown,
    StairsDown { source: bool },
    FiveCrossing { left_low: bool, left_high: bool, right_low: bool, right_high: bool },
    ChestCorridor,
    Library { tall: bool },
    PortalRoom,
    Filler { steps: i32 },
}

#[derive(Debug)]
pub struct ShPiece {
    pub kind: ShKind,
    pub orient: Dir,
    pub door: Door,
    pub depth: i32,
    /// Chest or spawner already placed (chest corridor, portal room).
    pub placed: Mutex<bool>,
}

/// Weighted pieces (game order): (weight, maximum).
const WEIGHTS: [(i32, i32); 11] =
    [(40, 0), (5, 5), (20, 0), (20, 0), (10, 6), (5, 5), (5, 5), (5, 4), (5, 4), (10, 2), (20, 1)];

pub(crate) fn orient_box(f: [i32; 3], off: [i32; 3], w: i32, h: i32, d: i32, dir: Dir) -> BBox {
    let (x, y, z) = (f[0], f[1], f[2]);
    let (a, b, c) = (off[0], off[1], off[2]);
    match dir {
        Dir::South => BBox::new([x + a, y + b, z + c], [x + w - 1 + a, y + h - 1 + b, z + d - 1 + c]),
        Dir::West => BBox::new([x - d + 1 + c, y + b, z + a], [x + c, y + h - 1 + b, z + w - 1 + a]),
        Dir::East => BBox::new([x + c, y + b, z + a], [x + d - 1 + c, y + h - 1 + b, z + w - 1 + a]),
        _ => BBox::new([x + a, y + b, z - d + 1 + c], [x + w - 1 + a, y + h - 1 + b, z + c]),
    }
}

struct Builder {
    pieces: Vec<(BBox, ShKind, Dir, Door, i32)>,
    weights: Vec<(usize, i32, i32, i32)>,
    imposed: Option<usize>,
    previous: Option<usize>,
    portal: bool,
    pending: Vec<usize>,
}

fn random_door(r: &mut DecoRng) -> Door {
    match r.next_int_bounded(5) {
        2 => Door::Wood,
        3 => Door::Grates,
        4 => Door::Iron,
        _ => Door::Opening,
    }
}

fn ok_box(b: &BBox) -> bool {
    b.min[1] > 10
}

impl Builder {
    fn collides(&self, b: &BBox) -> Option<usize> {
        self.pieces.iter().position(|p| p.0.intersects(b))
    }

    /// Creation of a piece of type `t` (weight index), `None` if the box does not fit.
    fn create(
        &mut self,
        t: usize,
        r: &mut DecoRng,
        f: [i32; 3],
        dir: Dir,
        depth: i32,
    ) -> Option<(BBox, ShKind, Dir, Door, i32)> {
        let free = |b: &BBox, s: &Builder| ok_box(b) && s.collides(b).is_none();
        let (off, w, h, d) = match t {
            0 => ([-1, -1, 0], 5, 5, 7),
            1 => ([-1, -1, 0], 9, 5, 11),
            2 | 3 => ([-1, -1, 0], 5, 5, 5),
            4 => ([-4, -1, 0], 11, 7, 11),
            5 => ([-1, -7, 0], 5, 11, 8),
            6 => ([-1, -7, 0], 5, 11, 5),
            7 => ([-4, -3, 0], 10, 9, 11),
            8 => ([-1, -1, 0], 5, 5, 7),
            9 => {
                let mut b = orient_box(f, [-4, -1, 0], 14, 11, 15, dir);
                if !free(&b, self) {
                    b = orient_box(f, [-4, -1, 0], 14, 6, 15, dir);
                    if !free(&b, self) {
                        return None;
                    }
                }
                let door = random_door(r);
                return Some((b, ShKind::Library { tall: b.span(1) > 6 }, dir, door, depth));
            }
            _ => {
                let b = orient_box(f, [-4, -1, 0], 11, 8, 16, dir);
                return free(&b, self).then_some((b, ShKind::PortalRoom, dir, Door::Opening, depth));
            }
        };
        let b = orient_box(f, off, w, h, d, dir);
        if !free(&b, self) {
            return None;
        }
        let door = random_door(r);
        let kind = match t {
            0 => {
                let left = r.next_int_bounded(2) == 0;
                let right = r.next_int_bounded(2) == 0;
                ShKind::Straight { left, right }
            }
            1 => ShKind::PrisonHall,
            2 => ShKind::LeftTurn,
            3 => ShKind::RightTurn,
            4 => ShKind::RoomCrossing(r.next_int_bounded(5)),
            5 => ShKind::StraightStairsDown,
            6 => ShKind::StairsDown { source: false },
            7 => {
                let left_low = r.next_bool();
                let left_high = r.next_bool();
                let right_low = r.next_bool();
                let right_high = r.next_int_bounded(3) > 0;
                ShKind::FiveCrossing { left_low, left_high, right_low, right_high }
            }
            _ => ShKind::ChestCorridor,
        };
        Some((b, kind, dir, door, depth))
    }

    fn small_door_piece(
        &mut self,
        r: &mut DecoRng,
        f: [i32; 3],
        dir: Dir,
        depth: i32,
    ) -> Option<(BBox, ShKind, Dir, Door, i32)> {
        // Weights update: are there limited pieces left?
        let mut any = false;
        let mut total = 0;
        for &(_, w, count, max) in &self.weights {
            if max > 0 && count < max {
                any = true;
            }
            total += w;
        }
        if !any {
            return None;
        }
        if let Some(t) = self.imposed.take()
            && let Some(p) = self.create(t, r, f, dir, depth)
        {
            return Some(p);
        }
        for _ in 0..5 {
            let mut sel = r.next_int_bounded(total);
            for i in 0..self.weights.len() {
                let (t, w, count, max) = self.weights[i];
                sel -= w;
                if sel < 0 {
                    let can = max == 0 || count < max;
                    let extra = match t {
                        9 => depth > 4,
                        10 => depth > 5,
                        _ => true,
                    };
                    if !(can && extra) || self.previous == Some(t) {
                        break;
                    }
                    if let Some(p) = self.create(t, r, f, dir, depth) {
                        self.weights[i].2 += 1;
                        self.previous = Some(t);
                        let (_, _, c, m) = self.weights[i];
                        if !(m == 0 || c < m) {
                            self.weights.remove(i);
                        }
                        return Some(p);
                    }
                }
            }
        }
        // Filler corridor.
        let b = orient_box(f, [-1, -1, 0], 5, 5, 4, dir);
        let ci = self.collides(&b)?;
        let cb = self.pieces[ci].0;
        if cb.min[1] == b.min[1] {
            for dd in (1..=2).rev() {
                let bb = orient_box(f, [-1, -1, 0], 5, 5, dd, dir);
                if !cb.intersects(&bb) {
                    let fb = orient_box(f, [-1, -1, 0], 5, 5, dd + 1, dir);
                    if fb.min[1] > 1 {
                        let steps = if matches!(dir, Dir::North | Dir::South) { fb.span(2) } else { fb.span(0) };
                        return Some((fb, ShKind::Filler { steps }, dir, Door::Opening, depth));
                    }
                    return None;
                }
            }
        }
        None
    }

    fn add(&mut self, r: &mut DecoRng, f: [i32; 3], dir: Dir, depth: i32) {
        if depth > 50 {
            return;
        }
        let s = self.pieces[0].0;
        if (f[0] - s.min[0]).abs() > 112 || (f[2] - s.min[2]).abs() > 112 {
            return;
        }
        if let Some(p) = self.small_door_piece(r, f, dir, depth + 1) {
            self.pieces.push(p);
            self.pending.push(self.pieces.len() - 1);
        }
    }

    fn forward(&mut self, i: usize, r: &mut DecoRng, xo: i32, yo: i32) {
        let (b, _, o, _, d) = self.pieces[i];
        let f = match o {
            Dir::North => [b.min[0] + xo, b.min[1] + yo, b.min[2] - 1],
            Dir::South => [b.min[0] + xo, b.min[1] + yo, b.max[2] + 1],
            Dir::West => [b.min[0] - 1, b.min[1] + yo, b.min[2] + xo],
            _ => [b.max[0] + 1, b.min[1] + yo, b.min[2] + xo],
        };
        self.add(r, f, o, d);
    }

    fn left(&mut self, i: usize, r: &mut DecoRng, yo: i32, zo: i32) {
        let (b, _, o, _, d) = self.pieces[i];
        let (f, nd) = match o {
            Dir::North | Dir::South => ([b.min[0] - 1, b.min[1] + yo, b.min[2] + zo], Dir::West),
            _ => ([b.min[0] + zo, b.min[1] + yo, b.min[2] - 1], Dir::North),
        };
        self.add(r, f, nd, d);
    }

    fn right(&mut self, i: usize, r: &mut DecoRng, yo: i32, zo: i32) {
        let (b, _, o, _, d) = self.pieces[i];
        let (f, nd) = match o {
            Dir::North | Dir::South => ([b.max[0] + 1, b.min[1] + yo, b.min[2] + zo], Dir::East),
            _ => ([b.min[0] + zo, b.min[1] + yo, b.max[2] + 1], Dir::South),
        };
        self.add(r, f, nd, d);
    }

    fn children(&mut self, i: usize, r: &mut DecoRng) {
        let (_, kind, o, _, _) = self.pieces[i];
        match kind {
            ShKind::Straight { left, right } => {
                self.forward(i, r, 1, 1);
                if left {
                    self.left(i, r, 1, 2);
                }
                if right {
                    self.right(i, r, 1, 2);
                }
            }
            ShKind::PrisonHall | ShKind::ChestCorridor | ShKind::StraightStairsDown => self.forward(i, r, 1, 1),
            ShKind::LeftTurn => {
                if o != Dir::North && o != Dir::East {
                    self.right(i, r, 1, 1)
                } else {
                    self.left(i, r, 1, 1)
                }
            }
            ShKind::RightTurn => {
                if o != Dir::North && o != Dir::East {
                    self.left(i, r, 1, 1)
                } else {
                    self.right(i, r, 1, 1)
                }
            }
            ShKind::RoomCrossing(_) => {
                self.forward(i, r, 4, 1);
                self.left(i, r, 1, 4);
                self.right(i, r, 1, 4);
            }
            ShKind::StairsDown { source } => {
                if source {
                    self.imposed = Some(7);
                }
                self.forward(i, r, 1, 1);
            }
            ShKind::FiveCrossing { left_low, left_high, right_low, right_high } => {
                let (mut a, mut b) = (3, 5);
                if o == Dir::West || o == Dir::North {
                    a = 8 - a;
                    b = 8 - b;
                }
                self.forward(i, r, 5, 1);
                if left_low {
                    self.left(i, r, a, 1);
                }
                if left_high {
                    self.left(i, r, b, 7);
                }
                if right_low {
                    self.right(i, r, a, 1);
                }
                if right_high {
                    self.right(i, r, b, 7);
                }
            }
            ShKind::PortalRoom => self.portal = true,
            ShKind::Library { .. } | ShKind::Filler { .. } => {}
        }
    }
}

pub fn stronghold(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    let (cx, cz) = (g.cx, g.cz);
    let sea = g.t.gc.sea;
    let gen_min = g.t.g.settings.min_y;
    let mut tries: i64 = 0;
    loop {
        let r = &mut g.rng;
        r.set_large_feature_seed(g.sg.seed.wrapping_add(tries), cx, cz);
        tries += 1;
        let dir = [Dir::North, Dir::East, Dir::South, Dir::West][r.next_int_bounded(4) as usize];
        let (x0, z0) = (16 * cx + 2, 16 * cz + 2);
        // Starting stairs 5 × 11 × 5 (square: same box in all orientations).
        let sb = BBox::new([x0, 64, z0], [x0 + 4, 74, z0 + 4]);
        let mut b = Builder {
            pieces: vec![(sb, ShKind::StairsDown { source: true }, dir, Door::Opening, 0)],
            weights: WEIGHTS.iter().enumerate().map(|(i, &(w, m))| (i, w, 0, m)).collect(),
            imposed: None,
            previous: None,
            portal: false,
            pending: Vec::new(),
        };
        b.children(0, r);
        while !b.pending.is_empty() {
            let pos = r.next_int_bounded(b.pending.len() as i32) as usize;
            let i = b.pending.remove(pos);
            b.children(i, r);
        }
        // Below sea level.
        let mut all = b.pieces[0].0;
        for p in &b.pieces[1..] {
            all.encapsulate(&p.0);
        }
        let max_y = sea - 10;
        let mut y = all.span(1) + gen_min + 1;
        if y < max_y {
            y += r.next_int_bounded(max_y - y);
        }
        let dy = y - all.max[1];
        if !b.portal {
            continue;
        }
        let pieces = b
            .pieces
            .into_iter()
            .map(|(bb, kind, o, door, depth)| {
                let bbox = bb.moved([0, dy, 0]);
                let sp = ShPiece { kind, orient: o, door, depth, placed: Mutex::new(false) };
                Piece::Coded { id: "stronghold", bbox, data: Arc::new(CodedPiece::Stronghold(Arc::new(sp))) }
            })
            .collect();
        return Some(([16 * cx, 0, 16 * cz], pieces));
    }
}

// ---------------------------------------------------------------- Placement

fn brick(p: &P, rng: &mut DecoRng, edge: bool) -> State {
    if !edge {
        return p.state("cave_air");
    }
    let f = rng.next_float();
    p.state(if f < 0.2 {
        "cracked_stone_bricks"
    } else if f < 0.5 {
        "mossy_stone_bricks"
    } else if f < 0.55 {
        "infested_stone_bricks"
    } else {
        "stone_bricks"
    })
}

/// Box with brick selector (draw only for the placed cells).
#[allow(clippy::too_many_arguments)]
fn bricks(p: &mut P, rng: &mut DecoRng, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, skip_air: bool) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            for z in z0..=z1 {
                if skip_air && p.is_air(x, y, z) {
                    continue;
                }
                let edge = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                let s = brick(p, rng, edge);
                p.place(s, x, y, z);
            }
        }
    }
}

fn with(p: &P, s: State, k: &str, v: &str) -> State {
    match p.b.prop(k) {
        Some(pr) if p.b.has(s, pr) => p.b.with_str(s, pr, v),
        _ => s,
    }
}

fn facing(p: &P, n: &str, d: Dir) -> State {
    with(p, p.state(n), "facing", d.name())
}

fn small_door(p: &mut P, door: Door, x: i32, y: i32, z: i32) {
    let air = p.state("cave_air");
    let sb = p.state("stone_bricks");
    match door {
        Door::Opening => p.fill(x, y, z, x + 2, y + 2, z, air, air, false),
        Door::Wood | Door::Iron => {
            for (dx, dy) in [(0, 0), (0, 1), (0, 2), (1, 2), (2, 2), (2, 1), (2, 0)] {
                p.place(sb, x + dx, y + dy, z);
            }
            let n = if door == Door::Wood { "oak_door" } else { "iron_door" };
            let d = p.state(n);
            p.place(d, x + 1, y, z);
            let up = with(p, d, "half", "upper");
            p.place(up, x + 1, y + 1, z);
            if door == Door::Iron {
                let b1 = facing(p, "stone_button", Dir::North);
                p.place(b1, x + 2, y + 1, z + 1);
                let b2 = facing(p, "stone_button", Dir::South);
                p.place(b2, x + 2, y + 1, z - 1);
            }
        }
        Door::Grates => {
            p.place(air, x + 1, y, z);
            p.place(air, x + 1, y + 1, z);
            let bars = p.state("iron_bars");
            let w = with(p, bars, "west", "true");
            let e = with(p, bars, "east", "true");
            let ew = with(p, w, "east", "true");
            p.place(w, x, y, z);
            p.place(w, x, y + 1, z);
            p.place(ew, x, y + 2, z);
            p.place(ew, x + 1, y + 2, z);
            p.place(ew, x + 2, y + 2, z);
            p.place(e, x + 2, y + 1, z);
            p.place(e, x + 2, y, z);
        }
    }
}

pub fn place(cx: &mut Ctx, sg: &StructureGen, sp: &ShPiece, bbox: BBox, rng: &mut DecoRng, zone: BBox) {
    let mut p = P::new(cx.r, &sg.templates.transforms, zone, bbox, Some(sp.orient));
    let air = p.state("cave_air");
    let sb = p.state("stone_bricks");
    let slab = p.state("smooth_stone_slab");
    match sp.kind {
        ShKind::ChestCorridor => {
            bricks(&mut p, rng, 0, 0, 0, 4, 4, 6, true);
            small_door(&mut p, sp.door, 1, 1, 0);
            small_door(&mut p, Door::Opening, 1, 1, 6);
            p.fill(3, 1, 2, 3, 1, 4, sb, sb, false);
            let bs = p.state("stone_brick_slab");
            for (x, y, z) in [(3, 1, 1), (3, 1, 5), (3, 2, 2), (3, 2, 4)] {
                p.place(bs, x, y, z);
            }
            for z in 2..=4 {
                p.place(bs, 2, 1, z);
            }
            let w = p.world(3, 2, 3);
            let mut placed = sp.placed.lock().expect("piece");
            if !*placed && p.zone.contains(w) {
                *placed = true;
                p.chest(rng, w, "minecraft:chests/stronghold_corridor", None);
            }
        }
        ShKind::Filler { steps } => {
            for i in 0..steps {
                for x in 0..=4 {
                    p.place(sb, x, 0, i);
                }
                for y in 1..=3 {
                    p.place(sb, 0, y, i);
                    p.place(air, 1, y, i);
                    p.place(air, 2, y, i);
                    p.place(air, 3, y, i);
                    p.place(sb, 4, y, i);
                }
                for x in 0..=4 {
                    p.place(sb, x, 4, i);
                }
            }
        }
        ShKind::FiveCrossing { left_low, left_high, right_low, right_high } => {
            bricks(&mut p, rng, 0, 0, 0, 9, 8, 10, true);
            small_door(&mut p, sp.door, 4, 3, 0);
            if left_low {
                p.fill(0, 3, 1, 0, 5, 3, air, air, false);
            }
            if right_low {
                p.fill(9, 3, 1, 9, 5, 3, air, air, false);
            }
            if left_high {
                p.fill(0, 5, 7, 0, 7, 9, air, air, false);
            }
            if right_high {
                p.fill(9, 5, 7, 9, 7, 9, air, air, false);
            }
            p.fill(5, 1, 10, 7, 3, 10, air, air, false);
            bricks(&mut p, rng, 1, 2, 1, 8, 2, 6, false);
            bricks(&mut p, rng, 4, 1, 5, 4, 4, 9, false);
            bricks(&mut p, rng, 8, 1, 5, 8, 4, 9, false);
            bricks(&mut p, rng, 1, 4, 7, 3, 4, 9, false);
            bricks(&mut p, rng, 1, 3, 5, 3, 3, 6, false);
            p.fill(1, 3, 4, 3, 3, 4, slab, slab, false);
            p.fill(1, 4, 6, 3, 4, 6, slab, slab, false);
            bricks(&mut p, rng, 5, 1, 7, 7, 1, 8, false);
            p.fill(5, 1, 9, 7, 1, 9, slab, slab, false);
            p.fill(5, 2, 7, 7, 2, 7, slab, slab, false);
            p.fill(4, 5, 7, 4, 5, 9, slab, slab, false);
            p.fill(8, 5, 7, 8, 5, 9, slab, slab, false);
            let dbl = with(&p, slab, "type", "double");
            p.fill(5, 5, 7, 7, 5, 9, dbl, dbl, false);
            let t = facing(&p, "wall_torch", Dir::South);
            p.place(t, 6, 5, 6);
        }
        ShKind::LeftTurn | ShKind::RightTurn => {
            bricks(&mut p, rng, 0, 0, 0, 4, 4, 4, true);
            small_door(&mut p, sp.door, 1, 1, 0);
            let o = sp.orient;
            let other = o != Dir::North && o != Dir::East;
            let east_side = (sp.kind == ShKind::LeftTurn) == other;
            if east_side {
                p.fill(4, 1, 1, 4, 3, 3, air, air, false);
            } else {
                p.fill(0, 1, 1, 0, 3, 3, air, air, false);
            }
        }
        ShKind::Library { tall } => library(&mut p, sp, rng, tall),
        ShKind::PortalRoom => portal_room(&mut p, sp, rng),
        ShKind::PrisonHall => {
            bricks(&mut p, rng, 0, 0, 0, 8, 4, 10, true);
            small_door(&mut p, sp.door, 1, 1, 0);
            p.fill(1, 1, 10, 3, 3, 10, air, air, false);
            for z in [1, 3, 7, 9] {
                bricks(&mut p, rng, 4, 1, z, 4, 3, z, false);
            }
            let bars = p.state("iron_bars");
            let ns = with(&p, with(&p, bars, "north", "true"), "south", "true");
            let nse = with(&p, ns, "east", "true");
            let we = with(&p, with(&p, bars, "west", "true"), "east", "true");
            for y in 1..=3 {
                p.place(ns, 4, y, 4);
                p.place(nse, 4, y, 5);
                p.place(ns, 4, y, 6);
                p.place(we, 5, y, 5);
                p.place(we, 6, y, 5);
                p.place(we, 7, y, 5);
            }
            p.place(ns, 4, 3, 2);
            p.place(ns, 4, 3, 8);
            let db = facing(&p, "iron_door", Dir::West);
            let dt = with(&p, db, "half", "upper");
            p.place(db, 4, 1, 2);
            p.place(dt, 4, 2, 2);
            p.place(db, 4, 1, 8);
            p.place(dt, 4, 2, 8);
        }
        ShKind::RoomCrossing(t) => room_crossing(&mut p, sp, rng, t),
        ShKind::StairsDown { .. } => {
            bricks(&mut p, rng, 0, 0, 0, 4, 10, 4, true);
            small_door(&mut p, sp.door, 1, 7, 0);
            small_door(&mut p, Door::Opening, 1, 1, 4);
            for (s, x, y, z) in [
                (sb, 2, 6, 1),
                (sb, 1, 5, 1),
                (slab, 1, 6, 1),
                (sb, 1, 5, 2),
                (sb, 1, 4, 3),
                (slab, 1, 5, 3),
                (sb, 2, 4, 3),
                (sb, 3, 3, 3),
                (slab, 3, 4, 3),
                (sb, 3, 3, 2),
                (sb, 3, 2, 1),
                (slab, 3, 3, 1),
                (sb, 2, 2, 1),
                (sb, 1, 1, 1),
                (slab, 1, 2, 1),
                (sb, 1, 1, 2),
                (slab, 1, 1, 3),
            ] {
                p.place(s, x, y, z);
            }
        }
        ShKind::Straight { left, right } => {
            bricks(&mut p, rng, 0, 0, 0, 4, 4, 6, true);
            small_door(&mut p, sp.door, 1, 1, 0);
            small_door(&mut p, Door::Opening, 1, 1, 6);
            let e = facing(&p, "wall_torch", Dir::East);
            let w = facing(&p, "wall_torch", Dir::West);
            p.maybe_block(rng, 0.1, 1, 2, 1, e);
            p.maybe_block(rng, 0.1, 3, 2, 1, w);
            p.maybe_block(rng, 0.1, 1, 2, 5, e);
            p.maybe_block(rng, 0.1, 3, 2, 5, w);
            if left {
                p.fill(0, 1, 2, 0, 3, 4, air, air, false);
            }
            if right {
                p.fill(4, 1, 2, 4, 3, 4, air, air, false);
            }
        }
        ShKind::StraightStairsDown => {
            bricks(&mut p, rng, 0, 0, 0, 4, 10, 7, true);
            small_door(&mut p, sp.door, 1, 7, 0);
            small_door(&mut p, Door::Opening, 1, 1, 7);
            let st = facing(&p, "cobblestone_stairs", Dir::South);
            for i in 0..6 {
                for x in 1..=3 {
                    p.place(st, x, 6 - i, 1 + i);
                }
                if i < 5 {
                    for x in 1..=3 {
                        p.place(sb, x, 5 - i, 1 + i);
                    }
                }
            }
        }
    }
}

fn library(p: &mut P, sp: &ShPiece, rng: &mut DecoRng, tall: bool) {
    let h = if tall { 11 } else { 6 };
    bricks(p, rng, 0, 0, 0, 13, h - 1, 14, true);
    small_door(p, sp.door, 4, 1, 0);
    let web = p.state("cobweb");
    p.maybe_fill(rng, 0.07, 2, 1, 1, 11, 4, 13, web, web, false, false);
    let planks = p.state("oak_planks");
    let shelf = p.state("bookshelf");
    for d in 1..=13 {
        if (d - 1) % 4 == 0 {
            p.fill(1, 1, d, 1, 4, d, planks, planks, false);
            p.fill(12, 1, d, 12, 4, d, planks, planks, false);
            let te = facing(p, "wall_torch", Dir::East);
            p.place(te, 2, 3, d);
            let tw = facing(p, "wall_torch", Dir::West);
            p.place(tw, 11, 3, d);
            if tall {
                p.fill(1, 6, d, 1, 9, d, planks, planks, false);
                p.fill(12, 6, d, 12, 9, d, planks, planks, false);
            }
        } else {
            p.fill(1, 1, d, 1, 4, d, shelf, shelf, false);
            p.fill(12, 1, d, 12, 4, d, shelf, shelf, false);
            if tall {
                p.fill(1, 6, d, 1, 9, d, shelf, shelf, false);
                p.fill(12, 6, d, 12, 9, d, shelf, shelf, false);
            }
        }
    }
    let mut d = 3;
    while d < 12 {
        p.fill(3, 1, d, 4, 3, d, shelf, shelf, false);
        p.fill(6, 1, d, 7, 3, d, shelf, shelf, false);
        p.fill(9, 1, d, 10, 3, d, shelf, shelf, false);
        d += 2;
    }
    if tall {
        p.fill(1, 5, 1, 3, 5, 13, planks, planks, false);
        p.fill(10, 5, 1, 12, 5, 13, planks, planks, false);
        p.fill(4, 5, 1, 9, 5, 2, planks, planks, false);
        p.fill(4, 5, 12, 9, 5, 13, planks, planks, false);
        p.place(planks, 9, 5, 11);
        p.place(planks, 8, 5, 11);
        p.place(planks, 9, 5, 10);
        let fence = p.state("oak_fence");
        let f = |p: &P, sides: &[&str]| sides.iter().fold(fence, |s, k| with(p, s, k, "true"));
        let we = f(p, &["west", "east"]);
        let ns = f(p, &["north", "south"]);
        p.fill(3, 6, 3, 3, 6, 11, ns, ns, false);
        p.fill(10, 6, 3, 10, 6, 9, ns, ns, false);
        p.fill(4, 6, 2, 9, 6, 2, we, we, false);
        p.fill(4, 6, 12, 7, 6, 12, we, we, false);
        let a = f(p, &["north", "east"]);
        p.place(a, 3, 6, 2);
        let b = f(p, &["south", "east"]);
        p.place(b, 3, 6, 12);
        let c = f(p, &["north", "west"]);
        p.place(c, 10, 6, 2);
        let sw = f(p, &["south", "west"]);
        let ne = f(p, &["north", "east"]);
        for i in 0..=2 {
            p.place(sw, 8 + i, 6, 12 - i);
            if i != 2 {
                p.place(ne, 8 + i, 6, 11 - i);
            }
        }
        let ladder = facing(p, "ladder", Dir::South);
        for y in 1..=7 {
            p.place(ladder, 10, y, 13);
        }
        let e = f(p, &["east"]);
        let w = f(p, &["west"]);
        p.place(e, 6, 9, 7);
        p.place(w, 7, 9, 7);
        p.place(e, 6, 8, 7);
        p.place(w, 7, 8, 7);
        let nswe = f(p, &["north", "south", "west", "east"]);
        p.place(nswe, 6, 7, 7);
        p.place(nswe, 7, 7, 7);
        p.place(e, 5, 7, 7);
        p.place(w, 8, 7, 7);
        let en = f(p, &["east", "north"]);
        p.place(en, 6, 7, 6);
        let es = f(p, &["east", "south"]);
        p.place(es, 6, 7, 8);
        let wn = f(p, &["west", "north"]);
        p.place(wn, 7, 7, 6);
        let ws = f(p, &["west", "south"]);
        p.place(ws, 7, 7, 8);
        let torch = p.state("torch");
        for (x, z) in [(5, 7), (8, 7), (6, 6), (6, 8), (7, 6), (7, 8)] {
            p.place(torch, x, 8, z);
        }
    }
    let w = p.world(3, 3, 5);
    p.chest(rng, w, "minecraft:chests/stronghold_library", None);
    if tall {
        let air = p.state("cave_air");
        p.place(air, 12, 9, 1);
        let w = p.world(12, 8, 1);
        p.chest(rng, w, "minecraft:chests/stronghold_library", None);
    }
}

fn portal_room(p: &mut P, sp: &ShPiece, rng: &mut DecoRng) {
    bricks(p, rng, 0, 0, 0, 10, 7, 15, false);
    small_door(p, Door::Grates, 4, 1, 0);
    bricks(p, rng, 1, 6, 1, 1, 6, 14, false);
    bricks(p, rng, 9, 6, 1, 9, 6, 14, false);
    bricks(p, rng, 2, 6, 1, 8, 6, 2, false);
    bricks(p, rng, 2, 6, 14, 8, 6, 14, false);
    bricks(p, rng, 1, 1, 1, 2, 1, 4, false);
    bricks(p, rng, 8, 1, 1, 9, 1, 4, false);
    let lava = p.state("lava");
    p.fill(1, 1, 1, 1, 1, 3, lava, lava, false);
    p.fill(9, 1, 1, 9, 1, 3, lava, lava, false);
    bricks(p, rng, 3, 1, 8, 7, 1, 12, false);
    p.fill(4, 1, 9, 6, 1, 11, lava, lava, false);
    let bars = p.state("iron_bars");
    let ns = with(p, with(p, bars, "north", "true"), "south", "true");
    let we = with(p, with(p, bars, "west", "true"), "east", "true");
    let mut z = 3;
    while z < 14 {
        p.fill(0, 3, z, 0, 4, z, ns, ns, false);
        p.fill(10, 3, z, 10, 4, z, ns, ns, false);
        z += 2;
    }
    let mut x = 2;
    while x < 9 {
        p.fill(x, 3, 15, x, 4, 15, we, we, false);
        x += 2;
    }
    let st = facing(p, "stone_brick_stairs", Dir::North);
    bricks(p, rng, 4, 1, 5, 6, 1, 7, false);
    bricks(p, rng, 4, 2, 6, 6, 2, 7, false);
    bricks(p, rng, 4, 3, 7, 6, 3, 7, false);
    for x in 4..=6 {
        p.place(st, x, 1, 4);
        p.place(st, x, 2, 5);
        p.place(st, x, 3, 6);
    }
    let mut eyes = [false; 12];
    let mut all = true;
    for e in eyes.iter_mut() {
        *e = rng.next_float() > 0.9;
        all &= *e;
    }
    let frame =
        |p: &P, d: Dir, eye: bool| with(p, facing(p, "end_portal_frame", d), "eye", if eye { "true" } else { "false" });
    let spots = [
        (Dir::North, 4, 8),
        (Dir::North, 5, 8),
        (Dir::North, 6, 8),
        (Dir::South, 4, 12),
        (Dir::South, 5, 12),
        (Dir::South, 6, 12),
        (Dir::East, 3, 9),
        (Dir::East, 3, 10),
        (Dir::East, 3, 11),
        (Dir::West, 7, 9),
        (Dir::West, 7, 10),
        (Dir::West, 7, 11),
    ];
    for (i, (d, x, z)) in spots.iter().enumerate() {
        let s = frame(p, *d, eyes[i]);
        p.place(s, *x, 3, *z);
    }
    if all {
        let portal = p.state("end_portal");
        for z in 9..=11 {
            for x in 4..=6 {
                p.place(portal, x, 3, z);
            }
        }
    }
    let mut placed = sp.placed.lock().expect("piece");
    if !*placed {
        let w = p.world(5, 3, 6);
        if p.zone.contains(w) {
            *placed = true;
            let s = p.state("spawner");
            p.r.set_p(w, s, 2);
            p.r.note_block_entity(w, BeData::Spawner { entity: "minecraft:silverfish" });
        }
    }
}

fn room_crossing(p: &mut P, sp: &ShPiece, rng: &mut DecoRng, t: i32) {
    let air = p.state("cave_air");
    let sb = p.state("stone_bricks");
    let slab = p.state("smooth_stone_slab");
    bricks(p, rng, 0, 0, 0, 10, 6, 10, true);
    small_door(p, sp.door, 4, 1, 0);
    p.fill(4, 1, 10, 6, 3, 10, air, air, false);
    p.fill(0, 1, 4, 0, 3, 6, air, air, false);
    p.fill(10, 1, 4, 10, 3, 6, air, air, false);
    match t {
        0 => {
            p.place(sb, 5, 1, 5);
            p.place(sb, 5, 2, 5);
            p.place(sb, 5, 3, 5);
            for (d, x, z) in [(Dir::West, 4, 5), (Dir::East, 6, 5), (Dir::South, 5, 4), (Dir::North, 5, 6)] {
                let t = facing(p, "wall_torch", d);
                p.place(t, x, 3, z);
            }
            for (x, z) in [(4, 4), (4, 5), (4, 6), (6, 4), (6, 5), (6, 6), (5, 4), (5, 6)] {
                p.place(slab, x, 1, z);
            }
        }
        1 => {
            for i in 0..5 {
                p.place(sb, 3, 1, 3 + i);
                p.place(sb, 7, 1, 3 + i);
                p.place(sb, 3 + i, 1, 3);
                p.place(sb, 3 + i, 1, 7);
            }
            p.place(sb, 5, 1, 5);
            p.place(sb, 5, 2, 5);
            p.place(sb, 5, 3, 5);
            let water = p.state("water");
            p.place(water, 5, 4, 5);
        }
        2 => {
            let cob = p.state("cobblestone");
            for z in 1..=9 {
                p.place(cob, 1, 3, z);
                p.place(cob, 9, 3, z);
            }
            for x in 1..=9 {
                p.place(cob, x, 3, 1);
                p.place(cob, x, 3, 9);
            }
            for (x, y, z) in [(5, 1, 4), (5, 1, 6), (5, 3, 4), (5, 3, 6), (4, 1, 5), (6, 1, 5), (4, 3, 5), (6, 3, 5)] {
                p.place(cob, x, y, z);
            }
            for y in 1..=3 {
                p.place(cob, 4, y, 4);
                p.place(cob, 6, y, 4);
                p.place(cob, 4, y, 6);
                p.place(cob, 6, y, 6);
            }
            let torch = p.state("wall_torch");
            p.place(torch, 5, 3, 5);
            let planks = p.state("oak_planks");
            for z in 2..=8 {
                p.place(planks, 2, 3, z);
                p.place(planks, 3, 3, z);
                if z <= 3 || z >= 7 {
                    p.place(planks, 4, 3, z);
                    p.place(planks, 5, 3, z);
                    p.place(planks, 6, 3, z);
                }
                p.place(planks, 7, 3, z);
                p.place(planks, 8, 3, z);
            }
            let ladder = facing(p, "ladder", Dir::West);
            for y in 1..=3 {
                p.place(ladder, 9, y, 3);
            }
            let w = p.world(3, 4, 8);
            p.chest(rng, w, "minecraft:chests/stronghold_crossing", None);
        }
        _ => {}
    }
}
