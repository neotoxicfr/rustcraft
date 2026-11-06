//! Equivalence proofs of the animal scans (fox, panda, dolphin, pufferfish, nautilus, shulker) on a
//! fresh flat world (26.3 version data mandatory): a pool, a mixed crowd and 200 items on the
//! ground. Every new path gives the same choice, in the same list order, and leaves the mob and
//! the nautilus brain in the same state as the old path; every indexed species read of the ticks
//! is checked against the scan (`index::verify`). Micro-benches ignored (`--ignored --nocapture`).

use crate::blocks::probe::Probe;
use crate::mobs::ground_items::{first_of, in_pickup_reach};
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::sealife::{dolphin, fish, nautilus};
use crate::mobs::species::Kind;
use crate::mobs::wild::{fox, panda};
use crate::player::Stack;
use crate::server::Server;
use crate::tests_g3f13::{finish, open};

/// Deterministic draws (xorshift): positions and items never depend on the clock.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn within(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const ITEMS: [&str; 8] = [
    "minecraft:sweet_berries",
    "minecraft:bamboo",
    "minecraft:cake",
    "minecraft:cod",
    "minecraft:stone",
    "minecraft:chicken",
    "minecraft:apple",
    "minecraft:glow_berries",
];

/// Surface of the flat world (feet height of a land mob) and the pool dug in it.
const LAND_Y: f64 = -60.0;
const WATER_Y: f64 = -62.5;

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn add(s: &mut Server, kind: Kind, pos: [f64; 3], setup: impl FnOnce(&mut Mob)) -> i32 {
    let mut m = s.new_mob(kind, pos, 0.0);
    m.persistent = true;
    setup(&mut m);
    let id = m.id;
    s.add_mob(m);
    id
}

fn drop_item(s: &mut Server, r: &mut Draw, pos: [f64; 3]) {
    let stack = Stack { id: ITEMS[r.below(ITEMS.len())].into(), count: 1 + r.below(3) as i32, components: None };
    let delay = if r.below(4) == 0 { 10 } else { 0 };
    s.spawn_item(pos, [0.0; 3], stack, delay);
}

/// Land crowd west of the pool, water crowd in it, items around the pickers and scattered.
fn populate(s: &mut Server) {
    let mut r = Draw(0x9E37_79B9_7F4A_7C15);
    let land = |r: &mut Draw| [r.within(-28.0, 4.0), LAND_Y, r.within(-28.0, 28.0)];
    let water = |r: &mut Draw| [r.within(10.0, 38.0), WATER_Y, r.within(-6.0, 6.0)];
    let chickens: Vec<i32> = (0..10).map(|_| add(s, Kind::Chicken, land(&mut r), |_| {})).collect();
    let chicken_uuids: Vec<u128> = chickens.iter().filter_map(|&id| s.mob_ref(id).map(|m| m.uuid)).collect();
    for (k, n) in [(Kind::Rabbit, 4), (Kind::Zombie, 6), (Kind::Wolf, 4), (Kind::Villager, 2), (Kind::Cow, 6)] {
        for _ in 0..n {
            add(s, k, land(&mut r), |_| {});
        }
    }
    let mut pickers = Vec::new();
    for i in 0..16 {
        let trusted = chicken_uuids[i % chicken_uuids.len()];
        let at = land(&mut r);
        pickers.push((at, 0.1));
        add(s, Kind::Fox, at, |m| {
            if i % 2 == 0 {
                fox::add_trusted(m, trusted);
            }
            // A third hold an inedible item: only food then replaces it.
            if i % 3 == 0 {
                m.equipment[0] = Some(Stack { id: "minecraft:stone".into(), count: 1, components: None });
                m.wild.since_eaten = 5;
            }
        });
    }
    for _ in 0..12 {
        let at = land(&mut r);
        pickers.push((at, 0.1));
        add(s, Kind::Panda, at, |_| {});
    }
    for _ in 0..10 {
        let at = water(&mut r);
        pickers.push((at, 0.1));
        add(s, Kind::Dolphin, at, |_| {});
    }
    for (k, n) in [(Kind::Pufferfish, 8), (Kind::Cod, 12), (Kind::Turtle, 2), (Kind::Drowned, 3), (Kind::Axolotl, 1)] {
        for _ in 0..n {
            add(s, k, water(&mut r), |_| {});
        }
    }
    for i in 0..8 {
        add(s, Kind::Nautilus, water(&mut r), |m| {
            if i % 2 == 0 {
                m.in_love = 600;
            }
        });
    }
    // Two to four items in the pickup reach of each picker, then the rest scattered.
    let mut n = 0;
    for &(at, dy) in &pickers {
        for _ in 0..2 + r.below(3) {
            let pos = [at[0] + r.within(-1.1, 1.1), at[1] + dy, at[2] + r.within(-1.1, 1.1)];
            drop_item(s, &mut r, pos);
            n += 1;
        }
    }
    while n < 200 {
        let at = if r.below(3) == 0 { water(&mut r) } else { land(&mut r) };
        drop_item(s, &mut r, [at[0], at[1] + 0.1, at[2]]);
        n += 1;
    }
}

fn open_scene(tag: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = open(tag);
    cmd(&mut p, "fill 8 -63 -8 40 -61 8 minecraft:water");
    populate(&mut p.s);
    (p, dir)
}

fn slots_of(s: &Server, kinds: &[Kind]) -> Vec<usize> {
    (0..s.mobs.list.len()).filter(|&i| s.mobs.list.get(i).is_some_and(|m| kinds.contains(&m.kind))).collect()
}

fn inflate(b: rc_physics::Aabb, x: f64, y: f64) -> rc_physics::Aabb {
    rc_physics::Aabb { min: [b.min[0] - x, b.min[1] - y, b.min[2] - x], max: [b.max[0] + x, b.max[1] + y, b.max[2] + x] }
}

/// Counts of an old/new comparison: (results found, results empty).
#[derive(Default, Debug)]
struct Seen(usize, usize);

impl Seen {
    fn note(&mut self, found: bool) {
        if found { self.0 += 1 } else { self.1 += 1 }
    }
}

/// Pickups and item searches of the fox, panda and dolphin: same item as the old path.
fn compare_item_choices(s: &Server) -> [Seen; 4] {
    let mut seen: [Seen; 4] = Default::default();
    for i in slots_of(s, &[Kind::Fox, Kind::Panda, Kind::Dolphin]) {
        let m = s.mobs.list.get(i).expect("listed mob");
        let (choice, nearest) = match m.kind {
            Kind::Fox => (
                Some((fox::pickup_choice(s, m, true), fox::pickup_choice(s, m, false))),
                (first_of(fox::items_near(s, m), true), first_of(fox::items_near(s, m), false)),
            ),
            Kind::Panda => (
                Some((panda::pickup_choice(s, m, true), panda::pickup_choice(s, m, false))),
                (first_of(panda::ground_food(s, m, 6.0), true), first_of(panda::ground_food(s, m, 6.0), false)),
            ),
            _ => (None, (first_of(dolphin::items_near(s, m), true), first_of(dolphin::items_near(s, m), false))),
        };
        if let Some((new, old)) = choice {
            assert_eq!(new, old, "{:?} {}: other item picked up", m.kind, m.id);
            seen[0].note(new.is_some());
            // The first item in reach is not always the one taken (fox holding stone, panda tag).
            let first_in_reach = s.life.drops.iter().find(|d| in_pickup_reach(d, &m.bbox())).map(|d| d.id);
            if new.is_some() && new.as_ref().map(|c| c.0) != first_in_reach {
                seen[1].0 += 1;
            }
        }
        assert_eq!(nearest.0, nearest.1, "{:?} {}: other item searched", m.kind, m.id);
        seen[if m.kind == Kind::Dolphin { 3 } else { 2 }].note(nearest.0.is_some());
    }
    seen
}

/// Pufferfish: the puffing test and the stung mobs, as held or looked up again.
fn compare_pufferfish(s: &Server) -> Seen {
    let mut seen = Seen::default();
    for i in slots_of(s, &[Kind::Pufferfish]) {
        let b = s.mobs.list.get(i).expect("listed mob").bbox();
        for grow in [0.3, 2.0, 6.0] {
            let area = inflate(b, grow, grow);
            let new = fish::scary_mob_near(s, &area, true);
            assert_eq!(new, fish::scary_mob_near(s, &area, false), "puffing test differs at ±{grow}");
            assert_eq!(fish::stung_mobs(s, &area, true), fish::stung_mobs(s, &area, false), "stung mobs differ at ±{grow}");
            seen.note(new);
        }
    }
    seen
}

/// Mob taken out of the list, two copies run through `f` with the new then the old path: same
/// result, same mob state afterwards (sight caches included); the original goes back unchanged.
fn compare_taken<R: PartialEq + std::fmt::Debug>(
    s: &mut Server,
    i: usize,
    f: impl Fn(&Server, &mut Mob, bool) -> R,
) -> R {
    let m = s.mobs.list.take(i).expect("listed mob");
    let (mut a, mut b) = (m.clone(), m.clone());
    let new = f(s, &mut a, true);
    let old = f(s, &mut b, false);
    assert_eq!(new, old, "{:?} {}: other result", m.kind, m.id);
    assert_eq!(format!("{a:?}"), format!("{b:?}"), "{:?} {}: other mob state", m.kind, m.id);
    s.mobs.list.put(i, m);
    new
}

/// Fox alert in one pass, nautilus charge stopped at the first mob.
fn compare_alerts_and_charges(s: &mut Server) -> (Seen, Seen) {
    let (mut alert, mut charge) = (Seen::default(), Seen::default());
    for i in slots_of(s, &[Kind::Fox]) {
        for (x, y) in [(1.0, 1.0), (4.0, 2.0), (12.0, 6.0)] {
            let found = compare_taken(s, i, |s, m, one| fox::alertable_mob_near(s, m, &inflate(m.bbox(), x, y), one));
            alert.note(found);
        }
    }
    for i in slots_of(s, &[Kind::Nautilus, Kind::Dolphin, Kind::Pufferfish]) {
        for grow in [0.5, 2.0, 6.0] {
            let found = compare_taken(s, i, |s, m, one| s.charged_mob(m, &inflate(m.bbox(), grow, grow), one));
            charge.note(found.is_some());
        }
    }
    (alert, charge)
}

/// Nautilus brain: the closest visible entity accepted by a filter, list walked in place or
/// copied; same answer, same verdicts kept in the brain, same mob.
fn compare_visible(s: &mut Server) -> Seen {
    let mut seen = Seen::default();
    let filters: [&dyn Fn(&Server, Ent) -> bool; 4] = [
        &|_, _| true,
        &|_, e| e == Ent::Player,
        &|s, e| matches!(e, Ent::Mob(id) if s.mob_ref(id).is_some_and(|o| o.kind == Kind::Pufferfish)),
        &|s, e| matches!(e, Ent::Mob(id) if s.mob_ref(id).is_some_and(|o| o.kind == Kind::Nautilus && o.in_love > 0)),
    ];
    for i in slots_of(s, &[Kind::Nautilus]) {
        let mut m = s.mobs.list.take(i).expect("listed mob");
        let brain = m.sea.brain.take().expect("nautilus brain");
        for f in filters {
            let (mut ma, mut mb, mut ba, mut bb) = (m.clone(), m.clone(), (*brain).clone(), (*brain).clone());
            let new = s.nautilus_visible_closest(&mut ma, &mut ba, f, true);
            let old = s.nautilus_visible_closest(&mut mb, &mut bb, f, false);
            assert_eq!(new, old, "nautilus {}: other visible entity", m.id);
            assert_eq!(format!("{ba:?}"), format!("{bb:?}"), "nautilus {}: other kept verdicts", m.id);
            assert_eq!(format!("{ma:?}"), format!("{mb:?}"), "nautilus {}: other mob state", m.id);
            seen.note(new.is_some());
        }
        m.sea.brain = Some(brain);
        s.mobs.list.put(i, m);
    }
    seen
}

/// A path restarted from a moved path leaves the same navigation as from a copied one.
fn compare_path_restart(s: &mut Server) -> usize {
    let mut compared = 0;
    for i in slots_of(s, &[Kind::Nautilus, Kind::Dolphin]) {
        let mut m = s.mobs.list.take(i).expect("listed mob");
        let at = m.block_pos();
        let target = [(at[0] + 6).min(38), -62, at[2].clamp(-6, 6) - 3];
        if let Some(path) = s.create_path(&mut m, target, 0) {
            m.nav.path = Some(path);
            let (mut moved, mut copied) = (m.clone(), m.clone());
            let p = moved.nav.path.take();
            let started = s.move_to_path(&mut moved, p, 1.0);
            let p = copied.nav.path.clone();
            assert_eq!(started, s.move_to_path(&mut copied, p, 1.0), "mob {}: other path start", m.id);
            assert_eq!(format!("{:?}", moved.nav), format!("{:?}", copied.nav), "mob {}: other navigation", m.id);
            compared += 1;
        }
        s.mobs.list.put(i, m);
    }
    compared
}

#[test]
fn animal_scans_match_the_old_paths() {
    let (mut p, dir) = open_scene("animal-scans");
    let start = crate::mobs::index::stats::kind_queries();
    let items = compare_item_choices(&p.s);
    eprintln!("items at rest: pickups {:?}, not the first in reach {}, searches {:?}, dolphin {:?}", items[0], items[1].0, items[2], items[3]);
    assert!(items[0].0 >= 10 && items[0].1 > 0, "pickups found and missed: {:?}", items[0]);
    assert!(items[1].0 > 0, "no pickup where the first item in reach is refused: scene to fix");
    assert!(items[2].0 > 0 && items[3].0 > 0, "item searches without a hit");
    // Sensors, pickups, pushes and puffing run; brains fill their visible lists.
    game_ticks(&mut p, 45);
    let puff = compare_pufferfish(&p.s);
    let (alert, charge) = compare_alerts_and_charges(&mut p.s);
    let visible = compare_visible(&mut p.s);
    let paths = compare_path_restart(&mut p.s);
    let items = compare_item_choices(&p.s);
    eprintln!("after 45 ticks: puff {puff:?}, alert {alert:?}, charge {charge:?}, visible {visible:?}, paths {paths}, pickups {:?}", items[0]);
    for (what, seen) in [("puffing", &puff), ("fox alert", &alert), ("charge", &charge), ("visible", &visible)] {
        assert!(seen.0 > 0 && seen.1 > 0, "{what}: both outcomes needed, seen {seen:?}");
    }
    assert!(paths > 0, "no path restart compared");
    game_ticks(&mut p, 100);
    let (served, scanned) = crate::mobs::index::stats::kind_queries();
    eprintln!("species queries over the run: {} served by the index, {} sent to the scan", served - start.0, scanned - start.1);
    finish(p, dir);
}

/// The partner test reads only three fields: same answer as the rule of the game on every case
/// (distinct, other nautilus, both in love, both adults), and as the old copy of the whole mob.
#[test]
fn nautilus_partner_test_reads_the_right_fields() {
    let (mut p, dir) = open("nautilus-suitor");
    let s = &mut p.s;
    let mut cases = 0;
    for (kind, love_a, love_b, age_a, age_b) in partner_cases() {
        let mut a = s.new_mob(Kind::Nautilus, [0.5, LAND_Y, 0.5], 0.0);
        let mut b = s.new_mob(kind, [1.5, LAND_Y, 0.5], 0.0);
        (a.in_love, b.in_love, a.age, b.age) = (love_a, love_b, age_a, age_b);
        let rule = kind == Kind::Nautilus && love_a > 0 && love_b > 0 && age_a == 0 && age_b == 0;
        assert_eq!(nautilus::Suitor::of(&a).can_mate(&b), rule, "{kind:?} love {love_a}/{love_b} age {age_a}/{age_b}");
        assert_eq!(nautilus::Suitor::of(&a.clone()).can_mate(&b), rule);
        assert!(!nautilus::Suitor::of(&a).can_mate(&a), "a nautilus mates with itself");
        cases += 1;
    }
    assert_eq!(cases, 2 * 2 * 2 * 3 * 3);
    finish(p, dir);
}

fn partner_cases() -> Vec<(Kind, i32, i32, i32, i32)> {
    let mut v = Vec::new();
    for kind in [Kind::Nautilus, Kind::ZombieNautilus] {
        for love_a in [0, 600] {
            for love_b in [0, 1] {
                for age_a in [0, -24000, 6000] {
                    for age_b in [0, -1, 6000] {
                        v.push((kind, love_a, love_b, age_a, age_b));
                    }
                }
            }
        }
    }
    v
}

/// A birth from a love the player caused counts `animals_bred` once.
#[test]
fn nautilus_birth_counts_one_breeding() {
    let (mut p, dir) = open("nautilus-birth");
    let s = &mut p.s;
    let partner = add(s, Kind::Nautilus, [1.5, LAND_Y, 0.5], |o| o.in_love = 600);
    let mut m = s.new_mob(Kind::Nautilus, [0.5, LAND_Y, 0.5], 0.0);
    (m.in_love, m.love_cause) = (600, true);
    let before = s.prog.stats.get("minecraft:custom", "minecraft:animals_bred");
    let mobs_before = s.mobs.list.len();
    s.nautilus_breed(&mut m, partner);
    assert_eq!(s.prog.stats.get("minecraft:custom", "minecraft:animals_bred") - before, 1, "breedings counted");
    assert_eq!(s.mobs.list.len(), mobs_before + 1, "no baby");
    assert_eq!((m.in_love, m.love_cause, m.age), (0, false, 6000));
    finish(p, dir);
}

/// The dolphin takes the first item in reach, whole stack in the mouth.
#[test]
fn dolphin_takes_the_first_item_in_reach() {
    let (mut p, dir) = open("dolphin-pickup");
    cmd(&mut p, "fill 8 -63 -8 40 -61 8 minecraft:water");
    let s = &mut p.s;
    let at = [20.5, WATER_Y, 0.5];
    let d = add(s, Kind::Dolphin, at, |m| m.can_pickup = true);
    let stone = |n| Stack { id: "minecraft:stone".into(), count: n, components: None };
    s.spawn_item([at[0] + 3.0, at[1] + 0.1, at[2]], [0.0; 3], stone(9), 0); // out of reach
    s.spawn_item([at[0] + 0.5, at[1] + 0.1, at[2]], [0.0; 3], Stack { id: "minecraft:cod".into(), count: 2, components: None }, 0);
    s.spawn_item([at[0] - 0.5, at[1] + 0.1, at[2]], [0.0; 3], stone(3), 0);
    let i = s.mob_index(d).expect("dolphin");
    let mut m = s.mobs.list.take(i).expect("dolphin");
    dolphin::pickup(s, &mut m);
    let held = m.equipment[0].clone().expect("item picked up");
    assert_eq!((held.id.as_str(), held.count), ("minecraft:cod", 2));
    s.mobs.list.put(i, m);
    finish(p, dir);
}

// ---------- Micro-benches (quiet machine; `--ignored --nocapture`) ----------

fn per_call_ns(n: u32, mut f: impl FnMut() -> usize) -> f64 {
    let mut sink = 0;
    let t = std::time::Instant::now();
    for _ in 0..n {
        sink += f();
    }
    let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
    std::hint::black_box(sink);
    ns
}

/// The player's pufferfish contact scan at 1,000 mobs and no pufferfish: species index against
/// the filtered walk of the whole list.
#[test]
#[ignore = "measurement bench"]
fn bench_pufferfish_contact_scan() {
    let (mut p, dir) = open("bench-puff");
    let mut r = Draw(7);
    for i in 0..1000 {
        let k = [Kind::Cow, Kind::Zombie, Kind::Pig, Kind::Cod][i % 4];
        add(&mut p.s, k, [r.within(-28.0, 150.0), LAND_Y, r.within(-28.0, 40.0)], |_| {});
    }
    crate::mobs::index::set_verify(false);
    let s = &p.s;
    let area = inflate(s.player_aabb(), 1.0, 0.5);
    let walk = per_call_ns(20_000, || {
        s.mobs.list.iter().filter(|o| o.kind == Kind::Pufferfish && !o.removed && o.touches(&area)).count()
    });
    let index = per_call_ns(20_000, || s.mobs.of_kind(Kind::Pufferfish).filter(|o| !o.removed && o.touches(&area)).count());
    eprintln!("bench_pufferfish_contact_scan N=1000: walk {walk:.0} ns, species index {index:.0} ns per tick");
    crate::mobs::index::set_verify(true);
    finish(p, dir);
}

/// Pickup choice and item search with 200 items on the ground, per call.
#[test]
#[ignore = "measurement bench"]
fn bench_pickups_with_200_items() {
    let (p, dir) = open_scene("bench-pickups");
    crate::mobs::index::set_verify(false);
    let s = &p.s;
    for k in [Kind::Fox, Kind::Panda, Kind::Dolphin] {
        let slots = slots_of(s, &[k]);
        let mobs: Vec<&Mob> = slots.iter().filter_map(|&i| s.mobs.list.get(i)).collect();
        let run = |first: bool| {
            per_call_ns(2_000, || {
                mobs.iter()
                    .map(|m| match k {
                        Kind::Fox => fox::pickup_choice(s, m, first).is_some() as usize + first_of(fox::items_near(s, m), first).is_some() as usize,
                        Kind::Panda => panda::pickup_choice(s, m, first).is_some() as usize + first_of(panda::ground_food(s, m, 8.0), first).is_some() as usize,
                        _ => first_of(dolphin::items_near(s, m), first).is_some() as usize,
                    })
                    .sum()
            }) / mobs.len() as f64
        };
        let (new, old) = (run(true), run(false));
        eprintln!("bench_pickups {k:?} (200 items): old {old:.0} ns, new {new:.0} ns per mob");
    }
    crate::mobs::index::set_verify(true);
    finish(p, dir);
}

/// The nautilus partner test set up: a copy of three fields against a copy of the whole mob.
#[test]
#[ignore = "measurement bench"]
fn bench_nautilus_suitor_copy() {
    let (mut p, dir) = open("bench-suitor");
    let m = p.s.new_mob(Kind::Nautilus, [0.5, LAND_Y, 0.5], 0.0);
    let o = m.clone();
    let fields = per_call_ns(200_000, || nautilus::Suitor::of(std::hint::black_box(&m)).can_mate(&o) as usize);
    let whole = per_call_ns(200_000, || nautilus::Suitor::of(&std::hint::black_box(m.clone())).can_mate(&o) as usize);
    eprintln!("bench_nautilus_suitor_copy: whole mob {whole:.0} ns, three fields {fields:.1} ns");
    finish(p, dir);
}
