//! Dried ghast block (`server-mobs.md` § 4.F14): random tick that schedules a tick
//! in 5,000 (waterlogged, or still hydrated); waterlogged, it hydrates by one step (transition
//! sound) up to 3, then spawns a happy ghastling turned like the block; dry, it
//! loses a step.

use super::super::species::Kind;
use crate::server::Server;

pub fn is_dried_ghast(name: &str) -> bool {
    name == "minecraft:dried_ghast"
}

fn hydration(s: &Server, state: u32) -> i32 {
    s.data.state_properties(state).find(|(k, _)| *k == "hydration").and_then(|(_, v)| v.parse().ok()).unwrap_or(0)
}

fn waterlogged(s: &Server, state: u32) -> bool {
    s.data.state_properties(state).any(|(k, v)| k == "waterlogged" && v == "true")
}

/// Yaw of a horizontal direction.
fn yaw_of(facing: &str) -> f32 {
    match facing {
        "south" => 0.0,
        "west" => 90.0,
        "north" => 180.0,
        _ => 270.0,
    }
}

impl Server {
    /// Random tick: tick scheduled in 5,000 if there is none already.
    pub(crate) fn dried_ghast_random_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        if !is_dried_ghast(self.state_name(state)) {
            return false;
        }
        let block = self.v().block(state);
        if (waterlogged(self, state) || hydration(self, state) > 0) && !self.has_scheduled_tick(p, block) {
            self.schedule_tick(p, block, 5000);
        }
        true
    }

    /// Scheduled tick: hydration, dehydration or hatching.
    pub(crate) fn dried_ghast_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        if !is_dried_ghast(self.state_name(state)) {
            return false;
        }
        let h = hydration(self, state);
        if waterlogged(self, state) {
            if h < 3 {
                self.block_sound(p, "minecraft:block.dried_ghast.transition", 1.0, 1.0);
                if let Some(n) = self.with_prop(state, "hydration", &(h + 1).to_string()) {
                    self.set_block(p, n, 2);
                }
                self.game_event_block("minecraft:block_change", p, None, Some(state));
            } else {
                self.spawn_ghastling(p, state);
            }
        } else if h > 0 {
            if let Some(n) = self.with_prop(state, "hydration", &(h - 1).to_string()) {
                self.set_block(p, n, 2);
            }
            self.game_event_block("minecraft:block_change", p, None, Some(state));
        }
        true
    }

    /// Placement sound (in water if the block is placed waterlogged).
    pub(crate) fn dried_ghast_placed(&mut self, p: [i32; 3], state: u32) {
        if !is_dried_ghast(self.state_name(state)) {
            return;
        }
        let sound = if waterlogged(self, state) {
            "minecraft:block.dried_ghast.place_in_water"
        } else {
            "minecraft:block.dried_ghast.place"
        };
        self.block_sound(p, sound, 1.0, 1.0);
    }

    /// Block removed, ghastling at the bottom of the block turned like it, sound.
    fn spawn_ghastling(&mut self, p: [i32; 3], state: u32) {
        self.remove_block(p, false);
        let facing =
            self.data.state_properties(state).find(|(k, _)| *k == "facing").map_or("north", |(_, v)| v).to_owned();
        let yaw = yaw_of(&facing);
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let mut g = self.new_mob(Kind::HappyGhast, pos, yaw);
        g.age = -24000;
        g.head_yaw = yaw;
        g.body_yaw = yaw;
        g.yaw = yaw;
        self.mob_sound(&g, "minecraft:entity.ghastling.spawn", 1.0, 1.0);
        self.add_mob(g);
    }
}
