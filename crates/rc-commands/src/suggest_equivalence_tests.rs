//! Equivalence proof of `RUSTCRAFT_OPT_SUGGESTFAST` on the client side: the suggestions built in
//! one pass (first occurrences kept by a hash set, fixed lists built once, criteria kept in
//! order, values already in lowercase compared as they are) are those of the former path — texts,
//! ranges, tooltips and order — on tricky lists, then on every command of a corpus truncated at
//! every position (what the client computes at each key press), at three permission levels of
//! the real tree. The former results are also checked against a fingerprint recorded with the
//! former path, and two deliberate errors (last occurrence kept, sort key left unfolded) must be
//! seen by the proof.

use crate::args::{BLOCK, STAT_TYPE, build_slot_names, criteria_list, slot_count};
use crate::parse::{Source, completions};
use crate::suggest::{
    SABOTAGE, SABOTAGE_KEEP_LAST, SABOTAGE_UNFOLDED_KEY, Suggestion, Suggestions, Tooltip, force_fast_suggestions,
};
use crate::{Data, Tree, parse};

/// `f` run with `RUSTCRAFT_OPT_SUGGESTFAST` forced to `fast` on this thread.
fn with_switch<T>(fast: bool, f: impl FnOnce() -> T) -> T {
    force_fast_suggestions(Some(fast));
    let out = f();
    force_fast_suggestions(None);
    out
}

/// `f` run on the fast path with one deliberate error.
fn sabotaged<T>(sabotage: u8, f: impl FnOnce() -> T) -> T {
    SABOTAGE.with(|s| s.set(sabotage));
    let out = with_switch(true, f);
    SABOTAGE.with(|s| s.set(0));
    out
}

fn suggestion(start: usize, end: usize, text: &str, tooltip: Option<&str>) -> Suggestion {
    Suggestion { start, end, text: text.to_owned(), tooltip: tooltip.map(|k| Tooltip::Key(k.to_owned())) }
}

/// Lists where the order or the kept occurrence could differ: equal but for case, same text with
/// another tooltip, the folding cases of `game_case_insensitive_order`, several ranges.
fn tricky_lists() -> Vec<Vec<Suggestion>> {
    let word = |t: &str| suggestion(3, 6, t, None);
    let folding = [
        "ß", "SS", "ss", "ΑΣ", "ας", "İ", "i", "\u{10400}", "\u{10428}", "a\u{10400}", "a\u{10428}", "\u{1F600}",
        "\u{FF21}", "_x", "a", "ab", "AB_", "alice", "Bob",
    ];
    vec![
        vec![word("bob"), word("Bob"), word("BOB"), word("bob"), word("alice")],
        vec![
            suggestion(3, 6, "x", Some("a")),
            suggestion(3, 6, "x", None),
            suggestion(3, 6, "X", Some("a")),
            suggestion(3, 6, "x", Some("a")),
            suggestion(3, 6, "x", Some("b")),
        ],
        folding.iter().map(|t| word(t)).collect(),
        folding.iter().rev().map(|t| word(t)).collect(),
        vec![word("Bob"), suggestion(1, 6, "x Bob", None), suggestion(3, 9, "Bob ßİ", None), word("Bob")],
        vec![suggestion(1, 6, "x Bob", None), word("Bob"), suggestion(1, 6, "x bob", None), word("bob")],
        Vec::new(),
    ]
}

#[test]
fn one_pass_creation_keeps_the_former_result_on_tricky_lists() {
    let input: Vec<char> = "/x Bob ßİ".chars().collect();
    for list in tricky_lists() {
        let former = with_switch(false, || Suggestions::create(&input, list.clone()));
        let now = with_switch(true, || Suggestions::create(&input, list.clone()));
        assert_eq!(now, former, "{list:?}");
    }
}

#[test]
fn tricky_lists_see_the_sabotages() {
    let input: Vec<char> = "/x Bob ßİ".chars().collect();
    for sabotage in [SABOTAGE_KEEP_LAST, SABOTAGE_UNFOLDED_KEY] {
        let seen = tricky_lists().into_iter().any(|list| {
            let former = with_switch(false, || Suggestions::create(&input, list.clone()));
            sabotaged(sabotage, || Suggestions::create(&input, list)) != former
        });
        assert!(seen, "sabotage {sabotage} not seen by the tricky lists");
    }
}

#[test]
fn slot_lookup_matches_the_rebuilt_list() {
    let names = build_slot_names();
    assert!(names.len() > 150, "only {} slot names", names.len());
    let unknown = ["", "container.54", "hotbar.9", "armor", "weapon.*.x", "Contents", "player.crafting.4"];
    for name in names.iter().map(|(n, _)| n.as_str()).chain(unknown) {
        let former = with_switch(false, || slot_count(name));
        let now = with_switch(true, || slot_count(name));
        assert_eq!(now, former, "{name}");
    }
    assert_eq!(with_switch(true, || slot_count("container.*")), Some(54));
    assert_eq!(with_switch(true, || slot_count("contents")), Some(1));
}

/// The criteria in the order the suggestions end in.
fn criteria_in_order(d: &Data) -> Vec<String> {
    let mut v = criteria_list(d);
    v.sort_by(|a, b| crate::suggest::cmp_ignore_case(a, b));
    v
}

#[test]
fn criteria_are_listed_again_after_a_registry_change() {
    let mut d = Data::default();
    d.set_registry(STAT_TYPE, vec!["minecraft:mined".into(), "minecraft:custom".into()]);
    d.set_registry(BLOCK, vec!["minecraft:stone".into()]);
    assert_eq!(d.objective_criteria(criteria_list), criteria_in_order(&d));
    assert!(d.objective_criteria(criteria_list).iter().any(|c| c == "minecraft.mined:minecraft.stone"));
    d.set_registry(BLOCK, vec!["minecraft:dirt".into()]);
    assert_eq!(d.objective_criteria(criteria_list), criteria_in_order(&d));
    assert!(d.objective_criteria(criteria_list).iter().any(|c| c == "minecraft.mined:minecraft.dirt"));
    assert!(!d.objective_criteria(criteria_list).iter().any(|c| c == "minecraft.mined:minecraft.stone"));
}

/// Commands whose truncations reach every kind of local suggestion: literals, items and their
/// components, block states and loose properties of a tag, the 9 000 objective criteria, slots,
/// colours, enums, selectors and their options, coordinates, particles, sounds, summonable
/// entities (with tooltips), resources and tags, player names equal but for case.
pub(crate) const CORPUS: &[&str] = &[
    "/",
    "/gi",
    "/give @s diamond_sword[custom_name='x',!damage] 1",
    "/give @a minecraft:stone[",
    "/setblock ~ ~ ~ minecraft:oak_stairs[facing=east,half=top]",
    "/setblock ~ ~ ~ chest{",
    "/fill ~ ~ ~ ~1 ~1 ~1 stone replace #logs[axis=y]",
    "/execute if block ~ ~-1 ~ #minecraft:logs[axis=x] run say x",
    "/clone ~ ~ ~ ~ ~ ~ ~ ~ ~ filtered #minecraft:wool",
    "/scoreboard objectives add x minecraft.used:minecraft.dia",
    "/scoreboard objectives add y teamkill.re",
    "/scoreboard objectives setdisplay sidebar.team.r",
    "/item replace entity @s armor.he",
    "/item replace block ~ ~ ~ container.1",
    "/team modify red color dark_",
    "/team join ",
    "/gamemode spec",
    "/tp @e[type=minecraft:cow,limit=3,sort=nea",
    "/tp @s ^ ^ ^",
    "/tp 1 2",
    "/execute as @a at @s anchored ey",
    "/execute positioned 1 2 3 rotated ~ ~ facing entity @p eyes run tp ",
    "/particle minecraft:dust{color:[1.0,0.0,0.0],scale:1} ~ ~ ~",
    "/playsound minecraft:entity.pig.amb",
    "/summon minecraft:zom",
    "/effect give @s minecraft:spe",
    "/time set 1",
    "/clear @s *[damage]",
    "/data get entity @s Inventory[0].id",
    "/tell bo",
    "/msg Zo",
    "/enchant @s sharp",
    "/locate structure #minecraft:vil",
    "/loot give @s loot minecraft:blocks/st",
    "/attribute @s minecraft:armor base set 1",
    "/damage @s 1 minecraft:fall",
    "/kill @e[tag=",
];

/// The five permission levels of the real tree (extractor), and the version data with players
/// and teams (two names equal but for case, one twice).
pub(crate) fn real_trees() -> (Vec<Tree>, Data) {
    let version = rc_testdata::version_data().expect("26.3 version data missing");
    let path = rc_data::commands_path(&version.version);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("command tree missing ({}): {e}", path.display()));
    let mut trees = Vec::new();
    let mut at = 0usize;
    while at + 4 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let body = &bytes[at + 4..at + 4 + len];
        let packet = rc_proto_v777::commands::read_commands(&mut rc_proto::Reader::new(body)).expect("command tree");
        trees.push(Tree::new(&packet));
        at += 4 + len;
    }
    assert_eq!(trees.len(), 5, "command tree incomplete: redo the data preparation");
    let mut d = Data::from_version(&version);
    d.players = ["Alex", "bob", "Bob", "bob", "Zoé"].map(str::to_owned).to_vec();
    d.teams = ["red", "Blue", "blue"].map(str::to_owned).to_vec();
    d.target_block = Some([1, -60, 2]);
    d.target_point = Some([1.5, -59.0, 2.25]);
    d.target_entity = Some("00000000-0000-0001-0000-000000000002".into());
    (trees, d)
}

/// Parse and suggestions of every truncation of `text` at permission level `op`, as text.
fn truncations_traced(tree: &Tree, data: &Data, text: &str, op: i32, out: &mut Vec<String>) {
    let src = Source { restricted: true, selectors: op >= 2 };
    let chars: Vec<char> = text.chars().collect();
    for end in 1..=chars.len() {
        let input = &chars[..end];
        let p = parse(tree, input, data, src);
        let c = completions(tree, &p, input, input.len(), data, src);
        let s = Suggestions::merge(input, c.local);
        out.push(format!("{op} {:?} {:?} server {} {:?}", input.iter().collect::<String>(), p, c.server, s));
    }
}

/// Every truncation of the corpus at levels 0, 2 and 4, on the path chosen by the caller.
fn corpus_traced(trees: &[Tree], data: &Data) -> Vec<String> {
    let mut out = Vec::new();
    for op in [0, 2, 4] {
        let mut d = data.clone();
        d.op_level = op;
        for text in CORPUS {
            truncations_traced(&trees[op as usize], &d, text, op, &mut out);
        }
    }
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
const FORMER_CORPUS_FINGERPRINT: u64 = 17_208_984_829_149_098_860;

#[test]
fn corpus_suggestions_match_the_former_path() {
    let (trees, data) = real_trees();
    let former = with_switch(false, || corpus_traced(&trees, &data));
    assert!(former.len() > 3_000, "only {} truncations traced", former.len());
    assert!(
        former.iter().any(|t| t.contains("minecraft.used:minecraft.diamond_sword")),
        "the criteria were never suggested"
    );
    assert_eq!(fingerprint(&former), FORMER_CORPUS_FINGERPRINT, "former path changed: fingerprint to record");
    let now = with_switch(true, || corpus_traced(&trees, &data));
    assert_eq!(now.len(), former.len());
    for (n, f) in now.iter().zip(&former) {
        assert_eq!(n, f);
    }
    for sabotage in [SABOTAGE_KEEP_LAST, SABOTAGE_UNFOLDED_KEY] {
        let wrong = sabotaged(sabotage, || corpus_traced(&trees, &data));
        assert_ne!(wrong, former, "sabotage {sabotage} not seen by the corpus");
    }
}
