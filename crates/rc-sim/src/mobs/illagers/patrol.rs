//! Patrol spawner: every 12,000 to 13,199 ticks, by day,
//! one chance in 5, far from a village, at 24-47 blocks from the player on each axis: a leader
//! (ominous banner, distant target) and ⌈effective difficulty⌉ more pillagers, offset by
//! ±4 blocks. Forbidden before 120,000 clock ticks (`early_game` timeline) and in
//! biomes that refuse it (mushroom).

use super::super::spawn::{Group, Reason};
use super::super::species::Kind;
use crate::server::Server;

impl Server {
    /// Spawner tick (world random), after that of the phantoms.
    pub(crate) fn patrol_spawner_tick(&mut self) {
        let spawn_enemies = self.level.difficulty_id() != 0 && self.rule("minecraft:spawn_monsters", true);
        if !spawn_enemies || !self.rule("minecraft:spawn_patrols", true) {
            return;
        }
        if self.dim.name != "minecraft:overworld" {
            return;
        }
        self.mobs.ill.patrol_next -= 1;
        if self.mobs.ill.patrol_next > 0 {
            return;
        }
        self.mobs.ill.patrol_next += 12000 + self.mobs.rng.next_int_bound(1200);
        if !self.bright_outside() {
            return;
        }
        if self.mobs.rng.next_int_bound(5) != 0 {
            return;
        }
        // Player drawn among those of the world (host, then guests entered): their position and mode.
        let players = self.world_players();
        let (mode, pos) = players[self.mobs.rng.next_int_bound(players.len() as i32) as usize];
        if mode == 3 {
            return;
        }
        let pp = super::super::mth::block_of(pos);
        if self.close_to_village(pp, 2) {
            return;
        }
        // Left operand first: the distance, then the sign.
        let ax = 24 + self.mobs.rng.next_int_bound(24);
        let x = ax * if self.mobs.rng.next_bool() { -1 } else { 1 };
        let az = 24 + self.mobs.rng.next_int_bound(24);
        let z = az * if self.mobs.rng.next_bool() { -1 } else { 1 };
        let mut p = [pp[0] + x, pp[1], pp[2] + z];
        if !self.chunks_loaded_box(p[0] - 10, p[2] - 10, p[0] + 10, p[2] + 10) {
            return;
        }
        if !self.can_patrol_spawn(p) {
            return;
        }
        let (eff, _) = self.local_difficulty(p);
        let size = eff.ceil() as i32 + 1;
        for i in 0..size {
            p[1] = self.surface_y(p[0], p[2], 5);
            if i == 0 {
                if !self.spawn_patrol_member(p, true) {
                    break;
                }
            } else {
                self.spawn_patrol_member(p, false);
            }
            p[0] += self.mobs.rng.next_int_bound(5) - self.mobs.rng.next_int_bound(5);
            p[2] += self.mobs.rng.next_int_bound(5) - self.mobs.rng.next_int_bound(5);
        }
    }

    /// Attribute `gameplay/can_pillager_patrol_spawn`: biome (false in mushroom) and
    /// `early_game` timeline of the Overworld (true from 120,000 clock ticks).
    fn can_patrol_spawn(&self, p: [i32; 3]) -> bool {
        let clock = self.clocks.iter().find(|(n, _)| n == "minecraft:overworld").map_or(self.time, |c| c.1);
        if clock < 120_000 {
            return false;
        }
        let biome = self.biome_str(p);
        self.life
            .pack
            .get("worldgen/biome", biome.trim_start_matches("minecraft:"))
            .and_then(|b| b["attributes"]["minecraft:gameplay/can_pillager_patrol_spawn"].as_bool())
            .unwrap_or(true)
    }

    /// All chunks of the rectangle loaded.
    pub(crate) fn chunks_loaded_box(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> bool {
        for cx in (x0 >> 4)..=(x1 >> 4) {
            for cz in (z0 >> 4)..=(z1 >> 4) {
                if !self.chunks.get(&(cx, cz)).is_some_and(Option::is_some) {
                    return false;
                }
            }
        }
        true
    }

    /// Free block, patroller rules (block light ≤ 8, monster rules
    /// without light), pillager created, leader designated, finalized.
    fn spawn_patrol_member(&mut self, p: [i32; 3], leader: bool) -> bool {
        if !self.valid_empty_spawn(p, Kind::Pillager) {
            return false;
        }
        if self.light(p).1 > 8 || !self.any_light_monster_rules(p) {
            return false;
        }
        let pos = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        let mut m = self.new_mob(Kind::Pillager, pos, 0.0);
        if leader {
            m.ill.patrol_leader = true;
            // The game picks the target BEFORE placing the mob, still at the origin (0, 0, 0):
            // the leader's distant target is within ±500 blocks of the world center, y = 0.
            let at = m.pos;
            m.pos = [0.0; 3];
            self.find_patrol_target(&mut m);
            m.pos = at;
        }
        let mut g = Group::default();
        self.finalize_spawn(&mut m, Reason::Patrol, &mut g);
        self.add_mob(m);
        true
    }

    /// Non-peaceful difficulty and valid block below.
    fn any_light_monster_rules(&self, p: [i32; 3]) -> bool {
        self.level.difficulty_id() != 0 && self.valid_spawn_on([p[0], p[1] - 1, p[2]], Kind::Pillager)
    }
}
