//! Decorations on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`): placement, survival, breaking,
//! frames, armor stands, cushions, network data. World absent: tests ignored.

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::DKind;
use crate::blocks::Dir;
use crate::blocks::probe::Probe;
use crate::nbtio;
use crate::player::Stack;

const BASE: &str = "fill 96 -60 96 115 -60 115 minecraft:stone\nfill 96 -59 96 115 -45 115 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [104.5, -59.0, 104.5];
    p.s.yaw = 0.0;
    p.s.pitch = 0.0;
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.dead = false;
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    p.s.life.drops.clear();
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn hold(p: &mut Probe, id: &str, n: i32) {
    let h = 36 + p.s.held as usize;
    p.s.slots[h] = Some(Stack { id: id.to_owned(), count: n, components: None });
}

/// Decorations present (identifier, compound).
fn decos(p: &Probe, k: DKind) -> Vec<(i32, Nbt)> {
    p.s.entities
        .values()
        .flat_map(|l| l.iter())
        .filter(|(_, e)| super::kind_of(e) == Some(k))
        .cloned()
        .collect()
}

fn dropped(p: &Probe, id: &str) -> i32 {
    p.s.life.drops.iter().filter(|d| d.stack.id == id && !d.is_removed()).map(|d| d.stack.count).sum()
}

fn ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.time += 1;
        p.s.deco_tick();
    }
}

fn meta_of(p: &Probe, id: i32) -> Vec<(u8, Meta)> {
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            rc_client::Event::Entity(rc_client::EntityEvent::Data { id: i, values }) if i == id => Some(values),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Painting on a one-block wall: only a 1 × 1 variant holds; game compound (face,
/// hanging block, center), appearance at the corner of the block with the 3D direction; the wall removed, it
/// drops as an item at the next check (101 ticks), not before.
#[test]
fn painting_placed_survives_and_pops() {
    let Some((mut p, dir)) = probe("deco-painting") else { return };
    p.run("setblock 106 -58 104 minecraft:stone");
    hold(&mut p, "minecraft:painting", 2);
    p.take_events();
    // West face of the block: the painting looks west, hung at 105 -58 104.
    p.s.use_item_on_block([106, -58, 104], Dir::West, [0.0, 0.5, 0.5]);
    let list = decos(&p, DKind::Painting);
    assert_eq!(list.len(), 1, "painting not placed");
    let (id, e) = &list[0];
    assert_eq!(nbtio::int(e, "facing"), Some(1), "2D facing west");
    assert_eq!(e.get("block_pos"), Some(&Nbt::IntArray(vec![105, -58, 104])));
    let (w, h) = p.s.painting_size(e);
    assert_eq!((w, h), (1, 1), "only a 1 × 1 variant holds on a block");
    assert!(p.s.p_tag("minecraft:painting_variant", "minecraft:placeable", e.get("variant").and_then(Nbt::as_str).unwrap()));
    let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap();
    assert_eq!(pos, [105.0 + 0.5 + 0.46875, -57.5, 104.5], "center of the painting against the wall");
    assert_eq!(p.s.slots[36 + p.s.held as usize].as_ref().map(|s| s.count), Some(1), "item consumed");
    let add = p.take_events().into_iter().find_map(|ev| match ev {
        rc_client::Event::Entity(rc_client::EntityEvent::Add(a)) if a.id == *id => Some(a),
        _ => None,
    });
    let add = add.expect("appearance announced");
    assert_eq!(add.pos, [105.0, -58.0, 104.0], "corner of the hanging block");
    assert_eq!(add.data, Dir::West as i32, "3D direction");
    // On the floor: refused (painting only on a wall).
    p.s.use_item_on_block([104, -60, 106], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(decos(&p, DKind::Painting).len(), 1);
    // Wall removed: nothing before the check, then the item drops.
    p.run("setblock 106 -58 104 minecraft:air");
    ticks(&mut p, 100);
    assert_eq!(decos(&p, DKind::Painting).len(), 1, "checked too early");
    ticks(&mut p, 1);
    assert!(decos(&p, DKind::Painting).is_empty(), "painting without support stayed hung");
    assert_eq!(dropped(&p, "minecraft:painting"), 1, "painting item dropped");
    done(p, dir);
}

/// Frame on the floor: item placed (consumed), turned by an eighth; the first hit knocks off
/// the item alone, the second breaks the frame (frame item dropped).
#[test]
fn item_frame_holds_rotates_and_breaks() {
    let Some((mut p, dir)) = probe("deco-frame") else { return };
    hold(&mut p, "minecraft:item_frame", 1);
    p.s.use_item_on_block([105, -60, 105], Dir::Up, [0.5, 1.0, 0.5]);
    let list = decos(&p, DKind::Frame);
    assert_eq!(list.len(), 1, "frame not placed");
    let (id, e) = list[0].clone();
    assert_eq!(nbtio::int(&e, "Facing"), Some(1), "frame turned upwards");
    assert_eq!(nbtio::f32s::<2>(e.get("Rotation")), Some([0.0, -90.0]));
    assert!(p.s.slots[36 + p.s.held as usize].is_none(), "frame consumed");
    hold(&mut p, "minecraft:diamond", 3);
    p.take_events();
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    let e = decos(&p, DKind::Frame)[0].1.clone();
    assert_eq!(e.get("Item").and_then(Stack::from_nbt).map(|s| (s.id, s.count)), Some(("minecraft:diamond".into(), 1)));
    assert_eq!(p.s.slots[36 + p.s.held as usize].as_ref().map(|s| s.count), Some(2));
    assert!(meta_of(&p, id).iter().any(|(i, m)| *i == 9 && matches!(m, Meta::Item(Some(_)))), "item sent");
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert_eq!(nbtio::int(&decos(&p, DKind::Frame)[0].1, "ItemRotation"), Some(1));
    assert!(p.s.deco_attacked(id));
    assert_eq!(decos(&p, DKind::Frame).len(), 1, "the frame stays on the first hit");
    assert_eq!(dropped(&p, "minecraft:diamond"), 1);
    assert!(p.s.deco_attacked(id));
    assert!(decos(&p, DKind::Frame).is_empty());
    assert_eq!(dropped(&p, "minecraft:item_frame"), 1);
    done(p, dir);
}

/// Armor stand: placed on top, turned by an eighth of a turn; helmet placed on click;
/// a hit makes it wobble (event 32), a second within 5 ticks breaks it (stand and
/// helmet dropped).
#[test]
fn armor_stand_equips_and_breaks_in_two_hits() {
    let Some((mut p, dir)) = probe("deco-stand") else { return };
    p.s.yaw = 30.0;
    hold(&mut p, "minecraft:armor_stand", 1);
    p.s.use_item_on_block([106, -60, 106], Dir::Up, [0.5, 1.0, 0.5]);
    let list = decos(&p, DKind::Stand);
    assert_eq!(list.len(), 1, "stand not placed");
    let (id, e) = list[0].clone();
    assert_eq!(nbtio::f64s::<3>(e.get("Pos")), Some([106.5, -59.0, 106.5]));
    // 30 − 180 = −150 → rounded to the eighth: −135.
    assert_eq!(nbtio::f32s::<2>(e.get("Rotation")), Some([-135.0, 0.0]));
    // Second placement at the same spot: refused (entity in the box).
    hold(&mut p, "minecraft:armor_stand", 1);
    p.s.use_item_on_block([106, -60, 106], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(decos(&p, DKind::Stand).len(), 1);
    hold(&mut p, "minecraft:diamond_helmet", 1);
    assert!(p.s.deco_interact(id, 0, [0.0, 1.8, 0.0], false));
    let e = decos(&p, DKind::Stand)[0].1.clone();
    assert_eq!(e.get("equipment").and_then(|q| q.get("head")).and_then(Stack::from_nbt).map(|s| s.id), Some("minecraft:diamond_helmet".into()));
    assert!(p.s.slots[36 + p.s.held as usize].is_none(), "helmet passed to the stand");
    p.take_events();
    p.s.mobs.attack_ticker = 100;
    assert!(p.s.deco_attacked(id));
    let shook = p.take_events().iter().any(|ev| {
        matches!(ev, rc_client::Event::Entity(rc_client::EntityEvent::Event { id: i, event: 32 }) if *i == id)
    });
    assert!(shook, "event 32 missing");
    assert_eq!(decos(&p, DKind::Stand).len(), 1, "broken on the first hit");
    p.s.time += 3;
    p.s.mobs.attack_ticker = 100;
    assert!(p.s.deco_attacked(id));
    assert!(decos(&p, DKind::Stand).is_empty(), "not broken on the second hit");
    assert_eq!(dropped(&p, "minecraft:armor_stand"), 1);
    assert_eq!(dropped(&p, "minecraft:diamond_helmet"), 1);
    done(p, dir);
}

/// Cushion: placed on top of a block with the item's color; refused on a side;
/// anchor removed, it drops as an item at the check; network data: color (index 8).
#[test]
fn cushion_needs_an_anchor() {
    let Some((mut p, dir)) = probe("deco-cushion") else { return };
    p.run("setblock 106 -59 104 minecraft:stone");
    hold(&mut p, "minecraft:red_cushion", 1);
    p.s.use_item_on_block([106, -59, 104], Dir::West, [0.0, 0.5, 0.5]);
    assert!(decos(&p, DKind::Cushion).is_empty(), "placed on a side");
    p.take_events();
    p.s.use_item_on_block([106, -59, 104], Dir::Up, [0.5, 1.0, 0.5]);
    let list = decos(&p, DKind::Cushion);
    assert_eq!(list.len(), 1, "cushion not placed");
    let (id, e) = list[0].clone();
    assert_eq!(e.get("color").and_then(Nbt::as_str), Some("red"));
    assert_eq!(nbtio::f64s::<3>(e.get("Pos")), Some([106.5, -58.0, 104.5]));
    assert!(meta_of(&p, id).iter().any(|(i, m)| *i == 8 && *m == Meta::Variant(14, 43)), "color sent");
    p.run("setblock 106 -59 104 minecraft:air");
    ticks(&mut p, 101);
    assert!(decos(&p, DKind::Cushion).is_empty(), "cushion without anchor stayed");
    assert_eq!(dropped(&p, "minecraft:red_cushion"), 1);
    done(p, dir);
}

/// Cushion: the player sits on it (seat 0.35 below the top of the cushion… minus the attachment),
/// announced as passenger; sneaking, they get up onto the top of the cushion, re-ride delay of 60.
#[test]
fn cushion_seats_the_player() {
    let Some((mut p, dir)) = probe("deco-seat") else { return };
    hold(&mut p, "minecraft:blue_cushion", 1);
    p.s.use_item_on_block([105, -60, 105], Dir::Up, [0.5, 1.0, 0.5]);
    let (id, _) = decos(&p, DKind::Cushion)[0].clone();
    p.take_events();
    // Sneaking: no seat.
    assert!(p.s.deco_interact(id, 0, [0.0; 3], true));
    assert_eq!(p.s.deco.seat, None);
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert_eq!(p.s.deco.seat, Some(id));
    let player = p.s.player_id;
    let mounted = p.take_events().iter().any(|ev| {
        matches!(ev, rc_client::Event::Entity(rc_client::EntityEvent::Passengers { vehicle, passengers })
            if *vehicle == id && passengers == &vec![player])
    });
    assert!(mounted, "passenger not announced");
    p.s.teleport = None;
    ticks(&mut p, 1);
    assert_eq!(p.s.pos, [105.5, -59.0 + 0.25 - 0.6, 105.5], "seated position");
    p.s.sneaking = true;
    ticks(&mut p, 1);
    assert_eq!(p.s.deco.seat, None, "sneaking without getting up");
    assert_eq!(p.s.pos, [105.5, -58.75, 105.5], "placed on the cushion");
    // The forced dismount of the player from their vehicle resets the player's delay to 0 after the 60 of a passenger removal.
    assert_eq!(p.s.mobs.mounts.riding.boarding_cooldown, 0);
    done(p, dir);
}

/// Armor stand on a removed pillar: it falls to the ground (gravity 0.08, drag 0.98)
/// and stays there, resting velocity −0.0784.
#[test]
fn armor_stand_falls_when_its_support_goes() {
    let Some((mut p, dir)) = probe("deco-stand-fall") else { return };
    p.run("fill 106 -59 106 106 -56 106 minecraft:stone");
    hold(&mut p, "minecraft:armor_stand", 1);
    p.s.use_item_on_block([106, -56, 106], Dir::Up, [0.5, 1.0, 0.5]);
    let (id, e) = decos(&p, DKind::Stand)[0].clone();
    assert_eq!(nbtio::f64s::<3>(e.get("Pos")).map(|v| v[1]), Some(-55.0));
    ticks(&mut p, 3);
    assert_eq!(nbtio::f64s::<3>(decos(&p, DKind::Stand)[0].1.get("Pos")).map(|v| v[1]), Some(-55.0), "placed, it does not move");
    p.run("fill 106 -59 106 106 -56 106 minecraft:air");
    ticks(&mut p, 60);
    let e = decos(&p, DKind::Stand).into_iter().find(|(i, _)| *i == id).expect("stand").1;
    assert_eq!(nbtio::f64s::<3>(e.get("Pos")).map(|v| v[1]), Some(-59.0), "not on the ground");
    assert_eq!(nbtio::int(&e, "OnGround"), Some(1));
    assert_eq!(nbtio::motion(e.get("Motion")).map(|v| v[1]), Some(-0.08 * f64::from(0.98f32)));
    done(p, dir);
}

/// Explosion that touches block-like entities: painting broken (item dropped); without that, or
/// direct source in water, it stays. Water at the center does not count (only the direct
/// source), and an explosion by a creative player drops nothing.
#[test]
fn explosion_breaks_hanging_things() {
    use crate::mobs::mob::Ent;
    let Some((mut p, dir)) = probe("deco-boom") else { return };
    p.run("setblock 106 -58 104 minecraft:stone");
    hold(&mut p, "minecraft:painting", 2);
    p.s.use_item_on_block([106, -58, 104], Dir::West, [0.0, 0.5, 0.5]);
    assert_eq!(decos(&p, DKind::Painting).len(), 1);
    p.s.deco_explosion([103.5, -58.0, 104.5], 4.0, None, false, false);
    assert_eq!(decos(&p, DKind::Painting).len(), 1, "explosion without block-like entities");
    p.s.deco_explosion([103.5, -58.0, 104.5], 4.0, None, true, true);
    assert_eq!(decos(&p, DKind::Painting).len(), 1, "direct source in water");
    p.run("setblock 103 -58 104 minecraft:water");
    p.s.deco_explosion([103.5, -58.0, 104.5], 4.0, None, false, true);
    assert!(decos(&p, DKind::Painting).is_empty(), "water at the center, dry source: painting stayed");
    assert_eq!(dropped(&p, "minecraft:painting"), 1);
    // Player author in creative: broken, nothing drops.
    p.run("setblock 103 -58 104 minecraft:air");
    p.s.use_item_on_block([106, -58, 104], Dir::West, [0.0, 0.5, 0.5]);
    assert_eq!(decos(&p, DKind::Painting).len(), 1);
    p.s.game_mode = 1;
    p.s.deco_explosion([103.5, -58.0, 104.5], 4.0, Some(Ent::Player), false, true);
    assert!(decos(&p, DKind::Painting).is_empty());
    assert_eq!(dropped(&p, "minecraft:painting"), 1, "item dropped for a creative author");
    done(p, dir);
}

/// Support in an unloaded neighbouring chunk: the entity is not simulated ("entity ticking"),
/// the painting stays hung instead of reading void.
#[test]
fn hanging_waits_for_loaded_neighbours() {
    let Some((mut p, dir)) = probe("deco-edge") else { return };
    p.run("fill 80 -40 80 127 -40 127 minecraft:air");
    p.run("setblock 112 -58 104 minecraft:stone");
    hold(&mut p, "minecraft:painting", 1);
    p.s.use_item_on_block([112, -58, 104], Dir::West, [0.0, 0.5, 0.5]);
    assert_eq!(decos(&p, DKind::Painting).len(), 1, "painting not placed");
    // The wall's chunk (7, 6) leaves memory; the painting's chunk (6, 6) stays simulated.
    assert!(p.s.chunks.remove(&(7, 6)).is_some());
    ticks(&mut p, 202);
    assert_eq!(decos(&p, DKind::Painting).len(), 1, "painting fell: support read in an absent chunk");
    assert_eq!(dropped(&p, "minecraft:painting"), 0);
    done(p, dir);
}

/// Sitting on a cushion: a `/tp` makes them get off (they stay at the arrival point), switching
/// to spectator too (placed on the cushion); a seated player does not take a portal.
#[test]
fn cushion_seat_cleared_by_teleport_and_spectator() {
    let Some((mut p, dir)) = probe("deco-seat-tp") else { return };
    hold(&mut p, "minecraft:blue_cushion", 1);
    p.s.use_item_on_block([105, -60, 105], Dir::Up, [0.5, 1.0, 0.5]);
    let (id, _) = decos(&p, DKind::Cushion)[0].clone();
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert_eq!(p.s.deco.seat, Some(id));
    let mut src = p.s.player_source();
    src.level = 4;
    let (ok, _) = p.s.perform(src.clone(), "tp @s 110 -59 110");
    assert!(ok, "teleportation refused");
    assert_eq!(p.s.deco.seat, None, "still seated after /tp");
    p.s.teleport = None;
    ticks(&mut p, 3);
    // Integer coordinates centered on the block (position argument of the game).
    assert_eq!(p.s.pos, [110.5, -59.0, 110.5], "brought back onto the cushion");
    let (ok, _) = p.s.perform(src, "tp @s 105.5 -59 106.5");
    assert!(ok);
    p.s.teleport = None;
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert_eq!(p.s.deco.seat, Some(id), "no new seat");
    p.s.set_game_mode(3);
    assert_eq!(p.s.deco.seat, None, "spectator still seated");
    assert_eq!(p.s.pos, [105.5, -58.75, 105.5], "placed on the cushion");
    done(p, dir);
}

/// Frame that arrived with a fresh chunk (End ship): registered, hence breakable.
#[test]
fn generated_frame_is_registered() {
    let Some((mut p, dir)) = probe("deco-gen") else { return };
    p.s.deco_adopt(&[(6, 6)]);
    let frame = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:item_frame".into())),
        ("Pos".into(), Nbt::List(vec![Nbt::Double(105.5), Nbt::Double(-59.0 + 0.03125), Nbt::Double(105.5)])),
        ("Facing".into(), Nbt::Byte(1)),
        ("block_pos".into(), Nbt::IntArray(vec![105, -59, 105])),
    ]);
    p.s.gen_arrived(vec![((6, 6), vec![frame])], Vec::new());
    let (id, _) = decos(&p, DKind::Frame)[0].clone();
    assert!(p.s.deco_attacked(id), "generated frame outside the registry");
    assert!(decos(&p, DKind::Frame).is_empty(), "generated frame not broken");
    done(p, dir);
}

/// Name tag on an armor stand: name set (data 2), tag consumed.
#[test]
fn name_tag_names_an_armor_stand() {
    let Some((mut p, dir)) = probe("deco-tag") else { return };
    hold(&mut p, "minecraft:armor_stand", 1);
    p.s.use_item_on_block([106, -60, 106], Dir::Up, [0.5, 1.0, 0.5]);
    let (id, _) = decos(&p, DKind::Stand)[0].clone();
    hold(&mut p, "minecraft:name_tag", 1);
    let name = Nbt::String("Welcome".into());
    crate::items::set_component(p.s.slots[36 + p.s.held as usize].as_mut().unwrap(), "minecraft:custom_name", name.clone());
    p.take_events();
    assert!(p.s.deco_interact(id, 0, [0.0, 1.0, 0.0], false));
    assert_eq!(decos(&p, DKind::Stand)[0].1.get("CustomName"), Some(&name), "name missing");
    assert!(meta_of(&p, id).contains(&(2, Meta::Text(Some(name)))), "name not sent");
    assert!(p.s.slots[36 + p.s.held as usize].is_none(), "tag not consumed");
    done(p, dir);
}

/// Frame behind a conductive block: the comparator reads rotation % 8 + 1 with an item, 0
/// without; each click changes the reading.
#[test]
fn comparator_reads_item_frame() {
    let Some((mut p, dir)) = probe("deco-comparator") else { return };
    p.run("setblock 106 -59 104 minecraft:stone");
    p.run("setblock 105 -59 104 minecraft:comparator[facing=east]");
    hold(&mut p, "minecraft:item_frame", 1);
    p.s.use_item_on_block([106, -59, 104], Dir::East, [1.0, 0.5, 0.5]);
    let (id, _) = decos(&p, DKind::Frame)[0].clone();
    let c = [105, -59, 104];
    let input = |p: &Probe| p.s.comparator_input(c, p.s.get(c));
    assert_eq!(input(&p), 0, "empty frame");
    hold(&mut p, "minecraft:diamond", 1);
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert_eq!(input(&p), 1, "item placed, rotation 0");
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert!(p.s.deco_interact(id, 0, [0.0; 3], false));
    assert_eq!(input(&p), 3, "rotation 2");
    done(p, dir);
}

/// Mannequin profile: name alone → partial profile (name present, no id or properties, empty
/// appearance); name and id → full profile; empty compound → default, nothing sent.
#[test]
fn mannequin_profile_bytes() {
    let n = super::profile_bytes(&Nbt::String("Notch".into())).unwrap();
    assert_eq!(n, [0, 1, 5, b'N', b'o', b't', b'c', b'h', 0, 0, 0, 0, 0, 0]);
    let full = Nbt::Compound(vec![
        ("name".into(), Nbt::String("A".into())),
        ("id".into(), Nbt::IntArray(vec![0, 0, 0, 7])),
        ("model".into(), Nbt::String("slim".into())),
    ]);
    let mut want = vec![1];
    want.extend_from_slice(&7u128.to_be_bytes());
    want.extend_from_slice(&[1, b'A', 0, 0, 0, 0, 1, 1]);
    assert_eq!(super::profile_bytes(&full).unwrap(), want);
    assert_eq!(super::profile_bytes(&Nbt::Compound(Vec::new())), None);
}

/// `/summon` of a frame: registered, hence breakable like a placed frame.
#[test]
fn summoned_frame_is_simulated() {
    let Some((mut p, dir)) = probe("deco-summon") else { return };
    let mut src = p.s.player_source();
    src.level = 4;
    let (ok, _) = p.s.perform(src, "summon minecraft:item_frame 105.5 -59.0 105.5 {Facing:1b,block_pos:[I;105,-59,105]}");
    assert!(ok, "summon refused");
    let (id, _) = decos(&p, DKind::Frame)[0].clone();
    assert!(p.s.deco_attacked(id));
    assert!(decos(&p, DKind::Frame).is_empty(), "summoned frame not broken");
    assert_eq!(dropped(&p, "minecraft:item_frame"), 1);
    done(p, dir);
}

/// Mannequin read from the world: hidden layers removed from the mask, description hidden, immobile.
#[test]
fn mannequin_meta_from_nbt() {
    let Some((p, dir)) = probe("deco-mannequin") else { return };
    let e = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:mannequin".into())),
        ("hidden_layers".into(), Nbt::List(vec![Nbt::String("cape".into()), Nbt::String("hat".into())])),
        ("immovable".into(), Nbt::Byte(1)),
        ("hide_description".into(), Nbt::Byte(1)),
        ("pose".into(), Nbt::String("crouching".into())),
    ]);
    let mut v = Vec::new();
    p.s.deco_meta(&e, &mut v);
    assert!(v.contains(&(16, Meta::Byte(0x7F & !0x41))));
    assert!(v.contains(&(18, Meta::Bool(true))));
    assert!(v.contains(&(19, Meta::Text(None))));
    assert!(v.contains(&(6, Meta::Pose(5))));
    done(p, dir);
}

/// Boxes: 2 × 2 painting shifted half a block left and up, frame with enlarged map.
#[test]
fn hanging_boxes_follow_the_game() {
    let b = super::painting_box([0, 0, 0], Dir::North, 2, 2);
    // North: left = west (−x); wall south of the block (z + 0.5 + 0.46875).
    assert_eq!(b.min, [-1.0, 0.0, 0.5 + 0.46875 - 0.03125]);
    assert_eq!(b.max, [1.0, 2.0, 0.5 + 0.46875 + 0.03125]);
    let f = super::frame_box([0, 0, 0], Dir::Up, true);
    assert_eq!(f.min, [0.0, 0.03125 - 0.03125, 0.0]);
    assert_eq!(f.max, [1.0, 0.0625, 1.0]);
}
