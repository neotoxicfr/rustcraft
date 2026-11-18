//! Ground items and orbs, both ways of each switch (`drops::DropSwitches`): a pickup stored
//! straight into the inventory, bodies of removed items and orbs recycled for the next ones, the
//! size of their hot structures. Fresh flat world (`tests_g3f13::open`: 26.3 version data
//! mandatory, no test world).

use crate::blocks::probe::Probe;
use crate::drops::DropSwitches;
use crate::player::Stack;
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: format!("minecraft:{id}"), count, components: None }
}

/// The 46 slots of the player's menu with the 36 storage slots and the off hand (9 to 45) filled
/// with `fill`, then the slots of `set` replaced.
fn inventory(fill: Option<Stack>, set: &[(usize, Option<Stack>)]) -> Vec<Option<Stack>> {
    let mut slots: Vec<Option<Stack>> = (0..46).map(|k| if k >= 9 { fill.clone() } else { None }).collect();
    for (k, s) in set {
        slots[*k] = s.clone();
    }
    slots
}

/// A pickup stored straight into the inventory gives what the trial on a copy of the slots gave:
/// same stack left on the ground, same slots, for an empty inventory, a full one, a partial
/// stack, the creative mode and a worn item. Sabotage check: one case leaves part of the stack
/// (the inventory's own answer, "something was stored", would lose it) and one leaves all of it.
#[test]
fn pickup_in_place_stores_like_the_trial_on_a_copy() {
    let (mut p, dir) = open("pickup");
    let mut worn = Stack { id: "minecraft:iron_sword".into(), count: 1, components: None };
    crate::items::set_component(&mut worn, "minecraft:damage", rc_proto::nbt::Nbt::Int(5));
    let dirt = Some(stack("dirt", 64));
    let cases = [
        ("empty inventory", inventory(None, &[]), stack("stone", 10), false),
        ("full inventory", inventory(dirt.clone(), &[]), stack("stone", 10), false),
        ("partial stack", inventory(dirt.clone(), &[(20, Some(stack("stone", 60)))]), stack("stone", 10), false),
        (
            "two partial stacks",
            inventory(dirt.clone(), &[(20, Some(stack("stone", 60))), (40, Some(stack("stone", 63)))]),
            stack("stone", 10),
            false,
        ),
        ("creative, full", inventory(dirt.clone(), &[]), stack("stone", 10), true),
        ("worn item, free slot", inventory(dirt.clone(), &[(12, None)]), worn.clone(), false),
        ("worn item, full", inventory(dirt.clone(), &[]), worn, false),
    ];
    let pickup = |p: &mut Probe, switches: DropSwitches, slots: &[Option<Stack>], s: &Stack, creative: bool| {
        crate::drops::FORCED_SWITCHES.with(|c| c.set(Some(switches)));
        p.s.slots = slots.to_vec();
        let rest = p.s.pickup_into_inventory(s.clone(), creative, 0);
        crate::drops::FORCED_SWITCHES.with(|c| c.set(None));
        (rest.map(|r| (r.id, r.count)), p.s.slots.clone())
    };
    let in_place = DropSwitches { pickup_in_place: true, ..DropSwitches::ALL_OFF };
    let mut lefts = Vec::new();
    for (name, slots, s, creative) in cases {
        let trial = pickup(&mut p, DropSwitches::ALL_OFF, &slots, &s, creative);
        let direct = pickup(&mut p, in_place, &slots, &s, creative);
        assert_eq!(direct, trial, "{name}");
        lefts.push(trial.0.map_or(0, |r| r.1));
    }
    assert!(lefts.contains(&6) && lefts.contains(&10), "partial and whole stacks left: {lefts:?}");
    finish(p, dir);
}

/// Items and orbs that expire (age 5,999) leave their bodies to those born next: with the pool on
/// or off, 30 items and 30 orbs dropped from several heights onto the floor, an ice path and
/// water move the same way and send the same packets every tick. Sabotage check: the pool was
/// filled, then used.
#[test]
fn recycled_bodies_move_like_new_ones() {
    let run = |pool: bool| {
        crate::drops::FORCED_SWITCHES.with(|c| c.set(Some(DropSwitches { body_pool: pool, ..DropSwitches::ALL_ON })));
        let (mut p, dir) = open(&format!("bodies-{pool}"));
        cmd(&mut p, "fill 4 -61 10 12 -61 10 minecraft:blue_ice");
        cmd(&mut p, "fill 4 -60 14 8 -60 16 minecraft:water");
        for k in 0..30 {
            let (x, z) = (4.5 + f64::from(k % 6), 10.5 + f64::from(k / 6) * 1.5);
            let motion = format!("Motion:[{:.2}d,0.3d,0.1d]", f64::from(k % 5) * 0.1);
            cmd(
                &mut p,
                &format!(
                    "summon minecraft:item {x} -50 {z} {{Age:5999s,{motion},Item:{{id:\"minecraft:stone\",count:1}}}}"
                ),
            );
            cmd(&mut p, &format!("summon minecraft:experience_orb {x} -48 {z} {{Age:5999s,Value:3}}"));
        }
        for _ in 0..3 {
            p.s.game_tick();
            p.s.tick += 1;
        }
        let pooled = p.s.walk.spare_bodies.len();
        let _ = p.take_events();
        for k in 0..30 {
            let (x, z) = (4.5 + f64::from(k % 6) * 1.3, 10.2 + f64::from(k / 6) * 1.3);
            let y = -58.0 + f64::from(k % 4);
            let motion = format!("Motion:[{:.2}d,0.0d,0.05d]", f64::from(k % 3) * 0.15);
            cmd(
                &mut p,
                &format!("summon minecraft:item {x} {y} {z} {{{motion},Item:{{id:\"minecraft:dirt\",count:1}}}}"),
            );
            cmd(&mut p, &format!("summon minecraft:experience_orb {x} {} {z} {{Value:7}}", y + 0.5));
        }
        let left = p.s.walk.spare_bodies.len();
        let mut seen = Vec::new();
        for _ in 0..60 {
            p.s.game_tick();
            p.s.tick += 1;
            let items: Vec<_> =
                p.s.life.drops.iter().map(|d| (d.id, d.pos.map(f64::to_bits), d.vel.map(f64::to_bits))).collect();
            let orbs: Vec<_> = p.s.life.orbs.iter().map(|o| (o.id, o.pos.map(f64::to_bits), o.count)).collect();
            seen.push((items, orbs, format!("{:?}", p.take_events())));
        }
        crate::drops::FORCED_SWITCHES.with(|c| c.set(None));
        finish(p, dir);
        (pooled, left, seen)
    };
    let fresh = |pool: bool| std::thread::spawn(move || run(pool)).join().expect("simulation");
    let ((pooled, left, recycled), (none, _, new)) = (fresh(true), fresh(false));
    assert_eq!((pooled, none), (60, 0), "the 60 expired bodies kept with the pool only");
    assert_eq!(left, 0, "the 60 new items and orbs took them");
    assert!(recycled.iter().any(|t| t.0.len() == 30), "the new items are on the ground");
    assert_eq!(recycled, new);
}

/// The entity chunks saved with 500 items and orbs beside entities kept as read (item frames,
/// armor stands, a lead knot): the kept compounds lent to the encoding give the same bytes as
/// their clones, and are back in their lists afterwards. Sabotage check: the chunks carry kept
/// entities, the lead knot stays in its list but not in the save.
#[test]
fn entity_chunks_saved_from_lent_compounds() {
    let (mut p, dir) = open("save-lent");
    for k in 0..500 {
        let (x, z) = (2.5 + f64::from(k % 50) * 0.7, -20.5 + f64::from(k / 50) * 4.0);
        let what = if k % 2 == 0 { "item" } else { "experience_orb" };
        cmd(
            &mut p,
            &format!("summon minecraft:{what} {x} -60 {z} {{Item:{{id:\"minecraft:stone\",count:1}},Value:3}}"),
        );
    }
    for k in 0..12 {
        let x = 3 + k * 3;
        cmd(&mut p, &format!("summon minecraft:armor_stand {x}.5 -60 6.5"));
        cmd(&mut p, &format!("setblock {x} -59 9 minecraft:stone"));
        cmd(&mut p, &format!("summon minecraft:item_frame {x}.5 -59.5 8.96875 {{Facing:2b}}"));
    }
    cmd(&mut p, "setblock 20 -60 20 minecraft:oak_fence");
    cmd(&mut p, "summon minecraft:leash_knot 20.5 -59.625 20.5");
    let before = format!("{:?}", p.s.entities);
    let save = |p: &mut Probe, lent: bool| {
        crate::drops::FORCED_SWITCHES.with(|c| c.set(Some(DropSwitches { save_lent: lent, ..DropSwitches::ALL_ON })));
        let drop_chunks = p.s.life.drop_chunks.clone();
        let mut jobs = Vec::new();
        p.s.drops_save_jobs(&mut jobs);
        p.s.life.drop_chunks = drop_chunks;
        crate::drops::FORCED_SWITCHES.with(|c| c.set(None));
        let chunks: Vec<_> = jobs
            .into_iter()
            .filter_map(|j| match j {
                crate::save::Job::Chunk { cx, cz, nbt, .. } => Some((cx, cz, nbt)),
                _ => None,
            })
            .collect();
        (chunks, format!("{:?}", p.s.entities))
    };
    let (cloned, after_clones) = save(&mut p, false);
    let (lent, after_lending) = save(&mut p, true);
    assert_eq!(after_clones, before, "kept entities untouched by the clones");
    assert_eq!(after_lending, before, "kept entities back in their lists");
    assert_eq!(lent, cloned, "same entity chunks");
    let kept: usize = p.s.entities.values().map(Vec::len).sum();
    assert!(kept >= 25, "kept entities to lend: {kept}");
    let bytes = |name: &str| lent.iter().filter(|c| c.2.windows(name.len()).any(|w| w == name.as_bytes())).count();
    assert!(bytes("minecraft:item_frame") > 0 && bytes("minecraft:item") > 0, "saved with items and frames");
    assert_eq!(bytes("minecraft:leash_knot"), 0, "lead knot never saved");
    assert!(before.contains("leash_knot"), "lead knot kept in its list");
    finish(p, dir);
}

/// Size of the structures walked every tick (bytes): an item, an orb and their network tracker
/// may not grow unnoticed (a list of items is moved by every removal in it).
#[test]
fn ground_item_and_orb_sizes() {
    let sizes = [
        ("item", std::mem::size_of::<crate::drops::ItemEnt>(), 368),
        ("orb", std::mem::size_of::<crate::drops::Orb>(), 288),
        ("tracker", std::mem::size_of::<crate::mobs::track::Tracker>(), 184),
    ];
    for (name, size, bound) in sizes {
        assert!(size <= bound, "{name}: {size} bytes, more than {bound}");
    }
}
