//! Equivalence proof of the function and command paths (`FUNCTIONCACHE`, `MACROCACHE`,
//! `SHAREDSOURCE`, `PARSECACHE`, `PARSERNOALLOC`, `SCHEDULEQUEUE`): one corpus of functions
//! (macros called with the same then other arguments, an invalid and a missing one, `return run`,
//! the command quota, forks, `execute store`, tags, `schedule` append / replace / clear, a name
//! changed between `execute as` and `say`, commands typed by an operator, valid and not) runs on
//! fresh copies of the flat world, every path off, each path alone, then all on: the results, the
//! events sent to the client, the scores, the storage and the entities are the same. Each world
//! runs on its own thread, so the test seeds (`rng::test_seed`) are the same for all.

use std::path::Path;

use super::paths::CommandPaths;
use crate::blocks::probe::Probe;

const MCMETA: &str = r#"{"pack":{"description":"fc","pack_format":121,"min_format":121,"max_format":121}}"#;

const FUNCTIONS: &[(&str, &str)] = &[
    (
        "plain",
        "scoreboard players add #a fc 1\n\
         scoreboard players operation #b fc += #a fc\n\
         execute store result score #s fc run scoreboard players get #a fc\n\
         execute store success storage fc:out ok byte 1 run scoreboard players get #b fc\n\
         data modify storage fc:out list append value 1\n\
         execute if score #a fc matches 1.. run say plain é\n\
         tellraw @a {\"text\":\"Zoé ✓\"}\n",
    ),
    (
        "mac",
        "# two parameters, the second used twice\n\
         $scoreboard players set #m fc $(v)\n\
         $say $(t) then $(v) then $(t)\n\
         scoreboard players add #m fc 100\n\
         $execute if score #m fc matches $(v)..$(v) run say not reached\n",
    ),
    ("ret", "scoreboard players set #after fc 0\nreturn run scoreboard players set #r fc 5\nscoreboard players set #after fc 1\n"),
    ("val", "return 42\n"),
    ("outer", "return run function fc:val\nsay never\n"),
    ("rec", "scoreboard players add #n fc 1\nexecute if score #n fc matches ..300 run function fc:rec\n"),
    (
        "fork",
        "execute as @e[type=minecraft:armor_stand,tag=fc] at @s run function fc:each\n\
         execute as @e[type=minecraft:armor_stand,tag=fc] run return run function fc:inc\n\
         say after fork\n",
    ),
    (
        "each",
        "scoreboard players add @s fc 1\nsay @s\n\
         execute store result entity @s Rotation[0] float 1 run scoreboard players get @s fc\n",
    ),
    ("inc", "scoreboard players add #inc fc 1\n"),
    ("rename", "data merge entity @s {CustomName:\"Bob\"}\nsay my name before the merge\n"),
    (
        "name",
        "execute as @e[type=minecraft:armor_stand,tag=named] run function fc:rename\n\
         execute as @e[type=minecraft:armor_stand,tag=named] run say now\n",
    ),
    (
        "sched",
        "schedule function fc:later 2t append\nschedule function fc:later 2t append\n\
         schedule function fc:later 3t\nschedule function #fc:all 1t\nschedule function fc:other 5t\n\
         schedule clear fc:other\nschedule function fc:other 4t append\n",
    ),
    ("later", "scoreboard players add #later fc 1\nsay later\n"),
    ("other", "say other\n"),
    ("tick", "scoreboard players add #ticks fc 1\nfunction fc:mac with storage fc:args\n"),
    ("load", "say loaded\n"),
    ("cond", "execute if function fc:val run say yes\nexecute unless function fc:noret run say no result\n"),
    ("noret", "scoreboard players set #nr fc 1\n"),
    ("cont", "scoreboard players \\\n    set #c fc \\\n 3\n"),
];

const TAGS: &[(&str, &str)] = &[
    ("fc/tags/function/all.json", r#"{"values":["fc:inc","fc:later"]}"#),
    ("minecraft/tags/function/tick.json", r#"{"values":["fc:tick"]}"#),
    ("minecraft/tags/function/load.json", r#"{"values":["fc:load"]}"#),
];

fn write_pack(world: &Path) {
    let pack = world.join("datapacks").join("fc");
    let write = |rel: &str, body: &str| {
        let file = pack.join(rel);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("pack folder");
        std::fs::write(file, body).expect("pack file");
    };
    write("pack.mcmeta", MCMETA);
    for (name, body) in FUNCTIONS {
        write(&format!("data/fc/function/{name}.mcfunction"), body);
    }
    for (path, body) in TAGS {
        write(&format!("data/{path}"), body);
    }
}

/// Commands of the corpus run by the server.
const SERVER_COMMANDS: &[&str] = &[
    "function fc:plain",
    "function fc:plain",
    "function fc:cont",
    "function fc:mac {v:3,t:\"a b\"}",
    "function fc:mac {v:3,t:\"a b\"}",
    "function fc:mac {v:0,t:x}",
    "function fc:mac {v:1,t:x}",
    "function fc:mac {v:2,t:x}",
    "function fc:mac {v:4,t:x}",
    "function fc:mac {v:5,t:x}",
    "function fc:mac {v:6,t:x}",
    "function fc:mac {v:7,t:x}",
    "function fc:mac {v:8,t:x}",
    "function fc:mac {v:9,t:x}",
    "function fc:mac {v:3,t:\"a b\"}",
    "function fc:mac {v:9,t:x}",
    "function fc:mac {v:1.5f,t:float}",
    "function fc:mac {v:\"abc\",t:bad}",
    "function fc:mac {t:\"no v\"}",
    "function fc:mac",
    "function fc:mac with storage fc:args",
    "function fc:ret",
    "function fc:outer",
    "function fc:rec",
    "function fc:fork",
    "function fc:name",
    "function fc:sched",
    "function fc:cond",
    "function #fc:all",
    "function #fc:missing",
    "function fc:missing",
    "execute if function fc:val",
    "execute store result score #ev fc run function fc:val",
    "gamerule max_command_sequence_length 10",
    "function fc:plain",
    "function fc:rec",
    "gamerule max_command_sequence_length 65536",
    "execute as @e[type=minecraft:armor_stand] run function fc:each",
];

/// Commands typed by an operator, twice each (a repeated text comes from the parse cache).
const PLAYER_COMMANDS: &[&str] = &[
    "function fc:plain",
    "function fc:mac {v:7,t:p}",
    "say typed",
    "/scoreboard players get #a fc",
    "scoreboard bogus",
    "foo bar",
    "tp @s ~ ~ ~",
    "execute as @e[type=minecraft:armor_stand,limit=2] run say hi",
];

fn digest_line(log: &mut Vec<String>, what: impl std::fmt::Debug) {
    log.push(format!("{what:?}"));
}

/// The events sent to the client since the last call, one line each; the advancement updates
/// are left out (they carry the wall-clock time of the recipe unlocked when the player joins).
pub(super) fn log_events(p: &Probe, log: &mut Vec<String>) {
    for e in p.take_events() {
        let line = format!("{e:?}");
        if !line.starts_with("Progress(Advancements(") {
            log.push(line);
        }
    }
}

/// The corpus on a fresh flat world opened with `paths`: every result, event and the final
/// state, one line each.
fn corpus_trace(paths: CommandPaths) -> Vec<String> {
    let run = move || {
        CommandPaths::forced(paths, || {
            let (p, dir) = crate::tests_g3f13::open("fncache");
            let data = p.s.data.clone();
            drop(p);
            write_pack(&dir);
            let mut p = crate::tests_g3f13::reopen(&dir, data);
            assert_eq!(p.s.cmd.paths, paths, "world opened on other paths");
            assert!(p.s.cmd.functions.len() >= FUNCTIONS.len(), "pack not loaded: {:?}", p.s.cmd.functions.keys());
            let log = drive(&mut p);
            crate::tests_g3f13::finish(p, dir);
            log
        })
    };
    std::thread::Builder::new().stack_size(crate::server::SOLO_STACK).spawn(run).expect("thread").join().expect("world run")
}

fn drive(p: &mut Probe) -> Vec<String> {
    let mut log = Vec::new();
    let server = |p: &mut Probe, c: &str| {
        let src = p.s.server_source();
        p.s.perform(src, c)
    };
    for c in [
        "scoreboard objectives add fc dummy",
        "summon minecraft:armor_stand 2 -60 2 {Tags:[\"fc\"],NoGravity:1b}",
        "summon minecraft:armor_stand 4 -60 2 {Tags:[\"fc\"],NoGravity:1b}",
        "summon minecraft:armor_stand 6 -60 2 {Tags:[\"named\"],CustomName:\"Ann\",NoGravity:1b}",
        "data merge storage fc:args {v:5,t:\"stored\"}",
    ] {
        assert!(server(p, c).0, "setup refused: {c}");
    }
    let _ = p.take_events();
    let operator = |p: &mut Probe, c: &str| {
        let mut src = p.s.player_source();
        src.level = 4;
        p.s.perform(src, c)
    };
    // Each command run by the server (silent: results only), then by an operator (its messages
    // and errors reach the chat).
    for c in SERVER_COMMANDS {
        let r = server(p, c);
        log.push(format!("server {c} -> {r:?}"));
        log_events(p, &mut log);
        let r = operator(p, c);
        log.push(format!("operator {c} -> {r:?}"));
        log_events(p, &mut log);
    }
    for c in PLAYER_COMMANDS.iter().chain(PLAYER_COMMANDS) {
        let r = operator(p, c);
        log.push(format!("operator {c} -> {r:?}"));
        log_events(p, &mut log);
    }
    for t in 0..30 {
        p.s.game_tick();
        p.s.tick += 1;
        log.push(format!("tick {t}"));
        log_events(p, &mut log);
    }
    digest_line(&mut log, p.s.cmd.scores.to_nbt());
    digest_line(&mut log, &p.s.cmd.storage);
    let mut pending = p.s.cmd.scheduled.names();
    pending.sort();
    digest_line(&mut log, pending);
    // The armor stands, as the player sees them (`data get` answers in the chat).
    for tag in ["fc", "named"] {
        let mut src = p.s.player_source();
        src.level = 4;
        p.s.perform(src, &format!("execute as @e[type=minecraft:armor_stand,tag={tag}] run data get entity @s"));
        log_events(p, &mut log);
    }
    log
}

pub(super) fn assert_same(reference: &[String], got: &[String], what: &str) {
    if let Some(i) = (0..reference.len().max(got.len())).find(|&i| reference.get(i) != got.get(i)) {
        panic!(
            "{what}: first difference at line {i} of {}:\n  before: {:?}\n  now:    {:?}",
            reference.len(),
            reference.get(i),
            got.get(i)
        );
    }
}

/// One switch on alone.
fn only(set: impl Fn(&mut CommandPaths)) -> CommandPaths {
    let mut p = CommandPaths::OLD;
    set(&mut p);
    p
}

#[test]
fn functions_and_commands_run_the_same_on_every_path() {
    let reference = corpus_trace(CommandPaths::OLD);
    // The corpus did what it claims: macros instantiated, quota hit, scheduled functions ran.
    let text = reference.join("\n");
    for needle in [
        "commands.function.error.parse",
        "commands.function.error.missing_argument",
        "commands.function.error.missing_arguments",
        "Text(\"later\")",
        "Text(\"other\")",
        "then 9 then x",
        "Text(\"loaded\")",
        "command.unknown.command",
    ] {
        assert!(text.contains(needle), "the corpus never produced {needle}");
    }
    assert!(reference.len() > 300, "trace too short: {} lines", reference.len());
    let cases = [
        ("function cache", only(|p| p.function_cache = true)),
        ("macro cache", only(|p| p.macro_cache = true)),
        ("shared source", only(|p| p.shared_source = true)),
        ("parse cache", only(|p| p.parse_cache = true)),
        ("parser without copies", only(|p| p.parser_no_alloc = true)),
        ("schedule queue", only(|p| p.schedule_queue = true)),
        ("every path", CommandPaths::NEW),
    ];
    for (what, paths) in cases {
        assert_same(&reference, &corpus_trace(paths), what);
    }
}
