//! Work the End skips or shares must give what the full work gives: reused crystal compounds
//! against a rebuild, the arena ticket memo against the chunk map, the shared dragon contact walk
//! against a walk per box (that last comparison is asserted inside `dragon_box_targets`).

use rc_proto::nbt::Nbt;

use super::super::jrand::JRand;
use super::super::species::Kind;
use super::crystal::{Crystal, rebuild_snapshots, refresh_snapshots};
use super::dragon::Dragon;

fn debug_text<T: std::fmt::Debug>(v: &T) -> String {
    format!("{v:?}")
}

/// A crystal read from a compound with extra tags, which every save must carry along.
fn loaded_crystal(id: i32) -> Crystal {
    let e = Nbt::Compound(vec![
        ("Pos".into(), Nbt::List(vec![Nbt::Double(1.5), Nbt::Double(64.0), Nbt::Double(-3.5)])),
        ("Tags".into(), Nbt::List(vec![Nbt::String("pillar".into())])),
        ("Fire".into(), Nbt::Short(-1)),
    ]);
    Crystal::load(id, &e)
}

/// One random change of the kind the game makes between two snapshots.
fn mutate(list: &mut Vec<Crystal>, r: &mut JRand, next_id: &mut i32) {
    let n = list.len() as i32;
    let pick = if n > 0 { r.next_int_bound(n) as usize } else { 0 };
    match r.next_int_bound(11) {
        0 => {
            *next_id += 1;
            list.push(Crystal::new(*next_id, u128::from(*next_id as u32), [0.5, 70.0, 0.5], 0.0));
        }
        1 => {
            *next_id += 1;
            list.push(loaded_crystal(*next_id));
        }
        2 if n > 0 => list[pick].set_beam(Some([r.next_int_bound(9), 128, r.next_int_bound(9)])),
        3 if n > 0 => list[pick].set_beam(None),
        4 if n > 0 => list[pick].vel[r.next_int_bound(3) as usize] += r.next_double() - 0.5,
        5 if n > 0 => list[pick].invulnerable ^= true,
        6 if n > 0 => list[pick].show_bottom ^= true,
        7 if n > 0 => list[pick].removed = true,
        // Signed zero and NaN: saved doubles that a plain float comparison would call equal or
        // never equal.
        8 if n > 0 => list[pick].pos[0] = if r.next_int_bound(2) == 0 { -0.0 } else { 0.0 },
        9 if n > 0 => list[pick].vel[1] = f64::NAN,
        // The entity walk re-sorts the list by rank.
        10 => list.reverse(),
        _ => {}
    }
}

#[test]
fn reused_crystal_compounds_match_a_full_rebuild() {
    let mut r = JRand::new(31);
    let mut next_id = 100;
    let mut list: Vec<Crystal> =
        (0..10).map(|i| Crystal::new(i, 7 + i as u128, [f64::from(i), 70.0, 0.0], 0.0)).collect();
    let (mut snapshots, mut fields) = (Vec::new(), Vec::new());
    for step in 0..4000 {
        for _ in 0..r.next_int_bound(3) {
            mutate(&mut list, &mut r, &mut next_id);
        }
        refresh_snapshots(&list, &mut snapshots, &mut fields);
        assert_eq!(debug_text(&snapshots), debug_text(&rebuild_snapshots(&list)), "step {step}");
        if list.len() > 40 {
            list.retain(|c| !c.removed);
            list.truncate(20);
        }
    }
}

#[test]
fn unchanged_crystals_rebuild_nothing() {
    let list: Vec<Crystal> = (0..10).map(|i| Crystal::new(i, 7 + i as u128, [f64::from(i), 70.0, 0.0], 0.0)).collect();
    let (mut snapshots, mut fields) = (Vec::new(), Vec::new());
    assert_eq!(refresh_snapshots(&list, &mut snapshots, &mut fields), 10);
    assert_eq!(refresh_snapshots(&list, &mut snapshots, &mut fields), 0, "unchanged End rebuilt compounds");
    let mut moved = list.clone();
    moved[3].set_beam(Some([0, 128, 0]));
    assert_eq!(refresh_snapshots(&moved, &mut snapshots, &mut fields), 1);
}

/// Crystals in a world, changed at random ticks: after every tick, the compound a command reads
/// is the crystal's save, for every living crystal and no other.
#[test]
fn command_view_of_crystals_follows_random_changes() {
    let (mut p, dir) = crate::tests_g3f13::open("end-crystal-snapshots");
    let mut r = JRand::new(5);
    for i in 0..6 {
        p.s.spawn_crystal([f64::from(i * 3) + 0.5, -59.0, 4.5], 0.0, true);
    }
    for tick in 0..300 {
        let living: Vec<i32> = p.s.mobs.end.crystals.iter().filter(|c| !c.removed).map(|c| c.id).collect();
        let pick = |r: &mut JRand| living.get(r.next_int_bound(living.len().max(1) as i32) as usize).copied();
        match r.next_int_bound(8) {
            0 => {
                p.s.spawn_crystal([f64::from(r.next_int_bound(20)) + 0.5, -59.0, 8.5], 0.0, r.next_int_bound(2) == 0);
            }
            1 => {
                if let Some(id) = pick(&mut r) {
                    p.s.end_kill_other(id);
                }
            }
            2 => {
                if let Some(c) = pick(&mut r).and_then(|id| p.s.mobs.end.crystals.iter_mut().find(|c| c.id == id)) {
                    c.set_beam(Some([0, -50, 0]));
                }
            }
            3 => p.s.crystals_blast_push([5.0, -59.0, 4.0], 3.0, 1.0),
            _ => {}
        }
        p.s.mobs_tick();
        let mut expected: Vec<(i32, String)> = Vec::new();
        for c in p.s.mobs.end.crystals.iter().filter(|c| !c.removed) {
            expected.push((c.id, debug_text(&c.save())));
        }
        let seen: Vec<(i32, String)> = p.s.mobs.end.snapshots.iter().map(|(id, n)| (*id, debug_text(n))).collect();
        assert_eq!(seen, expected, "tick {tick}");
        for (id, nbt) in &expected {
            assert_eq!(p.s.end_other(*id).map(debug_text).as_ref(), Some(nbt), "tick {tick}, crystal {id}");
        }
    }
    crate::tests_g3f13::finish(p, dir);
}

/// The arena ticket memo survives chunk insertions and is dropped when the chunk map is swapped:
/// a ticket chunk that left the map is loaded again on the next fight tick.
#[test]
fn arena_ticket_memo_follows_the_chunk_map() {
    let (mut p, dir) = crate::tests_g3f13::open("end-arena-memo");
    let mut f = super::fight::Fight::read(None);
    // The fight zone is centred 128 above the origin: the player (y -60) stays in it.
    f.origin = [0, -188, 0];
    f.bar_player = true;
    p.s.mobs.end.fight = Some(Box::new(f));
    p.s.mobs.end.fight_checked = true;
    p.s.fight_tick();
    assert!(p.s.chunks.contains_key(&(9, -9)), "ticket chunk not loaded");
    p.s.fight_tick();
    assert_eq!(p.s.mobs.end.arena_ticket_present_at, Some(p.s.chunk_map_swaps), "memo not taken");
    // A chunk entering the map (the player flying toward new chunks) keeps the memo.
    let swaps = p.s.chunk_map_swaps;
    p.s.ensure_loaded(&[(30, 30)]);
    assert!(p.s.chunks.contains_key(&(30, 30)), "far chunk not loaded");
    assert_eq!(p.s.chunk_map_swaps, swaps, "a chunk insertion dropped the arena memo");
    // A chunk leaving the map always comes with a swap of the map.
    p.s.chunks.remove(&(9, -9));
    p.s.chunk_map_swaps += 1;
    p.s.fight_tick();
    assert!(p.s.chunks.contains_key(&(9, -9)), "stale memo: missing ticket chunk not reloaded");
    crate::tests_g3f13::finish(p, dir);
}

/// A named dragon renames the fight bar once, then keeps it without new packets; a new name
/// renames it again.
#[test]
fn named_dragon_renames_the_fight_bar_once() {
    let (mut p, dir) = crate::tests_g3f13::open("end-dragon-name");
    let mut m = p.s.new_mob(Kind::EnderDragon, [0.5, -50.0, 0.5], 0.0);
    let mut f = super::fight::Fight::read(None);
    f.dragon_uuid = Some(m.uuid);
    (f.bar_player, f.bar_visible) = (true, true);
    p.s.mobs.end.fight = Some(Box::new(f));
    let renames = |p: &crate::blocks::probe::Probe| {
        p.take_events()
            .into_iter()
            .filter_map(|e| match e {
                rc_client::Event::Boss(b) => match b.op {
                    rc_proto_v777::play::BossOp::Name(n) => n,
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<Nbt>>()
    };
    let first = Nbt::String("Smaug".into());
    m.custom_name = Some(first.clone());
    let _ = renames(&p);
    for _ in 0..3 {
        p.s.fight_update_dragon(&m);
    }
    assert_eq!(renames(&p), vec![first.clone()], "bar renamed once");
    assert_eq!(p.s.mobs.end.fight.as_ref().and_then(|f| f.bar_name.clone()), Some(first));
    let second = Nbt::String("Glaurung".into());
    m.custom_name = Some(second.clone());
    p.s.fight_update_dragon(&m);
    assert_eq!(renames(&p), vec![second], "new name not sent");
    crate::tests_g3f13::finish(p, dir);
}

/// Dragon parts laid out around `at` as a level flight facing north puts them.
fn dragon_at(p: &mut crate::blocks::probe::Probe, at: [f64; 3]) -> (super::super::mob::Mob, Box<Dragon>) {
    let mut m = p.s.new_mob(Kind::EnderDragon, at, 0.0);
    let mut d = m.end.dragon.take().expect("dragon state");
    let offsets = [
        [0.0, 0.0, -6.5],
        [0.0, 0.0, -5.5],
        [0.0, 0.0, -0.5],
        [0.0, 0.0, 2.0],
        [0.0, 0.0, 4.0],
        [0.0, 0.0, 6.0],
        [4.5, 2.0, 0.0],
        [-4.5, 2.0, 0.0],
    ];
    for (part, o) in d.parts.iter_mut().zip(offsets) {
        *part = [at[0] + o[0], at[1] + o[1], at[2] + o[2]];
    }
    (m, d)
}

fn zombie(p: &mut crate::blocks::probe::Probe, at: [f64; 3], health: f32) -> i32 {
    let mut z = p.s.new_mob(Kind::Zombie, at, 0.0);
    z.persistent = true;
    z.health = health;
    let id = z.id;
    p.s.add_mob(z);
    id
}

/// A zombie in front of the head (outside both wing boxes) is found by the shared walk and
/// bitten; a dying one, killed by the head, must no longer be seen by the neck. The walk of each
/// box is compared with its own walk inside `dragon_box_targets`.
#[test]
fn shared_dragon_contact_walk_matches_a_walk_per_box() {
    let (mut p, dir) = crate::tests_g3f13::open("end-dragon-contacts");
    let at = [24.5, -59.0, 24.5];
    let (mut m, mut d) = dragon_at(&mut p, at);
    let head = [at[0], at[1], at[2] - 7.5];
    let bitten = zombie(&mut p, head, 20.0);
    let state = debug_text(&d);
    p.s.dragon_contacts(&mut m, &mut d);
    let health = p.s.mob_ref(bitten).expect("bitten zombie").health;
    assert!(health < 15.0, "head bite missed the zombie in front of it ({health})");
    // The state lent back to the mob for the after-hit effects came back whole.
    assert_eq!(debug_text(&d), state, "dragon state lost while lent");

    let at = [-24.5, -59.0, 24.5];
    let (mut m, mut d) = dragon_at(&mut p, at);
    let dying = zombie(&mut p, [at[0], at[1], at[2] - 7.0], 1.0);
    let pushed = zombie(&mut p, [at[0] + 6.0, at[1], at[2]], 20.0);
    p.s.dragon_contacts(&mut m, &mut d);
    assert!(p.s.mob_ref(dying).is_none_or(|z| !z.alive()), "head bite did not kill the dying zombie");
    let v = p.s.mob_ref(pushed).expect("pushed zombie").vel;
    assert!(v[0] > 0.0, "wing did not push the zombie beside it ({v:?})");
    crate::tests_g3f13::finish(p, dir);
}

/// A dragon state with lists in it: flight graph and a hold path.
fn dragon_with_lists(p: &mut crate::blocks::probe::Probe, at: [f64; 3]) -> (super::super::mob::Mob, Box<Dragon>) {
    let (m, mut d) = dragon_at(p, at);
    d.graph.build(|_, _| 64);
    d.ph.hold_path = Some(super::graph::Path { nodes: vec![[0, 64, 0]; 8], next: 0 });
    (m, d)
}

/// Both ways of lending the state to the mob for a thorns hit hand the mob the whole state and
/// get it back whole, whatever the switch says in this process.
#[test]
fn dragon_state_lent_both_ways_comes_back_whole() {
    let (mut p, dir) = crate::tests_g3f13::open("end-dragon-lend");
    for move_out in [false, true] {
        let (mut m, mut d) = dragon_with_lists(&mut p, [24.5, -59.0, 24.5]);
        let state = debug_text(&d);
        p.s.lend_dragon_state(&mut m, &mut d, move_out, |s, m| {
            let lent = m.end.dragon.as_ref().map(debug_text);
            assert_eq!(lent.as_ref(), Some(&state), "mob not lent the whole state (move_out {move_out})");
            s.player_thorns(m, "minecraft:mob_attack");
        });
        assert_eq!(debug_text(&d), state, "dragon state lost while lent (move_out {move_out})");
        assert!(m.end.dragon.is_none(), "state left in the mob after the lend (move_out {move_out})");
    }
    crate::tests_g3f13::finish(p, dir);
}

// ---------- Micro-benches (one core: this work is serial within a tick) ----------

/// Nanoseconds per call of `f`, best of 5 rounds of `n` calls.
fn ns_per_call(n: u32, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        for _ in 0..n {
            f();
        }
        best = best.min(t.elapsed().as_nanos() as f64 / f64::from(n));
    }
    best
}

#[test]
#[ignore = "measurement bench"]
fn bench_end_crystal_snapshots() {
    // Ten pillar crystals, as on the main island.
    let list: Vec<Crystal> = (0..10).map(loaded_crystal).collect();
    let (mut snapshots, mut fields) = (Vec::new(), Vec::new());
    let rebuild = ns_per_call(20_000, || {
        std::hint::black_box(rebuild_snapshots(std::hint::black_box(&list)));
    });
    let reuse = ns_per_call(20_000, || {
        std::hint::black_box(refresh_snapshots(std::hint::black_box(&list), &mut snapshots, &mut fields));
    });
    eprintln!("bench_end_crystal_snapshots 10 crystals: rebuild {rebuild:.0} ns, reuse {reuse:.0} ns per tick");
}

#[test]
#[ignore = "measurement bench"]
fn bench_end_dragon_state_lend() {
    // The real lend, both ways, around an empty effect: each pays the same Box and the same move
    // back (which frees the old lists after a copy).
    let (mut p, dir) = crate::tests_g3f13::open("end-bench-lend");
    let (mut m, mut d) = dragon_with_lists(&mut p, [24.5, -59.0, 24.5]);
    let mut lend = |move_out: bool| {
        ns_per_call(200_000, || {
            p.s.lend_dragon_state(&mut m, &mut d, move_out, |_, m| {
                std::hint::black_box(m);
            });
        })
    };
    // Alternating passes, best of each: the order of the two ways does not favour one.
    let (mut clone, mut take) = (f64::MAX, f64::MAX);
    for _ in 0..3 {
        clone = clone.min(lend(false));
        take = take.min(lend(true));
    }
    crate::tests_g3f13::finish(p, dir);
    let size = std::mem::size_of::<Dragon>();
    eprintln!(
        "bench_end_dragon_state_lend ({size} bytes): deep copy {clone:.0} ns, move out and back {take:.0} ns per lend"
    );
}

#[test]
#[ignore = "measurement bench"]
fn bench_end_dragon_contact_walk() {
    // Without the test-only check of the species box bounds, as in a release run.
    crate::mobs::index::set_verify(false);
    let (mut p, dir) = crate::tests_g3f13::open("end-bench-contacts");
    for i in 0..300 {
        zombie(&mut p, [f64::from(i % 20) * 3.0 - 30.0, -59.0, f64::from(i / 20) * 3.0 - 60.0], 20.0);
    }
    let (m, d) = dragon_at(&mut p, [0.5, -50.0, 40.5]);
    let boxes = [
        super::dragon::wing_contact_box(&d, 6),
        super::dragon::wing_contact_box(&d, 7),
        super::dragon::bite_contact_box(&d, 0),
        super::dragon::bite_contact_box(&d, 1),
    ];
    let per_box = ns_per_call(20_000, || {
        for b in &boxes {
            std::hint::black_box(p.s.mobs_touching_dragon_box(&m, b));
        }
    });
    let union = super::dragon::contact_boxes_union(&d);
    let shared = ns_per_call(20_000, || {
        let list = p.s.mobs_touching_dragon_box(&m, &union);
        for b in &boxes {
            let ids: Vec<i32> = list.iter().filter(|(_, x)| x.intersects(b)).map(|(id, _)| *id).collect();
            std::hint::black_box(ids);
        }
    });
    eprintln!(
        "bench_end_dragon_contact_walk 300 mobs: four walks {per_box:.0} ns, one shared walk {shared:.0} ns per tick"
    );
    crate::tests_g3f13::finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_end_arena_ticket_and_empty_tracking() {
    let (mut p, dir) = crate::tests_g3f13::open("end-bench-arena");
    let r = 9;
    let arena: Vec<(i32, i32)> = (-r..=r).flat_map(|x| (-r..=r).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&arena);
    let lookups = ns_per_call(20_000, || {
        std::hint::black_box(p.s.arena_ticket_present());
    });
    let kind = ns_per_call(200_000, || {
        std::hint::black_box(p.s.entity_kind_id(std::hint::black_box("minecraft:end_crystal")));
    });
    eprintln!(
        "bench_end_arena_ticket: 361 lookups {lookups:.0} ns per tick (memo: one comparison); \
         crystal kind lookup skipped by an empty tracking {kind:.0} ns per tick"
    );
    crate::tests_g3f13::finish(p, dir);
}
