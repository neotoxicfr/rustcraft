//! Summoning of the wither at the block (wither skeleton skull placed by the player or a
//! dispenser): T pattern of soul sand or soul soil topped with three skulls, searched in
//! the 24 orientations as for the golems (`golems/build.rs`). The pattern is erased, the wither
//! placed at the foot of the T (+0.55) turned along the pattern's axis, in the summoning phase (220 ticks, one
//! third of its HP), "summoned entity" advancement for players within 50 blocks.

use super::super::species::Kind;
use super::INVULNERABLE_TICKS;
use crate::server::Server;

/// Wither skeleton skull (floor or wall).
fn is_wither_skull(name: &str) -> bool {
    name == "minecraft:wither_skeleton_skull" || name == "minecraft:wither_skeleton_wall_skull"
}

impl Server {
    /// Skull placed at `p` (player: the reaction to placement, dispenser).
    pub(crate) fn wither_check_spawn(&mut self, p: [i32; 3]) {
        let st = self.st(p);
        if !is_wither_skull(self.state_name(st)) || p[1] < self.dim.height.min_y || self.level.difficulty_id() == 0 {
            return;
        }
        let Some(mt) = self.wither_pattern(p, true) else { return };
        // Construction only, without finalization.
        let at = mt.block(1, 2);
        let yaw = if mt.forward_on_x() { 0.0 } else { 90.0 };
        let pos = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 0.55, f64::from(at[2]) + 0.5];
        let mut w = self.new_mob(Kind::Wither, pos, yaw);
        self.clear_pattern_blocks(&mt);
        // Position, rotation, old values; body turned the same, head at 0.
        w.yaw = yaw;
        w.pitch = 0.0;
        w.body_yaw = yaw;
        w.head_yaw = 0.0;
        w.head_yaw_o = 0.0;
        // Switch to invulnerable.
        w.wither.inv = INVULNERABLE_TICKS;
        w.wither.bar_progress = 0.0;
        w.health = w.max_health() / 3.0;
        let b = w.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 50.0), max: b.max.map(|v| v + 50.0) };
        if self.game_mode != 3 && !self.life.vit.dead && self.player_aabb().intersects(&area) {
            self.trig_summoned(crate::progress::pred::Who::Mob(&w));
        }
        self.add_mob(w);
        self.update_pattern_blocks(&mt);
    }

    /// The wither spawn test (dispenser): two blocks above the bottom of the world, outside
    /// peaceful, base pattern completed by the skull.
    pub(crate) fn wither_can_spawn_mob(&self, p: [i32; 3]) -> bool {
        p[1] >= self.dim.height.min_y + 2 && self.level.difficulty_id() != 0 && self.wither_pattern(p, false).is_some()
    }

    /// Dispenser: skull placed turned like the facade (22.5° segments; vertical: 0),
    /// then the spawn check.
    pub(crate) fn wither_dispense_skull(&mut self, front: [i32; 3], step: [i32; 3]) {
        // South 0, west 1, north 2, east 3; × 4 segments.
        let seg = match step {
            [0, 0, 1] => 0,
            [-1, 0, 0] => 4,
            [0, 0, -1] => 8,
            [1, 0, 0] => 12,
            _ => 0,
        };
        let def = self.default_state("minecraft:wither_skeleton_skull");
        let st = self.v().set(def, "rotation", &seg.to_string());
        self.set_update(front, st);
        self.game_event_block("minecraft:block_place", front, None, None);
        self.wither_check_spawn(front);
    }
}
