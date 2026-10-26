//! Swim sounds and splashes of all mobs (common engine, brought in with F08).
//!
//! Swimming: when the distance traveled crosses the next step without a "ground" step having
//! taken place (in the air above a non-empty block, or while swimming), in water. Splash: on
//! entering the water, except on the entity's very first tick. The splash particles
//! are not sent by the server, but their draws on the entity's random source are reproduced.

use super::super::mob::Mob;
use super::super::species::Kind;
use crate::server::Server;

/// The game's "monster" classes (hostile swim and splash sounds). Slimes, phantoms and
/// golems are not part of them.
fn hostile(k: Kind) -> bool {
    matches!(
        k,
        Kind::Zombie
            | Kind::Skeleton
            | Kind::Creeper
            | Kind::Spider
            | Kind::Enderman
            | Kind::Husk
            | Kind::Stray
            | Kind::Bogged
            | Kind::Parched
            | Kind::ZombieVillager
            | Kind::Giant
            | Kind::CaveSpider
            | Kind::Silverfish
            | Kind::Endermite
            | Kind::Drowned
            | Kind::Guardian
            | Kind::ElderGuardian
    )
}

/// Does the species' movement emission produce sounds (swim, step)? Guardians, bats,
/// silverfish and endermites only emit events.
pub fn emits_sounds(k: Kind) -> bool {
    !matches!(k, Kind::Guardian | Kind::ElderGuardian | Kind::Bat | Kind::Silverfish | Kind::Endermite)
}

/// Swim sound of the species.
pub fn swim_sound(m: &Mob) -> &'static str {
    match m.kind {
        Kind::Drowned => "minecraft:entity.drowned.swim",
        Kind::Turtle => "minecraft:entity.turtle.swim",
        // F10: axolotl; tadpole (fish).
        Kind::Axolotl => "minecraft:entity.axolotl.swim",
        Kind::Tadpole => "minecraft:entity.fish.swim",
        // Skeleton horse on the ground: no step in water (mounted gallop not simulated).
        Kind::SkeletonHorse if m.on_ground => "minecraft:entity.skeleton_horse.step_water",
        Kind::SkeletonHorse => "minecraft:entity.skeleton_horse.swim",
        k if hostile(k) => "minecraft:entity.hostile.swim",
        _ => "minecraft:entity.generic.swim",
    }
}

/// Species-specific swim volume (turtle × 1.5, skeleton horse capped).
fn swim_volume(m: &Mob, v: f32) -> f32 {
    match m.kind {
        Kind::Turtle => v * 1.5,
        Kind::SkeletonHorse if m.on_ground => 0.3,
        Kind::SkeletonHorse => 0.1f32.min(v * 25.0),
        _ => v,
    }
}

/// Slow splash sound (fast: always the generic one).
pub fn splash_sound(k: Kind) -> &'static str {
    if k == Kind::Axolotl {
        return "minecraft:entity.axolotl.splash";
    }
    if hostile(k) { "minecraft:entity.hostile.splash" } else { "minecraft:entity.generic.splash" }
}

/// Sound speed of a movement: √(x² × 0.2 + y² + z² × 0.2) × modulator, capped at 1.
pub fn sound_speed(v: [f64; 3], scale: f32) -> f32 {
    let k = f64::from(0.2f32);
    1.0f32.min((v[0] * v[0] * k + v[1] * v[1] + v[2] * v[2] * k).sqrt() as f32 * scale)
}

/// Number of particles of a splash: `i < 1 + width × 20` in floats.
pub fn splash_particles(width: f32) -> u32 {
    let lim = 1.0f32 + width * 20.0;
    let mut n = 0u32;
    while (n as f32) < lim {
        n += 1;
    }
    n
}

/// Does the step count as a ground step (block sound): on the ground or on a ladder, without swimming.
pub fn grounded_step(m: &Mob, climbing: bool) -> bool {
    (m.on_ground || climbing) && !m.aqua.swimming
}

impl Server {
    /// Swim sound: modulator 0.35 (mob alone, without a steering rider).
    pub(crate) fn mob_swim_sound(&mut self, m: &mut Mob) {
        let v = swim_volume(m, sound_speed(m.vel, 0.35));
        let pitch = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.4;
        self.mob_sound(m, swim_sound(m), v, pitch);
    }

    /// Splash on entering the water, movement from before the current.
    pub(crate) fn mob_splash(&mut self, m: &mut Mob, vel: [f64; 3]) {
        let speed = sound_speed(vel, 0.2);
        let name = if speed < 0.25 { splash_sound(m.kind) } else { "minecraft:entity.generic.splash" };
        let pitch = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.4;
        self.mob_sound(m, name, speed, pitch);
        // Bubbles (3 draws each) then droplets (2 draws): nothing is sent.
        let n = splash_particles(m.width());
        for _ in 0..n {
            m.rng.next_double();
            m.rng.next_double();
            m.rng.next_double();
        }
        for _ in 0..n {
            m.rng.next_double();
            m.rng.next_double();
        }
        self.mob_game_event(m, "minecraft:splash");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particles_and_speed() {
        // Adult drowned 0.6: 1 + 12 = 13 → 13 particles; guardian 0.85: 18; small 0.49: 10.8 → 11.
        assert_eq!(splash_particles(0.6), 13);
        assert_eq!(splash_particles(0.85), 18);
        assert_eq!(splash_particles(0.49), 11);
        assert_eq!(sound_speed([0.0, -1.0, 0.0], 0.2), 0.2);
        assert_eq!(sound_speed([0.0, -10.0, 0.0], 0.2), 1.0);
        assert_eq!(splash_sound(Kind::Guardian), "minecraft:entity.hostile.splash");
        assert_eq!(splash_sound(Kind::Slime), "minecraft:entity.generic.splash");
    }
}
