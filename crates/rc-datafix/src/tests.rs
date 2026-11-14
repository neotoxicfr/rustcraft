mod oracle;
mod snbt;

use rc_proto::nbt::Nbt;
use snbt::nbt;

use crate::chain::{CHAIN, OLDEST_CONVERTIBLE};
use crate::kind::Kind;
use crate::map::Schema;
use crate::{Context, FileKind, Outcome, data_version, requires_file_fixing, upgrade};

/// The game asks to move files first for any world before its last file move (4899).
#[test]
fn file_fixing_needed_below_the_last_file_move() {
    for (version, needed) in [(-1, true), (1343, true), (4771, true), (4772, true), (4898, true), (4899, false), (5023, false), (6000, false)] {
        assert_eq!(requires_file_fixing(version), needed, "level.dat at {version}");
    }
}

/// A file without `DataVersion` takes its reader's default; any integer width is read.
#[test]
fn version_read_with_each_readers_default() {
    let empty = nbt("{}");
    for (kind, default) in [
        (FileKind::Chunk, -1),
        (FileKind::EntityChunk, -1),
        (FileKind::Player, -1),
        (FileKind::Level, -1),
        (FileKind::PoiChunk, 1945),
        (FileKind::Hotbar, 1343),
        (FileKind::Stats, 1343),
        (FileKind::Structure, 500),
        (FileKind::DebugProfile, 4649),
    ] {
        assert_eq!(data_version(&empty, kind), default, "{kind:?}");
    }
    assert_eq!(data_version(&nbt("{DataVersion: 4950s}"), FileKind::Chunk), 4950);
    assert_eq!(data_version(&nbt("{DataVersion: 4950L}"), FileKind::Chunk), 4950);
}

/// Current and newer data are left alone, data older than the chain too (byte for byte), and
/// converted data says the current version.
#[test]
fn only_data_the_chain_covers_is_converted() {
    let context = Context::default();
    let palette = r#"sections: [{Y: 0b, block_states: {palette: [{Name: "stone"}]}}]"#;
    for (version, expected) in [
        (crate::CURRENT_VERSION, Outcome::Current),
        (6000, Outcome::Newer(6000)),
        (OLDEST_CONVERTIBLE - 1, Outcome::TooOld(OLDEST_CONVERTIBLE - 1)),
        (2000, Outcome::TooOld(2000)),
    ] {
        let original = nbt(&format!("{{DataVersion: {version}, {palette}}}"));
        let mut chunk = original.clone();
        assert_eq!(upgrade(FileKind::Chunk, &mut chunk, &context), expected);
        assert_eq!(chunk, original, "version {version}: data changed");
    }
    let mut absent = nbt(&format!("{{{palette}}}"));
    assert_eq!(upgrade(FileKind::Chunk, &mut absent, &context), Outcome::TooOld(-1));
    let mut old = nbt(&format!("{{DataVersion: {OLDEST_CONVERTIBLE}, {palette}}}"));
    assert_eq!(upgrade(FileKind::Chunk, &mut old, &context), Outcome::Upgraded(OLDEST_CONVERTIBLE));
    assert_eq!(old.get("DataVersion"), Some(&Nbt::Int(crate::CURRENT_VERSION)));
    let state = &old.get("sections").unwrap();
    // A block state is untyped inside: its identifier keeps the form it was written in.
    assert_eq!(snbt::show(state), r#"[{"Y":0b,"block_states":{"palette":[{"id":"stone"}]}}]"#);
}

/// The chain is in the game's order, inside the versions it claims to cover.
#[test]
fn chain_is_ordered_and_covers_its_versions() {
    let keys: Vec<i32> = CHAIN.iter().map(|r| r.key()).collect();
    assert!(keys.windows(2).all(|w| w[0] <= w[1]), "rules out of order: {keys:?}");
    assert!(keys.iter().all(|&k| k > OLDEST_CONVERTIBLE * 10 && k <= crate::CURRENT_VERSION * 10));
}

/// Skipping the walks of rules whose kind a file cannot hold changes nothing: every case of the
/// game's converter gives the same tree both ways (`RUSTCRAFT_OPT_DATAFIX_SKIP_WALKS_WITHOUT_TARGET`).
#[test]
fn skipped_walks_change_nothing() {
    for case in oracle::cases() {
        let context = Context { dimension: case.dimension, generator: None };
        let from = data_version(&nbt(case.input), case.kind);
        let [mut skipped, mut walked] = [nbt(case.input), nbt(case.input)];
        crate::chain::apply_with(case.kind.root_kind(), &mut skipped, from, &context, true);
        crate::chain::apply_with(case.kind.root_kind(), &mut walked, from, &context, false);
        assert_eq!(skipped, walked, "{}", case.name);
    }
    // The file kinds that hold no item stack, and those that do.
    let at = Schema::of_version(4950);
    assert!(!crate::map::can_hold(Kind::Chunk, Kind::Chunk, at) && !crate::map::can_hold(Kind::Chunk, Kind::DebugProfile, at));
    assert!(crate::map::can_hold(Kind::Chunk, Kind::ItemStack, at) && crate::map::can_hold(Kind::ItemStack, Kind::ItemStack, at));
    assert!(!crate::map::can_hold(Kind::PoiChunk, Kind::ItemStack, at) && !crate::map::can_hold(Kind::Level, Kind::BlockState, at));
}

/// Micro-bench (figures only, never checked): a chunk of 26.2 with 24 sections of 8 states, 20
/// chests of 27 stacks and 8 minecarts converted, rules walking every file or only those holding
/// their kind. `cargo test --release -p rc-datafix --lib -- --ignored --nocapture bench_chunk`
#[test]
#[ignore = "micro-bench"]
fn bench_chunk_conversion() {
    let state = r#"{Name: "minecraft:oak_stairs", Properties: {facing: "north", half: "top", shape: "straight"}}"#;
    let palette = [state; 8].join(", ");
    let sections: Vec<String> =
        (-4..20).map(|y| format!(r#"{{Y: {y}b, block_states: {{palette: [{palette}]}}, biomes: {{palette: ["minecraft:plains"]}}}}"#)).collect();
    let stack = r#"{Slot: 0b, id: "minecraft:filled_map", count: 1, components: {"minecraft:map_decorations": {"+": {type: "minecraft:mansion"}},
        "minecraft:lore": [{text: "a"}], "minecraft:custom_name": {text: "b"}}}"#;
    let items = [stack; 27].join(", ");
    let chests: Vec<String> = (0..20).map(|i| format!(r#"{{id: "minecraft:chest", x: {i}, y: 64, z: 0, Items: [{items}]}}"#)).collect();
    let carts: Vec<String> =
        (0..8).map(|_| format!(r#"{{id: "minecraft:chest_minecart", DisplayState: {state}, Items: [{items}]}}"#)).collect();
    let chunk = |block_entities: &[String], entities: &[String]| {
        nbt(&format!(
            r#"{{DataVersion: 4950, xPos: 0, zPos: 0, Status: "minecraft:full", sections: [{}], block_entities: [{}], entities: [{}]}}"#,
            sections.join(", "),
            block_entities.join(", "),
            entities.join(", ")
        ))
    };
    let filled = chunk(&chests, &carts);
    // Most chunks: terrain and a block entity or two.
    let typical = chunk(&chests[..2], &[]);
    let context = Context { dimension: Some("minecraft:overworld"), generator: None };
    let runs = 200;
    for (name, chunk) in [("20 chests and 8 minecarts", &filled), ("2 chests", &typical)] {
        let mut best = [f64::MAX; 2];
        for pass in 0..8 {
            let skip = pass % 2 == 1;
            let started = std::time::Instant::now();
            for _ in 0..runs {
                let mut c = chunk.clone();
                crate::chain::apply_with(Kind::Chunk, &mut c, 4950, &context, skip);
                std::hint::black_box(&c);
            }
            let converting = started.elapsed();
            // The copies of the input alone, taken out of the figure.
            let clone_start = std::time::Instant::now();
            for _ in 0..runs {
                std::hint::black_box(chunk.clone());
            }
            let per_chunk = converting.saturating_sub(clone_start.elapsed()).as_secs_f64() / f64::from(runs) * 1e6;
            best[usize::from(skip)] = best[usize::from(skip)].min(per_chunk);
        }
        eprintln!(
            "chunk conversion, {name}: every walk {:.0} us, walks skipped {:.0} us per chunk (best of 4, machine loaded)",
            best[0], best[1]
        );
    }
}

/// The walk reaches item stacks wherever the map puts them: in a chest, a shulker box item in
/// it, a bundle in that, a minecart's chest, a passenger's hand, a trade, a hover text.
#[test]
fn walk_reaches_every_nested_item_stack() {
    let mut chunk = nbt(
        r#"{block_entities: [{id: "minecraft:chest", Items: [
                {id: "minecraft:shulker_box", components: {"minecraft:container": [{slot: 0, item: {id: "minecraft:bundle",
                    components: {"minecraft:bundle_contents": [{id: "a"}, "b"]}}}]}}]}],
            entities: [{id: "minecraft:chest_minecart", Items: [{id: "c"}],
                Passengers: [{id: "minecraft:zombie", equipment: {mainhand: {id: "d"}},
                    Passengers: [{id: "minecraft:villager", Offers: {Recipes: [{buy: {id: "e"}, sell: {id: "f"}}]}}]}]},
                {id: "minecraft:text_display", text: {text: "x", hover_event: {action: "show_item", id: "g"}}},
                {id: "othermod:thing", Items: [{id: "not visited"}]}]}"#,
    );
    let mut seen = Vec::new();
    crate::walk::visit(Kind::Chunk, &mut chunk, Schema::of_version(5023), &mut |kind, node| {
        if kind == Kind::ItemStack {
            seen.push(node.get("id").and_then(Nbt::as_str).map_or_else(|| snbt::show(node), str::to_owned));
        }
    });
    // In the map's order: chunk entities before block entities, passengers before their vehicle.
    assert_eq!(seen, ["e", "f", "d", "c", "g", "minecraft:shulker_box", "minecraft:bundle", "a", "\"b\""]);
}
