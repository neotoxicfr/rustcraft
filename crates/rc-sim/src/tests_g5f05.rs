//! G5-F05: experience orbs (merging, Mending), ground items and orbs burnt by the blocks they
//! cross, display entities' data, interaction entity, marker, shears wear; on a copy of the test
//! world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests skipped.
#![cfg(test)]

use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::player::Stack;
use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [110.5, -59.0, 110.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    p.s.life.weather.raining = false; // rain would put the fire out
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    p.take_events();
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn cmd(p: &mut Probe, c: &str) -> bool {
    let mut src = p.s.player_source();
    src.level = 4;
    p.s.perform(src, c).0
}

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: id.into(), count, components: None }
}

fn damaged(id: &str, damage: i32, ench: &[(&str, i32)]) -> Stack {
    let mut s = stack(id, 1);
    crate::items::set_component(&mut s, "minecraft:damage", Nbt::Int(damage));
    if !ench.is_empty() {
        let e = ench.iter().map(|(k, l)| ((*k).to_owned(), Nbt::Int(*l))).collect();
        crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(e));
    }
    s
}

fn wear(s: Option<&Stack>) -> i64 {
    s.and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(Nbt::as_i64).unwrap_or(0)
}

/// Data sent for entity `id` since the last read.
fn data_of(p: &Probe, id: i32) -> Vec<(u8, Meta)> {
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if i == id => Some(values),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Identifier of the last frozen entity summoned.
fn last_other(p: &Probe) -> i32 {
    p.s.next_entity
}

fn other(p: &Probe, id: i32) -> &Nbt {
    p.s.entities.values().flatten().find(|e| e.0 == id).map(|e| &e.1).expect("entity kept")
}

/// Marker: summoned and killed without a single packet for the client.
#[test]
fn marker_is_never_sent() {
    let Some((mut p, dir)) = probe("g5f05-marker") else { return };
    assert!(cmd(&mut p, "summon minecraft:marker 110 -59 112 {Tags:[\"m\"]}"));
    let id = last_other(&p);
    let ev = p.take_events();
    let sent = ev.iter().any(|e| match e {
        Event::Entity(EntityEvent::Add(a)) => a.id == id,
        Event::Entity(EntityEvent::Data { id: i, .. }) => *i == id,
        _ => false,
    });
    assert!(!sent, "marker announced: {ev:?}");
    assert!(cmd(&mut p, "kill @e[tag=m]"));
    let removed = p.take_events().iter().any(|e| matches!(e, Event::Entity(EntityEvent::Remove(l)) if l.contains(&id)));
    assert!(!removed, "removal of a marker never announced");
    done(p, dir);
}

/// Shears: worn by a zero-hardness block (short grass), never by fire; a pickaxe on the same
/// grass is not.
#[test]
fn shears_wear_on_zero_hardness_but_not_fire() {
    let Some((mut p, dir)) = probe("g5f05-shears") else { return };
    let held = 36 + p.s.held as usize;
    let grass = p.s.data.blocks.iter().position(|b| b.name == "minecraft:short_grass").expect("grass");
    let grass = p.s.data.blocks[grass].default_state;
    let fire = p.s.data.blocks.iter().position(|b| b.name == "minecraft:fire").expect("fire");
    let fire = p.s.data.blocks[fire].default_state;
    p.s.slots[held] = Some(stack("minecraft:shears", 1));
    p.s.on_block_broken([110, -59, 112], grass);
    assert_eq!(wear(p.s.slots[held].as_ref()), 1, "shears on short grass");
    p.s.on_block_broken([110, -59, 112], fire);
    assert_eq!(wear(p.s.slots[held].as_ref()), 1, "shears on fire");
    p.s.slots[held] = Some(stack("minecraft:iron_pickaxe", 1));
    p.s.on_block_broken([110, -59, 112], grass);
    assert_eq!(wear(p.s.slots[held].as_ref()), 0, "pickaxe on short grass");
    done(p, dir);
}

/// Two orbs of the same value with identifiers 40 apart, 0.9 apart: merged at their first tick
/// (count 2, youngest age); a third 1.2 away, or of another value, stays.
#[test]
fn orbs_merge_every_20_ticks() {
    let Some((mut p, dir)) = probe("g5f05-orbmerge") else { return };
    p.s.pos = [120.5, -59.0, 120.5];
    p.s.game_mode = 3; // nobody to follow or pick up
    p.s.add_orb([105.5, -59.0, 105.5], 3);
    p.s.add_orb([106.4, -59.0, 105.5], 3);
    p.s.add_orb([106.7, -59.0, 105.5], 3);
    p.s.add_orb([105.5, -59.0, 106.0], 7);
    let base = p.s.life.orbs[0].id;
    for (k, o) in p.s.life.orbs.iter_mut().enumerate() {
        o.id = base + 40 * k as i32;
        o.vel = [0.0; 3];
        o.age = 100 - k as i32;
    }
    p.s.life.orbs[2].pos[0] = 106.7 + 0.5; // 1.7 from the first: outside the inflated box
    p.s.drops_tick();
    let left: Vec<(i32, i32, i32)> = p.s.life.orbs.iter().map(|o| (o.value, o.count, o.age)).collect();
    assert_eq!(left.len(), 3, "{left:?}");
    assert_eq!((left[0].0, left[0].1), (3, 2), "{left:?}");
    assert_eq!(left[0].2, 100, "youngest age kept then aged one tick: {left:?}");
    done(p, dir);
}

/// Reward split near an existing orb 0.6 away: merged into it (box of 1 against the orb's box).
#[test]
fn reward_merges_into_orb_by_boxes() {
    let Some((mut p, dir)) = probe("g5f05-orbspawn") else { return };
    p.s.add_orb([105.5, -59.0, 105.5], 3);
    let mut probe_rng = p.s.life.rng;
    let r = probe_rng.next_int_bound(40);
    p.s.life.orbs[0].id = r + 4000;
    p.s.spawn_orbs([106.1, -59.0, 105.5], 3);
    assert_eq!(p.s.life.orbs.len(), 1, "a second orb was created");
    assert_eq!(p.s.life.orbs[0].count, 2);
    done(p, dir);
}

/// Mending: a damaged pickaxe in hand takes the orb (3 points → 6 of durability), the player
/// gets nothing; with 2 points of wear left, the remainder (2 points) goes to the player.
#[test]
fn mending_repairs_before_the_player_gets_xp() {
    let Some((mut p, dir)) = probe("g5f05-mending") else { return };
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = Some(damaged("minecraft:iron_pickaxe", 10, &[("minecraft:mending", 1)]));
    let before = p.s.life.vit.xp_total;
    assert_eq!(p.s.repair_with_xp(3), 0);
    assert_eq!(wear(p.s.slots[held].as_ref()), 4);
    assert_eq!(p.s.life.vit.xp_total, before);
    p.s.slots[held] = Some(damaged("minecraft:iron_pickaxe", 2, &[("minecraft:mending", 1)]));
    assert_eq!(p.s.repair_with_xp(3), 2);
    let s = p.s.slots[held].as_ref().expect("pickaxe");
    assert!(crate::items::component(s, "minecraft:damage").is_none(), "repaired: damage component dropped");
    // Without Mending (or undamaged): everything to the player.
    p.s.slots[held] = Some(damaged("minecraft:iron_pickaxe", 10, &[("minecraft:unbreaking", 1)]));
    assert_eq!(p.s.repair_with_xp(3), 3);
    // Through the pickup: an orb at the feet repairs the boots.
    p.s.slots[held] = None;
    p.s.slots[8] = Some(damaged("minecraft:iron_boots", 20, &[("minecraft:mending", 1)]));
    p.s.life.vit.xp_delay = 0;
    p.s.add_orb(p.s.pos, 7);
    p.s.life.orbs[0].vel = [0.0; 3];
    for _ in 0..3 {
        p.s.drops_tick();
    }
    assert!(p.s.life.orbs.is_empty(), "orb picked up");
    assert_eq!(wear(p.s.slots[8].as_ref()), 6);
    assert_eq!(p.s.life.vit.xp_total, before);
    done(p, dir);
}

/// Items burnt by the blocks they cross: on a cactus (1 a tick), in lava (destroyed in two hits),
/// in fire; a netherite ingot resists lava; an orb in lava is destroyed too.
#[test]
fn items_and_orbs_burn_and_prick() {
    let Some((mut p, dir)) = probe("g5f05-burn") else { return };
    p.s.pos = [125.5, -59.0, 125.5];
    p.s.game_mode = 3;
    p.run("setblock 104 -59 104 minecraft:cactus\nsetblock 108 -59 104 minecraft:lava\nsetblock 112 -59 104 minecraft:fire\n");
    p.s.spawn_item([104.5, -58.0, 104.5], [0.0; 3], stack("minecraft:dirt", 1), 32767);
    p.s.spawn_item([108.5, -58.9, 104.5], [0.0; 3], stack("minecraft:dirt", 1), 32767);
    p.s.spawn_item([108.5, -58.9, 104.6], [0.0; 3], stack("minecraft:netherite_ingot", 1), 32767);
    p.s.spawn_item([112.5, -58.9, 104.5], [0.0; 3], stack("minecraft:dirt", 1), 32767);
    p.s.add_orb([108.5, -58.9, 104.4], 3);
    let ids: Vec<i32> = p.s.life.drops.iter().map(|d| d.id).collect();
    let orb = p.s.life.orbs[0].id;
    let alive = |p: &Probe, id: i32| p.s.life.drops.iter().any(|d| d.id == id);
    let mut gone_at = [0; 4];
    for t in 1..=60 {
        p.s.drops_tick();
        for (k, &id) in ids.iter().enumerate() {
            if gone_at[k] == 0 && !alive(&p, id) {
                gone_at[k] = t;
            }
        }
    }
    assert!(gone_at[0] > 0 && gone_at[0] <= 20, "cactus: {gone_at:?}");
    assert!(gone_at[1] > 0 && gone_at[1] <= 5, "lava: {gone_at:?}");
    assert_eq!(gone_at[2], 0, "netherite in lava: {gone_at:?}");
    assert!(gone_at[3] > 0, "fire: {gone_at:?}");
    assert!(!p.s.life.orbs.iter().any(|o| o.id == orb), "orb in lava");
    done(p, dir);
}

/// Burning item: the "on fire" flag is sent, then the fire burns it 1 point a second.
#[test]
fn burning_item_sends_flag_and_burns() {
    let Some((mut p, dir)) = probe("g5f05-onfire") else { return };
    p.s.game_mode = 3;
    p.s.spawn_item([104.5, -59.0, 104.5], [0.0; 3], stack("minecraft:dirt", 1), 32767);
    let d = p.s.life.drops.last_mut().expect("item");
    let id = d.id;
    d.fire = 41;
    p.take_events();
    p.s.drops_tick();
    assert!(data_of(&p, id).contains(&(0, Meta::Byte(1))), "flag on");
    for _ in 0..30 {
        p.s.drops_tick();
    }
    let d = p.s.life.drops.iter().find(|d| d.id == id).expect("still there");
    assert_eq!(d.health, 3, "two seconds of fire");
    for _ in 0..20 {
        p.s.drops_tick();
    }
    assert!(data_of(&p, id).contains(&(0, Meta::Byte(0))), "flag off");
    done(p, dir);
}

/// Block display: matrix transformation decomposed and sent (11 to 14), block state (14,
/// mandatory serializer), billboard, brightness; `/data` resends the interpolation start and only
/// what changed.
#[test]
fn block_display_data_sent() {
    let Some((mut p, dir)) = probe("g5f05-display") else { return };
    assert!(cmd(
        &mut p,
        "summon minecraft:block_display 110 -59 112 {block_state:{Name:\"minecraft:stone\"},billboard:\"center\",\
         brightness:{block:15,sky:4},transformation:[2f,0f,0f,1f,0f,3f,0f,2f,0f,0f,4f,3f,0f,0f,0f,1f]}"
    ));
    let id = last_other(&p);
    let v = data_of(&p, id);
    assert!(v.contains(&(11, Meta::Vector3([1.0, 2.0, 3.0]))), "{v:?}");
    assert!(v.contains(&(12, Meta::Vector3([2.0, 3.0, 4.0]))), "{v:?}");
    // Left rotation exactly the identity: default, not sent. Right rotation: conjugate of the
    // identity (−0, −0, −0, 1), different to the bit: sent, like the game.
    assert!(!v.iter().any(|(i, _)| *i == 13), "{v:?}");
    let right = v.iter().find_map(|(i, m)| match m {
        Meta::Quaternion(q) if *i == 14 => Some(q.map(f32::to_bits)),
        _ => None,
    });
    assert_eq!(right, Some([0x8000_0000, 0x8000_0000, 0x8000_0000, 0x3f80_0000]), "{v:?}");
    assert!(v.contains(&(15, Meta::Byte(3))), "{v:?}");
    assert!(v.contains(&(16, Meta::Int(15 << 4 | 4 << 20))), "{v:?}");
    let stone = p.s.data.blocks.iter().find(|b| b.name == "minecraft:stone").expect("stone").default_state;
    assert!(v.contains(&(23, Meta::State(stone as i32))), "{v:?}");
    assert!(!v.iter().any(|(i, _)| *i == 8), "start delay at its default: {v:?}");
    assert!(cmd(&mut p, "data merge entity @e[type=minecraft:block_display,limit=1] {interpolation_duration:5}"));
    let v = data_of(&p, id);
    assert!(v.contains(&(8, Meta::Int(0))), "start always resent: {v:?}");
    assert!(v.contains(&(9, Meta::Int(5))), "{v:?}");
    assert!(!v.iter().any(|(i, _)| *i == 11 || *i == 23), "unchanged data resent: {v:?}");
    // Back to the default billboard: sent although it is the default.
    assert!(cmd(&mut p, "data merge entity @e[type=minecraft:block_display,limit=1] {billboard:\"fixed\"}"));
    assert!(data_of(&p, id).contains(&(15, Meta::Byte(0))));
    done(p, dir);
}

/// Text display: text resolved at summon (score of the player), style flags, line width;
/// item display: stack and context.
#[test]
fn text_and_item_display_data_sent() {
    let Some((mut p, dir)) = probe("g5f05-text") else { return };
    assert!(cmd(&mut p, "scoreboard objectives add g5 dummy"));
    assert!(cmd(&mut p, "scoreboard players set S3 g5 42"));
    assert!(cmd(
        &mut p,
        "summon minecraft:text_display 110 -59 112 {text:{score:{name:\"S3\",objective:\"g5\"}},shadow:1b,\
         alignment:\"left\",line_width:80,text_opacity:200}"
    ));
    let id = last_other(&p);
    let v = data_of(&p, id);
    let text = v.iter().find(|(i, _)| *i == 23).map(|(_, m)| m.clone());
    let Some(Meta::Component(t)) = text else { panic!("text not sent as a mandatory component: {v:?}") };
    let plain = rc_text::from_nbt(&t).map(|c| rc_text::plain_string(&c, &rc_text::Lang::default()));
    assert_eq!(plain.as_deref(), Some("42"), "{t:?}");
    assert!(v.contains(&(24, Meta::Int(80))), "{v:?}");
    assert!(v.contains(&(26, Meta::Byte(200u8 as i8))), "{v:?}");
    assert!(v.contains(&(27, Meta::Byte(1 | 8))), "{v:?}");
    assert!(other(&p, id).get("text").is_some_and(|t| !format!("{t:?}").contains("score")), "resolved text kept");
    assert!(cmd(&mut p, "summon minecraft:item_display 110 -59 113 {item:{id:\"minecraft:diamond\"},item_display:\"head\"}"));
    let id = last_other(&p);
    let v = data_of(&p, id);
    assert!(v.iter().any(|(i, m)| *i == 23 && matches!(m, Meta::Item(Some(_)))), "{v:?}");
    assert!(v.contains(&(24, Meta::Byte(5))), "{v:?}");
    done(p, dir);
}

/// Interaction: size and response sent; a hit records the player (`execute on attacker`), a use
/// records it too (`execute on target`).
#[test]
fn interaction_records_attack_and_use() {
    let Some((mut p, dir)) = probe("g5f05-interaction") else { return };
    assert!(cmd(&mut p, "summon minecraft:interaction 110.5 -59 111.5 {width:2f,height:3f,response:1b,Tags:[\"i\"]}"));
    let id = last_other(&p);
    let v = data_of(&p, id);
    assert!(v.contains(&(8, Meta::Float(2.0))) && v.contains(&(9, Meta::Float(3.0))), "{v:?}");
    assert!(v.contains(&(10, Meta::Bool(true))), "{v:?}");
    assert_eq!(p.s.relation_attacker(E::Other(id)), None);
    let ids = *p.s.peer.ids();
    let mut pk = Vec::new();
    rc_proto::varint::write_varint(&mut pk, ids.attack);
    rc_proto::varint::write_varint(&mut pk, id);
    assert!(p.s.mobs_packet(&pk));
    let attack = other(&p, id).get("attack").cloned().expect("attack recorded");
    assert_eq!(crate::nbtio::uuid(attack.get("player")), Some(p.s.uuid));
    assert_eq!(attack.get("timestamp").and_then(Nbt::as_i64), Some(p.s.time));
    assert_eq!(p.s.relation_attacker(E::Other(id)), Some(E::Player));
    assert_eq!(p.s.relation_target(E::Other(id)), None);
    let mut pk = Vec::new();
    rc_proto::varint::write_varint(&mut pk, ids.interact);
    rc_proto::varint::write_varint(&mut pk, id);
    rc_proto::varint::write_varint(&mut pk, 0);
    assert!(p.s.mobs_packet(&pk));
    assert_eq!(p.s.relation_target(E::Other(id)), Some(E::Player));
    // Out of reach: nothing recorded.
    assert!(cmd(&mut p, "summon minecraft:interaction 130.5 -59 130.5"));
    let far = last_other(&p);
    let mut pk = Vec::new();
    rc_proto::varint::write_varint(&mut pk, ids.attack);
    rc_proto::varint::write_varint(&mut pk, far);
    assert!(p.s.mobs_packet(&pk));
    assert!(other(&p, far).get("attack").is_none());
    done(p, dir);
}
