//! Equivalence proof of `RUSTCRAFT_OPT_SUGGESTFAST` on the server side: the suggestions the
//! integrated server computes (tree shared instead of copied, lists deduplicated in one pass,
//! criteria kept in order, the providers of the arguments marked `ask_server`: objectives,
//! triggers, entity tags, functions and function tags, storages, time markers and timelines,
//! scheduled functions, tick rates, tests, templates, score holders) are those of the former path
//! for every command of a corpus truncated at every position, at permission levels 0, 2 and 4.
//! The former results are checked against a fingerprint recorded with the former path.

use rc_commands::suggest::force_fast_suggestions;

use crate::blocks::probe::Probe;

const MCMETA: &str = r#"{"pack":{"description":"ss","pack_format":121,"min_format":121,"max_format":121}}"#;

/// Functions, a nested one and a function tag, for the `function` providers.
fn write_pack(world: &std::path::Path) {
    let pack = world.join("datapacks").join("ss");
    let write = |rel: &str, body: &str| {
        let file = pack.join(rel);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("pack folder");
        std::fs::write(file, body).expect("pack file");
    };
    write("pack.mcmeta", MCMETA);
    write("data/ss/function/alpha.mcfunction", "say alpha\n");
    write("data/ss/function/sub/beta.mcfunction", "say beta\n");
    write("data/ss/tags/function/group.json", r#"{"values":["ss:alpha"]}"#);
}

/// State read by the providers: objectives (one a trigger the player may use), entity tags
/// equal but for case, a storage, a scheduled function, a team.
const SETUP: &[&str] = &[
    "scoreboard objectives add Alpha dummy",
    "scoreboard objectives add alpha_two dummy",
    "scoreboard objectives add trig trigger",
    "scoreboard players enable S3 trig",
    "tag S3 add Bob",
    "tag S3 add bob",
    "tag S3 add zed",
    "data modify storage ss:store a set value 1",
    "data modify storage ss:other b set value 2",
    "schedule function ss:alpha 100t",
    "team add Reds",
];

/// Commands whose truncations reach the server's providers and the local suggestions it computes
/// like the client.
const CORPUS: &[&str] = &[
    "/",
    "/trigger tr",
    "/function ss:",
    "/function #ss:gr",
    "/tag @s remove b",
    "/data get storage ss:",
    "/time set ",
    "/time query ",
    "/schedule clear ss:",
    "/scoreboard players set @s Al",
    "/scoreboard players operation @s Alpha += S3 al",
    "/scoreboard objectives add x minecraft.used:minecraft.dia",
    "/tick rate 2",
    "/tick step 1",
    "/tick sprint 6",
    "/place template minecraft:",
    "/test run ",
    "/give @s minecraft:diamond_sw",
    "/setblock ~ ~ ~ minecraft:oak_stairs[facing=e",
    "/team join R",
    "/execute if entity @e[tag=b",
    "/item replace entity @s armor.he",
    "/loot give @s loot minecraft:blocks/st",
];

/// Suggestions of every truncation of the corpus at levels 0, 2 and 4, on the path forced.
fn corpus_traced(p: &mut Probe, fast: bool) -> Vec<String> {
    force_fast_suggestions(Some(fast));
    let mut out = Vec::new();
    for op in [0u8, 2, 4] {
        for text in CORPUS {
            let chars: Vec<char> = text.chars().collect();
            for end in 1..=chars.len() {
                let input: String = chars[..end].iter().collect();
                let s = p.s.compute_suggestions(&input, op);
                out.push(format!("{op} {input:?} {s:?}"));
            }
        }
    }
    force_fast_suggestions(None);
    out
}

/// FNV-1a of the traces, in order.
fn fingerprint(traces: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in traces {
        for b in t.bytes().chain(std::iter::once(b'\n')) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Fingerprint of the corpus traces recorded with the former path (`RUSTCRAFT_OPT_SUGGESTFAST=0`).
const FORMER_SERVER_FINGERPRINT: u64 = 9_867_571_266_016_822_840;

/// Flat world with the pack and the providers' state (also the world of the micro-bench).
pub(crate) fn prepared_world(tag: &str) -> (Probe, std::path::PathBuf) {
    let (p, dir) = crate::tests_g3f13::open(tag);
    let data = p.s.data.clone();
    drop(p);
    write_pack(&dir);
    let mut p = crate::tests_g3f13::reopen(&dir, data);
    assert!(p.s.cmd.functions.contains_key("ss:alpha"), "pack not loaded");
    p.s.level.allow_commands = true;
    p.s.cmd.op = 4;
    for c in SETUP {
        let src = p.s.server_source();
        assert!(p.s.perform(src, c).0, "setup refused: {c}");
    }
    let _ = p.take_events();
    (p, dir)
}

fn both_paths() -> (Vec<String>, Vec<String>) {
    let (mut p, dir) = prepared_world("suggest");
    let former = corpus_traced(&mut p, false);
    let now = corpus_traced(&mut p, true);
    crate::tests_g3f13::finish(p, dir);
    (former, now)
}

#[test]
fn server_suggestions_match_the_former_path() {
    let (former, now) = std::thread::Builder::new()
        .stack_size(crate::server::SOLO_STACK)
        .spawn(both_paths)
        .expect("thread")
        .join()
        .expect("world run");
    assert!(former.len() > 1_500, "only {} truncations traced", former.len());
    for needle in ["\"trig\"", "\"ss:sub/beta\"", "\"#ss:group\"", "\"ss:store\"", "\"Bob\"", "\"alpha_two\""] {
        assert!(former.iter().any(|t| t.contains(needle)), "{needle} never suggested");
    }
    assert_eq!(fingerprint(&former), FORMER_SERVER_FINGERPRINT, "former path changed: fingerprint to record");
    assert_eq!(now.len(), former.len());
    for (n, f) in now.iter().zip(&former) {
        assert_eq!(n, f);
    }
}
