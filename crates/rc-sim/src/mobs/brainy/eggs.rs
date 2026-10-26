//! Blocks that make the family spawn: frogspawn (2 to 5 persistent tadpoles after 3,600 to
//! 11,999 ticks) and sniffer egg (three stages, 24,000 ticks or 12,000 on a block of the
//! `sniffer_egg_hatch_boost` tag, baby sniffer). Block random draws.

use super::super::species::Kind;
use crate::server::Server;

impl Server {
    /// Scheduled hatch tick.
    pub(crate) fn bz_egg_on_place(&mut self, state: u32, p: [i32; 3]) {
        match self.state_name(state) {
            "minecraft:frogspawn" => {
                let delay = 3600 + self.bs.rng.next_int_bound(12000 - 3600);
                let b = self.v().block(state);
                self.schedule_tick(p, b, delay);
            }
            "minecraft:sniffer_egg" => {
                let boosted = self.btag_id([p[0], p[1] - 1, p[2]], crate::tag_bits::TagId::SnifferEggHatchBoost);
                if boosted {
                    self.level_event(3009, p, 0);
                }
                let hatch = if boosted { 12000 } else { 24000 };
                // Game event "block placed" (no source, egg state), before the random draw.
                let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
                self.game_event_src("minecraft:block_place", c, None, Some(state));
                let delay = hatch / 3 + self.bs.rng.next_int_bound(300);
                let b = self.v().block(state);
                self.schedule_tick(p, b, delay);
            }
            _ => {}
        }
    }

    /// Scheduled tick; true if it was one of these blocks.
    pub(crate) fn bz_egg_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        let name = self.state_name(state).to_owned();
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        match name.as_str() {
            "minecraft:frogspawn" => {
                let below = [p[0], p[1] - 1, p[2]];
                let water = self.mob_source_water(below) || self.btag_id(below, crate::tag_bits::TagId::SupportsFrogspawn);
                let survives = water && self.mob_fluid(p) == super::super::env::Fluid::None;
                self.destroy_block(p, false, 512);
                if survives {
                    self.world_sound("minecraft:block.frogspawn.hatch", 4, c, 1.0, 1.0);
                    let n = 2 + self.bs.rng.next_int_bound(4);
                    for _ in 0..n {
                        let off =
                            |s: &mut Server| s.bs.rng.next_double().clamp(f64::from(0.2f32), 0.799_999_997_019_767_8);
                        let x = f64::from(p[0]) + off(self);
                        let z = f64::from(p[2]) + off(self);
                        let yaw = (1 + self.bs.rng.next_int_bound(360)) as f32;
                        let mut t =
                            self.new_mob(Kind::Tadpole, [x, f64::from(p[1]) - 0.5, z], super::super::mth::wrap(yaw));
                        t.persistent = true;
                        self.add_mob(t);
                    }
                }
                true
            }
            "minecraft:sniffer_egg" => {
                let hatch = self.v().get_int(state, "hatch");
                let pitch = 0.9 + self.bs.rng.next_float() * 0.2;
                if hatch < 2 {
                    self.world_sound("minecraft:block.sniffer_egg.crack", 4, c, 0.7, pitch);
                    let ns = self.v().set_int(state, "hatch", hatch + 1);
                    self.set_block(p, ns, 2);
                } else {
                    self.world_sound("minecraft:block.sniffer_egg.hatch", 4, c, 0.7, pitch);
                    self.destroy_block(p, false, 512);
                    let yaw = super::super::mth::wrap(self.bs.rng.next_float() * 360.0);
                    let mut s = self.new_mob(Kind::Sniffer, [c[0], c[1], c[2]], yaw);
                    s.age = super::baby_start_age(Kind::Sniffer);
                    self.add_mob(s);
                }
                true
            }
            _ => false,
        }
    }
}
