//! What a world does for all its players reaches the guests too: the cat and wandering trader
//! spawners draw any living player, the dragon fight and the raids register every player of the
//! world in their bars, the ravager's roar strikes guests and living decorations, every player
//! near a summoned boss or golem gets the advancement, and a projectile's player owner is known
//! wherever that player stands.

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::tests_g3f05::{
    FLOOR, G, T, drain, feet, guest_packets, guest_ready, host_ready, id_of, ids, join, open, publish, server_tick,
    sound_id, sounds_in,
};
use crate::mobs::end::fight::{Fight, Stage};
use crate::mobs::jrand::JRand;
use crate::mobs::species::Kind;
use crate::projectiles::physics::BlockHit;
use crate::projectiles::{PKind, Proj};

fn guest(t: &T) -> &super::ctx::PlayerCtx {
    t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap()
}

/// Boss bar packets among the guest's packets.
fn boss_packets(t: &T, packets: &[Vec<u8>]) -> usize {
    packets.iter().filter(|p| id_of(p) == ids(t).boss_event).count()
}

/// Boss bar events the host's client received.
fn host_bars(t: &T) -> usize {
    drain(t).iter().filter(|e| matches!(e, Event::Boss(_))).count()
}

/// Whether the advancement `id` is done for the host and for the guest.
fn advancement_done(t: &T, id: &str) -> (bool, bool) {
    let i = t.s.prog.tree.get(id).unwrap_or_else(|| panic!("advancement {id}"));
    (t.s.prog.done(i), guest(t).prog.done(i))
}

fn command(t: &mut T, line: &str) {
    let src = t.s.server_source();
    assert!(t.s.perform(src, line).0, "command refused: {line}");
}

/// A world open to one guest: the host at `host`, the guest at `guest_at`, both ready.
fn host_and_guest(tag: &str, host: [f64; 3], guest_at: [f64; 3]) -> (T, G) {
    let mut t = open(tag);
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    host_ready(&mut t, host);
    guest_ready(&mut t, guest_at);
    (t, g)
}

fn guest_mut(t: &mut T) -> &mut super::ctx::PlayerCtx {
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap()
}

/// The host dead, the guest alive: the guest is the world's random player (one draw of the world
/// random); everyone dead: none, without a draw. Fails on the code before (the spawners read the
/// host alone: with the host dead nothing was drawn); sabotage verified (failure): the list
/// limited to the installed player.
#[test]
fn the_random_player_of_a_world_may_be_a_guest() {
    let guest_at = feet([4, FLOOR + 1, 4]);
    let (mut t, _g) = host_and_guest("random-player", feet([-4, FLOOR + 1, -4]), guest_at);
    t.s.life.vit.dead = true;
    let mut expected = t.s.mobs.rng;
    assert_eq!(t.s.random_living_player_here(), Some(guest_at), "the living guest is drawn");
    assert_eq!(expected.next_int_bound(1), 0);
    assert_eq!(t.s.mobs.rng.next_long(), expected.next_long(), "one draw over a list of one");
    guest_mut(&mut t).vit.dead = true;
    let before = t.s.mobs.rng;
    assert_eq!(t.s.random_living_player_here(), None, "nobody alive");
    assert_eq!(t.s.mobs.rng.next_long(), { before }.next_long(), "no draw without a living player");
}

/// Cat spawner due, the host dead and the guest alive: the spawner draws around the guest (the
/// world random moves by the player draw and the cell offsets). Fails on the code before (a dead
/// host stopped the spawner before any draw).
#[test]
fn the_cat_spawner_draws_around_a_living_guest() {
    let (mut t, _g) = host_and_guest("cat-guest", feet([-4, FLOOR + 1, -4]), feet([4, FLOOR + 1, 4]));
    t.s.life.vit.dead = true;
    t.s.mobs.spawners.cat_next = 1;
    let mut expected = t.s.mobs.rng;
    let _ = expected.next_int_bound(1);
    let _ = (expected.next_int_bound(24), expected.next_bool(), expected.next_int_bound(24), expected.next_bool());
    t.s.cat_spawner_tick();
    assert_eq!(t.s.mobs.rng.next_long(), expected.next_long(), "the spawner drew a player and a cell");
}

/// Wandering trader attempt, the host dead and the guest alive: the attempt goes on around the
/// guest (its 1-in-10 draw fails here, so the chance is not reset). Fails on the code before (a
/// dead host made the attempt a success at once: chance back to 25).
#[test]
fn the_wandering_trader_attempt_goes_on_for_a_living_guest() {
    let (mut t, _g) = host_and_guest("trader-guest", feet([-4, FLOOR + 1, -4]), feet([4, FLOOR + 1, 4]));
    t.s.life.vit.dead = true;
    // A seed whose first 1-in-10 draw fails after the 1-in-100 one.
    let seed = (0..1000).find(|&s| {
        let mut r = JRand::new(s);
        let _ = r.next_int_bound(100);
        r.next_int_bound(10) != 0
    });
    let npc = &mut t.s.mobs.npc;
    npc.rng = JRand::new(seed.expect("a seed"));
    (npc.tick_delay, npc.spawn_delay, npc.spawn_chance, npc.loaded) = (1, 1, 100, true);
    let world_random = t.s.mobs.rng;
    t.s.trader_spawner_tick();
    assert_eq!(t.s.mobs.npc.spawn_chance, 75, "the attempt failed on its own draw, not at once");
    assert_ne!(t.s.mobs.rng.next_long(), { world_random }.next_long(), "the guest was drawn");
}

/// A zombie siege night: the guest stands by an occupied village bell, the host far away: the
/// siege is set up around the guest, its centre 32 blocks from them. Fails on the code before (the
/// installed player alone was looked at: no village around the host, no siege).
#[test]
fn a_zombie_siege_is_set_up_around_a_guest_in_a_village() {
    let guest_at = feet([4, FLOOR + 1, 4]);
    let (mut t, _g) = host_and_guest("siege-guest", [300.5, f64::from(FLOOR + 1), 0.5], guest_at);
    let bell = [5, FLOOR + 1, 5];
    command(&mut t, &format!("setblock {} {} {} minecraft:bell", bell[0], bell[1], bell[2]));
    server_tick(&mut t);
    assert!(t.s.poi_acquire(bell), "the bell's meeting point taken by a villager");
    assert!(t.s.is_village([4, FLOOR + 1, 4]), "the guest stands in a village");
    assert!(t.s.setup_siege(), "a siege around the guest");
    let at = t.s.mobs.spawners.siege_at;
    let (dx, dz) = (f64::from(at[0]) - 4.0, f64::from(at[2]) - 4.0);
    let d = (dx * dx + dz * dz).sqrt();
    assert!((30.0..=33.0).contains(&d), "centre {at:?} is {d:.1} blocks from the guest");
}

/// The host far out (beyond 192 blocks of the fight's centre), the guest near the portal: the
/// fight registers the guest alone, who gets the bar, the host nothing; the fight then holds a
/// player; at the respawn's end the guest alone gets `summoned_entity`. Fails on the code before
/// (the host's position alone decided: nobody registered, no bar, no advancement for the guest);
/// sabotage verified (failure): the zone test on the installed player only.
#[test]
fn the_dragon_fight_registers_a_guest_near_the_portal() {
    let (mut t, g) = host_and_guest("fight-guest", [400.5, f64::from(FLOOR + 1), 0.5], feet([2, FLOOR + 1, 2]));
    let mut fight = Fight::with_players_in_bar();
    fight.viewers = Default::default();
    t.s.mobs.end.fight = Some(Box::new(fight));
    t.s.mobs.end.fight_checked = true;
    let _ = (drain(&t), guest_packets(&g));
    t.s.fbar_update_players();
    let f = t.s.mobs.end.fight.as_ref().unwrap();
    assert_eq!(f.viewers.profiles(), &[guest(&t).uuid], "the guest alone registered");
    assert!(f.has_players(), "the fight goes on for the guest");
    assert_eq!(boss_packets(&t, &guest_packets(&g)), 1, "the guest gets the bar");
    assert_eq!(host_bars(&t), 0, "the host far away gets nothing");
    t.s.fight_set_stage(Stage::End);
    assert_eq!(advancement_done(&t, "minecraft:end/respawn_dragon"), (false, true), "summoned for the guest alone");
    // The guest leaves the zone: removed, told alone.
    let c = t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
    c.pos = [400.5, f64::from(FLOOR + 1), 0.5];
    let _ = (drain(&t), guest_packets(&g));
    t.s.fbar_update_players();
    assert!(!t.s.mobs.end.fight.as_ref().unwrap().has_players(), "nobody left in the fight");
    assert_eq!(boss_packets(&t, &guest_packets(&g)), 1, "the guest's bar removed");
    assert_eq!(host_bars(&t), 0);
}

/// A raid around the guest, the host far away: the bar registers the guest alone; the horn
/// reaches the guest (within 64 blocks), not the host; at the victory the guest, a hero, gets the
/// hero of the village effect and the `raid_win` statistic. Fails on the code before (bar, horn
/// and victory for the host alone); sabotage verified (failure): the victory on the installed
/// player only.
#[test]
fn a_raid_reaches_the_guests_in_it() {
    let (mut t, g) = host_and_guest("raid-guest", [300.5, f64::from(FLOOR + 1), 0.5], feet([2, FLOOR + 1, 2]));
    let mut r = crate::mobs::illagers::raids::test_raid([0, FLOOR + 1, 0], 2);
    r.active = true;
    let id = r.id;
    t.s.mobs.ill.raids.loaded = true;
    t.s.mobs.ill.raids.list.push(r);
    let _ = (drain(&t), guest_packets(&g));
    t.s.raid_update_players(id);
    let viewers = t.s.mobs.ill.raids.list[0].viewers.clone();
    assert_eq!(viewers.profiles(), &[guest(&t).uuid], "the guest alone registered");
    assert_eq!(boss_packets(&t, &guest_packets(&g)), 1, "the guest gets the bar");
    assert_eq!(host_bars(&t), 0, "the host far away gets nothing");
    t.s.raid_horn(id, [0, FLOOR + 1, 0]);
    let horn = sound_id(&t, "minecraft:event.raid.horn");
    assert!(sounds_in(&t, &guest_packets(&g)).contains(&horn), "the guest hears the horn");
    assert!(!drain(&t).iter().any(|e| matches!(e, Event::Sound(_))), "the host 300 blocks away does not");
    let heroes = vec![guest(&t).uuid];
    t.s.reward_raid_heroes(&heroes, 1);
    assert!(guest(&t).fx.has("minecraft:hero_of_the_village"), "hero effect for the guest");
    assert!(!t.s.life.fx.has("minecraft:hero_of_the_village"), "none for the host");
}

/// The ravager's roar next to the guest, an armor stand and a mannequin: the guest is hurt (not
/// pushed), the mannequin hurt and pushed, the stand pushed (with `mob_griefing`), the host far
/// away untouched; without `mob_griefing`, the stand stays put. Fails on the code before (mobs
/// and the installed player only).
#[test]
fn the_ravager_roar_strikes_guests_and_living_decorations() {
    let (mut t, _g) = host_and_guest("roar-guest", feet([-5, FLOOR + 1, -5]), feet([2, FLOOR + 1, 2]));
    command(&mut t, &format!("summon minecraft:armor_stand 3.5 {} 4.5", FLOOR + 1));
    command(&mut t, &format!("summon minecraft:mannequin 4.5 {} 2.5", FLOOR + 1));
    let living = t.s.deco_living();
    let of_kind = |kind: &str| {
        living.iter().find(|d| t.s.deco_nbt(d.id).and_then(|e| e.get("id")).and_then(Nbt::as_str) == Some(kind)).map(|d| d.id)
    };
    let (stand, mannequin) = (of_kind("minecraft:armor_stand").expect("stand"), of_kind("minecraft:mannequin").expect("mannequin"));
    let motion = |t: &T, id: i32| crate::nbtio::motion(t.s.deco_nbt(id).and_then(|e| e.get("Motion"))).unwrap_or_default();
    let health = |t: &T, id: i32| match t.s.deco_nbt(id).and_then(|e| e.get("Health")) {
        Some(Nbt::Float(h)) => *h,
        _ => 20.0,
    };
    let roar = |t: &mut T| {
        let rav = t.s.new_mob(Kind::Ravager, feet([3, FLOOR + 1, 3]), 0.0);
        let rid = rav.id;
        t.s.add_mob(rav);
        let i = t.s.mob_index(rid).expect("ravager");
        let mut m = t.s.mobs.list.take(i).expect("ravager");
        t.s.ravager_roar(&mut m);
        t.s.mobs.list.put(i, m);
    };
    roar(&mut t);
    assert!(guest(&t).vit.health < 20.0, "the guest is hurt");
    assert_eq!(t.s.life.vit.health, 20.0, "the host far away is not");
    assert!(health(&t, mannequin) < 20.0, "the mannequin is hurt");
    assert!(motion(&t, mannequin)[1] > 0.0, "the mannequin is pushed");
    assert!(motion(&t, stand)[1] > 0.0, "the stand is pushed with mob_griefing");
    crate::nbtio::set(&mut t.s.rules, "minecraft:mob_griefing", Nbt::Byte(0));
    t.s.deco_edit(stand, |n| crate::nbtio::set(n, "Motion", Nbt::List(vec![Nbt::Double(0.0); 3])));
    roar(&mut t);
    assert_eq!(motion(&t, stand), [0.0; 3], "without mob_griefing, the stand is left alone");
}

/// An iron golem built near the guest, the host farther: the guest alone gets `summoned_entity`
/// ("Hired Help"); the wither's search walks the same players. Fails on the code before (the
/// installed player alone was looked at); sabotage verified (failure): the other players left
/// out of the search.
#[test]
fn a_summoning_reaches_the_players_around_it() {
    let (mut t, _g) = host_and_guest("golem-guest", feet([-6, FLOOR + 1, -6]), feet([2, FLOOR + 1, 4]));
    let y = FLOOR + 1;
    command(&mut t, &format!("setblock 4 {y} 4 minecraft:iron_block"));
    command(&mut t, &format!("setblock 3 {} 4 minecraft:iron_block", y + 1));
    command(&mut t, &format!("setblock 5 {} 4 minecraft:iron_block", y + 1));
    command(&mut t, &format!("setblock 4 {} 4 minecraft:iron_block", y + 1));
    command(&mut t, &format!("setblock 4 {} 4 minecraft:carved_pumpkin", y + 2));
    assert!(t.s.mobs.list.iter().any(|m| m.kind == Kind::IronGolem), "golem built");
    assert_eq!(advancement_done(&t, "minecraft:adventure/summon_iron_golem"), (false, true));
    let wither = t.s.new_mob(Kind::Wither, feet([0, FLOOR + 1, 0]), 0.0);
    let b = wither.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - 50.0), max: b.max.map(|v| v + 50.0) };
    t.s.trig_summoned_for_players_touching(&area, crate::progress::pred::Who::Mob(&wither));
    assert_eq!(advancement_done(&t, "minecraft:nether/summon_wither"), (true, true), "both within 50 blocks");
}

/// A flaming arrow of the guest hits TNT while the host is the installed player, without
/// `mob_griefing`: a player's projectile may always interact, the TNT is primed; one whose owner
/// no longer resolves too. Fails on the code before (only the installed player's projectile
/// counted as a player's: refused like a mob's).
#[test]
fn a_projectile_of_any_player_may_prime_tnt() {
    let (mut t, _g) = host_and_guest("tnt-guest", feet([-5, FLOOR + 1, -5]), feet([5, FLOOR + 1, 5]));
    crate::nbtio::set(&mut t.s.rules, "minecraft:mob_griefing", Nbt::Byte(0));
    let tnt = t.s.default_state("minecraft:tnt");
    let air = t.s.default_state("minecraft:air");
    for (owner, at) in [(guest(&t).uuid, [0, FLOOR + 1, 0]), (0x5eed_u128, [2, FLOOR + 1, 0])] {
        t.s.set_block(at, tnt, 2);
        let item = crate::player::Stack { id: "minecraft:arrow".into(), count: 1, components: None };
        let mut a = Proj::new(PKind::Arrow, 900_950, feet([at[0], at[1] + 1, at[2]]), item);
        (a.owner, a.fire) = (Some(owner), 100);
        let loc = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 1.0, f64::from(at[2]) + 0.5];
        t.s.block_projectile_hit(&mut a, BlockHit { loc, pos: at, face: 1, inside: false, border: false });
        assert_eq!(t.s.get(at), air, "TNT primed for owner {owner:x}");
    }
}
