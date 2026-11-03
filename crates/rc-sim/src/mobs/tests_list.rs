//! Proof of the mob list API (`MobList` and the `Mobs` methods that change its slots): random
//! sequences of every operation the server uses, mirrored on a plain vector of slots (the list as it
//! was before the API), compared after each step. Without game data.

use super::Mobs;
use super::jrand::JRand;
use super::mob::Mob;
use super::species::Kind;

/// Small deterministic random of the test.
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
}

fn mob(id: i32) -> Box<Mob> {
    let kind = [Kind::Cow, Kind::Zombie, Kind::Sheep][id as usize % 3];
    Box::new(Mob::new(kind, id, 0x1000 + id as u128, [f64::from(id), 64.0, 0.0], 0.0, JRand::new(1)))
}

/// What the test follows of a mob: identifier, health (changed in place), rank (sorted on).
type Seen = (i32, f32, u64);

fn seen_of(m: &Mob) -> Seen {
    (m.id, m.health, m.rank)
}

/// Every read of the API agrees with the reference slots.
fn check(mobs: &Mobs, model: &[Option<Seen>], step: &str) {
    let slots: Vec<Option<Seen>> = mobs.list.slots().iter().map(|s| s.as_deref().map(seen_of)).collect();
    assert_eq!(slots, model, "{step}: slots");
    assert_eq!(mobs.list.len(), model.len(), "{step}: len");
    let want: Vec<i32> = model.iter().flatten().map(|s| s.0).collect();
    let walked: Vec<i32> = mobs.list.iter().map(|m| m.id).collect();
    assert_eq!(walked, want, "{step}: iter skips the empty slots, in order");
    let backwards: Vec<i32> = mobs.list.iter().rev().map(|m| m.id).collect();
    assert!(backwards.iter().eq(want.iter().rev()), "{step}: iter backwards");
    for i in 0..=model.len() {
        let want = model.get(i).copied().flatten().map(|s| s.0);
        assert_eq!(mobs.list.get(i).map(|m| m.id), want, "{step}: get({i})");
    }
}

#[test]
fn list_api_matches_a_plain_vector_of_slots() {
    let mut r = Rng(0x5107_5A11);
    for round in 0..300 {
        let mut mobs = Mobs::default();
        let mut model: Vec<Option<Seen>> = Vec::new();
        // Mobs taken out, with their slot; forgotten when the slots shift (as the server never
        // keeps a slot number across a removal).
        let mut out: Vec<(usize, Box<Mob>)> = Vec::new();
        let mut next = 1;
        for step in 0..120 {
            let n = model.len();
            let what = match r.below(10) {
                0 | 1 => {
                    mobs.push(mob(next));
                    next += 1;
                    model.push(mobs.list.get(n).map(seen_of));
                    "push"
                }
                2 if n > 0 => {
                    let i = r.below(n as u64) as usize;
                    let got = mobs.list.take(i);
                    assert_eq!(got.as_deref().map(seen_of), model[i].take(), "take({i})");
                    out.extend(got.map(|m| (i, m)));
                    "take"
                }
                3 => {
                    if let Some((i, m)) = out.pop() {
                        model[i] = Some(seen_of(&m));
                        mobs.list.put(i, m);
                    }
                    "put"
                }
                4 => {
                    if let Some((i, m)) = out.pop() {
                        let s = seen_of(&m);
                        // Past the end the mob is handed back; in range it is put back.
                        let beyond = n + r.below(3) as usize;
                        let m = mobs.list.try_put(beyond, m).expect_err("slot past the end");
                        assert_eq!(seen_of(&m), s);
                        assert!(mobs.list.try_put(i, m).is_ok(), "try_put({i}) refused");
                        model[i] = Some(s);
                    }
                    "try_put"
                }
                5 if n > 0 => {
                    let i = r.below(n as u64) as usize;
                    if let Some(m) = mobs.list.get_mut(i) {
                        m.health += 1.0;
                    }
                    if let Some(s) = &mut model[i] {
                        s.1 += 1.0;
                    }
                    for m in mobs.list.iter_mut().filter(|m| m.id % 5 == 0) {
                        m.health += 2.0;
                    }
                    model.iter_mut().flatten().filter(|s| s.0 % 5 == 0).for_each(|s| s.1 += 2.0);
                    "get_mut, iter_mut"
                }
                6 if n > 0 => {
                    out.clear();
                    let i = r.below(n as u64) as usize;
                    let got = mobs.remove_slot(i);
                    assert_eq!(got.as_deref().map(seen_of), model.remove(i), "remove_slot({i})");
                    "remove_slot"
                }
                7 => {
                    out.clear();
                    mobs.drop_empty_slots();
                    model.retain(Option::is_some);
                    "drop_empty_slots"
                }
                8 if n > 0 => {
                    out.clear();
                    // A mob back with an older rank, then the list put back in rank order (stable,
                    // empty slots first).
                    let i = r.below(n as u64) as usize;
                    if let Some(m) = mobs.list.get_mut(i) {
                        m.rank = r.below(m.rank.max(1));
                        model[i] = Some(seen_of(m));
                    }
                    mobs.sort_by_rank();
                    model.sort_by_key(|s| s.map_or(0, |s| s.2));
                    "sort_by_rank"
                }
                _ => "nothing",
            };
            check(&mobs, &model, &format!("round {round} step {step} ({what})"));
        }
    }
}

/// The species walks, read-only and mutable, give the mobs of the plain walk in its order.
#[test]
fn species_walks_follow_the_slots() {
    let mut mobs = Mobs::default();
    for id in 1..=30 {
        mobs.push(mob(id));
    }
    for i in [3, 4, 17] {
        mobs.list.take(i);
    }
    let farm = [Kind::Cow, Kind::Sheep];
    let want: Vec<i32> = mobs.list.iter().filter(|m| farm.contains(&m.kind)).map(|m| m.id).collect();
    assert_eq!(want.len(), 18, "20 farm animals, 2 of them taken out");
    assert_eq!(mobs.of_kinds(farm).map(|m| m.id).collect::<Vec<_>>(), want);
    assert_eq!(mobs.of_kinds_mut(farm).map(|m| m.id).collect::<Vec<_>>(), want);
}
