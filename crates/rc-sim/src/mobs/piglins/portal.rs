//! Nether portal that spawns zombified piglins (random block tick): outside peaceful, monster
//! spawning allowed, environment attribute `gameplay/nether_portal_spawns_piglin` true
//! (Overworld), draw `2000 < difficulty`, player close enough; goes down the portal column to
//! the frame and spawns the mob above the first non-portal block if it accepts the spawn.

use super::super::spawn::{Group, Reason};
use super::super::species::Kind;
use crate::server::Server;

const KEY: &str = "minecraft:gameplay/nether_portal_spawns_piglin";

impl Server {
    /// Random tick of a Nether portal block; false if the state is not one.
    pub(crate) fn pig_portal_random_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        if self.state_name(state) != "minecraft:nether_portal" {
            return false;
        }
        let diff = self.level.difficulty_id();
        let monsters = self.rule("minecraft:spawn_mobs", true) && self.rule("minecraft:spawn_monsters", true);
        if monsters
            && diff != 0
            && super::env_bool(self, p, KEY, false)
            && self.bs.rng.next_int_bound(2000) < diff
            && self.player_close_for_spawning(p)
        {
            let mut q = p;
            while self.state_name(self.st(q)) == "minecraft:nether_portal" {
                q[1] -= 1;
            }
            if self.valid_spawn_on(q, Kind::ZombifiedPiglin) {
                self.pig_spawn_structure(Kind::ZombifiedPiglin, [q[0], q[1] + 1, q[2]]);
            }
        }
        true
    }

    /// Non-spectator player within 128 blocks (horizontal) of the chunk center.
    pub(crate) fn player_close_for_spawning(&self, p: [i32; 3]) -> bool {
        if self.game_mode == 3 {
            return false;
        }
        let cx = f64::from(p[0].div_euclid(16) * 16 + 8) - self.pos[0];
        let cz = f64::from(p[2].div_euclid(16) * 16 + 8) - self.pos[2];
        cx * cx + cz * cz < 128.0 * 128.0
    }

    /// "Structure" spawn of a type at a block position, without ground search: block center,
    /// drawn orientation (world random), finalization, addition, ambient sound.
    pub(crate) fn pig_spawn_structure(&mut self, kind: Kind, p: [i32; 3]) {
        let yaw = super::super::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let mut m = self.new_mob(kind, pos, yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let mut g = Group::default();
        self.finalize_spawn(&mut m, Reason::Structure, &mut g);
        // The portal delay set on the mob (and its mount) has no effect: mobs
        // do not cross portals yet in singleplayer.
        if let Some(a) = kind.info().ambient {
            let a = a.to_owned();
            self.make_sound(&mut m, &a);
        }
        self.add_mob(m);
    }
}
