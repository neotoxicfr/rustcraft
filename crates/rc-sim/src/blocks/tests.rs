//! Unit tests of the block engine's building blocks (no world).

use super::*;

#[test]
fn java_random_matches_reference() {
    // Reference values of the game's generator for seed 42.
    let mut r = JavaRandom::new(42);
    assert_eq!(r.next_int(), -1170105035);
    assert_eq!(r.next_int_bound(10), 3);
    let mut r = JavaRandom::new(0);
    assert_eq!(r.next_int_bound(100), 60);
    assert!((JavaRandom::new(42).next_double() - 0.7275636800328681).abs() < 1e-15);
}

#[test]
fn java_hash_set_order() {
    // Iteration order of a hashed set of the game for P and its 6 neighbours (§ 3.3):
    // h = (y + 31 z) 31 + x, slot = (h ^ h >>> 16) & 15, increasing slots.
    let p = [0, 0, 0];
    let mut m: JavaMap<()> = JavaMap::new();
    m.insert(p, ());
    for d in Dir::ALL {
        m.insert(rel(p, d), ());
    }
    let order: Vec<P> = m.iter().map(|e| e.0).collect();
    let mut expect: Vec<(u32, usize, P)> = Vec::new();
    let mut all = vec![p];
    all.extend(Dir::ALL.iter().map(|&d| rel(p, d)));
    for (i, q) in all.iter().enumerate() {
        let h = java_hash(*q) as u32;
        expect.push(((h ^ (h >> 16)) & 15, i, *q));
    }
    expect.sort();
    assert_eq!(order, expect.iter().map(|e| e.2).collect::<Vec<_>>());
    assert_eq!(m.len(), 7);
}

#[test]
fn java_map_grows_like_the_game() {
    let mut m: JavaMap<i32> = JavaMap::new();
    for i in 0..13 {
        m.insert([i, 0, 0], i);
    }
    // 13 entries > 12: table of 32 slots; x from 0 to 12 fall in slots 0 to 12.
    let order: Vec<i32> = m.iter().map(|e| e.1).collect();
    assert_eq!(order, (0..13).collect::<Vec<_>>());
    assert_eq!(m.remove([3, 0, 0]), Some(3));
    assert_eq!(m.len(), 12);
}

#[test]
fn directions() {
    for d in Dir::ALL {
        assert_eq!(d.opposite().opposite(), d);
        assert_eq!(Dir::parse(d.name()), Some(d));
    }
    assert_eq!(Dir::North.clockwise(), Dir::East);
    assert_eq!(Dir::West.counter_clockwise(), Dir::South);
}

/// Probe: replays `RUSTCRAFT_PROBE` on a copy of `RUSTCRAFT_SOLO_WORLD`, writes the readout
/// to `RUSTCRAFT_PROBE_OUT` (or the output).
#[test]
#[ignore]
fn probe_script() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let Some(script) = std::env::var_os("RUSTCRAFT_PROBE") else { return };
    let script = std::fs::read_to_string(script).expect("script");
    // `RUSTCRAFT_PROBE_KEEP`: work on this folder (already copied) and keep it (in-game view).
    let keep = std::env::var_os("RUSTCRAFT_PROBE_KEEP").map(std::path::PathBuf::from);
    let dir = keep.clone().unwrap_or_else(|| crate::tests::fresh_copy(&world, "probe"));
    let mut p = probe::Probe::open(&dir, std::sync::Arc::new(data));
    p.run(&script);
    match std::env::var_os("RUSTCRAFT_PROBE_OUT") {
        Some(out) => std::fs::write(out, &p.out).expect("write"),
        None => print!("{}", p.out),
    }
    drop(p);
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Probe on a copy of the test world (area 4000..4031 generated); `None` without a world.
fn probe(tag: &str) -> Option<(probe::Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let p = probe::Probe::open(&dir, std::sync::Arc::new(data));
    if p.s.get([4000, -61, 4000]) == p.s.get([4000, 400, 4000]) {
        // Area absent from the copy.
        return None;
    }
    Some((p, dir))
}

fn name_at(p: &probe::Probe, at: P) -> String {
    probe::state_string(&p.s, p.s.get(at))
}

const BASE: &str = "fill 4000 -60 4000 4031 -60 4031 minecraft:stone\nfill 4000 -59 4000 4031 -40 4031 minecraft:air\n";

#[test]
fn wire_line_loses_one_per_block() {
    let Some((mut p, dir)) = probe("wire") else { return };
    let mut s = String::from(BASE);
    s.push_str("setblock 4000 -59 4005 minecraft:redstone_block\n");
    s.push_str("fill 4001 -59 4005 4017 -59 4005 minecraft:redstone_wire\ntick 2\n");
    p.run(&s);
    for i in 1..=17 {
        let want = (16 - i).max(0);
        let got = p.s.v().get_int(p.s.get([4000 + i, -59, 4005]), "power");
        assert_eq!(got, want, "dust {i}");
    }
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn piston_pushes_twelve_not_thirteen() {
    let Some((mut p, dir)) = probe("piston12") else { return };
    let mut s = String::from(BASE);
    s.push_str("setblock 4002 -59 4010 minecraft:piston[facing=east]\n");
    s.push_str("fill 4003 -59 4010 4014 -59 4010 minecraft:stone\n");
    s.push_str("setblock 4002 -59 4012 minecraft:piston[facing=east]\n");
    s.push_str("fill 4003 -59 4012 4015 -59 4012 minecraft:stone\n");
    s.push_str("wait 2\nsetblock 4001 -59 4010 minecraft:redstone_block\nsetblock 4001 -59 4012 minecraft:redstone_block\ntick 4\n");
    p.run(&s);
    assert!(name_at(&p, [4003, -59, 4010]).starts_with("minecraft:piston_head"), "{}", name_at(&p, [4003, -59, 4010]));
    assert_eq!(name_at(&p, [4015, -59, 4010]), "minecraft:stone");
    assert_eq!(name_at(&p, [4002, -59, 4012]), "minecraft:piston[extended=false,facing=east]");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn water_flows_seven_blocks() {
    let Some((mut p, dir)) = probe("water") else { return };
    let mut s = String::from(BASE);
    s.push_str("setblock 4015 -59 4015 minecraft:water\ntick 60\n");
    p.run(&s);
    let lvl = |x: i32| p.s.fluid_at([4015 + x, -59, 4015]).amount;
    assert_eq!((lvl(0), lvl(1), lvl(7), lvl(8)), (8, 7, 1, 0));
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sand_falls_and_lands() {
    let Some((mut p, dir)) = probe("sand") else { return };
    let mut s = String::from(BASE);
    s.push_str("setblock 4005 -50 4005 minecraft:sand\ntick 40\n");
    p.run(&s);
    assert_eq!(name_at(&p, [4005, -59, 4005]), "minecraft:sand");
    assert_eq!(name_at(&p, [4005, -50, 4005]), "minecraft:air");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn placement_two_block_and_connections() {
    let Some((mut p, dir)) = probe("place") else { return };
    p.run(BASE);
    let s = &mut p.s;
    // Player north of the area, looking south.
    s.pos = [4005.5, -59.0, 4000.5];
    s.yaw = 0.0;
    s.pitch = 30.0;
    s.game_mode = 1;
    let top = |x: i32, z: i32| [f64::from(x) + 0.5, -59.0, f64::from(z) + 0.5];
    assert!(s.place_item("minecraft:oak_door", [4005, -60, 4004], Dir::Up, top(4005, 4004)));
    assert!(probe::state_string(s, s.get([4005, -59, 4004])).contains("half=lower"));
    assert!(probe::state_string(s, s.get([4005, -58, 4004])).contains("half=upper"));
    assert!(s.place_item("minecraft:red_bed", [4008, -60, 4004], Dir::Up, top(4008, 4004)));
    assert!(probe::state_string(s, s.get([4008, -59, 4005])).contains("part=head"));
    // Two fences side by side: connected.
    assert!(s.place_item("minecraft:oak_fence", [4010, -60, 4004], Dir::Up, top(4010, 4004)));
    assert!(s.place_item("minecraft:oak_fence", [4011, -60, 4004], Dir::Up, top(4011, 4004)));
    assert!(probe::state_string(s, s.get([4010, -59, 4004])).contains("east=true"));
    // Double slab: second slab on the first.
    assert!(s.place_item("minecraft:stone_slab", [4013, -60, 4004], Dir::Up, top(4013, 4004)));
    assert!(s.place_item("minecraft:stone_slab", [4013, -59, 4004], Dir::Up, [4013.5, -58.5, 4004.5]));
    assert!(probe::state_string(s, s.get([4013, -59, 4004])).contains("type=double"));
    // Wall torch against the east face of a block.
    assert!(s.place_item("minecraft:stone", [4015, -60, 4004], Dir::Up, top(4015, 4004)));
    assert!(s.place_item("minecraft:torch", [4015, -59, 4004], Dir::East, [4016.0, -58.5, 4004.5]));
    assert!(probe::state_string(s, s.get([4016, -59, 4004])).starts_with("minecraft:wall_torch[facing=east"));
    // Break the support block: the torch falls (shape cascade).
    s.player_destroy([4015, -59, 4004]);
    assert_eq!(probe::state_string(s, s.get([4016, -59, 4004])), "minecraft:air");
    // Break the upper half of the door: the lower one disappears.
    s.player_destroy([4005, -58, 4004]);
    assert_eq!(probe::state_string(s, s.get([4005, -59, 4004])), "minecraft:air");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}
