//! `/schedule` on a fresh flat world, against the game's rules: the queue is saved with the world
//! and read back, an `append` of the same tick runs once, a due function that clears or replaces
//! another one due at the same tick stops it, ids are complete (`foo` is `minecraft:foo`),
//! unknown and macro functions are refused while an unknown tag is accepted like an empty one,
//! the due tick is returned wrapped into 0..2^31-1.

use std::path::{Path, PathBuf};

use crate::blocks::probe::Probe;

const MCMETA: &str = r#"{"pack":{"description":"sc","pack_format":121,"min_format":121,"max_format":121}}"#;

const FUNCTIONS: &[(&str, &str)] = &[
    ("sc/function/mark", "scoreboard players add #mark sc 1\n"),
    ("sc/function/first", "scoreboard players add #first sc 1\nschedule clear sc:second\n"),
    ("sc/function/second", "scoreboard players add #second sc 1\n"),
    ("sc/function/again", "scoreboard players add #again sc 1\nschedule function sc:second 1t\n"),
    ("sc/function/macro", "$say $(text)\n"),
    ("sc/function/tagged", "scoreboard players add #tagged sc 1\n"),
    ("minecraft/function/plain", "scoreboard players add #plain sc 1\n"),
];

const TAG_ALL: (&str, &str) = ("sc/tags/function/all.json", r#"{"values":["sc:tagged"]}"#);

/// Flat world with the `sc` pack and the `sc` objective.
fn open_with_pack(tag: &str) -> (Probe, PathBuf) {
    let (p, dir) = crate::tests_g3f13::open(tag);
    let data = p.s.data.clone();
    drop(p);
    let pack = dir.join("datapacks").join("sc");
    let write = |rel: &str, body: &str| {
        let file = pack.join(rel);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("pack folder");
        std::fs::write(file, body).expect("pack file");
    };
    write("pack.mcmeta", MCMETA);
    for (name, body) in FUNCTIONS {
        write(&format!("data/{name}.mcfunction"), body);
    }
    write(&format!("data/{}", TAG_ALL.0), TAG_ALL.1);
    let mut p = crate::tests_g3f13::reopen(&dir, data);
    assert!(p.s.cmd.functions.contains_key("sc:mark"), "pack not loaded");
    server(&mut p, "scoreboard objectives add sc dummy");
    (p, dir)
}

fn server(p: &mut Probe, c: &str) -> (bool, i32) {
    let src = p.s.server_source();
    p.s.perform(src, c)
}

/// The command run by an operator: (success, value, chat lines).
fn operator(p: &mut Probe, c: &str) -> (bool, i32, String) {
    let _ = p.take_events();
    let mut src = p.s.player_source();
    src.level = 4;
    let (ok, v) = p.s.perform(src, c);
    let chat: Vec<String> = p.take_events().iter().map(|e| format!("{e:?}")).collect();
    (ok, v, chat.join("\n"))
}

fn score(p: &Probe, holder: &str) -> i32 {
    p.s.cmd.scores.get(holder, "sc").unwrap_or(0)
}

fn ticks(p: &mut Probe, n: usize) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn finish(p: Probe, dir: &Path) {
    crate::tests_g3f13::finish(p, dir.to_owned());
}

/// Entries saved at closing and read back at opening: they run at their tick after the reopening,
/// tags included, the replaced and duplicated ones not. Before: the queue was never written.
#[test]
fn scheduled_functions_survive_closing_the_world() {
    let (mut p, dir) = open_with_pack("schedule-saved");
    for c in [
        "schedule function sc:mark 5t",
        "schedule function sc:mark 9t append",
        "schedule function sc:mark 9t append",
        "schedule function #sc:all 7t",
        "schedule function sc:second 3t",
        "schedule function sc:second 4t",
    ] {
        assert!(server(&mut p, c).0, "refused: {c}");
    }
    let start = p.s.time;
    p.s.save(true);
    let data = p.s.data.clone();
    drop(p);
    let mut p = crate::tests_g3f13::reopen(&dir, data);
    let entries: Vec<(i64, String)> = p.s.cmd.scheduled.entries().into_iter().map(|(t, id, _)| (t - start, id)).collect();
    let want = [(4, "sc:second"), (5, "sc:mark"), (7, "#sc:all"), (9, "sc:mark")];
    let want: Vec<(i64, String)> = want.iter().map(|(t, id)| (*t, (*id).to_owned())).collect();
    assert_eq!(entries, want, "queue read back");
    assert_eq!(p.s.time, start, "time read back");
    ticks(&mut p, 4);
    assert_eq!((score(&p, "#second"), score(&p, "#mark")), (1, 0));
    ticks(&mut p, 5);
    assert_eq!((score(&p, "#mark"), score(&p, "#tagged")), (2, 1), "marks at 5 and 9, the tag at 7");
    assert!(p.s.cmd.scheduled.is_empty());
    finish(p, &dir);
}

/// Two `append` for the same tick: one entry, one run. Before: two runs.
#[test]
fn append_for_the_same_tick_runs_once() {
    let (mut p, dir) = open_with_pack("schedule-append");
    server(&mut p, "schedule function sc:mark 2t append");
    server(&mut p, "schedule function sc:mark 2t append");
    assert_eq!(p.s.cmd.scheduled.len(), 1);
    ticks(&mut p, 2);
    assert_eq!(score(&p, "#mark"), 1);
    finish(p, &dir);
}

/// A due function that clears another entry due at the same tick stops it; one that replaces it
/// moves it to the next tick. Before: every due entry ran.
#[test]
fn a_due_function_stops_the_entries_it_clears() {
    let (mut p, dir) = open_with_pack("schedule-clear");
    server(&mut p, "schedule function sc:first 1t");
    server(&mut p, "schedule function sc:second 1t");
    ticks(&mut p, 1);
    assert_eq!((score(&p, "#first"), score(&p, "#second")), (1, 0), "cleared by the first function");
    server(&mut p, "schedule function sc:again 1t");
    server(&mut p, "schedule function sc:second 1t");
    ticks(&mut p, 1);
    assert_eq!((score(&p, "#again"), score(&p, "#second")), (1, 0), "replaced for the next tick");
    ticks(&mut p, 1);
    assert_eq!(score(&p, "#second"), 1);
    finish(p, &dir);
}

/// Replies and ids like the game: the id is complete, `clear` finds it under both spellings, an
/// unknown function and a macro are refused, an unknown tag is queued, the due tick is returned
/// wrapped.
#[test]
fn schedule_ids_and_replies_follow_the_game() {
    let (mut p, dir) = open_with_pack("schedule-ids");
    let (ok, _, chat) = operator(&mut p, "schedule function plain 3t");
    assert!(ok && chat.contains("commands.schedule.created.function") && chat.contains("minecraft:plain"), "{chat}");
    assert_eq!(p.s.cmd.scheduled.names(), ["minecraft:plain"]);
    let (ok, n, chat) = operator(&mut p, "schedule clear minecraft:plain");
    assert!(ok && n == 1 && chat.contains("minecraft:plain"), "{chat}");
    operator(&mut p, "schedule function minecraft:plain 3t");
    let (ok, n, _) = operator(&mut p, "schedule clear plain");
    assert!(ok && n == 1, "clear without namespace");
    let (ok, _, chat) = operator(&mut p, "schedule clear plain");
    assert!(!ok && chat.contains("commands.schedule.cleared.failure") && chat.contains("minecraft:plain"), "{chat}");
    for (c, key) in [
        ("schedule function sc:missing 3t", "arguments.function.unknown"),
        ("schedule function sc:macro 3t", "commands.schedule.macro"),
        ("schedule function sc:macro 0t", "commands.schedule.same_tick"),
    ] {
        let (ok, _, chat) = operator(&mut p, c);
        assert!(!ok && chat.contains(key), "{c}: {chat}");
    }
    assert!(p.s.cmd.scheduled.is_empty(), "nothing refused was added");
    let (ok, _, chat) = operator(&mut p, "schedule function #sc:all 3t");
    assert!(ok && chat.contains("commands.schedule.created.tag") && !chat.contains("#sc"), "{chat}");
    assert_eq!(p.s.cmd.scheduled.names(), ["#sc:all"]);
    // `clear` takes a plain identifier: a tag's entry cannot be cleared (the game's quirk).
    assert!(!operator(&mut p, "schedule clear #sc:all").0);
    assert_eq!(p.s.cmd.scheduled.names(), ["#sc:all"]);
    // An unknown tag is accepted like an empty one (the game's lookup defaults to an empty list):
    // queued, due tick returned, nothing run when due.
    let due = p.s.time + 3;
    let (ok, v, chat) = operator(&mut p, "schedule function #sc:missing 3t");
    assert!(ok && chat.contains("commands.schedule.created.tag") && chat.contains("sc:missing"), "{chat}");
    assert_eq!(i64::from(v), due);
    assert!(p.s.cmd.scheduled.names().iter().any(|n| n == "#sc:missing"), "unknown tag queued");
    ticks(&mut p, 3);
    assert!(p.s.cmd.scheduled.is_empty(), "both tags ran at their tick");
    assert_eq!(score(&p, "#tagged"), 1, "the known tag ran once, the unknown one ran nothing");
    // Due tick past 2^31 - 1: wrapped like the game's floor modulo.
    p.s.time = i64::from(i32::MAX) + 4;
    let (ok, v) = server(&mut p, "schedule function sc:mark 2t");
    assert!(ok);
    assert_eq!(v, 6);
    finish(p, &dir);
}
