//! Selector searches by candidate view (`RUSTCRAFT_OPT_SELECTOR_CANDIDATE_VIEW`), type first
//! (`…_SELECTOR_TYPE_FIRST`), the first-match `execute if entity`
//! (`…_EXECUTE_IF_ENTITY_FIRST_MATCH`), the entity lookup by UUID and the kept entity index,
//! each against the former code on a fresh flat world of about 1,000 mixed entities (26.3 version
//! data required): same entities, same order, same draws on the world's random.

use super::ents::E;
use super::paths::CommandPaths;
use super::select::parse;
use crate::blocks::probe::Probe;

/// Fixed-seed generator of the test (no clock).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }

    fn coin(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    assert!(p.s.perform(src, c).0, "command refused: {c}");
}

const MOBS: [&str; 6] = ["zombie", "husk", "cow", "pig", "chicken", "skeleton"];
const TAGS: [&str; 3] = ["a", "b", "c"];

/// Flat world with about 1,000 entities of every family: mobs (one dead), markers, armor stands,
/// ground items, orbs, minecarts, an End crystal; tags, names, rotations, teams and scores.
fn mixed_world(tag: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    let mut r = Lcg(7);
    for i in 0..1_000 {
        let (x, y, z) = (r.below(640) as f64 / 10.0, -60.0 + r.below(40) as f64 / 10.0, r.below(480) as f64 / 10.0 - 16.0);
        let tags: Vec<String> = TAGS.iter().filter(|_| r.coin(30)).map(|t| format!("\"{t}\"")).collect();
        let rot = format!("Rotation:[{}f,{}f]", r.below(360) as i32 - 180, r.below(180) as i32 - 90);
        let name = if r.coin(10) { ",CustomName:\"bob\"" } else { "" };
        let common = format!("Tags:[{}],{rot}{name}", tags.join(","));
        let text = match i % 10 {
            0..=4 => format!("summon minecraft:{} {x} {y} {z} {{NoAI:1b,Silent:1b,PersistenceRequired:1b,{common}}}", r.pick(&MOBS)),
            5 | 6 => format!("summon minecraft:marker {x} {y} {z} {{{common}}}"),
            7 => format!("summon minecraft:armor_stand {x} {y} {z} {{NoGravity:1b,Small:{}b,{common}}}", r.below(2)),
            8 => format!("summon minecraft:item {x} {y} {z} {{Item:{{id:\"minecraft:stone\",count:1}},PickupDelay:32767s,NoGravity:1b,{common}}}"),
            _ if i % 20 == 9 => format!("summon minecraft:experience_orb {x} {y} {z} {{Value:3,{common}}}"),
            _ => format!("summon minecraft:minecart {x} {y} {z} {{{common}}}"),
        };
        cmd(&mut p, &text);
    }
    cmd(&mut p, "summon minecraft:end_crystal 8 -58 8 {ShowBottom:0b}");
    for c in [
        "team add red",
        "team add blue",
        "team join red @e[tag=a,type=!player,limit=150]",
        "team join blue @e[tag=b,tag=!a,limit=80]",
        "scoreboard objectives add s dummy",
        "scoreboard objectives add t dummy",
        "execute as @e[type=!player] store result score @s s run data get entity @s Pos[0]",
        "execute as @e[tag=c] store result score @s t run data get entity @s Pos[2]",
        "tag @p add a",
    ] {
        cmd(&mut p, c);
    }
    // One mob dead in its slot (not listed by `@e`, still found by identifier).
    let first = p.s.mobs.list.iter().next().map(|m| m.id).expect("mob");
    p.s.mobs.get_mut(first).expect("mob").health = 0.0;
    let _ = p.take_events();
    (p, dir)
}

/// One selector of the usual shapes, options drawn from the generator.
fn generated_selector(r: &mut Lcg) -> String {
    let letter = r.pick(&["e", "e", "e", "e", "a", "p", "r", "n", "s"]);
    let mut opts: Vec<String> = Vec::new();
    let current = letter == "s";
    if r.coin(45) {
        let t = r.pick(&["zombie", "cow", "marker", "armor_stand", "item", "experience_orb", "minecart", "end_crystal", "player"]);
        opts.push(format!("type={}{t}", if r.coin(70) { "" } else { "minecraft:" }));
    }
    if r.coin(25) {
        opts.push(format!("type=!{}", r.pick(&["husk", "pig", "marker", "item"])));
    }
    if r.coin(10) {
        opts.push(format!("type={}#minecraft:{}", if r.coin(50) { "" } else { "!" }, r.pick(&["undead", "skeletons", "zombies"])));
    }
    if r.coin(40) {
        let (a, b) = (r.below(30), r.below(30) + 1);
        opts.push(format!("distance={}", r.pick(&[&format!("..{b}"), &format!("{a}.."), &format!("{}..{}", a.min(b), a.max(b)), "..0.5"])));
    }
    if r.coin(15) {
        opts.push(format!("x={},y=-60,z={}", r.below(64), r.below(48) as i64 - 16));
    }
    if r.coin(15) {
        opts.push(format!("dx={},dy={},dz={}", r.below(20) as i64 - 10, r.below(4), r.below(20) as i64 - 10));
    }
    if !current && r.coin(35) {
        opts.push(format!("sort={}", r.pick(&["nearest", "furthest", "random", "arbitrary"])));
    }
    if !current && r.coin(35) {
        opts.push(format!("limit={}", 1 + r.below(40)));
    }
    if r.coin(35) {
        let t = r.pick(&TAGS);
        opts.push(format!("tag={}{}", if r.coin(30) { "!" } else { "" }, if r.coin(10) { "" } else { t }));
    }
    if r.coin(15) {
        opts.push(format!("team={}{}", if r.coin(30) { "!" } else { "" }, r.pick(&["red", "blue", ""])));
    }
    if r.coin(15) {
        opts.push(format!("scores={{s={}..{}}}", r.below(30), 30 + r.below(30)));
    }
    if r.coin(5) {
        opts.push(format!("scores={{s=..{},t={}..}}", r.below(64), r.below(20) as i64 - 10));
    }
    if r.coin(8) {
        opts.push(format!("name={}{}", if r.coin(40) { "!" } else { "" }, r.pick(&["bob", "Zombie", "Cow", "Marker", "Stone"])));
    }
    if r.coin(8) {
        opts.push(format!("y_rotation={}..{}", r.below(360) as i64 - 180, r.below(360) as i64 - 180));
    }
    if r.coin(8) {
        opts.push(format!("x_rotation=..{}", r.below(180) as i64 - 90));
    }
    if r.coin(5) {
        opts.push(format!("nbt={}{{Tags:[\"{}\"]}}", if r.coin(30) { "!" } else { "" }, r.pick(&TAGS)));
    }
    if r.coin(3) {
        opts.push("gamemode=survival".to_owned());
    }
    if r.coin(3) {
        opts.push("level=0..".to_owned());
    }
    if opts.is_empty() { format!("@{letter}") } else { format!("@{letter}[{}]", opts.join(",")) }
}

/// Source at a drawn place, executed by a drawn entity (for `@s`).
fn drawn_source(p: &Probe, r: &mut Lcg, ents: &[E]) -> super::src::Src {
    let mut src = p.s.player_source();
    src.pos = [r.below(640) as f64 / 10.0, -60.0 + r.below(30) as f64 / 10.0, r.below(480) as f64 / 10.0 - 16.0];
    src.entity = Some(ents[r.below(ents.len() as u64) as usize]);
    src
}

/// The paths compared with the former search (every other command path as the defaults).
fn variants() -> [(&'static str, CommandPaths); 4] {
    let with = |view: bool, type_first: bool, any: bool| CommandPaths {
        selector_candidate_view: view,
        selector_type_first: type_first,
        execute_if_entity_first_match: any,
        ..CommandPaths::NEW
    };
    [
        ("candidate view", with(true, false, false)),
        ("candidate view, type first", with(true, true, false)),
        ("candidate view, first match", with(true, false, true)),
        ("every path", CommandPaths::NEW),
    ]
}

/// 1,500 drawn selectors from drawn sources: every variant gives the former list, leaves the
/// world's random where the former search left it, and `execute if entity`'s answer (empty or
/// not) matches.
#[test]
fn generated_selectors_match_the_former_search() {
    // The comparison is this test's own (the search's built-in check against the former one off).
    crate::mobs::index::set_verify(false);
    let (mut p, dir) = mixed_world("select-equivalence");
    let old = CommandPaths { selector_candidate_view: false, selector_type_first: false, execute_if_entity_first_match: false, ..CommandPaths::NEW };
    let ents = p.s.all_ents();
    assert!(ents.len() > 900, "test setup: {} entities", ents.len());
    let mut r = Lcg(2026);
    let (mut empty, mut many, mut shuffled) = (0, 0, 0);
    for _ in 0..1_500 {
        let text = generated_selector(&mut r);
        let sel = parse(&text).expect("generated selector");
        let src = drawn_source(&p, &mut r, &ents);
        let random_before = p.s.life.rng;
        p.s.cmd.paths = old;
        let expected = p.s.select(&sel, &src);
        let random_after = p.s.life.rng;
        for (name, paths) in variants() {
            p.s.cmd.paths = paths;
            p.s.life.rng = random_before;
            assert_eq!(p.s.select(&sel, &src), expected, "{name}: {text}");
            assert_eq!(p.s.life.rng, random_after, "{name}: {text}: world's random");
            p.s.life.rng = random_before;
            assert_eq!(p.s.select_finds_any(&sel, &src), !expected.is_empty(), "{name}: {text}: any");
            assert_eq!(p.s.life.rng, random_after, "{name}: {text}: world's random after `any`");
        }
        p.s.life.rng = random_after;
        empty += usize::from(expected.is_empty());
        many += usize::from(expected.len() > 1);
        shuffled += usize::from(random_after != random_before);
    }
    crate::mobs::index::set_verify(true);
    // The drawn selectors are not trivial: some find nothing, some several, some shuffle.
    eprintln!("generated selectors: {empty} empty, {many} with several entities, {shuffled} shuffled");
    assert!(empty > 50 && many > 100 && shuffled > 10, "generator: {empty} empty, {many} several, {shuffled} shuffled");
    crate::tests_g3f13::finish(p, dir);
}

/// Whole commands: `execute as … at @s if|unless entity … run …` (first match), the terminal
/// form (count) and a random order before another step give the same results, scores and world
/// random on every path.
#[test]
fn execute_if_entity_matches_the_former_runs() {
    crate::mobs::index::set_verify(false);
    let (mut p, dir) = mixed_world("select-execute");
    cmd(&mut p, "scoreboard objectives add hits dummy");
    let commands = [
        "execute as @e[type=zombie] at @s if entity @e[distance=..3,type=!zombie] run scoreboard players add @s hits 1",
        "execute as @e[type=marker,tag=a] at @s unless entity @e[distance=..2,type=marker,tag=!a] run scoreboard players add @s hits 2",
        "execute as @e[type=cow] at @s if entity @e[distance=..4,sort=random] run scoreboard players add @s hits 4",
        "execute if entity @e[type=zombie,tag=b]",
        "execute unless entity @e[type=pig,distance=..1]",
        "execute if entity @e[sort=random,type=marker]",
        "execute as @e[type=item,limit=30] at @s if entity @s[tag=a] run scoreboard players add @s hits 8",
        "execute as @e[type=marker] if entity @s[scores={s=10..40}] run scoreboard players add @s hits 16",
    ];
    let src = p.s.server_source();
    let trace = |p: &mut Probe, paths: CommandPaths| {
        p.s.cmd.paths = paths;
        let _ = p.s.perform(src.clone(), "scoreboard players reset * hits");
        let random_before = p.s.life.rng;
        let results: Vec<(bool, i32)> = commands.iter().map(|c| p.s.perform(src.clone(), c)).collect();
        let scores: Vec<(E, Option<i32>)> =
            p.s.all_ents().into_iter().map(|e| (e, p.s.cmd.scores.get(&p.s.ent_score_name(e), "hits"))).collect();
        let random_after = p.s.life.rng;
        p.s.life.rng = random_before;
        (results, scores, random_after)
    };
    let reference = trace(&mut p, CommandPaths::OLD);
    assert!(reference.1.iter().filter(|(_, v)| v.is_some()).count() > 50, "test setup: few hits");
    for (name, paths) in variants() {
        assert!(trace(&mut p, paths) == reference, "{name}: execute runs differ");
    }
    crate::mobs::index::set_verify(true);
    crate::tests_g3f13::finish(p, dir);
}

/// Lookup by UUID against the former one: every entity, UUID 0, an unknown UUID, a dead mob, a
/// UUID shared by a mob and a kept compound (the smaller identifier wins), a UUID shared by two
/// mobs, the dead player.
#[test]
fn entity_by_uuid_matches_the_former_lookup() {
    let (mut p, dir) = mixed_world("select-uuid");
    let check = |p: &Probe, u: u128, what: &str| {
        assert_eq!(p.s.ent_by_uuid(u), p.s.ent_by_uuid_reference(u), "{what}: UUID {u:x}");
    };
    for e in p.s.all_ents() {
        check(&p, p.s.ent_uuid(e), &format!("{e:?}"));
    }
    check(&p, 0, "UUID 0");
    check(&p, 0x1234_5678, "unknown UUID");
    let dead = p.s.mobs.list.iter().find(|m| !m.alive()).expect("dead mob");
    let dead_uuid = dead.uuid;
    check(&p, dead_uuid, "dead mob");
    assert_eq!(p.s.ent_by_uuid(dead_uuid), None, "a dead mob is not listed");
    // A marker given the UUID of a later zombie: the marker (smaller identifier) comes first.
    let zombie = p.s.mobs.list.iter().filter(|m| m.alive()).last().expect("mob");
    let (zombie_id, zombie_uuid) = (zombie.id, zombie.uuid);
    let marker = p.s.all_ents().into_iter().find(|e| matches!(e, E::Other(_)) && p.s.ent_type(*e) == "minecraft:marker");
    let Some(E::Other(marker_id)) = marker else { panic!("test setup: no marker") };
    assert!(marker_id < zombie_id, "test setup: marker after the zombie");
    let (c, k) = p.s.kept_entity_slot(marker_id).expect("marker slot");
    crate::nbtio::set(&mut p.s.entities.get_mut(&c).expect("chunk")[k].1, "UUID", crate::nbtio::uuid_nbt(zombie_uuid));
    check(&p, zombie_uuid, "mob and kept compound");
    assert_eq!(p.s.ent_by_uuid(zombie_uuid), Some(E::Other(marker_id)));
    // Two living mobs with one UUID: the smaller identifier.
    let other = p.s.mobs.list.iter().find(|m| m.alive() && m.id != zombie_id).map(|m| m.id).expect("mob");
    p.s.mobs.get_mut(other).expect("mob").uuid = dead_uuid;
    p.s.mobs.get_mut(first_alive_after(&p, other)).expect("mob").uuid = dead_uuid;
    check(&p, dead_uuid, "two living mobs");
    p.s.life.vit.dead = true;
    check(&p, p.s.uuid, "dead player");
    p.s.life.vit.dead = false;
    crate::tests_g3f13::finish(p, dir);
}

fn first_alive_after(p: &Probe, id: i32) -> i32 {
    p.s.mobs.list.iter().filter(|m| m.alive() && m.id > id).map(|m| m.id).next().expect("later mob")
}

/// The kept entity index follows kills, moves to another chunk and new compounds; a hint the
/// lists outdated is never trusted.
#[test]
fn kept_entity_index_follows_the_lists() {
    let (mut p, dir) = mixed_world("select-kept-index");
    let markers = |p: &Probe| -> Vec<i32> {
        p.s.all_ents()
            .into_iter()
            .filter_map(|e| match e {
                E::Other(i) if p.s.ent_type(e) == "minecraft:marker" => Some(i),
                _ => None,
            })
            .collect()
    };
    let all = markers(&p);
    let slots = |p: &Probe, ids: &[i32]| -> Vec<Option<((i32, i32), usize)>> {
        ids.iter().map(|&i| p.s.kept_entity_slot(i)).collect()
    };
    let scanned = |p: &Probe, ids: &[i32]| -> Vec<Option<((i32, i32), usize)>> {
        ids.iter().map(|&i| super::kept_entity_index::scan(&p.s.entities, i)).collect()
    };
    assert_eq!(slots(&p, &all), scanned(&p, &all));
    // A kill shifts the list behind the index's back: the next lookups still agree with the scan.
    let (chunk, first_slot) = p.s.kept_entity_slot(all[0]).expect("slot");
    let behind = p.s.entities[&chunk].get(first_slot + 1).map(|(i, _)| *i);
    let killed = crate::nbtio::uuid_text(p.s.ent_uuid(E::Other(all[0])));
    cmd(&mut p, &format!("kill {killed}"));
    assert_eq!(p.s.kept_entity_slot(all[0]), None, "killed marker still found");
    if let Some(b) = behind {
        // The hint built before the kill pointed one slot too far: exactly what the check refuses.
        assert_eq!(p.s.kept_index.raw_hint(b), Some((chunk, first_slot + 1)));
        assert_eq!(p.s.kept_entity_slot(b), Some((chunk, first_slot)));
    }
    assert_eq!(slots(&p, &all), scanned(&p, &all));
    cmd(&mut p, "execute as @e[type=marker,tag=b] run tp @s ~40 ~ ~");
    cmd(&mut p, "summon minecraft:marker 3 -60 3 {Tags:[\"new\"]}");
    let now = markers(&p);
    assert!(now.len() == all.len(), "{} markers, {} before", now.len(), all.len());
    assert_eq!(slots(&p, &now), scanned(&p, &now));
    assert_eq!(slots(&p, &all), scanned(&p, &all));
    // Every marker is still selected with its tags after the moves.
    let src = p.s.server_source();
    let tagged = p.s.select(&parse("@e[type=marker,tag=b]").expect("selector"), &src);
    assert!(!tagged.is_empty(), "moved markers lost");
    crate::tests_g3f13::finish(p, dir);
}

/// The candidate view formats a UUID once per entity for its scores, the former search once per
/// objective and entity.
#[test]
fn scores_format_each_uuid_once() {
    crate::mobs::index::set_verify(false);
    let (mut p, dir) = mixed_world("select-score-format");
    let formatted = || crate::nbtio::UUID_TEXTS_FORMATTED.with(std::cell::Cell::get);
    let sel = parse("@e[type=marker,scores={s=0..,t=..100}]").expect("selector");
    let src = p.s.server_source();
    let count = |p: &mut Probe, paths: CommandPaths| {
        p.s.cmd.paths = paths;
        let before = formatted();
        let found = p.s.select(&sel, &src).len();
        (found, formatted() - before)
    };
    let (found_old, old) = count(&mut p, CommandPaths::OLD);
    let (found_new, new) = count(&mut p, CommandPaths::NEW);
    assert_eq!(found_new, found_old);
    let markers = p.s.all_ents().into_iter().filter(|e| p.s.ent_type(*e) == "minecraft:marker").count() as u64;
    assert!(new <= markers && old > new, "UUID texts formatted: {old} before, {new} now, {markers} markers");
    crate::mobs::index::set_verify(true);
    crate::tests_g3f13::finish(p, dir);
}
