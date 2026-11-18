//! Per-section remeshing: on the real columns of the reference capture (outside the repository) and
//! the models of the installed game, a block or a light cell changed at random. For each of the
//! 3 × 3 columns around the change:
//! - if `reach` designates it, the partial mesh (sections of `reach` redone, the others taken
//!   from the previous mesh) is identical to the full mesh of the new world;
//! - otherwise, its full mesh has not changed.
//!
//! Ignored (with a message) if the capture, the data cache or the game are missing; failure under
//! `RUSTCRAFT_OPT_REQUIRE=1`.

mod common;

use std::sync::Arc;

use common::{Cols, job, load, same};
use rc_mesh::blocks::{BiomePalette, Tables, mesh};
use rc_mesh::wire::reach;
use rc_world::edge;

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }

    /// Local coordinate, often on an edge or one cell from the edge (cases that touch the
    /// neighbours: light is read at one block, states at two).
    fn coord(&mut self) -> usize {
        match self.next(6) {
            0 => 0,
            1 => 1,
            2 => 14,
            3 => 15,
            _ => self.next(16) as usize,
        }
    }
}

/// Compares, for the 3 × 3 columns around (`cx`, `cz`), the mesh before (`old`), the
/// full mesh after (`new`) and the partial mesh designated by `reach`.
#[allow(clippy::too_many_arguments)]
fn check(
    old: &Cols,
    new: &Cols,
    t: &Tables,
    palette: &BiomePalette,
    (cx, cz): (i32, i32),
    hit: Option<(rc_mesh::wire::Sections, std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>)>,
    what: &str,
) -> usize {
    let mut partial = 0;
    for dx in -1..=1 {
        for dz in -1..=1 {
            let (x, z) = (cx + dx, cz + dz);
            if !old.contains_key(&(x, z)) {
                continue;
            }
            let before = mesh(&job(old, x, z), t, palette, -64);
            let after = mesh(&job(new, x, z), t, palette, -64);
            match &hit {
                Some((mask, xs, zs)) if xs.contains(&dx) && zs.contains(&dz) => {
                    let mut j = job(new, x, z);
                    let mut words = Vec::new();
                    rc_mesh::blocks::pack(&before, &mut words);
                    assert!(same(&rc_mesh::blocks::unpack(&words, 0).expect("flat"), &before));
                    j.reuse = Some((*mask, Arc::new(words)));
                    let p = mesh(&j, t, palette, -64);
                    assert!(same(&p, &after), "{what}: partial mesh of ({x}, {z}) differs from the full one");
                    // In-place write of the partial mesh over its base, on every path.
                    common::check_in_place(&before, &p, Some(*mask));
                    partial += 1;
                }
                _ => assert!(same(&before, &after), "{what}: column ({x}, {z}) changed out of reach"),
            }
        }
    }
    partial
}

/// Blocks changed one cell from the edge (1 or 14 on an axis) near the surface, between air and stone:
/// the ambient occlusion of a face of the neighbouring column reads the state two cells along
/// its normal. With the "cell on the edge" rule alone (`of_index`), a neighbour changes without being
/// designated; `near_of_index` designates it.
#[test]
fn block_two_cells_from_edge_reaches_neighbour() {
    let Some((d, cols, t, palette)) = load() else { return };
    let mut keys: Vec<(i32, i32)> = cols
        .keys()
        .copied()
        .filter(|&(x, z)| rc_mesh::wire::NEIGHBORS.iter().all(|(dx, dz)| cols.contains_key(&(x + dx, z + dz))))
        .collect();
    keys.sort_unstable();
    let stone = d.blocks.iter().find(|b| b.name == "minecraft:stone").expect("stone").first_state;
    let bits = d.state_bits() as u8;
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut trials, mut partial) = (0, 0);
    for _ in 0..400 {
        let (cx, cz) = keys[rng.next(keys.len() as u64) as usize];
        let center = &cols[&(cx, cz)];
        let sections = center.sections.len() as i32;
        let mut col = (**center).clone();
        let top = (0..sections as usize).rev().find(|&s| !col.sections[s].is_empty()).unwrap_or(4) as i32;
        let near = |r: &mut Rng| [1, 14][r.next(2) as usize];
        let mut c = [rng.next(16) as usize, rng.next(16) as usize, rng.next(16) as usize];
        let axis = rng.next(3) as usize;
        c[axis] = near(&mut rng);
        let (x, y, z) = (c[0], c[1], c[2]);
        let sy = (top - rng.next(2) as i32).max(0);
        let i = rc_world::block_index(x, y, z);
        let old = col.sections[sy as usize].blocks.get(i);
        let state = if t.is_air(old) { stone } else { 0 };
        col.sections[sy as usize].set_blocks(&[(i as u16, state)], bits, |s| t.is_air(s));
        let hit = reach(edge::near_of_index(i), sy, sections, false, |_| true);
        let what = format!("block ({x}, {y}, {z}) of section {sy} of ({cx}, {cz}) → #{state}");
        let mut after = cols.clone();
        after.insert((cx, cz), Arc::new(col));
        partial += check(&cols, &after, &t, &palette, (cx, cz), hit, &what);
        trials += 1;
    }
    eprintln!("{trials} changes near the edges, {partial} partial meshes identical to the full ones");
}

/// Partial remesh of one section whose neighbours above and below stay outside the remesh:
/// covering blocks on their border rows (row 15 below, row 0 above) hide the faces of the
/// section's boundary blocks. The partial mesh reads those rows (decoded alone, or from the whole
/// neighbour) and equals the full mesh, for both removal rules and both fill orders.
#[test]
fn partial_remesh_reads_the_border_rows_of_its_neighbours() {
    use rc_mesh::cull::CullPaths;
    let Some((d, cols, t, palette)) = load() else { return };
    let first = |name: &str| d.blocks.iter().find(|b| b.name == name).expect(name).first_state;
    let (stone, glass) = (first("minecraft:stone"), first("minecraft:glass"));
    let bits = d.state_bits() as u8;
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let mut checked = 0;
    for &(x, z) in keys.iter().step_by(keys.len().div_ceil(24)) {
        let mut col = (*cols[&(x, z)]).clone();
        // A section with neighbours above and below, near the surface (air above, often).
        let top = (0..col.sections.len()).rev().find(|&s| !col.sections[s].is_empty()).unwrap_or(4);
        let si = top.clamp(1, col.sections.len() - 2);
        let at = |x: usize, y: usize, z: usize| rc_world::block_index(x, y, z) as u16;
        let pattern = || (0..16).flat_map(|z| (0..16).map(move |x| (x, z))).filter(|&(x, z)| (x + 2 * z) % 3 == 0);
        let below: Vec<(u16, u32)> = pattern().map(|(x, z)| (at(x, 15, z), stone)).collect();
        let above: Vec<(u16, u32)> = pattern().map(|(x, z)| (at(x, 0, z), stone)).collect();
        let inside: Vec<(u16, u32)> = pattern()
            .flat_map(|(x, z)| [(at(x, 0, z), if x % 2 == 0 { stone } else { glass }), (at(x, 15, z), glass)])
            .collect();
        col.sections[si - 1].set_blocks(&below, bits, |s| t.is_air(s));
        col.sections[si + 1].set_blocks(&above, bits, |s| t.is_air(s));
        col.sections[si].set_blocks(&inside, bits, |s| t.is_air(s));
        let mut world = cols.clone();
        world.insert((x, z), Arc::new(col));
        let mut mask = rc_mesh::wire::Sections::default();
        mask.insert(si as i32);
        for shape in [false, true] {
            let d0 = CullPaths { shape, ..CullPaths::DEFAULT };
            let full = mesh(&common::job_with(&world, x, z, d0), &t, &palette, -64);
            let words: rc_mesh::blocks::Packed = Arc::new(rc_mesh::blocks::pack_exact(&full));
            for p in [d0, CullPaths { border_rows_only: false, ..d0 }, CullPaths { fill_rows: false, ..d0 }] {
                let mut j = common::job_with(&world, x, z, p);
                j.reuse = Some((mask, words.clone()));
                let part = mesh(&j, &t, &palette, -64);
                assert!(same(&part, &full), "({x}, {z}), section {si}, {p:?}: partial mesh differs from the full one");
                checked += 1;
            }
        }
    }
    eprintln!("{checked} partial meshes reading their neighbours' border rows identical to the full ones");
    assert!(checked >= 24 * 3, "{checked} checks");
}

/// A stair turned, a lever thrown, a redstone wire repowered or reshaped on the surface of real
/// columns: same face counts, faces that change group within the section. The partial mesh equals
/// the full one, and its in-place write (remeshed sections only compared, one range or several)
/// gives a full upload (`common::check_in_place`).
#[test]
fn in_place_follows_state_changes_of_stairs_levers_and_wire() {
    let Some((d, cols, t, palette)) = load() else { return };
    let mut keys: Vec<(i32, i32)> = cols
        .keys()
        .copied()
        .filter(|&(x, z)| rc_mesh::wire::NEIGHBORS.iter().all(|(dx, dz)| cols.contains_key(&(x + dx, z + dz))))
        .collect();
    keys.sort_unstable();
    let find = |name: &str| d.blocks.iter().find(|b| b.name == name).expect(name);
    let blocks = [find("minecraft:oak_stairs"), find("minecraft:lever"), find("minecraft:redstone_wire")];
    let bits = d.state_bits() as u8;
    let mut rng = Rng(0x51f1_5eed_c0ff_ee11);
    let (mut in_place, mut several, mut trials) = (0, 0, 0);
    for round in 0..120 {
        let (cx, cz) = keys[rng.next(keys.len() as u64) as usize];
        let mut col = (*cols[&(cx, cz)]).clone();
        let sections = col.sections.len() as i32;
        // First air cell above a non-air block, in the highest non-empty section.
        let Some(sy) = (0..col.sections.len()).rev().find(|&s| !col.sections[s].is_empty()) else { continue };
        let (x, z) = (rng.next(16) as usize, rng.next(16) as usize);
        let air = |c: &rc_world::Column, y: usize| t.is_air(c.sections[sy].blocks.get(rc_world::block_index(x, y, z)));
        let Some(y) = (1..16).find(|&y| air(&col, y) && !air(&col, y - 1)) else { continue };
        let b = blocks[round % 3];
        let pick = |r: &mut Rng| b.first_state + r.next(u64::from(b.state_count)) as u32;
        let (first, second) = (pick(&mut rng), pick(&mut rng));
        if first == second {
            continue;
        }
        let i = rc_world::block_index(x, y, z);
        col.sections[sy].set_blocks(&[(i as u16, first)], bits, |s| t.is_air(s));
        let mut world = cols.clone();
        world.insert((cx, cz), Arc::new(col.clone()));
        let before = mesh(&job(&world, cx, cz), &t, &palette, -64);
        col.sections[sy].set_blocks(&[(i as u16, second)], bits, |s| t.is_air(s));
        world.insert((cx, cz), Arc::new(col));
        let after = mesh(&job(&world, cx, cz), &t, &palette, -64);
        let (mask, ..) = reach(edge::near_of_index(i), sy as i32, sections, false, |_| true).expect("reach");
        let mut j = job(&world, cx, cz);
        j.reuse = Some((mask, Arc::new(rc_mesh::blocks::pack_exact(&before)) as rc_mesh::blocks::Packed));
        let p = mesh(&j, &t, &palette, -64);
        assert!(same(&p, &after), "{} #{first} -> #{second} at ({cx}, {cz}): partial mesh differs", b.name);
        if let Some(writes) = common::check_in_place(&before, &p, Some(mask)) {
            in_place += 1;
            several += usize::from(writes > 1);
        }
        trials += 1;
    }
    eprintln!("{trials} state changes, {in_place} written in place, {several} with several ranges");
    assert!(trials >= 60 && in_place >= 20, "draw too poor: {trials} / {in_place}");
}

#[test]
fn partial_remesh_equals_full_remesh() {
    let Some((d, cols, t, palette)) = load() else { return };
    let mut keys: Vec<(i32, i32)> = cols
        .keys()
        .copied()
        .filter(|&(x, z)| rc_mesh::wire::NEIGHBORS.iter().all(|(dx, dz)| cols.contains_key(&(x + dx, z + dz))))
        .collect();
    keys.sort_unstable();
    assert!(!keys.is_empty(), "no column surrounded by its 8 neighbours");
    let names = [
        "air",
        "stone",
        "glass",
        "water",
        "lava",
        "oak_leaves",
        "torch",
        "oak_slab",
        "grass_block",
        "short_grass",
        "oak_fence",
        "ice",
        "snow",
        "oak_stairs",
        "glowstone",
        "kelp",
        "seagrass",
    ];
    let blocks: Vec<&rc_data::BlockInfo> = names
        .iter()
        .filter_map(|n| d.blocks.iter().find(|b| b.name == *n || b.name == format!("minecraft:{n}")))
        .collect();
    assert!(blocks.len() >= 10, "test blocks not found");
    let bits = d.state_bits() as u8;
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut trials, mut partial) = (0, 0);
    for round in 0..256 {
        let (cx, cz) = keys[rng.next(keys.len() as u64) as usize];
        let center = &cols[&(cx, cz)];
        let sections = center.sections.len() as i32;
        let mut col = (**center).clone();
        let (x, y, z) = (rng.coord(), rng.coord(), rng.coord());
        let i = rc_world::block_index(x, y, z);
        let hit;
        let what;
        if round % 2 == 0 {
            // Block changed, in a non-empty section by preference (where there are faces).
            let filled: Vec<i32> = (0..sections).filter(|&s| !col.sections[s as usize].is_empty()).collect();
            let sy = filled.get(rng.next(filled.len().max(1) as u64) as usize).copied().unwrap_or(4);
            let b = blocks[rng.next(blocks.len() as u64) as usize];
            let state = b.first_state + rng.next(u64::from(b.state_count)) as u32;
            if col.sections[sy as usize].blocks.get(i) == state {
                continue;
            }
            col.sections[sy as usize].set_blocks(&[(i as u16, state)], bits, |s| t.is_air(s));
            hit = reach(edge::near_of_index(i), sy, sections, false, |_| true);
            what =
                format!("round {round}: block ({x}, {y}, {z}) of section {sy} of ({cx}, {cz}) → {} #{state}", b.name);
        } else {
            // Light cell changed (sky or block); one time in three, sky sheets without
            // data just below (read through the bottom row).
            let sky = rng.next(2) == 0;
            let hollow = rng.next(3) == 0;
            // Empty sheets: just above the highest non-empty section (its top faces then
            // read the sky of the bottom row of the changed sheet).
            let top = (0..sections as usize).rev().find(|&s| !col.sections[s].is_empty()).unwrap_or(4);
            let layer =
                if hollow { (top + 3).clamp(4, sections as usize) } else { 1 + rng.next(sections as u64) as usize };
            let mut old = (**center).clone();
            if hollow {
                for k in layer - 3..layer {
                    old.sky_light[k] = None;
                }
                col.sky_light = old.sky_light.clone();
            }
            // Empty sheets below: cell of the bottom row, the one they read.
            let i = if hollow { rc_world::block_index(x, 0, z) } else { i };
            let layers = if sky { &mut col.sky_light } else { &mut col.block_light };
            let Some(Some(l)) = layers.get_mut(layer) else { continue };
            let l = Arc::make_mut(l);
            l[i >> 1] ^= 0x5 << ((i & 1) * 4);
            let sky_data =
                |k: i32| usize::try_from(k).ok().and_then(|k| col.sky_light.get(k)).is_some_and(Option::is_some);
            hit = reach(edge::of_index(i), layer as i32 - 1, sections, true, sky_data);
            what = format!(
                "round {round}: {} light ({x}, {y}, {z}) of sheet {layer} of ({cx}, {cz}){}",
                if sky { "sky" } else { "block" },
                if hollow { ", empty sheets below" } else { "" }
            );
            if hollow {
                // The previous world has the same empty sheets; only the cell differs.
                let mut before = cols.clone();
                before.insert((cx, cz), Arc::new(old));
                let mut after = cols.clone();
                after.insert((cx, cz), Arc::new(col));
                partial += check(&before, &after, &t, &palette, (cx, cz), hit, &what);
                trials += 1;
                continue;
            }
        }
        let mut after = cols.clone();
        after.insert((cx, cz), Arc::new(col));
        partial += check(&cols, &after, &t, &palette, (cx, cz), hit, &what);
        trials += 1;
    }
    eprintln!("{trials} changes, {partial} partial meshes identical to the full ones");
    assert!(trials >= 160 && partial >= trials);
}
