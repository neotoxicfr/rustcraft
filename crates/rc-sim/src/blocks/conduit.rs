//! Integrated-server conduit (G6-F12): block entity tick.
//!
//! Every 40 game ticks, the frame is re-examined: the 27 blocks of the 3 × 3 × 3 cube centred
//! on the conduit must hold water (the waterlogged conduit included), then, among the
//! 42 places of the three rings of radius 2 (planes x = 0, y = 0 and z = 0 outside the central cube), the
//! blocks of the tag `conduit_effect_block` are counted. At 16 blocks the conduit is active: state
//! change announced by the activation or deactivation sound; effect `conduit_power` (260 ticks,
//! ambient) to players in water or in rain whose feet block is within
//! `n / 7 × 16` blocks (distance between blocks); at 42 blocks it hunts: a hostile target in
//! water or in rain, drawn at random in the 8-block cube around the conduit, kept as long
//! as it lives within 8 blocks, takes 4 magic damage at each examination. The target lives in the NBT
//! `Target` (UUID), re-read on load; changing it sends the block entity back to the client. When active,
//! the conduit plays its ambience every 80 ticks and a short ambience at a drawn interval
//! (60 to 99 ticks).
//!
//! The active state and the deadline of the short ambience live in memory as in the game: they
//! restart from zero on load, hence the activation sound replayed at the first examination.

use super::P;
use super::level::BeTick;
use crate::mobs::golems::is_enemy;
use crate::server::Server;

/// Period of the frame examination (game ticks).
const REFRESH: i64 = 40;
/// Frame blocks for activation and for hunting.
const MIN_ACTIVE: usize = 16;
const MIN_HUNT: usize = 42;
/// Hunting range (cube around the block, then distance between blocks).
const KILL_RANGE: i32 = 8;

/// Frame places (offsets from the conduit), in the game's walk order.
pub(super) const FRAME: [[i32; 3]; 42] = frame();

const fn frame() -> [[i32; 3]; 42] {
    let mut out = [[0; 3]; 42];
    let mut n = 0;
    let mut ox: i32 = -2;
    while ox <= 2 {
        let mut oy: i32 = -2;
        while oy <= 2 {
            let mut oz: i32 = -2;
            while oz <= 2 {
                let (ax, ay, az) = (ox.abs(), oy.abs(), oz.abs());
                let outside = ax > 1 || ay > 1 || az > 1;
                let ring = (ox == 0 && (ay == 2 || az == 2))
                    || (oy == 0 && (ax == 2 || az == 2))
                    || (oz == 0 && (ax == 2 || ay == 2));
                if outside && ring {
                    out[n] = [ox, oy, oz];
                    n += 1;
                }
                oz += 1;
            }
            oy += 1;
        }
        ox += 1;
    }
    assert!(n == 42);
    out
}

/// Ticker state: active at the last examination, tick after which the short ambience replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ConduitTick {
    pub active: bool,
    pub next_ambient: i64,
}

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

/// Square of the distance between two blocks (low corners), like the game's comparison.
fn dist2(a: P, b: P) -> f64 {
    (0..3).map(|i| f64::from(a[i] - b[i]).powi(2)).sum()
}

impl Server {
    /// Ticker placed on creation or load of the block entity (new state, inactive).
    pub(crate) fn conduit_ticker_register(&mut self, p: P) {
        if matches!(self.bs.be_ticks.get(&p), Some((_, BeTick::Conduit(_)))) {
            return;
        }
        let id = self.add_ticker(p);
        self.bs.be_ticks.insert(p, (id, BeTick::Conduit(ConduitTick::default())));
    }

    fn conduit_store(&mut self, p: P, c: ConduitTick) {
        if let Some((_, t)) = self.bs.be_ticks.get_mut(&p) {
            *t = BeTick::Conduit(c);
        }
    }

    /// Frame blocks around the conduit at `p`; 0 if the 3 × 3 × 3 cube is not all water.
    pub(crate) fn conduit_frame(&self, p: P) -> usize {
        let wet = (-1..=1).all(|dx| {
            (-1..=1).all(|dy| (-1..=1).all(|dz| self.fluid_at([p[0] + dx, p[1] + dy, p[2] + dz]).kind == 1))
        });
        if !wet {
            return 0;
        }
        let Some(tag) = self.block_tags.get("minecraft:conduit_effect_block") else { return 0 };
        FRAME
            .iter()
            .filter(|o| {
                let s = self.get([p[0] + o[0], p[1] + o[1], p[2] + o[2]]);
                tag.binary_search(&u32::from(self.v().block(s))).is_ok()
            })
            .count()
    }

    /// Block entity tick.
    pub(crate) fn conduit_tick(&mut self, p: P, mut c: ConduitTick) {
        let t = self.time;
        if t % REFRESH == 0 {
            let n = self.conduit_frame(p);
            let active = n >= MIN_ACTIVE;
            if active != c.active {
                let name = if active { "minecraft:block.conduit.activate" } else { "minecraft:block.conduit.deactivate" };
                self.play_sound(name, 4, center(p), 1.0, 1.0);
            }
            c.active = active;
            // State stored before the effects (an effect or a hit can remove the conduit).
            self.conduit_store(p, c);
            if active {
                self.conduit_effects(p, n);
                self.conduit_attack(p, n >= MIN_HUNT);
            }
        }
        if c.active {
            if t % 80 == 0 {
                self.play_sound("minecraft:block.conduit.ambient", 4, center(p), 1.0, 1.0);
            }
            if t > c.next_ambient {
                c.next_ambient = t + 60 + i64::from(self.bs.rng.next_int_bound(40));
                self.conduit_store(p, c);
                self.play_sound("minecraft:block.conduit.ambient.short", 4, center(p), 1.0, 1.0);
            }
        }
    }

    /// `conduit_power` to wet players (spectators excluded) in range, the host first.
    fn conduit_effects(&mut self, p: P, n: usize) {
        let range = f64::from((n / 7 * 16) as i32);
        let reach = |sv: &Server| {
            sv.game_mode != 3
                && dist2(p, crate::mobs::mth::block_of(sv.pos)) < range * range
                && sv.player_in_water_or_rain()
        };
        let give = |sv: &mut Server| {
            let mut e = crate::effects::Inst::new("minecraft:conduit_power", 260, 0);
            e.ambient = true;
            sv.add_effect(e);
        };
        if reach(self) {
            give(self);
        }
        let guests = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..guests {
            self.as_guest(i, |sv| {
                if reach(sv) {
                    give(sv);
                }
            });
        }
    }

    /// Target kept, dropped or chosen; 4 magic damage to a living target; target changed:
    /// NBT rewritten and block entity sent back to the client.
    fn conduit_attack(&mut self, p: P, hunting: bool) {
        let old = self.be_get(p).and_then(|b| crate::nbtio::uuid(b.get("Target")));
        let new = match (hunting, old) {
            (false, _) => None,
            (true, Some(u)) => self
                .mobs
                .get_uuid(u)
                .filter(|m| m.alive() && dist2(p, m.block_pos()) < f64::from(KILL_RANGE * KILL_RANGE))
                .map(|_| u),
            (true, None) => self.conduit_pick(p),
        };
        if let Some(u) = new
            && let Some(i) = self.mobs.index_of_uuid(u)
            && let Some(mut m) = self.mobs.list[i].take()
        {
            if !m.removed {
                self.play_sound("minecraft:block.conduit.attack.target", 4, m.pos, 1.0, 1.0);
                self.mob_hurt(&mut m, "minecraft:magic", 4.0, None, None);
            }
            self.mobs.list[i] = Some(m);
        }
        if new != old {
            if let Some(be) = self.be_get_mut(p) {
                match new {
                    Some(u) => crate::nbtio::set(be, "Target", crate::nbtio::uuid_nbt(u)),
                    None => crate::nbtio::remove(be, "Target"),
                }
            }
            self.changed.push(p);
        }
    }

    /// Target drawn among wet enemies whose box touches the 8-block cube.
    fn conduit_pick(&mut self, p: P) -> Option<u128> {
        let r = f64::from(KILL_RANGE);
        let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let area = rc_physics::Aabb { min: [x - r, y - r, z - r], max: [x + 1.0 + r, y + 1.0 + r, z + 1.0 + r] };
        let found: Vec<u128> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| !o.removed && is_enemy(o.kind) && o.touches(&area) && self.mob_in_water_or_rain(o))
            .map(|o| o.uuid)
            .collect();
        if found.is_empty() {
            return None;
        }
        let i = self.bs.rng.next_int_bound(found.len() as i32) as usize;
        found.get(i).copied()
    }

    /// Network update of the conduit's block entity: its target only (empty without a target).
    pub(crate) fn conduit_update_tag(&self, p: P) -> Option<rc_proto::nbt::Nbt> {
        let be = self.be_get(p)?;
        let mut out = Vec::new();
        if let Some(t) = be.get("Target") {
            out.push(("Target".to_owned(), t.clone()));
        }
        Some(rc_proto::nbt::Nbt::Compound(out))
    }
}

#[cfg(test)]
mod tests {
    use super::FRAME;

    /// The complete frame has 42 distinct places, all outside the central cube, each in
    /// one of the three planes of the conduit and on the edge (radius 2) of its ring.
    #[test]
    fn frame_has_the_42_ring_places() {
        let mut seen = std::collections::HashSet::new();
        for o in FRAME {
            assert!(seen.insert(o), "place {o:?} en double");
            assert!(o.iter().any(|v| v.abs() == 2), "{o:?} in the central cube");
            assert!(o.contains(&0), "{o:?} outside the three planes");
        }
        assert_eq!(seen.len(), 42);
    }
}
