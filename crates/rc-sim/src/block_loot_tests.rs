//! The shared block loot draw (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT`) against the three former draws it
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
    // Fixed seed, no structures: a random seed let a village land under the test now and then.
    w.seed = 0;
    w.generate_structures = false;
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

/// How the block leaves the world.
#[derive(Clone, Copy, Debug)]
enum Removal {
    MinedByWorld,
    MinedByPlayer,
    Blown,
}

impl Removal {
    const ALL: [Removal; 3] = [Removal::MinedByWorld, Removal::MinedByPlayer, Removal::Blown];

    fn label(self) -> &'static str {
        match self {
            Removal::MinedByWorld => "mined by the world",
            Removal::MinedByPlayer => "mined by the player",
            Removal::Blown => "blown up",
        }
    }
}

/// `state` at `at` removed by `removal`, drawn by the shared or the former code.
fn remove(p: &mut Probe, draw: Draw, removal: Removal, at: [i32; 3], state: u32, tool: Option<&Stack>) -> Vec<Stack> {
    let s = &mut p.s;
    match (removal, draw) {
        (Removal::MinedByWorld, Draw::Shared) => s.draw_block_loot(at, state, BlockRemoval::Mined { tool, by_player: false }),
        (Removal::MinedByWorld, Draw::Former) => s.block_drops_with_owned_context(at, state, tool),
        (Removal::MinedByPlayer, Draw::Shared) => s.draw_block_loot(at, state, BlockRemoval::Mined { tool, by_player: true }),
        (Removal::MinedByPlayer, Draw::Former) => {
            let name = s.data.block_of(state).map(|b| b.name.clone()).unwrap_or_default();
            s.mined_block_loot_with_owned_context(at, state, &name, tool, true)
        }
        (Removal::Blown, Draw::Shared) => s.draw_block_loot(at, state, BlockRemoval::Blown { radius: Some(4.0) }),
        (Removal::Blown, Draw::Former) => s.explosion_drops_with_owned_context(at, state, Some(4.0)),
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
        // Each removal in turn, three draws in a row (the same order as before the enum).
        for st in remove(p, draw, Removal::ALL[(i / 3) % 3], at, state, tool) {
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

/// A wall variant draws its standing block's table (the jar has none under its own name), and
/// a block the game gives no table drops nothing even when the pack holds a file under its name:
/// for every removal, by the shared draw and by the former ones.
#[test]
fn wall_variants_drop_their_standing_block() {
    let (mut p, _dir) = open("wall");
    p.s.life.pack.insert_test_file("loot_table/blocks/bedrock", r#"{"pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]}]}"#);
    let wall_cases = [
        ("minecraft:wall_torch", "minecraft:torch"),
        ("minecraft:oak_wall_sign", "minecraft:oak_sign"),
        ("minecraft:white_wall_banner", "minecraft:white_banner"),
        ("minecraft:skeleton_wall_skull", "minecraft:skeleton_skull"),
        ("minecraft:oak_wall_hanging_sign", "minecraft:oak_hanging_sign"),
        ("minecraft:redstone_wall_torch", "minecraft:redstone_torch"),
    ];
    for (wall, item) in wall_cases {
        let state = p.s.data.default_state_of(wall).expect("wall block");
        let table = format!("minecraft:blocks/{}", item.trim_start_matches("minecraft:"));
        assert_eq!(p.s.block_loot_table_key(state), Some(table.as_str()), "{wall}");
        for (removal, drops) in every_removal_without_decay(&mut p, state) {
            let ids: Vec<&str> = drops.iter().map(|s| s.id.as_str()).collect();
            assert_eq!(ids, [item], "{wall} {removal}");
        }
    }
    // In 26.3: the wall torches, signs, hanging signs, banners, heads and coral fans name their
    // standing block's table; air, fluids, bedrock, portals, command and structure blocks… none.
    let own_table = |b: &rc_data::BlockInfo| format!("minecraft:blocks/{}", b.name.trim_start_matches("minecraft:"));
    let blocks = &p.s.data.blocks;
    let redirected = blocks.iter().filter(|b| b.loot_table.as_ref().is_some_and(|t| *t != own_table(b))).count();
    let without = blocks.iter().filter(|b| b.loot_table.is_none()).count();
    assert_eq!((redirected, without), (63, 22), "blocks naming another table, blocks without one");
    let bedrock = p.s.data.default_state_of("minecraft:bedrock").expect("bedrock");
    assert_eq!(p.s.block_loot_table_key(bedrock), None, "bedrock has no table");
    for (removal, drops) in every_removal_without_decay(&mut p, bedrock) {
        assert!(drops.is_empty(), "bedrock {removal}: {drops:?}");
    }
}

/// `state` removed at `LOG` by each draw, shared then former, with no tool and an explosion whose
/// loot does not decay (a survival condition then always passes).
fn every_removal_without_decay(p: &mut Probe, state: u32) -> Vec<(&'static str, Vec<Stack>)> {
    let name = p.s.data.block_of(state).map(|b| b.name.clone()).unwrap_or_default();
    let s = &mut p.s;
    vec![
        ("shared, mined by the world", s.draw_block_loot(LOG, state, BlockRemoval::Mined { tool: None, by_player: false })),
        ("shared, mined by the player", s.draw_block_loot(LOG, state, BlockRemoval::Mined { tool: None, by_player: true })),
        ("shared, blown up", s.draw_block_loot(LOG, state, BlockRemoval::Blown { radius: None })),
        ("former, mined by the world", s.block_drops_with_owned_context(LOG, state, None)),
        ("former, mined by the player", s.mined_block_loot_with_owned_context(LOG, state, &name, None, true)),
        ("former, blown up", s.explosion_drops_with_owned_context(LOG, state, None)),
    ]
}

/// `/loot … mine` draws the table the game gives the block: a wall torch gives a torch, a block
/// without a table is refused.
#[test]
fn loot_mine_uses_the_block_table() {
    let (mut p, _dir) = open("wall-loot-mine");
    // Facing south: hung on the block to its north.
    p.run(&format!("setblock {} {} {} minecraft:stone\n", LOG[0], LOG[1], LOG[2] - 1));
    p.run(&format!("setblock {} {} {} minecraft:wall_torch[facing=south]\n", LOG[0], LOG[1], LOG[2]));
    let placed = p.s.data.block_of(p.s.st(LOG)).map(|b| b.name.as_str());
    assert_eq!(placed, Some("minecraft:wall_torch"), "wall torch placed");
    let at = format!("{} {} {}", LOG[0], LOG[1], LOG[2]);
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, &format!("loot spawn {at} mine {at}"));
    assert!(ok, "loot of a wall torch refused");
    let torches = p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == "minecraft:torch").count();
    assert_eq!(torches, 1, "a torch from the wall torch's table");
    p.run(&format!("setblock {} {} {} minecraft:bedrock\n", LOG[0], LOG[1], LOG[2]));
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, &format!("loot spawn {at} mine {at}"));
    assert!(!ok, "bedrock has no loot table");
}

/// Allocations of one removal of `state`, averaged over `n` after a first one (tables and
/// sequences then known).
fn allocations_per_removal(p: &mut Probe, draw: Draw, removal: Removal, at: [i32; 3], state: u32, n: u64) -> f64 {
    let tool = enchanted("minecraft:diamond_pickaxe", "minecraft:fortune", 3);
    drop(remove(p, draw, removal, at, state, Some(&tool)));
    let before = crate::test_alloc::allocations();
    for _ in 0..n {
        drop(remove(p, draw, removal, at, state, Some(&tool)));
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
    // The counts and the margin need the whole new path (the engine and the sequences' lookup);
    // with a switch at 0, the shared draw only has to allocate no more than the former one.
    let switches_on = crate::loot::loot_context_borrowed() && crate::rng::sequence_looked_up_before_insert();
    for removal in Removal::ALL {
        let former = allocations_per_removal(&mut p, Draw::Former, removal, LOG, stone, 100);
        let shared = allocations_per_removal(&mut p, Draw::Shared, removal, LOG, stone, 100);
        let label = removal.label();
        eprintln!("stone {label}: {former:.1} allocations before, {shared:.1} after");
        if !switches_on {
            assert!(shared <= former, "stone {label}: {shared} allocations, the former draw {former}");
            continue;
        }
        if !matches!(removal, Removal::Blown) {
            assert_eq!(shared, 2.0, "stone {label}: only the list and the stack");
        }
        assert!(shared + 2.5 <= former, "stone {label}: {shared} allocations, the former draw {former}");
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
        for removal in Removal::ALL {
            let mut best = [f64::MAX; 2];
            let mut allocs = [0.0; 2];
            for (d, draw) in [Draw::Former, Draw::Shared].into_iter().enumerate() {
                allocs[d] = allocations_per_removal(&mut p, draw, removal, at, state, 1_000);
            }
            // Best of 7 trials of 5 000 removals, the two draws alternated (the minimum resists the load).
            for _ in 0..7 {
                for (d, draw) in [Draw::Former, Draw::Shared].into_iter().enumerate() {
                    let t = std::time::Instant::now();
                    for _ in 0..5_000 {
                        drop(remove(&mut p, draw, removal, at, state, Some(&tool)));
                    }
                    best[d] = best[d].min(t.elapsed().as_nanos() as f64 / 5_000.0);
                }
            }
            eprintln!(
                "{label:10} {:20}: before {:6.0} ns {:5.1} allocations, after {:6.0} ns {:5.1} allocations",
                removal.label(), best[0], allocs[0], best[1], allocs[1]
            );
        }
    }
}
