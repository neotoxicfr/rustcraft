//! Players among themselves on an open world, like the game's entity tracking: an observer paired
//! late is told every synced value that differs from the defaults (health, air, freezing, stuck
//! arrows…); the values a player writes during a tick reach its observers in one packet at the
//! tracking pass; a dead player leaves the others' world after 20 ticks and comes back as a new
//! entity when it respawns; the pose shown to the others is the player's real one (swimming,
//! gliding, asleep, spin attack, a crouch or crawl forced by a low ceiling); a living decoration
//! given a transmit range shows on the locator bar like any living entity.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Meta;
use rc_proto_v777::waypoint::{WaypointId, WaypointOp, WaypointPos};

use super::tests_g3f05::{
    G, T, Told, body, drain, guest_packets, guest_told, host_told, id_of, ids, join, open, publish, server_tick,
};
use crate::mobs::PlayerPose;
use crate::mobs::species::Kind;

const FLOOR: i32 = 199;

fn run(t: &mut T, c: &str) -> bool {
    let mut src = t.s.player_source();
    src.level = 4;
    t.s.perform(src, c).0
}

/// A guest that joined and was paired with the host (its chunks sent, then the tracking pass).
fn paired_guest(t: &mut T) -> G {
    let g = join(t, "Guest", 2);
    for _ in 0..3 {
        t.s.lan_tick();
    }
    let host = t.s.player_id;
    assert!(guest_told(t, &g, host).contains(&Told::Added), "the guest sees the host");
    let _ = drain(t);
    g
}

/// The values of every data packet in `told`, packet by packet.
fn data_packets(told: &[Told]) -> Vec<&Vec<(u8, Meta)>> {
    told.iter().filter_map(|step| if let Told::Data(values) = step { Some(values) } else { None }).collect()
}

/// Value of synced index `index` in the last data packet that carries it.
fn last_value(told: &[Told], index: u8) -> Option<Meta> {
    let mut values = data_packets(told).into_iter().flat_map(|values| values.iter());
    values.rfind(|(i, _)| *i == index).map(|(_, v)| v.clone())
}

/// Values written for the host before any guest arrived: a guest paired later is told each one
/// that differs from the entity defaults, in index order. Fails on the code before (the pairing
/// only carried flags, sneaking, effect particles and skin parts); sabotage verified (failure):
/// pairing values left empty.
#[test]
fn observer_paired_later_is_told_the_values_written_before_it_arrived() {
    let mut t = open("pairing-held-values");
    publish(&mut t, false);
    // Before any guest: the host gets its breath back after a dive, thaws, takes two arrows.
    t.s.life.vit.air = 200;
    t.s.life.vit.ticks_frozen = 100;
    t.s.vitals_tick();
    let host = t.s.player_id;
    let _ = t.s.send(Event::Entity(EntityEvent::Data { id: host, values: vec![(12, Meta::Int(2))] }));
    let (air, frozen) = (t.s.life.vit.air, t.s.life.vit.ticks_frozen);
    assert!(air < 300 && frozen > 0, "air {air}, frozen {frozen}");

    let g = join(&mut t, "Guest", 2);
    for _ in 0..3 {
        t.s.lan_tick();
    }
    let told = guest_told(&t, &g, host);
    assert_eq!(told.first(), Some(&Told::Added), "{told:?}");
    let pairing = data_packets(&told)[0];
    for wanted in [(1, Meta::Int(air)), (7, Meta::Int(frozen)), (9, Meta::Float(20.0)), (12, Meta::Int(2))] {
        assert!(pairing.contains(&wanted), "{wanted:?} missing from the pairing data {pairing:?}");
    }
    assert!(pairing.windows(2).all(|pair| pair[0].0 < pair[1].0), "index order: {pairing:?}");
}

/// Three values written at three moments of a tick reach an observer together, in one packet at
/// the tracking pass, in index order; the player's own client still gets each one at once. Fails
/// on the code before (three packets, each sent at once to every player); sabotage verified
/// (failure): the tick's values sent one packet per write.
#[test]
fn values_of_one_tick_reach_the_observers_in_one_packet() {
    let mut t = open("one-packet-per-tick");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    let host = t.s.player_id;
    let _ = guest_told(&t, &g, host);
    for values in [vec![(12, Meta::Int(1))], vec![(1, Meta::Int(120))], vec![(7, Meta::Int(40))]] {
        let _ = t.s.send(Event::Entity(EntityEvent::Data { id: host, values }));
    }
    assert!(data_packets(&guest_told(&t, &g, host)).is_empty(), "nothing before the tracking pass");
    let own = super::tests_g3f05::host_told(&t, host);
    assert_eq!(data_packets(&own).len(), 3, "its own client is told each one at once");
    t.s.lan_tick();
    let told = guest_told(&t, &g, host);
    let packets = data_packets(&told);
    assert_eq!(packets.len(), 1, "{told:?}");
    assert_eq!(packets[0], &vec![(1, Meta::Int(120)), (7, Meta::Int(40)), (12, Meta::Int(1))]);
}

/// Death: the observer is told the health of zero (its client lays the body down), the body leaves
/// its world 20 ticks later with the puff, and the respawned player comes back as a new entity
/// with full health. Respawning before the 20 ticks removes the old entity and adds the new one in
/// the same pass. Fails on the code before (the health never sent, the body kept until the
/// respawn, the respawned player the same entity); sabotage verified (failure): no removal after
/// death; the entity not renewed at respawn.
#[test]
fn a_dead_player_leaves_and_comes_back_as_a_new_entity() {
    let mut t = open("death-and-respawn");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    let host = t.s.player_id;
    let _ = guest_told(&t, &g, host);
    assert!(t.s.hurt("minecraft:generic_kill", 1000.0) && t.s.life.vit.dead, "the host died");
    t.s.vitals_tick();
    t.s.lan_tick();
    let told = guest_told(&t, &g, host);
    assert_eq!(last_value(&told, 9), Some(Meta::Float(0.0)), "health of zero: {told:?}");
    assert!(!told.contains(&Told::Removed), "the body stays during the death animation");
    for _ in 0..19 {
        t.s.vitals_tick();
        t.s.lan_tick();
    }
    assert_eq!(t.s.life.vit.death_time, 20);
    let told = guest_told(&t, &g, host);
    assert_eq!(told.last(), Some(&Told::Removed), "removed with the puff: {told:?}");
    assert_eq!(told.iter().filter(|step| **step == Told::Removed).count(), 1);
    t.s.perform_respawn();
    t.s.lan_tick();
    let told = guest_told(&t, &g, host);
    assert_eq!(told.first(), Some(&Told::Added), "{told:?}");
    assert_eq!(last_value(&told, 9), Some(Meta::Float(20.0)), "new entity, full health: {told:?}");

    // Respawn before the end of the death animation (immediate respawn): old out, new in.
    t.s.loaded = true;
    // The respawned host sees the guest again once the guest's chunk has reached it.
    let guest = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().player_id;
    let host_told = |t: &T| super::tests_g3f05::host_told(t, guest);
    assert_eq!(host_told(&t), vec![Told::Removed], "the removed host stopped seeing the guest");
    t.s.sent.insert((0, 0));
    t.s.lan_tick();
    assert_eq!(host_told(&t).first(), Some(&Told::Added), "the host sees the guest again");
    assert!(t.s.hurt("minecraft:generic_kill", 1000.0));
    t.s.vitals_tick();
    t.s.lan_tick();
    let _ = guest_told(&t, &g, host);
    let _ = host_told(&t);
    t.s.perform_respawn();
    // The guest's chunk goes out again to the new host entity within the same tick.
    t.s.sent.insert((0, 0));
    t.s.lan_tick();
    let told = guest_told(&t, &g, host);
    assert_eq!(told[..2], [Told::Removed, Told::Added], "{told:?}");
    assert_eq!(last_value(&told, 9), Some(Meta::Float(20.0)));
    // The old host entity stopped tracking the guest: removed from the new one's world, then paired
    // again, like the game, even though the guest never left its sight. Sabotage verified
    // (failure): only the observed side renewed.
    let told_to_host = host_told(&t);
    assert_eq!(told_to_host[..2], [Told::Removed, Told::Added], "{told_to_host:?}");
}

/// Singleplayer respawn, the stream its own client gets: the new entity starts from the defaults,
/// so the player's skin parts and main arm (data 15 and 16) go out again after the respawn packet,
/// and a score kept by the keep-inventory rule (data 18) too, like the game's new player. A wanted
/// change of the singleplayer stream (the code before this series sent neither); sabotage verified
/// (failure): the model data not sent at respawn.
#[test]
fn a_respawned_player_is_told_its_model_and_kept_score_again() {
    let mut t = open("solo-respawn-stream");
    let host = t.s.player_id;
    t.s.set_model(0x3f, 0);
    assert!(run(&mut t, "gamerule keep_inventory true"));
    t.s.life.vit.score = 7;
    t.s.vitals_tick();
    assert!(t.s.hurt("minecraft:generic_kill", 1000.0) && t.s.life.vit.dead, "the player died");
    t.s.vitals_tick();
    let _ = drain(&t);
    t.s.perform_respawn();
    t.s.loaded = true;
    t.s.vitals_tick();
    let told = super::tests_g3f05::host_told(&t, host);
    let model = [(15, Meta::Variant(0, 42)), (16, Meta::Byte(0x3f))];
    assert!(data_packets(&told).iter().any(|values| model.iter().all(|v| values.contains(v))), "model: {told:?}");
    assert_eq!(last_value(&told, 18), Some(Meta::Int(7)), "kept score: {told:?}");
}

/// The pose another player sees is the player's own: swimming, gliding, spinning with a trident,
/// asleep, crouching, standing when flying with shift held. Fails on the code before (only
/// crouching or standing went out); sabotage verified (failure): the pose sent from shift alone.
#[test]
fn others_see_every_pose_of_a_player() {
    let mut t = open("pose-to-observers");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    let host = t.s.player_id;
    let shown = |t: &mut T, set: &dyn Fn(&mut crate::server::Server, bool)| {
        set(&mut t.s, true);
        t.s.update_player_pose();
        t.s.lan_tick();
        set(&mut t.s, false);
        let pose = last_value(&guest_told(t, &g, host), 6);
        t.s.update_player_pose();
        t.s.lan_tick();
        let _ = guest_told(t, &g, host);
        pose
    };
    assert_eq!(shown(&mut t, &|s, on| s.mobs.sea.player_swimming = on), Some(Meta::Pose(3)), "swimming");
    assert_eq!(shown(&mut t, &|s, on| s.proj_player.fall_flying = on), Some(Meta::Pose(1)), "gliding");
    assert_eq!(shown(&mut t, &|s, on| s.proj_player.spin = i32::from(on) * 10), Some(Meta::Pose(4)), "spin attack");
    let bed = [0, FLOOR + 1, 0];
    assert_eq!(shown(&mut t, &|s, on| s.life.vit.sleeping = on.then_some(bed)), Some(Meta::Pose(2)), "asleep");
    assert_eq!(shown(&mut t, &|s, on| s.sneaking = on), Some(Meta::Pose(5)), "crouching");
    t.s.flying = true;
    assert_eq!(shown(&mut t, &|s, on| s.sneaking = on), None, "shift held while flying: still standing");
}

/// A ceiling 1.5 blocks above the feet forces a crouch (box 1.5 high, hidden from the locator
/// bar, crouching for the others); 1 block forces a crawl (the swimming pose, box 0.6); open air
/// lets the player stand. A spectator keeps the pose it asks for. Fails on the code before (the
/// forced crouch was not modelled); sabotage verified (failure): the room never measured.
#[test]
fn a_low_ceiling_forces_a_crouch_then_a_crawl() {
    let mut t = open("ceiling-pose");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    let host = t.s.player_id;
    let _ = guest_told(&t, &g, host);
    let range = |t: &T| t.s.player_attribute(crate::waypoints::TRANSMIT_RANGE);
    assert!(range(&t) > 0.0, "on the bar standing");
    // Feet at FLOOR + 1: a top slab in the cell above the head leaves 1.5 blocks.
    assert!(run(&mut t, &format!("setblock 0 {} 0 minecraft:stone_slab[type=top]", FLOOR + 2)));
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Crouching);
    assert_eq!(t.s.player_dims().1, 1.5);
    assert_eq!(range(&t), 0.0, "a forced crouch hides from the bar too");
    t.s.lan_tick();
    assert_eq!(last_value(&guest_told(&t, &g, host), 6), Some(Meta::Pose(5)));
    // A full block there: one block of room, the player crawls.
    assert!(run(&mut t, &format!("setblock 0 {} 0 minecraft:stone", FLOOR + 2)));
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Swimming);
    assert_eq!(t.s.player_dims().1, 0.6);
    t.s.lan_tick();
    assert_eq!(last_value(&guest_told(&t, &g, host), 6), Some(Meta::Pose(3)));
    // A spectator is never forced.
    t.s.game_mode = 3;
    assert_eq!(t.s.player_pose(), PlayerPose::Standing);
    t.s.game_mode = 0;
    // Open air again: standing.
    assert!(run(&mut t, &format!("setblock 0 {} 0 minecraft:air", FLOOR + 2)));
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Standing);
    t.s.lan_tick();
    assert_eq!(last_value(&guest_told(&t, &g, host), 6), Some(Meta::Pose(0)));
}

/// A bed under glass: lying down is allowed (glass does not suffocate), but the flat box no longer
/// fits on the bed, so the end-of-tick rule keeps the pose the bed set: asleep, box 0.2, shown
/// asleep to the others; getting up sets the standing pose again. Fails on the code before (the
/// rule kept the standing pose of the tick before, and the guest saw the sleeper standing).
#[test]
fn a_player_asleep_under_glass_stays_asleep() {
    let mut t = open("asleep-under-glass");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    let host = t.s.player_id;
    let bed = FLOOR + 1;
    for c in [
        format!("setblock 1 {bed} 0 minecraft:red_bed[facing=east,part=foot]"),
        format!("setblock 2 {bed} 0 minecraft:red_bed[facing=east,part=head]"),
        format!("setblock 1 {} 0 minecraft:glass", bed + 1),
        format!("setblock 2 {} 0 minecraft:glass", bed + 1),
        "time set 18000".to_owned(),
    ] {
        assert!(run(&mut t, &c), "{c}");
    }
    t.s.update_player_pose();
    let _ = guest_told(&t, &g, host);
    assert!(t.s.use_bed([1, bed, 0]) && t.s.life.vit.sleeping.is_some(), "lying down under glass");
    t.s.update_player_pose();
    assert_eq!(t.s.life.vit.pose_room.headroom, crate::mobs::Headroom::None, "not even the flat box fits");
    assert_eq!(t.s.player_pose(), PlayerPose::Asleep);
    assert_eq!(t.s.player_dims().1, f64::from(0.2f32));
    t.s.lan_tick();
    assert_eq!(last_value(&guest_told(&t, &g, host), 6), Some(Meta::Pose(2)), "the guest sees the sleeper asleep");

    t.s.wake_up();
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Standing);
    t.s.lan_tick();
    assert_eq!(last_value(&guest_told(&t, &g, host), 6), Some(Meta::Pose(0)), "standing again");
}

/// A shulker 1.5 blocks above the feet forces a crouch like a slab would: the fit test counts the
/// solid entities, like the game's. Dead, it no longer counts. Fails on the code before (the test
/// read the blocks only, the player stayed standing).
#[test]
fn a_shulker_overhead_forces_a_crouch() {
    let mut t = open("shulker-ceiling");
    let shulker = t.s.new_mob(crate::mobs::species::Kind::Shulker, [0.5, f64::from(FLOOR) + 2.5, 0.5], 0.0);
    t.s.add_mob(shulker);
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Crouching, "the shulker's box is the ceiling");
    t.s.mobs.list.iter_mut().for_each(|m| m.health = 0.0);
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Standing, "a dead shulker is not solid");
}

/// A player crawling along a one-block tunnel moves freely: the server checks the move with the box
/// of its pose, like the game, not a standing box that would touch the ceiling at every step.
/// Fails on the code before (the move was refused and the player sent back); sabotage verified
/// (failure): the standing box in the check.
#[test]
fn a_crawling_player_moves_along_a_one_block_tunnel() {
    let mut t = open("crawl-tunnel");
    for x in -1..=3 {
        assert!(run(&mut t, &format!("setblock {x} {} 0 minecraft:stone", FLOOR + 2)));
    }
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Swimming, "crawling");
    let _ = drain(&t);
    let target = [1.5, f64::from(FLOOR + 1), 0.5];
    t.s.on_move(Some(target), None, true);
    assert_eq!(t.s.pos, target, "the move is accepted");
    assert!(!drain(&t).iter().any(|e| matches!(e, Event::Teleport(_))), "not sent back");
}

/// `RUSTCRAFT_OPT_LIVING_DECO_ATTRIBUTE_ONE_WALK`: whether a living decoration's attribute can
/// stray from its default reads the same through one walk of its fields as through two lookups,
/// for the waypoint range and the scale, on compounds with and without saved entries and worn
/// stacks of their own. Sabotage verified (failure): the walk skipping the equipment.
#[test]
fn living_decoration_attribute_check_is_the_same_on_both_paths() {
    use crate::decoration::stand::attribute_may_differ_with;
    use rc_proto::nbt::Nbt;
    let entry = |id: &str| Nbt::Compound(vec![("id".into(), Nbt::String(id.into())), ("base".into(), Nbt::Double(50.0))]);
    let stack = |components: Option<&str>| {
        let mut fields = vec![("id".into(), Nbt::String("minecraft:leather_helmet".into())), ("count".into(), Nbt::Int(1))];
        if let Some(c) = components {
            fields.push(("components".into(), Nbt::Compound(vec![(c.into(), Nbt::Compound(Vec::new()))])));
        }
        Nbt::Compound(fields)
    };
    let stand = |attributes: Option<Vec<Nbt>>, head: Option<Nbt>| {
        let mut fields = vec![("id".into(), Nbt::String("minecraft:armor_stand".into())), ("Invisible".into(), Nbt::Byte(0))];
        if let Some(list) = attributes {
            fields.push(("attributes".into(), Nbt::List(list)));
        }
        if let Some(s) = head {
            fields.push(("equipment".into(), Nbt::Compound(vec![("head".into(), s)])));
        }
        fields.push(("Pos".into(), Nbt::List(vec![Nbt::Double(0.0); 3])));
        Nbt::Compound(fields)
    };
    let range = crate::waypoints::TRANSMIT_RANGE;
    let cases = [
        (stand(None, None), false, false),
        (stand(Some(vec![entry(range)]), None), true, false),
        (stand(Some(vec![entry("minecraft:scale")]), None), false, true),
        (stand(None, Some(stack(None))), false, false),
        (stand(None, Some(stack(Some("minecraft:attribute_modifiers")))), true, true),
        (stand(None, Some(stack(Some("minecraft:enchantments")))), true, true),
        (stand(Some(vec![entry("minecraft:armor")]), Some(stack(Some("minecraft:custom_name")))), false, false),
    ];
    for (k, (e, range_may_differ, scale_may_differ)) in cases.iter().enumerate() {
        for one_walk in [true, false] {
            assert_eq!(attribute_may_differ_with(e, range, one_walk), *range_may_differ, "case {k}, one walk {one_walk}");
            assert_eq!(attribute_may_differ_with(e, "minecraft:scale", one_walk), *scale_may_differ, "case {k}, one walk {one_walk}");
        }
    }
}

/// An armor stand given a transmit range by `/attribute` shows on the host's bar after its first
/// tick, like any living entity of the game; `/waypoint list` names it, `/waypoint modify` sends
/// it again in its new colour, and back to zero it is gone. Fails on the code before (living
/// decorations were never transmitters); sabotage verified (failure): decorations left out of the
/// snapshot.
#[test]
fn an_armor_stand_with_a_range_shows_on_the_locator_bar() {
    let mut t = open("waypoint-armor-stand");
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    t.s.load_now(&around);
    assert!(run(&mut t, &format!("summon minecraft:armor_stand 6.5 {} 0.5 {{NoGravity:1b}}", FLOOR + 1)));
    assert!(run(&mut t, "attribute @e[type=armor_stand,limit=1] minecraft:waypoint_transmit_range base set 100"));
    let (id, uuid) = t.s.deco_living().into_iter().map(|d| (d.id, d.uuid)).next().expect("the armor stand");
    let _ = drain(&t);
    t.s.waypoints.note_movement_packet(t.s.player_id);
    t.s.waypoints_sync();
    let waypoints_about_stand = |t: &T| -> Vec<(WaypointOp, WaypointPos)> {
        drain(t)
            .into_iter()
            .filter_map(|e| if let Event::Waypoint(w) = e { Some(*w) } else { None })
            .filter(|w| w.id == WaypointId::Uuid(uuid))
            .map(|w| (w.op, w.pos))
            .collect()
    };
    assert!(waypoints_about_stand(&t).is_empty(), "no connection before its first tick");
    t.s.game_tick();
    t.s.tick += 1;
    let _ = drain(&t);
    t.s.waypoints.note_movement_packet(t.s.player_id);
    t.s.waypoints_sync();
    let block = WaypointPos::Block([6, FLOOR + 1, 0]);
    assert_eq!(waypoints_about_stand(&t), vec![(WaypointOp::Track, block)], "stand {id} on the bar");

    let mut src = t.s.player_source();
    src.level = 4;
    assert_eq!(t.s.perform(src, "waypoint list"), (true, 2), "host and armor stand");
    assert!(run(&mut t, "waypoint modify @e[type=armor_stand,limit=1] color red"));
    let ops: Vec<WaypointOp> = waypoints_about_stand(&t).into_iter().map(|(op, _)| op).collect();
    assert_eq!(ops, vec![WaypointOp::Untrack, WaypointOp::Track], "tracked again in its new colour");
    assert!(run(&mut t, "attribute @e[type=armor_stand,limit=1] minecraft:waypoint_transmit_range base set 0"));
    t.s.waypoints_sync();
    assert_eq!(waypoints_about_stand(&t), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
}

/// Base value of attribute `name` in every attribute packet a guest received about `entity`, packet
/// by packet (the packets are read back by the client's decoder).
fn attribute_packets(t: &T, g: &G, entity: i32, name: &str) -> Vec<Option<f64>> {
    use super::tests_g3f05::{body, guest_packets, id_of, ids};
    let reg = t.s.data.registry("minecraft:attribute").expect("attribute registry");
    let wanted = reg.entries.iter().position(|e| e == name).expect("known attribute") as i32;
    let packet_id = ids(t).update_attributes;
    guest_packets(g)
        .iter()
        .filter(|p| id_of(p) == packet_id)
        .filter_map(|p| rc_proto_v777::play::read_update_attributes(&mut rc_proto::Reader::new(body(p))).ok())
        .filter(|(id, _)| *id == entity)
        .map(|(_, attributes)| attributes.iter().find(|a| a.id == wanted).map(|a| a.base))
        .collect()
}

/// A player's synced attributes reach the other players like the game's tracker sends them: a
/// guest paired later is told the host's scale with the rest of the pairing, and a change made
/// while it watches reaches it at the next tracking pass, once. Fails on the code before (the
/// attributes only ever went to the player's own client); sabotage verified (failure): the
/// pairing's attributes left out; the tick's changes not recorded.
#[test]
fn a_players_attributes_reach_the_other_players() {
    let mut t = open("attributes-to-observers");
    publish(&mut t, false);
    assert!(run(&mut t, "attribute @s minecraft:scale base set 2"));
    t.s.sync_attributes();
    let host = t.s.player_id;
    let g = join(&mut t, "Guest", 2);
    for _ in 0..3 {
        t.s.lan_tick();
    }
    assert_eq!(attribute_packets(&t, &g, host, "minecraft:scale"), vec![Some(2.0)], "told at pairing");
    assert!(run(&mut t, "attribute @s minecraft:scale base set 0.5"));
    t.s.sync_attributes();
    assert!(attribute_packets(&t, &g, host, "minecraft:scale").is_empty(), "nothing before the tracking pass");
    t.s.lan_tick();
    assert_eq!(attribute_packets(&t, &g, host, "minecraft:scale"), vec![Some(0.5)], "the change, once");
    t.s.sync_attributes();
    t.s.lan_tick();
    assert!(attribute_packets(&t, &g, host, "minecraft:scale").is_empty(), "unchanged: nothing more");
}

/// After the end credits the game makes a new player entity, as after a death: the observers drop
/// the old entity and are paired with the new one, and the player's own client is told to keep
/// only its attribute modifiers (its synced values start from the defaults and the kept ones are
/// told again). The new entity has no arrow stuck in it, and its effects are added again: the
/// absorption effect fills a fresh reserve instead of carrying over what was left. Fails on the
/// code before (the same entity kept, the client told to keep all its data; the arrows and the
/// absorption left carried over); sabotage verified (failure): the entity not renewed.
#[test]
fn after_the_end_credits_the_player_is_a_new_entity() {
    let mut t = open("credits-new-entity");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    let host = t.s.player_id;
    let _ = guest_told(&t, &g, host);
    assert!(run(&mut t, "setworldspawn 0 200 0"));
    // Three arrows stuck, and one point left of an Absorption I reserve of four.
    t.s.proj_player.arrow_count = 3;
    t.s.proj_player.arrow_remove = 5;
    assert!(run(&mut t, "effect give @s minecraft:absorption 60 0"));
    assert_eq!(t.s.life.fx.absorption, 4.0);
    t.s.life.fx.absorption = 1.0;
    t.s.dims.won_game = true;
    t.s.respawn_after_credits();
    assert_eq!((t.s.proj_player.arrow_count, t.s.proj_player.arrow_remove), (0, 0), "no arrow stuck in the new entity");
    assert_eq!(t.s.life.fx.absorption, 4.0, "the absorption effect fills the new entity's reserve again");
    let keep: Vec<u8> = drain(&t).into_iter().filter_map(|e| if let Event::Respawn { keep, .. } = e { Some(keep) } else { None }).collect();
    assert_eq!(keep, vec![crate::dims::KEEP_ATTRIBUTE_MODIFIERS], "attribute modifiers kept, entity data dropped");
    t.s.lan_tick();
    let told = guest_told(&t, &g, host);
    let renewal: Vec<&Told> = told.iter().filter(|step| matches!(step, Told::Removed | Told::Added)).collect();
    assert_eq!(renewal, vec![&Told::Removed, &Told::Added], "old entity out, new one in: {told:?}");
}

/// Mounting inside a wall stands the player up, like any entity starting a ride in the game: with
/// no room at all at the seat, the end-of-tick rule keeps that standing pose instead of the crawl
/// of before. Fails on the code before (the crawl was kept); sabotage verified (failure): the
/// mount path without the stand-up.
#[test]
fn mounting_inside_a_wall_stands_the_player_up() {
    let mut t = open("mount-in-wall");
    for x in -1..=3 {
        assert!(run(&mut t, &format!("setblock {x} {} 0 minecraft:stone", FLOOR + 2)));
    }
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Swimming, "crawling under the one-block ceiling");
    let mut horse = t.s.new_mob(crate::mobs::species::Kind::Horse, t.s.pos, 0.0);
    horse.no_ai = true;
    assert!(t.s.player_start_riding(&mut horse), "mounted");
    t.s.add_mob(horse);
    t.s.update_player_pose();
    assert_eq!(t.s.life.vit.pose_room.headroom, crate::mobs::Headroom::None, "the seat is inside the ceiling");
    assert_eq!(t.s.player_pose(), PlayerPose::Standing);
}

/// The boxes of a player's poses follow its scale attribute, like the game's: at half size a
/// one-block gap leaves room to stand (box 0.9 high), at double size a three-block room only lets
/// it crouch (3.0 high, standing would be 3.6), and the server box is the scaled one. Fails on the
/// code before (crawling, then standing: the scale was ignored).
#[test]
fn the_pose_boxes_follow_the_players_scale() {
    let mut t = open("scaled-poses");
    assert!(run(&mut t, &format!("fill -1 {0} -1 1 {0} 1 minecraft:stone", FLOOR + 2)));
    assert!(run(&mut t, "attribute @s minecraft:scale base set 0.5"));
    t.s.sync_attributes();
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Standing, "half size: room to stand under one block");
    let b = t.s.player_aabb();
    assert!((b.max[1] - b.min[1] - 0.9).abs() < 1e-9 && (b.max[0] - b.min[0] - 0.3).abs() < 1e-9, "{b:?}");

    assert!(run(&mut t, &format!("fill -1 {0} -1 1 {0} 1 minecraft:air", FLOOR + 2)));
    assert!(run(&mut t, &format!("fill -1 {0} -1 1 {0} 1 minecraft:stone", FLOOR + 4)));
    assert!(run(&mut t, "attribute @s minecraft:scale base set 2"));
    t.s.sync_attributes();
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), PlayerPose::Crouching, "double size: only crouching fits three blocks");
}

/// A host removed after its death leaves the guests' world as it was: the mobs it tracked stay on
/// their clients and the block changes around still reach them, while its own client alone is
/// told the mobs are gone; at its respawn only its own client is told the items and orbs again.
/// Fails on the code before (the host's view emptied: every mob removed from the guests' clients
/// until the host respawned, no block change sent to them; items and orbs announced again to
/// guests that still showed them).
#[test]
fn a_removed_host_leaves_the_guests_world_as_it_was() {
    let mut t = open("removed-host-guests");
    publish(&mut t, false);
    let g = paired_guest(&mut t);
    assert!(run(&mut t, &format!("summon minecraft:cow 3.5 {} 3.5 {{NoAI:1b,PersistenceRequired:1b}}", FLOOR + 1)));
    let cow = t.s.mobs.list.iter().find(|m| m.kind == Kind::Cow).expect("the cow").id;
    // Out of every player's reach: an orb follows a player within 8 blocks.
    assert!(run(&mut t, &format!("summon minecraft:experience_orb -5.5 {} -5.5 {{Value:3}}", FLOOR + 1)));
    assert!(run(&mut t, &format!("summon minecraft:item -5.5 {} 5.5 {{Item:{{id:\"minecraft:stone\"}}}}", FLOOR + 1)));
    let (orb, item) = (t.s.life.orbs[0].id, t.s.life.drops[0].id);
    server_tick(&mut t);
    assert!(guest_told(&t, &g, cow).contains(&Told::Added), "the guest sees the cow");
    let _ = drain(&t);

    assert!(t.s.hurt("minecraft:generic_kill", 1000.0) && t.s.life.vit.dead, "the host died");
    for _ in 0..25 {
        server_tick(&mut t);
    }
    assert!(t.s.life.vit.removed_after_death(), "the host removed");
    assert!(!guest_told(&t, &g, cow).contains(&Told::Removed), "the cow stays on the guest's client");
    assert!(host_told(&t, cow).contains(&Told::Removed), "the host's client told the cow is gone");

    let stone = t.s.default_state("minecraft:stone");
    t.s.set_block([5, FLOOR + 2, 5], stone, 3);
    server_tick(&mut t);
    let block_update = ids(&t).block_update;
    assert!(guest_packets(&g).iter().any(|p| id_of(p) == block_update), "the block change reaches the guest");

    // The respawned host's client alone is told the items and orbs again: the guest still sees them.
    t.s.perform_respawn();
    server_tick(&mut t);
    let add_entity = ids(&t).add_entity;
    let added: Vec<i32> = guest_packets(&g).iter().filter(|p| id_of(p) == add_entity).map(|p| id_of(body(p))).collect();
    assert!(!added.contains(&orb) && !added.contains(&item), "nothing announced again to the guest: {added:?}");
}
