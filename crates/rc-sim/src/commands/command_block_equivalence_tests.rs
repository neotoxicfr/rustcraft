//! Equivalence proof of the command block paths (`COMMANDBLOCKBORROW`, `LOCALTIMECACHE`,
//! `PARSECACHE`): impulse, repeating and chain blocks (conditional chain, `Searge`, tracked and
//! untracked output, failing command, `UpdateLastExecution:0b`, custom name, a block that switches
//! itself off, another that replaces the block behind it) run 200 ticks on fresh copies of the flat
//! world, every path off, each path alone, then all on: the events and the block entity data of
//! every command block after each tick are the same. The output stamps read a frozen clock that
//! moves one second per tick (minute changes included).

use super::cmdblock::FROZEN_NOW;
use super::paths::CommandPaths;
use crate::blocks::probe::Probe;

const Y: i32 = -58;

/// (x, z, block, NBT) of the blocks placed, in order.
const BLOCKS: &[(i32, i32, &str, &str)] = &[
    // Repeating chain: counter, then a chain that counts, a conditional chain, a failing one.
    (3, 8, "repeating_command_block[facing=east]", "{auto:1b,Command:'scoreboard players add #rep cb 1'}"),
    (4, 8, "chain_command_block[facing=east]", "{auto:1b,Command:'scoreboard players add #chain cb 1'}"),
    (5, 8, "chain_command_block[facing=east,conditional=true]", "{auto:1b,Command:'say cond'}"),
    (6, 8, "chain_command_block[facing=east]", "{auto:1b,TrackOutput:1b,Command:'foo bar'}"),
    (7, 8, "chain_command_block[facing=east,conditional=true]", "{auto:1b,Command:'say never'}"),
    // Lone repeating blocks.
    (3, 10, "repeating_command_block", "{auto:1b,Command:'Searge'}"),
    (5, 10, "repeating_command_block", "{auto:1b,TrackOutput:1b,Command:'unknowncommand here'}"),
    (7, 10, "repeating_command_block", "{auto:1b,UpdateLastExecution:0b,Command:'scoreboard players add #nolast cb 1'}"),
    (9, 10, "repeating_command_block", "{auto:1b,CustomName:'\"Named\"',TrackOutput:1b,Command:'say @s'}"),
    (11, 10, "repeating_command_block", "{auto:1b,TrackOutput:1b,Command:'data merge block ~ ~ ~ {auto:0b}'}"),
    (13, 10, "repeating_command_block[facing=east]", "{auto:1b,Command:'execute if score #rep cb matches 20.. run setblock ~1 ~ ~ minecraft:stone'}"),
    (14, 10, "chain_command_block[facing=east]", "{auto:1b,Command:'scoreboard players add #replaced cb 1'}"),
    // Impulse block powered once by a redstone block placed next to it.
    (3, 12, "command_block", "{TrackOutput:1b,Command:'scoreboard players set #impulse cb 7'}"),
];

fn place_blocks(p: &mut Probe) {
    let run = |p: &mut Probe, c: &str| {
        let src = p.s.server_source();
        assert!(p.s.perform(src, c).0, "setup refused: {c}");
    };
    run(p, "scoreboard objectives add cb dummy");
    for (x, z, block, nbt) in BLOCKS {
        run(p, &format!("setblock {x} {Y} {z} minecraft:{block}{nbt}"));
    }
    run(p, &format!("setblock 3 {} 12 minecraft:redstone_block", Y + 1));
}

/// Every command block's data after each of 200 ticks, and the events.
fn trace(paths: CommandPaths) -> Vec<String> {
    let run = move || {
        CommandPaths::forced(paths, || {
            let (mut p, dir) = crate::tests_g3f13::open("cmdblocks");
            assert_eq!(p.s.cmd.paths, paths, "world opened on other paths");
            FROZEN_NOW.with(|f| f.set(Some(1_700_000_000)));
            place_blocks(&mut p);
            let _ = p.take_events();
            let mut log = Vec::new();
            for t in 0..200u64 {
                FROZEN_NOW.with(|f| f.set(Some(1_700_000_000 + t)));
                p.s.game_tick();
                p.s.tick += 1;
                log.push(format!("tick {t}"));
                super::function_cache_tests::log_events(&p, &mut log);
                for (x, z, _, _) in BLOCKS {
                    log.push(format!("{x} {z} {:?}", p.s.be_get([*x, Y, *z])));
                }
            }
            log.push(format!("{:?}", p.s.cmd.scores.to_nbt()));
            FROZEN_NOW.with(|f| f.set(None));
            crate::tests_g3f13::finish(p, dir);
            log
        })
    };
    std::thread::Builder::new().stack_size(crate::server::SOLO_STACK).spawn(run).expect("thread").join().expect("world run")
}

#[test]
fn command_blocks_run_the_same_on_every_path() {
    let reference = trace(CommandPaths::OLD);
    let text = reference.join("\n");
    // The blocks did what they claim: chain counted, Searge answered, a failure was stamped (its
    // last output line: the context offering the command), the impulse block ran once, the
    // self-stopping block stopped, the replaced chain stopped.
    for needle in ["#itzlipofutzli", "suggest_command", "Text(\"cond\")", "Named"] {
        assert!(text.contains(needle), "the blocks never produced {needle}");
    }
    // One output stamp per tick (the frozen clock moves a second per tick, across minutes).
    let stamps: std::collections::BTreeSet<&str> =
        text.match_indices("String(\"[").map(|(i, _)| &text[i + 8..(i + 19).min(text.len())]).collect();
    assert!(stamps.len() >= 190, "{} distinct output stamps: {stamps:?}", stamps.len());
    let last = reference.last().expect("scores");
    for holder in ["#rep", "#chain", "#nolast", "#impulse", "#replaced"] {
        assert!(last.contains(holder), "{holder} never counted: {last}");
    }
    let cases = [
        ("command block borrow", {
            let mut p = CommandPaths::OLD;
            p.command_block_borrow = true;
            p
        }),
        ("local time cache", {
            let mut p = CommandPaths::OLD;
            p.local_time_cache = true;
            p
        }),
        ("parse cache", {
            let mut p = CommandPaths::OLD;
            p.parse_cache = true;
            p
        }),
        ("every path", CommandPaths::NEW),
    ];
    for (what, paths) in cases {
        let got = trace(paths);
        if let Some(i) = (0..reference.len().max(got.len())).find(|&i| reference.get(i) != got.get(i)) {
            panic!("{what}: first difference at line {i}:\n  before: {:?}\n  now:    {:?}", reference.get(i), got.get(i));
        }
    }
}
