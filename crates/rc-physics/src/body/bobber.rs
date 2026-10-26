//! Client-side fishing bobber (`docs/specs/entities-objects.md` § 13.5): the game moves it
//! itself at each tick, on both sides; the server only re-syncs its position from time to
//! time. Three states: in flight (gravity, stop on impact, switch to floating on contact with
//! water), hooked to an entity (follows its position), floating (pull back to the surface,
//! dive when a fish bites).

use super::{Body, clip_blocks, f};
use crate::{Fluid, Tables, World, l2, sub};

/// State of the bobber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BobberState {
    #[default]
    Flying,
    Hooked,
    Bobbing,
}

/// Own state of the bobber kept from one tick to the next: its state and the "synchronised" randomness (same
/// seed on both sides: low bits of the UUID mixed with the tick's game time).
#[derive(Debug, Clone, Default)]
pub struct Bobber {
    pub state: BobberState,
    /// Bite seen at the previous tick (the dive starts when it turns true).
    pub biting: bool,
    rng: i64,
}

/// Data of a tick: hooked entity (feet of the hook, already at 0.8 of its height) if
/// it is known and valid, bite in progress, seed of the tick, draw of the entity's own
/// randomness in [0, 1) (own to each client in the game too).
#[derive(Debug, Clone, Copy)]
pub struct BobberTick {
    pub hooked: Option<[f64; 3]>,
    pub biting: bool,
    pub seed: i64,
    pub rand: f32,
}

const MULT: i64 = 0x5_DEEC_E66D;
const MASK: i64 = (1 << 48) - 1;

impl Bobber {
    pub(crate) fn reseed(&mut self, seed: i64) {
        self.rng = (seed ^ MULT) & MASK;
    }

    fn next_float(&mut self) -> f32 {
        self.rng = (self.rng.wrapping_mul(MULT).wrapping_add(0xB)) & MASK;
        (self.rng >> (48 - 24)) as f32 / (1 << 24) as f32
    }

    /// Bite received (data 9 set to true): diving vertical speed drawn from the synchronised
    /// randomness, in the state the last tick left it.
    pub fn bite_velocity(&mut self) -> f64 {
        let r = self.next_float();
        f64::from(-0.4f32 * (0.6 + r * (1.0 - 0.6)))
    }
}

impl Body {
    /// Client tick of the bobber (without the rotation, done by the caller).
    pub fn tick_bobber(&mut self, w: &dyn World, t: &Tables, b: &mut Bobber, inp: &BobberTick) {
        const G: f32 = 0.03;
        b.reseed(inp.seed);
        self.old = self.pos;
        // Tick base: fluids touched and push of the currents.
        self.update_fluids(w, t);
        let bp = self.block_pos();
        let s = Self::state(w, bp);
        let water_here = t.fluids.fluid(s) == Fluid::Water;
        let liquid =
            if water_here { f64::from(t.fluids.height_at(s, Self::state(w, [bp[0], bp[1] + 1, bp[2]]))) } else { 0.0 };
        let in_water = liquid > 0.0;
        match b.state {
            BobberState::Flying => {
                if inp.hooked.is_some() {
                    self.vel = [0.0; 3];
                    b.state = BobberState::Hooked;
                    return;
                }
                if in_water {
                    let v = self.vel;
                    self.vel = [v[0] * 0.3, v[1] * 0.2, v[2] * 0.3];
                    b.state = BobberState::Bobbing;
                    return;
                }
                // Upcoming impact against a block: the speed is brought to the square of the distance of the
                // point hit.
                let to = std::array::from_fn(|i| self.pos[i] + self.vel[i]);
                if let Some(hit) = clip_blocks(w, self.pos, to) {
                    // The game takes the SQUARE of the distance to the point hit (defect reproduced).
                    let d = l2(sub(hit, self.pos));
                    let len = l2(self.vel).sqrt();
                    self.vel = if len < f(1.0e-5) { [0.0; 3] } else { self.vel.map(|v| v / len * d) };
                }
            }
            BobberState::Hooked => {
                match inp.hooked {
                    Some(p) => self.pos = p,
                    None => b.state = BobberState::Flying,
                }
                return;
            }
            BobberState::Bobbing => {
                let v = self.vel;
                let mut force = self.pos[1] + v[1] - f64::from(bp[1]) - liquid;
                if force.abs() < 0.01 {
                    force += if force > 0.0 {
                        0.1
                    } else if force < 0.0 {
                        -0.1
                    } else {
                        0.0
                    };
                }
                self.vel = [v[0] * 0.9, v[1] - force * f(inp.rand) * 0.2, v[2] * 0.9];
                if in_water && inp.biting {
                    let (a, c) = (b.next_float(), b.next_float());
                    self.vel[1] += -0.1 * f(a) * f(c);
                }
            }
        }
        let g = self.gravity(f(G));
        if !water_here && !self.on_ground && inp.hooked.is_none() {
            self.vel[1] -= f(G);
        }
        self.travel(w, t, g, 0.98, 0.500_001);
        self.block_effects(w, t, false);
        if b.state == BobberState::Flying && (self.on_ground || self.horizontal_collision) {
            self.vel = [0.0; 3];
        }
        self.vel = self.vel.map(|v| v * 0.92);
    }
}
