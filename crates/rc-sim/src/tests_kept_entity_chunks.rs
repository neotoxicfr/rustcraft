//! Kept entity compounds (armor stands, markers, displays...) and their chunk: a command that
//! changes one marks its chunk for the next save, and a move into another chunk files it under
//! that chunk (saved there, shown and forgotten with it), as the game does. On a copy of the test
//! world (`RUSTCRAFT_SOLO_WORLD`); absent: tests skipped.
#![cfg(test)]

use std::collections::BTreeMap;

use crate::blocks::probe::Probe;
use crate::commands::ents::TEST_KILL_REWRITES_ONE_CHUNK;
use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

const BASE: &str = "fill 96 -60 96 143 -60 143 minecraft:stone\nfill 96 -59 96 143 -50 143 minecraft:air\n";
/// Chunk (6, 6) holds x, z in 96..112; chunk (7, 7) holds 112..128.
const HOME: (i32, i32) = (6, 6);
const AWAY: (i32, i32) = (7, 7);

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [104.5, -59.0, 104.5];
    p.s.loaded = true;
    assert!(p.s.chunks.contains_key(&HOME) && p.s.chunks.contains_key(&AWAY), "both chunks read");
    p.take_events();
    Some((p, dir))
}

fn cmd(p: &mut Probe, c: &str) {
    let mut src = p.s.player_source();
    src.level = 4;
    assert!(p.s.perform(src, c).0, "command failed: {c}");
}

/// Entities of each entity chunk, by chunk position.
type EntitiesByChunk = BTreeMap<(i32, i32), Vec<Nbt>>;

/// Entities written per entity chunk by a save.
fn saved(p: &mut Probe) -> EntitiesByChunk {
    let mut jobs = Vec::new();
    p.s.drops_save_jobs(&mut jobs);
    jobs.iter()
        .filter_map(|j| match j {
            crate::save::Job::Chunk { cx, cz, nbt, .. } => {
                let root = crate::nbtio::parse_root(nbt).expect("entity chunk NBT");
                Some(((*cx, *cz), crate::nbtio::list(root.get("Entities")).to_vec()))
            }
            _ => None,
        })
        .collect()
}

fn of_type<'a>(ents: &'a [Nbt], ty: &str) -> Vec<&'a Nbt> {
    ents.iter().filter(|e| e.get("id").and_then(Nbt::as_str) == Some(ty)).collect()
}

/// Summons `ty` at `at` (chunk HOME unless said), saves once so no chunk is pending, and
/// checks that a save without change leaves HOME alone (no live entity there).
fn summon_settled(p: &mut Probe, ty: &str, at: &str) -> i32 {
    cmd(p, &format!("summon {ty} {at}"));
    let id = p.s.next_entity;
    saved(p);
    assert!(!saved(p).contains_key(&HOME), "precondition: nothing else rewrites the chunk");
    p.take_events();
    id
}

fn filed_under(p: &Probe, id: i32) -> Vec<(i32, i32)> {
    p.s.entities.iter().filter(|(_, l)| l.iter().any(|e| e.0 == id)).map(|(c, _)| *c).collect()
}

fn tags(e: &Nbt) -> Vec<String> {
    crate::nbtio::list(e.get("Tags")).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect()
}

/// `/tag` and `/data merge` on a kept compound reach the save (its chunk was not rewritten).
#[test]
fn tag_and_data_merge_on_kept_entity_are_saved() {
    let Some((mut p, _dir)) = probe("kept-dirty") else { return };
    summon_settled(&mut p, "minecraft:marker", "100.5 -59 100.5");
    cmd(&mut p, "tag @e[type=minecraft:marker] add kept");
    let out = saved(&mut p);
    let marker = out.get(&HOME).map(|l| of_type(l, "minecraft:marker")).unwrap_or_default();
    assert_eq!(marker.len(), 1, "chunk rewritten after /tag");
    assert_eq!(tags(marker[0]), ["kept"]);
    cmd(&mut p, "data merge entity @e[type=minecraft:marker,limit=1] {data:{answer:42}}");
    let out = saved(&mut p);
    let marker = out.get(&HOME).map(|l| of_type(l, "minecraft:marker")).unwrap_or_default();
    assert_eq!(marker.len(), 1, "chunk rewritten after /data merge");
    let answer = marker[0].get("data").and_then(|d| crate::nbtio::int(d, "answer"));
    assert_eq!(answer, Some(42));
}

/// `/tp` into another chunk: listed and saved under the new chunk, no longer under the old one.
#[test]
fn teleport_files_kept_entity_under_new_chunk() {
    let Some((mut p, _dir)) = probe("kept-tp") else { return };
    let id = summon_settled(&mut p, "minecraft:marker", "100.5 -59 100.5");
    cmd(&mut p, "tp @e[type=minecraft:marker] 120.5 -59 120.5");
    assert_eq!(filed_under(&p, id), [AWAY]);
    let out = saved(&mut p);
    let home = out.get(&HOME).expect("old chunk rewritten");
    assert!(of_type(home, "minecraft:marker").is_empty(), "still saved in the old chunk");
    let away = out.get(&AWAY).map(|l| of_type(l, "minecraft:marker")).unwrap_or_default();
    assert_eq!(away.len(), 1, "saved in the new chunk");
    assert_eq!(crate::nbtio::f64s::<3>(away[0].get("Pos")), Some([120.5, -59.0, 120.5]));
    // A short move inside the chunk keeps it there.
    cmd(&mut p, "tp @e[type=minecraft:marker] 121.5 -59 121.5");
    assert_eq!(filed_under(&p, id), [AWAY]);
}

/// `/data merge` of a new `Pos` files the compound under the chunk of that position.
#[test]
fn data_merge_pos_files_kept_entity_under_new_chunk() {
    let Some((mut p, _dir)) = probe("kept-datapos") else { return };
    let id = summon_settled(&mut p, "minecraft:marker", "100.5 -59 100.5");
    cmd(&mut p, "data merge entity @e[type=minecraft:marker,limit=1] {Pos:[120.5d,-59.0d,120.5d]}");
    assert_eq!(filed_under(&p, id), [AWAY]);
    let out = saved(&mut p);
    assert!(out.get(&HOME).is_some_and(|l| of_type(l, "minecraft:marker").is_empty()));
    assert_eq!(out.get(&AWAY).map(|l| of_type(l, "minecraft:marker").len()), Some(1));
}

/// A move between the chunks the client was sent and the others: removed when it leaves them,
/// announced when it enters them, a plain teleport inside them.
#[test]
fn teleport_across_view_border_removes_or_announces() {
    let Some((mut p, _dir)) = probe("kept-view") else { return };
    p.s.sent = [HOME].into_iter().collect();
    let id = summon_settled(&mut p, "minecraft:block_display", "100.5 -59 100.5");
    let teleported = |ev: &[Event]| ev.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Teleport(t)) if t.id == id));
    let added = |ev: &[Event]| ev.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Add(a)) if a.id == id));
    let removed =
        |ev: &[Event]| ev.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Remove(ids)) if ids.contains(&id)));
    cmd(&mut p, "tp @e[type=minecraft:block_display] 120.5 -59 120.5");
    let ev = p.take_events();
    assert!(removed(&ev) && !teleported(&ev), "left the view: removed, {ev:?}");
    cmd(&mut p, "tp @e[type=minecraft:block_display] 100.5 -59 100.5");
    let ev = p.take_events();
    assert!(added(&ev) && !teleported(&ev) && !removed(&ev), "entered the view: announced, {ev:?}");
    p.s.sent.insert(AWAY);
    cmd(&mut p, "tp @e[type=minecraft:block_display] 120.5 -59 120.5");
    let ev = p.take_events();
    assert!(teleported(&ev) && !added(&ev) && !removed(&ev), "inside the view: teleported, {ev:?}");
}

/// Text sent for `id` in the entity data of `ev` (last one).
fn sent_text(ev: &[Event], id: i32) -> Option<Nbt> {
    ev.iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if *i == id => Some(values),
            _ => None,
        })
        .flatten()
        .filter_map(|(k, v)| match v {
            rc_proto_v777::entity::Meta::Component(t) if *k == 23 => Some(t.clone()),
            _ => None,
        })
        .next_back()
}

/// `/data merge` that brings a text display into the view: announced with its text resolved
/// (the game reads the whole compound, text included, before showing the entity).
#[test]
fn data_merge_into_view_announces_resolved_text() {
    let Some((mut p, _dir)) = probe("kept-text-view") else { return };
    p.s.sent = [HOME].into_iter().collect();
    let id = summon_settled(&mut p, "minecraft:text_display", "120.5 -59 120.5");
    cmd(&mut p, "data merge entity @e[type=minecraft:text_display,limit=1] {Pos:[100.5d,-59.0d,100.5d],text:{selector:\"@s\"}}");
    let ev = p.take_events();
    assert!(ev.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Add(a)) if a.id == id)), "announced, {ev:?}");
    let kept = p.s.entities[&HOME].iter().find(|e| e.0 == id).and_then(|e| e.1.get("text").cloned());
    assert!(kept.as_ref().is_some_and(|t| t.get("selector").is_none()), "text resolved in the compound: {kept:?}");
    assert_eq!(sent_text(&ev, id), kept, "the client was given another text than the resolved one");
}

/// Position the client holds for `id` after `ev`: announce, then relative moves, syncs, teleports.
fn client_position(ev: &[Event], id: i32, mut at: Option<[f64; 3]>) -> Option<[f64; 3]> {
    use rc_proto_v777::entity::{Delta, apply_delta};
    for e in ev {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.id == id => at = Some(a.pos),
            Event::Entity(EntityEvent::Move(m)) if m.id == id => {
                if let (Some(base), Some(Delta::Linear(d))) = (at, &m.delta) {
                    at = Some(apply_delta(base, *d));
                }
            }
            Event::Entity(EntityEvent::Sync(s)) if s.id == id => at = s.path.steps.last().map(|(p, _)| *p),
            Event::Entity(EntityEvent::Teleport(t)) if t.id == id => at = Some(t.pos),
            _ => {}
        }
    }
    at
}

/// A tracked armor stand pushed into the view is announced where its tracking last sent it, so
/// the next relative move (coded from there) lands it where it is on the server.
#[test]
fn stand_pushed_into_view_lands_where_it_is() {
    let Some((mut p, _dir)) = probe("kept-stand-view") else { return };
    p.s.sent = [HOME].into_iter().collect();
    let id = summon_settled(&mut p, "minecraft:armor_stand", "112.2 -59 100.5 {NoGravity:1b}");
    let tick = |p: &mut Probe| {
        p.s.time += 1;
        p.s.deco_tick();
    };
    (0..4).for_each(|_| tick(&mut p));
    p.take_events();
    p.s.deco_piston_move(id, [-0.5, 0.0, 0.0]);
    assert_eq!(filed_under(&p, id), [HOME]);
    let mut ev = p.take_events();
    (0..6).for_each(|_| tick(&mut p));
    ev.extend(p.take_events());
    let at = client_position(&ev, id, None).expect("announced");
    let server = [111.7, -59.0, 100.5];
    assert!(at.iter().zip(server).all(|(c, s)| (c - s).abs() < 1.0 / 4096.0), "client at {at:?}, server at {server:?}");
}

/// A weightless armor stand pushed by a piston across a chunk border is filed there (the
/// decorations' own edits go through the same refile).
#[test]
fn pushed_armor_stand_files_under_new_chunk() {
    let Some((mut p, _dir)) = probe("kept-push") else { return };
    let id = summon_settled(&mut p, "minecraft:armor_stand", "111.9 -59 100.5 {NoGravity:1b}");
    p.s.deco_piston_move(id, [0.5, 0.0, 0.0]);
    assert_eq!(filed_under(&p, id), [(7, 6)]);
    let out = saved(&mut p);
    assert!(out.get(&HOME).is_some_and(|l| of_type(l, "minecraft:armor_stand").is_empty()));
    assert_eq!(out.get(&(7, 6)).map(|l| of_type(l, "minecraft:armor_stand").len()), Some(1));
}

/// Chunk (5, 5) holds x, z in 80..96: a freshly generated chunk in these tests.
const GENERATED: (i32, i32) = (5, 5);

/// Item frame compound as a structure leaves it with a freshly generated chunk (End ship).
fn generated_frame() -> Nbt {
    Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:item_frame".into())),
        ("UUID".into(), Nbt::IntArray(vec![9, 10, 11, 12])),
        ("Pos".into(), Nbt::List(vec![Nbt::Double(88.5), Nbt::Double(-59.0 + 0.03125), Nbt::Double(88.5)])),
        ("Facing".into(), Nbt::Byte(1)),
        ("block_pos".into(), Nbt::IntArray(vec![88, -59, 88])),
    ])
}

/// Structure entities of a freshly generated chunk exist nowhere on disk: the next save writes
/// them even when nothing else touches that chunk.
#[test]
fn generated_structure_entity_is_saved() {
    let Some((mut p, _dir)) = probe("kept-generated") else { return };
    saved(&mut p);
    assert!(!saved(&mut p).contains_key(&GENERATED), "precondition: nothing else rewrites the chunk");
    p.s.gen_arrived(vec![(GENERATED, vec![generated_frame()])], Vec::new());
    let out = saved(&mut p);
    let frames = out.get(&GENERATED).map(|l| of_type(l, "minecraft:item_frame").len());
    assert_eq!(frames, Some(1), "generated frame never written");
}

/// Entities on disk after a full save, the arrival of a generated chunk with a structure entity,
/// then `kill` of a kept compound, with `kill` marking only its chunk (`one_chunk`) or every
/// entity chunk; also the number of chunks the last save wrote.
fn disk_after_kill(one_chunk: bool) -> Option<(EntitiesByChunk, usize)> {
    let (mut p, _dir) = probe(if one_chunk { "kept-kill-one" } else { "kept-kill-all" })?;
    TEST_KILL_REWRITES_ONE_CHUNK.with(|c| c.set(Some(one_chunk)));
    cmd(&mut p, "summon minecraft:marker 100.5 -59 100.5 {UUID:[I;1,2,3,4],Tags:[\"doomed\"]}");
    cmd(&mut p, "summon minecraft:marker 120.5 -59 120.5 {UUID:[I;5,6,7,8]}");
    // Every chunk read from the world written once: `disk` starts as the full world contents.
    p.s.life.drop_chunks.extend(p.s.entities.keys().copied().collect::<Vec<_>>());
    let mut disk = saved(&mut p);
    // Arrives after that save: only its own marking can bring it to disk.
    p.s.gen_arrived(vec![(GENERATED, vec![generated_frame()])], Vec::new());
    cmd(&mut p, "kill @e[type=minecraft:marker,tag=doomed]");
    let after = saved(&mut p);
    let written = after.len();
    disk.extend(after);
    TEST_KILL_REWRITES_ONE_CHUNK.with(|c| c.set(None));
    Some((disk, written))
}

/// Equivalence of the two `kill` paths: the same entities on disk, the doomed one gone from its
/// chunk; the new path rewrites fewer chunks.
#[test]
fn kill_marks_only_its_chunk_same_disk_contents() {
    let Some((one, written_one)) = disk_after_kill(true) else { return };
    let (all, written_all) = disk_after_kill(false).expect("second world");
    assert_eq!(one, all, "entities on disk differ between the two kill paths");
    let doomed = |l: &Vec<Nbt>| l.iter().any(|e| tags(e) == ["doomed"]);
    assert!(!one.values().any(doomed), "killed marker still saved");
    assert_eq!(one.get(&AWAY).map(|l| of_type(l, "minecraft:marker").len()), Some(1));
    assert_eq!(one.get(&GENERATED).map(|l| of_type(l, "minecraft:item_frame").len()), Some(1));
    assert!(written_one < written_all, "one chunk path wrote {written_one}, all chunks path {written_all}");
    eprintln!("kill then save: {written_one} entity chunks written (all chunks path: {written_all})");
}

/// Micro-bench (ignored, `--ignored --nocapture`): a decoration edit (weightless armor stand
/// pushed back and forth inside its chunk) against the chunk check the edit now adds (its
/// position read back and compared with the chunk it is filed under).
#[test]
#[ignore]
fn bench_decoration_edit_chunk_check() {
    let Some((mut p, _dir)) = probe("kept-bench") else { return };
    let id = summon_settled(&mut p, "minecraft:armor_stand", "100.5 -59 100.5 {NoGravity:1b}");
    let n = 200_000u32;
    for _ in 0..2 {
        let t = std::time::Instant::now();
        for k in 0..n {
            let dx = if k % 2 == 0 { 0.25 } else { -0.25 };
            p.s.deco_piston_move(id, std::hint::black_box([dx, 0.0, 0.0]));
        }
        let edit = t.elapsed().as_nanos() as f64 / f64::from(n);
        let e = p.s.entities[&HOME].iter().find(|e| e.0 == id).map(|e| e.1.clone()).expect("stand");
        let t = std::time::Instant::now();
        let mut moved = 0u32;
        for _ in 0..n {
            let pos = crate::nbtio::f64s::<3>(std::hint::black_box(&e).get("Pos")).unwrap_or_default();
            moved += u32::from(crate::decoration::chunk_of(pos) != HOME);
        }
        let check = t.elapsed().as_nanos() as f64 / f64::from(n);
        eprintln!("decoration edit {edit:.0} ns, of which chunk check {check:.1} ns ({moved} moves)");
    }
    assert_eq!(filed_under(&p, id), [HOME]);
}
