//! Choice of the movement packet at each tick, official game rule:
//! movement if the distance from the last send exceeds 2·10⁻⁴ (squared) or every 20 ticks,
//! rotation if the yaw or pitch has changed, otherwise state only if ground or collision change.

use crate::link::Move;

#[derive(Debug, Clone, Default)]
pub struct PositionSync {
    last_pos: [f64; 3],
    last_yaw: f32,
    last_pitch: f32,
    reminder: u32,
    last_on_ground: bool,
    last_collision: bool,
}

impl PositionSync {
    /// Packet to send for this tick (and update of the reference state).
    pub fn tick(&mut self, pos: [f64; 3], yaw: f32, pitch: f32, on_ground: bool, collision: bool) -> Option<Move> {
        let d = [pos[0] - self.last_pos[0], pos[1] - self.last_pos[1], pos[2] - self.last_pos[2]];
        self.reminder += 1;
        let moved = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 2.0e-4f64 * 2.0e-4 || self.reminder >= 20;
        let rotated =
            f64::from(yaw) - f64::from(self.last_yaw) != 0.0 || f64::from(pitch) - f64::from(self.last_pitch) != 0.0;
        let m = match (moved, rotated) {
            (true, true) => Some(Move::PosRot { pos, yaw, pitch }),
            (true, false) => Some(Move::Pos { pos }),
            (false, true) => Some(Move::Rot { yaw, pitch }),
            (false, false) => {
                (self.last_on_ground != on_ground || self.last_collision != collision).then_some(Move::Status)
            }
        };
        if moved {
            self.last_pos = pos;
            self.reminder = 0;
        }
        if rotated {
            self.last_yaw = yaw;
            self.last_pitch = pitch;
        }
        self.last_on_ground = on_ground;
        self.last_collision = collision;
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_rules() {
        let mut s = PositionSync { last_pos: [0.0, 64.0, 0.0], ..Default::default() };
        // Stationary: nothing for 19 ticks, position reminder at the 20th.
        for _ in 0..19 {
            assert_eq!(s.tick([0.0, 64.0, 0.0], 0.0, 0.0, false, false), None);
        }
        assert_eq!(s.tick([0.0, 64.0, 0.0], 0.0, 0.0, false, false), Some(Move::Pos { pos: [0.0, 64.0, 0.0] }));
        // Below the threshold: nothing; above: position.
        assert_eq!(s.tick([1.0e-4, 64.0, 0.0], 0.0, 0.0, false, false), None);
        assert!(matches!(s.tick([1.0e-3, 64.0, 0.0], 0.0, 0.0, false, false), Some(Move::Pos { .. })));
        // Rotation alone, then both.
        assert_eq!(s.tick([1.0e-3, 64.0, 0.0], 5.0, 0.0, false, false), Some(Move::Rot { yaw: 5.0, pitch: 0.0 }));
        assert!(matches!(s.tick([1.0, 64.0, 0.0], 6.0, 0.0, false, false), Some(Move::PosRot { .. })));
        // Ground contact change alone.
        assert_eq!(s.tick([1.0, 64.0, 0.0], 6.0, 0.0, true, false), Some(Move::Status));
        assert_eq!(s.tick([1.0, 64.0, 0.0], 6.0, 0.0, true, false), None);
    }
}
