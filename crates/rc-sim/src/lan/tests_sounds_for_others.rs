//! Sounds the acting player's own client plays (landing on honey or powder snow, axe, wax,
//! flint and steel, eating): the server sends them to the other LAN players only.

use rc_client::Event;

use super::tests_g3f05::{
    G, T, drain, feet, guest_packets, guest_ready, hold_one, host_ready, host_sound_ids, join, open, publish,
    server_tick, sound_id, sounds_in,
};

const FLOOR: i32 = 199;

/// Host 5 blocks from a guest who acts, block `block` set at each cell given, nothing pending.
fn host_beside_acting_guest(tag: &str, blocks: &[([i32; 3], &str)]) -> (T, G) {
    let mut t = open(tag);
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    host_ready(&mut t, feet([0, FLOOR + 1, -2]));
    guest_ready(&mut t, feet([0, FLOOR + 1, 3]));
    for &(p, block) in blocks {
        let s = t.s.default_state(block);
        t.s.set_block(p, s, 2);
    }
    let _ = guest_packets(&g);
    drain(&t);
    (t, g)
}

/// The guest's own client plays sound `name`: the host hears it once, the guest gets nothing.
fn heard_by_the_host_only(t: &T, g: &G, name: &str) {
    assert!(sounds_in(t, &guest_packets(g)).is_empty(), "{name}: sent back to the guest");
    assert_eq!(host_sound_ids(t), [sound_id(t, name)], "{name}: host near the guest");
}

/// The guest lands on `block` after 5 blocks: its landing sound is the guest's own client's, so
/// the host only hears `heard`. Before, nobody else heard it.
fn landing_sound_reaches_the_host_only(tag: &str, block: &str, heard: &str) {
    let (mut t, g) = host_beside_acting_guest(tag, &[([0, FLOOR, 3], block)]);
    t.s.as_guest(0, |sv| {
        sv.life.vit.fall = 5.0;
        sv.on_ground = true;
        let at = feet([0, FLOOR + 1, 3]);
        sv.pos = at;
        sv.after_move([at[0], at[1] + 0.1, at[2]], false);
    });
    heard_by_the_host_only(&t, &g, heard);
}

#[test]
fn honey_landing_sound_reaches_the_other_players() {
    landing_sound_reaches_the_host_only("land-honey", "minecraft:honey_block", "minecraft:block.honey_block.slide");
}

#[test]
fn powder_snow_fall_sound_reaches_the_other_players() {
    landing_sound_reaches_the_host_only("land-snow", "minecraft:powder_snow", "minecraft:entity.player.small_fall");
}

/// An axe stripping a log: the acting player's own client plays the sound, the others get it
/// from the server.
#[test]
fn axe_sound_reaches_the_other_players() {
    let log = [1, FLOOR + 1, 3];
    let (mut t, g) = host_beside_acting_guest("axesound", &[(log, "minecraft:oak_log")]);
    t.s.as_guest(0, |sv| {
        let slot = hold_one(sv, "minecraft:iron_axe");
        assert!(sv.item_use_on(slot, log, crate::blocks::Dir::North), "log stripped");
    });
    heard_by_the_host_only(&t, &g, "minecraft:item.axe.strip");
}

/// Honeycomb waxing copper: sound and particles (world event 3003) to the others only.
#[test]
fn wax_sound_and_particles_reach_the_other_players() {
    let copper = [-1, FLOOR + 1, 3];
    let (mut t, g) = host_beside_acting_guest("waxsound", &[(copper, "minecraft:copper_block")]);
    t.s.as_guest(0, |sv| {
        let slot = hold_one(sv, "minecraft:honeycomb");
        assert!(sv.item_use_on(slot, copper, crate::blocks::Dir::North), "copper waxed");
    });
    assert!(sounds_in(&t, &guest_packets(&g)).is_empty(), "wax sound sent back to the guest");
    let (wax, evs) = (sound_id(&t, "minecraft:item.honeycomb.wax_on"), drain(&t));
    let is_wax = |e: &&Event| matches!(e, Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == wax));
    let wax_particles = evs.iter().filter(|e| matches!(e, Event::LevelEvent { kind: 3003, .. })).count();
    assert_eq!((evs.iter().filter(is_wax).count(), wax_particles), (1, 1), "host near the waxed copper");
}

/// Flint and steel lighting a fire on the floor: the acting player's own client plays its sound.
#[test]
fn flint_and_steel_sound_reaches_the_other_players() {
    let (mut t, g) = host_beside_acting_guest("flintsound", &[]);
    t.s.as_guest(0, |sv| {
        let slot = hold_one(sv, "minecraft:flint_and_steel");
        assert!(sv.use_igniter(slot, "minecraft:flint_and_steel", [0, FLOOR, 5], crate::blocks::Dir::Up));
    });
    heard_by_the_host_only(&t, &g, "minecraft:item.flintandsteel.use");
}

/// Bread eaten by the guest: 6 chewing sounds (every 4 ticks once 21.875 % of the 32 ticks have
/// passed) and one at the end reach the host only; the food's own sound goes to everyone.
#[test]
fn eating_sounds_reach_the_other_players() {
    let (mut t, g) = host_beside_acting_guest("eatsounds", &[]);
    t.s.as_guest(0, |sv| {
        sv.life.vit.food = 10;
        hold_one(sv, "minecraft:bread");
        sv.on_use_item(0);
    });
    let (mut host, mut guest) = (Vec::new(), Vec::new());
    for _ in 0..40 {
        server_tick(&mut t);
        host.extend(host_sound_ids(&t));
        guest.extend(sounds_in(&t, &guest_packets(&g)));
    }
    let eat = sound_id(&t, "minecraft:entity.generic.eat");
    let count = |v: &[i32]| v.iter().filter(|&&s| s == eat).count();
    assert_eq!(count(&host), 8, "host: 6 while chewing, 1 at the end, the food's own");
    assert_eq!(count(&guest), 1, "guest: only the food's own sound");
}

/// A stick given a drinkable consumable of 0.8 s and a food component by its stack's patch is
/// consumed like the game reads that stack: 16 ticks, the patched sound every 4 ticks once
/// 21.875 % have passed (3 times), once at the end, then the food's own to everyone. Before,
/// the server read the item's default components: a stick could not be consumed at all.
#[test]
fn a_stack_patched_consumable_is_the_one_consumed_and_heard() {
    use rc_proto::nbt::Nbt;
    let (mut t, g) = host_beside_acting_guest("patchedsounds", &[]);
    t.s.as_guest(0, |sv| {
        sv.life.vit.food = 10;
        let slot = hold_one(sv, "minecraft:stick");
        let consumable = Nbt::Compound(vec![
            ("consume_seconds".into(), Nbt::Float(0.8)),
            ("animation".into(), Nbt::String("drink".into())),
            ("sound".into(), Nbt::String("minecraft:entity.generic.drink".into())),
        ]);
        let food = Nbt::Compound(vec![("nutrition".into(), Nbt::Int(1)), ("saturation".into(), Nbt::Float(0.1))]);
        let patch = vec![("minecraft:consumable".into(), consumable), ("minecraft:food".into(), food)];
        if let Some(s) = sv.slots[slot].as_mut() {
            s.components = Some(Nbt::Compound(patch));
        }
        sv.on_use_item(0);
    });
    let (mut host, mut guest) = (Vec::new(), Vec::new());
    for _ in 0..24 {
        server_tick(&mut t);
        host.extend(host_sound_ids(&t));
        guest.extend(sounds_in(&t, &guest_packets(&g)));
    }
    let drink = sound_id(&t, "minecraft:entity.generic.drink");
    let count = |v: &[i32]| v.iter().filter(|&&s| s == drink).count();
    assert_eq!(count(&host), 5, "host: 3 while drinking, 1 at the end, the food's own");
    assert_eq!(count(&guest), 1, "guest: only the food's own sound");
    t.s.as_guest(0, |sv| {
        let slot = 36 + sv.held as usize;
        assert!(sv.slots[slot].is_none(), "the stick was consumed");
        assert_eq!(sv.life.vit.food, 11, "the patched food was eaten");
    });
}
