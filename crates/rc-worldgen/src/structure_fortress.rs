//! Nether fortress (`generation-decoration.md` § 8.7): assembly by weighted pieces (two
//! lists: bridge and castle) from the starting crossing, queue of children drawn at random, then
//! lifted between y 48 and 70; placement of the pieces (Nether bricks, fences, chests, blaze
//! spawner, nether wart).

use std::sync::{Arc, Mutex};

use crate::decor::Ctx;
use crate::decor_blocks::Dir;
use crate::decor_region::BeData;
use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece, StructureGen};
use crate::structure_coded::CodedPiece;
use crate::structure_piece::{P, SetProp, st};
use crate::structure_stronghold::orient_box;
use crate::template::BBox;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FKind {
    Start,
    BridgeCrossing,
    BridgeStraight,
    RoomCrossing,
    StairsRoom,
    Throne,
    Entrance,
    SmallCorridor,
    SmallCrossing,
    RightTurn,
    LeftTurn,
    CorridorStairs,
    TBalcony,
    StalkRoom,
    EndFiller { seed: i32 },
}

#[derive(Debug)]
pub struct FPiece {
    pub kind: FKind,
    pub orient: Dir,
    pub depth: i32,
    /// Chest to place (corners) or spawner already placed (throne).
    pub flag: Mutex<bool>,
}

/// Drawn types: indices 0–5 = bridge list, 6–12 = castle list.
const KINDS: [FKind; 13] = [
    FKind::BridgeStraight,
    FKind::BridgeCrossing,
    FKind::RoomCrossing,
    FKind::StairsRoom,
    FKind::Throne,
    FKind::Entrance,
    FKind::SmallCorridor,
    FKind::SmallCrossing,
    FKind::RightTurn,
    FKind::LeftTurn,
    FKind::CorridorStairs,
    FKind::TBalcony,
    FKind::StalkRoom,
];

/// (type, weight, maximum, allowed twice in a row).
const BRIDGE: [(usize, i32, i32, bool); 6] =
    [(0, 30, 0, true), (1, 10, 4, false), (2, 10, 4, false), (3, 10, 3, false), (4, 5, 2, false), (5, 5, 1, false)];
const CASTLE: [(usize, i32, i32, bool); 7] = [
    (6, 25, 0, true),
    (7, 15, 5, false),
    (8, 5, 10, false),
    (9, 5, 10, false),
    (10, 10, 3, true),
    (11, 7, 2, false),
    (12, 5, 2, false),
];

/// Relative box (offset, size) of a type.
fn shape(k: FKind) -> ([i32; 3], [i32; 3]) {
    match k {
        FKind::BridgeStraight => ([-1, -3, 0], [5, 10, 19]),
        FKind::Start | FKind::BridgeCrossing => ([-8, -3, 0], [19, 10, 19]),
        FKind::RoomCrossing => ([-2, 0, 0], [7, 9, 7]),
        FKind::StairsRoom => ([-2, 0, 0], [7, 11, 7]),
        FKind::Throne => ([-2, 0, 0], [7, 8, 9]),
        FKind::Entrance | FKind::StalkRoom => ([-5, -3, 0], [13, 14, 13]),
        FKind::SmallCorridor | FKind::SmallCrossing | FKind::RightTurn | FKind::LeftTurn => ([-1, 0, 0], [5, 7, 5]),
        FKind::CorridorStairs => ([-1, -7, 0], [5, 14, 10]),
        FKind::TBalcony => ([-3, 0, 0], [9, 7, 9]),
        FKind::EndFiller { .. } => ([-1, -3, 0], [5, 10, 8]),
    }
}

struct W {
    t: usize,
    weight: i32,
    max: i32,
    in_row: bool,
    count: i32,
}

struct Builder {
    pieces: Vec<(BBox, FKind, Dir, i32, bool)>,
    lists: [Vec<W>; 2],
    previous: Option<usize>,
    pending: Vec<usize>,
}

impl Builder {
    fn free(&self, b: &BBox) -> bool {
        b.min[1] > 10 && !self.pieces.iter().any(|p| p.0.intersects(b))
    }

    /// Bridge end (own seed drawn only if the box fits).
    fn end_filler(&self, r: &mut DecoRng, f: [i32; 3], dir: Dir, depth: i32) -> Option<(BBox, FKind, Dir, i32, bool)> {
        let (o, s) = shape(FKind::EndFiller { seed: 0 });
        let b = orient_box(f, o, s[0], s[1], s[2], dir);
        if !self.free(&b) {
            return None;
        }
        Some((b, FKind::EndFiller { seed: r.next_int() }, dir, depth, false))
    }

    fn create(
        &self,
        t: usize,
        r: &mut DecoRng,
        f: [i32; 3],
        dir: Dir,
        depth: i32,
    ) -> Option<(BBox, FKind, Dir, i32, bool)> {
        let k = KINDS[t];
        let (o, s) = shape(k);
        let b = orient_box(f, o, s[0], s[1], s[2], dir);
        if !self.free(&b) {
            return None;
        }
        let chest = matches!(k, FKind::RightTurn | FKind::LeftTurn) && r.next_int_bounded(3) == 0;
        Some((b, k, dir, depth, chest))
    }

    fn generate(
        &mut self,
        castle: bool,
        r: &mut DecoRng,
        f: [i32; 3],
        dir: Dir,
        depth: i32,
    ) -> Option<(BBox, FKind, Dir, i32, bool)> {
        let li = castle as usize;
        let mut any = false;
        let mut total = 0;
        for w in &self.lists[li] {
            if w.max > 0 && w.count < w.max {
                any = true;
            }
            total += w.weight;
        }
        if any && total > 0 && depth <= 30 {
            for _ in 0..5 {
                let mut sel = r.next_int_bounded(total);
                let mut i = 0;
                while i < self.lists[li].len() {
                    let (t, weight, max, in_row, count) = {
                        let w = &self.lists[li][i];
                        (w.t, w.weight, w.max, w.in_row, w.count)
                    };
                    sel -= weight;
                    if sel < 0 {
                        if !(max == 0 || count < max) || (self.previous == Some(t) && !in_row) {
                            break;
                        }
                        if let Some(p) = self.create(t, r, f, dir, depth) {
                            let w = &mut self.lists[li][i];
                            w.count += 1;
                            self.previous = Some(t);
                            if !(w.max == 0 || w.count < w.max) {
                                self.lists[li].remove(i);
                            }
                            return Some(p);
                        }
                    }
                    i += 1;
                }
            }
        }
        self.end_filler(r, f, dir, depth)
    }

    fn add(&mut self, r: &mut DecoRng, f: [i32; 3], dir: Dir, depth: i32, castle: bool) {
        let s = self.pieces[0].0;
        if (f[0] - s.min[0]).abs() <= 112 && (f[2] - s.min[2]).abs() <= 112 {
            if let Some(p) = self.generate(castle, r, f, dir, depth + 1) {
                self.pieces.push(p);
                self.pending.push(self.pieces.len() - 1);
            }
        } else {
            // Bridge end created (draw included) but never added.
            let _ = self.end_filler(r, f, dir, depth);
        }
    }

    fn forward(&mut self, i: usize, r: &mut DecoRng, xo: i32, yo: i32, castle: bool) {
        let (b, _, o, d, _) = self.pieces[i];
        let f = match o {
            Dir::North => [b.min[0] + xo, b.min[1] + yo, b.min[2] - 1],
            Dir::South => [b.min[0] + xo, b.min[1] + yo, b.max[2] + 1],
            Dir::West => [b.min[0] - 1, b.min[1] + yo, b.min[2] + xo],
            _ => [b.max[0] + 1, b.min[1] + yo, b.min[2] + xo],
        };
        self.add(r, f, o, d, castle);
    }

    fn left(&mut self, i: usize, r: &mut DecoRng, yo: i32, zo: i32, castle: bool) {
        let (b, _, o, d, _) = self.pieces[i];
        let (f, nd) = match o {
            Dir::North | Dir::South => ([b.min[0] - 1, b.min[1] + yo, b.min[2] + zo], Dir::West),
            _ => ([b.min[0] + zo, b.min[1] + yo, b.min[2] - 1], Dir::North),
        };
        self.add(r, f, nd, d, castle);
    }

    fn right(&mut self, i: usize, r: &mut DecoRng, yo: i32, zo: i32, castle: bool) {
        let (b, _, o, d, _) = self.pieces[i];
        let (f, nd) = match o {
            Dir::North | Dir::South => ([b.max[0] + 1, b.min[1] + yo, b.min[2] + zo], Dir::East),
            _ => ([b.min[0] + zo, b.min[1] + yo, b.max[2] + 1], Dir::South),
        };
        self.add(r, f, nd, d, castle);
    }

    fn children(&mut self, i: usize, r: &mut DecoRng) {
        let (_, kind, o, _, _) = self.pieces[i];
        match kind {
            FKind::Start | FKind::BridgeCrossing => {
                self.forward(i, r, 8, 3, false);
                self.left(i, r, 3, 8, false);
                self.right(i, r, 3, 8, false);
            }
            FKind::BridgeStraight => self.forward(i, r, 1, 3, false),
            FKind::RoomCrossing => {
                self.forward(i, r, 2, 0, false);
                self.left(i, r, 0, 2, false);
                self.right(i, r, 0, 2, false);
            }
            FKind::StairsRoom => self.right(i, r, 6, 2, false),
            FKind::Entrance => self.forward(i, r, 5, 3, true),
            FKind::SmallCorridor | FKind::CorridorStairs => self.forward(i, r, 1, 0, true),
            FKind::SmallCrossing => {
                self.forward(i, r, 1, 0, true);
                self.left(i, r, 0, 1, true);
                self.right(i, r, 0, 1, true);
            }
            FKind::LeftTurn => self.left(i, r, 0, 1, true),
            FKind::RightTurn => self.right(i, r, 0, 1, true),
            FKind::TBalcony => {
                let zo = if matches!(o, Dir::West | Dir::North) { 5 } else { 1 };
                let c = r.next_int_bounded(8) > 0;
                self.left(i, r, 0, zo, c);
                let c = r.next_int_bounded(8) > 0;
                self.right(i, r, 0, zo, c);
            }
            FKind::StalkRoom => {
                self.forward(i, r, 5, 3, true);
                self.forward(i, r, 5, 11, true);
            }
            FKind::Throne | FKind::EndFiller { .. } => {}
        }
    }
}

pub fn fortress(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    let r = &mut g.rng;
    let (x0, z0) = (16 * g.cx + 2, 16 * g.cz + 2);
    let dir = [Dir::North, Dir::East, Dir::South, Dir::West][r.next_int_bounded(4) as usize];
    let sb = BBox::new([x0, 64, z0], [x0 + 18, 73, z0 + 18]);
    let list = |l: &[(usize, i32, i32, bool)]| -> Vec<W> {
        l.iter().map(|&(t, weight, max, in_row)| W { t, weight, max, in_row, count: 0 }).collect()
    };
    let mut b = Builder {
        pieces: vec![(sb, FKind::Start, dir, 0, false)],
        lists: [list(&BRIDGE), list(&CASTLE)],
        previous: None,
        pending: Vec::new(),
    };
    b.children(0, r);
    while !b.pending.is_empty() {
        let pos = r.next_int_bounded(b.pending.len() as i32) as usize;
        let i = b.pending.remove(pos);
        b.children(i, r);
    }
    let mut all = b.pieces[0].0;
    for p in &b.pieces[1..] {
        all.encapsulate(&p.0);
    }
    let span = 70 - 48 + 1 - all.span(1);
    let y0 = if span > 1 { 48 + r.next_int_bounded(span) } else { 48 };
    let dy = y0 - all.min[1];
    let pieces = b
        .pieces
        .into_iter()
        .map(|(bb, kind, orient, depth, flag)| {
            let fp = FPiece { kind, orient, depth, flag: Mutex::new(flag) };
            Piece::Coded {
                id: "nether_fortress",
                bbox: bb.moved([0, dy, 0]),
                data: Arc::new(CodedPiece::Fortress(Arc::new(fp))),
            }
        })
        .collect();
    Some(([16 * g.cx, 64, 16 * g.cz], pieces))
}

// ---------------------------------------------------------------- Placement

/// Chest of a corner (only once, if it is within the cut-out).
fn chest(p: &mut P, fp: &FPiece, rng: &mut DecoRng, x: i32, y: i32, z: i32) {
    let w = p.world(x, y, z);
    let mut need = fp.flag.lock().expect("piece");
    if *need && p.zone.contains(w) {
        *need = false;
        p.chest(rng, w, "minecraft:chests/nether_bridge", None);
    }
}

pub fn place(cx: &mut Ctx, sg: &StructureGen, fp: &FPiece, bbox: BBox, rng: &mut DecoRng, zone: BBox) {
    let mut p = P::new(cx.r, &sg.templates.transforms, zone, bbox, Some(fp.orient));
    let b = p.b;
    let nb = st(b, "nether_bricks");
    let air = st(b, "air");
    let fence = st(b, "nether_brick_fence");
    match fp.kind {
        FKind::Start | FKind::BridgeCrossing => {
            p.bx(7, 3, 0, 11, 4, 18, nb);
            p.bx(0, 3, 7, 18, 4, 11, nb);
            p.bx(8, 5, 0, 10, 7, 18, air);
            p.bx(0, 5, 8, 18, 7, 10, air);
            p.bx(7, 5, 0, 7, 5, 7, nb);
            p.bx(7, 5, 11, 7, 5, 18, nb);
            p.bx(11, 5, 0, 11, 5, 7, nb);
            p.bx(11, 5, 11, 11, 5, 18, nb);
            p.bx(0, 5, 7, 7, 5, 7, nb);
            p.bx(11, 5, 7, 18, 5, 7, nb);
            p.bx(0, 5, 11, 7, 5, 11, nb);
            p.bx(11, 5, 11, 18, 5, 11, nb);
            p.bx(7, 2, 0, 11, 2, 5, nb);
            p.bx(7, 2, 13, 11, 2, 18, nb);
            p.bx(7, 0, 0, 11, 1, 3, nb);
            p.bx(7, 0, 15, 11, 1, 18, nb);
            for x in 7..=11 {
                for z in 0..=2 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, x, -1, 18 - z);
                }
            }
            p.bx(0, 2, 7, 5, 2, 11, nb);
            p.bx(13, 2, 7, 18, 2, 11, nb);
            p.bx(0, 0, 7, 3, 1, 11, nb);
            p.bx(15, 0, 7, 18, 1, 11, nb);
            for x in 0..=2 {
                for z in 7..=11 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, 18 - x, -1, z);
                }
            }
        }
        FKind::EndFiller { seed } => {
            let mut own = DecoRng::legacy(i64::from(seed));
            for x in 0..=4 {
                for y in 3..=4 {
                    let z = own.next_int_bounded(8);
                    p.bx(x, y, 0, x, y, z, nb);
                }
            }
            let z = own.next_int_bounded(8);
            p.bx(0, 5, 0, 0, 5, z, nb);
            let z = own.next_int_bounded(8);
            p.bx(4, 5, 0, 4, 5, z, nb);
            for x in 0..=4 {
                let zx = own.next_int_bounded(5);
                p.bx(x, 2, 0, x, 2, zx, nb);
            }
            for x in 0..=4 {
                for y in 0..=1 {
                    let zx = own.next_int_bounded(3);
                    p.bx(x, y, 0, x, y, zx, nb);
                }
            }
        }
        FKind::BridgeStraight => {
            p.bx(0, 3, 0, 4, 4, 18, nb);
            p.bx(1, 5, 0, 3, 7, 18, air);
            p.bx(0, 5, 0, 0, 5, 18, nb);
            p.bx(4, 5, 0, 4, 5, 18, nb);
            p.bx(0, 2, 0, 4, 2, 5, nb);
            p.bx(0, 2, 13, 4, 2, 18, nb);
            p.bx(0, 0, 0, 4, 1, 3, nb);
            p.bx(0, 0, 15, 4, 1, 18, nb);
            for x in 0..=4 {
                for z in 0..=2 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, x, -1, 18 - z);
                }
            }
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            let nse_f = ns_f.set(b, "east", "true");
            let nsw_f = ns_f.set(b, "west", "true");
            p.bx(0, 1, 1, 0, 4, 1, nse_f);
            p.bx(0, 3, 4, 0, 4, 4, nse_f);
            p.bx(0, 3, 14, 0, 4, 14, nse_f);
            p.bx(0, 1, 17, 0, 4, 17, nse_f);
            p.bx(4, 1, 1, 4, 4, 1, nsw_f);
            p.bx(4, 3, 4, 4, 4, 4, nsw_f);
            p.bx(4, 3, 14, 4, 4, 14, nsw_f);
            p.bx(4, 1, 17, 4, 4, 17, nsw_f);
        }
        FKind::CorridorStairs => {
            let stairs = st(b, "nether_brick_stairs").set(b, "facing", "south");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            for step in 0..=9 {
                let floor = (7 - step).max(1);
                let roof = (floor + 5).max(14 - step).min(13);
                let z = step;
                p.bx(0, 0, z, 4, floor, z, nb);
                p.bx(1, floor + 1, z, 3, roof - 1, z, air);
                if step <= 6 {
                    p.place(stairs, 1, floor + 1, z);
                    p.place(stairs, 2, floor + 1, z);
                    p.place(stairs, 3, floor + 1, z);
                }
                p.bx(0, roof, z, 4, roof, z, nb);
                p.bx(0, floor + 1, z, 0, roof - 1, z, nb);
                p.bx(4, floor + 1, z, 4, roof - 1, z, nb);
                if (step & 1) == 0 {
                    p.bx(0, floor + 2, z, 0, floor + 3, z, ns_f);
                    p.bx(4, floor + 2, z, 4, floor + 3, z, ns_f);
                }
                for x in 0..=4 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::TBalcony => {
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            p.bx(0, 0, 0, 8, 1, 8, nb);
            p.bx(0, 2, 0, 8, 5, 8, air);
            p.bx(0, 6, 0, 8, 6, 5, nb);
            p.bx(0, 2, 0, 2, 5, 0, nb);
            p.bx(6, 2, 0, 8, 5, 0, nb);
            p.bx(1, 3, 0, 1, 4, 0, we_f);
            p.bx(7, 3, 0, 7, 4, 0, we_f);
            p.bx(0, 2, 4, 8, 2, 8, nb);
            p.bx(1, 1, 4, 2, 2, 4, air);
            p.bx(6, 1, 4, 7, 2, 4, air);
            p.bx(1, 3, 8, 7, 3, 8, we_f);
            p.place(fence.set(b, "east", "true").set(b, "south", "true"), 0, 3, 8);
            p.place(fence.set(b, "west", "true").set(b, "south", "true"), 8, 3, 8);
            p.bx(0, 3, 6, 0, 3, 7, ns_f);
            p.bx(8, 3, 6, 8, 3, 7, ns_f);
            p.bx(0, 3, 4, 0, 5, 5, nb);
            p.bx(8, 3, 4, 8, 5, 5, nb);
            p.bx(1, 3, 5, 2, 5, 5, nb);
            p.bx(6, 3, 5, 7, 5, 5, nb);
            p.bx(1, 4, 5, 1, 5, 5, we_f);
            p.bx(7, 4, 5, 7, 5, 5, we_f);
            for z in 0..=5 {
                for x in 0..=8 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::Entrance => {
            p.bx(0, 3, 0, 12, 4, 12, nb);
            p.bx(0, 5, 0, 12, 13, 12, air);
            p.bx(0, 5, 0, 1, 12, 12, nb);
            p.bx(11, 5, 0, 12, 12, 12, nb);
            p.bx(2, 5, 11, 4, 12, 12, nb);
            p.bx(8, 5, 11, 10, 12, 12, nb);
            p.bx(5, 9, 11, 7, 12, 12, nb);
            p.bx(2, 5, 0, 4, 12, 1, nb);
            p.bx(8, 5, 0, 10, 12, 1, nb);
            p.bx(5, 9, 0, 7, 12, 1, nb);
            p.bx(2, 11, 2, 10, 12, 10, nb);
            p.bx(5, 8, 0, 7, 8, 0, fence);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            for i in (1..=11).step_by(2) {
                p.bx(i, 10, 0, i, 11, 0, we_f);
                p.bx(i, 10, 12, i, 11, 12, we_f);
                p.bx(0, 10, i, 0, 11, i, ns_f);
                p.bx(12, 10, i, 12, 11, i, ns_f);
                p.place(nb, i, 13, 0);
                p.place(nb, i, 13, 12);
                p.place(nb, 0, 13, i);
                p.place(nb, 12, 13, i);
                if i != 11 {
                    p.place(we_f, i + 1, 13, 0);
                    p.place(we_f, i + 1, 13, 12);
                    p.place(ns_f, 0, 13, i + 1);
                    p.place(ns_f, 12, 13, i + 1);
                }
            }
            p.place(fence.set(b, "north", "true").set(b, "east", "true"), 0, 13, 0);
            p.place(fence.set(b, "south", "true").set(b, "east", "true"), 0, 13, 12);
            p.place(fence.set(b, "south", "true").set(b, "west", "true"), 12, 13, 12);
            p.place(fence.set(b, "north", "true").set(b, "west", "true"), 12, 13, 0);
            for z in (3..=9).step_by(2) {
                p.fill(1, 7, z, 1, 8, z, ns_f.set(b, "west", "true"), ns_f.set(b, "west", "true"), false);
                p.fill(11, 7, z, 11, 8, z, ns_f.set(b, "east", "true"), ns_f.set(b, "east", "true"), false);
            }
            p.bx(4, 2, 0, 8, 2, 12, nb);
            p.bx(0, 2, 4, 12, 2, 8, nb);
            p.bx(4, 0, 0, 8, 1, 3, nb);
            p.bx(4, 0, 9, 8, 1, 12, nb);
            p.bx(0, 0, 4, 3, 1, 8, nb);
            p.bx(9, 0, 4, 12, 1, 8, nb);
            for x in 4..=8 {
                for z in 0..=2 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, x, -1, 12 - z);
                }
            }
            for x in 0..=2 {
                for z in 4..=8 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, 12 - x, -1, z);
                }
            }
            p.bx(5, 5, 5, 7, 5, 7, nb);
            p.bx(6, 1, 6, 6, 4, 6, air);
            p.place(nb, 6, 0, 6);
            p.place(st(b, "lava"), 6, 5, 6);
            let w = p.world(6, 5, 6);
            if p.zone.contains(w) {
                p.r.schedule_fluid_tick(w, st(b, "lava"));
            }
        }
        FKind::SmallCrossing => {
            p.bx(0, 0, 0, 4, 1, 4, nb);
            p.bx(0, 2, 0, 4, 5, 4, air);
            p.bx(0, 2, 0, 0, 5, 0, nb);
            p.bx(4, 2, 0, 4, 5, 0, nb);
            p.bx(0, 2, 4, 0, 5, 4, nb);
            p.bx(4, 2, 4, 4, 5, 4, nb);
            p.bx(0, 6, 0, 4, 6, 4, nb);
            for x in 0..=4 {
                for z in 0..=4 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::LeftTurn => {
            p.bx(0, 0, 0, 4, 1, 4, nb);
            p.bx(0, 2, 0, 4, 5, 4, air);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            p.bx(4, 2, 0, 4, 5, 4, nb);
            p.bx(4, 3, 1, 4, 4, 1, ns_f);
            p.bx(4, 3, 3, 4, 4, 3, ns_f);
            p.bx(0, 2, 0, 0, 5, 0, nb);
            p.bx(0, 2, 4, 3, 5, 4, nb);
            p.bx(1, 3, 4, 1, 4, 4, we_f);
            p.bx(3, 3, 4, 3, 4, 4, we_f);
            chest(&mut p, fp, rng, 3, 2, 3);
            p.bx(0, 6, 0, 4, 6, 4, nb);
            for x in 0..=4 {
                for z in 0..=4 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::SmallCorridor => {
            p.bx(0, 0, 0, 4, 1, 4, nb);
            p.bx(0, 2, 0, 4, 5, 4, air);
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            p.bx(0, 2, 0, 0, 5, 4, nb);
            p.bx(4, 2, 0, 4, 5, 4, nb);
            p.bx(0, 3, 1, 0, 4, 1, ns_f);
            p.bx(0, 3, 3, 0, 4, 3, ns_f);
            p.bx(4, 3, 1, 4, 4, 1, ns_f);
            p.bx(4, 3, 3, 4, 4, 3, ns_f);
            p.bx(0, 6, 0, 4, 6, 4, nb);
            for x in 0..=4 {
                for z in 0..=4 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::RightTurn => {
            p.bx(0, 0, 0, 4, 1, 4, nb);
            p.bx(0, 2, 0, 4, 5, 4, air);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            p.bx(0, 2, 0, 0, 5, 4, nb);
            p.bx(0, 3, 1, 0, 4, 1, ns_f);
            p.bx(0, 3, 3, 0, 4, 3, ns_f);
            p.bx(4, 2, 0, 4, 5, 0, nb);
            p.bx(1, 2, 4, 4, 5, 4, nb);
            p.bx(1, 3, 4, 1, 4, 4, we_f);
            p.bx(3, 3, 4, 3, 4, 4, we_f);
            chest(&mut p, fp, rng, 1, 2, 3);
            p.bx(0, 6, 0, 4, 6, 4, nb);
            for x in 0..=4 {
                for z in 0..=4 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::StalkRoom => {
            p.bx(0, 3, 0, 12, 4, 12, nb);
            p.bx(0, 5, 0, 12, 13, 12, air);
            p.bx(0, 5, 0, 1, 12, 12, nb);
            p.bx(11, 5, 0, 12, 12, 12, nb);
            p.bx(2, 5, 11, 4, 12, 12, nb);
            p.bx(8, 5, 11, 10, 12, 12, nb);
            p.bx(5, 9, 11, 7, 12, 12, nb);
            p.bx(2, 5, 0, 4, 12, 1, nb);
            p.bx(8, 5, 0, 10, 12, 1, nb);
            p.bx(5, 9, 0, 7, 12, 1, nb);
            p.bx(2, 11, 2, 10, 12, 10, nb);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            let nsw_f = ns_f.set(b, "west", "true");
            let nse_f = ns_f.set(b, "east", "true");
            for i in (1..=11).step_by(2) {
                p.bx(i, 10, 0, i, 11, 0, we_f);
                p.bx(i, 10, 12, i, 11, 12, we_f);
                p.bx(0, 10, i, 0, 11, i, ns_f);
                p.bx(12, 10, i, 12, 11, i, ns_f);
                p.place(nb, i, 13, 0);
                p.place(nb, i, 13, 12);
                p.place(nb, 0, 13, i);
                p.place(nb, 12, 13, i);
                if i != 11 {
                    p.place(we_f, i + 1, 13, 0);
                    p.place(we_f, i + 1, 13, 12);
                    p.place(ns_f, 0, 13, i + 1);
                    p.place(ns_f, 12, 13, i + 1);
                }
            }
            p.place(fence.set(b, "north", "true").set(b, "east", "true"), 0, 13, 0);
            p.place(fence.set(b, "south", "true").set(b, "east", "true"), 0, 13, 12);
            p.place(fence.set(b, "south", "true").set(b, "west", "true"), 12, 13, 12);
            p.place(fence.set(b, "north", "true").set(b, "west", "true"), 12, 13, 0);
            for z in (3..=9).step_by(2) {
                p.bx(1, 7, z, 1, 8, z, nsw_f);
                p.bx(11, 7, z, 11, 8, z, nse_f);
            }
            let stairs = st(b, "nether_brick_stairs").set(b, "facing", "north");
            for i in 0..=6 {
                let z = i + 4;
                for x in 5..=7 {
                    p.place(stairs, x, 5 + i, z);
                }
                if (5..=8).contains(&z) {
                    p.bx(5, 5, z, 7, i + 4, z, nb);
                } else if (9..=10).contains(&z) {
                    p.bx(5, 8, z, 7, i + 4, z, nb);
                }
                if i >= 1 {
                    p.bx(5, 6 + i, z, 7, 9 + i, z, air);
                }
            }
            for x in 5..=7 {
                p.place(stairs, x, 12, 11);
            }
            p.bx(5, 6, 7, 5, 7, 7, nse_f);
            p.bx(7, 6, 7, 7, 7, 7, nsw_f);
            p.bx(5, 13, 12, 7, 13, 12, air);
            p.bx(2, 5, 2, 3, 5, 3, nb);
            p.bx(2, 5, 9, 3, 5, 10, nb);
            p.bx(2, 5, 4, 2, 5, 8, nb);
            p.bx(9, 5, 2, 10, 5, 3, nb);
            p.bx(9, 5, 9, 10, 5, 10, nb);
            p.bx(10, 5, 4, 10, 5, 8, nb);
            let east_st = stairs.set(b, "facing", "east");
            let west_st = stairs.set(b, "facing", "west");
            p.place(west_st, 4, 5, 2);
            p.place(west_st, 4, 5, 3);
            p.place(west_st, 4, 5, 9);
            p.place(west_st, 4, 5, 10);
            p.place(east_st, 8, 5, 2);
            p.place(east_st, 8, 5, 3);
            p.place(east_st, 8, 5, 9);
            p.place(east_st, 8, 5, 10);
            p.fill(3, 4, 4, 4, 4, 8, st(b, "soul_sand"), st(b, "soul_sand"), false);
            p.fill(8, 4, 4, 9, 4, 8, st(b, "soul_sand"), st(b, "soul_sand"), false);
            p.fill(3, 5, 4, 4, 5, 8, st(b, "nether_wart"), st(b, "nether_wart"), false);
            p.fill(8, 5, 4, 9, 5, 8, st(b, "nether_wart"), st(b, "nether_wart"), false);
            p.bx(4, 2, 0, 8, 2, 12, nb);
            p.bx(0, 2, 4, 12, 2, 8, nb);
            p.bx(4, 0, 0, 8, 1, 3, nb);
            p.bx(4, 0, 9, 8, 1, 12, nb);
            p.bx(0, 0, 4, 3, 1, 8, nb);
            p.bx(9, 0, 4, 12, 1, 8, nb);
            for x in 4..=8 {
                for z in 0..=2 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, x, -1, 12 - z);
                }
            }
            for x in 0..=2 {
                for z in 4..=8 {
                    p.fill_column_down(nb, x, -1, z);
                    p.fill_column_down(nb, 12 - x, -1, z);
                }
            }
        }
        FKind::Throne => {
            p.bx(0, 2, 0, 6, 7, 7, air);
            p.bx(1, 0, 0, 5, 1, 7, nb);
            p.bx(1, 2, 1, 5, 2, 7, nb);
            p.bx(1, 3, 2, 5, 3, 7, nb);
            p.bx(1, 4, 3, 5, 4, 7, nb);
            p.bx(1, 2, 0, 1, 4, 2, nb);
            p.bx(5, 2, 0, 5, 4, 2, nb);
            p.bx(1, 5, 2, 1, 5, 3, nb);
            p.bx(5, 5, 2, 5, 5, 3, nb);
            p.bx(0, 5, 3, 0, 5, 8, nb);
            p.bx(6, 5, 3, 6, 5, 8, nb);
            p.bx(1, 5, 8, 5, 5, 8, nb);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            p.place(fence.set(b, "west", "true"), 1, 6, 3);
            p.place(fence.set(b, "east", "true"), 5, 6, 3);
            p.place(fence.set(b, "east", "true").set(b, "north", "true"), 0, 6, 3);
            p.place(fence.set(b, "west", "true").set(b, "north", "true"), 6, 6, 3);
            p.bx(0, 6, 4, 0, 6, 7, ns_f);
            p.bx(6, 6, 4, 6, 6, 7, ns_f);
            p.place(fence.set(b, "east", "true").set(b, "south", "true"), 0, 6, 8);
            p.place(fence.set(b, "west", "true").set(b, "south", "true"), 6, 6, 8);
            p.bx(1, 6, 8, 5, 6, 8, we_f);
            p.place(fence.set(b, "east", "true"), 1, 7, 8);
            p.bx(2, 7, 8, 4, 7, 8, we_f);
            p.place(fence.set(b, "west", "true"), 5, 7, 8);
            p.place(fence.set(b, "east", "true"), 2, 8, 8);
            p.place(we_f, 3, 8, 8);
            p.place(fence.set(b, "west", "true"), 4, 8, 8);
            let w = p.world(3, 5, 5);
            let mut done = fp.flag.lock().expect("piece");
            if !*done && p.zone.contains(w) {
                *done = true;
                p.r.set_p(w, st(b, "spawner"), 2);
                p.r.note_block_entity(w, BeData::Spawner { entity: "minecraft:blaze" });
            }
            drop(done);
            for x in 0..=6 {
                for z in 0..=6 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::RoomCrossing => {
            p.bx(0, 0, 0, 6, 1, 6, nb);
            p.bx(0, 2, 0, 6, 7, 6, air);
            p.bx(0, 2, 0, 1, 6, 0, nb);
            p.bx(0, 2, 6, 1, 6, 6, nb);
            p.bx(5, 2, 0, 6, 6, 0, nb);
            p.bx(5, 2, 6, 6, 6, 6, nb);
            p.bx(0, 2, 0, 0, 6, 1, nb);
            p.bx(0, 2, 5, 0, 6, 6, nb);
            p.bx(6, 2, 0, 6, 6, 1, nb);
            p.bx(6, 2, 5, 6, 6, 6, nb);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            p.bx(2, 6, 0, 4, 6, 0, nb);
            p.bx(2, 5, 0, 4, 5, 0, we_f);
            p.bx(2, 6, 6, 4, 6, 6, nb);
            p.bx(2, 5, 6, 4, 5, 6, we_f);
            p.bx(0, 6, 2, 0, 6, 4, nb);
            p.bx(0, 5, 2, 0, 5, 4, ns_f);
            p.bx(6, 6, 2, 6, 6, 4, nb);
            p.bx(6, 5, 2, 6, 5, 4, ns_f);
            for x in 0..=6 {
                for z in 0..=6 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
        FKind::StairsRoom => {
            p.bx(0, 0, 0, 6, 1, 6, nb);
            p.bx(0, 2, 0, 6, 10, 6, air);
            p.bx(0, 2, 0, 1, 8, 0, nb);
            p.bx(5, 2, 0, 6, 8, 0, nb);
            p.bx(0, 2, 1, 0, 8, 6, nb);
            p.bx(6, 2, 1, 6, 8, 6, nb);
            p.bx(1, 2, 6, 5, 8, 6, nb);
            let we_f = fence.set(b, "west", "true").set(b, "east", "true");
            let ns_f = fence.set(b, "north", "true").set(b, "south", "true");
            p.bx(0, 3, 2, 0, 5, 4, ns_f);
            p.bx(6, 3, 2, 6, 5, 2, ns_f);
            p.bx(6, 3, 4, 6, 5, 4, ns_f);
            p.place(nb, 5, 2, 5);
            p.bx(4, 2, 5, 4, 3, 5, nb);
            p.bx(3, 2, 5, 3, 4, 5, nb);
            p.bx(2, 2, 5, 2, 5, 5, nb);
            p.bx(1, 2, 5, 1, 6, 5, nb);
            p.bx(1, 7, 1, 5, 7, 4, nb);
            p.bx(6, 8, 2, 6, 8, 4, air);
            p.bx(2, 6, 0, 4, 8, 0, nb);
            p.bx(2, 5, 0, 4, 5, 0, we_f);
            for x in 0..=6 {
                for z in 0..=6 {
                    p.fill_column_down(nb, x, -1, z);
                }
            }
        }
    }
}
