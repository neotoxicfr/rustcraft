//! Equivalence proof of `RUSTCRAFT_OPT_LAZYFEEDBACK`: one corpus of commands (tags, effects,
//! teleports, kills of one and of many entities, scores, storage, blocks, items, boss bars…) runs
//! from every kind of source — an operator, the silent server (a function), the server's own
//! reporting source (its successes reach the operators), a command block that tracks its output
//! and one that does not, a `/debug function` trace — then again with `send_command_feedback`
//! off. Every event sent to the client and every kept block output is the same whether the
//! replies are built always (former path) or only when someone receives them. Facts the game
//! fixes are checked on both runs (so a guard that drops a recipient fails even though both
//! paths share it), and a silent function builds no entity name at all.

use std::path::Path;

use super::Origin;
use super::paths::CommandPaths;
use super::src::Src;
use crate::blocks::probe::Probe;

const MCMETA: &str = r#"{"pack":{"description":"lf","pack_format":121,"min_format":121,"max_format":121}}"#;

/// The corpus, one command per line (also the body of `lf:work`).
const WORK: &str = "summon minecraft:item 3 -60 3 {Item:{id:\"minecraft:stone\",count:1},PickupDelay:32767}
summon minecraft:item 3 -60 4 {Item:{id:\"minecraft:dirt\",count:1},PickupDelay:32767}
summon minecraft:item 3 -60 5 {Item:{id:\"minecraft:dirt\",count:2},PickupDelay:32767}
tag @e[type=minecraft:pig] add seen
tag @e[type=minecraft:pig,limit=1,sort=nearest] add one
tag @e[type=minecraft:pig,limit=1,sort=nearest] list
tag @e[type=minecraft:pig] list
effect give @e[type=minecraft:pig] minecraft:speed 5 0 true
effect give @e[type=minecraft:pig,limit=1,sort=nearest] minecraft:slowness 5 0 true
effect clear @e[type=minecraft:pig,limit=1,sort=nearest] minecraft:slowness
effect clear @e[type=minecraft:pig]
tp @e[type=minecraft:pig,limit=1,sort=nearest] 2.5 -60 2.5
tp @e[type=minecraft:pig,limit=1,sort=furthest] @e[type=minecraft:pig,limit=1,sort=nearest]
kill @e[type=minecraft:item,limit=1,sort=nearest]
kill @e[type=minecraft:item]
scoreboard players add @e[type=minecraft:pig] lf 1
data modify storage lf:s n set value 1
data get storage lf:s n
data get storage lf:s n 2
data get storage lf:s
data merge storage lf:s {m:2}
data get entity @e[type=minecraft:pig,limit=1,sort=nearest] Health
time query gametime
setblock 5 -60 5 minecraft:stone
fill 6 -60 6 7 -60 7 minecraft:dirt
particle minecraft:flame 0 -59 0 0 0 0 0 1
random value 1..6
give @a minecraft:apple 2
experience add @a 1 points
clear @a minecraft:apple 1
title @a actionbar \"lf\"
bossbar add lf:b \"B\"
bossbar set lf:b value 5
bossbar set lf:b value 5
bossbar get lf:b value
bossbar list
playsound minecraft:entity.pig.ambient master @a
weather clear 100
";

fn write_pack(world: &Path) {
    let pack = world.join("datapacks").join("lf");
    let write = |rel: &str, body: &str| {
        let file = pack.join(rel);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("pack folder");
        std::fs::write(file, body).expect("pack file");
    };
    write("pack.mcmeta", MCMETA);
    write("data/lf/function/work.mcfunction", WORK);
}

const TRACKING_BLOCK: [i32; 3] = [9, -60, 9];
const SILENT_BLOCK: [i32; 3] = [11, -60, 9];

/// A command block's own source at `p` (its successes go to its output).
fn block_source(p: &Probe, at: [i32; 3]) -> Src {
    let mut s = p.s.server_source();
    s.origin = Origin::Block(at);
    s.pos = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.5, f64::from(at[2]) + 0.5];
    s.name = "@".into();
    s.display = rc_text::Component::text("@");
    s.silent = false;
    s
}

fn operator(p: &Probe) -> Src {
    let mut s = p.s.player_source();
    s.level = 4;
    s
}

/// The events sent to the client since the last call (advancement updates left out: they
/// carry the wall clock).
fn log_events(p: &Probe, log: &mut Vec<String>) {
    super::function_cache_tests::log_events(p, log);
}

/// The message kept by a command block, without its `[HH:mm:ss]` stamp (the wall clock).
fn kept_output(p: &Probe, at: [i32; 3]) -> Option<String> {
    let out = p.s.be_get(at)?.get("LastOutput")?;
    rc_text::from_nbt(out).map(|c| format!("{:?}", c.extra))
}

/// Every line of the corpus from `src`, its result and what it sent.
fn run_lines(p: &mut Probe, src: &dyn Fn(&Probe) -> Src, log: &mut Vec<String>, block: Option<[i32; 3]>) {
    for line in WORK.lines() {
        let r = p.s.perform(src(p), line);
        log.push(format!("{line} -> {r:?}"));
        log_events(p, log);
        if let Some(at) = block {
            log.push(format!("kept {:?}", kept_output(p, at)));
        }
    }
}

/// Entity names built while the silent server runs the corpus as a function.
fn names_built_by_silent_function(p: &mut Probe) -> u64 {
    let before = super::ents::DISPLAY_NAMES_BUILT.with(std::cell::Cell::get);
    let src = p.s.server_source();
    let _ = p.s.perform(src, "function lf:work");
    super::ents::DISPLAY_NAMES_BUILT.with(std::cell::Cell::get) - before
}

fn drive(p: &mut Probe) -> (Vec<String>, u64) {
    // The host is an operator (its successes relayed from other sources reach it).
    p.s.level.allow_commands = true;
    for c in [
        "scoreboard objectives add lf dummy",
        "summon minecraft:pig 2.5 -60 2.5 {NoAI:1b,Tags:[\"lf\"]}",
        "summon minecraft:pig 4.5 -60 2.5 {NoAI:1b,Tags:[\"lf\"]}",
        "summon minecraft:pig 6.5 -60 2.5 {NoAI:1b,Tags:[\"lf\"],CustomName:\"Porky\"}",
        "setblock 9 -60 9 minecraft:command_block{TrackOutput:1b}",
        "setblock 11 -60 9 minecraft:command_block{TrackOutput:0b}",
    ] {
        let src = p.s.server_source();
        assert!(p.s.perform(src, c).0, "setup refused: {c}");
    }
    let _ = p.take_events();
    let mut log = Vec::new();
    for feedback in ["true", "false"] {
        let src = p.s.server_source();
        p.s.perform(src, &format!("gamerule send_command_feedback {feedback}"));
        let _ = p.take_events();
        log.push(format!("== operator, feedback {feedback}"));
        run_lines(p, &operator, &mut log, None);
        log.push(format!("== silent server function, feedback {feedback}"));
        let src = p.s.server_source();
        let r = p.s.perform(src, "function lf:work");
        log.push(format!("function -> {r:?}"));
        log_events(p, &mut log);
        log.push(format!("== reporting server source, feedback {feedback}"));
        run_lines(p, &|p: &Probe| p.s.server_reply_source(), &mut log, None);
        log.push(format!("== tracking command block, feedback {feedback}"));
        run_lines(p, &|p: &Probe| block_source(p, TRACKING_BLOCK), &mut log, Some(TRACKING_BLOCK));
        log.push(format!("== command block without output, feedback {feedback}"));
        run_lines(p, &|p: &Probe| block_source(p, SILENT_BLOCK), &mut log, Some(SILENT_BLOCK));
        log.push(format!("== trace, feedback {feedback}"));
        let trace = super::cmds_ents::trace_of(|| {
            let src = p.s.server_source();
            let _ = p.s.perform(src, "function lf:work");
        });
        log.extend(trace.lines().map(str::to_owned));
        log_events(p, &mut log);
    }
    let src = p.s.server_source();
    p.s.perform(src, "gamerule send_command_feedback true");
    // A single target sent to another dimension and back: its name in the reply.
    log.push("== other dimension".into());
    for c in
        ["execute in minecraft:the_nether run tp @s 0.5 80 0.5", "execute in minecraft:overworld run tp @s 0.5 -60 0.5"]
    {
        let r = p.s.perform(operator(p), c);
        log.push(format!("{c} -> {r:?}"));
        log_events(p, &mut log);
    }
    let names = names_built_by_silent_function(p);
    let _ = p.take_events();
    log.push(format!("{:?}", p.s.cmd.storage));
    log.push(format!("{:?}", p.s.cmd.scores.to_nbt()));
    (log, names)
}

/// The corpus on a fresh flat world opened with `paths` (its own thread: same test seeds).
fn corpus_trace(paths: CommandPaths) -> (Vec<String>, u64) {
    let run = move || {
        CommandPaths::forced(paths, || {
            let (p, dir) = crate::tests_g3f13::open("lazyfb");
            let data = p.s.data.clone();
            drop(p);
            write_pack(&dir);
            let mut p = crate::tests_g3f13::reopen(&dir, data);
            assert_eq!(p.s.cmd.paths, paths, "world opened on other paths");
            assert!(p.s.cmd.functions.contains_key("lf:work"), "pack not loaded");
            let out = drive(&mut p);
            crate::tests_g3f13::finish(p, dir);
            out
        })
    };
    std::thread::Builder::new()
        .stack_size(crate::server::SOLO_STACK)
        .spawn(run)
        .expect("thread")
        .join()
        .expect("world run")
}

/// Lines of one section of the log (from its `== name` header to the next one).
fn section<'a>(log: &'a [String], name: &str) -> Vec<&'a str> {
    let start = log.iter().position(|l| l == name).unwrap_or_else(|| panic!("section {name} missing"));
    log[start + 1..].iter().take_while(|l| !l.starts_with("== ")).map(String::as_str).collect()
}

/// What the game fixes, whatever the path: who receives what.
fn check_recipients(log: &[String], what: &str) {
    let has = |name: &str, needle: &str| section(log, name).iter().any(|l| l.contains(needle));
    let chat = "Chat {";
    for needle in
        ["commands.tag.add.success.multiple", "commands.kill.success.single", "commands.effect.give.success.single"]
    {
        assert!(has("== operator, feedback true", needle), "{what}: the operator never got {needle}");
    }
    assert!(!has("== silent server function, feedback true", chat), "{what}: a silent function spoke");
    assert!(has("== reporting server source, feedback true", "chat.type.admin"), "{what}: no success relayed");
    assert!(has("== tracking command block, feedback true", "kept Some("), "{what}: no output kept");
    assert!(!has("== command block without output, feedback true", "kept Some("), "{what}: output without tracking");
    let traced = section(log, "== trace, feedback true").iter().filter(|l| l.contains("[M] ")).count();
    assert!(traced > 20, "{what}: {traced} messages traced");
    for name in ["== operator, feedback false", "== reporting server source, feedback false"] {
        assert!(!has(name, "chat.type.admin"), "{what}: relayed without feedback ({name})");
    }
    // Failures still reach the source; successes do not.
    let quiet = section(log, "== operator, feedback false");
    assert!(!quiet.iter().any(|l| l.contains("commands.tag.add.success")), "{what}: feedback off");
    assert!(quiet.iter().any(|l| l.contains("commands.tag.add.failed")), "{what}: a failure is still told");
    let traced = section(log, "== trace, feedback false").iter().filter(|l| l.contains("[M] ")).count();
    assert!(traced > 20, "{what}: the trace takes the messages even without feedback ({traced})");
    let back = section(log, "== other dimension").join("\n");
    assert!(back.matches("commands.teleport.success.location.single").count() == 2, "{what}: {back}");
    assert!(back.contains("S3"), "{what}: the player is not named: {back}");
}

#[test]
fn replies_are_the_same_whether_built_always_or_only_when_received() {
    let (reference, names_before) = corpus_trace(CommandPaths::OLD);
    check_recipients(&reference, "former path");
    assert!(reference.len() > 500, "trace too short: {} lines", reference.len());
    assert!(names_before > 0, "the former path names the entities of a silent function");
    let mut lazy = CommandPaths::OLD;
    lazy.lazy_feedback = true;
    for (what, paths) in [("lazy feedback", lazy), ("every path", CommandPaths::NEW)] {
        let (got, names) = corpus_trace(paths);
        super::function_cache_tests::assert_same(&reference, &got, what);
        check_recipients(&got, what);
        assert_eq!(names, 0, "{what}: a silent function built {names} entity names");
    }
}
