//! Proof that the loot engine reads a block the same way after the borrowed context
//! (`RUSTCRAFT_OPT_LOOTCTXBORROW`): every block table of the base pack is drawn for several states
//! of its block and several tools, with the server's block tags, and the stacks are fingerprinted.
//! The expected value was recorded on the code before the change (properties copied into the
//! context as owned strings).

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::loot::{Ctx, Loot};
use crate::player::Stack;

/// Fingerprint of every block table, recorded before the borrowed context.
const BLOCK_TABLES_FINGERPRINT: u64 = 0x651f_c71b_4e03_9258;

/// Up to `count` states of the block, spread over its whole range, its default state first.
fn sample_states(block: &rc_data::BlockInfo, count: u32) -> Vec<u32> {
    let mut states = vec![block.default_state];
    let step = (block.state_count / count).max(1);
    let mut s = block.first_state;
    while s < block.first_state + block.state_count && states.len() < count as usize + 1 {
        if s != block.default_state {
            states.push(s);
        }
        s += step;
    }
    states
}

fn enchanted(id: &str, enchantment: &str, level: i32) -> Stack {
    let enchantments = Nbt::Compound(vec![(enchantment.into(), Nbt::Int(level))]);
    let components = Nbt::Compound(vec![("minecraft:enchantments".into(), enchantments)]);
    Stack { id: id.into(), count: 1, components: Some(components) }
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3))
}

#[test]
fn block_tables_read_the_block_like_before() {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-lootblocks-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("lootblocks");
    w.preset = crate::newworld::Preset::Flat;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let p = crate::blocks::probe::Probe::open(&dir, data.clone());
    let s = &p.s;
    let tag = |t: &str, n: &str| s.block_name_tagged(t, n);
    let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &tag };
    let tools = [
        None,
        Some(enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3)),
        Some(enchanted("minecraft:diamond_pickaxe", "minecraft:silk_touch", 1)),
        Some(Stack { id: "minecraft:shears".into(), count: 1, components: None }),
    ];
    let mut total = 0xcbf2_9ce4_8422_2325u64;
    let mut tables = 0;
    for block in &data.blocks {
        let id = format!("minecraft:blocks/{}", block.name.trim_start_matches("minecraft:"));
        if s.life.pack.get("loot_table", &id).is_none() {
            continue;
        }
        tables += 1;
        for state in sample_states(block, 6) {
            for (t, tool) in tools.iter().enumerate() {
                let props: Vec<(String, String)> =
                    data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
                let ctx = Ctx { block: Some((block.name.as_str(), props)), tool: Some(tool.as_ref()), ..Default::default() };
                let mut rng = crate::mobs::jrand::JRand::new(i64::from(state) * 4 + t as i64);
                for st in loot.drops(&id, &ctx, &mut rng) {
                    let c = st.components.as_ref().map_or_else(String::new, crate::commands::snbt::to_snbt);
                    total = fnv(total, format!("{state} {t} {} {} {c};", st.id, st.count).as_bytes());
                }
            }
        }
    }
    assert!(tables > 1000, "base pack block tables missing: {tables}");
    eprintln!("block tables fingerprint: {tables} tables, {total:016x}");
    assert_eq!(total, BLOCK_TABLES_FINGERPRINT, "the loot of a block table changed");
}
