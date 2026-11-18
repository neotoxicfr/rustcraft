//! Proof that the loot engine reads a block the same way after the borrowed context
//! (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT`): every block table of the base pack is drawn for several states
//! of its block and several tools, with the server's block tags, and the stacks are fingerprinted.
//! The expected value was recorded on the code before the change (properties copied into the
//! context as owned strings).

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::loot::{BlockProps, Ctx, Loot};
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

/// How the test hands the block's properties to the context.
#[derive(Clone, Copy, Debug)]
enum PropsForm {
    /// Copied as owned strings, as before the borrowed context.
    Listed,
    /// Lent by state.
    State,
}

#[test]
fn block_tables_read_the_block_like_before() {
    for form in [PropsForm::Listed, PropsForm::State] {
        assert_eq!(block_tables_fingerprint(form), BLOCK_TABLES_FINGERPRINT, "{form:?}: the loot of a block table changed");
    }
}

fn block_tables_fingerprint(form: PropsForm) -> u64 {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-lootblocks-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("lootblocks");
    w.preset = crate::newworld::Preset::Flat;
    // Fixed seed, no structures: a random seed let a village land under the test now and then.
    w.seed = 0;
    w.generate_structures = false;
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
                let props = match form {
                    PropsForm::Listed => BlockProps::Listed(
                        data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
                    ),
                    PropsForm::State => BlockProps::State(state),
                };
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
    eprintln!("block tables fingerprint ({form:?}): {tables} tables, {total:016x}");
    total
}

/// The table bonus chance read in place equals the one read through a copied list, for lists
/// with skipped values, empty lists and levels past the end (negative levels included).
#[test]
fn table_bonus_chance_in_place_matches_the_listed_one() {
    let lists = [
        serde_json::json!([0.05, 0.0625, 0.083_333_336, 0.1]),
        serde_json::json!([]),
        serde_json::json!(["x", 0.5, null, 0.75]),
        serde_json::json!(0.5),
        serde_json::Value::Null,
    ];
    for list in &lists {
        for lvl in [-3, 0, 1, 2, 3, 4, 9, i32::MAX] {
            assert_eq!(
                super::table_bonus_chance(list, lvl),
                super::table_bonus_chance_listed(list, lvl),
                "{list} at level {lvl}"
            );
        }
    }
}

/// Stacks split as the table makes them equal those split after the run, on every base pack
/// table with the same generators.
#[test]
fn split_while_running_matches_split_after() {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-lootsplit-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("lootsplit");
    w.preset = crate::newworld::Preset::Flat;
    // Fixed seed, no structures: a random seed let a village land under the test now and then.
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let p = crate::blocks::probe::Probe::open(&dir, data);
    let s = &p.s;
    let no_tag = |_: &str, _: &str| false;
    let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
    let pick = enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3);
    let ctx = Ctx { tool: Some(Some(&pick)), luck: 1.0, killed_by_player: true, looting: 2, ..Default::default() };
    let tables = s.life.pack.all("loot_table");
    assert!(tables.len() > 1000, "base pack tables missing: {}", tables.len());
    let mut stacks = 0;
    for (id, table) in &tables {
        for seed in 0..4 {
            let after = loot.split_after_run(table, &ctx, &mut crate::mobs::jrand::JRand::new(seed));
            let during = loot.drops_of_table(table, &ctx, &mut crate::mobs::jrand::JRand::new(seed));
            let text = |v: &[Stack]| format!("{v:?}");
            assert_eq!(text(&during), text(&after), "{id} seed {seed}");
            stacks += after.len();
        }
    }
    // Stacks over their maximum size: `block_loot::tests` (sticks of 40 to 100).
    assert!(stacks > 1000, "the tables made almost nothing: {stacks} stacks");
}

/// The inline levels of the running tables behave like the former list, past the inline ones.
#[test]
fn running_tables_behave_like_a_list() {
    let mut tables = super::RunningTables::default();
    let mut model: Vec<usize> = Vec::new();
    for step in 0..40usize {
        let id = (step * 7919) % 13;
        if step % 5 == 4 {
            tables.pop();
            model.pop();
        } else {
            tables.push(id);
            model.push(id);
        }
        for table in 0..13 {
            assert_eq!(tables.contains(table), model.contains(&table), "step {step}, table {table}");
        }
    }
}
