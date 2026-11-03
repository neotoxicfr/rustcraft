//! The shared block loot draw (`RUSTCRAFT_OPT_LOOTCTXBORROW`) against the three former draws it
//! replaces: same stacks, same state of the random sequences and of the level's random, for a
//! block mined by the world, mined by the player and blown up. Plus its allocation count.

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use super::BlockRemoval;
use crate::blocks::probe::Probe;
use crate::player::Stack;

/// Test table of the oak log: block tag and state conditions, a silk touch or fortune tool
/// test, a table bonus, an explosion pool, stacks over the maximum size and a copied state.
const OAK_LOG_TABLE: &str = r##"{"type":"minecraft:block","random_sequence":"minecraft:blocks/oak_log","pools":[
 {"rolls":{"type":"minecraft:uniform","min":1,"max":3},
  "condition":{"type":"minecraft:match_block","blocks":"#minecraft:logs","state":{"axis":"y"}},
  "entries":[
   {"type":"minecraft:item","name":"minecraft:oak_log","weight":2,
    "condition":{"type":"minecraft:match_tool","predicate":{"predicates":{"minecraft:enchantments":[
      {"enchantments":["minecraft:silk_touch","minecraft:fortune"],"levels":{"min":1}}]}}}},
   {"type":"minecraft:item","name":"minecraft:stick","weight":3,
    "modifier":[{"type":"minecraft:set_count","count":{"type":"minecraft:uniform","min":40,"max":100}},
                {"type":"minecraft:copy_state","block":"minecraft:oak_log","properties":["axis"]}]}]},
 {"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:apple",
   "condition":{"type":"minecraft:table_bonus","enchantment":"minecraft:fortune","chances":[0.3,0.5,0.7,0.9]}}]},
 {"rolls":2,"modifier":{"type":"minecraft:explosion_decay"},
  "entries":[{"type":"minecraft:item","name":"minecraft:diamond","condition":{"type":"minecraft:survives_explosion"},
   "modifier":{"type":"minecraft:set_count","count":{"type":"minecraft:uniform","min":2,"max":9}}}]}]}"##;

const BARREL: [i32; 3] = [8, -59, 8];
const LOG: [i32; 3] = [10, -59, 8];

fn enchanted(id: &str, enchantment: &str, level: i32) -> Stack {
    let enchantments = Nbt::Compound(vec![(enchantment.into(), Nbt::Int(level))]);
    let components = Nbt::Compound(vec![("minecraft:enchantments".into(), enchantments)]);
    Stack { id: id.into(), count: 1, components: Some(components) }
}

fn open(name: &str) -> (Probe, std::path::PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-blockloot-{name}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("blockloot");
    w.preset = crate::newworld::Preset::Flat;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.life.pack.insert_test_file("loot_table/blocks/oak_log", OAK_LOG_TABLE);
    p.run(&format!("setblock {} {} {} minecraft:barrel[facing=up]\n", BARREL[0], BARREL[1], BARREL[2]));
    let be = p.s.block_entity_mut(BARREL, "minecraft:barrel").expect("barrel block entity");
    crate::nbtio::set(be, "CustomName", Nbt::String("Loot box".into()));
    (p, dir)
}

/// Which code draws: the shared draw or the former one of the same removal.
#[derive(Clone, Copy, PartialEq)]
enum Draw {
    Shared,
    Former,
}

/// Removal `kind` (0 mined by the world, 1 by the player, 2 blown up) of `state` at `at`.
fn remove(p: &mut Probe, draw: Draw, kind: usize, at: [i32; 3], state: u32, tool: Option<&Stack>) -> Vec<Stack> {
    let s = &mut p.s;
    match (kind, draw) {
        (0, Draw::Shared) => s.draw_block_loot(at, state, BlockRemoval::Mined { tool, by_player: false }),
        (0, Draw::Former) => s.block_drops_with_owned_context(at, state, tool),
        (1, Draw::Shared) => s.draw_block_loot(at, state, BlockRemoval::Mined { tool, by_player: true }),
        (1, Draw::Former) => {
            let name = s.data.block_of(state).map(|b| b.name.clone()).unwrap_or_default();
            s.mined_block_loot_with_owned_context(at, state, &name, tool, true)
        }
        (_, Draw::Shared) => s.draw_block_loot(at, state, BlockRemoval::Blown { radius: Some(4.0) }),
        (_, Draw::Former) => s.explosion_drops_with_owned_context(at, state, Some(4.0)),
    }
}

/// Every removal of the test, in order: the stacks written out, then the saved sequences and
/// the level's random after the last one.
fn run_all(p: &mut Probe, draw: Draw) -> (String, Option<Nbt>, (u64, u64)) {
    let tools = [
        None,
        Some(enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3)),
        Some(enchanted("minecraft:diamond_axe", "minecraft:silk_touch", 1)),
    ];
    let oak = p.s.data.block_named("minecraft:oak_log").expect("oak log").first_state;
    let barrel = p.s.st(BARREL);
    let stone = p.s.data.default_state_of("minecraft:stone").expect("stone");
    let mut out = String::new();
    for i in 0..48usize {
        let (at, state) = match i % 4 {
            0 => (BARREL, barrel),
            1 => (LOG, stone),
            _ => (LOG, oak + (i as u32 / 4) % 3),
        };
        let tool = tools[i % 3].as_ref();
        for st in remove(p, draw, (i / 3) % 3, at, state, tool) {
            let c = st.components.as_ref().map_or_else(String::new, crate::commands::snbt::to_snbt);
            out.push_str(&format!("{i}: {} {} {c}\n", st.id, st.count));
        }
    }
    let mut seqs = p.s.life.seqs.clone();
    (out, seqs.to_nbt(), p.s.life.rng.parts())
}

#[test]
fn shared_draw_matches_the_former_draws() {
    let (mut p, _dir) = open("same");
    let start = (p.s.life.seqs.clone(), p.s.life.rng);
    let former = run_all(&mut p, Draw::Former);
    (p.s.life.seqs, p.s.life.rng) = start;
    let shared = run_all(&mut p, Draw::Shared);
    // The test is only worth something if every case drew something.
    for needle in ["minecraft:stick 64", "block_state", "Loot box", "minecraft:apple", "minecraft:diamond", "minecraft:cobblestone"] {
        assert!(former.0.contains(needle), "{needle} never drawn:\n{}", former.0);
    }
    assert!(former.1.is_some(), "no sequence used");
    assert_eq!(former.0, shared.0, "stacks differ");
    assert_eq!(former.1, shared.1, "saved sequences differ");
    assert_eq!(former.2, shared.2, "level random differs");
}

/// Allocations of one removal of `state`, averaged over `n` after a first one (tables and
/// sequences then known).
fn allocations_per_removal(p: &mut Probe, draw: Draw, kind: usize, at: [i32; 3], state: u32, n: u64) -> f64 {
    let tool = enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3);
    drop(remove(p, draw, kind, at, state, Some(&tool)));
    let before = crate::test_alloc::allocations();
    for _ in 0..n {
        drop(remove(p, draw, kind, at, state, Some(&tool)));
    }
    (crate::test_alloc::allocations() - before) as f64 / n as f64
}

/// Mining a stone block allocates the list of stacks and its one stack's name, nothing else
/// (blown up, only when the cobblestone survives); the former draws also wrote the table's
/// name and copied the sequence's name, twice for a mined block.
#[test]
fn shared_draw_allocates_less() {
    let (mut p, _dir) = open("allocs");
    let stone = p.s.data.default_state_of("minecraft:stone").expect("stone");
    for kind in 0..3 {
        let former = allocations_per_removal(&mut p, Draw::Former, kind, LOG, stone, 100);
        let shared = allocations_per_removal(&mut p, Draw::Shared, kind, LOG, stone, 100);
        eprintln!("removal {kind}: {former:.1} allocations before, {shared:.1} after");
        // The exact count needs the whole new path (the engine and the sequences' lookup).
        let switches_on = crate::loot::loot_context_borrowed() && crate::rng::sequence_looked_up_before_insert();
        if kind < 2 && switches_on {
            assert_eq!(shared, 2.0, "mined stone (removal {kind}): only the list and the stack");
        }
        assert!(shared + 2.5 <= former, "removal {kind}: {shared} allocations, the former draw {former}");
    }
}

/// Micro-bench (loaded machine): `cargo test --release -p rc-sim block_loot_bench -- --ignored
/// --nocapture`. One thread: the draw runs on the server thread.
#[test]
#[ignore]
fn block_loot_bench() {
    let (mut p, _dir) = open("bench");
    let oak = p.s.data.block_named("minecraft:oak_leaves").expect("leaves").default_state;
    let stone = p.s.data.default_state_of("minecraft:stone").expect("stone");
    let barrel = p.s.st(BARREL);
    let tool = enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3);
    for (label, at, state) in [("stone", LOG, stone), ("oak leaves", LOG, oak), ("barrel", BARREL, barrel)] {
        for (kind, removal) in ["mined by the world", "mined by the player", "blown up"].iter().enumerate() {
            let mut best = [f64::MAX; 2];
            let mut allocs = [0.0; 2];
            for (d, draw) in [Draw::Former, Draw::Shared].into_iter().enumerate() {
                allocs[d] = allocations_per_removal(&mut p, draw, kind, at, state, 1_000);
            }
            // Best of 7 trials of 5 000 removals, the two draws alternated (the minimum resists the load).
            for _ in 0..7 {
                for (d, draw) in [Draw::Former, Draw::Shared].into_iter().enumerate() {
                    let t = std::time::Instant::now();
                    for _ in 0..5_000 {
                        drop(remove(&mut p, draw, kind, at, state, Some(&tool)));
                    }
                    best[d] = best[d].min(t.elapsed().as_nanos() as f64 / 5_000.0);
                }
            }
            eprintln!(
                "{label:10} {removal:20}: before {:6.0} ns {:5.1} allocations, after {:6.0} ns {:5.1} allocations",
                best[0], allocs[0], best[1], allocs[1]
            );
        }
    }
}
