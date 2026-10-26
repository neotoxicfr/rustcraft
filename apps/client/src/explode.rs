//! `explode` packet (`protocol-supplements.md` § 3): sound, central particle, block
//! particles on the next tick (512 at most), knockback of the local player.

use rc_client::extra::Explosion;
use rc_proto_v777::entity::{LevelParticles, Particle};
use rc_proto_v777::play::SoundPacket;

use crate::App;

/// Block particles waiting for the next world tick.
#[derive(Default)]
pub(crate) struct Pending {
    /// (center, radius, destroyed blocks, particles (particle, scale, speed, weight)).
    list: Vec<([f64; 3], f32, i32, BlockParticles)>,
}

/// Block particles of an explosion: (particle, scale, speed, weight).
type BlockParticles = Vec<(Particle, f32, f32, i32)>;

/// "Blocks" category of the sounds.
const BLOCKS: i32 = 4;

/// Weighted draw: index whose cumulative weight exceeds `r × total`.
fn weighted(weights: impl Iterator<Item = i32> + Clone, r: f32) -> usize {
    let total: i64 = weights.clone().map(|w| i64::from(w.max(0))).sum();
    let mut x = (f64::from(r) * total as f64) as i64;
    for (i, w) in weights.enumerate() {
        x -= i64::from(w.max(0));
        if x < 0 {
            return i;
        }
    }
    0
}

impl App {
    pub(crate) fn on_explode(&mut self, x: Explosion) {
        // 1. Sound at the center, volume 4, pitch (1 + (r1 − r2) × 0.2) × 0.7, no delay.
        if x.play_sound {
            let (r1, r2) = (self.world_random.f(), self.world_random.f());
            let seed = (f64::from(self.world_random.f()) * 2f64.powi(53)) as i64;
            self.play_sound(SoundPacket {
                sound: x.sound.clone(),
                category: BLOCKS,
                pos: Some(x.center),
                entity: None,
                volume: 4.0,
                pitch: (1.0 + (r1 - r2) * 0.2) * 0.7,
                seed,
            });
        }
        // 2. Particle at the center, velocity (1, 0, 0).
        self.level_particles(&LevelParticles {
            particle: x.particle.clone(),
            force: false,
            always_visible: false,
            pos: x.center,
            delta: [1.0, 0.0, 0.0],
            speed: [1.0; 3],
            count: 0,
            mode: 0,
        });
        // 3. Block particles on the next tick.
        if !x.block_particles.is_empty() {
            self.exact.explosions.list.push((x.center, x.radius, x.blocks, x.block_particles));
        }
        // 4. Knockback added to the velocity of the local player.
        if let Some(k) = x.knockback
            && self.player.push(k)
            && rc_data::env_flag!("RUSTCRAFT_PROTOLOG")
        {
            eprintln!("explosion: knockback {k:?}, velocity {:?}", self.player.vel);
        }
    }

    /// World tick: block particles of the explosions of the previous tick.
    pub(crate) fn explosion_particles(&mut self) {
        let list = std::mem::take(&mut self.exact.explosions.list);
        // Only with the particles option "All" (0).
        if list.is_empty() || self.options.i("particles") != 0 {
            return;
        }
        let total: i64 = list.iter().map(|e| i64::from(e.2.max(0))).sum();
        let n = total.min(512);
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let tables = self.tables.clone();
        let mut spawns = Vec::new();
        for _ in 0..n {
            let r = &mut self.world_random;
            let (center, radius, _, parts) = &list[weighted(list.iter().map(|e| e.2), r.f())];
            let mut d = [0.0f64; 3].map(|_| f64::from(r.f()) * 2.0 - 1.0);
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len < 1e-4 {
                continue;
            }
            d = d.map(|v| v / len);
            let radius = f64::from(*radius);
            let dist = f64::from(r.f()).cbrt() * radius;
            let p: [f64; 3] = std::array::from_fn(|i| center[i] + d[i] * dist);
            let s = view.state(p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
            let air = tables.air.get(s as usize).copied().unwrap_or(true);
            if !air {
                continue;
            }
            let speed = 0.5 / (dist / radius + 0.1) * f64::from(r.f()) * f64::from(r.f()) + 0.3;
            let (particle, scale, entry_speed, _) = &parts[weighted(parts.iter().map(|e| e.3), r.f())];
            let at: [f64; 3] = std::array::from_fn(|i| center[i] + d[i] * dist * f64::from(*scale));
            let vel = d.map(|v| v * speed * f64::from(*entry_speed));
            spawns.push((particle.clone(), at, vel));
        }
        for (particle, at, vel) in spawns {
            self.level_particles(&LevelParticles {
                particle,
                force: false,
                always_visible: false,
                pos: at,
                delta: vel.map(|v| v as f32),
                speed: [1.0; 3],
                count: 0,
                mode: 0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::weighted;

    #[test]
    fn weights() {
        let w = [1, 0, 3];
        assert_eq!(weighted(w.iter().copied(), 0.0), 0);
        assert_eq!(weighted(w.iter().copied(), 0.3), 2);
        assert_eq!(weighted(w.iter().copied(), 0.99), 2);
    }
}
