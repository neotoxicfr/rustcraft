//! Scoreboard criteria kept by the server: read-only player values
//! (`health`, `food`, `air`, `armor`, `xp`, `level`) and cleanup of removed entities.

use crate::server::Server;

/// Last values written into the read-only objectives. The game restarts from the sentinels
/// at each new player (login, respawn): everything is then rewritten at the next tick.
/// An objective created in the meantime stays empty as long as the value does not change (like the game).
#[derive(Debug, Clone, Copy)]
pub struct CriteriaWatch {
    health: f32,
    food: i32,
    air: i32,
    armor: i32,
    xp: i32,
    level: i32,
}

impl Default for CriteriaWatch {
    fn default() -> CriteriaWatch {
        // Smallest positive float and minimum integer: never equal to a real value.
        CriteriaWatch {
            health: f32::from_bits(1),
            food: i32::MIN,
            air: i32::MIN,
            armor: i32::MIN,
            xp: i32::MIN,
            level: i32::MIN,
        }
    }
}

impl Server {
    /// Player tick: each changed value is written into the objectives of its criterion
    /// (health + absorption rounded up), in the game's order.
    pub(crate) fn criteria_tick(&mut self) {
        let v = &self.life.vit;
        let now = CriteriaWatch {
            health: v.health + self.life.fx.absorption,
            food: v.food,
            air: v.air,
            armor: self.armor_value(),
            xp: v.xp_total,
            level: v.xp_level,
        };
        let last = std::mem::replace(&mut self.life.vit.crit, now);
        // Comparisons only as long as no objective exists (common case).
        if self.cmd.scores.objectives.is_empty() {
            return;
        }
        let changes = [
            (now.health != last.health, "health", now.health.ceil() as i32),
            (now.food != last.food, "food", now.food),
            (now.air != last.air, "air", now.air),
            (now.armor != last.armor, "armor", now.armor),
            (now.xp != last.xp, "xp", now.xp),
            (now.level != last.level, "level", now.level),
        ];
        for (changed, criteria, value) in changes {
            if changed && self.cmd.scores.objectives.iter().any(|o| o.criteria == criteria) {
                let holder = self.name.clone();
                self.criteria_update(criteria, &holder, |_| value);
            }
        }
    }

    /// Non-player entity permanently removed (dead or deleted, not unloaded): its scores
    /// and team disappear.
    pub(crate) fn entity_scores_removed(&mut self, uuid: u128) {
        if self.cmd.scores.has_entries() {
            self.cmd.scores.entity_removed(&crate::nbtio::uuid_text(uuid));
        }
    }
}
