//! Boats and rafts of the integrated server: expected values computed by hand from the game's
//! rules (no value read off our own code). Test world required (`RUSTCRAFT_SOLO_WORLD`).

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::commands::ents::E;
use crate::minecart::CKind;
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Boat box height and the buoyancy's equilibrium: the boat floats when (surface − y) / height
/// is 0.65.
const HEIGHT: f64 = 0.5625;
const FLOATING_DEPTH: f64 = 0.65 * HEIGHT;

fn open(tag: &str) -> Option<Server> {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data missing: test skipped");
        return None;
    };
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut s = crate::portal_tests::open_world(&dir, std::sync::Arc::new(data));
    s.loaded = true;
    s.teleport = None;
    s.game_mode = 0;
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
    assert!(ok || !c.starts_with("summon"), "command refused: {c}");
}

/// Origin near the player: the water row of the pool.
fn origin(s: &Server) -> [i32; 3] {
    let b = s.pos.map(|v| v.floor() as i32);
    [b[0] + 3, b[1], b[2] + 3]
}

/// A pool of still water sources at row `o[1]` over x ∈ [x−3, x+3], z ∈ [z, z+8] (stone under it),
/// a stone rim at z − 2 … z − 1 to stand on, air above.
fn pool(s: &mut Server, o: [i32; 3]) {
    let [x, y, z] = o;
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:air", x - 4, y, z - 3, x + 4, y + 4, z + 9));
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:stone", x - 4, y - 1, z - 3, x + 4, y - 1, z + 9));
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:stone", x - 4, y, z - 2, x + 4, y, z - 1));
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:water", x - 3, y, z, x + 3, y, z + 8));
}

/// The surface of a still water source with air above: 8/9 of the block, read in single precision.
fn surface(o: [i32; 3]) -> f64 {
    f64::from(o[1] as f32 + 8.0f32 / 9.0)
}

fn last_boat(s: &Server, ty: &str) -> i32 {
    let found = s.all_ents().into_iter().rfind(|e| s.ent_type(*e) == ty).expect("summoned boat");
    let E::Other(id) = found else { panic!("{ty} is not a vehicle: {found:?}") };
    assert!(s.cart_ref(id).is_some_and(|c| c.kind.is_boat()), "{ty} is not a simulated boat");
    id
}

fn summon_floating(s: &mut Server, ty: &str, at: [f64; 2], o: [i32; 3], nbt: &str) -> i32 {
    let y = surface(o) - FLOATING_DEPTH;
    cmd(s, format!("summon {ty} {} {y} {} {nbt}", at[0], at[1]).trim_end());
    last_boat(s, ty)
}

/// The boat's own state carried by every vehicle: kept small (`docs/perf.md`, L16-BATEAUX).
#[test]
fn boat_state_stays_small() {
    assert_eq!(std::mem::size_of::<super::BoatState>(), 48);
}

/// Clicking water with a boat item: the boat appears where the item ray meets the water,
/// facing the player's yaw; one item is used up, the "used" statistic and `entity_place` follow.
#[test]
fn boat_item_places_a_boat_on_the_water_it_aims_at() {
    let Some(mut s) = open("boat-item") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    // Standing on the rim, looking south at the water 2.5 blocks ahead.
    s.pos = [f64::from(o[0]) + 0.5, f64::from(o[1] + 1), f64::from(o[2]) - 0.5];
    s.yaw = 0.0;
    let drop = s.player_eye_y() - surface(o);
    s.pitch = drop.atan2(2.5).to_degrees() as f32;
    s.held = 0;
    s.slots[36] = Some(Stack { id: "minecraft:oak_boat".into(), count: 2, components: None });
    let aimed = s.item_ray(rc_physics::ray::Fluids::Any);
    assert!(!aimed.miss, "the ray reaches the water");
    let events = s.mobs.rec.vib.log.len();
    assert!(s.ray_item_use(0), "the boat item is handled by the server");
    let id = last_boat(&s, "minecraft:oak_boat");
    let c = s.cart_ref(id).expect("placed boat");
    assert_eq!(c.pos, aimed.at, "placed where the ray meets the water");
    // The ray meets the fluid's shape in double precision: the block plus 8/9.
    let shape_top = f64::from(o[1]) + f64::from(8.0f32 / 9.0);
    assert!((c.pos[1] - shape_top).abs() < 1e-9, "on the surface: {}", c.pos[1]);
    assert_eq!(c.yaw, 0.0, "facing the player's yaw");
    assert_eq!(s.slots[36].as_ref().map(|st| st.count), Some(1), "one boat used up");
    assert_eq!(s.prog.stats.get("minecraft:used", "minecraft:oak_boat"), 1);
    let placed = s.mobs.rec.vib.log[events..].iter().any(|e| e.0 == "minecraft:entity_place" && e.1 == aimed.at);
    assert!(placed, "entity_place at the hit point");
    // Creative, aiming south-west at free water: a second boat, and the item stays.
    s.game_mode = 1;
    s.yaw = 60.0;
    let boats = |s: &Server| s.carts.list.iter().flatten().filter(|c| c.kind.is_boat()).count();
    assert_eq!(boats(&s), 1);
    assert!(s.ray_item_use(0));
    assert_eq!(boats(&s), 2, "second boat placed");
    assert_eq!(s.slots[36].as_ref().map(|st| st.count), Some(1), "creative keeps the item");
}

/// A boat summoned in still water: each tick its velocity loses the gravity (0.04) and gains the
/// buoyancy (surface − y) / height × 0.04 / 0.65, then × 0.75; it settles 0.65 × height below
/// the surface. A new boat has no previous status: no splashdown on its first tick.
#[test]
fn boat_floats_and_settles_on_still_water() {
    let Some(mut s) = open("boat-float") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let level = surface(o);
    let start = level - 0.29;
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    cmd(&mut s, &format!("summon minecraft:birch_boat {x} {start} {z}"));
    let id = last_boat(&s, "minecraft:birch_boat");
    let (mut y, mut vy) = (start, 0.0f64);
    for t in 1..=12 {
        step(&mut s, 1);
        let buoyancy = (level - y) / HEIGHT;
        vy = (vy - 0.04 + buoyancy * (0.04 / 0.65)) * 0.75;
        y += vy;
        let c = s.cart_ref(id).expect("boat");
        assert!((c.pos[1] - y).abs() < 1e-9, "tick {t}: y {} expected {y}", c.pos[1]);
        assert_eq!(c.boat.float.status, rc_physics::mounts::Status::InWater, "tick {t}");
        assert_eq!([c.pos[0], c.pos[2]], [x, z], "tick {t}: no drift in still water");
    }
    step(&mut s, 200);
    let c = s.cart_ref(id).expect("boat");
    assert!((c.pos[1] - (level - FLOATING_DEPTH)).abs() < 1e-3, "settled at {}", c.pos[1]);
}

/// Right click boards the boat (seat at a third of its height, less the player's 0.6); the paddle
/// packet sets the paddle data; the server holds a piloted boat still and takes the position its
/// pilot sends (`move_vehicle`); sneaking gets the player off beside it, the stat counts the trip.
#[test]
fn player_boards_paddles_and_pilots_a_boat() {
    let Some(mut s) = open("boat-pilot") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    let id = summon_floating(&mut s, "minecraft:oak_boat", [x, z], o, "");
    let boat_y = s.cart_ref(id).expect("boat").pos[1];
    s.pos = [x - 1.0, f64::from(o[1]) + 1.0, z];
    assert!(s.cart_interact(id, 0), "interaction taken by the boat");
    assert_eq!(s.pcart.vehicle, Some(id), "player on board");
    assert_eq!(s.cart_ref(id).map(|c| c.passengers.clone()), Some(vec![Ent::Player]));
    let seat = [x, boat_y + 0.5625f32 as f64 / 3.0 - 0.6, z];
    assert!((0..3).all(|k| (s.pos[k] - seat[k]).abs() < 1e-6), "seat {:?} expected {seat:?}", s.pos);
    s.boat_paddles_from_pilot(true, false);
    let c = s.cart_ref(id).expect("boat");
    assert_eq!((c.meta(11), c.meta(12)), (Some(&Meta::Bool(true)), Some(&Meta::Bool(false))), "paddle data");
    s.teleport = None;
    step(&mut s, 1);
    let c = s.cart_ref(id).expect("boat");
    assert_eq!(c.vel, [0.0; 3], "a piloted boat is held still by the server");
    assert_eq!(c.boat.paddle_angles, [std::f32::consts::PI / 8.0, 0.0], "the left paddle turns");
    let to = [c.pos[0] + 0.3, c.pos[1], c.pos[2] - 0.2];
    s.move_vehicle(to, 90.0, 0.0, false);
    let c = s.cart_ref(id).expect("boat");
    assert_eq!((c.pos, c.yaw), (to, 90.0), "the pilot's position is taken");
    s.sneaking = true;
    step(&mut s, 1);
    assert_eq!(s.pcart.vehicle, None, "off while sneaking");
    assert!(s.cart_ref(id).is_some_and(|c| c.passengers.is_empty()));
}

/// Hits add 10 damage each (a fist: 1 point); past 40 the boat breaks into its item. A chest
/// boat drops its contents too.
#[test]
fn chest_boat_breaks_into_its_item_and_its_contents() {
    let Some(mut s) = open("boat-break") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    let items = "{Items:[{Slot:0b,id:\"minecraft:dirt\",count:5},{Slot:20b,id:\"minecraft:stone\",count:2}]}";
    let id = summon_floating(&mut s, "minecraft:oak_chest_boat", [x, z], o, items);
    s.pos = [x - 1.5, f64::from(o[1]) + 1.0, z];
    let drops = s.life.drops.len();
    for hit in 1..=4 {
        s.mobs.attack_ticker = 100;
        assert!(s.cart_attacked(id), "hit taken by the boat");
        assert!(s.cart_ref(id).is_some_and(|c| c.damage == 10.0 * hit as f32), "hit {hit}");
    }
    s.mobs.attack_ticker = 100;
    s.cart_attacked(id);
    assert!(s.cart_ref(id).is_none(), "broken at the 5th hit (50 > 40)");
    let count = |item: &str| -> i32 {
        s.life.drops[drops..].iter().filter(|d| d.stack.id == item).map(|d| d.stack.count).sum()
    };
    assert_eq!(count("minecraft:oak_chest_boat"), 1);
    assert_eq!((count("minecraft:dirt"), count("minecraft:stone")), (5, 2));
}

/// A chest boat seats one: right click boards it; sneaking opens its chest of three rows instead.
#[test]
fn chest_boat_opens_its_chest_when_sneaking() {
    let Some(mut s) = open("boat-chest") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    let id = summon_floating(&mut s, "minecraft:spruce_chest_boat", [x, z], o, "");
    s.pos = [x - 1.0, f64::from(o[1]) + 1.0, z];
    s.sneaking = true;
    assert!(s.cart_interact(id, 0));
    assert_eq!(s.pcart.vehicle, None, "sneaking does not board");
    let menu = s.life.menu.as_ref().expect("chest open");
    assert_eq!((menu.cart, menu.slots.len()), (Some(id), 27 + 36), "the boat's 27 slots then the inventory");
}

/// The save of a chest boat: the entity's common fields then its chest, without the minecart's
/// own fields; read back unchanged.
#[test]
fn chest_boat_saves_in_the_game_format() {
    let Some(mut s) = open("boat-save") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    let items = "{Items:[{Slot:3b,id:\"minecraft:apple\",count:4}]}";
    let id = summon_floating(&mut s, "minecraft:bamboo_chest_raft", [x, z], o, items);
    let saved = s.cart_to_nbt(s.cart_ref(id).expect("raft"));
    let Nbt::Compound(fields) = &saved else { panic!("compound expected") };
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    let expected = [
        "id",
        "Pos",
        "Motion",
        "Rotation",
        "fall_distance",
        "Fire",
        "Air",
        "OnGround",
        "Invulnerable",
        "PortalCooldown",
        "UUID",
        "Items",
    ];
    assert_eq!(keys, expected);
    let again = s.cart_from_nbt(&saved).expect("read back");
    assert_eq!(again.kind, CKind::from_name("minecraft:bamboo_chest_raft").expect("kind"));
    assert_eq!(s.cart_to_nbt(&again), saved, "read back unchanged");
}

/// A mob touching a boat no player pilots boards it (a free seat, narrower than the boat).
#[test]
fn mob_touching_a_boat_boards_it() {
    let Some(mut s) = open("boat-board") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    let id = summon_floating(&mut s, "minecraft:acacia_boat", [x, z], o, "");
    let y = s.cart_ref(id).expect("boat").pos[1];
    cmd(&mut s, &format!("summon minecraft:zombie {} {y} {z} {{NoAI:1b,PersistenceRequired:1b}}", x + 0.9));
    let E::Mob(zombie) = s.all_ents().into_iter().rfind(|e| s.ent_type(*e) == "minecraft:zombie").expect("zombie")
    else {
        panic!("zombie expected")
    };
    step(&mut s, 1);
    assert_eq!(s.mob_ref(zombie).and_then(|m| m.mount.vehicle), Some(id), "the zombie boarded");
    assert_eq!(s.cart_ref(id).map(|c| c.passengers.clone()), Some(vec![Ent::Mob(zombie)]));
}

/// Every entity collides with a boat as with a block, never the boat itself. The solid entity
/// searches walk the vehicle list only while it holds a boat: the flag that says so follows the
/// boats in and out (both paths of `RUSTCRAFT_OPT_SKIP_VEHICLES_WITHOUT_BOATS` then agree).
#[test]
fn boats_are_solid_to_other_entities() {
    use crate::solid_entities::RoomFor;
    let Some(mut s) = open("boat-solid") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    step(&mut s, 1);
    assert!(!s.carts.any_boat, "no boat in the test world");
    let (x, z) = (f64::from(o[0]) + 0.5, f64::from(o[2]) + 4.5);
    let id = summon_floating(&mut s, "minecraft:cherry_boat", [x, z], o, "");
    assert!(s.carts.any_boat, "set when the boat enters the list");
    let c = s.cart_ref(id).expect("boat");
    let b = c.bbox();
    let on_top = rc_physics::Aabb { min: [b.min[0], b.max[1] - 0.1, b.min[2]], max: [b.max[0], b.max[1] + 0.5, b.max[2]] };
    assert!(!s.room_free(RoomFor::NoEntity, &on_top), "a box overlapping the boat is not free");
    assert!(s.room_free(Server::room_for_object(id, c.pos), &on_top), "the boat is not solid to itself");
    step(&mut s, 1);
    assert!(s.carts.any_boat, "kept while the boat lives");
    cmd(&mut s, "kill @e[type=minecraft:cherry_boat]");
    step(&mut s, 1);
    assert!(!s.carts.any_boat, "cleared once the boat is gone");
    assert!(s.room_free(RoomFor::NoEntity, &on_top));
}

/// Over the top of a bubble column that pulls down (nothing above it), a boat waits: 60 at the first
/// contact, one less each tick; the tick it reaches 0 the passengers are thrown out and the boat is
/// dragged down by 0.7.
#[test]
fn bubble_column_pulling_down_sinks_a_boat_after_sixty_ticks() {
    let Some(mut s) = open("boat-bubbles") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    let col = [o[0], o[1], o[2] + 4];
    cmd(&mut s, &format!("setblock {} {} {} minecraft:magma_block", col[0], col[1] - 1, col[2]));
    cmd(&mut s, &format!("setblock {} {} {} minecraft:bubble_column[drag=true]", col[0], col[1], col[2]));
    let at = [f64::from(col[0]) + 0.5, f64::from(col[2]) + 0.5];
    let passenger = "{Passengers:[{id:\"minecraft:zombie\",NoAI:1b,PersistenceRequired:1b}]}";
    let id = summon_floating(&mut s, "minecraft:jungle_boat", at, o, passenger);
    assert_eq!(s.cart_ref(id).map(|c| c.passengers.len()), Some(1), "the zombie rides it");
    step(&mut s, 1);
    assert_eq!(s.cart_ref(id).map(|c| c.bubble_time()), Some(59), "60 at the first contact, then one less");
    step(&mut s, 58);
    let c = s.cart_ref(id).expect("boat");
    assert_eq!((c.bubble_time(), c.passengers.len()), (1, 1), "still waiting");
    let vy = c.vel[1];
    step(&mut s, 1);
    let c = s.cart_ref(id).expect("boat");
    assert_eq!((c.bubble_time(), c.passengers.len()), (0, 0), "passenger thrown out");
    assert!(c.vel[1] < vy - 0.6, "dragged down: {} after {vy}", c.vel[1]);
}

/// A dispenser facing water places the boat just outside its face (0.5625 + half the boat's
/// width from its centre), one block up, facing away; a dispenser facing a wall throws the item.
#[test]
fn dispenser_places_a_boat_on_the_water_in_front() {
    let Some(mut s) = open("boat-dispense") else { return };
    let o = origin(&s);
    pool(&mut s, o);
    // A dispenser on the rim facing south, water in front.
    let p = [o[0], o[1], o[2] - 1];
    cmd(&mut s, &format!("setblock {} {} {} minecraft:dispenser[facing=south]", p[0], p[1], p[2]));
    let stack = Stack { id: "minecraft:mangrove_boat".into(), count: 3, components: None };
    let rest = s.dispense_item(p, crate::blocks::Dir::South, 0, stack.clone());
    assert_eq!(rest.map(|st| st.count), Some(2), "one boat used");
    let id = last_boat(&s, "minecraft:mangrove_boat");
    let c = s.cart_ref(id).expect("dispensed boat");
    let expected = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 1.5, f64::from(p[2]) + 0.5 + 0.5625 + 0.6875];
    assert_eq!((c.pos, c.yaw), (expected, 0.0));
    // Facing the stone rim: thrown as an item.
    let drops = s.life.drops.len();
    let rest = s.dispense_item(p, crate::blocks::Dir::West, 0, stack);
    assert_eq!(rest.map(|st| st.count), Some(2));
    assert!(s.life.drops[drops..].iter().any(|d| d.stack.id == "minecraft:mangrove_boat"), "item thrown");
}
