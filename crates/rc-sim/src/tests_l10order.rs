//! L10-ORDER: one entity walk for every kind of entity, in the order they joined the world's
//! list (docs/gaps-audit2.md, "Entity tick order"). Before, the walk ticked family by family
//! (falling blocks and TNT, items, mobs, decorations, minecarts, projectiles): an entity of a
//! later family always ticked after one of an earlier family, whatever their order of arrival.
//! Fresh flat world (`tests_g3f13::open`: 26.3 version data mandatory, no test world).

use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;
use crate::tests_g3f13::{finish, open};

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

/// Position and velocity of the only live ground item.
fn item(p: &Probe) -> ([f64; 3], [f64; 3]) {
    let d = p.s.life.drops.iter().find(|d| !d.is_removed()).expect("ground item");
    (d.pos, d.vel)
}

const STAR: &str = "Item:{id:\"minecraft:nether_star\",count:1}";

/// A TNT minecart that explodes on its first tick (fuse 0), summoned BEFORE an item at rest in
/// the air: the minecart ticks first, its blast gives the item its velocity, then the item ticks
/// and moves in that same tick. Before: the items ticked before every minecart, so the item only
/// moved on the next tick.
#[test]
fn tnt_minecart_before_item_moves_it_the_same_tick() {
    let (mut p, dir) = open("l10-cart");
    cmd(&mut p, "summon minecraft:tnt_minecart 2.5 -50 4.5 {NoGravity:1b,fuse:0}");
    cmd(&mut p, &format!("summon minecraft:item 4.5 -50 4.5 {{NoGravity:1b,{STAR}}}"));
    game_ticks(&mut p, 1);
    let (pos, vel) = item(&p);
    assert!(vel[0] > 0.0, "blast received: {vel:?}");
    assert!(pos[0] > 4.5, "the item ticked after the explosion, in the same tick: {pos:?}");
    finish(p, dir);
}

/// An arrow flying at a pig summoned after it: the arrow ticks first and hits the pig where it
/// stands, then the pig ticks (and moves away with its own velocity). Before: the mobs ticked
/// before every projectile, so the pig moved out of the arrow's path first and was missed.
#[test]
fn arrow_hits_a_mob_summoned_after_it_before_it_moves() {
    let (mut p, dir) = open("l10-arrow");
    cmd(&mut p, "summon minecraft:arrow 2.0 -49.6 4.5 {Motion:[1.5d,0.0d,0.0d]}");
    cmd(&mut p, "summon minecraft:pig 3.5 -50 4.5 {NoGravity:1b,Motion:[0.0d,0.0d,1.5d]}");
    let pig = |p: &Probe| p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Pig).map(|m| (m.health, m.pos));
    let (full, _) = pig(&p).expect("pig");
    game_ticks(&mut p, 1);
    let (health, pos) = pig(&p).expect("pig");
    assert!(health < full, "the arrow hit the pig before its move: health {health}, at {pos:?}");
    assert!(pos[2] > 5.0, "the pig moved in its own tick: {pos:?}");
    finish(p, dir);
}

/// An entity that leaves the list (its chunk stops ticking) joins its end again when it comes
/// back: an item frozen in chunk 3 (simulation distance 2), then a TNT minecart summoned in chunk
/// 2 that explodes on its first tick, on the tick chunk 3 starts ticking again. The item is now
/// after the minecart: blast first, then the item's tick, which moves it in that same tick.
/// Before: family order (items first); a rank kept, or renewed but ticked at its old place, would
/// also tick the item before the blast.
#[test]
fn entity_back_in_a_ticking_chunk_ticks_at_the_end() {
    let (mut p, dir) = open("l10-back");
    p.s.sim_dist = 2;
    cmd(&mut p, &format!("summon minecraft:item 48.5 -50 4.5 {{NoGravity:1b,{STAR}}}"));
    // A second item, out of the blast's reach, that never leaves the list.
    cmd(&mut p, &format!("summon minecraft:item 40.5 -50 -12.5 {{NoGravity:1b,{STAR}}}"));
    let back = |p: &Probe| {
        let d = p.s.life.drops.iter().find(|d| !d.is_removed() && d.pos[2] > 0.0).expect("item of chunk 3");
        (d.pos, d.vel)
    };
    game_ticks(&mut p, 1);
    assert_eq!(back(&p).0, [48.5, -50.0, 4.5], "frozen outside the ticking chunks");
    cmd(&mut p, "summon minecraft:tnt_minecart 46.5 -50 4.5 {NoGravity:1b,fuse:0}");
    p.s.sim_dist = 3;
    game_ticks(&mut p, 1);
    let (pos, vel) = back(&p);
    assert!(vel[0] > 0.0, "blast received: {vel:?}");
    assert!(pos[0] > 48.5, "the item, back at the end of the list, ticked after the blast: {pos:?}");
    // The item list is in rank order again, the item that came back last (the merged walk
    // relies on that order).
    let ranks: Vec<u64> = p.s.life.drops.iter().map(|d| d.rank & !1).collect();
    assert!(ranks.is_sorted(), "item list out of rank order: {ranks:?}");
    assert!(p.s.life.drops.last().is_some_and(|d| d.pos[2] > 0.0), "the item back in its chunk is last");
    finish(p, dir);
}

/// The two ways of ticking items and orbs, a run with their list taken out (plates reading the
/// positions kept up to date) or one by one out of their slot (plates reading the list and the
/// one being ticked), give the same world: 24 items and 24 orbs falling on weighted plates that
/// count them while they move; plate states every tick and final positions identical. The
/// sabotage check: the plates must have been pressed and released.
#[test]
fn item_runs_and_one_by_one_give_the_same_world() {
    let run = |one: bool| {
        crate::drops::ONE_BY_ONE.with(|c| c.set(one));
        let (mut p, dir) = open(if one { "l10-one" } else { "l10-run" });
        cmd(&mut p, "fill 8 -60 8 11 -60 11 minecraft:light_weighted_pressure_plate");
        for k in 0..24 {
            let (x, z) = (8.2 + f64::from(k % 6) * 0.6, 8.2 + f64::from(k / 6) * 0.9);
            cmd(&mut p, &format!("summon minecraft:item {x} -57 {z} {{Item:{{id:\"minecraft:stone\",count:1}}}}"));
            cmd(&mut p, &format!("summon minecraft:experience_orb {x} -56 {z} {{Value:3}}"));
        }
        let mut trace = Vec::new();
        for _ in 0..60 {
            game_ticks(&mut p, 1);
            for (x, z) in (8..12).flat_map(|x| (8..12).map(move |z| (x, z))) {
                trace.push(crate::blocks::probe::state_string(&p.s, p.s.get([x, -60, z])));
            }
        }
        let items: Vec<_> = p.s.life.drops.iter().map(|d| (d.pos, d.stack.count)).collect();
        let orbs: Vec<_> = p.s.life.orbs.iter().map(|o| (o.pos, o.count)).collect();
        crate::drops::ONE_BY_ONE.with(|c| c.set(false));
        finish(p, dir);
        (trace, items, orbs)
    };
    // Each in a fresh thread: the "clock" random sources start from the same test seed there.
    let fresh = |one: bool| std::thread::spawn(move || run(one)).join().expect("simulation");
    let (runs, one) = (fresh(false), fresh(true));
    assert!(runs.0.iter().any(|s| !s.contains("power=0")), "no plate pressed");
    assert!(runs.0.iter().any(|s| s.contains("power=0")), "every plate pressed all along");
    assert_eq!(runs.0, one.0, "plate states");
    assert_eq!(runs.1, one.1, "items");
    assert_eq!(runs.2, one.2, "orbs");
}

/// Two items of the same family on both sides of a TNT in the order of arrival (item, TNT with
/// fuse 1, item), all at rest in the air: the first item ticks before the blast (no move that
/// tick), the second after it (it moves).
#[test]
fn walk_interleaves_families_by_rank() {
    let (mut p, dir) = open("l10-interleave");
    cmd(&mut p, &format!("summon minecraft:item 4.5 -50 4.5 {{NoGravity:1b,{STAR}}}"));
    cmd(&mut p, "summon minecraft:tnt 2.5 -50.5 4.5 {fuse:1s}");
    cmd(&mut p, &format!("summon minecraft:item 0.5 -50 4.5 {{NoGravity:1b,{STAR}}}"));
    game_ticks(&mut p, 1);
    let live: Vec<_> = p.s.life.drops.iter().filter(|d| !d.is_removed()).map(|d| (d.pos, d.vel)).collect();
    assert_eq!(live.len(), 2, "two stars");
    let (first, second) = (live[0], live[1]);
    // Along x only: our ground items still fall despite `NoGravity` (separate gap).
    assert!(first.1[0] > 0.0 && first.0[0] == 4.5, "first item: blast, no move yet: {first:?}");
    assert!(second.1[0] < 0.0 && second.0[0] < 0.5, "second item: blast then its move: {second:?}");
    finish(p, dir);
}
