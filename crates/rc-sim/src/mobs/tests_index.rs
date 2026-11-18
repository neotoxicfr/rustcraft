//! Equivalence proofs of the mob index, of the one-pass dimensions and of the box prefilter
//! (C6-M07), without game data; micro-benches ignored.
//!
//! Indexed searches compare themselves with the scan during ALL the tests of the
//! package (`index::find`, `Mob::may_touch` under `cfg(test)`); here we push them into the edge
//! cases: direct additions, empty slots, replacements, removals, duplicates, portal.

use super::Mobs;
use super::index::{scan_id, scan_uuid};
use super::jrand::JRand;
use super::mob::Mob;
use super::species::{Kind, reach};

/// Small deterministic random of the tests.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Mob of random species and state (babies, poses, sizes, openings).
fn random_mob(r: &mut Rng, id: i32, side: f64) -> Mob {
    let k = Kind::ALL[r.below(Kind::ALL.len() as u64) as usize];
    let pos = [r.unit() * side, 64.0 + r.unit() * 3.0, r.unit() * side];
    let mut m = Mob::new(k, id, 0x1000 + id as u128, pos, 0.0, JRand::new(id as i64));
    if r.below(3) == 0 {
        m.baby = true;
        m.age = -24000;
    }
    m.cube.size = 1 + r.below(6) as i32;
    m.x.size = r.below(12) as i32;
    m.sea.puff = r.below(3) as i32;
    m.variant = r.below(3) as i32;
    m.mount.camel_sitting_pose = r.below(2) == 0;
    m.bm.pose = r.below(20) as i32;
    m.bm.sniffer = r.below(8) as u8;
    m.end.attach = r.below(6) as i32;
    m.end.peek_amount = (r.below(5) as f32) * 0.25;
    if r.below(10) == 0 {
        m.health = 0.0;
    }
    m
}

/// Slots of the dead mobs emptied, as the end of the mob tick does before compacting.
fn take_out_the_dead(mobs: &mut Mobs) {
    for i in 0..mobs.list.len() {
        if mobs.list.get(i).is_some_and(|m| m.health <= 0.0) {
            mobs.list.take(i);
        }
    }
}

fn world(r: &mut Rng, n: usize, side: f64) -> Vec<Option<Box<Mob>>> {
    (0..n).map(|i| Some(Box::new(random_mob(r, i as i32 * 3 + 7, side)))).collect()
}

/// Index: all the list mutations seen in the repository, reads compared with the scan.
#[test]
fn index_equals_scan() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..200 {
        let mut mobs = Mobs::default();
        let mut next = 1;
        let mut taken: Vec<(usize, Box<Mob>)> = Vec::new();
        for _ in 0..(50 + r.below(250)) {
            let m = random_mob(&mut r, next, 32.0);
            next += 1 + r.below(3) as i32;
            mobs.push(Box::new(m));
        }
        mobs.index.rebuild(mobs.list.slots());
        for _ in 0..400 {
            let n = mobs.list.len();
            match r.below(10) {
                // Addition by `Mobs::push` or directly in the list (index not warned).
                0 => mobs.push(Box::new(random_mob(&mut r, next, 32.0))),
                1 => mobs.list.push_unindexed(Some(Box::new(random_mob(&mut r, next, 32.0)))),
                // Taken out for the duration of a tick, then put back in the same slot.
                2 if n > 0 => {
                    let i = r.below(n as u64) as usize;
                    if let Some(m) = mobs.list.take(i) {
                        taken.push((i, m));
                    }
                }
                3 => {
                    if let Some((i, m)) = taken.pop()
                        && mobs.list.get(i).is_none()
                    {
                        mobs.list.put(i, m);
                    }
                }
                // Slot replaced by another mob (conversion).
                4 if n > 0 => {
                    let i = r.below(n as u64) as usize;
                    if mobs.list.get(i).is_some() {
                        mobs.list.put(i, Box::new(random_mob(&mut r, next, 32.0)));
                    }
                }
                // Removal in the middle (portal to another dimension).
                5 if n > 0 => {
                    taken.clear();
                    mobs.remove_slot(r.below(n as u64) as usize);
                }
                // End of tick: removals then index brought up to date.
                6 => {
                    taken.clear();
                    take_out_the_dead(&mut mobs);
                    mobs.drop_empty_slots();
                }
                // Duplicate: same identifier and same UUID added twice.
                7 if n > 0 && round % 4 == 0 => {
                    if let Some(m) = mobs.list.get(r.below(n as u64) as usize) {
                        let c = Mob::new(m.kind, m.id, m.uuid, m.pos(), 0.0, JRand::new(1));
                        mobs.push(Box::new(c));
                    }
                }
                _ => {}
            }
            next += 1;
            // Reads: identifiers present, taken out, removed, never seen.
            for _ in 0..8 {
                let id = r.below(next as u64 + 5) as i32;
                assert_eq!(mobs.index_of(id), scan_id(mobs.list.slots(), id), "round {round} id {id}");
                let u = 0x1000 + r.below(next as u64 + 5) as u128;
                assert_eq!(mobs.index_of_uuid(u), scan_uuid(mobs.list.slots(), u), "round {round} uuid {u:x}");
                let want = mobs.list.iter().find(|o| o.uuid == u && o.health > 0.0).map(|o| o.id);
                assert_eq!(mobs.find_uuid_where(u, |o| o.health > 0.0).map(|o| o.id), want);
            }
        }
    }
}

/// Sabotage of the index: a wrong hint that the read did not check would be seen.
#[test]
fn index_sabotage_vu() {
    if !super::index::on() {
        return; // RUSTCRAFT_OPT_MOBINDEX=0: no hint to sabotage
    }
    let mut r = Rng(42);
    let mut mobs = Mobs::default();
    for i in 0..20 {
        mobs.push(Box::new(random_mob(&mut r, i, 8.0)));
    }
    // Slot 3 replaced by a mob of another identifier, without warning the index.
    let old = mobs.list.get(3).map(|m| m.id).expect("mob");
    mobs.list.put(3, Box::new(random_mob(&mut r, 999, 8.0)));
    // Checked read: the old identifier is absent, the new one found by scan.
    assert_eq!(mobs.index_of(old), None);
    assert_eq!(mobs.index_of(999), Some(3));
    // The raw (unchecked) hint still designated slot 3 for the old one: exactly the
    // error that the check avoids.
    assert_eq!(mobs.index.raw_hint(old), Some(3));
}

fn kind_scan(mobs: &Mobs, k: Kind) -> Vec<i32> {
    mobs.list.iter().filter(|o| o.kind == k).map(|o| o.id).collect()
}

fn kinds_scan(mobs: &Mobs, ks: &[Kind]) -> Vec<i32> {
    mobs.list.iter().filter(|o| ks.contains(&o.kind)).map(|o| o.id).collect()
}

/// World of `n` mobs of five species interleaved (the per-species lists cross each other).
fn mixed_world(r: &mut Rng, n: usize) -> Mobs {
    let kinds = [Kind::Cow, Kind::Zombie, Kind::Husk, Kind::Bee, Kind::Panda];
    let mut mobs = Mobs::default();
    for i in 0..n {
        let mut m = random_mob(r, i as i32 * 2 + 1, 32.0);
        m.kind = kinds[r.below(kinds.len() as u64) as usize];
        mobs.push(Box::new(m));
    }
    mobs
}

/// Mass removals (TNT in a crowd, `/kill` of a part, first or last mob, a block, everything):
/// after the tail re-index, the index has exactly the entries of an index rebuilt from scratch.
#[test]
fn tail_reindex_equals_rebuild() {
    let mut r = Rng(0x7A11);
    let n = 400;
    type Removed<'a> = &'a dyn Fn(usize, &mut Rng) -> bool;
    // Tails shorter than half the list go through the tail re-index, the others through a rebuild.
    let patterns: [(&str, Removed); 8] = [
        ("half", &|i, _| i % 2 == 0),
        ("first", &|i, _| i == 0),
        ("last ten", &|i, _| i >= n - 10),
        ("early block", &|i, _| (150..230).contains(&i)),
        ("late block", &|i, _| (250..330).contains(&i)),
        ("random tenth of the second half", &|i, r| i >= n / 2 && r.below(10) == 0),
        ("everything", &|_, _| true),
        ("one late", &|i, _| i == n - 37),
    ];
    for (what, gone) in patterns {
        let mut mobs = mixed_world(&mut r, n);
        for i in 0..n {
            if gone(i, &mut r) {
                mobs.list.take(i);
            }
        }
        mobs.drop_empty_slots();
        mobs.index.assert_same_as_rebuilt(mobs.list.slots(), what);
    }
    // Sort after a mob came back with an older rank: same entries as a rebuild.
    for _ in 0..50 {
        let mut mobs = mixed_world(&mut r, 100);
        let i = r.below(100) as usize;
        if let Some(m) = mobs.list.get_mut(i) {
            m.rank = r.below(m.rank);
        }
        mobs.sort_by_rank();
        mobs.index.assert_same_as_rebuilt(mobs.list.slots(), "sort");
    }
}

/// A mob put straight into the list (without `Mobs::push`) before a late removal: the tail re-index
/// must not keep entries that never saw it, whatever the per-species switch, or a later mob with the
/// same identifier would be served instead of that first one (run with `RUSTCRAFT_OPT_KINDINDEX=0`
/// too: the per-species length check no longer covers it there).
#[test]
fn direct_addition_before_a_tail_removal() {
    let mut r = Rng(0xD1);
    let mut mobs = Mobs::default();
    for i in 0..40 {
        mobs.push(Box::new(random_mob(&mut r, 100 + i, 8.0)));
    }
    // Slot 40, the index is not told.
    mobs.list.push_unindexed(Some(Box::new(random_mob(&mut r, 7, 8.0))));
    for i in 0..40 {
        mobs.push(Box::new(random_mob(&mut r, 300 + i, 8.0)));
    }
    // One late removal: a tail short enough for the tail re-index.
    mobs.list.take(75);
    mobs.drop_empty_slots();
    mobs.push(Box::new(random_mob(&mut r, 7, 8.0)));
    assert_eq!(mobs.index_of(7), Some(40), "the mob the index never saw is the first with this identifier");
}

/// The species list a zombie alerts is exactly the zombie class without the zombified piglin.
#[test]
fn zombie_class_matches_the_tag() {
    use super::goals::ZOMBIE_CLASS;
    for k in Kind::ALL {
        let class = k.zombie_like() && k != Kind::ZombifiedPiglin;
        assert_eq!(ZOMBIE_CLASS.contains(&k), class, "{k:?}");
    }
}

/// Per-species index: repository mutations (additions by `push` or direct, take-outs/put-backs, portal
/// removal, end of tick), each query compared with the scan (same sequence, same order).
#[test]
fn species_index_equals_scan() {
    let mut r = Rng(0xC6_07);
    let kinds = [Kind::Shulker, Kind::Bee, Kind::Vex, Kind::Cow, Kind::Villager];
    for round in 0..200 {
        let mut mobs = Mobs::default();
        let mut next = 1;
        let mut taken: Vec<(usize, Box<Mob>)> = Vec::new();
        let add = |r: &mut Rng, next: i32| {
            let mut m = random_mob(r, next, 32.0);
            m.kind = kinds[r.below(kinds.len() as u64) as usize];
            Box::new(m)
        };
        for _ in 0..(20 + r.below(200)) {
            mobs.push(add(&mut r, next));
            next += 1;
        }
        for step in 0..300 {
            let n = mobs.list.len();
            match r.below(7) {
                0 => mobs.push(add(&mut r, next)),
                1 if round % 3 == 0 => mobs.list.push_unindexed(Some(add(&mut r, next))),
                2 if n > 0 => {
                    let i = r.below(n as u64) as usize;
                    if let Some(m) = mobs.list.take(i) {
                        taken.push((i, m));
                    }
                }
                3 => {
                    if let Some((i, m)) = taken.pop()
                        && mobs.list.get(i).is_none()
                    {
                        mobs.list.put(i, m);
                    }
                }
                4 if n > 0 => {
                    taken.clear();
                    mobs.remove_slot(r.below(n as u64) as usize);
                }
                5 => {
                    taken.clear();
                    take_out_the_dead(&mut mobs);
                    mobs.drop_empty_slots();
                }
                // A mob back with an older rank (rejoined chunk), list put back in rank order.
                6 if n > 0 && taken.is_empty() => {
                    if let Some(m) = mobs.list.get_mut(r.below(n as u64) as usize) {
                        m.rank = r.below(m.rank.max(1));
                    }
                    mobs.sort_by_rank();
                }
                _ => {}
            }
            next += 1;
            for &k in &kinds {
                let got: Vec<i32> = mobs.of_kind(k).map(|o| o.id).collect();
                assert_eq!(got, kind_scan(&mobs, k), "round {round} step {step} {k:?}");
            }
            // Several species, one named twice, interleaved in the list: same sequence as the scan,
            // read-only and mutable.
            let pick = [kinds[step % 5], kinds[(step + 2) % 5], kinds[step % 5]];
            let want = kinds_scan(&mobs, &pick);
            assert_eq!(mobs.of_kinds(pick).map(|o| o.id).collect::<Vec<_>>(), want, "round {round} step {step}");
            assert_eq!(mobs.of_kinds_mut(pick).map(|o| o.id).collect::<Vec<_>>(), want, "round {round} step {step}");
        }
        // The index is indeed serving (not only the fallback) after an end of tick.
        mobs.index.refresh(mobs.list.slots(), true);
        assert_eq!(mobs.index.kind_slots(mobs.list.slots(), Kind::Shulker).is_some(), super::index::kind_on());
    }
}

/// Sabotage: a slot replaced by a mob of another species without warning the index
/// (mutation outside the assumption) is seen by the check of the tests.
#[test]
fn species_index_sabotage_seen() {
    if !super::index::kind_on() {
        return; // RUSTCRAFT_OPT_KINDINDEX=0: scan alone
    }
    let mut r = Rng(7);
    let mut mobs = Mobs::default();
    for i in 0..10 {
        let mut m = random_mob(&mut r, i, 8.0);
        m.kind = Kind::Cow;
        mobs.push(Box::new(m));
    }
    let mut s = random_mob(&mut r, 99, 8.0);
    s.kind = Kind::Shulker;
    mobs.list.put(4, Box::new(s));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mobs.of_kind(Kind::Shulker).count()));
    assert!(caught.is_err(), "replacement not seen by the check");
}

/// Dimensions in one pass: same bits as `width()` then `height()`.
#[test]
fn dims_one_pass_identical() {
    let mut r = Rng(7);
    for i in 0..20_000 {
        let m = random_mob(&mut r, i, 16.0);
        let (w, h) = m.dims_wh();
        assert_eq!((w.to_bits(), h.to_bits()), (m.width().to_bits(), m.height().to_bits()), "{:?}", m.kind);
    }
}

/// Static bound: every box of a bounded species fits within `reach`, and a bound trimmed by
/// 10 % would be caught (the test is sensitive to sabotage).
#[test]
fn static_bound_holds() {
    let mut r = Rng(11);
    let mut tight = false;
    for i in 0..50_000 {
        let m = random_mob(&mut r, i, 16.0);
        let Some((rr, hh)) = reach(m.kind) else { continue };
        let b = m.bbox();
        let p = m.pos();
        let within = |rr: f64, hh: f64| {
            b.min[0] >= p[0] - rr
                && b.max[0] <= p[0] + rr
                && b.min[2] >= p[2] - rr
                && b.max[2] <= p[2] + rr
                && b.min[1] >= p[1]
                && b.max[1] <= p[1] + hh
        };
        assert!(within(rr, hh), "{:?} outside its bound", m.kind);
        tight |= !within(rr * 0.9, hh * 0.9);
    }
    assert!(tight, "a bound trimmed by 10 % was never caught: insensitive test");
    // All the states that change the box, without randomness: each species, baby or adult, each
    // pose (warden digging and emerging, goat long jump…), each sniffer state,
    // camel sitting or standing.
    for k in Kind::ALL {
        let Some((rr, hh)) = reach(k) else { continue };
        for baby in [false, true] {
            for pose in 0..32 {
                for sniffer in 0..8u8 {
                    for sitting in [false, true] {
                        let mut m = Mob::new(k, 1, 1, [0.5, 64.0, 0.5], 0.0, JRand::new(1));
                        if baby {
                            m.baby = true;
                            m.age = -24000;
                        }
                        m.bm.pose = pose;
                        m.bm.sniffer = sniffer;
                        m.mount.camel_sitting_pose = sitting;
                        let (b, p) = (m.bbox(), m.pos());
                        let ok = b.min[0] >= p[0] - rr
                            && b.max[0] <= p[0] + rr
                            && b.min[2] >= p[2] - rr
                            && b.max[2] <= p[2] + rr
                            && b.min[1] >= p[1]
                            && b.max[1] <= p[1] + hh;
                        assert!(
                            ok,
                            "{k:?} (baby {baby}, pose {pose}, sniffer {sniffer}, sitting {sitting}) outside its bound"
                        );
                    }
                }
            }
        }
    }
}

/// Push and queries by box: same mobs, same order, with and without prefilter, on
/// 1,000 worlds of 50 to 300 mobs (babies, cubes, shulkers, phantoms, poses).
#[test]
fn prefilter_same_neighbours() {
    let mut r = Rng(0xC0FFEE);
    let mut pairs = 0usize;
    for _ in 0..1000 {
        let n = 50 + r.below(251) as usize;
        let side = 4.0 + r.unit() * 40.0;
        let list = world(&mut r, n, side);
        for m in list.iter().flatten() {
            let bb = m.bbox();
            let old: Vec<i32> =
                list.iter().flatten().filter(|o| o.alive() && o.bbox().intersects(&bb)).map(|o| o.id).collect();
            let new: Vec<i32> = list.iter().flatten().filter(|o| o.alive() && o.touches(&bb)).map(|o| o.id).collect();
            assert_eq!(old, new, "neighbours of {} ({:?})", m.id, m.kind);
            pairs += old.len();
        }
    }
    assert!(pairs > 10_000, "too few contacts ({pairs}): worlds too sparse to prove anything");
}

/// A bench that turns off the comparison with the scan only turns it off on its thread: the tests run
/// in parallel (`--include-ignored`) keep their proof.
#[test]
fn check_specific_to_the_thread() {
    std::thread::spawn(|| {
        super::index::set_verify(false);
        assert!(!super::index::verify());
    })
    .join()
    .expect("bench thread");
    assert!(super::index::verify(), "another thread's cut turned off this one's check");
}

// ---------- Micro-benches (calm machine; `--ignored --nocapture`) ----------

fn time<R>(f: impl FnOnce() -> R) -> (R, f64) {
    let t = std::time::Instant::now();
    let r = f();
    (r, t.elapsed().as_secs_f64() * 1e3)
}

/// 10⁶ searches by identifier on 100 / 1,000 / 5,000 mobs: scan against index.
#[test]
#[ignore = "measurement bench"]
fn bench_mob_ref() {
    super::index::set_verify(false);
    for n in [100usize, 1000, 5000] {
        let mut r = Rng(5);
        let mut mobs = Mobs::default();
        for i in 0..n {
            mobs.push(Box::new(random_mob(&mut r, i as i32 * 2 + 1, 32.0)));
        }
        // Present (odd identifiers), then absent (even: always the scan).
        for (what, odd) in [("present", 1), ("absent", 0)] {
            let ids: Vec<i32> = (0..1_000_000).map(|_| r.below(n as u64) as i32 * 2 + odd).collect();
            let (a, ta) = time(|| ids.iter().filter_map(|&id| scan_id(mobs.list.slots(), id)).sum::<usize>());
            let (b, tb) = time(|| ids.iter().filter_map(|&id| mobs.index_of(id)).sum::<usize>());
            assert_eq!(a, b);
            eprintln!("bench_mob_ref N={n} {what}: scan {ta:.1} ms, index {tb:.1} ms (×{:.1})", ta / tb.max(1e-9));
        }
    }
}

/// 10⁵ walks of a rare species (1 % shulkers) on 100 / 1,000 / 5,000 mobs.
#[test]
#[ignore = "measurement bench"]
fn bench_of_kind() {
    super::index::set_verify(false);
    for n in [100usize, 1000, 5000] {
        let mut r = Rng(11);
        let mut mobs = Mobs::default();
        for i in 0..n {
            let mut m = random_mob(&mut r, i as i32, 32.0);
            m.kind = if i % 100 == 0 { Kind::Shulker } else { Kind::Cow };
            mobs.push(Box::new(m));
        }
        let reps = 100_000;
        let (a, ta) = time(|| (0..reps).map(|_| kind_scan_count(&mobs)).sum::<usize>());
        let (b, tb) =
            time(|| (0..reps).map(|_| std::hint::black_box(&mobs).of_kind(Kind::Shulker).count()).sum::<usize>());
        assert_eq!(a, b);
        eprintln!("bench_of_kind N={n}: scan {ta:.1} ms, index {tb:.1} ms (×{:.1})", ta / tb.max(1e-9));
    }
}

fn kind_scan_count(mobs: &Mobs) -> usize {
    std::hint::black_box(mobs).list.iter().filter(|o| o.kind == Kind::Shulker).count()
}

/// The single-species walk as it was before `of_kinds` (bench reference).
fn single_species_walk(mobs: &Mobs, k: Kind) -> impl Iterator<Item = &Mob> {
    let (s, all) = match mobs.index.kind_slots(mobs.list.slots(), k) {
        Some(s) => (s, &mobs.list.slots()[..0]),
        None => (&[][..], mobs.list.slots()),
    };
    s.iter()
        .filter_map(|&i| mobs.list.get(i as usize))
        .chain(all.iter().flatten().map(|b| &**b))
        .filter(move |o| o.kind == k)
}

/// ns per query on 1,000 mobs: species absent (the common case of an alert or a neighbour search
/// far from its species), then 1 % present; one species and three merged.
#[test]
#[ignore = "measurement bench"]
fn bench_of_kinds() {
    super::index::set_verify(false);
    let reps = 1_000_000u32;
    for present in [false, true] {
        let mut r = Rng(13);
        let mut mobs = Mobs::default();
        for i in 0..1000 {
            let mut m = random_mob(&mut r, i, 32.0);
            m.kind = match i % 100 {
                0 if present => Kind::Shulker,
                50 if present => Kind::Vex,
                _ => Kind::Cow,
            };
            mobs.push(Box::new(m));
        }
        let three = [Kind::Shulker, Kind::Vex, Kind::Evoker];
        let b = std::hint::black_box;
        let ns = |t: f64| t * 1e6 / f64::from(reps);
        let (a, t_scan) = time(|| (0..reps).map(|_| kind_scan_count(b(&mobs))).sum::<usize>());
        let (c, t_before) = time(|| (0..reps).map(|_| single_species_walk(b(&mobs), Kind::Shulker).count()).sum::<usize>());
        let (d, t_one) = time(|| (0..reps).map(|_| b(&mobs).of_kind(Kind::Shulker).count()).sum::<usize>());
        let (e, t_scan3) = time(|| {
            (0..reps).map(|_| b(&mobs).list.iter().filter(|o| three.contains(&o.kind)).count()).sum::<usize>()
        });
        let (f, t_three) = time(|| (0..reps).map(|_| b(&mobs).of_kinds(three).count()).sum::<usize>());
        let (g, t_three_mut) =
            time(|| (0..reps).map(|_| std::hint::black_box(&mut mobs).of_kinds_mut(three).count()).sum::<usize>());
        assert!(a == c && c == d && e == f && f == g);
        eprintln!(
            "bench_of_kinds N=1000 present={present}: one species scan {:.1} ns, before {:.1} ns, of_kind {:.1} ns; \
             three species scan {:.1} ns, of_kinds {:.1} ns, of_kinds_mut {:.1} ns",
            ns(t_scan),
            ns(t_before),
            ns(t_one),
            ns(t_scan3),
            ns(t_three),
            ns(t_three_mut)
        );
    }
}

/// End-of-tick refresh after removals on 1,000 and 5,000 mobs: whole rebuild against the tail
/// re-index (µs per refresh). Removal patterns: one mob near the end, the last ten, half (TNT in
/// the crowd), one mob at the front.
#[test]
#[ignore = "measurement bench"]
fn bench_tail_refresh() {
    super::index::set_verify(false);
    for n in [1000usize, 5000] {
        let mut r = Rng(17);
        let before = mixed_world(&mut r, n);
        let patterns: [(&str, &dyn Fn(usize) -> bool); 4] = [
            ("one near the end", &|i| i == n - n / 20),
            ("last ten", &|i| i >= n - 10),
            ("half", &|i| i % 2 == 1),
            ("first", &|i| i == 0),
        ];
        for (what, gone) in patterns {
            let after: Vec<Option<Box<Mob>>> =
                before.list.slots().iter().enumerate().filter(|(i, _)| !gone(*i)).map(|(_, m)| m.clone()).collect();
            let first = (0..n).position(gone).expect("a removal");
            let reps = 2000;
            let mut index = super::index::MobIndex::default();
            let (mut t_full, mut t_tail) = (0.0, 0.0);
            for _ in 0..reps {
                index.rebuild(before.list.slots());
                t_full += time(|| index.rebuild(std::hint::black_box(&after))).1;
                index.rebuild(before.list.slots());
                t_tail += time(|| index.refresh_from(std::hint::black_box(&after), first, n)).1;
            }
            let us = |t: f64| t * 1e3 / f64::from(reps);
            eprintln!("bench_tail_refresh N={n} {what}: rebuild {:.1} µs, tail {:.1} µs", us(t_full), us(t_tail));
        }
    }
}

/// Neighbour sampling of each mob (core of `mob_push_entities`) at the density of the
/// "crowd" scene (100 animals in 24 × 24): 100, 400, 1,600 mobs.
#[test]
#[ignore = "measurement bench"]
fn bench_push() {
    super::index::set_verify(false);
    for n in [100usize, 400, 1600] {
        let mut r = Rng(9);
        let side = 24.0 * (n as f64 / 100.0).sqrt();
        // Farm animals, a third of them babies, like the crowd.
        let farm = [Kind::Cow, Kind::Sheep, Kind::Pig, Kind::Chicken];
        let list: Vec<Option<Box<Mob>>> = (0..n)
            .map(|i| {
                let mut m = random_mob(&mut r, i as i32, side);
                m.kind = farm[i % 4];
                m.health = 10.0;
                Some(Box::new(m))
            })
            .collect();
        let reps = (200_000 / n).max(2);
        let run = |f: &dyn Fn(&Mob, &rc_physics::Aabb) -> bool| {
            let mut c = 0usize;
            for _ in 0..reps {
                for m in list.iter().flatten() {
                    let bb = m.bbox();
                    c += list.iter().flatten().filter(|o| o.alive() && f(o, &bb)).count();
                }
            }
            c
        };
        let (a, t_old) = time(|| run(&|o, bb| Mob::bbox_at(o.pos(), o.width(), o.height()).intersects(bb)));
        let (b, t_once) = time(|| {
            run(&|o, bb| {
                let (w, h) = o.dims_wh();
                Mob::bbox_at(o.pos(), w, h).intersects(bb)
            })
        });
        let (c, t_filter) = time(|| run(&|o, bb| o.touches(bb)));
        assert!(a == b && b == c);
        let per = |t: f64| t / reps as f64;
        eprintln!(
            "bench_push N={n}: old per tick {:.3} ms, one pass {:.3} ms, prefilter+one pass {:.3} ms ({} contacts/tick)",
            per(t_old),
            per(t_once),
            per(t_filter),
            a / reps
        );
    }
}
