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

use super::tests_g3f05::{G, T, Told, drain, guest_told, join, open, publish};
use crate::mobs::PlayerPose;

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
    assert!(t.s.hurt("minecraft:generic_kill", 1000.0));
    t.s.vitals_tick();
    t.s.lan_tick();
    let _ = guest_told(&t, &g, host);
    t.s.perform_respawn();
    t.s.lan_tick();
    let told = guest_told(&t, &g, host);
    assert_eq!(told[..2], [Told::Removed, Told::Added], "{told:?}");
    assert_eq!(last_value(&told, 9), Some(Meta::Float(20.0)));
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
