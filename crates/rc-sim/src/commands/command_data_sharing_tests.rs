//! `RUSTCRAFT_OPT_CMDDATASHARE` on the integrated server: a world opened with its command data
//! read in the version data (tags of the jar shared with the parsing data) suggests every
//! truncation of the suggestion corpus and runs commands that read registries and tags exactly
//! like a world opened with the copies of the former path; and the heap the command data asks for,
//! counted by the tests' allocator (bytes requested on this thread: a count, not a timing).

use rc_commands::data::force_shared_command_data;

use super::suggestion_equivalence_tests::{FORMER_SERVER_FINGERPRINT, corpus_traced, fingerprint, prepared_world};
use crate::blocks::probe::Probe;
use crate::test_alloc::requested_bytes;

/// Commands whose result reads the registries and the tags of the parsing data.
const READING_COMMANDS: &[&str] = &[
    "setblock 1 -60 1 minecraft:oak_log[axis=x]",
    "execute if block 1 -60 1 #minecraft:logs run say log",
    "execute if block 1 -60 1 #minecraft:logs[axis=x] run say axis",
    "execute if block 1 -60 1 #minecraft:wool run say wool",
    "fill 1 -60 1 2 -60 2 minecraft:stone replace #minecraft:logs",
    "give S3 minecraft:dirtt 1",
    "give S3 minecraft:diamond_sword[minecraft:damage=3] 1",
    "clear S3 #minecraft:swords",
    "effect give S3 minecraft:spedd 1",
    "effect give S3 minecraft:speed 1",
    "summon minecraft:zombie 3 -60 3 {NoAI:1b}",
    "execute if entity @e[type=#minecraft:undead] run say undead",
    "kill @e[type=minecraft:zombie]",
    "playsound minecraft:entity.pig.ambient master S3",
    "playsound minecraft:entity.pig.nope master S3",
    "function #ss:group",
    "loot give S3 loot minecraft:blocks/stone",
];

/// The suggestion corpus and the reading commands on a world opened with `shared` command data.
fn world_traced(shared: bool) -> (Vec<String>, Vec<String>) {
    force_shared_command_data(Some(shared));
    let (mut p, dir) = prepared_world(if shared { "share-on" } else { "share-off" });
    let suggestions = corpus_traced(&mut p, false);
    let mut runs = corpus_traced(&mut p, true);
    assert_eq!(runs, suggestions, "fast suggestions differ (shared {shared})");
    runs.clear();
    for c in READING_COMMANDS {
        let src = p.s.player_source();
        let r = p.s.perform(src, c);
        runs.push(format!("{c} -> {r:?} {:?}", event_kinds(&p)));
    }
    crate::tests_g3f13::finish(p, dir);
    force_shared_command_data(None);
    (suggestions, runs)
}

/// Kinds of the events sent since the last call (what reached the client, in order).
fn event_kinds(p: &Probe) -> Vec<String> {
    p.take_events().iter().map(|e| format!("{e:?}").chars().take_while(|c| c.is_alphanumeric()).collect()).collect()
}

fn on_server_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(crate::server::SOLO_STACK).spawn(f).expect("thread").join().expect("world run")
}

#[test]
fn shared_command_data_suggests_and_runs_like_the_copies() {
    let (former_suggestions, former_runs) = on_server_thread(|| world_traced(false));
    assert_eq!(fingerprint(&former_suggestions), FORMER_SERVER_FINGERPRINT, "former path changed");
    for (refused, accepted) in [("minecraft:dirtt 1 -> (false", "S3 minecraft:diamond_sword"), ("spedd 1 -> (false", "speed 1 -> (true")] {
        assert!(former_runs.iter().any(|r| r.contains(refused)), "{refused}: {former_runs:?}");
        assert!(former_runs.iter().any(|r| r.contains(accepted) && r.contains("-> (true")), "{accepted}: {former_runs:?}");
    }
    let (suggestions, runs) = on_server_thread(|| world_traced(true));
    assert_eq!(suggestions.len(), former_suggestions.len());
    for (n, f) in suggestions.iter().zip(&former_suggestions) {
        assert_eq!(n, f);
    }
    assert_eq!(runs, former_runs);
}

/// Bytes requested by `f`, and its result.
fn bytes_of<T>(f: impl FnOnce() -> T) -> (u64, T) {
    let before = requested_bytes();
    let out = f();
    (requested_bytes() - before, out)
}

/// Heap of the command data of an open world on both paths, as `[bench]` lines; the shared path
/// must ask for a small part of what the copies ask for (counts, not timings).
#[test]
fn shared_command_data_asks_for_a_fraction_of_the_heap() {
    let run = || {
        let (p, dir) = crate::tests_g3f13::open("cmd-footprint");
        let data = p.s.data.clone();
        let tags = super::registry_tags(&data, &p.s.regs);
        let mut heap = [[0u64; 2]; 2];
        for (shared, slot) in [(false, 0), (true, 1)] {
            force_shared_command_data(Some(shared));
            // The holder measured is the second one, like the integrated server after the client:
            // the first is made here, so the count never hangs on the path the world opened with
            // (the environment's switch).
            let _first_holder = rc_commands::Data::from_shared_version(&data);
            let (built, mut d) = bytes_of(|| rc_commands::Data::from_shared_version(&data));
            let (tags_given, ()) = bytes_of(|| {
                for (reg, map) in &tags {
                    d.set_tags_from(reg, map);
                }
            });
            heap[slot] = [built, tags_given];
        }
        crate::tests_g3f13::finish(p, dir);
        // The first holder of a version data builds the block table (the client, at startup).
        let fresh = std::sync::Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
        let (first_holder, kept) = bytes_of(|| rc_commands::Data::from_shared_version(&fresh));
        drop(kept);
        force_shared_command_data(None);
        eprintln!("[bench] command_data version_data first_holder_bytes {first_holder}");
        heap
    };
    let [[copied, tags_copied], [shared, tags_shared]] = on_server_thread(run);
    eprintln!("[bench] command_data version_data copied_bytes {copied} shared_bytes {shared}");
    eprintln!("[bench] command_data jar_tags copied_bytes {tags_copied} shared_bytes {tags_shared}");
    assert!(shared * 10 < copied, "shared command data asks for {shared} bytes, the copies {copied}");
    assert!(tags_shared * 2 < tags_copied, "shared tags ask for {tags_shared} bytes, the copies {tags_copied}");
}

/// Heap the objective criteria kept by the command data after the first suggestion of a criterion
/// hold (`RUSTCRAFT_OPT_SUGGESTFAST`): packed in one text, against one string per criterion (what
/// a copy of each form asks for: counts, printed). The packed form must ask for less.
#[test]
fn kept_objective_criteria_ask_for_less_than_one_string_each() {
    let data = std::sync::Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    let d = rc_commands::Data::from_shared_version(&data);
    let kept = d.objective_criteria();
    assert!(kept.len() > 9_000, "{} criteria: registries missing", kept.len());
    let (packed, _) = bytes_of(|| kept.clone());
    let (one_string_each, _) = bytes_of(|| {
        let mut v = Vec::with_capacity(kept.len());
        v.extend(kept.iter().map(str::to_owned));
        v
    });
    eprintln!("[bench] objective_criteria kept count {} packed_bytes {packed} one_string_each_bytes {one_string_each}", kept.len());
    assert!(packed < one_string_each, "packed {packed} bytes, one string each {one_string_each}");
}

/// `RUSTCRAFT_OPT_CMDPACKETBYTES`: the levels that keep only their packet's bytes give the same
/// packet, decoded again, as the levels that keep it decoded, and the same bytes as the file
/// (what LAN guests receive, byte for byte); they ask for less heap (a count, printed).
#[test]
fn packets_kept_as_bytes_are_the_decoded_ones() {
    let run = || {
        let data = rc_testdata::version_data().expect("26.3 version data missing");
        let path = rc_data::commands_path(&data.version);
        let file = std::fs::read(&path).expect("command tree missing: redo the data preparation");
        let bodies = crate::lan::split_levels(&file);
        let decoded = super::load_levels_with(&path, false);
        let bytes_only = super::load_levels_with(&path, true);
        assert_eq!((decoded.len(), bytes_only.len(), bodies.len()), (5, 5, 5), "command tree incomplete");
        let (mut decoded_heap, mut bytes_heap) = (0, 0);
        for op in 0..5 {
            let packet = bytes_only[op].packet().expect("packet decoded again");
            assert_eq!(Some(&packet), decoded[op].packet().as_ref(), "level {op}");
            assert_eq!(&bytes_only[op].packet_bytes()[..], &bodies[op][..], "level {op}");
            assert_eq!(&decoded[op].packet_bytes()[..], &bodies[op][..], "level {op}");
            // Heap a kept decoded packet holds: what a copy of it asks for.
            let (packet_heap, _) = bytes_of(|| packet.clone());
            let (tree_heap, _) = bytes_of(|| rc_commands::Tree::new(&packet));
            eprintln!("[bench] command_tree level{op} decoded_packet_bytes {packet_heap} body_bytes {} tree_bytes {tree_heap}", bodies[op].len());
            decoded_heap += packet_heap;
            bytes_heap += bodies[op].len() as u64;
        }
        eprintln!("[bench] command_tree five_levels decoded_packets_bytes {decoded_heap} bodies_bytes {bytes_heap}");
        assert!(bytes_heap + 500_000 < decoded_heap, "{bytes_heap} bytes kept, {decoded_heap} decoded");
    };
    on_server_thread(run);
}
