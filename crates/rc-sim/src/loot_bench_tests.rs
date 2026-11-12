//! G6-F17 bench of the loot engine on the base pack: fingerprint of every table (same seeds,
//! same context: fortune III pickaxe, luck 1, looting II, killed by a player) and time per draw
//! of common tables. Written to compile unchanged before and after the series, to compare the
//! two. `cargo test --release -p rc-sim loot_bench -- --ignored --nocapture`; one line per
//! table to the file `RUSTCRAFT_LOOT_PRINT` when set.

use std::sync::Arc;
use std::time::Instant;

use rc_proto::nbt::Nbt;

use crate::loot::{Ctx, Loot};
use crate::player::Stack;

/// Fingerprint of every table of the base pack, recorded on the code before the series
/// (G6-F17) and unchanged since.
const EVERY_TABLE_FINGERPRINT: u64 = 0xaf88_0b1b_aa83_fb25;

/// The fingerprint of the bench, checked at every run of the tests (the bench's timing stays
/// ignored).
#[test]
fn every_table_draws_like_before() {
    assert_eq!(fingerprint_and_bench(false), EVERY_TABLE_FINGERPRINT, "the loot of a base pack table changed");
}

#[test]
#[ignore]
fn loot_bench() {
    assert_eq!(fingerprint_and_bench(true), EVERY_TABLE_FINGERPRINT, "the loot of a base pack table changed");
}

/// Fingerprint of the creeper's table killed by a skeleton, 64 deaths, recorded on the code
/// before the tag entries lent their names (each item of the tag was copied into a list). Its
/// pool of music discs is an expanded tag entry: the disc a given random gives depends on the
/// order of the tag's members, which the fingerprint of every table never reaches (no attacker).
const CREEPER_KILLED_BY_SKELETON_FINGERPRINT: u64 = 0xab04_7108_cfc0_8943;

#[test]
fn creeper_killed_by_skeleton_draws_like_before() {
    let p = flat_world("creeper");
    let s = &p.s;
    let no_tag = |_: &str, _: &str| false;
    let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
    // The creeper's table only asks whether the attacker is in the `skeletons` entity tag.
    let skeleton = ("minecraft:skeleton".to_owned(), vec!["minecraft:skeletons".to_owned()]);
    let ctx = Ctx { attacker: Some(skeleton), ..Default::default() };
    let mut h = FNV_START;
    let mut discs = hashbrown::HashSet::new();
    // 16 deaths in a row per generator: the first draws of generators with neighbouring seeds
    // are close to each other and would only reach a few discs.
    for seed in 0..4 {
        let mut rng = crate::mobs::jrand::JRand::new(seed);
        for _ in 0..16 {
            for st in loot.drops("minecraft:entities/creeper", &ctx, &mut rng) {
                if st.id.contains("music_disc") {
                    discs.insert(st.id.clone());
                }
                h = fnv(h, format!("{} {};", st.id, st.count).as_bytes());
            }
        }
    }
    eprintln!("creeper killed by a skeleton: {} discs, {h:016x}", discs.len());
    // The fingerprint is only worth something if the draw walks most of the tag.
    assert!(discs.len() >= 8, "too few discs drawn: {discs:?}");
    assert_eq!(h, CREEPER_KILLED_BY_SKELETON_FINGERPRINT, "the creeper's disc changed");
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3))
}

/// A new flat world on the base pack (its folder removed when the test ends).
fn flat_world(name: &str) -> crate::blocks::probe::Probe {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-lootbench-{name}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("lootbench");
    w.preset = crate::newworld::Preset::Flat;
    // Fixed seed, no structures: a random seed let a village land under the test now and then.
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    crate::blocks::probe::Probe::open(&dir, data)
}

fn fingerprint_and_bench(timed: bool) -> u64 {
    let p = flat_world(if timed { "timed" } else { "fingerprint" });
    let s = &p.s;
    let no_tag = |_: &str, _: &str| false;
    let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
    let fortune = Nbt::Compound(vec![(
        "minecraft:enchantments".into(),
        Nbt::Compound(vec![("minecraft:fortune".into(), Nbt::Int(3))]),
    )]);
    let pick = Stack { id: "minecraft:diamond_pickaxe".into(), count: 1, components: Some(fortune) };
    let ctx = Ctx { tool: Some(Some(&pick)), luck: 1.0, killed_by_player: true, looting: 2, ..Default::default() };
    let ids: Vec<String> = s.life.pack.all("loot_table").into_iter().map(|(k, _)| k).collect();
    assert!(ids.len() > 1000, "base pack tables missing: {}", ids.len());
    let mut lines = String::new();
    let mut total = FNV_START;
    for id in &ids {
        let mut h = FNV_START;
        for seed in 0..16 {
            let mut rng = crate::mobs::jrand::JRand::new(seed);
            for st in loot.drops(id, &ctx, &mut rng) {
                let c = st.components.as_ref().map_or_else(String::new, crate::commands::snbt::to_snbt);
                h = fnv(h, format!("{} {} {c};", st.id, st.count).as_bytes());
            }
        }
        lines.push_str(&format!("{id} {h:016x}\n"));
        total = fnv(total, &h.to_le_bytes());
    }
    if let Some(path) = rc_data::env_os!("RUSTCRAFT_LOOT_PRINT") {
        std::fs::write(path, &lines).expect("fingerprint file");
    }
    eprintln!("loot fingerprint: {} tables, {total:016x}", ids.len());
    if !timed {
        return total;
    }
    for id in ["minecraft:blocks/stone", "minecraft:blocks/oak_leaves", "minecraft:entities/zombie", "minecraft:chests/simple_dungeon", "minecraft:gameplay/fishing"] {
        // Best of 7 trials of 5 000 draws (the minimum resists the load of the machine).
        let mut rng = crate::mobs::jrand::JRand::new(1);
        let n = 5_000;
        let mut made = 0usize;
        for _ in 0..1_000 {
            made += loot.drops(id, &ctx, &mut rng).len();
        }
        let mut best = f64::MAX;
        for _ in 0..7 {
            let t = Instant::now();
            for _ in 0..n {
                made += loot.drops(id, &ctx, &mut rng).len();
            }
            best = best.min(t.elapsed().as_nanos() as f64 / f64::from(n));
        }
        eprintln!("loot draw {id}: {best:.0} ns ({made} stacks)");
    }
    total
}
