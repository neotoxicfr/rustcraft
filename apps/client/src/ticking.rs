//! Server rate (`ticking_state`, `ticking_step`, `protocol-supplements.md` § 2):
//! client tick duration and world freeze.

use crate::entities::{Entities, Entity};

/// Nominal client tick (20 per second): the client never ticks faster.
pub const TICK: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickRate {
    /// Server ticks per second (≥ 1).
    pub rate: f32,
    pub frozen: bool,
    /// Remaining steps during the freeze (`/tick step`).
    pub steps: i32,
    /// The world advances at this tick (recomputed at the start of each client tick).
    pub runs: bool,
}

impl Default for TickRate {
    fn default() -> Self {
        TickRate { rate: 20.0, frozen: false, steps: 0, runs: true }
    }
}

impl TickRate {
    pub fn set_state(&mut self, rate: f32, frozen: bool) {
        self.rate = rate.max(1.0);
        self.frozen = frozen;
    }

    /// Target duration of the client tick (s): `max(50 ms, ms per server tick)` if the world advances,
    /// otherwise 50 ms.
    pub fn tick_len(&self) -> f64 {
        if self.runs { TICK.max(1.0 / f64::from(self.rate)) } else { TICK }
    }

    /// Start of a client tick: does the world advance? Consumes a step during the freeze.
    pub fn begin_tick(&mut self) -> bool {
        self.runs = !self.frozen || self.steps > 0;
        if self.steps > 0 {
            self.steps -= 1;
        }
        self.runs
    }
}

/// Ticks played at most per frame; the surplus of a longer hitch is discarded.
pub const MAX_TICKS_PER_FRAME: u32 = 10;

/// Ticks due after a frame of `dt` seconds: all the elapsed time counts (no cap), the
/// fractional remainder is kept in `acc`, and at most 10 ticks are returned.
pub fn due_ticks(acc: &mut f64, dt: f64, tick_len: f64) -> u32 {
    *acc += dt.max(0.0);
    // Rounding margin: 0.3 s / 0.05 s is 5.999… in binary.
    let n = (*acc / tick_len + 1e-9).floor();
    *acc = (*acc - n * tick_len).max(0.0);
    (n as u64).min(u64::from(MAX_TICKS_PER_FRAME)) as u32
}

/// Frame duration in nominal ticks (50 ms), brought down to 0.5 beyond 7: fraction of the smoothing
/// of the save indicator (the game smooths with it, not with the tick fraction).
pub fn realtime_delta(elapsed: f64) -> f32 {
    let d = (elapsed / TICK) as f32;
    if d > 7.0 { 0.5 } else { d }
}

/// Singleplayer pause seen by the clock: the render tick fraction is frozen on entering pause
/// (world still to the frame) and restored on resume; the time elapsed during the pause
/// keeps the interface ticks running.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Pause {
    pub on: bool,
    held: f64,
}

impl Pause {
    /// New pause state (once per frame); true on entering pause.
    pub fn update(&mut self, on: bool, acc: &mut f64) -> bool {
        let entered = on && !self.on;
        if entered {
            self.held = *acc;
        } else if !on && self.on {
            *acc = self.held;
        }
        self.on = on;
        entered
    }

    /// Tick remainder (s) seen by the renderer: frozen during the pause.
    pub fn acc(&self, acc: f64) -> f64 {
        if self.on { self.held } else { acc }
    }
}

/// A "frozen" entity is not a player and carries no player (even indirectly).
fn carries_player(ents: &Entities, e: &Entity, depth: u32) -> bool {
    e.kind == "player"
        || (depth < 8
            && e.passengers
                .iter()
                .any(|p| *p == ents.local || ents.map.get(p).is_some_and(|q| carries_player(ents, q, depth + 1))))
}

/// Old state = current state: rendered at fraction 1.0, with no smoothing, as long as the entity no longer ticks
/// (the renderer interpolates between two equal values).
fn snap(e: &mut Entity) {
    e.pos_o = e.pos;
    e.yaw_o = e.yaw;
    e.pitch_o = e.pitch;
    e.body_o = e.body;
    e.head_o = e.head;
    e.walk_speed_o = e.walk_speed;
    e.cape_o = e.cape;
    e.bob_o = e.bob;
}

/// Entity tick: all if the world advances, otherwise only players and their mounts.
pub fn tick_entities(ents: &mut Entities, runs: bool) {
    if runs {
        ents.tick();
        return;
    }
    let keep: Vec<i32> = ents.map.iter().filter(|(_, e)| carries_player(ents, e, 0)).map(|(id, _)| *id).collect();
    for (id, e) in ents.map.iter_mut() {
        if !keep.contains(id) {
            snap(e);
        }
    }
    ents.tick_some(&keep);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence() {
        let mut t = TickRate::default();
        assert_eq!(t.tick_len(), 0.05);
        t.set_state(10.0, false);
        assert!((t.tick_len() - 0.1).abs() < 1e-9);
        t.set_state(40.0, false);
        assert_eq!(t.tick_len(), 0.05);
        t.set_state(0.0, false);
        assert_eq!(t.rate, 1.0);
        // Freeze with two steps: two ticks that advance, then nothing; 50 ms during the freeze.
        t.set_state(10.0, true);
        t.steps = 2;
        assert!(t.begin_tick());
        assert!(t.begin_tick());
        assert!(!t.begin_tick());
        assert_eq!(t.tick_len(), 0.05);
    }

    #[test]
    fn realtime_delta_is_frame_time_in_ticks() {
        assert!((realtime_delta(0.025) - 0.5).abs() < 1e-6);
        assert!((realtime_delta(1.0 / 60.0) - 1.0 / 3.0).abs() < 1e-6);
        assert!((realtime_delta(0.34) - 6.8).abs() < 1e-5);
        assert_eq!(realtime_delta(0.36), 0.5);
    }

    /// Pause: fraction frozen on entry (ticks keep accumulating), restored on exit.
    #[test]
    fn pause_freezes_then_restores_the_partial_tick() {
        let (mut p, mut acc) = (Pause::default(), 0.0);
        due_ticks(&mut acc, 0.02, TICK);
        assert!(p.update(true, &mut acc));
        assert!(!p.update(true, &mut acc), "a single entry into pause");
        due_ticks(&mut acc, 0.017, TICK);
        assert!((p.acc(acc) - 0.02).abs() < 1e-12, "render fraction frozen");
        assert!((acc - 0.037).abs() < 1e-12);
        p.update(false, &mut acc);
        assert!((acc - 0.02).abs() < 1e-12 && (p.acc(acc) - 0.02).abs() < 1e-12, "remainder from before the pause restored");
    }

    /// A 300 ms frame yields 6 ticks; 700 ms yields 10 (the rest is discarded); the fraction
    /// is kept from one frame to the next.
    #[test]
    fn frame_hitch_is_caught_up() {
        let mut acc = 0.0;
        assert_eq!(due_ticks(&mut acc, 0.3, TICK), 6);
        assert_eq!(due_ticks(&mut acc, 0.7, TICK), 10);
        assert!(acc < TICK);
        let mut acc = 0.0;
        assert_eq!(due_ticks(&mut acc, 0.03, TICK), 0);
        assert_eq!(due_ticks(&mut acc, 0.03, TICK), 1);
        assert!((acc - 0.01).abs() < 1e-9);
    }
}
