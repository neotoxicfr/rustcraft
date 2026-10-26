//! "Scattered" pieces (`generation-decoration.md` § 8.11–8.13): desert pyramid, jungle
//! temple, swamp hut; and the igloo (template pieces). Height fixed at the first placement
//! (memorised in the piece, like the game).

use std::sync::{Arc, Mutex};

use crate::decor::Ctx;
use crate::decor_blocks::{Dir, State};
use crate::decor_region::{BeData, Hm, Region};
use crate::decor_rng::DecoRng;
use crate::structure::{GenStart, Piece, StructureGen};
use crate::structure_coded::CodedPiece;
use crate::structure_piece::P;
use crate::structure_place::Proc;
use crate::structure_templ::{TKind, TemplPiece};
use crate::template::{BBox, Mirror, Rot};

#[derive(Debug)]
pub enum SKind {
    Pyramid { chests: [bool; 4], sand: Vec<[i32; 3]>, roof: [i32; 3] },
    Jungle { trap1: bool, trap2: bool, main: bool, hidden: bool },
    /// Witch and cat already placed.
    SwampHut { witch: bool, cat: bool },
}

/// Mutable state of a scattered piece (moved box, height, flags).
#[derive(Debug)]
pub struct ScatState {
    pub bbox: BBox,
    pub hpos: i32,
    pub kind: SKind,
}

#[derive(Debug)]
pub struct ScatPiece {
    pub orient: Dir,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub state: Mutex<ScatState>,
}

fn make_box(x: i32, y: i32, z: i32, d: Dir, w: i32, h: i32, dp: i32) -> BBox {
    if matches!(d, Dir::North | Dir::South) {
        BBox::new([x, y, z], [x + w - 1, y + h - 1, z + dp - 1])
    } else {
        BBox::new([x, y, z], [x + dp - 1, y + h - 1, z + w - 1])
    }
}

fn random_dir(r: &mut DecoRng) -> Dir {
    [Dir::North, Dir::East, Dir::South, Dir::West][r.next_int_bounded(4) as usize]
}

fn occupied(g: &GenStart, x: i32, z: i32) -> i32 {
    g.first_free(x, z, false) - 1
}

/// Common start (§ 8.11): biome at the centre, corners above the sea, piece at the corner of the chunk.
fn single(
    g: &mut GenStart,
    w: i32,
    d: i32,
    h: i32,
    kind: SKind,
    id: &'static str,
    check_sea: bool,
) -> Option<([i32; 3], Vec<Piece>)> {
    if !biome_on_center(g) {
        return None;
    }
    let (x0, z0) = (16 * g.cx, 16 * g.cz);
    if check_sea {
        let lowest =
            [occupied(g, x0, z0), occupied(g, x0, z0 + d), occupied(g, x0 + w, z0), occupied(g, x0 + w, z0 + d)]
                .into_iter()
                .min()
                .unwrap_or(0);
        if lowest < g.t.gc.sea {
            return None;
        }
    }
    let (cx, cz) = (x0 + 8, z0 + 8);
    let start = [cx, occupied(g, cx, cz), cz];
    let dir = random_dir(&mut g.rng);
    let bbox = make_box(x0, 64, z0, dir, w, h, d);
    let p =
        ScatPiece { orient: dir, width: w, height: h, depth: d, state: Mutex::new(ScatState { bbox, hpos: -1, kind }) };
    Some((start, vec![Piece::Coded { id, bbox, data: Arc::new(CodedPiece::Scattered(Arc::new(p))) }]))
}

fn biome_on_center(g: &GenStart) -> bool {
    let (x, z) = (16 * g.cx + 8, 16 * g.cz + 8);
    let (y0, y1) = (g.t.world_min_y - 1, g.t.world_min_y + g.t.world_height - 1);
    let mut qy = y0 >> 2;
    while qy <= y1 >> 2 {
        if g.def.biomes.get(g.biome_at(x, qy << 2, z) as usize).copied().unwrap_or(false) {
            return true;
        }
        qy += 1;
    }
    false
}

pub fn desert_pyramid(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    single(g, 21, 21, 15, SKind::Pyramid { chests: [false; 4], sand: Vec::new(), roof: [0; 3] }, "desert_pyramid", true)
}

pub fn jungle_temple(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    single(
        g,
        12,
        15,
        10,
        SKind::Jungle { trap1: false, trap2: false, main: false, hidden: false },
        "jungle_temple",
        true,
    )
}

pub fn swamp_hut(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    single(g, 7, 9, 7, SKind::SwampHut { witch: false, cat: false }, "swamp_hut", false)
}

// ---------------------------------------------------------------- Igloo (§ 8.4)

const IGLOO_TOP: &str = "minecraft:igloo/top";
const IGLOO_MIDDLE: &str = "minecraft:igloo/middle";
const IGLOO_BOTTOM: &str = "minecraft:igloo/bottom";

fn igloo_part(loc: &str) -> ([i32; 3], [i32; 3]) {
    match loc {
        IGLOO_TOP => ([3, 5, 5], [0, 0, 0]),
        IGLOO_MIDDLE => ([1, 3, 1], [2, -3, 4]),
        _ => ([3, 6, 7], [0, -3, -2]),
    }
}

pub fn igloo(g: &mut GenStart) -> Option<([i32; 3], Vec<Piece>)> {
    if !biome_on_center(g) {
        return None;
    }
    let (cx, cz) = (16 * g.cx + 8, 16 * g.cz + 8);
    let start = [cx, occupied(g, cx, cz), cz];
    let p = [16 * g.cx, 90, 16 * g.cz];
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let mut parts: Vec<(&str, i32)> = Vec::new();
    if g.rng.next_double() < 0.5 {
        let depth = g.rng.next_int_bounded(8) + 4;
        parts.push((IGLOO_BOTTOM, depth * 3));
        for i in 0..depth - 1 {
            parts.push((IGLOO_MIDDLE, i * 3));
        }
    }
    parts.push((IGLOO_TOP, 0));
    let b = g.b;
    let mut out = Vec::new();
    for (loc, depth) in parts {
        let t = g.sg.templates.get(loc, b)?;
        let (pivot, off) = igloo_part(loc);
        let pos = [p[0] + off[0], p[1] + off[1] - depth, p[2] + off[2]];
        let tp = TemplPiece {
            location: loc.to_owned(),
            pos: Mutex::new(pos),
            mirror: Mirror::None,
            rot,
            pivot,
            procs: Arc::new(vec![Proc::Ignore(b.block_id("minecraft:structure_block").into_iter().collect())]),
            kind: TKind::Igloo { top: loc == IGLOO_TOP, offset: off },
        };
        let bbox = t.bbox(pos, &crate::template::Settings { mirror: Mirror::None, rot, pivot });
        out.push(Piece::Coded { id: "igloo", bbox, data: Arc::new(CodedPiece::Template(Arc::new(tp))) });
    }
    Some((start, out))
}

// ---------------------------------------------------------------- Placement

/// Average height (MOTION_BLOCKING_NO_LEAVES) of the box columns within the cut-out.
fn average_height(r: &Region, st: &mut ScatState, zone: &BBox, offset: i32) -> bool {
    if st.hpos >= 0 {
        return true;
    }
    let (mut total, mut n) = (0, 0);
    for z in st.bbox.min[2]..=st.bbox.max[2] {
        for x in st.bbox.min[0]..=st.bbox.max[0] {
            if zone.contains([x, 64, z]) {
                total += r.height_at(Hm::MotionBlockingNoLeaves, x, z);
                n += 1;
            }
        }
    }
    if n == 0 {
        return false;
    }
    st.hpos = total / n;
    let dy = st.hpos - st.bbox.min[1] + offset;
    st.bbox = st.bbox.moved([0, dy, 0]);
    true
}

/// Minimum height over the whole box (read even outside the cut-out).
fn lowest_height(r: &Region, st: &mut ScatState, offset: i32) -> bool {
    if st.hpos >= 0 {
        return true;
    }
    let mut low = r.max_y() + 1;
    for z in st.bbox.min[2]..=st.bbox.max[2] {
        for x in st.bbox.min[0]..=st.bbox.max[0] {
            low = low.min(r.height_at(Hm::MotionBlockingNoLeaves, x, z));
        }
    }
    st.hpos = low;
    let dy = st.hpos - st.bbox.min[1] + offset;
    st.bbox = st.bbox.moved([0, dy, 0]);
    true
}

pub fn place(cx: &mut Ctx, sg: &StructureGen, sp: &ScatPiece, rng: &mut DecoRng, zone: BBox) {
    let mut st = sp.state.lock().expect("scattered piece");
    let tr = &sg.templates.transforms;
    match st.kind {
        SKind::Pyramid { .. } => {
            let off = -rng.next_int_bounded(3);
            if !lowest_height(cx.r, &mut st, off) {
                return;
            }
            let bbox = st.bbox;
            let mut p = P::new(cx.r, tr, zone, bbox, Some(sp.orient));
            pyramid(&mut p, sp, &mut st, rng);
        }
        SKind::Jungle { .. } => {
            if !average_height(cx.r, &mut st, &zone, 0) {
                return;
            }
            let bbox = st.bbox;
            let mut p = P::new(cx.r, tr, zone, bbox, Some(sp.orient));
            jungle(&mut p, &mut st, rng);
        }
        SKind::SwampHut { .. } => {
            if !average_height(cx.r, &mut st, &zone, 0) {
                return;
            }
            let bbox = st.bbox;
            let mut p = P::new(cx.r, tr, zone, bbox, Some(sp.orient));
            swamp(&mut p);
            // Witch then black cat, only once (flags kept in the piece and on
            // disk); the game finalises them at placement (hand, bonus, raid; cat variant according
            // to the place): done by the server at first load.
            let w = p.world(2, 2, 5);
            let SKind::SwampHut { witch, cat } = &mut st.kind else { unreachable!() };
            if p.zone.contains(w) && !(*witch && *cat) {
                use crate::structure_entities as se;
                let at = [f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5];
                let mark = || vec![("rc_finalize_structure".into(), rc_proto::nbt::Nbt::Byte(1))];
                if !*witch {
                    *witch = true;
                    p.r.add_entity(w[0], w[2], se::with(se::mob("minecraft:witch", at, 0.0, true), mark()));
                }
                if !*cat {
                    *cat = true;
                    let mut extra = mark();
                    extra.push(("variant".into(), rc_proto::nbt::Nbt::String("minecraft:all_black".into())));
                    p.r.add_entity(w[0], w[2], se::with(se::mob("minecraft:cat", at, 0.0, true), extra));
                }
            }
        }
    }
}

fn stairs(p: &P, n: &str, d: Dir) -> State {
    let s = p.state(n);
    match p.b.prop("facing") {
        Some(f) => p.b.with_dir(s, f, d),
        None => s,
    }
}

fn with(p: &P, s: State, k: &str, v: &str) -> State {
    match p.b.prop(k) {
        Some(pr) if p.b.has(s, pr) => p.b.with_str(s, pr, v),
        _ => s,
    }
}

fn pyramid(p: &mut P, sp: &ScatPiece, st: &mut ScatState, rng: &mut DecoRng) {
    let (w, d) = (sp.width, sp.depth);
    let sand_st = p.state("sandstone");
    let air = p.state("air");
    let cut = p.state("cut_sandstone");
    let chis = p.state("chiseled_sandstone");
    let orange = p.state("orange_terracotta");
    let blue = p.state("blue_terracotta");
    p.fill(0, -4, 0, w - 1, 0, d - 1, sand_st, sand_st, false);
    for i in 1..=9 {
        p.fill(i, i, i, w - 1 - i, i, d - 1 - i, sand_st, sand_st, false);
        p.fill(i + 1, i, i + 1, w - 2 - i, i, d - 2 - i, air, air, false);
    }
    for x in 0..w {
        for z in 0..d {
            p.fill_column_down(sand_st, x, -5, z);
        }
    }
    let n = stairs(p, "sandstone_stairs", Dir::North);
    let s = stairs(p, "sandstone_stairs", Dir::South);
    let e = stairs(p, "sandstone_stairs", Dir::East);
    let wst = stairs(p, "sandstone_stairs", Dir::West);
    p.fill(0, 0, 0, 4, 9, 4, sand_st, air, false);
    p.fill(1, 10, 1, 3, 10, 3, sand_st, sand_st, false);
    p.place(n, 2, 10, 0);
    p.place(s, 2, 10, 4);
    p.place(e, 0, 10, 2);
    p.place(wst, 4, 10, 2);
    p.fill(w - 5, 0, 0, w - 1, 9, 4, sand_st, air, false);
    p.fill(w - 4, 10, 1, w - 2, 10, 3, sand_st, sand_st, false);
    p.place(n, w - 3, 10, 0);
    p.place(s, w - 3, 10, 4);
    p.place(e, w - 5, 10, 2);
    p.place(wst, w - 1, 10, 2);
    p.fill(8, 0, 0, 12, 4, 4, sand_st, air, false);
    p.fill(9, 1, 0, 11, 3, 4, air, air, false);
    for (x, y) in [(9, 1), (9, 2), (9, 3), (10, 3), (11, 3), (11, 2), (11, 1)] {
        p.place(cut, x, y, 1);
    }
    p.fill(4, 1, 1, 8, 3, 3, sand_st, air, false);
    p.fill(4, 1, 2, 8, 2, 2, air, air, false);
    p.fill(12, 1, 1, 16, 3, 3, sand_st, air, false);
    p.fill(12, 1, 2, 16, 2, 2, air, air, false);
    p.fill(5, 4, 5, w - 6, 4, d - 6, sand_st, sand_st, false);
    p.fill(9, 4, 9, 11, 4, 11, air, air, false);
    for (x, z) in [(8, 8), (12, 8), (8, 12), (12, 12)] {
        p.fill(x, 1, z, x, 3, z, cut, cut, false);
    }
    p.fill(1, 1, 5, 4, 4, 11, sand_st, sand_st, false);
    p.fill(w - 5, 1, 5, w - 2, 4, 11, sand_st, sand_st, false);
    p.fill(6, 7, 9, 6, 7, 11, sand_st, sand_st, false);
    p.fill(w - 7, 7, 9, w - 7, 7, 11, sand_st, sand_st, false);
    p.fill(5, 5, 9, 5, 7, 11, cut, cut, false);
    p.fill(w - 6, 5, 9, w - 6, 7, 11, cut, cut, false);
    p.place(air, 5, 5, 10);
    p.place(air, 5, 6, 10);
    p.place(air, 6, 6, 10);
    p.place(air, w - 6, 5, 10);
    p.place(air, w - 6, 6, 10);
    p.place(air, w - 7, 6, 10);
    p.fill(2, 4, 4, 2, 6, 4, air, air, false);
    p.fill(w - 3, 4, 4, w - 3, 6, 4, air, air, false);
    p.place(n, 2, 4, 5);
    p.place(n, 2, 3, 4);
    p.place(n, w - 3, 4, 5);
    p.place(n, w - 3, 3, 4);
    p.fill(1, 1, 3, 2, 2, 3, sand_st, sand_st, false);
    p.fill(w - 3, 1, 3, w - 2, 2, 3, sand_st, sand_st, false);
    p.place(sand_st, 1, 1, 2);
    p.place(sand_st, w - 2, 1, 2);
    let slab = p.state("sandstone_slab");
    p.place(slab, 1, 2, 2);
    p.place(slab, w - 2, 2, 2);
    p.place(wst, 2, 1, 2);
    p.place(e, w - 3, 1, 2);
    p.fill(4, 3, 5, 4, 3, 17, sand_st, sand_st, false);
    p.fill(w - 5, 3, 5, w - 5, 3, 17, sand_st, sand_st, false);
    p.fill(3, 1, 5, 4, 2, 16, air, air, false);
    p.fill(w - 6, 1, 5, w - 5, 2, 16, air, air, false);
    let mut z = 5;
    while z <= 17 {
        p.place(cut, 4, 1, z);
        p.place(chis, 4, 2, z);
        p.place(cut, w - 5, 1, z);
        p.place(chis, w - 5, 2, z);
        z += 2;
    }
    for (x, z) in
        [(10, 7), (10, 8), (9, 9), (11, 9), (8, 10), (12, 10), (7, 10), (13, 10), (9, 11), (11, 11), (10, 12), (10, 13)]
    {
        p.place(orange, x, 0, z);
    }
    p.place(blue, 10, 0, 10);
    let column = [
        (2, 1, cut),
        (2, 2, orange),
        (2, 3, cut),
        (3, 1, cut),
        (3, 2, orange),
        (3, 3, cut),
        (4, 1, orange),
        (4, 2, chis),
        (4, 3, orange),
        (5, 1, cut),
        (5, 2, orange),
        (5, 3, cut),
        (6, 1, orange),
        (6, 2, chis),
        (6, 3, orange),
        (7, 1, orange),
        (7, 2, orange),
        (7, 3, orange),
        (8, 1, cut),
        (8, 2, cut),
        (8, 3, cut),
    ];
    for x in [0, w - 1] {
        for &(y, z, s) in &column {
            p.place(s, x, y, z);
        }
    }
    for x in [2, w - 3] {
        for &(y, dz, s) in &column {
            // Same pattern on the facade (z = 0), x − 1, x, x + 1.
            p.place(s, x + dz - 2, y, 0);
        }
    }
    p.fill(8, 4, 0, 12, 6, 0, cut, cut, false);
    p.place(air, 8, 6, 0);
    p.place(air, 12, 6, 0);
    p.place(orange, 9, 5, 0);
    p.place(chis, 10, 5, 0);
    p.place(orange, 11, 5, 0);
    p.fill(8, -14, 8, 12, -11, 12, cut, cut, false);
    p.fill(8, -10, 8, 12, -10, 12, chis, chis, false);
    p.fill(8, -9, 8, 12, -9, 12, cut, cut, false);
    p.fill(8, -8, 8, 12, -1, 12, sand_st, sand_st, false);
    p.fill(9, -11, 9, 11, -1, 11, air, air, false);
    let plate = p.state("stone_pressure_plate");
    p.place(plate, 10, -11, 10);
    let tnt = p.state("tnt");
    p.fill(9, -13, 9, 11, -13, 11, tnt, air, false);
    for (ax, az, cx, cz) in [(8, 10, 7, 10), (12, 10, 13, 10), (10, 8, 10, 7), (10, 12, 10, 13)] {
        p.place(air, ax, -11, az);
        p.place(air, ax, -10, az);
        p.place(chis, cx, -10, cz);
        p.place(cut, cx, -11, cz);
    }
    if let SKind::Pyramid { chests, .. } = &mut st.kind {
        // The game's horizontal order: south, west, north, east (2D index).
        for (i, (dx, dz)) in [(2, Dir::North), (3, Dir::East), (0, Dir::South), (1, Dir::West)]
            .iter()
            .map(|(i, d)| (*i, (d.offset()[0] * 2, d.offset()[2] * 2)))
        {
            if !chests[i] {
                let w = p.world(10 + dx, -11, 10 + dz);
                chests[i] = p.chest(rng, w, "minecraft:chests/desert_pyramid", None);
            }
        }
    }
    // Cave.
    let (x, y, z) = (16, -4, 13);
    let ccw = p.tr.rotate(p.state("sandstone_stairs"), Rot::Ccw90);
    p.place(ccw, 13, -1, 17);
    p.place(ccw, 14, -2, 17);
    p.place(ccw, 15, -3, 17);
    let sand = p.state("sand");
    let variant = p.r.region_rng.next_bool();
    for dx in [-4, -3, -2, -1, 0] {
        p.place(sand, x + dx, y + 4, z + 4);
    }
    p.place(sand, x - 2, y + 3, z + 4);
    p.place(if variant { sand } else { sand_st }, x - 1, y + 3, z + 4);
    p.place(if !variant { sand } else { sand_st }, x, y + 3, z + 4);
    p.place(sand, x - 1, y + 2, z + 4);
    p.place(sand_st, x, y + 2, z + 4);
    p.place(sand, x, y + 1, z + 4);
    for (yy, s) in [(y + 1, cut), (y + 2, chis), (-1, cut)] {
        p.fill(x - 3, yy, z - 3, x - 3, yy, z + 2, s, s, true);
        p.fill(x + 3, yy, z - 3, x + 3, yy, z + 2, s, s, true);
        p.fill(x - 3, yy, z - 3, x + 3, yy, z - 2, s, s, true);
        p.fill(x - 3, yy, z + 3, x + 3, yy, z + 3, s, s, true);
    }
    let mut sandpos: Vec<[i32; 3]> = Vec::new();
    for yy in y + 1..=y + 3 {
        for xx in x - 2..=x + 2 {
            for zz in z - 2..=z + 2 {
                sandpos.push(p.world(xx, yy, zz));
            }
        }
    }
    // Collapsed roof.
    for xx in x - 2..=x + 2 {
        for zz in z - 2..=z + 2 {
            let s = if p.r.region_rng.next_float() < 0.33 { sand_st } else { sand };
            p.place(s, xx, y + 4, zz);
        }
    }
    let corner = p.world(x - 2, y + 4, z - 2);
    let mut base = DecoRng::legacy(p.r.seed);
    let fork = base.next_long();
    let mut rr = DecoRng::legacy(crate::rng::position_seed(corner[0], corner[1], corner[2]) ^ fork);
    let rx = rr.next_int_bounded(5) + x - 2;
    let rz = rr.next_int_bounded(5) + z - 2;
    let roof = p.world(rx, y + 4, rz);
    p.place(blue, x, y, z);
    for (dx, dz) in [(1, -1), (1, 1), (-1, -1), (-1, 1), (2, 0), (-2, 0), (0, 2), (0, -2)] {
        p.place(orange, x + dx, y, z + dz);
    }
    p.place(orange, x + 3, y, z);
    sandpos.push(p.world(x + 3, y + 1, z));
    sandpos.push(p.world(x + 3, y + 2, z));
    p.place(cut, x + 4, y + 1, z);
    p.place(chis, x + 4, y + 2, z);
    p.place(orange, x - 3, y, z);
    sandpos.push(p.world(x - 3, y + 1, z));
    sandpos.push(p.world(x - 3, y + 2, z));
    p.place(cut, x - 4, y + 1, z);
    p.place(chis, x - 4, y + 2, z);
    p.place(orange, x, y, z + 3);
    sandpos.push(p.world(x, y + 1, z + 3));
    sandpos.push(p.world(x, y + 2, z + 3));
    p.place(orange, x, y, z - 3);
    sandpos.push(p.world(x, y + 1, z - 3));
    sandpos.push(p.world(x, y + 2, z - 3));
    p.place(cut, x, y + 1, z - 4);
    p.place(chis, x, -2, z - 4);
    if let SKind::Pyramid { sand, roof: r, .. } = &mut st.kind {
        sand.extend(sandpos);
        *r = roof;
    }
}

/// After the pyramid placement (§ 8.11.2): suspicious sand chosen, the rest as sand.
pub fn pyramid_after(r: &mut Region, pieces: &[Piece], zone: BBox) {
    let b = r.b;
    let mut set: Vec<[i32; 3]> = Vec::new();
    let mut all: Option<BBox> = None;
    let mut roofs = Vec::new();
    for pc in pieces {
        if let Piece::Coded { data, .. } = pc
            && let CodedPiece::Scattered(sp) = &**data
        {
            let st = sp.state.lock().expect("piece");
            match &mut all {
                Some(a) => a.encapsulate(&st.bbox),
                None => all = Some(st.bbox),
            }
            if let SKind::Pyramid { sand, roof, .. } = &st.kind {
                set.extend(sand.iter().copied());
                roofs.push(*roof);
            }
        }
    }
    let Some(all) = all else { return };
    let susp = |r: &mut Region, p: [i32; 3]| {
        if zone.contains(p)
            && let Some(id) = b.block_id("suspicious_sand")
        {
            r.set_p(p, b.default_state(id), 2);
            let seed = ((i64::from(p[0]) & 0x3FF_FFFF) << 38)
                | (i64::from(p[1]) & 0xFFF)
                | ((i64::from(p[2]) & 0x3FF_FFFF) << 12);
            r.note_block_entity(p, BeData::Loot { table: "minecraft:archaeology/desert_pyramid", seed });
        }
    };
    for rf in roofs {
        susp(r, rf);
    }
    set.sort_by(|a, b| (a[1], a[2], a[0]).cmp(&(b[1], b[2], b[0])));
    set.dedup();
    let c = all.center();
    let mut base = DecoRng::legacy(r.seed);
    let fork = base.next_long();
    let mut rr = DecoRng::legacy(crate::rng::position_seed(c[0], c[1], c[2]) ^ fork);
    rr.shuffle(&mut set);
    let mut n = (set.len() as i32).min(5 + rr.next_int_bounded(3));
    let sand = b.default_state(b.k.sand);
    for p in set {
        if n > 0 {
            n -= 1;
            susp(r, p);
        } else if zone.contains(p) {
            r.set_p(p, sand, 2);
        }
    }
}

fn jungle(p: &mut P, st: &mut ScatState, rng: &mut DecoRng) {
    let cobble = p.state("cobblestone");
    let mossy = p.state("mossy_cobblestone");
    let air = p.state("air");
    // Box with "mossy stone" selector: one draw per cell (y, x, z), even outside the cut-out.
    let sel = |p: &mut P, rng: &mut DecoRng, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32| {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    let s = if rng.next_float() < 0.4 { cobble } else { mossy };
                    p.place(s, x, y, z);
                }
            }
        }
    };
    sel(p, rng, 0, -4, 0, 11, 0, 14);
    sel(p, rng, 2, 1, 2, 9, 2, 2);
    sel(p, rng, 2, 1, 12, 9, 2, 12);
    sel(p, rng, 2, 1, 3, 2, 2, 11);
    sel(p, rng, 9, 1, 3, 9, 2, 11);
    sel(p, rng, 1, 3, 1, 10, 6, 1);
    sel(p, rng, 1, 3, 13, 10, 6, 13);
    sel(p, rng, 1, 3, 2, 1, 6, 12);
    sel(p, rng, 10, 3, 2, 10, 6, 12);
    sel(p, rng, 2, 3, 2, 9, 3, 12);
    sel(p, rng, 2, 6, 2, 9, 6, 12);
    sel(p, rng, 3, 7, 3, 8, 7, 11);
    sel(p, rng, 4, 8, 4, 7, 8, 10);
    for b in [
        [3, 1, 3, 8, 2, 11],
        [4, 3, 6, 7, 3, 9],
        [2, 4, 2, 9, 5, 12],
        [4, 6, 5, 7, 6, 9],
        [5, 7, 6, 6, 7, 8],
        [5, 1, 2, 6, 2, 2],
        [5, 2, 12, 6, 2, 12],
        [5, 5, 1, 6, 5, 1],
        [5, 5, 13, 6, 5, 13],
    ] {
        p.fill(b[0], b[1], b[2], b[3], b[4], b[5], air, air, false);
    }
    for (x, z) in [(1, 5), (10, 5), (1, 9), (10, 9)] {
        p.place(air, x, 5, z);
    }
    for z in [0, 14] {
        for x in [2, 4, 7, 9] {
            sel(p, rng, x, 4, z, x, 5, z);
        }
    }
    sel(p, rng, 5, 6, 0, 6, 6, 0);
    for x in [0, 11] {
        let mut z = 2;
        while z <= 12 {
            sel(p, rng, x, 4, z, x, 5, z);
            z += 2;
        }
        sel(p, rng, x, 6, 5, x, 6, 5);
        sel(p, rng, x, 6, 9, x, 6, 9);
    }
    for b in [
        [2, 7, 2, 2, 9, 2],
        [9, 7, 2, 9, 9, 2],
        [2, 7, 12, 2, 9, 12],
        [9, 7, 12, 9, 9, 12],
        [4, 9, 4, 4, 9, 4],
        [7, 9, 4, 7, 9, 4],
        [4, 9, 10, 4, 9, 10],
        [7, 9, 10, 7, 9, 10],
        [5, 9, 7, 6, 9, 7],
    ] {
        sel(p, rng, b[0], b[1], b[2], b[3], b[4], b[5]);
    }
    let e = stairs(p, "cobblestone_stairs", Dir::East);
    let w = stairs(p, "cobblestone_stairs", Dir::West);
    let s = stairs(p, "cobblestone_stairs", Dir::South);
    let n = stairs(p, "cobblestone_stairs", Dir::North);
    for (st_, x, y, z) in [
        (n, 5, 9, 6),
        (n, 6, 9, 6),
        (s, 5, 9, 8),
        (s, 6, 9, 8),
        (n, 4, 0, 0),
        (n, 5, 0, 0),
        (n, 6, 0, 0),
        (n, 7, 0, 0),
        (n, 4, 1, 8),
        (n, 4, 2, 9),
        (n, 4, 3, 10),
        (n, 7, 1, 8),
        (n, 7, 2, 9),
        (n, 7, 3, 10),
    ] {
        p.place(st_, x, y, z);
    }
    sel(p, rng, 4, 1, 9, 4, 1, 9);
    sel(p, rng, 7, 1, 9, 7, 1, 9);
    sel(p, rng, 4, 1, 10, 7, 2, 10);
    sel(p, rng, 5, 4, 5, 6, 4, 5);
    p.place(e, 4, 4, 5);
    p.place(w, 7, 4, 5);
    for i in 0..4 {
        p.place(s, 5, -i, 6 + i);
        p.place(s, 6, -i, 6 + i);
        p.fill(5, -i, 7 + i, 6, -i, 9 + i, air, air, false);
    }
    p.fill(1, -3, 12, 10, -1, 13, air, air, false);
    p.fill(1, -3, 1, 3, -1, 13, air, air, false);
    p.fill(1, -3, 1, 9, -1, 5, air, air, false);
    let mut z = 1;
    while z <= 13 {
        sel(p, rng, 1, -3, z, 1, -2, z);
        z += 2;
    }
    let mut z = 2;
    while z <= 12 {
        sel(p, rng, 1, -1, z, 3, -1, z);
        z += 2;
    }
    sel(p, rng, 2, -2, 1, 5, -2, 1);
    sel(p, rng, 7, -2, 1, 9, -2, 1);
    sel(p, rng, 6, -3, 1, 6, -3, 1);
    sel(p, rng, 6, -1, 1, 6, -1, 1);
    let hook = |p: &P, d: Dir| with(p, stairs(p, "tripwire_hook", d), "attached", "true");
    let wire = |p: &P, a: &str, c: &str| {
        let s = p.state("tripwire");
        let s = with(p, s, a, "true");
        let s = with(p, s, c, "true");
        with(p, s, "attached", "true")
    };
    let rs = |p: &P, sides: &[(&str, &str)]| {
        let mut s = p.state("redstone_wire");
        for (k, v) in sides {
            s = with(p, s, k, v);
        }
        s
    };
    let h = hook(p, Dir::East);
    p.place(h, 1, -3, 8);
    let h = hook(p, Dir::West);
    p.place(h, 4, -3, 8);
    let wr = wire(p, "east", "west");
    p.place(wr, 2, -3, 8);
    p.place(wr, 3, -3, 8);
    let ns = rs(p, &[("north", "side"), ("south", "side")]);
    for z in (2..=7).rev() {
        p.place(ns, 5, -3, z);
    }
    let r1 = rs(p, &[("north", "side"), ("west", "side")]);
    p.place(r1, 5, -3, 1);
    let r2 = rs(p, &[("east", "side"), ("west", "side")]);
    p.place(r2, 4, -3, 1);
    p.place(mossy, 3, -3, 1);
    let SKind::Jungle { trap1, trap2, main, hidden } = &mut st.kind else { return };
    if !*trap1 {
        *trap1 = dispenser(p, rng, 3, -2, 1, Dir::North);
    }
    let vine = p.state("vine");
    let v = with(p, vine, "south", "true");
    p.place(v, 3, -2, 2);
    let h = hook(p, Dir::North);
    p.place(h, 7, -3, 1);
    let h = hook(p, Dir::South);
    p.place(h, 7, -3, 5);
    let wr = wire(p, "north", "south");
    for z in 2..=4 {
        p.place(wr, 7, -3, z);
    }
    let r3 = rs(p, &[("east", "side"), ("west", "side")]);
    p.place(r3, 8, -3, 6);
    let r4 = rs(p, &[("west", "side"), ("south", "side")]);
    p.place(r4, 9, -3, 6);
    let r5 = rs(p, &[("north", "side"), ("south", "up")]);
    p.place(r5, 9, -3, 5);
    p.place(mossy, 9, -3, 4);
    p.place(ns, 9, -2, 4);
    if !*trap2 {
        *trap2 = dispenser(p, rng, 9, -2, 3, Dir::West);
    }
    let v = with(p, vine, "east", "true");
    p.place(v, 8, -1, 3);
    p.place(v, 8, -2, 3);
    if !*main {
        let w = p.world(8, -3, 3);
        *main = p.chest(rng, w, "minecraft:chests/jungle_temple", None);
    }
    for (x, y, z) in
        [(9, -3, 2), (8, -3, 1), (4, -3, 5), (5, -2, 5), (5, -1, 5), (6, -3, 5), (7, -2, 5), (7, -1, 5), (8, -3, 5)]
    {
        p.place(mossy, x, y, z);
    }
    sel(p, rng, 9, -1, 1, 9, -1, 5);
    p.fill(8, -3, 8, 10, -1, 10, air, air, false);
    let chis = p.state("chiseled_stone_bricks");
    for x in 8..=10 {
        p.place(chis, x, -2, 11);
    }
    let lever = with(p, stairs(p, "lever", Dir::North), "face", "wall");
    for x in 8..=10 {
        p.place(lever, x, -2, 12);
    }
    sel(p, rng, 8, -3, 8, 8, -3, 10);
    sel(p, rng, 10, -3, 8, 10, -3, 10);
    p.place(mossy, 10, -2, 9);
    p.place(ns, 8, -2, 9);
    p.place(ns, 8, -2, 10);
    let all = rs(p, &[("north", "side"), ("south", "side"), ("east", "side"), ("west", "side")]);
    p.place(all, 10, -1, 9);
    let piston = |p: &P, d: Dir| stairs(p, "sticky_piston", d);
    let pu = piston(p, Dir::Up);
    p.place(pu, 9, -2, 8);
    let pw = piston(p, Dir::West);
    p.place(pw, 10, -2, 8);
    p.place(pw, 10, -1, 8);
    let rep = stairs(p, "repeater", Dir::North);
    p.place(rep, 10, -2, 10);
    if !*hidden {
        let w = p.world(9, -3, 10);
        *hidden = p.chest(rng, w, "minecraft:chests/jungle_temple", None);
    }
}

fn dispenser(p: &mut P, rng: &mut DecoRng, x: i32, y: i32, z: i32, facing: Dir) -> bool {
    let w = p.world(x, y, z);
    let disp = p.b.block_id("dispenser");
    if !p.zone.contains(w) || disp == Some(p.b.block(p.r.get_p(w))) {
        return false;
    }
    let s = stairs(p, "dispenser", facing);
    p.place(s, x, y, z);
    if disp == Some(p.b.block(p.r.get_p(w))) {
        let seed = rng.next_long();
        p.r.note_block_entity(w, BeData::Loot { table: "minecraft:dispensers/jungle_temple", seed });
    }
    true
}

fn swamp(p: &mut P) {
    let planks = p.state("spruce_planks");
    let log = p.state("oak_log");
    let fence = p.state("oak_fence");
    let air = p.state("air");
    for b in [
        [1, 1, 1, 5, 1, 7],
        [1, 4, 2, 5, 4, 7],
        [2, 1, 0, 4, 1, 0],
        [2, 2, 2, 3, 3, 2],
        [1, 2, 3, 1, 3, 6],
        [5, 2, 3, 5, 3, 6],
        [2, 2, 7, 4, 3, 7],
    ] {
        p.fill(b[0], b[1], b[2], b[3], b[4], b[5], planks, planks, false);
    }
    for (x, z) in [(1, 2), (5, 2), (1, 7), (5, 7)] {
        p.fill(x, 0, z, x, 3, z, log, log, false);
    }
    p.place(fence, 2, 3, 2);
    p.place(fence, 3, 3, 7);
    p.place(air, 1, 3, 4);
    p.place(air, 5, 3, 4);
    p.place(air, 5, 3, 5);
    let pot = p.state("potted_red_mushroom");
    p.place(pot, 1, 3, 5);
    let ct = p.state("crafting_table");
    p.place(ct, 3, 2, 6);
    let cauldron = p.state("cauldron");
    p.place(cauldron, 4, 2, 6);
    p.place(fence, 1, 2, 1);
    p.place(fence, 5, 2, 1);
    let n = stairs(p, "spruce_stairs", Dir::North);
    let e = stairs(p, "spruce_stairs", Dir::East);
    let w = stairs(p, "spruce_stairs", Dir::West);
    let s = stairs(p, "spruce_stairs", Dir::South);
    p.fill(0, 4, 1, 6, 4, 1, n, n, false);
    p.fill(0, 4, 2, 0, 4, 7, e, e, false);
    p.fill(6, 4, 2, 6, 4, 7, w, w, false);
    p.fill(0, 4, 8, 6, 4, 8, s, s, false);
    let n_or = with(p, n, "shape", "outer_right");
    p.place(n_or, 0, 4, 1);
    let n_ol = with(p, n, "shape", "outer_left");
    p.place(n_ol, 6, 4, 1);
    let s_ol = with(p, s, "shape", "outer_left");
    p.place(s_ol, 0, 4, 8);
    let s_or = with(p, s, "shape", "outer_right");
    p.place(s_or, 6, 4, 8);
    for z in [2, 7] {
        for x in [1, 5] {
            p.fill_column_down(log, x, -1, z);
        }
    }
}
