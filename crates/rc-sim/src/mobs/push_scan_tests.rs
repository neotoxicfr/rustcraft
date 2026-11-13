//! Proofs of the push between mobs (`body.rs`, `docs/perf.md`):
//!
//! - the scan for the mobs a mob pushes collects the same mobs, in the same order, whether it
//!   tests the species bound or the liveness first (`RUSTCRAFT_OPT_PUSH_SCAN_BOUND_FIRST`), the
//!   bound in one go or not, and the same as a walk that uses no bound at all, in a pen with
//!   dead and removed mobs, riders, mobs on ladders, teams and species whose box has no fixed
//!   bound. Every push of the whole suite also runs every scan and compares them (`body.rs`);
//! - the species bound compared with a box in one go or comparison by comparison
//!   (`RUSTCRAFT_OPT_PUSH_BOUND_ALL_AT_ONCE`) answers like `Mob::may_touch`, for every pair of the
//!   pen and for boxes that touch the bound exactly, miss it by the smallest step, or are
//!   undefined or infinite;
//! - the pushes are the same wherever the contacts are collected (kept buffer, list in place,
//!   new vector: `RUSTCRAFT_OPT_PUSH_CONTACT_BUFFER`, `RUSTCRAFT_OPT_PUSH_CONTACTS_INLINE`), and
//!   the kept buffer saves at least one allocation per mob that touches another;
//! - a pen of 1,000 mobs with the cramming rule at 3, minecarts, teams and the player, 200 ticks:
//!   positions, velocities, health and random draws against a fingerprint stored before these
//!   switches existed. The fingerprint is the same with each switch turned the other way.
//!
//! Micro-bench at the end (ignored).

use super::body::{PushContact, PushContactStorage, PushScan};
use super::mob::{Ent, Mob, TouchBounds};
use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Small deterministic generator of the pens (xorshift).
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Species of the pens: fixed boxes (adults and babies), boxes without a fixed bound (cubes by
/// size, phantom, pufferfish, salmon), and the golem and warden with their own push reactions.
const PEN_KINDS: [Kind; 16] = [
    Kind::Cow,
    Kind::Sheep,
    Kind::Pig,
    Kind::Chicken,
    Kind::Zombie,
    Kind::Slime,
    Kind::Villager,
    Kind::MagmaCube,
    Kind::Wolf,
    Kind::Rabbit,
    Kind::Spider,
    Kind::IronGolem,
    Kind::Phantom,
    Kind::Pufferfish,
    Kind::Salmon,
    Kind::Warden,
];

/// Two teams that restrict collisions, joined by a part of the mobs of the pen.
fn add_teams(p: &mut Probe) {
    for (team, rule) in [("apart", "never"), ("friends", "pushOwnTeam")] {
        cmd(p, &format!("team add {team}"));
        cmd(p, &format!("team modify {team} collisionRule {rule}"));
    }
}

/// `n` persistent mobs of `kinds` in the rectangle from (`x0`, `z0`), `width` by `depth`, feet on
/// the flat ground: a fifth are babies, cubes get sizes 1 to 4, one mob in eleven joins a team.
fn fill_pen(p: &mut Probe, dice: &mut Dice, kinds: &[Kind], n: usize, [x0, z0]: [f64; 2], [width, depth]: [f64; 2]) {
    for i in 0..n {
        let at = [x0 + dice.unit() * width, -60.0, z0 + dice.unit() * depth];
        let mut m = p.s.new_mob(kinds[i % kinds.len()], at, (dice.next() % 360) as f32);
        m.persistent = true;
        if i % 5 == 0 {
            m.age = -24000;
        }
        if m.kind.is_cube() {
            m.set_size(1 + (dice.next() % 4) as i32, true);
        }
        if i % 11 == 0 {
            let team = if i % 22 == 0 { "apart" } else { "friends" };
            p.s.cmd.scores.join_team(&crate::nbtio::uuid_text(m.uuid), team);
        }
        p.s.add_mob(m);
    }
}

/// Dense pen for the pushes driven by hand (no tick): 600 mobs of every pen species in 22 × 22,
/// with dead and removed mobs left in the list, riders, a patch of ladders, a clump of 14 pigs
/// (more contacts than a list in place holds), 12 minecarts and the player inside.
fn hand_driven_pen(tag: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    add_teams(&mut p);
    cmd(&mut p, "fill 10 -60 10 13 -60 13 minecraft:ladder");
    let mut dice = Dice(0xC6_06);
    fill_pen(&mut p, &mut dice, &PEN_KINDS, 600, [0.0, 0.0], [22.0, 22.0]);
    let ids: Vec<i32> = p.s.mobs.list.iter().map(|m| m.id).collect();
    for (i, m) in p.s.mobs.list.iter_mut().enumerate() {
        match i % 13 {
            // Dying: still in the list, pushes nothing and takes no push.
            3 => m.health = 0.0,
            // Removed during the tick, slot not dropped yet.
            7 => m.removed = true,
            // Rides a mob further in the list.
            9 => m.mount.vehicle = Some(ids[(i + 40) % ids.len()]),
            _ => {}
        }
    }
    for i in 0..14 {
        let mut pig =
            p.s.new_mob(Kind::Pig, [20.2 + f64::from(i % 4) * 0.1, -60.0, 20.2 + f64::from(i / 4) * 0.1], 0.0);
        pig.persistent = true;
        p.s.add_mob(pig);
    }
    for i in 0..12 {
        cmd(&mut p, &format!("summon minecraft:minecart {}.5 -60 {}.5", 2 + (i % 4) * 5, 3 + (i / 4) * 7));
    }
    p.s.pos = [11.5, -60.0, 6.5];
    (p, dir)
}

/// The mobs `m` pushes by a walk that knows no bound: every mob of the list, alive, whose box
/// touches and that takes the push. With the count of riders.
fn pushed_mobs_without_bound(
    p: &Probe,
    m: &Mob,
    bb: &rc_physics::Aabb,
    team: Option<(Option<u32>, u8)>,
) -> (Vec<PushContact>, i32) {
    let mut contacts = Vec::new();
    let mut riders = 0;
    for (slot, other) in p.s.mobs.list.slots().iter().enumerate() {
        if let Some(other) = other
            && other.alive()
            && p.s.mob_takes_push(m, other, bb, team)
        {
            contacts.push((Ent::Mob(other.id), slot as u32));
            riders += i32::from(other.mount.vehicle.is_some());
        }
    }
    (contacts, riders)
}

/// What the pen of the scan test must contain for the comparison to mean something.
#[derive(Default, Debug)]
struct ScanCoverage {
    contacts: usize,
    riders: i32,
    /// Contacts whose species has no fixed bound.
    unbounded_contacts: usize,
    /// Dead or removed mobs whose bound reaches the pushing mob: the liveness test decides.
    dead_within_reach: usize,
    /// Mobs refused by a team rule (contacts without the team filter minus contacts with it).
    refused_by_team: usize,
}

/// `RUSTCRAFT_OPT_PUSH_SCAN_BOUND_FIRST`: for every mob of the pen taken out of its slot like in
/// its tick, every scan collects the mobs of the walk without bound, in the same order, with the
/// same count of riders.
#[test]
fn every_scan_collects_the_mobs_of_the_walk_without_bound() {
    let (mut p, _dir) = hand_driven_pen("push-scan-orders");
    let bounds = TouchBounds::fetch();
    let mut seen = ScanCoverage::default();
    for slot in 0..p.s.mobs.list.len() {
        let m = p.s.mobs.list.take(slot).expect("mob of the pen");
        let bb = m.bbox();
        let team = p.s.push_team(Some(m.uuid));
        let (expected, expected_riders) = pushed_mobs_without_bound(&p, &m, &bb, team);
        for scan in PushScan::EVERY {
            let mut contacts = Vec::new();
            let riders = p.s.collect_pushed_mobs(&m, &bb, team, scan, &mut contacts);
            assert_eq!(
                (&contacts, riders),
                (&expected, expected_riders),
                "mobs pushed by {} ({:?}), scan {scan:?}",
                m.id,
                m.kind
            );
        }
        seen.contacts += expected.len();
        seen.riders += expected_riders;
        seen.refused_by_team += pushed_mobs_without_bound(&p, &m, &bb, None).0.len() - expected.len();
        for other in p.s.mobs.list.iter() {
            seen.dead_within_reach +=
                usize::from(!other.alive() && bounds.may_touch(other, &bb) && other.bbox().intersects(&bb));
        }
        for (entity, _) in &expected {
            let Ent::Mob(id) = entity else { continue };
            let kind = p.s.mob_ref(*id).expect("contact").kind;
            seen.unbounded_contacts += usize::from(super::species::reach(kind).is_none());
        }
        p.s.mobs.list.put(slot, m);
    }
    eprintln!("push scan orders: {seen:?}");
    assert!(
        seen.contacts > 1000
            && seen.riders > 50
            && seen.unbounded_contacts > 200
            && seen.dead_within_reach > 200
            && seen.refused_by_team > 50,
        "coverage too thin: {seen:?}"
    );
}

/// Boxes around the bound of `m` (half-width `r`, height `h`): on each side one that touches the
/// bound exactly (not reached: the game's boxes touch only when they overlap), one a smallest
/// step further and one a smallest step inside; then boxes with an undefined or infinite side.
fn boxes_around_the_bound(m: &Mob, r: f64, h: f64) -> Vec<rc_physics::Aabb> {
    let p = m.pos();
    let (low, high) = ([p[0] - r, p[1], p[2] - r], [p[0] + r, p[1] + h, p[2] + r]);
    let around = rc_physics::Aabb {
        min: [low[0] - 1.0, low[1] - 1.0, low[2] - 1.0],
        max: [high[0] + 1.0, high[1] + 1.0, high[2] + 1.0],
    };
    let mut boxes = vec![around];
    for axis in 0..3 {
        for edge in [low[axis], low[axis].next_down(), low[axis].next_up()] {
            let mut below = around;
            below.max[axis] = edge;
            boxes.push(below);
        }
        for edge in [high[axis], high[axis].next_up(), high[axis].next_down()] {
            let mut above = around;
            above.min[axis] = edge;
            boxes.push(above);
        }
        for odd in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let (mut odd_min, mut odd_max) = (around, around);
            odd_min.min[axis] = odd;
            odd_max.max[axis] = odd;
            boxes.extend([odd_min, odd_max]);
        }
    }
    boxes
}

/// `RUSTCRAFT_OPT_PUSH_BOUND_ALL_AT_ONCE`: the bound compared in one go and comparison by
/// comparison both answer like `Mob::may_touch`, for every pair of mobs of the pen and for the
/// boxes at the edge of each mob's bound.
#[test]
fn bound_compared_all_at_once_answers_like_one_by_one() {
    let (p, _dir) = hand_driven_pen("push-bound-compare");
    let bounds = TouchBounds::fetch();
    let (mut reached, mut missed) = (0usize, 0usize);
    let mut check = |m: &Mob, bb: &rc_physics::Aabb| {
        let expected = m.may_touch(bb);
        assert_eq!(
            bounds.may_touch_all_at_once(m, bb),
            expected,
            "all at once: {:?} at {:?} against {bb:?}",
            m.kind,
            m.pos()
        );
        assert_eq!(bounds.may_touch(m, bb), expected, "one by one: {:?} at {:?} against {bb:?}", m.kind, m.pos());
        reached += usize::from(expected);
        missed += usize::from(!expected);
    };
    for m in p.s.mobs.list.iter() {
        for other in p.s.mobs.list.iter() {
            check(m, &other.bbox());
        }
        if let Some((r, h)) = super::species::reach(m.kind) {
            for bb in boxes_around_the_bound(m, r, h) {
                check(m, &bb);
            }
        }
    }
    eprintln!("bound compare: {reached} boxes reached, {missed} missed");
    // With `RUSTCRAFT_OPT_PUSHFILTER=0` no species has a bound: every box is reached.
    let enough_missed = if super::mob::push_filter() { missed > 200_000 } else { missed == 0 };
    assert!(reached > 20_000 && enough_missed, "coverage too thin: {reached} reached, {missed} missed");
}

/// One round of pushes driven by hand: every mob taken out of its slot, pushing with its contacts
/// collected in `storage`, put back; then every mob moved by its velocity so that the next round
/// meets other neighbours. Returns the number of mobs that pushed at least one mob.
fn push_round(p: &mut Probe, storage: PushContactStorage) -> usize {
    let mut pushing = 0;
    let mut pushed = Vec::new();
    for slot in 0..p.s.mobs.list.len() {
        let Some(mut m) = p.s.mobs.list.take(slot) else { continue };
        if m.alive() {
            pushed.clear();
            p.s.collect_pushed_mobs(
                &m,
                &m.bbox(),
                p.s.push_team(Some(m.uuid)),
                PushScan::LivenessThenBound,
                &mut pushed,
            );
            pushing += usize::from(!pushed.is_empty());
            p.s.mob_push_entities_storing_in(&mut m, storage);
        }
        p.s.mobs.list.put(slot, m);
    }
    for m in p.s.mobs.list.iter_mut() {
        let (pos, vel) = (m.pos(), m.vel);
        m.set_pos([pos[0] + vel[0] * 4.0, pos[1], pos[2] + vel[2] * 4.0]);
        m.vel = [vel[0] * 0.5, 0.0, vel[2] * 0.5];
    }
    pushing
}

/// SHA-256 of the mobs (identifier, position, velocity, health, random state, removal), of the
/// minecarts (position, velocity) and of the player's health.
fn pen_fingerprint(p: &Probe) -> String {
    let mut lines: Vec<String> =
        p.s.mobs
            .list
            .iter()
            .map(|m| {
                format!(
                    "{} {:?} {:?} {:x} {:?} {} {}",
                    m.id,
                    m.pos().map(f64::to_bits),
                    m.vel.map(f64::to_bits),
                    m.health.to_bits(),
                    m.rng,
                    m.removed,
                    m.needs_sync
                )
            })
            .collect();
    lines.sort();
    for c in p.s.carts.list.iter().flatten() {
        lines.push(format!("cart {} {:?} {:?}", c.id, c.pos.map(f64::to_bits), c.vel.map(f64::to_bits)));
    }
    lines.push(format!("player {:x}", p.s.life.vit.health.to_bits()));
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for line in &lines {
        ctx.update(line.as_bytes());
        ctx.update(b"\n");
    }
    ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Result of the hand-driven pen pushed with one contact storage.
#[derive(Debug, PartialEq, Eq)]
struct HandDrivenPushes {
    fingerprint: String,
    /// Mobs that pushed at least one mob, summed over the measured rounds.
    pushing: usize,
    /// Mobs hurt or killed by the cramming.
    hurt: usize,
}

/// 12 rounds of pushes in the hand-driven pen with the cramming rule at 3; the allocations of the
/// rounds are returned beside the result.
fn hand_driven_pushes(storage: PushContactStorage) -> (HandDrivenPushes, u64) {
    let (mut p, _dir) = hand_driven_pen(&format!("push-storage-{storage:?}"));
    cmd(&mut p, "gamerule max_entity_cramming 3");
    let full_health: Vec<(i32, u32)> = p.s.mobs.list.iter().map(|m| (m.id, m.health.to_bits())).collect();
    let allocations_before = crate::test_alloc::allocations();
    let pushing = (0..12).map(|_| push_round(&mut p, storage)).sum();
    let allocations = crate::test_alloc::allocations() - allocations_before;
    assert!(p.s.mobs.push_contacts.is_empty(), "the kept buffer is empty between two pushes");
    let hurt =
        p.s.mobs.list.iter().zip(&full_health).filter(|(m, before)| (m.id, m.health.to_bits()) != **before).count();
    (HandDrivenPushes { fingerprint: pen_fingerprint(&p), pushing, hurt }, allocations)
}

/// `RUSTCRAFT_OPT_PUSH_CONTACT_BUFFER`, `RUSTCRAFT_OPT_PUSH_CONTACTS_INLINE`: the same pen pushed
/// by hand with the three storages ends in the same state (each in a fresh thread: the per-thread
/// test seed restarts there); the kept buffer saves at least one allocation per mob that pushed
/// another, and the list in place only allocates for the clump that overflows it.
#[test]
fn every_contact_storage_pushes_the_same() {
    let run = |storage| std::thread::spawn(move || hand_driven_pushes(storage)).join().expect("pushes");
    let (fresh, fresh_allocations) = run(PushContactStorage::FreshVec);
    let (kept, kept_allocations) = run(PushContactStorage::KeptBuffer);
    let (inline, inline_allocations) = run(PushContactStorage::Inline);
    eprintln!(
        "contact storages: {fresh:?}; allocations over the rounds: new vector {fresh_allocations}, kept buffer \
         {kept_allocations}, in place {inline_allocations}"
    );
    assert!(fresh.pushing > 2000 && fresh.hurt > 100, "coverage too thin: {fresh:?}");
    assert_eq!(std::mem::size_of::<PushContact>(), 12, "a contact stays small: eight are kept in place");
    assert_eq!(kept, fresh, "kept buffer against a new vector per mob");
    assert_eq!(inline, fresh, "list in place against a new vector per mob");
    // The three runs are the same simulation: their allocations differ only by the contact lists.
    let saved = fresh_allocations - kept_allocations;
    assert!(
        saved as usize >= fresh.pushing,
        "the kept buffer saved {saved} allocations for {} mobs that pushed another",
        fresh.pushing
    );
    assert!(
        kept_allocations < inline_allocations && inline_allocations < fresh_allocations,
        "in place: {inline_allocations} allocations, between the kept buffer ({kept_allocations}) and a new vector \
         ({fresh_allocations}): only the clump overflows it"
    );
}

/// The pen of the tick fingerprint: 1,000 mobs in 40 × 34 on the flat world, the cramming rule at
/// 3 (the crowd thins out by cramming deaths: dying mobs stay in the list for a while), two teams
/// that restrict collisions, 12 minecarts, the player standing at its corner; `ticks` game ticks.
/// Returns the fingerprint and the number of mobs left.
fn crowded_pen_after(ticks: i32) -> (String, usize) {
    let (mut p, _dir) = crate::tests_g3f13::open("push-crowded-pen");
    cmd(&mut p, crate::tests_opt::PLAYER_TAKES_NO_DAMAGE);
    add_teams(&mut p);
    cmd(&mut p, "gamerule max_entity_cramming 3");
    let kinds = [
        Kind::Cow,
        Kind::Sheep,
        Kind::Pig,
        Kind::Chicken,
        Kind::Zombie,
        Kind::Slime,
        Kind::Villager,
        Kind::Rabbit,
        Kind::IronGolem,
        Kind::Wolf,
    ];
    let mut dice = Dice(0x00C6_0600);
    fill_pen(&mut p, &mut dice, &kinds, 1000, [0.0, -10.0], [40.0, 34.0]);
    for i in 0..12 {
        cmd(&mut p, &format!("summon minecraft:minecart {}.5 -60 {}.5", 3 + (i % 4) * 9, -6 + (i / 4) * 12));
    }
    for _ in 0..ticks {
        p.s.game_tick();
        p.s.tick += 1;
        let _ = p.take_events();
    }
    (pen_fingerprint(&p), p.s.mobs.list.iter().count())
}

/// Expected fingerprint of the crowded pen after 200 ticks and number of mobs left, recorded on
/// the code before the push switches of this file existed; identical with
/// `RUSTCRAFT_OPT_PUSH_SCAN_BOUND_FIRST=0`, `RUSTCRAFT_OPT_PUSH_CONTACT_BUFFER=0` and
/// `RUSTCRAFT_OPT_PUSH_CONTACTS_INLINE=1`. Replaced on purpose by L13-ENTITES: a conversion hands the
/// old mob's team to the new one, so the slimes split from a teamed slime (and the zombie villagers
/// of teamed villagers) keep its collision rule (with that hand-over alone taken out, the value
/// before comes back: "01f3a501…a9e8", 726 mobs left). Replaced on purpose by L15-ENTITES: the pen's
/// zombies killed the player, who now leaves the world 20 ticks after its death (nothing simulated
/// around it: too few cramming deaths for the coverage), so the player takes no damage now; the
/// value is the same with the removal neutralised (checked), and "07a524d1…9a92", 715 comes back
/// without the resistance and the removal. Replaced on purpose by the boats work: the push between
/// two mobs now multiplies in the game's order (× pow, then × 0.05), bit for bit; the former order
/// alone gives back "04491f20…9497", 720 (checked).
const CROWDED_PEN: (&str, usize) = ("eb4e57bb266d6bee4c767d59a46f8aea5601446c56de3d57dc20a0fb6eef0c54", 723);

/// 1,000 mobs, 200 ticks with the cramming rule at 3: same mobs, minecarts and player as the
/// stored fingerprint, in a fresh thread (the per-thread test seed restarts there).
#[test]
fn crowded_pen_ticks_like_the_stored_fingerprint() {
    let got = std::thread::spawn(|| crowded_pen_after(200)).join().expect("simulation");
    eprintln!("crowded pen: fingerprint {} ({} mobs left)", got.0, got.1);
    assert!(got.1 < 900, "coverage: the cramming killed too few mobs ({} left of 1000)", got.1);
    assert_eq!(
        (got.0.as_str(), got.1),
        CROWDED_PEN,
        "crowded pen fingerprint changed: an optimisation must never change it; an intended behaviour fix \
         replaces CROWDED_PEN with ({:?}, {})",
        got.0,
        got.1
    );
}

/// Median of the samples.
fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// The scans compared by the bench: the shipped ones and each part of the change alone.
#[derive(Clone, Copy, Debug)]
enum BenchScan {
    /// Old path: liveness first, bound table fetched for every mob.
    LivenessFirst,
    /// Liveness first, bound table fetched once.
    LivenessFirstTableOnce,
    /// Bound first, bound table fetched for every mob.
    BoundFirstTablePerMob,
    /// Bound first, bound table fetched once.
    BoundFirst,
    /// Bound first, bound table fetched once, comparisons of the bound combined before one jump.
    BoundAllAtOnceFirst,
}

/// One scan for the mobs `m` pushes, appended to `contacts`.
fn bench_scan(p: &Probe, m: &Mob, bb: &rc_physics::Aabb, scan: BenchScan, contacts: &mut Vec<PushContact>) {
    let s = &p.s;
    let bounds = TouchBounds::fetch();
    match scan {
        BenchScan::LivenessFirst => s.collect_pushed_mobs(m, bb, None, PushScan::LivenessThenBound, contacts),
        BenchScan::BoundFirst => s.collect_pushed_mobs(m, bb, None, PushScan::BoundThenLiveness, contacts),
        BenchScan::BoundAllAtOnceFirst => {
            s.collect_pushed_mobs(m, bb, None, PushScan::BoundAllAtOnceThenLiveness, contacts)
        }
        BenchScan::LivenessFirstTableOnce => {
            s.collect_pushed_mobs_where(m, bb, None, contacts, |other| other.alive() && bounds.may_touch(other, bb))
        }
        BenchScan::BoundFirstTablePerMob => {
            s.collect_pushed_mobs_where(m, bb, None, contacts, |other| other.may_touch(bb) && other.alive())
        }
    };
}

/// Micro-bench (ignored, one thread: the push runs on the server thread and each mob's push
/// depends on the previous ones): animals at the density of the "crowd" scene (100 in 24 × 24,
/// a third of them babies), N = 100, 400, 1,600, 5,000.
///
/// - scan alone: every mob taken out of its slot scans the list, old scan against new scan and
///   each part of the change alone, passes alternated; ns per mob of the list looked at and µs
///   for the scans of one tick;
/// - whole push (`mob_push_entities`, cramming rule off so that nothing dies) with each contact
///   storage: ns per mob and allocations per round.
#[test]
#[ignore = "micro-bench"]
fn bench_push_scan_and_contact_storage() {
    crate::mobs::index::set_verify(false);
    let layout = Mob::push_scan_field_offsets();
    println!(
        "[bench] push_scan layout: mob {} bytes; offsets position {}, species {}, health {}, removed {}",
        std::mem::size_of::<Mob>(),
        layout[0],
        layout[1],
        layout[2],
        layout[3]
    );
    for n in [100usize, 400, 1600, 5000] {
        let (mut p, _dir) = crate::tests_g3f13::open(&format!("push-bench-{n}"));
        cmd(&mut p, "gamerule max_entity_cramming 0");
        let side = 24.0 * (n as f64 / 100.0).sqrt();
        let kinds = [Kind::Cow, Kind::Sheep, Kind::Pig, Kind::Chicken];
        let mut dice = Dice(0x5EED);
        for i in 0..n {
            let at = [60.0 + dice.unit() * side, -60.0, 8.0 + dice.unit() * side];
            let mut m = p.s.new_mob(kinds[i % 4], at, 0.0);
            m.persistent = true;
            if i % 3 == 0 {
                m.age = -24000;
            }
            p.s.add_mob(m);
        }
        let rounds = (4_000_000 / (n * n)).max(1);
        let scans = [
            BenchScan::LivenessFirst,
            BenchScan::LivenessFirstTableOnce,
            BenchScan::BoundFirstTablePerMob,
            BenchScan::BoundFirst,
            BenchScan::BoundAllAtOnceFirst,
        ];
        let mut scan_times = scans.map(|_| Vec::new());
        let mut contacts = Vec::new();
        let mut touched = 0usize;
        for _pass in 0..7 {
            for (scan, times) in scans.iter().zip(&mut scan_times) {
                let start = std::time::Instant::now();
                for _ in 0..rounds {
                    for slot in 0..n {
                        let m = p.s.mobs.list.take(slot).expect("mob");
                        contacts.clear();
                        bench_scan(&p, &m, &m.bbox(), *scan, &mut contacts);
                        touched += contacts.len();
                        p.s.mobs.list.put(slot, m);
                    }
                }
                times.push(start.elapsed().as_secs_f64() / rounds as f64);
            }
        }
        std::hint::black_box(touched);
        for (scan, times) in scans.iter().zip(&mut scan_times) {
            let per_tick = median(times);
            println!(
                "[bench] push_scan N={n} {scan:?}: {:.2} ns per mob looked at, {:.1} µs for the scans of one tick",
                per_tick * 1e9 / (n * n) as f64,
                per_tick * 1e6
            );
        }
        let storages = [PushContactStorage::FreshVec, PushContactStorage::KeptBuffer, PushContactStorage::Inline];
        let mut push_times = [Vec::new(), Vec::new(), Vec::new()];
        let mut allocations = [0u64; 3];
        for _pass in 0..7 {
            for (i, storage) in storages.iter().enumerate() {
                let allocations_before = crate::test_alloc::allocations();
                let start = std::time::Instant::now();
                for _ in 0..rounds {
                    for slot in 0..n {
                        let mut m = p.s.mobs.list.take(slot).expect("mob");
                        p.s.mob_push_entities_storing_in(&mut m, *storage);
                        p.s.mobs.list.put(slot, m);
                    }
                }
                push_times[i].push(start.elapsed().as_secs_f64() / rounds as f64);
                allocations[i] = (crate::test_alloc::allocations() - allocations_before) / rounds as u64;
            }
        }
        for (i, storage) in storages.iter().enumerate() {
            println!(
                "[bench] push_contacts N={n} {storage:?}: {:.1} ns per mob, {} allocations per round of {n} pushes",
                median(&mut push_times[i]) * 1e9 / n as f64,
                allocations[i]
            );
        }
    }
}
