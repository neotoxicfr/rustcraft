//! Spawning of aquatic hostiles: placement in water, rules of the drowned and the guardians
//! (world random source, in the game's order); the monument's lists go through `spawn_list_at`.

use super::super::env::Fluid;
use super::super::spawn::Reason;
use super::super::species::Kind;
use crate::server::Server;

impl Server {
    /// The monster spawn light test (world random source: sky, then light).
    pub(crate) fn dark_enough_to_spawn(&mut self, p: [i32; 3]) -> bool {
        let (sky, block) = self.light(p);
        if i32::from(sky) > self.mobs.rng.next_int_bound(32) {
            return false;
        }
        if i32::from(block) > 0 {
            return false;
        }
        let raw = if self.world_thundering() {
            (i32::from(sky) - 10).max(0).max(i32::from(block))
        } else {
            self.mob_brightness(p)
        };
        let limit = self.mobs.rng.next_int_bound(8);
        raw <= limit
    }

    /// The visible-sky-under-water test.
    pub(crate) fn sky_from_below_water(&self, p: [i32; 3]) -> bool {
        let sea = super::super::surface::sea_level(self);
        if p[1] >= sea {
            return self.can_see_sky(p);
        }
        let scan = [p[0], sea, p[2]];
        if !self.can_see_sky(scan) {
            return false;
        }
        let mut y = sea - 1;
        while y > p[1] {
            let q = [p[0], y, p[2]];
            let dampens = self.solid_render(q) || self.data.state(self.st(q)).is_some_and(|i| i.light_dampening > 0);
            let liquid = matches!(self.bname(q), "minecraft:water" | "minecraft:lava" | "minecraft:bubble_column");
            if dampens && !liquid {
                return false;
            }
            y -= 1;
        }
        true
    }

    /// "In water" placement: water at the position, block above non-conductive.
    pub(crate) fn in_water_placement_ok(&self, p: [i32; 3]) -> bool {
        let above = [p[0], p[1] + 1, p[2]];
        self.mob_fluid(p) == Fluid::Water && !self.v().conductor(self.st(above))
    }

    /// Natural spawn rules of the family; `None` for the other species.
    pub(crate) fn aquatic_spawn_rules(&mut self, kind: Kind, p: [i32; 3]) -> Option<bool> {
        if !super::handles(kind) {
            return None;
        }
        // Monsters: never in peaceful.
        if self.level.difficulty_id() == 0 {
            return Some(false);
        }
        let below = [p[0], p[1] - 1, p[2]];
        Some(match kind {
            Kind::Drowned => {
                // A spawner is exempt from water (under the position and
                // at the position); the trial spawner is also exempt from light.
                let spawner = self.rule_is_spawner();
                if self.mob_fluid(below) != Fluid::Water && !spawner {
                    return Some(false);
                }
                let trial = self.mobs.spawners.rule_reason == Some(Reason::TrialSpawner);
                let can = (trial || self.dark_enough_to_spawn(p)) && (spawner || self.mob_fluid(p) == Fluid::Water);
                // Spawner or reinforcement: neither draw nor depth.
                if can && (spawner || self.mobs.spawners.rule_reason == Some(Reason::Reinforcement)) {
                    true
                } else if self.biome_tag(p, "minecraft:more_frequent_drowned_spawns") {
                    self.mobs.rng.next_int_bound(15) == 0 && can
                } else {
                    let sea = super::super::surface::sea_level(self);
                    self.mobs.rng.next_int_bound(40) == 0 && p[1] < sea - 5 && can
                }
            }
            _ => {
                (self.mobs.rng.next_int_bound(20) == 0 || !self.sky_from_below_water(p))
                    && (self.rule_is_spawner() || self.mob_fluid(p) == Fluid::Water)
                    && self.mob_fluid(below) == Fluid::Water
            }
        })
    }
}
