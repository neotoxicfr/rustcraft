//! Equivalence proof of `RUSTCRAFT_OPT_ENVIRONCACHE`: a camera trajectory (entering and
//! leaving water, lava, powder snow, biome and column changes, blocks placed and
//! removed under the camera, column replaced then removed, seed changed) replayed with and without
//! cache; `FrameEnv` identical field by field on every frame.

use std::sync::Arc;

use rc_world::{Column, Container, Section};

use super::*;

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

struct States {
    stone: u32,
    water: u32,
    lava: u32,
    snow: u32,
    direct_bits: u8,
}

fn states(d: &rc_data::VersionData) -> States {
    let def = |n: &str| d.blocks.iter().find(|b| b.name == n).map(|b| b.default_state).expect(n);
    States {
        stone: def("minecraft:stone"),
        water: def("minecraft:water"),
        lava: def("minecraft:lava"),
        snow: def("minecraft:powder_snow"),
        direct_bits: (usize::BITS - d.states.len().leading_zeros()) as u8,
    }
}

/// Test terrain: stone floor up to y = 62, water pool, lava pool, pile of powder snow,
/// and four biomes in a checkerboard of quarts.
fn block(s: &States, x: i32, y: i32, z: i32) -> u32 {
    if y <= 62 {
        return s.stone;
    }
    if (0..6).contains(&x) && (0..6).contains(&z) && y <= 64 {
        return s.water;
    }
    if (14..19).contains(&x) && (14..19).contains(&z) && y == 63 {
        return s.lava;
    }
    if (-8..-4).contains(&x) && (-2..2).contains(&z) && y <= 65 {
        return s.snow;
    }
    0
}

fn column(s: &States, cx: i32, cz: i32, shift: u32) -> Column {
    let sections: Vec<Section> = (0..HEIGHT / 16)
        .map(|si| {
            let mut v = vec![0u32; 4096];
            for ly in 0..16 {
                for lz in 0..16 {
                    for lx in 0..16 {
                        let (x, y, z) = (cx * 16 + lx, MIN_Y + si * 16 + ly, cz * 16 + lz);
                        v[rc_world::block_index(lx as usize, ly as usize, lz as usize)] = block(s, x, y, z);
                    }
                }
            }
            let mut b = vec![0u32; 64];
            for (i, e) in b.iter_mut().enumerate() {
                let (qx, qz) = (cx * 4 + (i & 3) as i32, cz * 4 + ((i >> 2) & 3) as i32);
                *e = ((qx + 2 * qz).rem_euclid(4) as u32 + shift) % 4;
            }
            Section {
                non_empty: v.iter().filter(|&&x| x != 0).count() as i16,
                blocks: Container::pack(&v, 4, 8, s.direct_bits),
                biomes: Container::pack(&b, 1, 3, 6),
            }
        })
        .collect();
    let n = sections.len() + 2;
    Column {
        x: cx,
        z: cz,
        sections: sections.into_boxed_slice(),
        sky_light: vec![None; n].into_boxed_slice(),
        block_light: vec![Some(Arc::new([0x77; rc_world::LIGHT_BYTES])); n].into_boxed_slice(),
        heightmaps: Vec::new(),
    }
}

/// Places `state` at block (x, y, z) of the grid.
fn set(grid: &mut ChunkGrid, s: &States, x: i32, y: i32, z: i32, state: u32) {
    let c = grid.get_mut(x >> 4, z >> 4).expect("column loaded");
    let ly = y - MIN_Y;
    let i = rc_world::block_index((x & 15) as usize, (ly & 15) as usize, (z & 15) as usize) as u16;
    c.sections[(ly >> 4) as usize].set_blocks(&[(i, state)], s.direct_bits, |v| v == 0);
}

#[test]
fn frame_env_identical_with_and_without_cache() {
    let Some(d) = rc_testdata::version_data() else { return };
    let t = rc_physics::Tables::new(&d);
    let s = states(&d);
    let mut grid = ChunkGrid::new(4);
    for cx in -2..=2 {
        for cz in -2..=2 {
            grid.insert(column(&s, cx, cz, 0));
        }
    }
    let biomes: Vec<String> =
        ["plains", "desert", "snowy_plains", "swamp"].iter().map(|b| format!("minecraft:{b}")).collect();
    let (mut off, mut on) = (EnvState::default(), EnvState::default());
    for (e, cache) in [(&mut off, false), (&mut on, true)] {
        e.join(&d, "minecraft:overworld", &biomes, false);
        e.cache.on = cache;
        e.env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        // Rain: the biome at the eye's block (precipitation) matters for the fog.
        e.env.weather.event(7, 1.0);
    }
    set_biome_zoom_seed(0x5eed_1234_abcd);
    // Trajectory: (start, end, step); eyes at varying height.
    let legs: [([f64; 3], [f64; 3]); 6] = [
        ([-12.3, 66.6, 0.4], [3.2, 63.9, 2.7]),  // powder snow then dive into the pool
        ([3.2, 63.9, 2.7], [3.2, 66.2, 2.7]),    // exit from the water through the surface
        ([3.2, 66.2, 2.7], [16.5, 63.3, 16.5]),  // toward the lava, through several columns
        ([16.5, 63.3, 16.5], [16.5, 64.4, 16.5]), // exit from the lava
        ([16.5, 64.4, 16.5], [-30.0, 70.0, -28.0]), // crossing of the biome checkerboard
        ([-30.0, 70.0, -28.0], [-40.0, 70.0, -40.0]), // outside the loaded columns
    ];
    let mut frame_no = 0u32;
    let mut run = |grid: &ChunkGrid, eye: [f64; 3], frame_no: &mut u32| {
        *frame_no += 1;
        let k = *frame_no;
        let w = WorldView { grid, min_y: MIN_Y, height: HEIGHT };
        let yaw = (k as f32 * 37.0) % 360.0 - 180.0;
        let pitch = (k as f32 * 13.0) % 180.0 - 90.0;
        let aspect = [16.0 / 9.0, 0.5, 3.5, 40.0][(k % 4) as usize];
        let pt = (k % 7) as f32 / 7.0;
        if k.is_multiple_of(3) {
            off.tick(&w, false);
            on.tick(&w, false);
        }
        let a = off.frame(&w, &t, &d, eye, yaw, pitch, aspect, pt, 0.016, 12, false);
        let b = on.frame(&w, &t, &d, eye, yaw, pitch, aspect, pt, 0.016, 12, false);
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "frame {k}, eye {eye:?}, yaw {yaw}, aspect {aspect}");
        a.fluid
    };
    let mut kinds = [0u32; 2];
    for (from, to) in legs {
        for i in 0..=400 {
            let f = f64::from(i) / 400.0;
            let eye: [f64; 3] = std::array::from_fn(|a| from[a] + (to[a] - from[a]) * f);
            kinds[usize::from(run(&grid, eye, &mut frame_no))] += 1;
        }
    }
    assert!(kinds[1] > 20 && kinds[0] > 20, "trajectory without fluid or without air: {kinds:?}");
    // In place: water, lava, snow placed then removed around the eye, frame after each placement.
    let eye = [-20.5, 70.6, 9.5];
    for state in [s.water, s.lava, s.snow] {
        for (dx, dy, dz) in [(0, 0, 0), (1, 0, 0), (0, 0, -1), (-1, 1, 1)] {
            let b = [-21 + dx, 70 + dy, 9 + dz];
            for _ in 0..3 {
                run(&grid, eye, &mut frame_no);
            }
            set(&mut grid, &s, b[0], b[1], b[2], state);
            for _ in 0..3 {
                run(&grid, eye, &mut frame_no);
            }
            set(&mut grid, &s, b[0], b[1], b[2], 0);
            run(&grid, eye, &mut frame_no);
        }
    }
    // Column replaced (other biomes), then removed, then seed changed.
    let eye = [-24.2, 70.6, 9.5];
    run(&grid, eye, &mut frame_no);
    grid.insert(column(&s, -2, 0, 1));
    run(&grid, eye, &mut frame_no);
    grid.remove(-2, 0);
    run(&grid, eye, &mut frame_no);
    grid.insert(column(&s, -2, 0, 2));
    run(&grid, eye, &mut frame_no);
    // Seed changed under a motionless eye, block after block (the desert, without precipitation,
    // changes the rain fog, which persists from one frame to the next).
    for i in 0..64 {
        let eye = [-30.5 + f64::from(i) * 0.75, 70.6, -20.5 + f64::from(i) * 0.5];
        set_biome_zoom_seed(0x5eed_1234_abcd);
        run(&grid, eye, &mut frame_no);
        set_biome_zoom_seed(-77 - i64::from(i));
        for _ in 0..3 {
            run(&grid, eye, &mut frame_no);
        }
    }
}
