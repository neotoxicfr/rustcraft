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

fn fingerprint_and_bench(timed: bool) -> u64 {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-lootbench-{timed}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("lootbench");
    w.preset = crate::newworld::Preset::Flat;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let p = crate::blocks::probe::Probe::open(&dir, data);
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
    let fnv = |h: u64, b: &[u8]| b.iter().fold(h, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3));
    let mut lines = String::new();
    let mut total = 0xcbf2_9ce4_8422_2325u64;
    for id in &ids {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
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
