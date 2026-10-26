//! Minecarts (G5-F06): expected trajectories computed by hand from the game's rules
//! (no value read off our own code), mounting, breaking, contents, variants,
//! save. Test world required (`RUSTCRAFT_SOLO_WORLD`).

use rc_proto::nbt::Nbt;

use super::CKind;
use crate::commands::ents::E;
use crate::player::Stack;
use crate::server::Server;

fn open(tag: &str) -> Option<Server> {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data missing: test skipped");
        return None;
    };
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut s = crate::portal_tests::open_world(&dir, std::sync::Arc::new(data));
    s.loaded = true;
    s.teleport = None;
    let c = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (c.0 + dx, c.1 + dz))).collect();
    s.load_now(&around);
    Some(s)
}

fn step(s: &mut Server, n: usize) {
    for _ in 0..n {
        s.game_tick();
        s.tick += 1;
    }
}

fn cmd(s: &mut Server, c: &str) {
    let src = s.server_source();
    let (ok, _) = s.perform(src, c);
    // `fill`/`setblock` without a change fail like in the game: only the summons count.
    assert!(ok || !c.starts_with("summon"), "command refused: {c}");
}

/// Flat east-west track of `len` rails starting at `o` (stone floor, air above).
fn track(s: &mut Server, o: [i32; 3], len: i32) {
    let [x, y, z] = o;
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:air", x - 2, y, z - 2, x + len + 2, y + 3, z + 2));
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:stone", x - 2, y - 1, z - 2, x + len + 2, y - 1, z + 2));
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:rail[shape=east_west]", x, y, z, x + len - 1, y, z));
}

/// Last entity summoned of the type ("commands" access, valid also without simulation).
fn last_of(s: &Server, ty: &str) -> E {
    s.all_ents().into_iter().rfind(|e| s.ent_type(*e) == ty).expect("summoned entity")
}

fn origin(s: &Server) -> [i32; 3] {
    let b = s.pos.map(|v| v.floor() as i32);
    [b[0] + 3, b[1], b[2] + 3]
}

/// Flat rail, empty minecart launched at 0.2 eastward: each tick it advances by its speed
/// (capped at 0.4) then slows by 0.96; rail height + 1/16 from the first tick; yaw 0,
/// flipped (eastward, `atan2` gives 180 then the half turn at ±170° brings it back to 0).
#[test]
fn flat_rail_slows_by_0_96() {
    let Some(mut s) = open("cart-flat") else { return };
    let o = origin(&s);
    track(&mut s, o, 14);
    let start = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{Motion:[0.2d,0.0d,0.0d]}}", start[0], start[1], start[2]));
    let e = last_of(&s, "minecraft:minecart");
    let (mut x, mut v) = (start[0], 0.2f64);
    for t in 1..=12 {
        step(&mut s, 1);
        x += v.clamp(-0.4, 0.4);
        v *= 0.96;
        let p = s.e_pos(e);
        assert!((p[0] - x).abs() < 1.0e-9, "tick {t}: x {} expected {x}", p[0]);
        assert!((p[1] - (start[1] + 0.0625)).abs() < 1.0e-9, "tick {t} : y {}", p[1]);
        assert!((p[2] - start[2]).abs() < 1.0e-9, "tick {t} : z {}", p[2]);
    }
    let E::Other(id) = e else { panic!("minecart expected") };
    let c = s.cart_ref(id).expect("simulated minecart");
    assert!((c.vel[0] - v).abs() < 1.0e-9, "speed {} expected {v}", c.vel[0]);
    assert_eq!((c.yaw, c.flipped), (0.0, true), "yaw and flip eastward");
}

/// Start on a powered booster rail, solid block to the west: 0.02 eastward on the first
/// tick, then + 0.06 per tick while on the rail (after the 0.96 slowdown).
#[test]
fn powered_rail_start_and_push() {
    let Some(mut s) = open("cart-boost") else { return };
    let o = origin(&s);
    track(&mut s, o, 14);
    let [x0, y0, z0] = o;
    cmd(&mut s, &format!("setblock {} {} {} minecraft:redstone_block", x0 + 1, y0 - 1, z0));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:stone", x0, y0, z0));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:powered_rail[shape=east_west,powered=true]", x0 + 1, y0, z0));
    let start = [f64::from(x0 + 1) + 0.5, f64::from(y0), f64::from(z0) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", start[0], start[1], start[2]));
    let e = last_of(&s, "minecraft:minecart");
    // Computed by hand: x, speed; the booster rail covers [x0+1, x0+2).
    let (mut x, mut v) = (start[0], 0.0f64);
    for t in 1..=8 {
        let powered = x < f64::from(x0 + 2);
        step(&mut s, 1);
        x += v;
        v *= 0.96;
        if powered {
            v = if v > 0.01 { v + 0.06 } else { 0.02 };
        }
        let p = s.e_pos(e);
        assert!((p[0] - x).abs() < 1.0e-9, "tick {t}: x {} expected {x}", p[0]);
    }
}

/// Mounting by right click (seat at + 0.1875 − 0.6), dismounting while sneaking onto a free cell beside,
/// 60-tick delay before mounting again.
#[test]
fn player_mount_and_dismount() {
    let Some(mut s) = open("cart-ride") else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    let o = [o[0] + 1, o[1], o[2]];
    track(&mut s, o, 3);
    s.game_mode = 0;
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    s.pos = [cp[0] - 1.0, cp[1], cp[2]];
    assert!(s.cart_interact(id, 0), "interaction taken by the minecart");
    assert_eq!(s.pcart.vehicle, Some(id), "player mounted");
    let c = s.cart_ref(id).unwrap();
    assert!((s.pos[1] - (c.pos[1] + 0.1875 - 0.6)).abs() < 1.0e-6, "seat {}", s.pos[1]);
    s.teleport = None;
    s.sneaking = true;
    step(&mut s, 1);
    assert_eq!(s.pcart.vehicle, None, "dismounted while sneaking");
    assert!(s.cart_ref(id).unwrap().passengers.is_empty());
    assert_eq!(s.mobs.mounts.riding.boarding_cooldown, 60);
    // Dismount cell: beside the minecart (not on it), stone floor.
    let c = s.cart_ref(id).unwrap();
    let d = ((s.pos[0] - c.pos[0]).powi(2) + (s.pos[2] - c.pos[2]).powi(2)).sqrt();
    assert!(d >= 0.99, "dismount beside: {:?} / {:?}", s.pos, c.pos);
}

/// `/tp` of a player seated in a minecart: dismounting the vehicle, they get out (passenger removed,
/// mount delay at 0) and arrive where the command sends them.
#[test]
fn teleport_leaves_the_minecart() {
    let Some(mut s) = open("cart-tp") else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    let o = [o[0] + 1, o[1], o[2]];
    track(&mut s, o, 3);
    s.game_mode = 0;
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    s.pos = [cp[0] - 1.0, cp[1], cp[2]];
    assert!(s.cart_interact(id, 0));
    assert_eq!(s.pcart.vehicle, Some(id));
    let to = [cp[0] + 5.0, cp[1] + 3.0, cp[2]];
    cmd(&mut s, &format!("tp @p {} {} {}", to[0], to[1], to[2]));
    assert_eq!(s.pcart.vehicle, None, "out of the minecart");
    assert!(s.cart_ref(id).unwrap().passengers.is_empty());
    assert_eq!(s.mobs.mounts.riding.boarding_cooldown, 0);
    assert_eq!(s.pos, to);
}

/// Bare-handed hits in survival: +10 damage per hit, broken beyond 40 (5th hit);
/// the dropped item is a minecart. Chest minecart: contents dropped with it.
#[test]
fn break_and_contents() {
    let Some(mut s) = open("cart-break") else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    let o = [o[0] + 1, o[1], o[2]];
    track(&mut s, o, 3);
    s.game_mode = 0;
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    s.pos = [cp[0] - 1.5, cp[1], cp[2]];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    let drops = s.life.drops.len();
    for hit in 1..=4 {
        s.mobs.attack_ticker = 100;
        assert!(s.cart_attacked(id));
        assert!(s.cart_ref(id).is_some_and(|c| c.damage == 10.0 * hit as f32), "hit {hit}");
    }
    s.mobs.attack_ticker = 100;
    s.cart_attacked(id);
    assert!(s.cart_ref(id).is_none(), "broken at the 5th hit (50 > 40)");
    let new: Vec<&str> = s.life.drops[drops..].iter().map(|d| d.stack.id.as_str()).collect();
    assert_eq!(new, ["minecraft:minecart"]);
    // Chest minecart with 3 stacks: 3 stacks (at most burst into pieces) then the item.
    cmd(
        &mut s,
        &format!(
            "summon minecraft:chest_minecart {} {} {} {{Items:[{{Slot:0b,id:\"minecraft:dirt\",count:5}},{{Slot:4b,id:\"minecraft:stone\",count:2}}]}}",
            cp[0], cp[1], cp[2]
        ),
    );
    let E::Other(cid) = last_of(&s, "minecraft:chest_minecart") else { panic!() };
    let drops = s.life.drops.len();
    s.game_mode = 1;
    s.cart_attacked(cid);
    assert!(s.cart_ref(cid).is_none(), "removed in one hit in creative");
    let ids: Vec<&str> = s.life.drops[drops..].iter().map(|d| d.stack.id.as_str()).collect();
    let dirt: i32 = s.life.drops[drops..].iter().filter(|d| d.stack.id == "minecraft:dirt").map(|d| d.stack.count).sum();
    assert_eq!(dirt, 5, "contents dropped even in creative: {ids:?}");
    assert!(!ids.contains(&"minecraft:chest_minecart"), "no item in creative");
}

/// Compound in the game's format read back then rewritten identically (key order included).
#[test]
fn nbt_round_trip() {
    let Some(mut s) = open("cart-nbt") else { return };
    let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
    let uuid = crate::nbtio::uuid_nbt(0x1234_5678_9abc_4def_8123_4567_89ab_cdef);
    let head = |id: &str| -> Vec<(String, Nbt)> {
        vec![
            ("id".into(), Nbt::String(id.into())),
            ("Pos".into(), dbl([10.5, 64.0625, -3.5])),
            ("Motion".into(), dbl([0.1, 0.0, 0.0])),
            ("Rotation".into(), Nbt::List(vec![Nbt::Float(90.0), Nbt::Float(0.0)])),
            ("fall_distance".into(), Nbt::Double(0.0)),
            ("Fire".into(), Nbt::Short(-1)),
            ("Air".into(), Nbt::Short(300)),
            ("OnGround".into(), Nbt::Byte(0)),
            ("Invulnerable".into(), Nbt::Byte(0)),
            ("PortalCooldown".into(), Nbt::Int(0)),
            ("UUID".into(), uuid.clone()),
            ("CustomName".into(), Nbt::String("Bob".into())),
            ("Tags".into(), Nbt::List(vec![Nbt::String("t".into())])),
            ("FlippedRotation".into(), Nbt::Byte(1)),
            ("HasTicked".into(), Nbt::Byte(0)),
        ]
    };
    let mut chest = head("minecraft:chest_minecart");
    chest.push(("LootTable".into(), Nbt::String("minecraft:chests/abandoned_mineshaft".into())));
    chest.push(("LootTableSeed".into(), Nbt::Long(42)));
    let mut hopper = head("minecraft:hopper_minecart");
    hopper.push((
        "Items".into(),
        Nbt::List(vec![Stack { id: "minecraft:dirt".into(), count: 3, components: None }.to_nbt(Some(2))]),
    ));
    hopper.push(("Enabled".into(), Nbt::Byte(0)));
    let mut tnt = head("minecraft:tnt_minecart");
    tnt.push(("fuse".into(), Nbt::Int(-1)));
    tnt.push(("explosion_power".into(), Nbt::Float(2.0)));
    let mut furnace = head("minecraft:furnace_minecart");
    furnace.push(("PushX".into(), Nbt::Double(0.5)));
    furnace.push(("PushZ".into(), Nbt::Double(-0.25)));
    furnace.push(("Fuel".into(), Nbt::Short(3000)));
    let mut spawner = head("minecraft:spawner_minecart");
    spawner.push(("Delay".into(), Nbt::Short(20)));
    spawner.push(("MaxNearbyEntities".into(), Nbt::Short(6)));
    let mut rideable = head("minecraft:minecart");
    rideable.insert(13, ("DisplayOffset".into(), Nbt::Int(3)));
    for f in [chest, hopper, tnt, furnace, spawner, rideable] {
        let n = Nbt::Compound(f);
        let c = s.cart_from_nbt(&n).expect("minecart read");
        assert_eq!(s.cart_to_nbt(&c), n, "round trip {:?}", n.get("id"));
    }
}

/// Powered activator rail: the TNT minecart primes (fuse 80, already 79 after its tick),
/// explodes at 0; once primed, it spares the rails and the block under them.
#[test]
fn primed_tnt_spares_rails() {
    let Some(mut s) = open("cart-tnt") else { return };
    let o = origin(&s);
    track(&mut s, o, 5);
    let [x0, y0, z0] = o;
    cmd(&mut s, &format!("setblock {} {} {} minecraft:redstone_block", x0 + 2, y0 - 1, z0));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:activator_rail[shape=east_west,powered=true]", x0 + 2, y0, z0));
    let cp = [f64::from(x0 + 2) + 0.5, f64::from(y0), f64::from(z0) + 0.5];
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:tnt_minecart") else { panic!() };
    step(&mut s, 1);
    assert_eq!(s.cart_ref(id).map(|c| c.fuse), Some(79), "primed on the activator");
    step(&mut s, 80);
    assert!(s.cart_ref(id).is_none(), "exploded");
    for x in x0..x0 + 5 {
        assert!(s.btag_id([x, y0, z0], crate::tag_bits::TagId::Rails), "rail {x} spared");
        assert!(!s.is_air([x, y0 - 1, z0]), "support of rail {x} spared");
    }
}

/// Hopper minecart: item on the ground above sucked up whole; powered activator rail:
/// disabled.
#[test]
fn hopper_sucks_and_activator_disables() {
    let Some(mut s) = open("cart-hopper") else { return };
    let o = origin(&s);
    track(&mut s, o, 3);
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:hopper_minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:hopper_minecart") else { panic!() };
    s.spawn_item([cp[0], cp[1] + 1.0, cp[2]], [0.0; 3], Stack { id: "minecraft:dirt".into(), count: 7, components: None }, 0);
    step(&mut s, 2);
    let c = s.cart_ref(id).unwrap();
    let n: i32 = c.items.iter().flatten().filter(|x| x.id == "minecraft:dirt").map(|x| x.count).sum();
    assert_eq!(n, 7, "item sucked up");
    let [x0, y0, z0] = o;
    cmd(&mut s, &format!("setblock {} {} {} minecraft:redstone_block", x0, y0 - 1, z0));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:activator_rail[shape=east_west,powered=true]", x0, y0, z0));
    step(&mut s, 1);
    assert!(!s.cart_ref(id).unwrap().enabled, "disabled by the powered activator");
}

/// Furnace minecart: coal given (3600 of fuel, push = minecart − player), lit on the
/// next tick; the fuel decreases.
#[test]
fn furnace_fuel_and_push() {
    let Some(mut s) = open("cart-furnace") else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    let o = [o[0] + 1, o[1], o[2]];
    track(&mut s, o, 10);
    s.game_mode = 0;
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:furnace_minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:furnace_minecart") else { panic!() };
    s.pos = [cp[0] - 1.0, cp[1], cp[2]];
    let slot = 36 + s.held as usize;
    s.slots[slot] = Some(Stack { id: "minecraft:coal".into(), count: 2, components: None });
    s.cart_interact(id, 0);
    let c = s.cart_ref(id).unwrap();
    assert_eq!(c.fuel, 3600);
    assert_eq!(c.push, [1.0, 0.0, 0.0], "push from the player");
    assert_eq!(s.slots[slot].as_ref().map(|x| x.count), Some(1), "one coal taken");
    step(&mut s, 1);
    let c = s.cart_ref(id).unwrap();
    assert_eq!(c.fuel, 3599);
    assert_eq!(c.meta(13), Some(&rc_proto_v777::entity::Meta::Bool(true)), "lit");
    assert!(c.vel[0] > 0.0, "pushed eastward");
}

/// Detector rail: powered when a minecart rolls over it, off 20 ticks after it leaves.
#[test]
fn detector_rail() {
    let Some(mut s) = open("cart-detector") else { return };
    let o = origin(&s);
    track(&mut s, o, 14);
    let [x0, y0, z0] = o;
    cmd(&mut s, &format!("setblock {} {} {} minecraft:detector_rail[shape=east_west]", x0 + 2, y0, z0));
    let cp = [f64::from(x0) + 0.5, f64::from(y0), f64::from(z0) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{Motion:[0.3d,0.0d,0.0d]}}", cp[0], cp[1], cp[2]));
    let powered = |s: &Server| s.v().get(s.st([x0 + 2, y0, z0]), "powered") == Some("true");
    let mut seen = false;
    for _ in 0..12 {
        step(&mut s, 1);
        seen |= powered(&s);
    }
    assert!(seen, "detector powered on passing");
    step(&mut s, 40);
    assert!(!powered(&s), "detector off after the passage");
}

/// A simple rolling minecart picks up a touched mob; saved with it.
#[test]
fn boards_a_mob_and_saves() {
    let Some(mut s) = open("cart-pickup") else { return };
    let o = origin(&s);
    track(&mut s, o, 14);
    let [x0, y0, z0] = o;
    cmd(&mut s, &format!("summon minecraft:pig {} {} {} {{NoAI:1b}}", f64::from(x0 + 4) + 0.5, y0, f64::from(z0) + 0.5));
    let cp = [f64::from(x0) + 0.5, f64::from(y0), f64::from(z0) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{Motion:[0.4d,0.0d,0.0d]}}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    step(&mut s, 12);
    let c = s.cart_ref(id).unwrap();
    assert_eq!(c.passengers.len(), 1, "pig boarded");
    let mut by_chunk = std::collections::BTreeMap::new();
    s.carts_into_chunks(&mut by_chunk);
    let saved: Vec<&Nbt> = by_chunk.values().flatten().filter(|n| n.get("UUID") == Some(&crate::nbtio::uuid_nbt(c.uuid))).collect();
    assert_eq!(saved.len(), 1);
    let pass = crate::nbtio::list(saved[0].get("Passengers"));
    assert_eq!(pass.first().and_then(|p| p.get("id")).and_then(Nbt::as_str), Some("minecraft:pig"));
    // The pig is not written a second time separately.
    let mut mobs = std::collections::BTreeMap::new();
    s.mobs_into_chunks(&mut mobs);
    let pigs = mobs.values().flatten().filter(|n| n.get("id").and_then(Nbt::as_str) == Some("minecraft:pig")).count();
    assert_eq!(pigs, 0, "passenger written in its minecart only");
}

/// Placement by the item: refused off a rail; on a slope, + 0.5; stack name carried over.
#[test]
fn placement_on_rail() {
    let Some(mut s) = open("cart-place") else { return };
    let o = origin(&s);
    track(&mut s, o, 3);
    let [x0, y0, z0] = o;
    let item = Stack { id: "minecraft:chest_minecart".into(), count: 1, components: None };
    assert!(s.place_minecart(&item, [x0, y0 - 1, z0]).is_none(), "refused on stone");
    let id = s.place_minecart(&item, [x0, y0, z0]).expect("placed");
    let c = s.cart_ref(id).unwrap();
    assert_eq!(c.kind, CKind::Chest);
    assert_eq!(c.pos, [f64::from(x0) + 0.5, f64::from(y0) + 0.0625, f64::from(z0) + 0.5]);
    // Ascending eastward: block and rail higher to the east.
    cmd(&mut s, &format!("setblock {} {} {} minecraft:stone", x0 + 2, y0, z0));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:rail[shape=east_west]", x0 + 2, y0 + 1, z0));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:rail[shape=ascending_east]", x0 + 1, y0, z0));
    let slope = s.rail_shape_at([x0 + 1, y0, z0]).is_some_and(|sh| sh.is_slope());
    assert!(slope, "slope placed");
    let id = s.place_minecart(&Stack { id: "minecraft:minecart".into(), count: 1, components: None }, [x0 + 1, y0, z0]).unwrap();
    assert_eq!(s.cart_ref(id).unwrap().pos[1], f64::from(y0) + 0.5625, "slope: + 0.5");
}

/// Hoppers (blocks) and chest minecart: the one below draws an item from the first
/// non-empty slot, the one above pointing down pours one into the first free slot.
#[test]
fn hoppers_and_chest_minecart() {
    let Some(mut s) = open("cart-hopperblock") else { return };
    let o = origin(&s);
    track(&mut s, o, 1);
    let [x, y, z] = o;
    cmd(&mut s, &format!("setblock {x} {y} {z} minecraft:hopper"));
    let up = [x, y + 2, z];
    cmd(
        &mut s,
        &format!(
            "setblock {} {} {} minecraft:hopper[facing=down]{{Items:[{{Slot:0b,id:\"minecraft:stone\",count:3}}]}}",
            up[0], up[1], up[2]
        ),
    );
    cmd(
        &mut s,
        &format!(
            "summon minecraft:chest_minecart {} {} {} {{Items:[{{Slot:0b,id:\"minecraft:dirt\",count:5}}]}}",
            f64::from(x) + 0.5,
            y + 1,
            f64::from(z) + 0.5
        ),
    );
    let E::Other(id) = last_of(&s, "minecraft:chest_minecart") else { panic!() };
    s.hopper_tick([x, y, z]);
    s.hopper_tick(up);
    let count = |s: &Server, p: [i32; 3]| s.be_item_counts(p).iter().map(|c| c.1).sum::<i32>();
    assert_eq!(count(&s, [x, y, z]), 1, "one item drawn from the minecart");
    assert_eq!(count(&s, up), 2, "one item poured into the minecart");
    let c = s.cart_ref(id).unwrap();
    let slot = |i: usize| c.items[i].as_ref().map(|x| (x.id.as_str(), x.count));
    assert_eq!(slot(0), Some(("minecraft:dirt", 4)));
    assert_eq!(slot(1), Some(("minecraft:stone", 1)));
}

/// Player passenger: the distance traveled counts in `minecart_one_cm` (rounded to the cm).
#[test]
fn travel_statistic() {
    let Some(mut s) = open("cart-ridestat") else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    let o = [o[0] + 1, o[1], o[2]];
    track(&mut s, o, 10);
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    s.pos = [cp[0] - 1.0, cp[1], cp[2]];
    assert!(s.cart_interact(id, 0));
    s.teleport = None;
    step(&mut s, 1);
    let before = s.prog.stats.get("minecraft:custom", "minecraft:minecart_one_cm");
    s.cart_mut(id).unwrap().vel = [0.3, 0.0, 0.0];
    let x0 = s.pos[0];
    step(&mut s, 1);
    let dx = s.pos[0] - x0;
    assert!(dx > 0.1, "the minecart rolled: {dx}");
    let got = s.prog.stats.get("minecraft:custom", "minecraft:minecart_one_cm") - before;
    assert_eq!(got, ((dx as f32) * 100.0).round() as i32, "distance en cm");
}

/// Hit by a non-living projectile (`hurt_other`): damage × 10 like the player's hit.
#[test]
fn projectile_hit() {
    let Some(mut s) = open("cart-projhit") else { return };
    let o = origin(&s);
    track(&mut s, o, 1);
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    let t = crate::projectiles::physics::Target::Cart(id);
    assert!(s.hurt_other(t, "minecraft:arrow", None, 3.0));
    assert_eq!(s.cart_ref(id).map(|c| c.damage), Some(30.0));
    assert!(s.hurt_other(t, "minecraft:arrow", None, 2.0));
    assert!(s.cart_ref(id).is_none(), "broken beyond 40");
}

/// Fishing hook attached to a minecart: follows the minecart at 0.8 × its height, and the reeled-in line
/// adds (player − hook) × 0.1 to its speed.
#[test]
fn hook_pulls_minecart() {
    use crate::projectiles::fishing::HookState;
    let Some(mut s) = open("cart-hook") else { return };
    let o = origin(&s);
    track(&mut s, o, 3);
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    step(&mut s, 1);
    s.game_mode = 0;
    s.slots[36 + s.held as usize] = Some(Stack { id: "minecraft:fishing_rod".into(), count: 1, components: None });
    assert!(s.fishing_rod_use(0));
    {
        let h = s.proj.hook.as_mut().expect("hook cast");
        h.hooked = Some(crate::projectiles::physics::Target::Cart(id));
        h.state = HookState::Hooked;
    }
    step(&mut s, 1);
    let c = s.cart_ref(id).expect("minecart");
    let (cpos, cvel) = (c.pos, c.vel);
    let h = s.proj.hook.as_ref().expect("still hooked");
    assert_eq!(h.state, HookState::Hooked, "stays hooked");
    let hp = h.p.pos;
    assert!((hp[1] - (cpos[1] + super::HEIGHT * 0.8)).abs() < 1.0e-9, "hook height {hp:?} / {cpos:?}");
    assert!((hp[0] - cpos[0]).abs() < 1.0e-9 && (hp[2] - cpos[2]).abs() < 1.0e-9);
    let rod = s.slots[36 + s.held as usize].clone().unwrap();
    let pp = s.pos;
    assert_eq!(s.hook_retrieve(&rod), 5, "wear from a pulled entity");
    let v = s.cart_ref(id).unwrap().vel;
    for i in 0..3 {
        let want = cvel[i] + (pp[i] - hp[i]) * 0.1;
        assert!((v[i] - want).abs() < 1.0e-12, "axis {i}: {} expected {want}", v[i]);
    }
}

/// End portal: the empty minecart goes through at once (no delay outside the player), arrives at
/// (100.5; 50; 0.5) yaw 90 absolute, speed rotated by the yaw difference (0 − 90: (x, z)
/// becomes (−z, x)), return delay 10. With the player seated: the player changes dimension
/// with it, stays a passenger, seat kept.
#[test]
fn end_portal() {
    let Some(mut s) = open("cart-portal") else { return };
    let o = origin(&s);
    track(&mut s, o, 3);
    let (x, y, z) = (o[0] + 1, o[1], o[2]);
    cmd(&mut s, &format!("setblock {x} {y} {z} minecraft:end_portal"));
    let cp = [f64::from(x) + 0.5, f64::from(y) + 0.2, f64::from(z) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{NoGravity:1b,Motion:[0.2d,0d,0.1d]}}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    s.carts_portal_tick();
    assert!(s.cart_ref(id).is_none(), "left the Overworld");
    let (pos, vel, yaw) = s.in_world(crate::dims::END, |w| {
        let c = w.cart_ref(id).expect("minecart in the End");
        (c.pos, c.vel, c.yaw)
    });
    assert_eq!(pos, [100.5, 50.0, 0.5]);
    assert_eq!(yaw, 90.0);
    assert!((vel[0] + 0.1).abs() < 1.0e-6 && vel[1] == 0.0 && (vel[2] - 0.2).abs() < 1.0e-6, "speed {vel:?}");
    assert_eq!(s.dims.ents.get(&id).map(|p| p.cooldown), Some(10), "return delay of a vehicle");
    // Player passenger.
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{NoGravity:1b}}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    assert!(s.cart_ride_forced(id, E::Player));
    let seat = s.pos[1] - cp[1];
    s.carts_portal_tick();
    assert_eq!(s.dim.name, crate::dims::END, "the player follows their minecart");
    assert_eq!(s.pcart.vehicle, Some(id));
    let c = s.cart_ref(id).expect("minecart in the player's world");
    assert_eq!(c.passengers, vec![crate::mobs::mob::Ent::Player]);
    assert_eq!(s.pos, [100.5, 50.0 + seat, 0.5], "seat kept");
}

/// Block effects: cobweb (the next movement × 0.25 and speed reset to zero), fire that
/// primes a TNT minecart (fire damage), fire state at −1 after a tick out of fire.
#[test]
fn block_effects() {
    let Some(mut s) = open("cart-blocks") else { return };
    let o = origin(&s);
    track(&mut s, o, 12);
    let [x, y, z] = o;
    // Cobweb: gravity-free minecart launched at 0.2 eastward in a corridor of cobwebs.
    let w = [x + 5, y, z - 2];
    cmd(&mut s, &format!("fill {} {} {} {} {} {} minecraft:cobweb", w[0], w[1], w[2], w[0] + 3, w[1], w[2]));
    let start = [f64::from(w[0]) + 0.5, f64::from(w[1]) + 0.1, f64::from(w[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{NoGravity:1b,Motion:[0.2d,0d,0d]}}", start[0], start[1], start[2]));
    let E::Other(web) = last_of(&s, "minecraft:minecart") else { panic!() };
    // Ordinary flat rail, negative fire reread.
    let cp = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {} {{Fire:-5s}}", cp[0], cp[1], cp[2]));
    let E::Other(plain) = last_of(&s, "minecraft:minecart") else { panic!() };
    // Fire on netherrack, TNT minecart inside it.
    let f = [x + 10, y, z + 2];
    cmd(&mut s, &format!("setblock {} {} {} minecraft:netherrack", f[0], f[1] - 1, f[2]));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:fire", f[0], f[1], f[2]));
    let fp = [f64::from(f[0]) + 0.5, f64::from(f[1]) + 0.1, f64::from(f[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {} {{NoGravity:1b}}", fp[0], fp[1], fp[2]));
    let E::Other(tnt) = last_of(&s, "minecraft:tnt_minecart") else { panic!() };
    step(&mut s, 1);
    // Tick 1: movement of 0.2 then drag 0.95; tick 2: 0.19 × 0.25, zero speed.
    let c = s.cart_ref(web).expect("minecart");
    assert!((c.pos[0] - (start[0] + 0.2)).abs() < 1.0e-9, "tick 1 : {}", c.pos[0]);
    step(&mut s, 1);
    let c = s.cart_ref(web).expect("minecart");
    let want = start[0] + 0.2 + 0.2 * f64::from(0.95f32) * 0.25;
    assert!((c.pos[0] - want).abs() < 1.0e-9, "stuck: x {} expected {want}", c.pos[0]);
    assert_eq!(c.vel, [0.0; 3], "speed reset to zero");
    assert_eq!(s.cart_ref(plain).map(|c| c.fire), Some(0), "out of fire: back to 0 (only the player has −20)");
    let lit = s.cart_ref(tnt).is_none_or(|c| c.fuse >= 0);
    assert!(lit, "fire primes the TNT minecart");
}

/// Minecart removed under the player: raised to the minecart's height (free spot), not left
/// at the seat 0.4125 lower.
#[test]
fn dismount_when_minecart_vanishes() {
    let Some(mut s) = open("cart-gone") else { return };
    let o = s.pos.map(|v| v.floor() as i32);
    let o = [o[0] + 1, o[1], o[2]];
    track(&mut s, o, 3);
    s.game_mode = 0;
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    assert!(s.cart_ride_forced(id, E::Player));
    step(&mut s, 1);
    s.teleport = None;
    let cy = s.cart_ref(id).expect("minecart").pos[1];
    cmd(&mut s, "kill @e[type=minecraft:minecart]");
    step(&mut s, 1);
    assert_eq!(s.pcart.vehicle, None);
    assert!((s.pos[1] - cy).abs() < 1.0e-9, "y {} expected {cy}", s.pos[1]);
    // `kill`: `entity_die` event of the minecart.
    assert!(s.mobs.rec.vib.log.iter().any(|e| e.0 == "minecraft:entity_die"));
}

/// Projectile direct source: a skull that does not burn knocks over the motionless TNT minecart
/// (over 40 damage) without lighting it, despite the "explosion" type; a burning projectile
/// lights it even with a type without fire.
#[test]
fn tnt_and_direct_source() {
    let Some(mut s) = open("cart-direct") else { return };
    let o = origin(&s);
    track(&mut s, o, 2);
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(a) = last_of(&s, "minecraft:tnt_minecart") else { panic!() };
    let t = crate::projectiles::physics::Target::Cart(a);
    assert!(s.with_direct(Some(false), |s| s.hurt_other(t, "minecraft:explosion", None, 10.0)));
    assert!(s.cart_ref(a).is_none(), "broken and dropped, not primed");
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {}", cp[0] + 1.0, cp[1], cp[2]));
    let E::Other(b) = last_of(&s, "minecraft:tnt_minecart") else { panic!() };
    let t = crate::projectiles::physics::Target::Cart(b);
    assert!(s.with_direct(Some(true), |s| s.hurt_other(t, "minecraft:arrow", None, 1.0)));
    assert!(s.cart_ref(b).is_some_and(|c| c.fuse >= 0), "burning projectile: primed");
}

/// Fall of a TNT minecart: a "frustum" stalagmite does not lengthen the fall (1 block:
/// nothing), the tip does (3.5: explosion).
#[test]
fn fall_onto_stalagmite() {
    let Some(mut s) = open("cart-drip") else { return };
    let o = origin(&s);
    track(&mut s, o, 8);
    let [x, y, z] = o;
    cmd(&mut s, &format!("setblock {} {} {} minecraft:pointed_dripstone[vertical_direction=up,thickness=frustum]", x + 4, y, z + 2));
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {}", f64::from(x + 4) + 0.5, f64::from(y) + 2.0, f64::from(z + 2) + 0.5));
    let E::Other(id) = last_of(&s, "minecraft:tnt_minecart") else { panic!() };
    step(&mut s, 20);
    assert!(s.cart_ref(id).is_some(), "no explosion on a frustum");
    assert!(!s.mobs.rec.vib.log.iter().any(|e| e.0 == "minecraft:explode"));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]", x + 4, y, z - 2));
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {}", f64::from(x + 4) + 0.5, f64::from(y) + 2.0, f64::from(z - 2) + 0.5));
    let E::Other(id) = last_of(&s, "minecraft:tnt_minecart") else { panic!() };
    step(&mut s, 20);
    assert!(s.cart_ref(id).is_none(), "the tip lengthens the fall: explosion");
}

/// Fuse at 0 on the tick of a crash into a wall (remaining horizontal speed ≥ 0.1): two
/// explosions, like the game.
#[test]
fn double_explosion() {
    let Some(mut s) = open("cart-double") else { return };
    let o = origin(&s);
    track(&mut s, o, 8);
    let [x, y, z] = o;
    cmd(&mut s, &format!("fill {} {} {} {} {} {} minecraft:stone", x + 6, y, z - 2, x + 6, y + 2, z + 2));
    let p = [f64::from(x + 5) + 0.5, f64::from(y) + 0.1, f64::from(z + 2) + 0.5];
    cmd(&mut s, &format!("summon minecraft:tnt_minecart {} {} {} {{NoGravity:1b,fuse:0,Motion:[0.3d,0d,-0.3d]}}", p[0], p[1], p[2]));
    s.mobs.rec.vib.log.clear();
    step(&mut s, 1);
    let n = s.mobs.rec.vib.log.iter().filter(|e| e.0 == "minecraft:explode").count();
    assert_eq!(n, 2, "two explosions");
}

/// Powered furnace: the 1-in-4 draw of the smoke advances the minecart's random source on the server side.
#[test]
fn furnace_draws_smoke() {
    let Some(mut s) = open("cart-smoke") else { return };
    let o = origin(&s);
    track(&mut s, o, 3);
    let cp = [f64::from(o[0]) + 1.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:furnace_minecart {} {} {} {{Fuel:100s}}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:furnace_minecart") else { panic!() };
    let mut want = s.cart_ref(id).unwrap().rng;
    step(&mut s, 1);
    let _ = want.next_int_bound(4);
    let mut got = s.cart_ref(id).unwrap().rng;
    assert_eq!(got.next_long(), want.next_long(), "one draw per tick with fuel");
}

/// Command block minecart: the command runs during activation, the rest of the tick starts
/// from its new position (`tp`: the neighboring minecart at the arrival is pushed in the same tick).
#[test]
fn command_during_activation() {
    let Some(mut s) = open("cart-cmd") else { return };
    let o = origin(&s);
    track(&mut s, o, 4);
    let [x, y, z] = o;
    cmd(&mut s, &format!("setblock {} {} {} minecraft:redstone_block", x + 1, y - 1, z));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:activator_rail[shape=east_west,powered=true]", x + 1, y, z));
    // Neighbor ticked BEFORE the command minecart: only the latter's push can move it
    // within this tick (the push acts on both).
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", f64::from(x + 2) + 0.5, y, f64::from(z + 2) + 0.5));
    let E::Other(other) = last_of(&s, "minecraft:minecart") else { panic!() };
    let cp = [f64::from(x + 1) + 0.2, f64::from(y) + 0.0625, f64::from(z) + 0.5];
    cmd(
        &mut s,
        &format!(
            "summon minecraft:command_block_minecart {} {} {} {{Motion:[0.1d,0d,0d],Command:\"tp @s ~ ~ ~2\"}}",
            cp[0], cp[1], cp[2]
        ),
    );
    let E::Other(id) = last_of(&s, "minecraft:command_block_minecart") else { panic!() };
    // First activation at the 4th tick (delay of 4 from 0).
    for _ in 0..4 {
        assert_eq!(s.cart_ref(id).and_then(|c| crate::nbtio::int(&c.nbt, "SuccessCount")), None);
        step(&mut s, 1);
    }
    let c = s.cart_ref(id).expect("minecart");
    assert_eq!(crate::nbtio::int(&c.nbt, "SuccessCount"), Some(1), "command run");
    assert!((c.pos[2] - (cp[2] + 2.0)).abs() < 1.0e-9, "teleported: {:?}", c.pos);
    let v = s.cart_ref(other).expect("neighbour").vel;
    assert!(v[0] != 0.0, "neighbor pushed within the command's tick: {v:?}");
}

/// Sweep hit with a sword on a minecart: the mob stuck to the minecart is hurt.
#[test]
fn sweep_around_minecart() {
    let Some(mut s) = open("cart-sweep") else { return };
    let o = origin(&s);
    track(&mut s, o, 2);
    s.game_mode = 0;
    let cp = [f64::from(o[0]) + 0.5, f64::from(o[1]), f64::from(o[2]) + 0.5];
    cmd(&mut s, &format!("summon minecraft:minecart {} {} {}", cp[0], cp[1], cp[2]));
    let E::Other(id) = last_of(&s, "minecraft:minecart") else { panic!() };
    cmd(&mut s, &format!("summon minecraft:zombie {} {} {} {{NoAI:1b}}", cp[0] + 1.0, cp[1], cp[2]));
    let E::Mob(zid) = last_of(&s, "minecraft:zombie") else { panic!() };
    s.pos = [cp[0] - 1.5, cp[1], cp[2]];
    s.on_ground = true;
    s.proj_player.known_move = [0.0; 3];
    s.slots[36 + s.held as usize] = Some(Stack { id: "minecraft:iron_sword".into(), count: 1, components: None });
    s.mobs.attack_ticker = 100;
    assert!(s.cart_attacked(id));
    let h = s.mob_ref(zid).map(|m| m.health).expect("zombie");
    assert!(h < 20.0, "zombie swept: {h}");
}

/// Bench (run by hand: `-- --ignored minecart_bench --nocapture`): tick of 64 minecarts
/// in motion on a track, and search for a container minecart by an inactive hopper (called
/// on every tick by any hopper without a neighboring block container).
#[test]
#[ignore = "bench"]
fn minecart_bench() {
    use std::time::Instant;
    let Some(mut s) = open("cart-bench") else { return };
    let o = origin(&s);
    track(&mut s, o, 70);
    for i in 0..64 {
        let x = f64::from(o[0] + i) + 0.5;
        cmd(&mut s, &format!("summon minecraft:chest_minecart {} {} {} {{Motion:[0.3d,0d,0d]}}", x, o[1], f64::from(o[2]) + 0.5));
    }
    let n = 2_000;
    let t = Instant::now();
    for _ in 0..n {
        s.minecarts_tick();
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    eprintln!("tick of 64 minecarts: {per:.2} µs");
    let probe = [o[0] - 5, o[1], o[2]];
    let m = 200_000;
    let t = Instant::now();
    let mut hits = 0;
    for _ in 0..m {
        hits += usize::from(s.cart_container_at(std::hint::black_box(probe)).is_some());
    }
    let per = t.elapsed().as_secs_f64() * 1e9 / f64::from(m);
    eprintln!("search by a hopper (64 minecarts, none touched): {per:.1} ns ({hits})");
    let probe = s.carts.list.iter().flatten().next().map(|c| c.block_pos()).unwrap();
    let t = Instant::now();
    let mut hits = 0;
    for _ in 0..m {
        hits += usize::from(s.cart_container_at(std::hint::black_box(probe)).is_some());
    }
    let per = t.elapsed().as_secs_f64() * 1e9 / f64::from(m);
    eprintln!("search by a hopper (64 minecarts, one touched): {per:.1} ns ({hits})");
}
