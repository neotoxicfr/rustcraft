//! L10-ENT: entity pushes (pistons on armor stands, mannequins and minecarts with a cap of
//! their own, opening shulker on minecarts, tracking of a still decoration), common entity
//! fields of ground items and orbs read and saved like the game, cramming without the
//! passengers, raids kept with their dimension. Fresh flat world (no test world required; 26.3
//! version data mandatory).

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Delta;
use rc_proto_v777::play::{BossEvent, BossOp};

use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::decoration::{DKind, kind_of};
use crate::dims::{END, NETHER, OVERWORLD, Transition, rel};
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

fn close(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() < 1.0e-6, "{what}: {got} instead of {want}");
}

/// Decoration of kind `k` whose position has this `z`: identifier and position.
fn deco(p: &Probe, k: DKind, z: f64) -> (i32, [f64; 3]) {
    p.s.entities
        .values()
        .flat_map(|l| l.iter())
        .filter(|(_, e)| kind_of(e) == Some(k))
        .filter_map(|(id, e)| Some((*id, crate::nbtio::f64s::<3>(e.get("Pos"))?)))
        .find(|(_, pos)| (pos[2] - z).abs() < 0.01)
        .expect("decoration")
}

fn cart_x(p: &Probe, z: f64) -> f64 {
    p.s.carts.list.iter().flatten().find(|c| (c.pos[2] - z).abs() < 0.01).expect("minecart").pos[0]
}

/// Pistons push armor stands, mannequins (an immovable one too) and minecarts like any entity
/// in the head's way: out of the head once it is extended (box from x 62.01), a weightless
/// stand placed as is, a marker stand ignored. Before: all of them stayed in place.
#[test]
fn pistons_push_stands_mannequins_and_minecarts() {
    let (mut p, dir) = open("l10-piston");
    cmd(&mut p, "summon minecraft:armor_stand 61.5 -60 0.5");
    cmd(&mut p, "summon minecraft:mannequin 61.5 -60 2.5 {immovable:1b}");
    cmd(&mut p, "summon minecraft:minecart 61.5 -60 4.5 {NoGravity:1b}");
    cmd(&mut p, "summon minecraft:armor_stand 61.5 -60 6.5 {NoGravity:1b}");
    cmd(&mut p, "summon minecraft:armor_stand 61.5 -60 8.5 {Marker:1b}");
    for z in [0, 2, 4, 6, 8] {
        cmd(&mut p, &format!("setblock 60 -60 {z} minecraft:piston[facing=east]"));
        cmd(&mut p, &format!("setblock 59 -60 {z} minecraft:redstone_block"));
    }
    game_ticks(&mut p, 4);
    // Half widths: stand 0.25, mannequin 0.3, minecart 0.49.
    close(deco(&p, DKind::Stand, 0.5).1[0], 62.26, "armor stand");
    close(deco(&p, DKind::Mannequin, 2.5).1[0], 62.31, "immovable mannequin");
    close(cart_x(&p, 4.5), 62.5, "minecart");
    close(deco(&p, DKind::Stand, 6.5).1[0], 62.26, "weightless stand");
    close(deco(&p, DKind::Stand, 8.5).1[0], 61.5, "marker stand");
    finish(p, dir);
}

/// Each entity has its own piston cap, 0.51 block per axis and per game tick: two pushes of
/// 0.4 in one tick move a stand by 0.51, a mannequin and a minecart by their own 0.51, and the
/// cap starts again at the next tick. Before: no piston move of these entities at all.
#[test]
fn piston_cap_is_per_entity_and_per_tick() {
    let (mut p, dir) = open("l10-cap");
    cmd(&mut p, "summon minecraft:armor_stand 20.5 -60 0.5");
    cmd(&mut p, "summon minecraft:mannequin 20.5 -60 3.5");
    cmd(&mut p, "summon minecraft:minecart 20.5 -60 6.5 {NoGravity:1b}");
    let (stand, man) = (deco(&p, DKind::Stand, 0.5).0, deco(&p, DKind::Mannequin, 3.5).0);
    let k = p.s.carts.list.iter().position(Option::is_some).expect("minecart");
    let mut c = p.s.carts.list[k].take().expect("minecart");
    for _ in 0..2 {
        p.s.deco_piston_move(stand, [0.4, 0.0, 0.0]);
        p.s.deco_piston_move(man, [0.4, 0.0, 0.0]);
        p.s.cart_piston_move(&mut c, [0.4, 0.0, 0.0]);
    }
    close(deco(&p, DKind::Stand, 0.5).1[0], 21.01, "stand capped");
    close(deco(&p, DKind::Mannequin, 3.5).1[0], 21.01, "mannequin capped on its own");
    close(c.pos[0], 21.01, "minecart capped on its own");
    p.s.time += 1;
    p.s.deco_piston_move(stand, [0.4, 0.0, 0.0]);
    p.s.cart_piston_move(&mut c, [0.4, 0.0, 0.0]);
    close(deco(&p, DKind::Stand, 0.5).1[0], 21.41, "stand at the next tick");
    close(c.pos[0], 21.41, "minecart at the next tick");
    p.s.carts.list[k] = Some(c);
    finish(p, dir);
}

/// An opening shulker lifts a minecart lying on it by the whole block (weightless minecart:
/// only the push moves it), and an immovable mannequin it lifts reaches the client through
/// its tracking. Before: the minecart stayed, the mannequin moved without a packet.
#[test]
fn shulker_lifts_minecarts_and_the_lifted_mannequin_is_sent() {
    let (mut p, dir) = open("l10-shulker");
    cmd(&mut p, "summon minecraft:minecart 40.5 -59 0.5 {NoGravity:1b}");
    cmd(&mut p, "summon minecraft:mannequin 44.5 -59 0.5 {immovable:1b}");
    for x in [40.5, 44.5] {
        cmd(&mut p, &format!("summon minecraft:shulker {x} -60 0.5 {{NoAI:1b,Peek:100b,PersistenceRequired:1b}}"));
    }
    let man = deco(&p, DKind::Mannequin, 0.5).0;
    let _ = p.take_events();
    game_ticks(&mut p, 25);
    let y = p.s.carts.list.iter().flatten().next().expect("minecart").pos[1];
    close(y, -58.0, "minecart on the shulker");
    close(deco(&p, DKind::Mannequin, 0.5).1[1], -58.0, "mannequin on the shulker");
    let up = p.take_events().iter().any(|e| match e {
        Event::Entity(EntityEvent::Move(m)) if m.id == man => matches!(m.delta, Some(Delta::Linear(d)) if d[1] > 0),
        Event::Entity(EntityEvent::Sync(s)) if s.id == man => s.path.steps.first().is_some_and(|st| st.0[1] > -58.5),
        _ => false,
    });
    assert!(up, "the client never learnt that the immovable mannequin rose");
    finish(p, dir);
}

/// Ground items and orbs keep the game's common entity fields (fall distance, air,
/// invulnerability, portal delay), written in the game's order by `/data` and the save alike
/// and read back after a save; an invulnerable item or orb refuses fire damage but not the
/// void's. Before: the four fields were neither read nor written, and fire hurt them.
#[test]
fn ground_items_and_orbs_keep_the_common_entity_fields() {
    let (mut p, dir) = open("l10-items");
    cmd(
        &mut p,
        "summon minecraft:item 20.5 -60 0.5 {Item:{id:\"minecraft:stone\",count:1},fall_distance:2.5d,Air:123s,\
         Invulnerable:1b,PortalCooldown:40,PickupDelay:7s,Age:5s,Health:4s}",
    );
    cmd(&mut p, "summon minecraft:experience_orb 24.5 -60 0.5 {Value:3,fall_distance:1.5d,Air:77s,Invulnerable:1b,PortalCooldown:30}");
    let keys = |n: &Nbt| match n {
        Nbt::Compound(f) => f.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let common = ["Pos", "Motion", "Rotation", "fall_distance", "Fire", "Air", "OnGround", "Invulnerable", "PortalCooldown", "UUID"];
    let order = |own: &[&str]| -> Vec<String> {
        std::iter::once("id").chain(common).chain(own.iter().copied()).map(str::to_owned).collect()
    };
    let check = |p: &Probe, what: &str, item: (f64, i16, i32)| {
        let d = p.s.life.drops.first().expect("item");
        let n = p.s.ent_nbt(E::Item(d.id));
        assert_eq!(keys(&n), order(&["Health", "Age", "PickupDelay", "Item"]), "{what}: item fields");
        let got = (n.get("fall_distance"), n.get("Air"), n.get("Invulnerable"), n.get("PortalCooldown"));
        let want = (Nbt::Double(item.0), Nbt::Short(item.1), Nbt::Byte(1), Nbt::Int(item.2));
        assert_eq!(got, (Some(&want.0), Some(&want.1), Some(&want.2), Some(&want.3)), "{what}: item values");
        let o = p.s.life.orbs.first().expect("orb");
        let n = p.s.ent_nbt(E::Orb(o.id));
        assert_eq!(keys(&n), order(&["Health", "Age", "Value", "Count"]), "{what}: orb fields");
        let got = (n.get("fall_distance"), n.get("Air"), n.get("Invulnerable"), n.get("PortalCooldown"));
        assert_eq!(got, (Some(&Nbt::Double(1.5)), Some(&Nbt::Short(77)), Some(&Nbt::Byte(1)), Some(&Nbt::Int(30))), "{what}: orb");
    };
    check(&p, "summoned", (2.5, 123, 40));
    let (item, orb) = (p.s.life.drops[0].id, p.s.life.orbs[0].id);
    assert!(!p.s.item_hurt(item, "minecraft:on_fire", 1.0, false), "fire hurt an invulnerable item");
    assert!(!p.s.orb_hurt(orb, "minecraft:on_fire", 1.0), "fire hurt an invulnerable orb");
    assert_eq!((p.s.life.drops[0].health, p.s.life.orbs[0].health), (4, 5));
    p.s.save(true);
    let data = p.s.data.clone();
    drop(p);
    let mut p = crate::tests_g3f13::reopen(&dir, data);
    check(&p, "read back", (2.5, 123, 40));
    let (item, orb) = (p.s.life.drops[0].id, p.s.life.orbs[0].id);
    assert!(p.s.item_hurt(item, "minecraft:out_of_world", 1.0, false), "the void spared an invulnerable item");
    assert!(p.s.orb_hurt(orb, "minecraft:out_of_world", 1.0), "the void spared an invulnerable orb");
    assert_eq!((p.s.life.drops[0].health, p.s.life.orbs[0].health), (3, 4));
    finish(p, dir);
}

/// The cramming counts only the pushed entities that ride nothing: a zombie touching three
/// others, two of them riding a mob further away, is never hurt with the rule at 3; the same
/// three on foot hurt it. Before: the riders counted and the zombie was hurt.
#[test]
fn cramming_ignores_the_riders() {
    let (mut p, dir) = open("l10-cram");
    cmd(&mut p, "gamerule max_entity_cramming 3");
    // Three zombies around `x` (the last two riding a zombie 20 blocks away if `riding`), then
    // 64 pushes of a fourth one there: hurt?
    let pushes = |p: &mut Probe, x: f64, riding: bool| {
        let vehicle = p.s.new_mob(Kind::Zombie, [x + 20.0, -60.0, 0.5], 0.0);
        let vid = vehicle.id;
        p.s.add_mob(vehicle);
        for i in 0..3 {
            let mut o = p.s.new_mob(Kind::Zombie, [x, -60.0, 0.5], 0.0);
            if riding && i > 0 {
                o.mount.vehicle = Some(vid);
            }
            p.s.add_mob(o);
        }
        let mut m = p.s.new_mob(Kind::Zombie, [x, -60.0, 0.5], 0.0);
        for _ in 0..64 {
            p.s.mob_push_entities(&mut m);
        }
        m.health < 20.0
    };
    assert!(!pushes(&mut p, 20.5, true), "hurt by cramming because of the riders");
    assert!(pushes(&mut p, 60.5, false), "three mobs on foot no longer cram");
    finish(p, dir);
}

/// Places 20 armor stands, 20 mannequins and 10 item frames along x (from 20 on).
fn crowd_of_decorations(p: &mut Probe) {
    for i in 0..20 {
        cmd(p, &format!("summon minecraft:armor_stand {}.5 -60 0.5", 20 + i));
        cmd(p, &format!("summon minecraft:mannequin {}.5 -60 3.5", 20 + i));
    }
    for i in 0..10 {
        let x = 20 + 2 * i;
        cmd(p, &format!("setblock {x} -59 7 minecraft:stone"));
        cmd(
            p,
            &format!(
                "summon minecraft:item_frame {x}.5 -58.5 6.96875 {{Facing:2b,block_pos:[I;{x},-59,6],\
                 Item:{{id:\"minecraft:diamond\",count:1}}}}"
            ),
        );
    }
}

/// The two lookups of the push scan (registry entry, identifier) find the same decorations in
/// the same order, attached only or all, for slices over each place and slices touching none.
#[test]
fn push_scan_lookups_agree() {
    let (mut p, dir) = open("l10-scan");
    crowd_of_decorations(&mut p);
    let mut found = 0;
    for x in (10..50).map(|x| f64::from(x) + 0.5) {
        for z in [0.5, 3.5, 6.5, 30.5] {
            let area = rc_physics::Aabb { min: [x - 0.3, -60.0, z - 0.6], max: [x + 0.3, -58.0, z + 0.6] };
            for attached in [false, true] {
                let a = p.s.deco_touching_with(&area, attached, true);
                assert_eq!(a, p.s.deco_touching_with(&area, attached, false), "slice at {x} {z} ({attached})");
                found += a.len();
            }
        }
    }
    assert_eq!(found, 20 + 20 + 2 * 10, "decorations met by the slices");
    finish(p, dir);
}

/// Micro-bench (ignored, `--ignored --nocapture`): the decoration and minecart scans of one
/// piston push whose slice touches nothing, among 40 armor stands and mannequins, 10 frames and
/// 100 minecarts: the former scan (attached decorations only, looked up by identifier), the new
/// one (every decoration, from its registry entry) and the new one by identifier
/// (`RUSTCRAFT_OPT_DECOSCAN=0`); the minecart walk the push now adds.
#[test]
#[ignore]
fn bench_l10ent_piston_push_scan() {
    let (mut p, dir) = open("l10-benchscan");
    crowd_of_decorations(&mut p);
    for i in 0..100 {
        cmd(&mut p, &format!("summon minecraft:minecart {}.5 -60 {}.5", 60 + i % 10, 20 + i / 10));
    }
    let area = rc_physics::Aabb { min: [100.0, -60.0, 0.0], max: [101.0, -59.0, 1.0] };
    let n = 20_000u32;
    let time = |f: &mut dyn FnMut() -> usize| {
        let t = std::time::Instant::now();
        let mut hits = 0;
        for _ in 0..n {
            hits += f();
        }
        (t.elapsed().as_nanos() as f64 / f64::from(n), hits)
    };
    for _ in 0..2 {
        let old = time(&mut || p.s.deco_touching_with(std::hint::black_box(&area), true, false).len());
        let new = time(&mut || p.s.deco_touching_with(std::hint::black_box(&area), false, true).len());
        let by_id = time(&mut || p.s.deco_touching_with(std::hint::black_box(&area), false, false).len());
        let carts = time(&mut || {
            p.s.carts.list.iter().flatten().filter(|c| !c.removed && c.bbox().intersects(std::hint::black_box(&area))).count()
        });
        eprintln!(
            "push scan: decorations former {:.0} ns, new {:.0} ns, new by identifier {:.0} ns; minecarts {:.0} ns ({} hits)",
            old.0,
            new.0,
            by_id.0,
            carts.0,
            old.1 + new.1 + by_id.1 + carts.1
        );
    }
    finish(p, dir);
}

/// Micro-bench (ignored, `--ignored --nocapture`): the raids of a parked world every tick, an
/// empty list (counter only, the world not visited) against one inactive raid (world visited).
#[test]
#[ignore]
fn bench_l10ent_parked_raids() {
    let (mut p, dir) = open("l10-benchraids");
    p.s.enter_world(NETHER);
    p.s.mobs.ill.raids.loaded = true;
    p.s.enter_world(OVERWORLD);
    let n = 20_000u32;
    let run = |p: &mut Probe| {
        let t = std::time::Instant::now();
        for _ in 0..n {
            p.s.parked_raids_tick(&[]);
        }
        t.elapsed().as_nanos() as f64 / f64::from(n)
    };
    for _ in 0..2 {
        let empty = run(&mut p);
        eprintln!("parked raids, empty list: {empty:.0} ns per tick");
    }
    let w = p.s.dims.parked.iter_mut().find(|w| w.dim.name == NETHER).expect("parked Nether");
    w.mobs.ill.raids.list.push(crate::mobs::illagers::raids::test_raid([100, 64, 100], 2));
    for _ in 0..2 {
        let one = run(&mut p);
        eprintln!("parked raids, one inactive raid: {one:.0} ns per tick");
    }
    finish(p, dir);
}

fn to(dim: &str) -> Transition {
    Transition {
        dim: dim.into(),
        pos: [0.5, 70.0, 0.5],
        yaw: 0.0,
        pitch: 0.0,
        relative: rel::ROTATION,
        sound: false,
        ticket: false,
        missing_respawn: false,
    }
}

/// Centers of the raids written in the raids file of dimension `dim`.
fn saved_raids(dir: &std::path::Path, dim: &str) -> Vec<[i32; 3]> {
    let path = crate::world::dimension_dir(dir, dim).join("data").join("minecraft").join("raids.dat");
    let root = crate::nbtio::read_dat(&path).unwrap_or_else(|e| panic!("{dim}: raids file {e:?}"));
    let data = root.get("data").expect("data");
    crate::nbtio::list(data.get("raids"))
        .iter()
        .filter_map(|r| match r.get("center") {
            Some(Nbt::IntArray(a)) => Some([a[0], a[1], a[2]]),
            _ => None,
        })
        .collect()
}

/// Raids belong to their dimension: the overworld's raid stays there while the player is in
/// the Nether (parked, inactive without a ticket, its world's counter still running) even with
/// its center chunk loaded in the Nether, where a raid of its own is kept apart; the player
/// leaves the raid bars of the world it leaves; a raid omen starts a raid in the End but never
/// in the Nether; each world saves its raids in its own folder. Before: the raids followed the
/// player (the overworld's raid stopped in the Nether for lack of a village), one file at the
/// world root, raids refused outside the overworld.
#[test]
fn raids_stay_with_their_dimension() {
    use crate::mobs::illagers::raids::test_raid;
    let (mut p, dir) = open("l10-raids");
    // Refused when already normal.
    let src = p.s.server_source();
    let _ = p.s.perform(src, "difficulty normal");
    assert_eq!(p.s.level.difficulty_id(), 2, "difficulty");
    let mut r = test_raid([8, -60, 8], 2);
    (r.active, r.bar_player, r.bar_visible) = (true, true, true);
    let bar = r.bar;
    let raids = &mut p.s.mobs.ill.raids;
    (raids.loaded, raids.dirty, raids.next_id) = (true, true, 2);
    raids.list.push(r);
    let chunk = p.s.chunks.get(&(0, 0)).cloned().flatten().expect("chunk (0, 0)");
    let _ = p.take_events();
    p.s.change_dimension(to(NETHER));
    let left = p.take_events().iter().any(|e| matches!(e, Event::Boss(BossEvent { id, op: BossOp::Remove }) if *id == bar));
    assert!(left, "raid bar kept when leaving the overworld");
    assert!(p.s.mobs.ill.raids.list.is_empty(), "the overworld's raid followed the player");
    // Chunk at the same place in the Nether: a raid that followed would stop there (no village).
    p.s.chunks.insert((0, 0), Some(chunk));
    let parked = |p: &Probe, dim: &str| {
        p.s.dims.parked.iter().find(|w| w.dim.name == dim).map(|w| w.mobs.ill.raids.clone()).expect("parked world")
    };
    let before = parked(&p, OVERWORLD).tick;
    game_ticks(&mut p, 5);
    let ow = parked(&p, OVERWORLD);
    assert_eq!(ow.list.len(), 1, "overworld raid lost");
    assert!(!ow.list[0].is_stopped() && !ow.list[0].active, "parked raid: ongoing and inactive");
    assert_eq!(ow.tick, before + 5, "counter of the parked raids");
    // A raid of the Nether's own; a raid omen there starts nothing (`can_start_raid` false).
    let mut nr = test_raid([100, 64, 100], 2);
    nr.id = 7;
    p.s.mobs.ill.raids.list.push(nr);
    p.s.mobs.ill.raids.dirty = true;
    crate::nbtio::set(&mut p.s.pdata.root, "raid_omen_position", Nbt::IntArray(vec![0, 70, 0]));
    assert!(!p.s.raid_omen_tick(), "omen not consumed");
    assert_eq!(p.s.mobs.ill.raids.list.len(), 1, "raid started in the Nether");
    // The End keeps the attribute's default: a raid starts there.
    p.s.change_dimension(to(END));
    crate::nbtio::set(&mut p.s.pdata.root, "raid_omen_position", Nbt::IntArray(vec![0, 70, 0]));
    assert!(!p.s.raid_omen_tick(), "omen not consumed in the End");
    assert_eq!(p.s.mobs.ill.raids.list.len(), 1, "no raid in the End");
    p.s.change_dimension(to(OVERWORLD));
    let centers: Vec<[i32; 3]> = p.s.mobs.ill.raids.list.iter().map(|r| r.center).collect();
    assert_eq!(centers, vec![[8, -60, 8]], "overworld raids on return");
    p.s.save(true);
    assert_eq!(saved_raids(&dir, OVERWORLD), vec![[8, -60, 8]]);
    assert_eq!(saved_raids(&dir, NETHER), vec![[100, 64, 100]]);
    assert_eq!(saved_raids(&dir, END), vec![[0, 70, 0]]);
    assert!(!dir.join("data").join("minecraft").join("raids.dat").exists(), "raids file at the world root");
    finish(p, dir);
}
