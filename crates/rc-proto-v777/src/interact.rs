//! Packets for interactions with the world (`docs/specs/interactions.md` § 0): mining, use,
//! attack, block pick, and server responses (sequence acknowledgement, cracks).

use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{BlockPos, Decode, Encode, Reader, Result};

/// `player_action`: action, block, face (0 down … 5 east, byte), sequence.
pub fn write_player_action(w: &mut Vec<u8>, action: i32, pos: [i32; 3], face: u8, seq: i32) {
    write_varint(w, action);
    BlockPos { x: pos[0], y: pos[1], z: pos[2] }.encode(w);
    w.push(face);
    write_varint(w, seq);
}

/// `use_item`: hand, sequence, yaw, pitch.
pub fn write_use_item(w: &mut Vec<u8>, hand: i32, seq: i32, yaw: f32, pitch: f32) {
    write_varint(w, hand);
    write_varint(w, seq);
    yaw.encode(w);
    pitch.encode(w);
}

/// `interact`: entity, hand, targeted point relative to the feet (compact vector), sneaking.
pub fn write_interact(w: &mut Vec<u8>, entity: i32, hand: i32, at: [f64; 3], sneaking: bool) {
    write_varint(w, entity);
    write_varint(w, hand);
    write_lp_vec3(w, at);
    sneaking.encode(w);
}

/// `spectator_action`: optional entity, coded `id + 1` (0 = none).
pub fn write_spectator_action(w: &mut Vec<u8>, entity: Option<i32>) {
    write_varint(w, entity.map_or(0, |e| e + 1));
}

/// `pick_item_from_block`: block, with data.
pub fn write_pick_block(w: &mut Vec<u8>, pos: [i32; 3], with_data: bool) {
    BlockPos { x: pos[0], y: pos[1], z: pos[2] }.encode(w);
    with_data.encode(w);
}

/// `pick_item_from_entity`: entity, with data.
pub fn write_pick_entity(w: &mut Vec<u8>, entity: i32, with_data: bool) {
    write_varint(w, entity);
    with_data.encode(w);
}

/// Compact vector (`entities.md` § 1.0.1): scale = largest absolute component rounded
/// up; three 15-bit components in `[-1, 1]`; scale beyond 3 as a varint.
pub fn write_lp_vec3(w: &mut Vec<u8>, v: [f64; 3]) {
    const LIMIT: f64 = 1.717_986_918_3e10;
    let s = v.map(|c| if c.is_nan() { 0.0 } else { c.clamp(-LIMIT, LIMIT) });
    let longest = s.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if longest < 3.051_944_088_384_301e-5 {
        w.push(0);
        return;
    }
    let scale = longest.ceil() as u64;
    let partial = scale & 3 != scale;
    let markers = if partial { (scale & 3) | 4 } else { scale };
    let pack = |c: f64| ((c / scale as f64 * 0.5 + 0.5) * 32766.0).round() as u64;
    let bits = markers | pack(s[0]) << 3 | pack(s[1]) << 18 | pack(s[2]) << 33;
    w.push(bits as u8);
    w.push((bits >> 8) as u8);
    w.extend_from_slice(&((bits >> 16) as u32).to_be_bytes());
    if partial {
        write_varint(w, (scale >> 2) as i32);
    }
}

/// `block_destruction`: miner (entity id), block, stage (0..9, other = remove).
pub fn read_block_destruction(r: &mut Reader<'_>) -> Result<(i32, [i32; 3], u8)> {
    let id = read_varint(r)?;
    let p = BlockPos::decode(r)?;
    Ok((id, [p.x, p.y, p.z], u8::decode(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lp_vec3_round_trip() {
        for v in [[0.0, 0.0, 0.0], [0.25, -0.5, 0.9], [1.3, 0.2, -0.7], [5.5, -2.0, 0.1]] {
            let mut w = Vec::new();
            write_lp_vec3(&mut w, v);
            let back = crate::entity::read_lp_vec3(&mut Reader::new(&w)).unwrap();
            for a in 0..3 {
                assert!((back[a] - v[a]).abs() < 1e-3, "{v:?} → {back:?}");
            }
        }
    }

    #[test]
    fn player_action_layout() {
        let mut w = Vec::new();
        write_player_action(&mut w, 0, [1, -60, 2], 1, 7);
        assert_eq!(w[0], 0);
        assert_eq!(w.len(), 1 + 8 + 1 + 1);
        assert_eq!(w[9], 1);
        assert_eq!(w[10], 7);
    }
}
