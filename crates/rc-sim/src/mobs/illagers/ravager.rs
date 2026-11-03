//! Ravager: base speed sliding toward 0.3 (0.35 with a target), foliage torn off on
//! collision (rule `mob_griefing`) otherwise jump, hit (10 ticks), stun
//! (40 ticks) then roar (damage 6 and strong push at 4 blocks). Crops it moves through
//! are broken (`mob_griefing`).

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::server::Server;

impl Server {
    /// End of the ravager's AI step (after the rest of the AI).
    pub(crate) fn ravager_ai_step(&mut self, m: &mut Mob) {
        // Effects of the blocks moved through (end of the move, before the ravager's own logic).
        self.ravager_trample_crops(m);
        if super::immobile(m) {
            m.attrs.speed.base = 0.0;
        } else {
            let max = if m.target.is_some() { 0.35 } else { 0.3 };
            let base = m.attrs.speed.base;
            m.attrs.speed.base = base + 0.1 * (max - base);
        }
        if m.horizontal_collision && self.rule("minecraft:mob_griefing", true) {
            let b = m.bbox();
            let (lo, hi) = (b.min.map(|v| (v - 0.2).floor() as i32), b.max.map(|v| (v + 0.2).floor() as i32));
            let mut destroyed = false;
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        let p = [x, y, z];
                        if self.btag_id(p, crate::tag_bits::TagId::Leaves) {
                            destroyed = self.mob_destroy_block(p, true, m.id) || destroyed;
                        }
                    }
                }
            }
            if !destroyed && m.on_ground {
                self.jump_from_ground(m);
            }
        }
        if m.ill.roar_tick > 0 {
            m.ill.roar_tick -= 1;
            if m.ill.roar_tick == 10 {
                self.ravager_roar(m);
            }
        }
        if m.ill.attack_tick > 0 {
            m.ill.attack_tick -= 1;
        }
        if m.ill.stun_tick > 0 {
            m.ill.stun_tick -= 1;
            // Stun particles (client side): one draw per tick.
            if m.rng.next_int_bound(6) == 0 {
                let _ = m.rng.next_double();
                let _ = m.rng.next_double();
            }
            if m.ill.stun_tick == 0 {
                self.mob_sound(m, "minecraft:entity.ravager.roar", 1.0, 1.0);
                m.ill.roar_tick = 20;
            }
        }
    }

    /// Crops (wheat, carrots, potatoes, beetroots, and growing torchflower and pitcher plants)
    /// inside the ravager's box shrunk by 1e-5: broken with drops (`mob_griefing`).
    /// Order for a stationary box, as in the official game: x, then y, then z. Stems (melon,
    /// pumpkin) are not crops for this effect.
    pub(crate) fn ravager_trample_crops(&mut self, m: &Mob) {
        if !m.alive() || !self.rule("minecraft:mob_griefing", true) {
            return;
        }
        let b = m.bbox();
        // Shrink in single precision, as the game does (1e-5 as a widened f32).
        let d = f64::from(1e-5f32);
        let (lo, hi) = (b.min.map(|v| (v + d).floor() as i32), b.max.map(|v| (v - d).floor() as i32));
        // Common case: no crop. Per-column pre-scan (one chunk lookup per column instead of one
        // per block); `RUSTCRAFT_OPT_RAVAGER_SCAN=0` keeps the plain pass only.
        if rc_data::env_str!("RUSTCRAFT_OPT_RAVAGER_SCAN") != Some("0") && !self.any_crop_in(lo, hi) {
            return;
        }
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let p = [x, y, z];
                    let st = self.st(p);
                    if st != 0 && is_crop(self.state_name(st)) {
                        self.mob_destroy_block(p, true, m.id);
                    }
                }
            }
        }
    }

    /// Is there a crop in the block range `lo..=hi`? (chunk read once per column).
    fn any_crop_in(&self, lo: [i32; 3], hi: [i32; 3]) -> bool {
        let min_y = self.dim.height.min_y;
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                let Some(c) = self.chunks.get(&(x >> 4, z >> 4)).and_then(Option::as_ref) else { continue };
                for y in lo[1]..=hi[1] {
                    let ly = y - min_y;
                    let Some(s) = usize::try_from(ly >> 4).ok().and_then(|i| c.sections.get(i)) else { continue };
                    let st = s.blocks.get(rc_world::block_index((x & 15) as usize, (ly & 15) as usize, (z & 15) as usize));
                    if st != 0 && is_crop(self.state_name(st)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// `roar`: mobs within 4 blocks (except ravagers; without `mob_griefing` or armor stands),
    /// 6 damage except illagers, push except player; event 69.
    fn ravager_roar(&mut self, m: &mut Mob) {
        if !m.alive() {
            return;
        }
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 4.0), max: b.max.map(|v| v + 4.0) };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .filter(|o| o.alive() && o.kind != Kind::Ravager && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let mut o = self.mobs.list.take(i).expect("mob");
            if !super::is_illager(o.kind) {
                self.mob_hurt_by(&mut o, m, "minecraft:mob_attack", 6.0);
            }
            strong_knockback(m.pos, &mut o);
            self.mobs.list.put(i, o);
        }
        if self.game_mode != 3 && self.ent_alive(Ent::Player) && self.player_aabb().intersects(&area) {
            self.player_hurt_by_mob(
                m,
                "minecraft:mob_attack",
                6.0,
                Some((m.pos[0] - self.pos[0], m.pos[2] - self.pos[2])),
            );
        }
        self.mob_event(m, 69);
    }
}

impl Server {
    /// Destruction of a block by an entity by a mob: block loot (empty tool),
    /// particles and sound (event 2001), block removed.
    pub(crate) fn mob_destroy_block(&mut self, p: [i32; 3], drop: bool, by: i32) -> bool {
        // The loot is handled by the common destruction (`destroy_block_by`).
        self.destroy_block_by(p, drop, 512, Some(crate::mobs::recent::vibration::Src::Living(Ent::Mob(by))))
    }
}

/// Blocks the ravager tramples when it moves through them.
fn is_crop(name: &str) -> bool {
    matches!(
        name,
        "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
            | "minecraft:torchflower_crop"
            | "minecraft:pitcher_crop"
    )
}

/// Horizontal push 4 / d², vertical 0.2.
fn strong_knockback(from: [f64; 3], o: &mut Mob) {
    let xd = o.pos[0] - from[0];
    let zd = o.pos[2] - from[2];
    let dd = (xd * xd + zd * zd).max(0.001);
    o.vel = [o.vel[0] + xd / dd * 4.0, o.vel[1] + 0.2, o.vel[2] + zd / dd * 4.0];
    o.needs_sync = true;
}
