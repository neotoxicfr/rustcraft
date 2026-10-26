//! Chorus fruit: 16 attempts in a cube of `diameter`,
//! descent down to a block of the tag `entities_can_teleport_to`, free spot with no liquid,
//! no block of the tag `consumable_does_not_teleport_to`; sound and particles (2017, 46).

use rc_client::{EntityEvent, Event};

use crate::server::Server;

impl Server {
    /// Vertical bounds of the attempts: from the world bottom to the dimension's logical height.
    fn chorus_bounds(&self) -> (f64, f64) {
        let h = self.dim.height;
        let logical = self
            .dim_type_field(&self.dim.name, "logical_height")
            .and_then(|v| v.as_i64())
            .map_or(h.sections as i32 * 16, |v| v as i32);
        (f64::from(h.min_y), f64::from(h.min_y + logical - 1))
    }

    /// Directed particles (event 2017): offset bounded to ±127 per axis, 8 bits each.
    fn chorus_trail(&self, old: [f64; 3], new: [f64; 3]) {
        let o = old.map(|v| v.floor() as i32);
        let t = new.map(|v| v.floor() as i32);
        let d = |i: usize| (t[i] - o[i]).clamp(-127, 127);
        let packed = ((d(0) & 255) << 16) | ((d(1) & 255) << 8) | (d(2) & 255);
        self.level_event(2017, o, packed);
    }

    /// Random teleportation of the player; true if successful. `trail`: directed particles
    /// (`directional_particles`, true by default).
    pub(crate) fn chorus_teleport(&mut self, diameter: f64, trail: bool) -> bool {
        let (min_y, max_y) = self.chorus_bounds();
        for _ in 0..16 {
            let r = &mut self.life.rng;
            let xx = self.pos[0] + (r.next_double() - 0.5) * diameter;
            let yy = (self.pos[1] + (r.next_double() - 0.5) * diameter).clamp(min_y, max_y);
            let zz = self.pos[2] + (r.next_double() - 0.5) * diameter;
            // Passenger (mount, cushion, minecart): it dismounts first, even if no attempt
            // succeeds.
            self.player_stop_riding();
            self.player_stop_riding_cart();
            self.cushion_stand_up();
            let old = self.pos;
            if self.random_teleport(xx, yy, zz) {
                self.game_event_at("minecraft:teleport", old, Some(crate::mobs::mob::Ent::Player));
                self.play_sound("minecraft:item.chorus_fruit.teleport", 7, self.pos, 1.0, 1.0);
                if trail {
                    self.chorus_trail(old, self.pos);
                }
                self.life.vit.fall = 0.0;
                return true;
            }
        }
        false
    }

    /// Same teleportation for a mob that eats (fox): draws on its own random,
    /// fox sound (neutral category) or fruit (players), particles 46 and directed.
    pub(crate) fn chorus_teleport_mob(&mut self, m: &mut crate::mobs::mob::Mob, diameter: f64, trail: bool) -> bool {
        let (min_y, max_y) = self.chorus_bounds();
        for _ in 0..16 {
            let xx = m.pos[0] + (m.rng.next_double() - 0.5) * diameter;
            let yy = (m.pos[1] + (m.rng.next_double() - 0.5) * diameter).clamp(min_y, max_y);
            let zz = m.pos[2] + (m.rng.next_double() - 0.5) * diameter;
            let old = m.pos;
            if self.mob_random_teleport(m, [xx, yy, zz], "minecraft:consumable_does_not_teleport_to") {
                self.mob_event(m, 46);
                self.game_event_at("minecraft:teleport", old, Some(crate::mobs::mob::Ent::Mob(m.id)));
                let (snd, cat) = if m.kind == crate::mobs::species::Kind::Fox {
                    ("minecraft:entity.fox.teleport", 6)
                } else {
                    ("minecraft:item.chorus_fruit.teleport", 7)
                };
                self.play_sound(snd, cat, m.pos, 1.0, 1.0);
                if trail {
                    self.chorus_trail(old, m.pos);
                }
                m.fall_distance = 0.0;
                return true;
            }
        }
        false
    }

    /// The random teleportation of a mob for the player.
    fn random_teleport(&mut self, xx: f64, yy: f64, zz: f64) -> bool {
        let mut y = yy;
        let mut p = [xx.floor() as i32, yy.floor() as i32, zz.floor() as i32];
        let c = (p[0] >> 4, p[2] >> 4);
        if !matches!(self.chunks.get(&c), Some(Some(_))) {
            return false;
        }
        while p[1] > self.dim.height.min_y {
            p[1] -= 1;
            let s = self.st(p);
            if self.chorus_block_tag(s, "minecraft:entities_can_teleport_to") {
                if self.chorus_block_tag(s, "minecraft:consumable_does_not_teleport_to") {
                    return false;
                }
                let hgt = if self.sneaking { 1.5 } else { 1.8 };
                let b = rc_physics::Aabb { min: [xx - 0.3, y, zz - 0.3], max: [xx + 0.3, y + hgt, zz + 0.3] };
                if !rc_physics::collide::is_free(&self.world(), &b) || self.chorus_box_liquid(&b) {
                    return false;
                }
                if self.chorus_box_tag(&b, "minecraft:consumable_does_not_teleport_to") {
                    return false;
                }
                self.send_relative_teleport([xx, y, zz], 0.0, 0.0, crate::dims::rel::ROTATION);
                self.mobs.player_vel = [0.0; 3];
                let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: 46 }));
                return true;
            }
            y -= 1.0;
        }
        false
    }

    fn chorus_block_tag(&self, s: u32, tag: &str) -> bool {
        let short = tag.trim_start_matches("minecraft:");
        let block = self.data.block_of(s).map(|b| b.name.as_str()).unwrap_or("");
        let Some(bid) = self.data.blocks.iter().position(|b| b.name == block) else { return false };
        [tag, short].iter().any(|t| self.block_tags.get(*t).is_some_and(|v| v.binary_search(&(bid as u32)).is_ok()))
    }

    fn chorus_cells(b: &rc_physics::Aabb) -> impl Iterator<Item = [i32; 3]> {
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| (v - 1.0e-7).floor() as i32);
        (lo[0]..=hi[0]).flat_map(move |x| (lo[1]..=hi[1]).flat_map(move |y| (lo[2]..=hi[2]).map(move |z| [x, y, z])))
    }

    fn chorus_box_liquid(&self, b: &rc_physics::Aabb) -> bool {
        Self::chorus_cells(b).any(|p| self.phys.fluids.fluid(self.st(p)) != rc_physics::Fluid::None)
    }

    fn chorus_box_tag(&self, b: &rc_physics::Aabb, tag: &str) -> bool {
        Self::chorus_cells(b).any(|p| self.chorus_block_tag(self.st(p), tag))
    }
}
