//! Structures with coded pieces (`generation-decoration.md` § 8.6–8.14): start and assembly of the
//! pieces (boxes, orientations, drawn choices). Placement is in `structure_place.rs`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::decor_blocks::Dir;
use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece};
use crate::template::BBox;

/// Own data of a coded piece.
#[derive(Debug, Clone)]
pub enum CodedPiece {
    BuriedTreasure,
    MsRoom {
        mesa: bool,
        entrances: Vec<BBox>,
    },
    /// `spider_placed`: spider spawner already placed, shared by all the chunks the
    /// piece crosses (the first decorated one that places it wins; the following ones no longer draw).
    MsCorridor {
        mesa: bool,
        dir: Dir,
        rails: bool,
        spider: bool,
        sections: i32,
        spider_placed: Arc<AtomicBool>,
    },
    MsCrossing {
        mesa: bool,
        dir: Dir,
        two_floors: bool,
    },
    MsStairs {
        mesa: bool,
        dir: Dir,
    },
    Template(Arc<crate::structure_templ::TemplPiece>),
    Scattered(Arc<crate::structure_scattered::ScatPiece>),
    Stronghold(Arc<crate::structure_stronghold::ShPiece>),
    Fortress(Arc<crate::structure_fortress::FPiece>),
    Monument(Arc<crate::structure_monument::Monument>),
}

fn coded(id: &'static str, bbox: BBox, data: CodedPiece) -> Piece {
    Piece::Coded { id, bbox, data: Arc::new(data) }
}

// ---------------------------------------------------------------- Buried treasure (§ 8.14)

pub fn buried_treasure(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    let (x, z) = (16 * g.cx + 8, 16 * g.cz + 8);
    let y = g.first_free(x, z, true) - 1;
    let p = [16 * g.cx + 9, 90, 16 * g.cz + 9];
    Some(([x, y, z], vec![coded("buried_treasure", BBox::new(p, p), CodedPiece::BuriedTreasure)]))
}

// ---------------------------------------------------------------- Mineshaft (§ 8.6)

struct Ms {
    pieces: Vec<(BBox, CodedPiece, i32)>,
    mesa: bool,
    start_min: [i32; 3],
}

impl Ms {
    fn collides(&self, b: &BBox) -> bool {
        self.pieces.iter().any(|(p, _, _)| p.intersects(b))
    }

    fn add_children(&mut self, i: usize, r: &mut DecoRng) {
        let (bb, data, depth) = self.pieces[i].clone();
        match data {
            CodedPiece::MsRoom { .. } => {
                let hs = (bb.span(1) - 3 - 1).max(1);
                let sides: [(Dir, i32); 4] = [
                    (Dir::North, bb.span(0)),
                    (Dir::South, bb.span(0)),
                    (Dir::West, bb.span(2)),
                    (Dir::East, bb.span(2)),
                ];
                for (d, ext) in sides {
                    let mut pos = 0;
                    while pos < ext {
                        pos += r.next_int_bounded(ext);
                        if pos + 3 > ext {
                            break;
                        }
                        let y = bb.min[1] + r.next_int_bounded(hs) + 1;
                        let foot = match d {
                            Dir::North => [bb.min[0] + pos, y, bb.min[2] - 1],
                            Dir::South => [bb.min[0] + pos, y, bb.max[2] + 1],
                            Dir::West => [bb.min[0] - 1, y, bb.min[2] + pos],
                            _ => [bb.max[0] + 1, y, bb.min[2] + pos],
                        };
                        if let Some(c) = self.generate(foot, d, depth, r) {
                            let cb = self.pieces[c].0;
                            let ent = match d {
                                Dir::North => {
                                    BBox::new([cb.min[0], cb.min[1], bb.min[2]], [cb.max[0], cb.max[1], bb.min[2] + 1])
                                }
                                Dir::South => {
                                    BBox::new([cb.min[0], cb.min[1], bb.max[2] - 1], [cb.max[0], cb.max[1], bb.max[2]])
                                }
                                Dir::West => {
                                    BBox::new([bb.min[0], cb.min[1], cb.min[2]], [bb.min[0] + 1, cb.max[1], cb.max[2]])
                                }
                                _ => {
                                    BBox::new([bb.max[0] - 1, cb.min[1], cb.min[2]], [bb.max[0], cb.max[1], cb.max[2]])
                                }
                            };
                            if let CodedPiece::MsRoom { entrances, .. } = &mut self.pieces[i].1 {
                                entrances.push(ent);
                            }
                        }
                        pos += 4;
                    }
                }
            }
            CodedPiece::MsCorridor { dir, .. } => {
                let e = r.next_int_bounded(4);
                let y = |r: &mut DecoRng| bb.min[1] - 1 + r.next_int_bounded(3);
                let (foot, d) = match (dir, e) {
                    (Dir::South, 0 | 1) => ([bb.min[0], y(r), bb.max[2] + 1], dir),
                    (Dir::South, 2) => ([bb.min[0] - 1, y(r), bb.max[2] - 3], Dir::West),
                    (Dir::South, _) => ([bb.max[0] + 1, y(r), bb.max[2] - 3], Dir::East),
                    (Dir::West, 0 | 1) => ([bb.min[0] - 1, y(r), bb.min[2]], dir),
                    (Dir::West, 2) => ([bb.min[0], y(r), bb.min[2] - 1], Dir::North),
                    (Dir::West, _) => ([bb.min[0], y(r), bb.max[2] + 1], Dir::South),
                    (Dir::East, 0 | 1) => ([bb.max[0] + 1, y(r), bb.min[2]], dir),
                    (Dir::East, 2) => ([bb.max[0] - 3, y(r), bb.min[2] - 1], Dir::North),
                    (Dir::East, _) => ([bb.max[0] - 3, y(r), bb.max[2] + 1], Dir::South),
                    (_, 0 | 1) => ([bb.min[0], y(r), bb.min[2] - 1], dir),
                    (_, 2) => ([bb.min[0] - 1, y(r), bb.min[2]], Dir::West),
                    (_, _) => ([bb.max[0] + 1, y(r), bb.min[2]], Dir::East),
                };
                self.generate(foot, d, depth, r);
                if depth < 8 {
                    if dir != Dir::North && dir != Dir::South {
                        let mut x = bb.min[0] + 3;
                        while x + 3 <= bb.max[0] {
                            match r.next_int_bounded(5) {
                                0 => {
                                    self.generate([x, bb.min[1], bb.min[2] - 1], Dir::North, depth + 1, r);
                                }
                                1 => {
                                    self.generate([x, bb.min[1], bb.max[2] + 1], Dir::South, depth + 1, r);
                                }
                                _ => {}
                            }
                            x += 5;
                        }
                    } else {
                        let mut z = bb.min[2] + 3;
                        while z + 3 <= bb.max[2] {
                            match r.next_int_bounded(5) {
                                0 => {
                                    self.generate([bb.min[0] - 1, bb.min[1], z], Dir::West, depth + 1, r);
                                }
                                1 => {
                                    self.generate([bb.max[0] + 1, bb.min[1], z], Dir::East, depth + 1, r);
                                }
                                _ => {}
                            }
                            z += 5;
                        }
                    }
                }
            }
            CodedPiece::MsCrossing { dir, two_floors, .. } => {
                let (x0, y0, z0, x1, z1) = (bb.min[0], bb.min[1], bb.min[2], bb.max[0], bb.max[2]);
                let n = ([x0 + 1, y0, z0 - 1], Dir::North);
                let s = ([x0 + 1, y0, z1 + 1], Dir::South);
                let w = ([x0 - 1, y0, z0 + 1], Dir::West);
                let e = ([x1 + 1, y0, z0 + 1], Dir::East);
                let list = match dir {
                    Dir::South => [s, w, e],
                    Dir::West => [n, s, w],
                    Dir::East => [n, s, e],
                    _ => [n, w, e],
                };
                for (f, d) in list {
                    self.generate(f, d, depth, r);
                }
                if two_floors {
                    let y = y0 + 4;
                    for (f, d) in [
                        ([x0 + 1, y, z0 - 1], Dir::North),
                        ([x0 - 1, y, z0 + 1], Dir::West),
                        ([x1 + 1, y, z0 + 1], Dir::East),
                        ([x0 + 1, y, z1 + 1], Dir::South),
                    ] {
                        if r.next_bool() {
                            self.generate(f, d, depth, r);
                        }
                    }
                }
            }
            CodedPiece::MsStairs { dir, .. } => {
                let f = match dir {
                    Dir::South => [bb.min[0], bb.min[1], bb.max[2] + 1],
                    Dir::West => [bb.min[0] - 1, bb.min[1], bb.min[2]],
                    Dir::East => [bb.max[0] + 1, bb.min[1], bb.min[2]],
                    _ => [bb.min[0], bb.min[1], bb.min[2] - 1],
                };
                self.generate(f, dir, depth, r);
            }
            CodedPiece::BuriedTreasure
            | CodedPiece::Template(_)
            | CodedPiece::Scattered(_)
            | CodedPiece::Stronghold(_)
            | CodedPiece::Fortress(_)
            | CodedPiece::Monument(_) => {}
        }
    }

    /// Creation of a following piece (§ 8.6.2); returns its index.
    fn generate(&mut self, f: [i32; 3], d: Dir, depth: i32, r: &mut DecoRng) -> Option<usize> {
        if depth > 8 || (f[0] - self.start_min[0]).abs() > 80 || (f[2] - self.start_min[2]).abs() > 80 {
            return None;
        }
        let mesa = self.mesa;
        let sel = r.next_int_bounded(100);
        let mk = |a: [i32; 3], b: [i32; 3]| {
            BBox::new([a[0] + f[0], a[1] + f[1], a[2] + f[2]], [b[0] + f[0], b[1] + f[1], b[2] + f[2]])
        };
        let piece = if sel >= 80 {
            let y1 = if r.next_int_bounded(4) == 0 { 6 } else { 2 };
            let b = match d {
                Dir::South => mk([-1, 0, 0], [3, y1, 4]),
                Dir::West => mk([-4, 0, -1], [0, y1, 3]),
                Dir::East => mk([0, 0, -1], [4, y1, 3]),
                _ => mk([-1, 0, -4], [3, y1, 0]),
            };
            if self.collides(&b) {
                return None;
            }
            (b, CodedPiece::MsCrossing { mesa, dir: d, two_floors: b.span(1) > 3 })
        } else if sel >= 70 {
            let b = match d {
                Dir::South => mk([0, -5, 0], [2, 2, 8]),
                Dir::West => mk([-8, -5, 0], [0, 2, 2]),
                Dir::East => mk([0, -5, 0], [8, 2, 2]),
                _ => mk([0, -5, -8], [2, 2, 0]),
            };
            if self.collides(&b) {
                return None;
            }
            (b, CodedPiece::MsStairs { mesa, dir: d })
        } else {
            let mut found = None;
            let mut k = r.next_int_bounded(3) + 2;
            while k > 0 {
                let l = k * 5;
                let b = match d {
                    Dir::South => mk([0, 0, 0], [2, 2, l - 1]),
                    Dir::West => mk([-(l - 1), 0, 0], [0, 2, 2]),
                    Dir::East => mk([0, 0, 0], [l - 1, 2, 2]),
                    _ => mk([0, 0, -(l - 1)], [2, 2, 0]),
                };
                if !self.collides(&b) {
                    found = Some(b);
                    break;
                }
                k -= 1;
            }
            let b = found?;
            let rails = r.next_int_bounded(3) == 0;
            let spider = !rails && r.next_int_bounded(23) == 0;
            let sections = if matches!(d, Dir::North | Dir::South) { b.span(2) / 5 } else { b.span(0) / 5 };
            let spider_placed = Arc::new(AtomicBool::new(false));
            (b, CodedPiece::MsCorridor { mesa, dir: d, rails, spider, sections, spider_placed })
        };
        self.pieces.push((piece.0, piece.1, depth + 1));
        let i = self.pieces.len() - 1;
        self.add_children(i, r);
        Some(i)
    }
}

pub fn mineshaft(g: &mut GenStart, mesa: bool) -> Option<([i32; 3], Vec<Piece>)> {
    let r = &mut g.rng;
    r.next_double();
    let (x0, z0) = (16 * g.cx + 2, 16 * g.cz + 2);
    let room = BBox::new(
        [x0, 50, z0],
        [x0 + 7 + r.next_int_bounded(6), 54 + r.next_int_bounded(6), z0 + 7 + r.next_int_bounded(6)],
    );
    let mut ms =
        Ms { pieces: vec![(room, CodedPiece::MsRoom { mesa, entrances: Vec::new() }, 0)], mesa, start_min: room.min };
    ms.add_children(0, r);
    let mut all = ms.pieces[0].0;
    for p in &ms.pieces[1..] {
        all.encapsulate(&p.0);
    }
    let sea = g.t.gc.sea;
    let dy = if mesa {
        let c = all.center();
        let s = g.t.base_height(c[0], c[2], false);
        let target = if s <= sea { sea } else { sea + g.rng.next_int_bounded(s - sea + 1) };
        target - c[1]
    } else {
        let max_y = sea - 10;
        let mut y = all.span(1) + g.t.g.settings.min_y + 1;
        if y < max_y {
            y += g.rng.next_int_bounded(max_y - y);
        }
        y - all.max[1]
    };
    let pieces = ms
        .pieces
        .into_iter()
        .map(|(b, mut d, _)| {
            if let CodedPiece::MsRoom { entrances, .. } = &mut d {
                for e in entrances.iter_mut() {
                    *e = e.moved([0, dy, 0]);
                }
            }
            let id = match d {
                CodedPiece::MsRoom { .. } => "mineshaft_room",
                CodedPiece::MsCorridor { .. } => "mineshaft_corridor",
                CodedPiece::MsCrossing { .. } => "mineshaft_crossing",
                _ => "mineshaft_stairs",
            };
            coded(id, b.moved([0, dy, 0]), d)
        })
        .collect();
    Some(([16 * g.cx + 8, 50 + dy, 16 * g.cz], pieces))
}
