//! Placement of the mineshaft pieces (`generation-decoration.md` § 8.6.3–8.6.7).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::decor::Ctx;
use crate::decor_blocks::{Dir, State};
use crate::decor_region::BeData;
use crate::decor_rng::DecoRng;
use crate::structure::StructureGen;
use crate::structure_coded::CodedPiece;
use crate::structure_piece::P;
use crate::template::BBox;

struct Mat {
    wood: State,
    planks: State,
    fence: State,
    air: State,
}

fn mat(p: &P, mesa: bool) -> Mat {
    let w = if mesa { "dark_oak" } else { "oak" };
    Mat {
        wood: p.state(&format!("{w}_log")),
        planks: p.state(&format!("{w}_planks")),
        fence: p.state(&format!("{w}_fence")),
        air: p.state("cave_air"),
    }
}

/// Common guard: blocking biome at the centre or liquid on a face of the enlarged box.
fn invalid(p: &P, blocking: &[bool]) -> bool {
    let (bb, z) = (p.bbox, p.zone);
    let x0 = (bb.min[0] - 1).max(z.min[0]);
    let y0 = (bb.min[1] - 1).max(z.min[1]);
    let z0 = (bb.min[2] - 1).max(z.min[2]);
    let x1 = (bb.max[0] + 1).min(z.max[0]);
    let y1 = (bb.max[1] + 1).min(z.max[1]);
    let z1 = (bb.max[2] + 1).min(z.max[2]);
    let biome = p.r.biome((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
    if blocking.get(biome as usize).copied().unwrap_or(false) {
        return true;
    }
    let liquid = |x: i32, y: i32, z: i32| p.b.has_flag(p.r.get(x, y, z), rc_data::flags::LIQUID);
    for x in x0..=x1 {
        for zz in z0..=z1 {
            if liquid(x, y0, zz) || liquid(x, y1, zz) {
                return true;
            }
        }
    }
    for x in x0..=x1 {
        for y in y0..=y1 {
            if liquid(x, y, z0) || liquid(x, y, z1) {
                return true;
            }
        }
    }
    for zz in z0..=z1 {
        for y in y0..=y1 {
            if liquid(x0, y, zz) || liquid(x1, y, zz) {
                return true;
            }
        }
    }
    false
}

/// "Floor": planks if interior and without solid upper face.
fn planks_floor(p: &mut P, m: &Mat, x: i32, y: i32, z: i32) {
    if p.is_interior(x, y, z) {
        let w = p.world(x, y, z);
        if !p.b.sturdy(p.r.get_p(w), Dir::Up) {
            p.r.set_p(w, m.planks, 2);
        }
    }
}

pub fn place(cx: &mut Ctx, sg: &StructureGen, d: &CodedPiece, bbox: BBox, rng: &mut DecoRng, zone: BBox) {
    let blocking = &sg.mineshaft_blocking;
    let tr = &sg.templates.transforms;

    let (orient, mesa) = match d {
        CodedPiece::MsCorridor { dir, mesa, .. } | CodedPiece::MsStairs { dir, mesa } => (Some(*dir), *mesa),
        CodedPiece::MsCrossing { mesa, .. } | CodedPiece::MsRoom { mesa, .. } => (None, *mesa),
        _ => {
            return;
        }
    };
    let mut p = P::new(cx.r, tr, zone, bbox, orient);
    let m = mat(&p, mesa);
    let chain = p.b.block_id("iron_chain");
    p.keep = [m.planks, m.wood, m.fence].iter().map(|&s| p.b.block(s)).chain(chain).collect();
    if invalid(&p, blocking) {
        return;
    }
    let air = m.air;
    match d {
        CodedPiece::MsRoom { entrances, .. } => {
            let bb = bbox;
            p.fill(
                bb.min[0],
                bb.min[1] + 1,
                bb.min[2],
                bb.max[0],
                (bb.min[1] + 3).min(bb.max[1]),
                bb.max[2],
                air,
                air,
                false,
            );
            for e in entrances {
                p.fill(e.min[0], e.max[1] - 2, e.min[2], e.max[0], e.max[1], e.max[2], air, air, false);
            }
            p.upper_half_sphere(bb.min[0], bb.min[1] + 4, bb.min[2], bb.max[0], bb.max[1], bb.max[2], air, false);
        }
        CodedPiece::MsStairs { .. } => {
            p.fill(0, 5, 0, 2, 7, 1, air, air, false);
            p.fill(0, 0, 7, 2, 2, 8, air, air, false);
            for i in 0..5 {
                p.fill(0, 5 - i - i32::from(i < 4), 2 + i, 2, 7 - i, 2 + i, air, air, false);
            }
        }
        CodedPiece::MsCrossing { two_floors, .. } => {
            let bb = bbox;
            let (x0, y0, z0, x1, y1, z1) = (bb.min[0], bb.min[1], bb.min[2], bb.max[0], bb.max[1], bb.max[2]);
            if *two_floors {
                p.fill(x0 + 1, y0, z0, x1 - 1, y0 + 2, z1, air, air, false);
                p.fill(x0, y0, z0 + 1, x1, y0 + 2, z1 - 1, air, air, false);
                p.fill(x0 + 1, y1 - 2, z0, x1 - 1, y1, z1, air, air, false);
                p.fill(x0, y1 - 2, z0 + 1, x1, y1, z1 - 1, air, air, false);
                p.fill(x0 + 1, y0 + 3, z0 + 1, x1 - 1, y0 + 3, z1 - 1, air, air, false);
            } else {
                p.fill(x0 + 1, y0, z0, x1 - 1, y1, z1, air, air, false);
                p.fill(x0, y0, z0 + 1, x1, y1, z1 - 1, air, air, false);
            }
            for (x, z) in [(x0 + 1, z0 + 1), (x0 + 1, z1 - 1), (x1 - 1, z0 + 1), (x1 - 1, z1 - 1)] {
                if !p.is_air(x, y1 + 1, z) {
                    p.fill(x, y0, z, x, y1, z, m.planks, air, false);
                }
            }
            for x in x0..=x1 {
                for z in z0..=z1 {
                    planks_floor(&mut p, &m, x, y0 - 1, z);
                }
            }
        }
        CodedPiece::MsCorridor { rails, spider, sections, spider_placed, .. } => {
            corridor(&mut p, &m, rng, *rails, *spider, *sections, spider_placed);
        }
        _ => {}
    }
}

fn corridor(p: &mut P, m: &Mat, rng: &mut DecoRng, rails: bool, spider: bool, sections: i32, placed: &AtomicBool) {
    let air = m.air;
    let len = sections * 5 - 1;
    let cobweb = p.b.default_state(p.b.k.cobweb);
    p.fill(0, 0, 0, 2, 1, len, air, air, false);
    p.maybe_fill(rng, 0.8, 0, 2, 0, 2, 2, len, air, air, false, false);
    if spider {
        p.maybe_fill(rng, 0.6, 0, 0, 0, 2, 1, len, cobweb, air, false, true);
    }
    for s in 0..sections {
        let z = 2 + s * 5;
        support(p, m, rng, z);
        for (pr, x, zz) in [
            (0.1, 0, z - 1),
            (0.1, 2, z - 1),
            (0.1, 0, z + 1),
            (0.1, 2, z + 1),
            (0.05, 0, z - 2),
            (0.05, 2, z - 2),
            (0.05, 0, z + 2),
            (0.05, 2, z + 2),
        ] {
            if p.is_interior(x, 2, zz) && rng.next_float() < pr && sturdy_neighbours(p, x, 2, zz, 2) {
                p.place(cobweb, x, 2, zz);
            }
        }
        if rng.next_int_bounded(100) == 0 {
            minecart_chest(p, rng, 2, 0, z - 1);
        }
        if rng.next_int_bounded(100) == 0 {
            minecart_chest(p, rng, 0, 0, z + 1);
        }
        // Flag of the piece (not of the chunk): a single placement per corridor, and no more draws
        // in the chunks decorated after the one that placed it.
        if spider && !placed.load(Ordering::Relaxed) {
            let nz = z - 1 + rng.next_int_bounded(3);
            let w = p.world(1, 0, nz);
            if p.zone.contains(w) && p.is_interior(1, 0, nz) {
                placed.store(true, Ordering::Relaxed);
                let sp = p.b.default_state(p.b.k.spawner);
                p.r.set_p(w, sp, 2);
                p.r.note_block_entity(w, BeData::Spawner { entity: "minecraft:cave_spider" });
            }
        }
    }
    for x in 0..=2 {
        for z in 0..=len {
            planks_floor(p, m, x, -1, z);
        }
    }
    double_support(p, m, 0, -1, 2);
    if sections > 1 {
        double_support(p, m, 0, -1, len - 2);
    }
    if rails {
        let rail = rail_state(p, "north_south");
        for z in 0..=len {
            let floor = p.get(1, -1, z);
            if !p.b.is_air(floor) && p.b.solid_render(floor) {
                let pr = if p.is_interior(1, 0, z) { 0.7 } else { 0.9 };
                p.maybe_block(rng, pr, 1, 0, z, rail);
            }
        }
    }
}

fn rail_state(p: &P, shape: &str) -> State {
    let s = p.state("rail");
    match p.b.prop("shape") {
        Some(pr) => p.b.with_str(s, pr, shape),
        None => s,
    }
}

fn minecart_chest(p: &mut P, rng: &mut DecoRng, x: i32, y: i32, z: i32) {
    let w = p.world(x, y, z);
    if p.zone.contains(w) && p.b.is_air(p.r.get_p(w)) && !p.b.is_air(p.r.get(w[0], w[1] - 1, w[2])) {
        let shape = if rng.next_bool() { "north_south" } else { "east_west" };
        let s = rail_state(p, shape);
        p.place(s, x, y, z);
        // Chest minecart placed at the centre of the cell, loot and seed of the game.
        let seed = rng.next_long();
        let at = [f64::from(w[0]) + 0.5, f64::from(w[1]) + 0.5, f64::from(w[2]) + 0.5];
        let e = crate::structure_entities::with(
            crate::structure_entities::mob("minecraft:chest_minecart", at, 0.0, false),
            vec![
                ("LootTable".into(), rc_proto::nbt::Nbt::String("minecraft:chests/abandoned_mineshaft".into())),
                ("LootTableSeed".into(), rc_proto::nbt::Nbt::Long(seed)),
            ],
        );
        p.r.add_entity(w[0], w[2], e);
    }
}

fn sturdy_neighbours(p: &P, x: i32, y: i32, z: i32, count: i32) -> bool {
    let w = p.world(x, y, z);
    let mut n = 0;
    for d in [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East] {
        let o = d.offset();
        let q = [w[0] + o[0], w[1] + o[1], w[2] + o[2]];
        if p.zone.contains(q) && p.b.sturdy(p.r.get_p(q), d.opposite()) {
            n += 1;
            if n >= count {
                return true;
            }
        }
    }
    false
}

fn support(p: &mut P, m: &Mat, rng: &mut DecoRng, z: i32) {
    let (x0, y0, y1, x1) = (0, 0, 2, 2);
    for x in x0..=x1 {
        if p.is_air(x, y1 + 1, z) {
            return;
        }
    }
    let b = p.b;
    let (west, east) = (b.prop("west"), b.prop("east"));
    let fw = west.map_or(m.fence, |pr| b.with_bool(m.fence, pr, true));
    let fe = east.map_or(m.fence, |pr| b.with_bool(m.fence, pr, true));
    p.fill(x0, y0, z, x0, y1 - 1, z, fw, m.air, false);
    p.fill(x1, y0, z, x1, y1 - 1, z, fe, m.air, false);
    if rng.next_int_bounded(4) == 0 {
        p.fill(x0, y1, z, x0, y1, z, m.planks, m.air, false);
        p.fill(x1, y1, z, x1, y1, z, m.planks, m.air, false);
    } else {
        p.fill(x0, y1, z, x1, y1, z, m.planks, m.air, false);
        let torch = p.state("wall_torch");
        let facing = b.prop("facing");
        let t_s = facing.map_or(torch, |f| b.with_dir(torch, f, Dir::South));
        let t_n = facing.map_or(torch, |f| b.with_dir(torch, f, Dir::North));
        p.maybe_block(rng, 0.05, x0 + 1, y1, z - 1, t_s);
        p.maybe_block(rng, 0.05, x0 + 1, y1, z + 1, t_n);
    }
}

fn double_support(p: &mut P, m: &Mat, x: i32, y: i32, z: i32) {
    for xx in [x, x + 2] {
        if p.b.block(p.get(xx, y, z)) == p.b.block(m.planks) {
            pillar_or_chain(p, m, xx, y, z);
        }
    }
}

fn falling(p: &P, s: State) -> bool {
    let n = p.b.block_name(p.b.block(s));
    let n = n.strip_prefix("minecraft:").unwrap_or(n);
    n.ends_with("sand")
        || n.ends_with("gravel")
        || n.ends_with("concrete_powder")
        || n.ends_with("anvil")
        || n == "dragon_egg"
}

fn pillar_or_chain(p: &mut P, m: &Mat, x: i32, y: i32, z: i32) {
    let w = p.world(x, y, z);
    if !p.zone.contains(w) {
        return;
    }
    let wy = w[1];
    let max_y = p.r.max_y();
    let min_y = p.r.min_y;
    let (mut below, mut above) = (true, true);
    let mut d = 1;
    while below || above {
        if below {
            let s = p.r.get(w[0], wy - d, w[2]);
            let empty = p.replaceable_by_structures(s) && !p.b.is(s, p.b.k.lava);
            if !empty && p.b.sturdy(s, Dir::Up) {
                for yy in wy - d + 1..wy {
                    p.r.set(w[0], yy, w[2], m.wood, 2);
                }
                return;
            }
            below = d <= 20 && empty && wy - d > min_y + 1;
        }
        if above {
            let s = p.r.get(w[0], wy + d, w[2]);
            let empty = p.replaceable_by_structures(s);
            let center_down = p.b.facts(s).center & (1 << Dir::Down as u8) != 0;
            if !empty && center_down && !falling(p, s) {
                p.r.set(w[0], wy + 1, w[2], m.fence, 2);
                let chain = p.state("iron_chain");
                for yy in wy + 2..wy + d {
                    p.r.set(w[0], yy, w[2], chain, 2);
                }
                return;
            }
            above = d <= 50 && empty && wy + d < max_y;
        }
        d += 1;
    }
}
