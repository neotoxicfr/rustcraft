//! G5-F03 (continued): pardon of dead players, wolf shaking, creeper (lighter, fall,
//! cloud), enderman (arrival box, carried block), conversion, synchronised attributes, buckets,
//! skeleton weapon, creative throw statistic, spawn point, spectator.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::SoundRef;

use super::goals::Goal;
use super::mob::{Ent, Mob};
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut g);
    m.pos = pos;
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Mob without spawn preparation (neither jockey nor baby drawn).
fn bare(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let m = p.s.new_mob(k, pos, 0.0);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn take(p: &mut Probe, id: i32) -> (usize, Box<Mob>) {
    let i = p.s.mob_index(id).expect("mob");
    (i, p.s.mobs.list[i].take().expect("mob"))
}

fn sounds(p: &Probe, events: &[Event]) -> Vec<String> {
    let reg = p.s.data.registry("minecraft:sound_event").expect("sound registry");
    events
        .iter()
        .filter_map(|e| match e {
            Event::Sound(s) => Some(match &s.sound {
                SoundRef::Id(i) => reg.entries.get(*i as usize).cloned().unwrap_or_default(),
                SoundRef::Named(n) => n.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn has_event(events: &[Event], id: i32, ev: i8) -> bool {
    events.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Event { id: x, event }) if *x == id && *event == ev))
}

/// Dead player: nearby neutral mobs angry at them drop it, not those
/// beyond 32 blocks.
#[test]
fn dead_players_forgiven() {
    let Some((mut p, dir)) = probe("g5f03-forgive") else { return };
    let me = p.s.uuid;
    let t = p.s.time;
    let near = spawn(&mut p, Kind::IronGolem, [216.5, -59.0, 210.5]);
    let far = spawn(&mut p, Kind::IronGolem, [216.5, -59.0, 250.5]);
    for id in [near, far] {
        let i = p.s.mob_index(id).unwrap();
        let m = p.s.mobs.list[i].as_mut().unwrap();
        m.anger_target = Some(Ent::Player);
        m.golem.angry_at = Some(me);
        m.target = Some(Ent::Player);
        m.anger_end = t + 500;
    }
    p.s.forgive_dead_player();
    let state = |p: &Probe, id: i32| {
        let m = p.s.mob_ref(id).unwrap();
        (m.anger_target, m.golem.angry_at, m.target, m.anger_end)
    };
    assert_eq!(state(&p, near), (None, None, None, -1), "anger kept near the dead player");
    assert_eq!(state(&p, far).0, Some(Ent::Player), "anger lost far away");
    cleanup(p, dir);
}

/// Wet wolf on the ground: shaking (event 8, sound), ends after 2; water interrupts it (56).
#[test]
fn wolf_shake() {
    let Some((mut p, dir)) = probe("g5f03-shake") else { return };
    let w = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 210.5]);
    let (i, mut m) = take(&mut p, w);
    m.x.wet = true;
    m.on_ground = true;
    let _ = p.take_events();
    p.s.wolf_shake_start(&mut m);
    assert!(m.x.shaking);
    p.s.wolf_shake_tick(&mut m);
    let ev = p.take_events();
    assert!(has_event(&ev, w, 8), "event 8 missing");
    assert!(sounds(&p, &ev).iter().any(|s| s == "minecraft:entity.wolf.shake"), "sound missing");
    for _ in 0..41 {
        p.s.wolf_shake_tick(&mut m);
    }
    assert!(!m.x.shaking && !m.x.wet, "shaking not finished");
    m.x.wet = true;
    p.s.wolf_shake_start(&mut m);
    m.in_water = true;
    let _ = p.take_events();
    p.s.wolf_shake_tick(&mut m);
    assert!(has_event(&p.take_events(), w, 56), "interruption missing");
    assert!(!m.x.shaking);
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Creeper: lit with flint and steel (worn) or with a fireball (consumed); the fall advances the fuse
/// to 5 ticks from the end; explosion under an effect: lingering cloud.
#[test]
fn creeper_lighter_fall_and_cloud() {
    let Some((mut p, dir)) = probe("g5f03-creeper") else { return };
    let c = spawn(&mut p, Kind::Creeper, [216.5, -59.0, 206.5]);
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(Stack { id: "minecraft:flint_and_steel".into(), count: 1, components: None });
    assert!(p.s.player_interact(c, 0), "flint and steel refused");
    assert!(p.s.mob_ref(c).unwrap().ignited);
    assert_eq!(p.s.slots[slot].as_ref().map(|s| p.s.life.items.damage(s)), Some(1));
    let c2 = spawn(&mut p, Kind::Creeper, [214.5, -59.0, 206.5]);
    p.s.slots[slot] = Some(Stack { id: "minecraft:fire_charge".into(), count: 2, components: None });
    assert!(p.s.player_interact(c2, 0));
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1));
    // Fall.
    let stone = p.s.get([216, -60, 216]);
    let (i, mut m) = take(&mut p, c2);
    m.ignited = false;
    m.swell = 0;
    p.s.fall_on(&mut m, stone, 10.0);
    assert_eq!(m.swell, 15);
    p.s.fall_on(&mut m, stone, 30.0);
    assert_eq!(m.swell, m.fuse - 5);
    p.s.mobs.list[i] = Some(m);
    // Cloud.
    let (i, mut m) = take(&mut p, c);
    p.s.give_mob_effect(&mut m, "minecraft:speed", 400, 0);
    m.swell = m.fuse - 1;
    let before = p.s.proj.clouds.len();
    assert!(p.s.creeper_tick(&mut m), "no explosion");
    p.s.mobs.list[i] = Some(m);
    let cloud = p.s.proj.clouds.get(before).expect("cloud missing");
    assert_eq!((cloud.radius, cloud.duration, cloud.wait_time), (2.5, 300, 10));
    cleanup(p, dir);
}

/// Enderman: arrival refused whose box touches a forbidden block (fire); carried block written
/// in the 26.3 state format.
#[test]
fn enderman_box_and_carried_block() {
    let Some((mut p, dir)) = probe("g5f03-ender") else { return };
    let e = spawn(&mut p, Kind::Enderman, [216.5, -59.0, 206.5]);
    p.run("setblock 210 -59 210 minecraft:fire\n");
    let fire = state_string(&p.s, p.s.get([210, -59, 210]));
    assert!(fire.starts_with("minecraft:fire"), "fire not placed: {fire}");
    assert!(p.s.btag([210, -59, 210], "minecraft:enderman_does_not_teleport_to"), "fire tag");
    let (i, mut m) = take(&mut p, e);
    assert!(!p.s.enderman_teleport(&mut m, [210.5, -59.0, 210.5]), "arrival in the fire");
    assert!(p.s.enderman_teleport(&mut m, [220.5, -59.0, 220.5]), "free arrival refused");
    let stone = p.s.get([216, -60, 216]);
    m.carried = Some(stone);
    let mut out = Nbt::Compound(vec![]);
    p.s.enderman_save_carried(&m, &mut out);
    assert_eq!(out.get("carriedBlockState"), Some(&Nbt::String("minecraft:stone".into())));
    let log = p.s.state_with("minecraft:oak_log", &[("axis".into(), "x".into())]).unwrap();
    m.carried = Some(log);
    p.s.enderman_save_carried(&m, &mut out);
    let c = out.get("carriedBlockState").unwrap();
    assert_eq!(c.get("id"), Some(&Nbt::String("minecraft:oak_log".into())));
    assert!(c.get("properties").is_some() && c.get("Name").is_none());
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Conversion: flags, invulnerability, tags and data kept, seat on the vehicle.
#[test]
fn conversion_copies_the_state() {
    let Some((mut p, dir)) = probe("g5f03-conv") else { return };
    let chicken = spawn(&mut p, Kind::Chicken, [216.5, -59.0, 206.5]);
    let z = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 206.5]);
    let (i, mut m) = take(&mut p, z);
    m.x.silent = true;
    m.x.name_visible = true;
    m.invulnerable = true;
    m.nbt = Some(Nbt::Compound(vec![("Tags".into(), Nbt::List(vec![Nbt::String("t".into())]))]));
    m.mount.vehicle = Some(chicken);
    p.s.mobs.get_mut(chicken).unwrap().mount.passengers.push(Ent::Mob(z));
    let n = p.s.converted(&mut m, Kind::Drowned, true, true);
    assert!(n.x.silent && n.x.name_visible && n.invulnerable);
    assert!(n.nbt.as_ref().and_then(|e| e.get("Tags")).is_some(), "tags lost");
    assert_eq!(n.mount.vehicle, Some(chicken));
    assert_eq!(p.s.mob_ref(chicken).unwrap().mount.passengers, vec![Ent::Mob(n.id)]);
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Synchronisable attributes: all sent when any mob is paired; afterwards, at the tracking
/// interval tick only, after the data, and only the changed attributes.
#[test]
fn synced_attributes() {
    let Some((mut p, dir)) = probe("g5f03-attr") else { return };
    let z = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 206.5]);
    let attrs = |ev: &[Event]| {
        ev.iter()
            .filter_map(|e| match e {
                Event::Attributes { entity, attributes } if *entity == z => Some(attributes.len()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let (i, mut m) = take(&mut p, z);
    p.s.view = 8;
    p.s.sent.insert((216 >> 4, 206 >> 4));
    let _ = p.take_events();
    p.s.send_changes(&mut m);
    assert!(m.track.seen, "not paired");
    let all = attrs(&p.take_events());
    assert!(all.len() == 1 && all[0] > 1, "attributes missing at pairing: {all:?}");
    // Tracking at tick 1 (outside the interval) then at tick 2 with a change: nothing yet.
    p.s.send_changes(&mut m);
    assert!(attrs(&p.take_events()).is_empty(), "attributes resent without a change");
    m.attrs.speed.base = 0.5;
    p.s.send_changes(&mut m);
    assert!(attrs(&p.take_events()).is_empty(), "attributes sent outside the interval");
    // Tick 3: interval, the speed alone.
    p.s.send_changes(&mut m);
    assert_eq!(attrs(&p.take_events()), vec![1], "only the changed attribute, once");
    p.s.send_changes(&mut m);
    assert!(attrs(&p.take_events()).is_empty(), "sent twice");
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Bucket: flags (silent, glowing, no gravity, invulnerable) written then read back.
#[test]
fn bucket_keeps_the_flags() {
    let mut m = Mob::new(Kind::Cod, 1, 1, [0.0; 3], 0.0, super::jrand::JRand::new(1));
    m.x.silent = true;
    m.x.glowing = true;
    m.surf.no_gravity = true;
    m.invulnerable = true;
    let data = super::interact::bucket_default_data(&m);
    for k in ["Silent", "Glowing", "NoGravity", "Invulnerable"] {
        assert_eq!(crate::nbtio::int(&data, k), Some(1), "{k}");
    }
    let mut n = Mob::new(Kind::Cod, 2, 2, [0.0; 3], 0.0, super::jrand::JRand::new(2));
    super::interact::load_bucket_default_data(&mut n, &data);
    assert!(n.x.silent && n.x.glowing && n.surf.no_gravity && n.invulnerable);
}

/// Skeleton with a bow in the off hand: it shoots (bow goal).
#[test]
fn skeleton_bow_in_off_hand() {
    let Some((mut p, dir)) = probe("g5f03-bow") else { return };
    let s = spawn(&mut p, Kind::Skeleton, [216.5, -59.0, 206.5]);
    let (i, mut m) = take(&mut p, s);
    m.equipment[0] = None;
    m.equipment[1] = Some(Stack { id: "minecraft:bow".into(), count: 1, components: None });
    super::brains::reassess_weapon(&p.s, &mut m);
    assert!(m.goals.goals.iter().any(|w| matches!(w.goal, Goal::Bow { .. })), "no bow");
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Creative throw outside the inventory: "dropped" and "items dropped" statistics.
#[test]
fn creative_throw_counts() {
    let Some((mut p, dir)) = probe("g5f03-throw") else { return };
    p.s.game_mode = 1;
    p.s.on_creative_slot(-1, Some(Stack { id: "minecraft:stone".into(), count: 3, components: None }));
    assert_eq!(p.s.stat_value("minecraft:dropped", "minecraft:stone"), 3);
    assert_eq!(p.s.stat_value("minecraft:custom", "minecraft:drop"), 1);
    cleanup(p, dir);
}

/// Spawn point: on a block with a full face within the radius; world in adventure: height only
/// corrected, at the centre of the point.
#[test]
fn spawn_point() {
    let Some((mut p, dir)) = probe("g5f03-spawn") else { return };
    p.s.level.spawn_pos = [216, -50, 216];
    let (pos, _, _) = p.s.world_spawn_point();
    assert!((pos[0] - 216.5).abs() <= 10.0 && (pos[2] - 216.5).abs() <= 10.0);
    // Above the column (terrain of the copy above the cleared zone): full face
    // below, free place.
    let b = [pos[0].floor() as i32, pos[1] as i32 - 1, pos[2].floor() as i32];
    assert!(p.s.face_full_up(p.s.get(b)), "not on a full face: {pos:?}");
    assert_eq!(pos[1] as i32, p.s.surface_y(b[0], b[2], 4), "not at the top of the column");
    p.s.level.game_type = 2;
    let (pos, _, _) = p.s.world_spawn_point();
    assert_eq!(pos, [216.5, -59.0, 216.5]);
    // Column full up to the ceiling: the climb stops at the highest block of the world (319).
    p.run("fill 216 -59 216 216 319 216 minecraft:stone\n");
    let (pos, _, _) = p.s.world_spawn_point();
    assert_eq!(pos, [216.5, 319.0, 216.5], "world ceiling");
    cleanup(p, dir);
}

/// Spectator: chest still sealed refused with a red message.
#[test]
fn spectator_sealed_chest() {
    let Some((mut p, dir)) = probe("g5f03-spectator") else { return };
    p.run("setblock 210 -59 210 minecraft:chest\n");
    let be = p.s.block_entity_mut([210, -59, 210], "minecraft:chest").expect("chest");
    crate::nbtio::set(be, "LootTable", Nbt::String("minecraft:chests/simple_dungeon".into()));
    p.s.game_mode = 3;
    let _ = p.take_events();
    assert!(p.s.loot_before_open(&[[210, -59, 210]]));
    let msg = p.take_events().into_iter().any(|e| match e {
        Event::ActionBar(c) => format!("{c:?}").contains("container.spectatorCantOpen"),
        _ => false,
    });
    assert!(msg, "message missing");
    cleanup(p, dir);
}

/// Fire of a mob: a negative fire read back returns to 0 after a tick (only the player keeps −20);
/// lightning on a mob at 0 lights it for only one tick (not 8 s).
#[test]
fn fire_reset_and_lightning() {
    let Some((mut p, dir)) = probe("g5f03-fire") else { return };
    let cow = spawn(&mut p, Kind::Cow, [216.5, -59.0, 208.5]);
    let (i, mut m) = take(&mut p, cow);
    m.fire = -5;
    p.s.tick_mob(&mut m);
    assert_eq!(m.fire, 0, "negative fire kept");
    p.s.mob_thunder_hit_default(&mut m);
    assert_eq!(m.fire, 1, "lightning: a single tick of fire");
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Synchronised entity flags: name always shown (3), silent (4), no gravity (5),
/// glowing through the flag (bit 6); ocelot: sprint and invisibility in the same byte.
#[test]
fn synced_flags() {
    use rc_proto_v777::entity::Meta;
    let mut m = Mob::new(Kind::Cow, 1, 1, [0.0; 3], 0.0, super::jrand::JRand::new(1));
    m.define_data();
    m.x.name_visible = true;
    m.x.silent = true;
    m.x.glowing = true;
    m.surf.no_gravity = true;
    m.sync_data();
    assert_eq!(m.get(3), Some(&Meta::Bool(true)), "name always shown");
    assert_eq!(m.get(4), Some(&Meta::Bool(true)), "silent");
    assert_eq!(m.get(5), Some(&Meta::Bool(true)), "no gravity");
    assert_eq!(m.get(0), Some(&Meta::Byte(0x40)), "glowing");
    let mut o = Mob::new(Kind::Ocelot, 2, 1, [0.0; 3], 0.0, super::jrand::JRand::new(1));
    o.define_data();
    o.surf.sprinting = true;
    o.x.glowing = true;
    o.sync_data();
    assert_eq!(o.get(0), Some(&Meta::Byte(0x48)), "ocelot: sprint and glowing");
}

/// Silent mob: neither sound nor seed drawn on the world random, but the pitch random of its voice
/// is drawn; sounds played by the world (crossbow) are still heard.
#[test]
fn silent_mob() {
    let Some((mut p, dir)) = probe("g5f03-silent") else { return };
    let cow = spawn(&mut p, Kind::Cow, [216.5, -59.0, 208.5]);
    let (i, mut m) = take(&mut p, cow);
    m.x.silent = true;
    let _ = p.take_events();
    let (mut world, mut own) = (p.s.mobs.rng, m.rng);
    p.s.make_sound(&mut m, "minecraft:entity.cow.ambient");
    assert!(sounds(&p, &p.take_events()).is_empty(), "sound of a silent mob");
    assert_eq!(world.next_long(), p.s.mobs.rng.next_long(), "seed drawn");
    let _ = (own.next_float(), own.next_float());
    assert_eq!(own.next_long(), m.rng.next_long(), "pitch random not drawn");
    p.s.mob_sound_unmuted(&m, "minecraft:item.crossbow.shoot", 1.0, 1.0);
    assert_eq!(sounds(&p, &p.take_events()), vec!["minecraft:item.crossbow.shoot".to_owned()]);
    m.x.silent = false;
    p.s.make_sound(&mut m, "minecraft:entity.cow.ambient");
    assert_eq!(sounds(&p, &p.take_events()), vec!["minecraft:entity.cow.ambient".to_owned()]);
    p.s.mobs.list[i] = Some(m);
    cleanup(p, dir);
}

/// Creative: the saddle placed on a strider and the name tag do not leave the hand (the game
/// works on a copy of the stack); in survival, one unit goes.
#[test]
fn saddle_and_name_tag_in_creative() {
    let Some((mut p, dir)) = probe("g5f03-creative") else { return };
    p.s.game_mode = 1;
    let strider = bare(&mut p, Kind::Strider, [216.5, -59.0, 206.5]);
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(Stack { id: "minecraft:saddle".into(), count: 2, components: None });
    assert!(p.s.player_interact(strider, 0), "saddle refused");
    let i = p.s.mob_index(strider).unwrap();
    assert!(p.s.mobs.list[i].as_ref().unwrap().equipment[super::mounts::SADDLE].is_some(), "saddle not placed");
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(2), "saddle removed in creative");
    let cow = spawn(&mut p, Kind::Cow, [212.5, -59.0, 206.5]);
    let mut tag = Stack { id: "minecraft:name_tag".into(), count: 2, components: None };
    crate::items::set_component(&mut tag, "minecraft:custom_name", Nbt::String("Rex".into()));
    p.s.slots[slot] = Some(tag);
    assert!(p.s.player_interact(cow, 0), "name tag refused");
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(2), "name tag removed in creative");
    p.s.game_mode = 0;
    let s2 = bare(&mut p, Kind::Strider, [220.5, -59.0, 206.5]);
    p.s.slots[slot] = Some(Stack { id: "minecraft:saddle".into(), count: 2, components: None });
    assert!(p.s.player_interact(s2, 0), "saddle refused in survival");
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1), "saddle kept in survival");
    cleanup(p, dir);
}

/// Fed pig: eating sound of its sound variant.
#[test]
fn pig_meal() {
    let Some((mut p, dir)) = probe("g5f03-pig") else { return };
    let pig = spawn(&mut p, Kind::Pig, [216.5, -59.0, 206.5]);
    let i = p.s.mob_index(pig).unwrap();
    p.s.mobs.list[i].as_mut().unwrap().age = 0;
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(Stack { id: "minecraft:carrot".into(), count: 3, components: None });
    let _ = p.take_events();
    assert!(p.s.player_interact(pig, 0), "carrot refused");
    let s = sounds(&p, &p.take_events());
    assert!(s.iter().any(|n| n.contains("pig") && n.ends_with(".eat")), "eating sound missing: {s:?}");
    cleanup(p, dir);
}
