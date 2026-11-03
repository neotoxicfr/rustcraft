//! Turtle eggs on the integrated server (G6-F08): hatching at random ticks, eggs lost when
//! trampled, landed on or broken by a player. Stacking (up to 4 eggs, same item, not
//! sneaking) goes through the placement (`placement.rs`, property `eggs`).
//!
//! - Random tick: a draw against the hatch chance of the time of day (1 from 21,062 to 21,904 in
//!   the overworld, 0.002 otherwise), then sand below (tag `sand`). Stages 0 → 1 → 2 crack the
//!   shell (sound, flags 2, "block changed" with the old state); at stage 2 the block is removed
//!   ("block destroyed") and each egg gives a baby turtle (age −24,000) whose home is the nest,
//!   placed 0.3 + 0.2·i east and 0.3 south of the block corner, after its block particles (2001).
//! - Trampled (each tick on the ground, not sneaking): 1 in 100; landed on (any fall, never a
//!   zombie): 1 in 3. Never by a turtle or a bat; a mob only when `mob_griefing` allows it; the
//!   player always (no spawn protection on the integrated server). The world random is drawn
//!   only for an entity that may break eggs.
//! - One egg less: sound, then the last egg removes the block, otherwise one egg fewer (flags 2,
//!   "block destroyed" with the old state, block particles 2001). A player breaking the block
//!   outside creative puts back one egg fewer after the loot (silk touch: one egg).
//! - Every state change on sand (placement, crack, egg lost): event 2012 (growth particles).

use super::{P, below, flag};
use crate::mobs::species::Kind as MobKind;
use crate::server::Server;

/// Hatch chance outside the window of the day timeline (default of the environment attribute).
const HATCH_DEFAULT: f32 = 0.002;
/// Window of the day timeline where every random tick hatches a stage (ticks of the day).
const HATCH_WINDOW: std::ops::Range<i64> = 21_062..21_905;
/// Age of a hatched turtle.
const BABY_AGE: i32 = -24_000;

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

impl Server {
    /// Environment attribute "turtle egg hatch chance" at the time of day: the overworld day
    /// timeline raises it to 1 in its window (maximum with the default), nothing elsewhere.
    pub(crate) fn turtle_egg_hatch_chance(&self) -> f32 {
        if self.dim.name == "minecraft:overworld" && HATCH_WINDOW.contains(&self.day_time()) { 1.0 } else { HATCH_DEFAULT }
    }

    fn turtle_egg_on_sand(&self, p: P) -> bool {
        self.btag_id(below(p), crate::tag_bits::TagId::Sand)
    }

    /// Any state change of a turtle egg placed on sand: growth particles (2012, 15).
    pub(crate) fn turtle_egg_on_place(&mut self, s: u32, p: P) {
        if self.view().block(s) == self.bs.t.id.turtle_egg && self.turtle_egg_on_sand(p) {
            self.level_event(2012, p, 15);
        }
    }

    pub(crate) fn turtle_egg_random_tick(&mut self, s: u32, p: P) {
        let chance = self.turtle_egg_hatch_chance();
        if !(chance > 0.0 && self.bs.rng.next_float() < chance) || !self.turtle_egg_on_sand(p) {
            return;
        }
        let hatch = self.view().get_int(s, "hatch");
        let pitch = 0.9 + self.bs.rng.next_float() * 0.2;
        if hatch < 2 {
            self.world_sound("minecraft:entity.turtle.egg_crack", 4, center(p), 0.7, pitch);
            let ns = self.view().set_int(s, "hatch", hatch + 1);
            self.set_block(p, ns, flag::CLIENTS);
            self.game_event_block("minecraft:block_change", p, None, Some(s));
            return;
        }
        self.world_sound("minecraft:entity.turtle.egg_hatch", 4, center(p), 0.7, pitch);
        self.remove_block(p, false);
        self.game_event_block("minecraft:block_destroy", p, None, Some(s));
        for i in 0..self.view().get_int(s, "eggs") {
            self.level_event(2001, p, s as i32);
            let at = [f64::from(p[0]) + 0.3 + f64::from(i) * 0.2, f64::from(p[1]), f64::from(p[2]) + 0.3];
            let mut t = self.new_mob(MobKind::Turtle, at, 0.0);
            t.age = BABY_AGE;
            t.surf.home = p;
            self.add_mob(t);
        }
    }

    /// May this entity crush eggs? `None`: the player.
    fn turtle_egg_breaker(&self, by: Option<MobKind>) -> bool {
        match by {
            Some(MobKind::Turtle | MobKind::Bat) => false,
            Some(_) => self.rule("minecraft:mob_griefing", true),
            None => true,
        }
    }

    /// One chance in `odds` (world random) that the eggs at `p` lose one.
    fn turtle_egg_crush(&mut self, p: P, by: Option<MobKind>, odds: i32) {
        let s = self.get(p);
        if self.view().block(s) == self.bs.t.id.turtle_egg
            && self.turtle_egg_breaker(by)
            && self.bs.rng.next_int_bound(odds) == 0
        {
            self.turtle_egg_decrease(p, s);
        }
    }

    /// Walked on by an entity on the ground (`None`: the player); a careful step (sneaking)
    /// spares the eggs.
    pub(crate) fn turtle_egg_step_on(&mut self, p: P, by: Option<MobKind>, careful: bool) {
        if !careful {
            self.turtle_egg_crush(p, by, 100);
        }
    }

    /// Landed on after a fall (`None`: the player); zombies never break them this way.
    pub(crate) fn turtle_egg_fall_on(&mut self, p: P, by: Option<MobKind>) {
        if !by.is_some_and(MobKind::zombie_like) {
            self.turtle_egg_crush(p, by, 3);
        }
    }

    /// One egg less (`s`: the eggs before): the last one removes the block.
    pub(crate) fn turtle_egg_decrease(&mut self, p: P, s: u32) {
        let pitch = 0.9 + self.bs.rng.next_float() * 0.2;
        self.world_sound("minecraft:entity.turtle.egg_break", 4, center(p), 0.7, pitch);
        let eggs = self.view().get_int(s, "eggs");
        if eggs <= 1 {
            self.destroy_block(p, false, 512);
        } else {
            let ns = self.view().set_int(s, "eggs", eggs - 1);
            self.set_block(p, ns, flag::CLIENTS);
            self.game_event_block("minecraft:block_destroy", p, None, Some(s));
            self.level_event(2001, p, s as i32);
        }
    }
}
