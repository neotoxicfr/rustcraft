//! The queue of `/schedule`: both layouts give the same runs, the game's rules (one entry per id
//! and tick, one due entry at a time, overdue ids left behind) and the saved file.

use super::*;

/// xorshift64*: the same sequence on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const NAMES: [&str; 8] = ["a:x", "a:y", "#a:t", "b:z", "a:w", "#b:u", "c:v", "zoé:é"];

/// One `/schedule` command as the server runs it, on one queue; due ticks are drawn within the
/// next 6 ticks, so several entries of one id often wait for the same tick.
fn apply(q: &mut ScheduledFunctions, rng: &mut Rng, now: i64, log: &mut Vec<String>) {
    let base = NAMES[rng.below(NAMES.len() as u64) as usize];
    let id = format!("{base}{}", rng.below(4));
    let due = now + 1 + rng.below(6) as i64;
    match rng.below(10) {
        0..=5 => q.add(due, id), // append
        6..=8 => {
            // replace (the default)
            q.remove_named(&id);
            q.add(due, id);
        }
        _ => log.push(format!("clear {id} -> {}", q.remove_named(&id))),
    }
}

/// How the due entries are taken in `trace`.
#[derive(Clone, Copy, PartialEq)]
enum Taking {
    OneAtATime,
    /// Sabotage: every due entry taken before any runs (the former rule).
    AllBeforeRunning,
}

/// 2,000 ticks of five schedules each (append, replace, clear, tags), entries read back overdue
/// now and then, the due functions themselves scheduling and clearing: every launch, clear count,
/// pending count and known id, one line each.
fn trace(ordered: bool, taking: Taking) -> Vec<String> {
    let mut q = ScheduledFunctions::new(ordered);
    let mut rng = Rng(0x5EED_0F5C);
    let mut log = Vec::new();
    for now in 0..2_000i64 {
        if rng.below(50) == 0 {
            // Read back from a file older than the world's time: due a few ticks ago.
            let id = format!("{}{}", NAMES[rng.below(8) as usize], rng.below(4));
            q.add(now - 1 - rng.below(3) as i64, id);
        }
        for _ in 0..5 {
            apply(&mut q, &mut rng, now, &mut log);
        }
        let mut ran = Vec::new();
        if taking == Taking::AllBeforeRunning {
            while let Some(t) = q.pop_due(now) {
                ran.push(t);
            }
            ran.reverse();
        }
        loop {
            let next = if taking == Taking::OneAtATime { q.pop_due(now) } else { ran.pop() };
            let Some(f) = next else { break };
            log.push(format!("{now} {f}"));
            // A launched function schedules or clears in turn.
            if rng.below(3) == 0 {
                apply(&mut q, &mut rng, now, &mut log);
            }
        }
        log.push(format!("pending {} known {:?}", q.len(), q.names()));
    }
    log
}

#[test]
fn ordered_queue_runs_like_the_list() {
    let list = trace(false, Taking::OneAtATime);
    let launched = list.iter().filter(|l| !l.starts_with("pending") && !l.starts_with("clear")).count();
    let cleared = list.iter().filter(|l| l.starts_with("clear") && !l.ends_with("-> 0")).count();
    assert!(launched > 1_000 && cleared > 100, "{launched} launched, {cleared} clears that removed something");
    assert_eq!(trace(true, Taking::OneAtATime), list, "ordered queue differs from the list");
    assert_ne!(trace(true, Taking::AllBeforeRunning), list, "sabotage (all due entries taken first) not detected");
}

/// `append` of an id already waiting for that tick adds nothing (the game keeps one entry per
/// id and tick); another tick is a second entry.
#[test]
fn an_id_waits_once_per_tick() {
    for ordered in [false, true] {
        let mut q = ScheduledFunctions::new(ordered);
        q.add(10, "a:x".into());
        q.add(10, "a:x".into());
        q.add(11, "a:x".into());
        assert_eq!(q.len(), 2, "ordered: {ordered}");
        assert_eq!(q.pop_due(11).as_deref(), Some("a:x"));
        assert_eq!(q.pop_due(11).as_deref(), Some("a:x"));
        assert_eq!(q.pop_due(11), None);
    }
}

/// A due entry cleared by the function run before it never runs; nothing due reads nothing.
#[test]
fn a_cleared_due_entry_does_not_run() {
    for ordered in [false, true] {
        let mut q = ScheduledFunctions::new(ordered);
        q.add(5, "a:first".into());
        q.add(5, "a:second".into());
        q.add(5, "#a:third".into());
        assert_eq!(q.pop_due(4), None);
        assert_eq!(q.pop_due(5).as_deref(), Some("a:first"));
        assert_eq!(q.remove_named("a:second"), 1, "ordered: {ordered}");
        assert_eq!(q.pop_due(5).as_deref(), Some("#a:third"));
        assert_eq!(q.pop_due(5), None);
        assert!(q.is_empty() && q.names().is_empty());
    }
}

/// An entry taken after its due tick leaves its id: listed, counted and removed by `clear`.
#[test]
fn an_overdue_entry_leaves_its_id_behind() {
    for ordered in [false, true] {
        let mut q = ScheduledFunctions::new(ordered);
        q.add(3, "a:late".into());
        q.add(8, "a:on_time".into());
        q.add(9, "a:on_time".into());
        assert_eq!(q.pop_due(8).as_deref(), Some("a:late"));
        assert_eq!(q.pop_due(8).as_deref(), Some("a:on_time"));
        assert_eq!(q.len(), 1);
        assert_eq!(q.names(), ["a:late", "a:on_time"], "ordered: {ordered}");
        assert_eq!(q.remove_named("a:late"), 1, "the overdue id is counted");
        assert_eq!(q.remove_named("a:late"), 0);
        assert_eq!(q.names(), ["a:on_time"]);
    }
}

fn s(t: &str) -> Nbt {
    Nbt::String(t.to_owned())
}

fn event(due: i64, id: &str, kind: &str, target: &str) -> Nbt {
    Nbt::Compound(vec![
        ("trigger_time".into(), Nbt::Long(due)),
        ("id".into(), s(id)),
        ("callback".into(), Nbt::Compound(vec![("type".into(), s(kind)), ("id".into(), s(target))])),
    ])
}

/// The saved file, as the game writes it: events in run order, a tag's callback without its `#`.
#[test]
fn saved_file_holds_the_events_in_run_order() {
    for ordered in [false, true] {
        let mut q = ScheduledFunctions::new(ordered);
        q.add(40, "minecraft:later".into());
        q.add(20, "#minecraft:tick".into());
        q.add(20, "ns:first/path".into());
        let expected = Nbt::Compound(vec![
            (
                "data".into(),
                Nbt::Compound(vec![(
                    "events".into(),
                    Nbt::List(vec![
                        event(20, "#minecraft:tick", "minecraft:function_tag", "minecraft:tick"),
                        event(20, "ns:first/path", "minecraft:function", "ns:first/path"),
                        event(40, "minecraft:later", "minecraft:function", "minecraft:later"),
                    ]),
                )]),
            ),
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
        ]);
        assert_eq!(q.file_nbt(), expected, "ordered: {ordered}");
    }
}

/// Reading back, like the game: events added in the file's order (a repeated id and tick once),
/// identifiers completed, an event the game refuses skipped, the others kept; a callback that
/// runs something else than its id keeps both.
#[test]
fn saved_events_are_read_like_the_game() {
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-schedule-read-{}", std::process::id())));
    let file = file_path(&world);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("data folder");
    let events = vec![
        event(30, "minecraft:b", "function", "b"),
        event(10, "#minecraft:t", "minecraft:function_tag", "t"),
        event(30, "minecraft:b", "minecraft:function", "minecraft:b"),
        event(12, "other", "minecraft:function", "ns:runs"),
        event(5, "bad", "minecraft:unknown", "ns:x"),
        event(5, "bad", "minecraft:function", "Not Valid"),
        Nbt::Compound(vec![("id".into(), s("no_time")), ("callback".into(), Nbt::Compound(Vec::new()))]),
    ];
    let root = Nbt::Compound(vec![
        ("data".into(), Nbt::Compound(vec![("events".into(), Nbt::List(events))])),
        ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ]);
    std::fs::write(&file, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).expect("file");
    for ordered in [false, true] {
        let mut q = ScheduledFunctions::new(ordered);
        q.read_saved(&world);
        let want = [
            (10, "#minecraft:t".to_owned(), "#minecraft:t".to_owned()),
            (12, "other".to_owned(), "ns:runs".to_owned()),
            (30, "minecraft:b".to_owned(), "minecraft:b".to_owned()),
        ];
        assert_eq!(q.entries(), want, "ordered: {ordered}");
        assert!(q.save_job(&world).is_none(), "a file read as is needs no write");
        q.add(50, "minecraft:b".into());
        assert!(q.save_job(&world).is_some(), "a change is written");
        assert!(q.save_job(&world).is_none(), "written once");
    }
}

/// No file, or a file without an event list: empty queue, written at the first save (the game
/// creates its server data when it is missing).
#[test]
fn missing_file_is_written_at_the_first_save() {
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-schedule-none-{}", std::process::id())));
    let mut q = ScheduledFunctions::new(true);
    q.read_saved(&world);
    assert!(q.is_empty());
    let Some(crate::save::Job::Dat { path, nbt, .. }) = q.save_job(&world) else { panic!("no write job") };
    assert_eq!(path, file_path(&world));
    let empty = Nbt::Compound(vec![
        ("data".into(), Nbt::Compound(vec![("events".into(), Nbt::List(Vec::new()))])),
        ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ]);
    assert_eq!(nbt, crate::nbtio::encode_root(&empty));
    assert!(q.save_job(&world).is_none());
}
