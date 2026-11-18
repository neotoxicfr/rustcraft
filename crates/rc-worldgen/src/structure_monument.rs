//! Ocean monument (`generation-decoration.md` § 8.9): a single piece (the building) that carries its
//! 5 × 5 × 3 room grid, drawn at creation (room graph, random closures as long
//! as the source stays reachable, assignment by room templates), then the wings and the
//! penthouse; placement per chunk (water, walls, rooms, elder guardians).

use std::sync::Arc;

use crate::decor::Ctx;
use crate::decor_blocks::{Dir, State};
use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece, StructureGen};
use crate::structure_coded::CodedPiece;
use crate::structure_piece::{P, st};
use crate::template::BBox;

const DOWN: usize = 0;
const UP: usize = 1;
const NORTH: usize = 2;
const SOUTH: usize = 3;
const WEST: usize = 4;
const EAST: usize = 5;
const STEPS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Room of the grid (or special: penthouse 1003, wings 1001 and 1002).
#[derive(Debug, Clone)]
pub struct Def {
    pub index: i32,
    pub conn: [Option<usize>; 6],
    pub open: [bool; 6],
    claimed: bool,
    source: bool,
    scan: i32,
}

impl Def {
    fn new(index: i32) -> Def {
        Def { index, conn: [None; 6], open: [false; 6], claimed: false, source: false, scan: 0 }
    }
}

fn room_index(x: i32, y: i32, z: i32) -> i32 {
    y * 25 + z * 5 + x
}

fn set_conn(g: &mut [Def], a: usize, d: usize, b: usize) {
    g[a].conn[d] = Some(b);
    g[b].conn[d ^ 1] = Some(a);
}

fn find_source(g: &mut [Def], i: usize, scan: i32) -> bool {
    if g[i].source {
        return true;
    }
    g[i].scan = scan;
    for f in 0..6 {
        if let Some(c) = g[i].conn[f]
            && g[i].open[f]
            && g[c].scan != scan
            && find_source(g, c, scan)
        {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RKind {
    Entry,
    Core,
    DoubleXY,
    DoubleYZ,
    DoubleZ,
    DoubleX,
    DoubleY,
    SimpleTop,
    Simple { design: i32 },
    Wing { design: i32 },
    Penthouse,
}

#[derive(Debug, Clone)]
pub struct Room {
    pub kind: RKind,
    pub bbox: BBox,
    /// Room of the grid (index into `Monument::graph`).
    pub def: usize,
}

#[derive(Debug)]
pub struct Monument {
    pub orient: Dir,
    pub graph: Vec<Def>,
    pub rooms: Vec<Room>,
}

/// Relative box of a grid room of `w × h × d` cells (§ 8.9.3).
fn room_box(o: Dir, index: i32, w: i32, h: i32, d: i32) -> BBox {
    let (rx, rz, ry) = (index % 5, index / 5 % 5, index / 25);
    let (sx, sy, sz) = (w * 8, h * 4, d * 8);
    let b = if matches!(o, Dir::North | Dir::South) {
        BBox::new([0, 0, 0], [sx - 1, sy - 1, sz - 1])
    } else {
        BBox::new([0, 0, 0], [sz - 1, sy - 1, sx - 1])
    };
    let m = match o {
        Dir::North => [rx * 8, ry * 4, -(rz + d) * 8 + 1],
        Dir::South => [rx * 8, ry * 4, rz * 8],
        Dir::West => [-(rz + d) * 8 + 1, ry * 4, rx * 8],
        _ => [rz * 8, ry * 4, rx * 8],
    };
    b.moved(m)
}

/// Room graph (§ 8.9.3): returns (graph, shuffled list + specials, source, core).
fn room_graph(r: &mut DecoRng) -> (Vec<Def>, Vec<usize>, usize, usize) {
    let mut g: Vec<Def> = Vec::new();
    let mut grid: [Option<usize>; 75] = [None; 75];
    let make = |g: &mut Vec<Def>, grid: &mut [Option<usize>; 75], x: i32, y: i32, z: i32| {
        let i = room_index(x, y, z);
        g.push(Def::new(i));
        grid[i as usize] = Some(g.len() - 1);
    };
    for x in 0..5 {
        for z in 0..4 {
            make(&mut g, &mut grid, x, 0, z);
        }
    }
    for x in 0..5 {
        for z in 0..4 {
            make(&mut g, &mut grid, x, 1, z);
        }
    }
    for x in 1..4 {
        for z in 0..2 {
            make(&mut g, &mut grid, x, 2, z);
        }
    }
    let source = grid[room_index(2, 0, 0) as usize].expect("source");
    for x in 0..5 {
        for z in 0..5 {
            for y in 0..3 {
                let Some(a) = grid[room_index(x, y, z) as usize] else { continue };
                for (d, s) in STEPS.iter().enumerate() {
                    let (nx, ny, nz) = (x + s[0], y + s[1], z + s[2]);
                    if !(0..5).contains(&nx) || !(0..5).contains(&nz) || !(0..3).contains(&ny) {
                        continue;
                    }
                    if let Some(b) = grid[room_index(nx, ny, nz) as usize] {
                        // The game swaps north and south between neighbouring rooms in z.
                        set_conn(&mut g, a, if nz == z { d } else { d ^ 1 }, b);
                    }
                }
            }
        }
    }
    g.push(Def::new(1003));
    let roof = g.len() - 1;
    g.push(Def::new(1001));
    let left = g.len() - 1;
    g.push(Def::new(1002));
    let right = g.len() - 1;
    set_conn(&mut g, grid[room_index(2, 2, 0) as usize].expect("roof"), UP, roof);
    set_conn(&mut g, grid[room_index(0, 1, 0) as usize].expect("wing"), SOUTH, left);
    set_conn(&mut g, grid[room_index(4, 1, 0) as usize].expect("wing"), SOUTH, right);
    for i in [roof, left, right] {
        g[i].claimed = true;
    }
    g[source].source = true;
    let core = grid[room_index(r.next_int_bounded(4), 0, 2) as usize].expect("core");
    let c = |g: &Vec<Def>, i: usize, d: usize| g[i].conn[d].expect("neighbouring room");
    g[core].claimed = true;
    let (e, n, u) = (c(&g, core, EAST), c(&g, core, NORTH), c(&g, core, UP));
    let en = c(&g, e, NORTH);
    for i in [e, n, en, u, c(&g, e, UP), c(&g, n, UP), c(&g, en, UP)] {
        g[i].claimed = true;
    }
    let mut defs: Vec<usize> = Vec::new();
    for i in grid.iter().flatten() {
        let o = g[*i].conn.map(|c| c.is_some());
        g[*i].open = o;
        defs.push(*i);
    }
    g[roof].open = g[roof].conn.map(|c| c.is_some());
    r.shuffle(&mut defs);
    let mut scan = 1;
    for &d in &defs {
        let (mut closed, mut tries) = (0, 0);
        while closed < 2 && tries < 5 {
            tries += 1;
            let f = r.next_int_bounded(6) as usize;
            if g[d].open[f] {
                let o = f ^ 1;
                let n = g[d].conn[f].expect("opening");
                g[d].open[f] = false;
                g[n].open[o] = false;
                let a = find_source(&mut g, d, scan);
                scan += 1;
                let ok = a && {
                    let b = find_source(&mut g, n, scan);
                    scan += 1;
                    b
                };
                if ok {
                    closed += 1;
                } else {
                    g[d].open[f] = true;
                    g[n].open[o] = true;
                }
            }
        }
    }
    defs.extend([roof, left, right]);
    (g, defs, source, core)
}

/// Adapted room template (game order); claims the covered rooms.
fn fit(g: &mut [Def], d: usize, o: Dir, r: &mut DecoRng) -> Room {
    let cl = |g: &[Def], i: usize, f: usize| g[i].conn[f].is_some_and(|c| g[c].claimed);
    let conn = |g: &[Def], i: usize, f: usize| g[i].conn[f].expect("neighbouring room");
    let idx = g[d].index;
    let open = g[d].open;
    if open[EAST] && !cl(g, d, EAST) && open[UP] && !cl(g, d, UP) {
        let e = conn(g, d, EAST);
        if g[e].open[UP] && !cl(g, e, UP) {
            let (u, eu) = (conn(g, d, UP), conn(g, e, UP));
            for i in [d, e, u, eu] {
                g[i].claimed = true;
            }
            return Room { kind: RKind::DoubleXY, bbox: room_box(o, idx, 2, 2, 1), def: d };
        }
    }
    if open[NORTH] && !cl(g, d, NORTH) && open[UP] && !cl(g, d, UP) {
        let n = conn(g, d, NORTH);
        if g[n].open[UP] && !cl(g, n, UP) {
            let (u, nu) = (conn(g, d, UP), conn(g, n, UP));
            for i in [d, n, u, nu] {
                g[i].claimed = true;
            }
            return Room { kind: RKind::DoubleYZ, bbox: room_box(o, idx, 1, 2, 2), def: d };
        }
    }
    if open[NORTH] && !cl(g, d, NORTH) {
        let n = conn(g, d, NORTH);
        g[d].claimed = true;
        g[n].claimed = true;
        return Room { kind: RKind::DoubleZ, bbox: room_box(o, idx, 1, 1, 2), def: d };
    }
    if open[EAST] && !cl(g, d, EAST) {
        let e = conn(g, d, EAST);
        g[d].claimed = true;
        g[e].claimed = true;
        return Room { kind: RKind::DoubleX, bbox: room_box(o, idx, 2, 1, 1), def: d };
    }
    if open[UP] && !cl(g, d, UP) {
        let u = conn(g, d, UP);
        g[d].claimed = true;
        g[u].claimed = true;
        return Room { kind: RKind::DoubleY, bbox: room_box(o, idx, 1, 2, 1), def: d };
    }
    g[d].claimed = true;
    if !open[WEST] && !open[EAST] && !open[NORTH] && !open[SOUTH] && !open[UP] {
        return Room { kind: RKind::SimpleTop, bbox: room_box(o, idx, 1, 1, 1), def: d };
    }
    let design = r.next_int_bounded(3);
    Room { kind: RKind::Simple { design }, bbox: room_box(o, idx, 1, 1, 1), def: d }
}

/// World position of a local point of the building.
fn local(b: &BBox, o: Dir, x: i32, y: i32, z: i32) -> [i32; 3] {
    match o {
        Dir::North => [b.min[0] + x, y + b.min[1], b.max[2] - z],
        Dir::South => [b.min[0] + x, y + b.min[1], b.min[2] + z],
        Dir::West => [b.max[0] - z, y + b.min[1], b.min[2] + x],
        _ => [b.min[0] + z, y + b.min[1], b.min[2] + x],
    }
}

fn corners(a: [i32; 3], b: [i32; 3]) -> BBox {
    BBox::new([a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])], [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])])
}

pub fn monument(g: &mut GenStart, around: &[bool]) -> Option<([i32; 3], Vec<Piece>)> {
    let (x9, z9) = (16 * g.cx + 9, 16 * g.cz + 9);
    let sea = g.t.gc.sea;
    // All the biomes within 29 blocks around (cube of quarts) must surround a monument.
    let (q0, q1) = ((x9 - 29) >> 2, (x9 + 29) >> 2);
    let (r0, r1) = ((z9 - 29) >> 2, (z9 + 29) >> 2);
    let (y0, y1) = ((sea - 29) >> 2, (sea + 29) >> 2);
    let mut ctx = crate::sampler::Ctx::cached();
    let mut s = crate::biome::Searcher::default();
    for qz in r0..=r1 {
        for qx in q0..=q1 {
            for qy in y0..=y1 {
                let b = g.t.source.biome_at(&g.t.g, &mut ctx, &mut s, qx, qy, qz);
                if !around.get(b as usize).copied().unwrap_or(false) {
                    return None;
                }
            }
        }
    }
    let (cx8, cz8) = (16 * g.cx + 8, 16 * g.cz + 8);
    let pos = [cx8, g.first_free(cx8, cz8, true) - 1, cz8];
    let r = &mut g.rng;
    let o = [Dir::North, Dir::East, Dir::South, Dir::West][r.next_int_bounded(4) as usize];
    let (west, north) = (16 * g.cx - 29, 16 * g.cz - 29);
    let bbox = BBox::new([west, 39, north], [west + 57, 61, north + 57]);
    let (mut graph, defs, source, core) = room_graph(r);
    graph[source].claimed = true;
    let mut rooms = vec![
        Room { kind: RKind::Entry, bbox: room_box(o, graph[source].index, 1, 1, 1), def: source },
        Room { kind: RKind::Core, bbox: room_box(o, graph[core].index, 2, 2, 2), def: core },
    ];
    for d in defs {
        if !graph[d].claimed && graph[d].index < 75 {
            rooms.push(fit(&mut graph, d, o, r));
        }
    }
    let off = local(&bbox, o, 9, 0, 22);
    for room in &mut rooms {
        room.bbox = room.bbox.moved(off);
    }
    let left = corners(local(&bbox, o, 1, 1, 1), local(&bbox, o, 23, 8, 21));
    let right = corners(local(&bbox, o, 34, 1, 1), local(&bbox, o, 56, 8, 21));
    let pent = corners(local(&bbox, o, 22, 13, 22), local(&bbox, o, 35, 17, 35));
    let w = r.next_int();
    rooms.push(Room { kind: RKind::Wing { design: w & 1 }, bbox: left, def: 0 });
    rooms.push(Room { kind: RKind::Wing { design: w.wrapping_add(1) & 1 }, bbox: right, def: 0 });
    rooms.push(Room { kind: RKind::Penthouse, bbox: pent, def: 0 });
    let m = Monument { orient: o, graph, rooms };
    let piece = Piece::Coded { id: "monument", bbox, data: Arc::new(CodedPiece::Monument(Arc::new(m))) };
    Some((pos, vec![piece]))
}

// ---------------------------------------------------------------- Placement

/// Primitives specific to the monument.
trait Mon {
    #[allow(clippy::too_many_arguments)]
    fn generate_water_box(&mut self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32);
    #[allow(clippy::too_many_arguments)]
    fn generate_box_on_fill_only(&mut self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, s: State);
    fn generate_default_floor(&mut self, xo: i32, zo: i32, down: bool);
    fn chunk_intersects(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> bool;
    fn elder(&mut self, x: i32, y: i32, z: i32);
}

impl Mon for P<'_, '_> {
    fn generate_water_box(&mut self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) {
        let b = self.b;
        let water = b.default_state(b.k.water);
        let air = b.default_state(b.k.air);
        let keep: [Option<u16>; 4] =
            [b.block_id("ice"), b.block_id("packed_ice"), b.block_id("blue_ice"), Some(b.k.water)];
        let sea = self.r.sea_level;
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    let s = self.get(x, y, z);
                    if keep.contains(&Some(b.block(s))) {
                        continue;
                    }
                    if y + self.bbox.min[1] >= sea && s != water {
                        self.place(air, x, y, z);
                    } else {
                        self.place(water, x, y, z);
                    }
                }
            }
        }
    }

    fn generate_box_on_fill_only(&mut self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, s: State) {
        let water = self.b.default_state(self.b.k.water);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if self.get(x, y, z) == water {
                        self.place(s, x, y, z);
                    }
                }
            }
        }
    }

    fn generate_default_floor(&mut self, xo: i32, zo: i32, down: bool) {
        let gray = st(self.b, "prismarine");
        let light = st(self.b, "prismarine_bricks");
        if down {
            self.bx(xo, 0, zo, xo + 2, 0, zo + 7, gray);
            self.bx(xo + 5, 0, zo, xo + 7, 0, zo + 7, gray);
            self.bx(xo + 3, 0, zo, xo + 4, 0, zo + 2, gray);
            self.bx(xo + 3, 0, zo + 5, xo + 4, 0, zo + 7, gray);
            self.bx(xo + 3, 0, zo + 2, xo + 4, 0, zo + 2, light);
            self.bx(xo + 3, 0, zo + 5, xo + 4, 0, zo + 5, light);
            self.bx(xo + 2, 0, zo + 3, xo + 2, 0, zo + 4, light);
            self.bx(xo + 5, 0, zo + 3, xo + 5, 0, zo + 4, light);
        } else {
            self.bx(xo, 0, zo, xo + 7, 0, zo + 7, gray);
        }
    }

    fn chunk_intersects(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> bool {
        let a = self.world(x0, 0, z0);
        let c = self.world(x1, 0, z1);
        self.zone.intersects_xz(a[0].min(c[0]), a[2].min(c[2]), a[0].max(c[0]), a[2].max(c[2]))
    }

    fn elder(&mut self, x: i32, y: i32, z: i32) {
        let w = self.world(x, y, z);
        if self.zone.contains(w) {
            // Elder guardian (persistent from the start) finalised by the game at placement (hand, follow
            // bonus): done by the server at its first load.
            let e = crate::structure_entities::with(
                crate::structure_entities::mob(
                    "minecraft:elder_guardian",
                    [f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5],
                    0.0,
                    true,
                ),
                vec![("rc_finalize_structure".into(), rc_proto::nbt::Nbt::Byte(1))],
            );
            self.r.add_entity(w[0], w[2], e);
        }
    }
}

pub fn place(cx: &mut Ctx, sg: &StructureGen, m: &Monument, bbox: BBox, rng: &mut DecoRng, zone: BBox) {
    let tr = &sg.templates.transforms;
    {
        let mut p = P::new(cx.r, tr, zone, bbox, Some(m.orient));
        building(&mut p, rng);
    }
    for room in &m.rooms {
        if room.bbox.intersects(&zone) {
            let mut p = P::new(cx.r, tr, zone, room.bbox, Some(m.orient));
            place_room(&mut p, m, room, rng);
        }
    }
}

fn building(p: &mut P, rng: &mut DecoRng) {
    let _ = rng;
    let b = p.b;
    let gray = st(b, "prismarine");
    let light = st(b, "prismarine_bricks");
    let black = st(b, "dark_prismarine");
    let lamp = st(b, "sea_lantern");
    let water_height = p.r.sea_level.max(64) - p.bbox.min[1];
    p.generate_water_box(0, 0, 0, 58, water_height, 58);
    wing(p, false, 0);
    wing(p, true, 33);
    if p.chunk_intersects(22, 5, 35, 17) {
        p.generate_water_box(25, 0, 0, 32, 8, 20);
        for i in 0..4 {
            p.bx(24, 2, 5 + i * 4, 24, 4, 5 + i * 4, light);
            p.bx(22, 4, 5 + i * 4, 23, 4, 5 + i * 4, light);
            p.place(light, 25, 5, 5 + i * 4);
            p.place(light, 26, 6, 5 + i * 4);
            p.place(lamp, 26, 5, 5 + i * 4);
            p.bx(33, 2, 5 + i * 4, 33, 4, 5 + i * 4, light);
            p.bx(34, 4, 5 + i * 4, 35, 4, 5 + i * 4, light);
            p.place(light, 32, 5, 5 + i * 4);
            p.place(light, 31, 6, 5 + i * 4);
            p.place(lamp, 31, 5, 5 + i * 4);
            p.bx(27, 6, 5 + i * 4, 30, 6, 5 + i * 4, gray);
        }
    }
    if p.chunk_intersects(15, 20, 42, 21) {
        p.bx(15, 0, 21, 42, 0, 21, gray);
        p.generate_water_box(26, 1, 21, 31, 3, 21);
        p.bx(21, 12, 21, 36, 12, 21, gray);
        p.bx(17, 11, 21, 40, 11, 21, gray);
        p.bx(16, 10, 21, 41, 10, 21, gray);
        p.bx(15, 7, 21, 42, 9, 21, gray);
        p.bx(16, 6, 21, 41, 6, 21, gray);
        p.bx(17, 5, 21, 40, 5, 21, gray);
        p.bx(21, 4, 21, 36, 4, 21, gray);
        p.bx(22, 3, 21, 26, 3, 21, gray);
        p.bx(31, 3, 21, 35, 3, 21, gray);
        p.bx(23, 2, 21, 25, 2, 21, gray);
        p.bx(32, 2, 21, 34, 2, 21, gray);
        p.bx(28, 4, 20, 29, 4, 21, light);
        p.place(light, 27, 3, 21);
        p.place(light, 30, 3, 21);
        p.place(light, 26, 2, 21);
        p.place(light, 31, 2, 21);
        p.place(light, 25, 1, 21);
        p.place(light, 32, 1, 21);
        for i in 0..7 {
            p.place(black, 28 - i, 6 + i, 21);
            p.place(black, 29 + i, 6 + i, 21);
        }
        for i in 0..4 {
            p.place(black, 28 - i, 9 + i, 21);
            p.place(black, 29 + i, 9 + i, 21);
        }
        p.place(black, 28, 12, 21);
        p.place(black, 29, 12, 21);
        for i in 0..3 {
            p.place(black, 22 - i * 2, 8, 21);
            p.place(black, 22 - i * 2, 9, 21);
            p.place(black, 35 + i * 2, 8, 21);
            p.place(black, 35 + i * 2, 9, 21);
        }
        p.generate_water_box(15, 13, 21, 42, 15, 21);
        p.generate_water_box(15, 1, 21, 15, 6, 21);
        p.generate_water_box(16, 1, 21, 16, 5, 21);
        p.generate_water_box(17, 1, 21, 20, 4, 21);
        p.generate_water_box(21, 1, 21, 21, 3, 21);
        p.generate_water_box(22, 1, 21, 22, 2, 21);
        p.generate_water_box(23, 1, 21, 24, 1, 21);
        p.generate_water_box(42, 1, 21, 42, 6, 21);
        p.generate_water_box(41, 1, 21, 41, 5, 21);
        p.generate_water_box(37, 1, 21, 40, 4, 21);
        p.generate_water_box(36, 1, 21, 36, 3, 21);
        p.generate_water_box(33, 1, 21, 34, 1, 21);
        p.generate_water_box(35, 1, 21, 35, 2, 21);
    }
    if p.chunk_intersects(21, 21, 36, 36) {
        p.bx(21, 0, 22, 36, 0, 36, gray);
        p.generate_water_box(21, 1, 22, 36, 23, 36);
        for i in 0..4 {
            p.bx(21 + i, 13 + i, 21 + i, 36 - i, 13 + i, 21 + i, light);
            p.bx(21 + i, 13 + i, 36 - i, 36 - i, 13 + i, 36 - i, light);
            p.bx(21 + i, 13 + i, 22 + i, 21 + i, 13 + i, 35 - i, light);
            p.bx(36 - i, 13 + i, 22 + i, 36 - i, 13 + i, 35 - i, light);
        }
        p.bx(25, 16, 25, 32, 16, 32, gray);
        p.bx(25, 17, 25, 25, 19, 25, light);
        p.bx(32, 17, 25, 32, 19, 25, light);
        p.bx(25, 17, 32, 25, 19, 32, light);
        p.bx(32, 17, 32, 32, 19, 32, light);
        p.place(light, 26, 20, 26);
        p.place(light, 27, 21, 27);
        p.place(lamp, 27, 20, 27);
        p.place(light, 26, 20, 31);
        p.place(light, 27, 21, 30);
        p.place(lamp, 27, 20, 30);
        p.place(light, 31, 20, 31);
        p.place(light, 30, 21, 30);
        p.place(lamp, 30, 20, 30);
        p.place(light, 31, 20, 26);
        p.place(light, 30, 21, 27);
        p.place(lamp, 30, 20, 27);
        p.bx(28, 21, 27, 29, 21, 27, gray);
        p.bx(27, 21, 28, 27, 21, 29, gray);
        p.bx(28, 21, 30, 29, 21, 30, gray);
        p.bx(30, 21, 28, 30, 21, 29, gray);
    }
    if p.chunk_intersects(0, 21, 6, 58) {
        p.bx(0, 0, 21, 6, 0, 57, gray);
        p.generate_water_box(0, 1, 21, 6, 7, 57);
        p.bx(4, 4, 21, 6, 4, 53, gray);
        for i in 0..4 {
            p.bx(i, i + 1, 21, i, i + 1, 57 - i, light);
        }
        for z in (23..53).step_by(3) {
            p.place(light, 5, 5, z);
        }
        p.place(light, 5, 5, 52);
        for i in 0..4 {
            p.bx(i, i + 1, 21, i, i + 1, 57 - i, light);
        }
        p.bx(4, 1, 52, 6, 3, 52, gray);
        p.bx(5, 1, 51, 5, 3, 53, gray);
    }
    if p.chunk_intersects(51, 21, 58, 58) {
        p.bx(51, 0, 21, 57, 0, 57, gray);
        p.generate_water_box(51, 1, 21, 57, 7, 57);
        p.bx(51, 4, 21, 53, 4, 53, gray);
        for i in 0..4 {
            p.bx(57 - i, i + 1, 21, 57 - i, i + 1, 57 - i, light);
        }
        for z in (23..53).step_by(3) {
            p.place(light, 52, 5, z);
        }
        p.place(light, 52, 5, 52);
        p.bx(51, 1, 52, 53, 3, 52, gray);
        p.bx(52, 1, 51, 52, 3, 53, gray);
    }
    if p.chunk_intersects(0, 51, 57, 57) {
        p.bx(7, 0, 51, 50, 0, 57, gray);
        p.generate_water_box(7, 1, 51, 50, 10, 57);
        for i in 0..4 {
            p.bx(i + 1, i + 1, 57 - i, 56 - i, i + 1, 57 - i, light);
        }
    }
    if p.chunk_intersects(7, 21, 13, 50) {
        p.bx(7, 0, 21, 13, 0, 50, gray);
        p.generate_water_box(7, 1, 21, 13, 10, 50);
        p.bx(11, 8, 21, 13, 8, 53, gray);
        for i in 0..4 {
            p.bx(i + 7, i + 5, 21, i + 7, i + 5, 54, light);
        }
        for z in (21..=45).step_by(3) {
            p.place(light, 12, 9, z);
        }
    }
    if p.chunk_intersects(44, 21, 50, 54) {
        p.bx(44, 0, 21, 50, 0, 50, gray);
        p.generate_water_box(44, 1, 21, 50, 10, 50);
        p.bx(44, 8, 21, 46, 8, 53, gray);
        for i in 0..4 {
            p.bx(50 - i, i + 5, 21, 50 - i, i + 5, 54, light);
        }
        for z in (21..=45).step_by(3) {
            p.place(light, 45, 9, z);
        }
    }
    if p.chunk_intersects(8, 44, 49, 54) {
        p.bx(14, 0, 44, 43, 0, 50, gray);
        p.generate_water_box(14, 1, 44, 43, 10, 50);
        for x in (12..=45).step_by(3) {
            p.place(light, x, 9, 45);
            p.place(light, x, 9, 52);
            if x == 12 || x == 18 || x == 24 || x == 33 || x == 39 || x == 45 {
                p.place(light, x, 9, 47);
                p.place(light, x, 9, 50);
                p.place(light, x, 10, 45);
                p.place(light, x, 10, 46);
                p.place(light, x, 10, 51);
                p.place(light, x, 10, 52);
                p.place(light, x, 11, 47);
                p.place(light, x, 11, 50);
                p.place(light, x, 12, 48);
                p.place(light, x, 12, 49);
            }
        }
        for i in 0..3 {
            p.bx(8 + i, 5 + i, 54, 49 - i, 5 + i, 54, gray);
        }
        p.bx(11, 8, 54, 46, 8, 54, light);
        p.bx(14, 8, 44, 43, 8, 53, gray);
    }
    if p.chunk_intersects(14, 21, 20, 43) {
        p.bx(14, 0, 21, 20, 0, 43, gray);
        p.generate_water_box(14, 1, 22, 20, 14, 43);
        p.bx(18, 12, 22, 20, 12, 39, gray);
        p.bx(18, 12, 21, 20, 12, 21, light);
        for i in 0..4 {
            p.bx(i + 14, i + 9, 21, i + 14, i + 9, 43 - i, light);
        }
        for z in (23..=39).step_by(3) {
            p.place(light, 19, 13, z);
        }
    }
    if p.chunk_intersects(37, 21, 43, 43) {
        p.bx(37, 0, 21, 43, 0, 43, gray);
        p.generate_water_box(37, 1, 22, 43, 14, 43);
        p.bx(37, 12, 22, 39, 12, 39, gray);
        p.bx(37, 12, 21, 39, 12, 21, light);
        for i in 0..4 {
            p.bx(43 - i, i + 9, 21, 43 - i, i + 9, 43 - i, light);
        }
        for z in (23..=39).step_by(3) {
            p.place(light, 38, 13, z);
        }
    }
    if p.chunk_intersects(15, 37, 42, 43) {
        p.bx(21, 0, 37, 36, 0, 43, gray);
        p.generate_water_box(21, 1, 37, 36, 14, 43);
        p.bx(21, 12, 37, 36, 12, 39, gray);
        for i in 0..4 {
            p.bx(15 + i, i + 9, 43 - i, 42 - i, i + 9, 43 - i, light);
        }
        for x in (21..=36).step_by(3) {
            p.place(light, x, 13, 38);
        }
    }
    for pillar_x in 0..7 {
        let mut pillar_z = 0;
        while pillar_z < 7 {
            if pillar_z == 0 && pillar_x == 3 {
                pillar_z = 6;
            }
            let bx = pillar_x * 9;
            let bz = pillar_z * 9;
            for w in 0..4 {
                for d in 0..4 {
                    p.place(light, bx + w, 0, bz + d);
                    p.fill_column_down(light, bx + w, -1, bz + d);
                }
            }
            if pillar_x != 0 && pillar_x != 6 {
                pillar_z += 6;
            } else {
                pillar_z += 1;
            }
        }
    }
    for i in 0..5 {
        p.generate_water_box(-1 - i, i * 2, -1 - i, -1 - i, 23, 58 + i);
        p.generate_water_box(58 + i, i * 2, -1 - i, 58 + i, 23, 58 + i);
        p.generate_water_box(-i, i * 2, -1 - i, 57 + i, 23, -1 - i);
        p.generate_water_box(-i, i * 2, 58 + i, 57 + i, 23, 58 + i);
    }
}

fn wing(p: &mut P, flipped: bool, xoff: i32) {
    let b = p.b;
    let gray = st(b, "prismarine");
    let light = st(b, "prismarine_bricks");
    if p.chunk_intersects(xoff, 0, xoff + 23, 20) {
        p.bx(xoff, 0, 0, xoff + 24, 0, 20, gray);
        p.generate_water_box(xoff, 1, 0, xoff + 24, 10, 20);
        for i in 0..4 {
            p.bx(xoff + i, i + 1, i, xoff + i, i + 1, 20, light);
            p.bx(xoff + i + 7, i + 5, i + 7, xoff + i + 7, i + 5, 20, light);
            p.bx(xoff + 17 - i, i + 5, i + 7, xoff + 17 - i, i + 5, 20, light);
            p.bx(xoff + 24 - i, i + 1, i, xoff + 24 - i, i + 1, 20, light);
            p.bx(xoff + i + 1, i + 1, i, xoff + 23 - i, i + 1, i, light);
            p.bx(xoff + i + 8, i + 5, i + 7, xoff + 16 - i, i + 5, i + 7, light);
        }
        p.bx(xoff + 4, 4, 4, xoff + 6, 4, 20, gray);
        p.bx(xoff + 7, 4, 4, xoff + 17, 4, 6, gray);
        p.bx(xoff + 18, 4, 4, xoff + 20, 4, 20, gray);
        p.bx(xoff + 11, 8, 11, xoff + 13, 8, 20, gray);
        p.place(light, xoff + 12, 9, 12);
        p.place(light, xoff + 12, 9, 15);
        p.place(light, xoff + 12, 9, 18);
        let left_pos = xoff + if flipped { 19 } else { 5 };
        let right_pos = xoff + if flipped { 5 } else { 19 };
        for z in (5..=20).rev().step_by(3) {
            p.place(light, left_pos, 5, z);
        }
        for z in (7..=19).rev().step_by(3) {
            p.place(light, right_pos, 5, z);
        }
        for i in 0..4 {
            let pos = if flipped { xoff + 24 - (17 - i * 3) } else { xoff + 17 - i * 3 };
            p.place(light, pos, 5, 5);
        }
        p.place(light, right_pos, 5, 5);
        p.bx(xoff + 11, 1, 12, xoff + 13, 7, 12, gray);
        p.bx(xoff + 12, 1, 11, xoff + 12, 7, 13, gray);
    }
}

fn place_room(p: &mut P, m: &Monument, room: &Room, rng: &mut DecoRng) {
    let b = p.b;
    let gr = &m.graph;
    let rd = room.def;
    let gray = st(b, "prismarine");
    let light = st(b, "prismarine_bricks");
    let black = st(b, "dark_prismarine");
    let lamp = st(b, "sea_lantern");
    let _ = (gr, rd, black, lamp, gray, light);
    match room.kind {
        RKind::Core => {
            p.generate_box_on_fill_only(1, 8, 0, 14, 8, 14, gray);
            let mut block = light;
            p.bx(0, 7, 0, 0, 7, 15, block);
            p.bx(15, 7, 0, 15, 7, 15, block);
            p.bx(1, 7, 0, 15, 7, 0, block);
            p.bx(1, 7, 15, 14, 7, 15, block);
            for yx in 1..=6 {
                block = light;
                if yx == 2 || yx == 6 {
                    block = gray;
                }
                for x in (0..=15).step_by(15) {
                    p.bx(x, yx, 0, x, yx, 1, block);
                    p.bx(x, yx, 6, x, yx, 9, block);
                    p.bx(x, yx, 14, x, yx, 15, block);
                }
                p.bx(1, yx, 0, 1, yx, 0, block);
                p.bx(6, yx, 0, 9, yx, 0, block);
                p.bx(14, yx, 0, 14, yx, 0, block);
                p.bx(1, yx, 15, 14, yx, 15, block);
            }
            p.bx(6, 3, 6, 9, 6, 9, black);
            p.bx(7, 4, 7, 8, 5, 8, st(b, "gold_block"));
            for yx in (3..=6).step_by(3) {
                for x in (6..=9).step_by(3) {
                    p.place(lamp, x, yx, 6);
                    p.place(lamp, x, yx, 9);
                }
            }
            p.bx(5, 1, 6, 5, 2, 6, light);
            p.bx(5, 1, 9, 5, 2, 9, light);
            p.bx(10, 1, 6, 10, 2, 6, light);
            p.bx(10, 1, 9, 10, 2, 9, light);
            p.bx(6, 1, 5, 6, 2, 5, light);
            p.bx(9, 1, 5, 9, 2, 5, light);
            p.bx(6, 1, 10, 6, 2, 10, light);
            p.bx(9, 1, 10, 9, 2, 10, light);
            p.bx(5, 2, 5, 5, 6, 5, light);
            p.bx(5, 2, 10, 5, 6, 10, light);
            p.bx(10, 2, 5, 10, 6, 5, light);
            p.bx(10, 2, 10, 10, 6, 10, light);
            p.bx(5, 7, 1, 5, 7, 6, light);
            p.bx(10, 7, 1, 10, 7, 6, light);
            p.bx(5, 7, 9, 5, 7, 14, light);
            p.bx(10, 7, 9, 10, 7, 14, light);
            p.bx(1, 7, 5, 6, 7, 5, light);
            p.bx(1, 7, 10, 6, 7, 10, light);
            p.bx(9, 7, 5, 14, 7, 5, light);
            p.bx(9, 7, 10, 14, 7, 10, light);
            p.bx(2, 1, 2, 2, 1, 3, light);
            p.bx(3, 1, 2, 3, 1, 2, light);
            p.bx(13, 1, 2, 13, 1, 3, light);
            p.bx(12, 1, 2, 12, 1, 2, light);
            p.bx(2, 1, 12, 2, 1, 13, light);
            p.bx(3, 1, 13, 3, 1, 13, light);
            p.bx(13, 1, 12, 13, 1, 13, light);
            p.bx(12, 1, 13, 12, 1, 13, light);
        }
        RKind::DoubleX => {
            let east = gr[rd].conn[EAST].expect("room");
            let west = rd;
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(8, 0, gr[east].open[DOWN]);
                p.generate_default_floor(0, 0, gr[west].open[DOWN]);
            }
            if gr[west].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 4, 1, 7, 4, 6, gray);
            }
            if gr[east].conn[UP].is_none() {
                p.generate_box_on_fill_only(8, 4, 1, 14, 4, 6, gray);
            }
            p.bx(0, 3, 0, 0, 3, 7, light);
            p.bx(15, 3, 0, 15, 3, 7, light);
            p.bx(1, 3, 0, 15, 3, 0, light);
            p.bx(1, 3, 7, 14, 3, 7, light);
            p.bx(0, 2, 0, 0, 2, 7, gray);
            p.bx(15, 2, 0, 15, 2, 7, gray);
            p.bx(1, 2, 0, 15, 2, 0, gray);
            p.bx(1, 2, 7, 14, 2, 7, gray);
            p.bx(0, 1, 0, 0, 1, 7, light);
            p.bx(15, 1, 0, 15, 1, 7, light);
            p.bx(1, 1, 0, 15, 1, 0, light);
            p.bx(1, 1, 7, 14, 1, 7, light);
            p.bx(5, 1, 0, 10, 1, 4, light);
            p.bx(6, 2, 0, 9, 2, 3, gray);
            p.bx(5, 3, 0, 10, 3, 4, light);
            p.place(lamp, 6, 2, 3);
            p.place(lamp, 9, 2, 3);
            if gr[west].open[SOUTH] {
                p.generate_water_box(3, 1, 0, 4, 2, 0);
            }
            if gr[west].open[NORTH] {
                p.generate_water_box(3, 1, 7, 4, 2, 7);
            }
            if gr[west].open[WEST] {
                p.generate_water_box(0, 1, 3, 0, 2, 4);
            }
            if gr[east].open[SOUTH] {
                p.generate_water_box(11, 1, 0, 12, 2, 0);
            }
            if gr[east].open[NORTH] {
                p.generate_water_box(11, 1, 7, 12, 2, 7);
            }
            if gr[east].open[EAST] {
                p.generate_water_box(15, 1, 3, 15, 2, 4);
            }
        }
        RKind::DoubleXY => {
            let east = gr[rd].conn[EAST].expect("room");
            let west = rd;
            let west_up = gr[west].conn[UP].expect("room");
            let east_up = gr[east].conn[UP].expect("room");
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(8, 0, gr[east].open[DOWN]);
                p.generate_default_floor(0, 0, gr[west].open[DOWN]);
            }
            if gr[west_up].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 8, 1, 7, 8, 6, gray);
            }
            if gr[east_up].conn[UP].is_none() {
                p.generate_box_on_fill_only(8, 8, 1, 14, 8, 6, gray);
            }
            for y in 1..=7 {
                let mut block = light;
                if y == 2 || y == 6 {
                    block = gray;
                }
                p.bx(0, y, 0, 0, y, 7, block);
                p.bx(15, y, 0, 15, y, 7, block);
                p.bx(1, y, 0, 15, y, 0, block);
                p.bx(1, y, 7, 14, y, 7, block);
            }
            p.bx(2, 1, 3, 2, 7, 4, light);
            p.bx(3, 1, 2, 4, 7, 2, light);
            p.bx(3, 1, 5, 4, 7, 5, light);
            p.bx(13, 1, 3, 13, 7, 4, light);
            p.bx(11, 1, 2, 12, 7, 2, light);
            p.bx(11, 1, 5, 12, 7, 5, light);
            p.bx(5, 1, 3, 5, 3, 4, light);
            p.bx(10, 1, 3, 10, 3, 4, light);
            p.bx(5, 7, 2, 10, 7, 5, light);
            p.bx(5, 5, 2, 5, 7, 2, light);
            p.bx(10, 5, 2, 10, 7, 2, light);
            p.bx(5, 5, 5, 5, 7, 5, light);
            p.bx(10, 5, 5, 10, 7, 5, light);
            p.place(light, 6, 6, 2);
            p.place(light, 9, 6, 2);
            p.place(light, 6, 6, 5);
            p.place(light, 9, 6, 5);
            p.bx(5, 4, 3, 6, 4, 4, light);
            p.bx(9, 4, 3, 10, 4, 4, light);
            p.place(lamp, 5, 4, 2);
            p.place(lamp, 5, 4, 5);
            p.place(lamp, 10, 4, 2);
            p.place(lamp, 10, 4, 5);
            if gr[west].open[SOUTH] {
                p.generate_water_box(3, 1, 0, 4, 2, 0);
            }
            if gr[west].open[NORTH] {
                p.generate_water_box(3, 1, 7, 4, 2, 7);
            }
            if gr[west].open[WEST] {
                p.generate_water_box(0, 1, 3, 0, 2, 4);
            }
            if gr[east].open[SOUTH] {
                p.generate_water_box(11, 1, 0, 12, 2, 0);
            }
            if gr[east].open[NORTH] {
                p.generate_water_box(11, 1, 7, 12, 2, 7);
            }
            if gr[east].open[EAST] {
                p.generate_water_box(15, 1, 3, 15, 2, 4);
            }
            if gr[west_up].open[SOUTH] {
                p.generate_water_box(3, 5, 0, 4, 6, 0);
            }
            if gr[west_up].open[NORTH] {
                p.generate_water_box(3, 5, 7, 4, 6, 7);
            }
            if gr[west_up].open[WEST] {
                p.generate_water_box(0, 5, 3, 0, 6, 4);
            }
            if gr[east_up].open[SOUTH] {
                p.generate_water_box(11, 5, 0, 12, 6, 0);
            }
            if gr[east_up].open[NORTH] {
                p.generate_water_box(11, 5, 7, 12, 6, 7);
            }
            if gr[east_up].open[EAST] {
                p.generate_water_box(15, 5, 3, 15, 6, 4);
            }
        }
        RKind::DoubleY => {
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(0, 0, gr[rd].open[DOWN]);
            }
            let above = gr[rd].conn[UP].expect("room");
            if gr[above].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 8, 1, 6, 8, 6, gray);
            }
            p.bx(0, 4, 0, 0, 4, 7, light);
            p.bx(7, 4, 0, 7, 4, 7, light);
            p.bx(1, 4, 0, 6, 4, 0, light);
            p.bx(1, 4, 7, 6, 4, 7, light);
            p.bx(2, 4, 1, 2, 4, 2, light);
            p.bx(1, 4, 2, 1, 4, 2, light);
            p.bx(5, 4, 1, 5, 4, 2, light);
            p.bx(6, 4, 2, 6, 4, 2, light);
            p.bx(2, 4, 5, 2, 4, 6, light);
            p.bx(1, 4, 5, 1, 4, 5, light);
            p.bx(5, 4, 5, 5, 4, 6, light);
            p.bx(6, 4, 5, 6, 4, 5, light);
            let mut definition = rd;
            for y in (1..=5).step_by(4) {
                let z = 0;
                if gr[definition].open[SOUTH] {
                    p.bx(2, y, z, 2, y + 2, z, light);
                    p.bx(5, y, z, 5, y + 2, z, light);
                    p.bx(3, y + 2, z, 4, y + 2, z, light);
                } else {
                    p.bx(0, y, z, 7, y + 2, z, light);
                    p.bx(0, y + 1, z, 7, y + 1, z, gray);
                }
                let var13 = 7;
                if gr[definition].open[NORTH] {
                    p.bx(2, y, var13, 2, y + 2, var13, light);
                    p.bx(5, y, var13, 5, y + 2, var13, light);
                    p.bx(3, y + 2, var13, 4, y + 2, var13, light);
                } else {
                    p.bx(0, y, var13, 7, y + 2, var13, light);
                    p.bx(0, y + 1, var13, 7, y + 1, var13, gray);
                }
                let x = 0;
                if gr[definition].open[WEST] {
                    p.bx(x, y, 2, x, y + 2, 2, light);
                    p.bx(x, y, 5, x, y + 2, 5, light);
                    p.bx(x, y + 2, 3, x, y + 2, 4, light);
                } else {
                    p.bx(x, y, 0, x, y + 2, 7, light);
                    p.bx(x, y + 1, 0, x, y + 1, 7, gray);
                }
                let var14 = 7;
                if gr[definition].open[EAST] {
                    p.bx(var14, y, 2, var14, y + 2, 2, light);
                    p.bx(var14, y, 5, var14, y + 2, 5, light);
                    p.bx(var14, y + 2, 3, var14, y + 2, 4, light);
                } else {
                    p.bx(var14, y, 0, var14, y + 2, 7, light);
                    p.bx(var14, y + 1, 0, var14, y + 1, 7, gray);
                }
                definition = above;
            }
        }
        RKind::DoubleYZ => {
            let north = gr[rd].conn[NORTH].expect("room");
            let south = rd;
            let north_up = gr[north].conn[UP].expect("room");
            let south_up = gr[south].conn[UP].expect("room");
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(0, 8, gr[north].open[DOWN]);
                p.generate_default_floor(0, 0, gr[south].open[DOWN]);
            }
            if gr[south_up].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 8, 1, 6, 8, 7, gray);
            }
            if gr[north_up].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 8, 8, 6, 8, 14, gray);
            }
            for y in 1..=7 {
                let mut block = light;
                if y == 2 || y == 6 {
                    block = gray;
                }
                p.bx(0, y, 0, 0, y, 15, block);
                p.bx(7, y, 0, 7, y, 15, block);
                p.bx(1, y, 0, 6, y, 0, block);
                p.bx(1, y, 15, 6, y, 15, block);
            }
            for y in 1..=7 {
                let mut block = black;
                if y == 2 || y == 6 {
                    block = lamp;
                }
                p.bx(3, y, 7, 4, y, 8, block);
            }
            if gr[south].open[SOUTH] {
                p.generate_water_box(3, 1, 0, 4, 2, 0);
            }
            if gr[south].open[EAST] {
                p.generate_water_box(7, 1, 3, 7, 2, 4);
            }
            if gr[south].open[WEST] {
                p.generate_water_box(0, 1, 3, 0, 2, 4);
            }
            if gr[north].open[NORTH] {
                p.generate_water_box(3, 1, 15, 4, 2, 15);
            }
            if gr[north].open[WEST] {
                p.generate_water_box(0, 1, 11, 0, 2, 12);
            }
            if gr[north].open[EAST] {
                p.generate_water_box(7, 1, 11, 7, 2, 12);
            }
            if gr[south_up].open[SOUTH] {
                p.generate_water_box(3, 5, 0, 4, 6, 0);
            }
            if gr[south_up].open[EAST] {
                p.generate_water_box(7, 5, 3, 7, 6, 4);
                p.bx(5, 4, 2, 6, 4, 5, light);
                p.bx(6, 1, 2, 6, 3, 2, light);
                p.bx(6, 1, 5, 6, 3, 5, light);
            }
            if gr[south_up].open[WEST] {
                p.generate_water_box(0, 5, 3, 0, 6, 4);
                p.bx(1, 4, 2, 2, 4, 5, light);
                p.bx(1, 1, 2, 1, 3, 2, light);
                p.bx(1, 1, 5, 1, 3, 5, light);
            }
            if gr[north_up].open[NORTH] {
                p.generate_water_box(3, 5, 15, 4, 6, 15);
            }
            if gr[north_up].open[WEST] {
                p.generate_water_box(0, 5, 11, 0, 6, 12);
                p.bx(1, 4, 10, 2, 4, 13, light);
                p.bx(1, 1, 10, 1, 3, 10, light);
                p.bx(1, 1, 13, 1, 3, 13, light);
            }
            if gr[north_up].open[EAST] {
                p.generate_water_box(7, 5, 11, 7, 6, 12);
                p.bx(5, 4, 10, 6, 4, 13, light);
                p.bx(6, 1, 10, 6, 3, 10, light);
                p.bx(6, 1, 13, 6, 3, 13, light);
            }
        }
        RKind::DoubleZ => {
            let north = gr[rd].conn[NORTH].expect("room");
            let south = rd;
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(0, 8, gr[north].open[DOWN]);
                p.generate_default_floor(0, 0, gr[south].open[DOWN]);
            }
            if gr[south].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 4, 1, 6, 4, 7, gray);
            }
            if gr[north].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 4, 8, 6, 4, 14, gray);
            }
            p.bx(0, 3, 0, 0, 3, 15, light);
            p.bx(7, 3, 0, 7, 3, 15, light);
            p.bx(1, 3, 0, 7, 3, 0, light);
            p.bx(1, 3, 15, 6, 3, 15, light);
            p.bx(0, 2, 0, 0, 2, 15, gray);
            p.bx(7, 2, 0, 7, 2, 15, gray);
            p.bx(1, 2, 0, 7, 2, 0, gray);
            p.bx(1, 2, 15, 6, 2, 15, gray);
            p.bx(0, 1, 0, 0, 1, 15, light);
            p.bx(7, 1, 0, 7, 1, 15, light);
            p.bx(1, 1, 0, 7, 1, 0, light);
            p.bx(1, 1, 15, 6, 1, 15, light);
            p.bx(1, 1, 1, 1, 1, 2, light);
            p.bx(6, 1, 1, 6, 1, 2, light);
            p.bx(1, 3, 1, 1, 3, 2, light);
            p.bx(6, 3, 1, 6, 3, 2, light);
            p.bx(1, 1, 13, 1, 1, 14, light);
            p.bx(6, 1, 13, 6, 1, 14, light);
            p.bx(1, 3, 13, 1, 3, 14, light);
            p.bx(6, 3, 13, 6, 3, 14, light);
            p.bx(2, 1, 6, 2, 3, 6, light);
            p.bx(5, 1, 6, 5, 3, 6, light);
            p.bx(2, 1, 9, 2, 3, 9, light);
            p.bx(5, 1, 9, 5, 3, 9, light);
            p.bx(3, 2, 6, 4, 2, 6, light);
            p.bx(3, 2, 9, 4, 2, 9, light);
            p.bx(2, 2, 7, 2, 2, 8, light);
            p.bx(5, 2, 7, 5, 2, 8, light);
            p.place(lamp, 2, 2, 5);
            p.place(lamp, 5, 2, 5);
            p.place(lamp, 2, 2, 10);
            p.place(lamp, 5, 2, 10);
            p.place(light, 2, 3, 5);
            p.place(light, 5, 3, 5);
            p.place(light, 2, 3, 10);
            p.place(light, 5, 3, 10);
            if gr[south].open[SOUTH] {
                p.generate_water_box(3, 1, 0, 4, 2, 0);
            }
            if gr[south].open[EAST] {
                p.generate_water_box(7, 1, 3, 7, 2, 4);
            }
            if gr[south].open[WEST] {
                p.generate_water_box(0, 1, 3, 0, 2, 4);
            }
            if gr[north].open[NORTH] {
                p.generate_water_box(3, 1, 15, 4, 2, 15);
            }
            if gr[north].open[WEST] {
                p.generate_water_box(0, 1, 11, 0, 2, 12);
            }
            if gr[north].open[EAST] {
                p.generate_water_box(7, 1, 11, 7, 2, 12);
            }
        }
        RKind::Entry => {
            p.bx(0, 3, 0, 2, 3, 7, light);
            p.bx(5, 3, 0, 7, 3, 7, light);
            p.bx(0, 2, 0, 1, 2, 7, light);
            p.bx(6, 2, 0, 7, 2, 7, light);
            p.bx(0, 1, 0, 0, 1, 7, light);
            p.bx(7, 1, 0, 7, 1, 7, light);
            p.bx(0, 1, 7, 7, 3, 7, light);
            p.bx(1, 1, 0, 2, 3, 0, light);
            p.bx(5, 1, 0, 6, 3, 0, light);
            if gr[rd].open[NORTH] {
                p.generate_water_box(3, 1, 7, 4, 2, 7);
            }
            if gr[rd].open[WEST] {
                p.generate_water_box(0, 1, 3, 1, 2, 4);
            }
            if gr[rd].open[EAST] {
                p.generate_water_box(6, 1, 3, 7, 2, 4);
            }
        }
        RKind::Penthouse => {
            p.bx(2, -1, 2, 11, -1, 11, light);
            p.bx(0, -1, 0, 1, -1, 11, gray);
            p.bx(12, -1, 0, 13, -1, 11, gray);
            p.bx(2, -1, 0, 11, -1, 1, gray);
            p.bx(2, -1, 12, 11, -1, 13, gray);
            p.bx(0, 0, 0, 0, 0, 13, light);
            p.bx(13, 0, 0, 13, 0, 13, light);
            p.bx(1, 0, 0, 12, 0, 0, light);
            p.bx(1, 0, 13, 12, 0, 13, light);
            for i in (2..=11).step_by(3) {
                p.place(lamp, 0, 0, i);
                p.place(lamp, 13, 0, i);
                p.place(lamp, i, 0, 0);
            }
            p.bx(2, 0, 3, 4, 0, 9, light);
            p.bx(9, 0, 3, 11, 0, 9, light);
            p.bx(4, 0, 9, 9, 0, 11, light);
            p.place(light, 5, 0, 8);
            p.place(light, 8, 0, 8);
            p.place(light, 10, 0, 10);
            p.place(light, 3, 0, 10);
            p.bx(3, 0, 3, 3, 0, 7, black);
            p.bx(10, 0, 3, 10, 0, 7, black);
            p.bx(6, 0, 10, 7, 0, 10, black);
            let mut x = 3;
            for _ in 0..2 {
                for z in (2..=8).step_by(3) {
                    p.bx(x, 0, z, x, 2, z, light);
                }
                x = 10;
            }
            p.bx(5, 0, 10, 5, 2, 10, light);
            p.bx(8, 0, 10, 8, 2, 10, light);
            p.bx(6, -1, 7, 7, -1, 8, black);
            p.generate_water_box(6, -1, 3, 7, -1, 4);
            p.elder(6, 1, 6);
        }
        RKind::Simple { design } => {
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(0, 0, gr[rd].open[DOWN]);
            }
            if gr[rd].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 4, 1, 6, 4, 6, gray);
            }
            let center = design != 0
                && rng.next_bool()
                && !gr[rd].open[DOWN]
                && !gr[rd].open[UP]
                && gr[rd].open.iter().filter(|o| **o).count() > 1;
            if design == 0 {
                p.bx(0, 1, 0, 2, 1, 2, light);
                p.bx(0, 3, 0, 2, 3, 2, light);
                p.bx(0, 2, 0, 0, 2, 2, gray);
                p.bx(1, 2, 0, 2, 2, 0, gray);
                p.place(lamp, 1, 2, 1);
                p.bx(5, 1, 0, 7, 1, 2, light);
                p.bx(5, 3, 0, 7, 3, 2, light);
                p.bx(7, 2, 0, 7, 2, 2, gray);
                p.bx(5, 2, 0, 6, 2, 0, gray);
                p.place(lamp, 6, 2, 1);
                p.bx(0, 1, 5, 2, 1, 7, light);
                p.bx(0, 3, 5, 2, 3, 7, light);
                p.bx(0, 2, 5, 0, 2, 7, gray);
                p.bx(1, 2, 7, 2, 2, 7, gray);
                p.place(lamp, 1, 2, 6);
                p.bx(5, 1, 5, 7, 1, 7, light);
                p.bx(5, 3, 5, 7, 3, 7, light);
                p.bx(7, 2, 5, 7, 2, 7, gray);
                p.bx(5, 2, 7, 6, 2, 7, gray);
                p.place(lamp, 6, 2, 6);
                if gr[rd].open[SOUTH] {
                    p.bx(3, 3, 0, 4, 3, 0, light);
                } else {
                    p.bx(3, 3, 0, 4, 3, 1, light);
                    p.bx(3, 2, 0, 4, 2, 0, gray);
                    p.bx(3, 1, 0, 4, 1, 1, light);
                }
                if gr[rd].open[NORTH] {
                    p.bx(3, 3, 7, 4, 3, 7, light);
                } else {
                    p.bx(3, 3, 6, 4, 3, 7, light);
                    p.bx(3, 2, 7, 4, 2, 7, gray);
                    p.bx(3, 1, 6, 4, 1, 7, light);
                }
                if gr[rd].open[WEST] {
                    p.bx(0, 3, 3, 0, 3, 4, light);
                } else {
                    p.bx(0, 3, 3, 1, 3, 4, light);
                    p.bx(0, 2, 3, 0, 2, 4, gray);
                    p.bx(0, 1, 3, 1, 1, 4, light);
                }
                if gr[rd].open[EAST] {
                    p.bx(7, 3, 3, 7, 3, 4, light);
                } else {
                    p.bx(6, 3, 3, 7, 3, 4, light);
                    p.bx(7, 2, 3, 7, 2, 4, gray);
                    p.bx(6, 1, 3, 7, 1, 4, light);
                }
            } else if design == 1 {
                p.bx(2, 1, 2, 2, 3, 2, light);
                p.bx(2, 1, 5, 2, 3, 5, light);
                p.bx(5, 1, 5, 5, 3, 5, light);
                p.bx(5, 1, 2, 5, 3, 2, light);
                p.place(lamp, 2, 2, 2);
                p.place(lamp, 2, 2, 5);
                p.place(lamp, 5, 2, 5);
                p.place(lamp, 5, 2, 2);
                p.bx(0, 1, 0, 1, 3, 0, light);
                p.bx(0, 1, 1, 0, 3, 1, light);
                p.bx(0, 1, 7, 1, 3, 7, light);
                p.bx(0, 1, 6, 0, 3, 6, light);
                p.bx(6, 1, 7, 7, 3, 7, light);
                p.bx(7, 1, 6, 7, 3, 6, light);
                p.bx(6, 1, 0, 7, 3, 0, light);
                p.bx(7, 1, 1, 7, 3, 1, light);
                p.place(gray, 1, 2, 0);
                p.place(gray, 0, 2, 1);
                p.place(gray, 1, 2, 7);
                p.place(gray, 0, 2, 6);
                p.place(gray, 6, 2, 7);
                p.place(gray, 7, 2, 6);
                p.place(gray, 6, 2, 0);
                p.place(gray, 7, 2, 1);
                if !gr[rd].open[SOUTH] {
                    p.bx(1, 3, 0, 6, 3, 0, light);
                    p.bx(1, 2, 0, 6, 2, 0, gray);
                    p.bx(1, 1, 0, 6, 1, 0, light);
                }
                if !gr[rd].open[NORTH] {
                    p.bx(1, 3, 7, 6, 3, 7, light);
                    p.bx(1, 2, 7, 6, 2, 7, gray);
                    p.bx(1, 1, 7, 6, 1, 7, light);
                }
                if !gr[rd].open[WEST] {
                    p.bx(0, 3, 1, 0, 3, 6, light);
                    p.bx(0, 2, 1, 0, 2, 6, gray);
                    p.bx(0, 1, 1, 0, 1, 6, light);
                }
                if !gr[rd].open[EAST] {
                    p.bx(7, 3, 1, 7, 3, 6, light);
                    p.bx(7, 2, 1, 7, 2, 6, gray);
                    p.bx(7, 1, 1, 7, 1, 6, light);
                }
            } else if design == 2 {
                p.bx(0, 1, 0, 0, 1, 7, light);
                p.bx(7, 1, 0, 7, 1, 7, light);
                p.bx(1, 1, 0, 6, 1, 0, light);
                p.bx(1, 1, 7, 6, 1, 7, light);
                p.bx(0, 2, 0, 0, 2, 7, black);
                p.bx(7, 2, 0, 7, 2, 7, black);
                p.bx(1, 2, 0, 6, 2, 0, black);
                p.bx(1, 2, 7, 6, 2, 7, black);
                p.bx(0, 3, 0, 0, 3, 7, light);
                p.bx(7, 3, 0, 7, 3, 7, light);
                p.bx(1, 3, 0, 6, 3, 0, light);
                p.bx(1, 3, 7, 6, 3, 7, light);
                p.bx(0, 1, 3, 0, 2, 4, black);
                p.bx(7, 1, 3, 7, 2, 4, black);
                p.bx(3, 1, 0, 4, 2, 0, black);
                p.bx(3, 1, 7, 4, 2, 7, black);
                if gr[rd].open[SOUTH] {
                    p.generate_water_box(3, 1, 0, 4, 2, 0);
                }
                if gr[rd].open[NORTH] {
                    p.generate_water_box(3, 1, 7, 4, 2, 7);
                }
                if gr[rd].open[WEST] {
                    p.generate_water_box(0, 1, 3, 0, 2, 4);
                }
                if gr[rd].open[EAST] {
                    p.generate_water_box(7, 1, 3, 7, 2, 4);
                }
            }
            if center {
                p.bx(3, 1, 3, 4, 1, 4, light);
                p.bx(3, 2, 3, 4, 2, 4, gray);
                p.bx(3, 3, 3, 4, 3, 4, light);
            }
        }
        RKind::SimpleTop => {
            if gr[rd].index / 25 > 0 {
                p.generate_default_floor(0, 0, gr[rd].open[DOWN]);
            }
            if gr[rd].conn[UP].is_none() {
                p.generate_box_on_fill_only(1, 4, 1, 6, 4, 6, gray);
            }
            for x in 1..=6 {
                for z in 1..=6 {
                    if rng.next_int_bounded(3) != 0 {
                        let y0 = 2 + if rng.next_int_bounded(4) == 0 { 0 } else { 1 };
                        let wet = st(b, "wet_sponge");
                        p.bx(x, y0, z, x, 3, z, wet);
                    }
                }
            }
            p.bx(0, 1, 0, 0, 1, 7, light);
            p.bx(7, 1, 0, 7, 1, 7, light);
            p.bx(1, 1, 0, 6, 1, 0, light);
            p.bx(1, 1, 7, 6, 1, 7, light);
            p.bx(0, 2, 0, 0, 2, 7, black);
            p.bx(7, 2, 0, 7, 2, 7, black);
            p.bx(1, 2, 0, 6, 2, 0, black);
            p.bx(1, 2, 7, 6, 2, 7, black);
            p.bx(0, 3, 0, 0, 3, 7, light);
            p.bx(7, 3, 0, 7, 3, 7, light);
            p.bx(1, 3, 0, 6, 3, 0, light);
            p.bx(1, 3, 7, 6, 3, 7, light);
            p.bx(0, 1, 3, 0, 2, 4, black);
            p.bx(7, 1, 3, 7, 2, 4, black);
            p.bx(3, 1, 0, 4, 2, 0, black);
            p.bx(3, 1, 7, 4, 2, 7, black);
            if gr[rd].open[SOUTH] {
                p.generate_water_box(3, 1, 0, 4, 2, 0);
            }
        }
        RKind::Wing { design } => {
            if design == 0 {
                for i in 0..4 {
                    p.bx(10 - i, 3 - i, 20 - i, 12 + i, 3 - i, 20, light);
                }
                p.bx(7, 0, 6, 15, 0, 16, light);
                p.bx(6, 0, 6, 6, 3, 20, light);
                p.bx(16, 0, 6, 16, 3, 20, light);
                p.bx(7, 1, 7, 7, 1, 20, light);
                p.bx(15, 1, 7, 15, 1, 20, light);
                p.bx(7, 1, 6, 9, 3, 6, light);
                p.bx(13, 1, 6, 15, 3, 6, light);
                p.bx(8, 1, 7, 9, 1, 7, light);
                p.bx(13, 1, 7, 14, 1, 7, light);
                p.bx(9, 0, 5, 13, 0, 5, light);
                p.bx(10, 0, 7, 12, 0, 7, black);
                p.bx(8, 0, 10, 8, 0, 12, black);
                p.bx(14, 0, 10, 14, 0, 12, black);
                for z in (7..=18).rev().step_by(3) {
                    p.place(lamp, 6, 3, z);
                    p.place(lamp, 16, 3, z);
                }
                p.place(lamp, 10, 0, 10);
                p.place(lamp, 12, 0, 10);
                p.place(lamp, 10, 0, 12);
                p.place(lamp, 12, 0, 12);
                p.place(lamp, 8, 3, 6);
                p.place(lamp, 14, 3, 6);
                p.place(light, 4, 2, 4);
                p.place(lamp, 4, 1, 4);
                p.place(light, 4, 0, 4);
                p.place(light, 18, 2, 4);
                p.place(lamp, 18, 1, 4);
                p.place(light, 18, 0, 4);
                p.place(light, 4, 2, 18);
                p.place(lamp, 4, 1, 18);
                p.place(light, 4, 0, 18);
                p.place(light, 18, 2, 18);
                p.place(lamp, 18, 1, 18);
                p.place(light, 18, 0, 18);
                p.place(light, 9, 7, 20);
                p.place(light, 13, 7, 20);
                p.bx(6, 0, 21, 7, 4, 21, light);
                p.bx(15, 0, 21, 16, 4, 21, light);
                p.elder(11, 2, 16);
            } else if design == 1 {
                p.bx(9, 3, 18, 13, 3, 20, light);
                p.bx(9, 0, 18, 9, 2, 18, light);
                p.bx(13, 0, 18, 13, 2, 18, light);
                let mut x = 9;
                for _ in 0..2 {
                    p.place(light, x, 6, 20);
                    p.place(lamp, x, 5, 20);
                    p.place(light, x, 4, 20);
                    x = 13;
                }
                p.bx(7, 3, 7, 15, 3, 14, light);
                let mut var14 = 10;
                for _ in 0..2 {
                    p.bx(var14, 0, 10, var14, 6, 10, light);
                    p.bx(var14, 0, 12, var14, 6, 12, light);
                    p.place(lamp, var14, 0, 10);
                    p.place(lamp, var14, 0, 12);
                    p.place(lamp, var14, 4, 10);
                    p.place(lamp, var14, 4, 12);
                    var14 = 12;
                }
                var14 = 8;
                for _ in 0..2 {
                    p.bx(var14, 0, 7, var14, 2, 7, light);
                    p.bx(var14, 0, 14, var14, 2, 14, light);
                    var14 = 14;
                }
                p.bx(8, 3, 8, 8, 3, 13, black);
                p.bx(14, 3, 8, 14, 3, 13, black);
                p.elder(11, 5, 13);
            }
        }
    }
}
