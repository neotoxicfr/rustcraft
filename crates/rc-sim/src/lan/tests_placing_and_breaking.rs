//! What the other LAN players see of a block placed or broken by one of them. The break effect
//! (pieces and sound, world event 2001) is shown by the breaker's own client, so the server sends
//! it to the other players only.

use rc_client::Event;
use rc_proto::Decode;

use super::tests_g3f05::{G, T, body, drain, feet, guest_packets, guest_ready, host_ready, id_of, ids, join, open, publish};

const FLOOR: i32 = 199;

/// Host 5 blocks from a guest, both in survival, block `block` set at each cell given, nothing
/// pending.
fn host_beside_guest(tag: &str, blocks: &[([i32; 3], &str)]) -> (T, G) {
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

/// Break effects (position, broken state) among packets `ps` received by a guest.
fn break_effects_in(t: &T, ps: &[Vec<u8>]) -> Vec<([i32; 3], i32)> {
    let level_event = ids(t).level_event;
    ps.iter()
        .filter(|p| id_of(p) == level_event)
        .filter_map(|p| {
            let mut r = rc_proto::Reader::new(body(p));
            let kind = i32::decode(&mut r).ok()?;
            let pos = rc_proto::BlockPos::decode(&mut r).ok()?;
            let data = i32::decode(&mut r).ok()?;
            (kind == 2001).then_some(([pos.x, pos.y, pos.z], data))
        })
        .collect()
}

/// Break effects (position, broken state) the host received.
fn host_break_effects(t: &T) -> Vec<([i32; 3], i32)> {
    drain(t)
        .into_iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind: 2001, pos, data, .. } => Some((pos, data)),
            _ => None,
        })
        .collect()
}

/// The host breaks stone next to a guest: the guest sees its pieces (the stone's state), the host's
/// own client shows them itself and receives nothing. Before, nobody else saw the break.
#[test]
fn a_block_the_host_breaks_shows_its_pieces_to_the_guest_only() {
    let stone = [1, FLOOR + 1, 0];
    let (mut t, g) = host_beside_guest("break-host", &[(stone, "minecraft:stone")]);
    let state = t.s.get(stone) as i32;
    assert!(t.s.player_destroy_block(stone), "stone broken");
    assert_eq!(break_effects_in(&t, &guest_packets(&g)), [(stone, state)], "the guest sees the break");
    assert!(host_break_effects(&t).is_empty(), "sent back to the breaker");
}

/// Same when the guest breaks: the host sees the pieces, the guest receives nothing.
#[test]
fn a_block_a_guest_breaks_shows_its_pieces_to_the_host_only() {
    let stone = [-1, FLOOR + 1, 2];
    let (mut t, g) = host_beside_guest("break-guest", &[(stone, "minecraft:stone")]);
    let state = t.s.get(stone) as i32;
    assert!(t.s.as_guest(0, |sv| sv.player_destroy_block(stone)), "stone broken by the guest");
    assert_eq!(host_break_effects(&t), [(stone, state)], "the host sees the break");
    assert!(break_effects_in(&t, &guest_packets(&g)).is_empty(), "sent back to the breaker");
}

/// Fire put out by hand has no break effect: only its extinguish sound (world event 1009).
#[test]
fn fire_put_out_by_hand_shows_no_pieces() {
    let fire = [1, FLOOR + 1, 1];
    let (mut t, g) = host_beside_guest("break-fire", &[(fire, "minecraft:fire")]);
    assert!(t.s.player_destroy_block(fire), "fire put out");
    assert!(break_effects_in(&t, &guest_packets(&g)).is_empty(), "no pieces for fire");
}

/// In creative, the upper half of a door broken takes the lower one without loot. Like the game:
/// the lower half's pieces and the breaker's own go to the others only, while the upper half,
/// already undone by its lower half's removal, shows its pieces to everyone. So the guest sees the
/// upper half twice and the lower once, the breaker the upper half once. Before, both halves'
/// pieces went to everyone and the breaker's own to nobody.
#[test]
fn both_halves_of_a_door_broken_in_creative_show_their_pieces_like_the_game() {
    let lower = [1, FLOOR + 1, 1];
    let upper = [1, FLOOR + 2, 1];
    let (mut t, g) = host_beside_guest("break-door", &[]);
    let door = t.s.default_state("minecraft:oak_door");
    let door_upper = t.s.view().set(door, rc_data::PropKey::Half, "upper");
    t.s.set_block(lower, door, 2);
    t.s.set_block(upper, door_upper, 2);
    let _ = guest_packets(&g);
    drain(&t);
    t.s.game_mode = 1;
    assert!(t.s.player_destroy_block(upper), "door broken");
    let mut seen = break_effects_in(&t, &guest_packets(&g));
    seen.sort_unstable();
    let (lower_effect, upper_effect) = ((lower, door as i32), (upper, door_upper as i32));
    assert_eq!(seen, [lower_effect, upper_effect, upper_effect], "what the guest sees");
    assert_eq!(host_break_effects(&t), [upper_effect], "what the breaker receives");
}
