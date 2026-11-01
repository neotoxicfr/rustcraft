//! Shared pass over the blocks crossed by a living entity (player and mobs), played after
//! its movement of the tick:
//! 1. cells swept by the box (shrunk by 1e-5) along each recorded movement,
//!    axis by axis (vertical first, then the longest horizontal axis second);
//! 2. effects of the blocks (fire, soul fire, cactus, lit campfire, powder snow) and of the
//!    fluids (water, lava) touched: direct damage (cactus, campfire) falls during the
//!    traversal, the others are grouped by step then applied in the fixed order of the types
//!    (freezing, end of freezing, ignition by fire, ignition by lava, extinguishing);
//! 3. rain at the feet or at the top of the box puts out; the extinguish sound plays if fire
//!    or freezing stopped during the pass; with no fire and no ignition, the counter returns
//!    to rest (−20 for the player, 0 for a mob).
//!
//! Traversal (shared with ghast flight): cells of the box moved back (step 0), then the
//! so-called "furthest" corner followed cell by cell, with the game's axis choice reproduced
//! as is (for a vertical movement it takes a bottom corner), then the remaining arrival cells.
//! Each group of cells is visited in the direction of the path (order of the direct damage).

use rc_physics::Aabb;

use crate::mobs::mob::Mob;
use crate::server::Server;

/// Step cap of a movement (beyond it, only the arrival cell counts).
const MAX_STEPS: i32 = 16;
/// Full freezing (ticks).
pub(crate) const FREEZE_REQUIRED: i32 = 140;

// ---------- Cell traversal ----------

/// Whole box (bounds included) of the cells touched by `b`.
fn cells(b: &Aabb) -> [i32; 6] {
    [
        b.min[0].floor() as i32,
        b.min[1].floor() as i32,
        b.min[2].floor() as i32,
        b.max[0].floor() as i32,
        b.max[1].floor() as i32,
        b.max[2].floor() as i32,
    ]
}

/// Min and max bounds of a whole box given by any two corners.
fn span(c: [i32; 6]) -> ([i32; 3], [i32; 3]) {
    ([c[0].min(c[3]), c[1].min(c[4]), c[2].min(c[5])], [c[0].max(c[3]), c[1].max(c[4]), c[2].max(c[5])])
}

/// Cells of `c` without a path, in the game's order: z in the outer loop, then y, x inside.
fn for_cells(c: [i32; 6], f: &mut dyn FnMut([i32; 3]) -> bool) -> bool {
    let (lo, hi) = span(c);
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if !f([x, y, z]) {
                    return false;
                }
            }
        }
    }
    true
}

/// Cells of `c` in the direction of the path `d`, game order: each axis starts from the min
/// edge if its component is positive or zero, from the max edge otherwise; vertical outer loop,
/// the dominant horizontal axis in the middle (x on a tie), the weakest inside.
fn for_cells_toward(c: [i32; 6], d: [f64; 3], f: &mut dyn FnMut([i32; 3]) -> bool) -> bool {
    let (lo, hi) = span(c);
    let [a0, a1, a2] = if d[0].abs() < d[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
    let s: [i32; 3] = d.map(|v| if v >= 0.0 { 1 } else { -1 });
    let begin: [i32; 3] = std::array::from_fn(|i| if s[i] > 0 { lo[i] } else { hi[i] });
    let n: [i32; 3] = std::array::from_fn(|i| hi[i] - lo[i]);
    let mut p = [0; 3];
    for i0 in 0..=n[a0] {
        p[a0] = begin[a0] + s[a0] * i0;
        for i1 in 0..=n[a1] {
            p[a1] = begin[a1] + s[a1] * i1;
            for i2 in 0..=n[a2] {
                p[a2] = begin[a2] + s[a2] * i2;
                if !f(p) {
                    return false;
                }
            }
        }
    }
    true
}

/// Cap of the seen-cells mask (bits): beyond it, the fallback list alone (oversized path).
const SEEN_MAX_BITS: i64 = 1 << 18;

/// Cells already seen by a traversal: bit mask over the bounding box of the path (one-cell
/// margin), fallback list for a cell outside the box or an oversized path. Reused buffers.
/// `RUSTCRAFT_OPT_INSIDE_SEEN_LINEAR`: list alone (previous lead, quadratic).
#[derive(Debug, Default)]
pub(crate) struct Seen {
    lo: [i32; 3],
    dim: [i64; 3],
    bits: Vec<u64>,
    spill: Vec<[i32; 3]>,
    /// List alone, whatever happens (reference of the tests and the bench).
    linear: bool,
}

impl Seen {
    /// Set as a list alone (previous lead), for comparisons.
    #[cfg(test)]
    pub(crate) fn linear() -> Self {
        Self { linear: true, ..Self::default() }
    }

    /// Empties the set, mask laid over the union of the whole boxes `a` and `b`.
    fn reset(&mut self, a: [i32; 6], b: [i32; 6]) {
        self.spill.clear();
        self.bits.clear();
        self.dim = [0; 3];
        if self.linear || rc_data::env_flag!("RUSTCRAFT_OPT_INSIDE_SEEN_LINEAR") {
            return;
        }
        let ((la, ha), (lb, hb)) = (span(a), span(b));
        let lo: [i32; 3] = std::array::from_fn(|i| la[i].min(lb[i]).saturating_sub(1));
        let dim: [i64; 3] =
            std::array::from_fn(|i| i64::from(ha[i].max(hb[i])) + 1 - i64::from(lo[i]) + 1);
        if dim.iter().any(|&d| d > SEEN_MAX_BITS) || dim[0] * dim[1] * dim[2] > SEEN_MAX_BITS {
            return;
        }
        self.lo = lo;
        self.dim = dim;
        self.bits.resize(((dim[0] * dim[1] * dim[2] + 63) / 64) as usize, 0);
    }

    /// True if `p` had not been seen yet (it is now).
    fn insert(&mut self, p: [i32; 3]) -> bool {
        let r: [i64; 3] = std::array::from_fn(|i| i64::from(p[i]) - i64::from(self.lo[i]));
        if (0..3).all(|i| r[i] >= 0 && r[i] < self.dim[i]) {
            let k = ((r[0] * self.dim[1] + r[1]) * self.dim[2] + r[2]) as usize;
            let (w, bit) = (k / 64, 1u64 << (k % 64));
            let fresh = self.bits[w] & bit == 0;
            self.bits[w] |= bit;
            return fresh;
        }
        if self.spill.contains(&p) {
            return false;
        }
        self.spill.push(p);
        true
    }
}

/// Intersection of a segment with the unit cell `c`: entry point through a face.
fn clip_cell(c: [i32; 3], from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    // Starting scale 1, fraction kept if 0 < t < scale (strict bounds, tolerance 1e-7).
    let mut scale = 1.0f64;
    let mut hit = false;
    for a in 0..3 {
        let (b1, b2) = ((a + 1) % 3, (a + 2) % 3);
        let plane = if d[a] > 1.0e-7 {
            f64::from(c[a])
        } else if d[a] < -1.0e-7 {
            f64::from(c[a] + 1)
        } else {
            continue;
        };
        let t = (plane - from[a]) / d[a];
        if !(t > 0.0 && t < scale) {
            continue;
        }
        let p1 = from[b1] + t * d[b1];
        let p2 = from[b2] + t * d[b2];
        let inside = |p: f64, i: usize| f64::from(c[i]) - 1.0e-7 < p && p < f64::from(c[i] + 1) + 1.0e-7;
        if inside(p1, b1) && inside(p2, b2) {
            scale = t;
            hit = true;
        }
    }
    hit.then(|| [from[0] + d[0] * scale, from[1] + d[1] * scale, from[2] + d[2] * scale])
}

/// The "furthest" corner (the game's axis combination reproduced as is).
fn furthest_corner(d: [f64; 3]) -> [i32; 3] {
    let (xd, yd, zd) = (d[0].abs(), d[1].abs(), d[2].abs());
    let sx = if d[0] >= 0.0 { 1 } else { -1 };
    let sy = if d[1] >= 0.0 { 1 } else { -1 };
    let sz = if d[2] >= 0.0 { 1 } else { -1 };
    if xd <= yd && xd <= zd {
        [-sx, -sz, sy]
    } else if yd <= zd {
        [sz, -sy, -sx]
    } else {
        [-sy, sx, -sz]
    }
}

/// Cells swept by the box `at_target` (arrival box) coming from `from` to `to`, each one
/// once with the number of the step where it is reached, in the game's order; `seen`: reused
/// buffer of the cells already seen. False if `visit` stops the traversal.
pub(crate) fn each_cell_stepped(
    from: [f64; 3],
    to: [f64; 3],
    at_target: &Aabb,
    seen: &mut Seen,
    visit: &mut dyn FnMut([i32; 3], i32) -> bool,
) -> bool {
    let t = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let lsq = t[0] * t[0] + t[1] * t[1] + t[2] * t[2];
    if lsq < f64::from(1.0e-5f32 * 1.0e-5f32) {
        return for_cells(cells(at_target), &mut |p| visit(p, 0));
    }
    let back = at_target.offset(-t[0], -t[1], -t[2]);
    seen.reset(cells(&back), cells(at_target));
    let ok = for_cells_toward(cells(&back), t, &mut |p| {
        seen.insert(p);
        visit(p, 0)
    });
    if !ok {
        return false;
    }
    // Corner traversed cell by cell along the path, box of the entity's size at each step.
    let size =
        [at_target.max[0] - at_target.min[0], at_target.max[1] - at_target.min[1], at_target.max[2] - at_target.min[2]];
    let dir = furthest_corner(t);
    let center = [
        (at_target.min[0] + at_target.max[0]) * 0.5,
        (at_target.min[1] + at_target.max[1]) * 0.5,
        (at_target.min[2] + at_target.max[2]) * 0.5,
    ];
    let to_corner: [f64; 3] = std::array::from_fn(|i| center[i] + size[i] * 0.5 * f64::from(dir[i]));
    let from_corner: [f64; 3] = std::array::from_fn(|i| to_corner[i] - t[i]);
    let mut cell = from_corner.map(|v| v.floor() as i32);
    let sign = t.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / t[i] });
    let frac = |v: f64| v - v.floor();
    let mut tt: [f64; 3] =
        std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(from_corner[i]) } else { frac(from_corner[i]) });
    let mut steps = 0;
    while tt[0] <= 1.0 || tt[1] <= 1.0 || tt[2] <= 1.0 {
        let a = if tt[0] < tt[1] {
            if tt[0] < tt[2] { 0 } else { 2 }
        } else if tt[1] < tt[2] {
            1
        } else {
            2
        };
        cell[a] += sign[a];
        tt[a] += delta[a];
        let Some(hit) = clip_cell(cell, from_corner, to_corner) else { continue };
        steps += 1;
        let eps = f64::from(1.0e-5f32);
        let ch: [f64; 3] =
            std::array::from_fn(|i| hit[i].clamp(f64::from(cell[i]) + eps, f64::from(cell[i]) + 1.0 - eps));
        let opp: [i32; 3] = std::array::from_fn(|i| (ch[i] - size[i] * f64::from(dir[i])).floor() as i32);
        let ok = for_cells_toward([cell[0], cell[1], cell[2], opp[0], opp[1], opp[2]], t, &mut |p| {
            !seen.insert(p) || visit(p, steps)
        });
        if !ok {
            return false;
        }
    }
    for_cells_toward(cells(at_target), t, &mut |p| !seen.insert(p) || visit(p, steps + 1))
}

/// Variant without step number (ghast flight, portals).
pub(crate) fn each_cell_between(
    from: [f64; 3],
    to: [f64; 3],
    at_target: &Aabb,
    visit: &mut dyn FnMut([i32; 3]) -> bool,
) -> bool {
    let mut seen = Seen::default();
    each_cell_stepped(from, to, at_target, &mut seen, &mut |p, _| visit(p))
}

// ---------- Movements of the tick ----------

/// Recorded movement: start, arrival, wanted movement (axis order of the traversal).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Move {
    from: [f64; 3],
    to: [f64; 3],
    wanted: Option<[f64; 3]>,
}

/// Movements of an entity's tick (at most 100: the two oldest are merged);
/// reused buffer, no allocation in steady state.
#[derive(Debug, Clone, Default)]
pub(crate) struct Moves {
    list: Vec<Move>,
}

impl Moves {
    /// Accepted displacement from `from` to `to` for a wanted movement `wanted`.
    pub(crate) fn record(&mut self, from: [f64; 3], to: [f64; 3], wanted: [f64; 3]) {
        if self.list.len() >= 100 {
            let a = self.list.remove(0);
            self.list[0] = Move { from: a.from, to: self.list[0].to, wanted: None };
        }
        self.list.push(Move { from, to, wanted: Some(wanted) });
    }

    pub(crate) fn clear(&mut self) {
        self.list.clear();
    }

    /// Movements to traverse: those of the tick, completed up to `pos` (or from `old` to `pos`
    /// if nothing was recorded); the list starts empty again.
    fn finish(&mut self, old: [f64; 3], pos: [f64; 3], out: &mut Vec<Move>) {
        out.clear();
        out.append(&mut self.list);
        match out.last() {
            None => out.push(Move { from: old, to: pos, wanted: None }),
            Some(l) => {
                let d = [l.to[0] - pos[0], l.to[1] - pos[1], l.to[2] - pos[2]];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > f64::from(9.999_999_4e-11f32) {
                    out.push(Move { from: l.to, to: pos, wanted: None });
                }
            }
        }
    }
}

// ---------- Effects ----------

/// Own effect of a crossed block.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Blk {
    /// Non-empty block with no effect (counts for the split into steps).
    Plain,
    /// Fire (damage 1) or soul fire (2).
    Fire(f32),
    Cactus,
    /// Lit campfire: direct damage (1, soul 2).
    Campfire(f32),
    PowderSnow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Fl {
    None,
    Water,
    Lava,
}

/// Cell kept by the traversal (block touched or fluid touched).
#[derive(Debug, Clone, Copy)]
struct Hit {
    p: [i32; 3],
    step: i32,
    blk: Option<Blk>,
    fluid: Fl,
}

/// Deferred effect, in the order it will be applied.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Late {
    Freeze,
    ClearFreeze,
    FireIgnite,
    LavaIgnite,
    Extinguish,
    /// After ignition by fire: "in fire" damage.
    InFire(f32),
    /// After ignition by lava: lava damage (and burn sound).
    LavaHurt,
    /// Before extinguishing: powder snow melts under a burning entity.
    SnowMelt([i32; 3]),
}

/// Effects of a step being collected.
#[derive(Debug, Default)]
struct Step {
    freeze: bool,
    clear_freeze: bool,
    fire: bool,
    lava: bool,
    extinguish: bool,
    in_fire: Vec<f32>,
    lava_hurt: u32,
    melt: Vec<[i32; 3]>,
}

impl Step {
    /// Empties the step into `out`, type by type (before, the effect, after).
    fn flush(&mut self, out: &mut Vec<Late>) {
        if std::mem::take(&mut self.freeze) {
            out.push(Late::Freeze);
        }
        if std::mem::take(&mut self.clear_freeze) {
            out.push(Late::ClearFreeze);
        }
        if std::mem::take(&mut self.fire) {
            out.push(Late::FireIgnite);
        }
        out.extend(self.in_fire.drain(..).map(Late::InFire));
        if std::mem::take(&mut self.lava) {
            out.push(Late::LavaIgnite);
        }
        out.extend((0..std::mem::take(&mut self.lava_hurt)).map(|_| Late::LavaHurt));
        out.extend(self.melt.drain(..).map(Late::SnowMelt));
        if std::mem::take(&mut self.extinguish) {
            out.push(Late::Extinguish);
        }
    }
}

/// Buffers reused from one pass to the next (no allocation in steady state).
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    moves: Vec<Move>,
    /// Cells seen by a traversal, cells kept over all the movements of the tick.
    seen: Seen,
    visited: Vec<[i32; 3]>,
    hits: Vec<Hit>,
    step: Step,
    late: Vec<Late>,
}

/// Entity subject to the pass.
pub(crate) enum Who<'a> {
    Player,
    Mob(&'a mut Mob),
    /// G5-F05: ground item and experience orb (not living: no campfire, no freezing; the
    /// movements are those of their physics).
    Item(&'a mut crate::drops::ItemEnt),
    Orb(&'a mut crate::drops::Orb),
}

/// Result of the pass useful to the rest of the tick.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Outcome {
    /// In powder snow this tick (freezing does not melt).
    pub in_powder_snow: bool,
}

impl Server {
    /// Player outside the pass (spectator, dead, dimension change): movements forgotten, the
    /// next pass starts from the current position.
    pub(crate) fn inside_forget_player(&mut self) {
        self.life.inside.clear();
        self.life.inside_old = None;
    }

    /// Accepted displacement of the player (movement packet), read back by the next pass.
    pub(crate) fn inside_record_player(&mut self, before: [f64; 3]) {
        let to = self.pos;
        if to != before && self.teleport.is_none() {
            let d = [to[0] - before[0], to[1] - before[1], to[2] - before[2]];
            self.life.inside.record(before, to, d);
        }
    }

    /// The crossed-block pass for `who` (see the module header).
    pub(crate) fn inside_blocks(&mut self, who: &mut Who) -> Outcome {
        self.inside_blocks_skip(who, true)
    }

    /// The pass, without the shortcut of the inert items and orbs (equivalence tests, bench).
    #[cfg(test)]
    pub(crate) fn inside_blocks_full(&mut self, who: &mut Who) -> Outcome {
        self.inside_blocks_skip(who, false)
    }

    /// The shortcut would skip this item (tests).
    #[cfg(test)]
    pub(crate) fn item_inert(&self, d: &mut crate::drops::ItemEnt) -> bool {
        self.inert_object(&Who::Item(d))
    }

    fn inside_blocks_skip(&mut self, who: &mut Who, skip: bool) -> Outcome {
        // Decided before taking the buffers out (nothing copied for an inert item).
        if skip && self.inert_object(who) {
            return Outcome::default();
        }
        let mut sc = std::mem::take(&mut self.mobs.inside);
        let out = self.inside_blocks_with(who, &mut sc);
        self.mobs.inside = sc;
        out
    }

    fn inside_blocks_with(&mut self, who: &mut Who, sc: &mut Scratch) -> Outcome {
        let mut out = Outcome::default();
        let pos = match who {
            Who::Player => {
                let (old, pos) = (self.life.inside_old.unwrap_or(self.pos), self.pos);
                self.life.inside.finish(old, pos, &mut sc.moves);
                self.life.inside_old = Some(pos);
                pos
            }
            Who::Mob(m) => {
                m.inside.finish(m.old, m.pos, &mut sc.moves);
                m.pos
            }
            Who::Item(d) => body_moves(&d.body, &mut sc.moves),
            Who::Orb(o) => body_moves(&o.body, &mut sc.moves),
        };
        let was_on_fire = self.who_on_fire(who);
        let was_freezing = self.who_frozen(who) > 0;
        let fire_before = self.who_fire(who);
        // Traversal (read-only), then direct effects in cell order.
        let (bbox, falling) = match who {
            Who::Player => (self.player_aabb(), self.life.vit.fall > 2.5),
            Who::Mob(m) => (m.bbox(), m.fall_distance > 2.5),
            Who::Item(d) => (d.body.bounding_box(), d.body.fall > 2.5),
            Who::Orb(o) => (o.body.bounding_box(), o.body.fall > 2.5),
        };
        self.collect_hits(&bbox, pos, falling, sc);
        let mut last_step = -1;
        sc.late.clear();
        for h in &sc.hits {
            if !self.who_alive(who) {
                break;
            }
            if h.step != last_step {
                last_step = h.step;
                sc.step.flush(&mut sc.late);
            }
            match h.blk {
                Some(Blk::Cactus) => {
                    self.who_hurt(who, "minecraft:cactus", 1.0);
                }
                // The campfire only burns the living.
                Some(Blk::Campfire(d)) if matches!(who, Who::Player | Who::Mob(_)) => {
                    self.who_hurt(who, "minecraft:campfire", d);
                }
                Some(Blk::Fire(d)) => {
                    sc.step.clear_freeze = true;
                    sc.step.fire = true;
                    sc.step.in_fire.push(d);
                }
                Some(Blk::PowderSnow) => {
                    sc.step.melt.push(h.p);
                    sc.step.freeze = true;
                    sc.step.extinguish = true;
                }
                Some(Blk::Plain | Blk::Campfire(_)) | None => {}
            }
            match h.fluid {
                Fl::Water => sc.step.extinguish = true,
                Fl::Lava => {
                    sc.step.clear_freeze = true;
                    sc.step.lava = true;
                    sc.step.lava_hurt += 1;
                }
                Fl::None => {}
            }
        }
        sc.step.flush(&mut sc.late);
        // Deferred effects, as long as the entity lives.
        for &e in &sc.late {
            if !self.who_alive(who) {
                break;
            }
            self.late_effect(who, e, &mut out);
        }
        // Contacts with the other crossed blocks (berries, cobweb, wither rose, cauldrons, plates,
        // tripwire: `contact`), deferred effects after these ones; the rain, the extinguish sound
        // and the return to rest below also see them.
        match who {
            Who::Player => self.player_contacts(),
            Who::Mob(m) => self.mob_contacts(m),
            // Plates and tripwire: right after, by the item and orb ticks.
            Who::Item(_) | Who::Orb(_) => {}
        }
        if self.who_fire(who) > 0 && self.who_in_rain(who) {
            self.who_clear_fire(who);
        }
        if (was_on_fire && !self.who_on_fire(who)) || (was_freezing && self.who_frozen(who) <= 0) {
            self.extinguish_sound(who);
        }
        let ignited = self.who_fire(who) > fire_before;
        if !self.who_on_fire(who) && !ignited {
            let rest = if let Who::Player = who { -20 } else { 0 };
            self.who_set_fire(who, rest);
        }
        out
    }

    /// G5-F05, skip the empty: an item or orb that does not burn can only be acted on by fire,
    /// soul fire, cactus and lava (water, powder snow and rain only put out, the campfire and the
    /// freezing are for the living). None of them in the cells of the boxes of its movements: the
    /// pass would change nothing. `RUSTCRAFT_OPT_ITEM_INSIDE_FULL=1`: full pass always.
    fn inert_object(&self, who: &Who) -> bool {
        let (fire, body) = match who {
            Who::Item(d) => (d.fire, &d.body),
            Who::Orb(o) => (o.fire, &o.body),
            Who::Player | Who::Mob(_) => return false,
        };
        if fire != 0 || rc_data::env_flag!("RUSTCRAFT_OPT_ITEM_INSIDE_FULL") {
            return false;
        }
        let moves = body.effect_moves();
        if moves.is_empty() {
            return true;
        }
        let (half, h) = (body.width / 2.0, body.height);
        let eps = f64::from(1.0e-5f32);
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for &(from, to, _) in moves {
            for p in [from, to] {
                let b = [p[0] - half + eps, p[1] + eps, p[2] - half + eps, p[0] + half - eps, p[1] + h - eps, p[2] + half - eps];
                for a in 0..3 {
                    lo[a] = lo[a].min(b[a].floor() as i32);
                    hi[a] = hi[a].max(b[a + 3].floor() as i32);
                }
            }
        }
        // Long path (more than 64 cells in its envelope): the full pass decides.
        let n = (0..3).map(|a| i64::from(hi[a]) - i64::from(lo[a]) + 1).try_fold(1i64, i64::checked_mul);
        let n = n.unwrap_or(i64::MAX);
        if n > 64 {
            return false;
        }
        let id = &self.bs.t.id;
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let st = self.st([x, y, z]);
                    let b = self.data.states.get(st as usize).map_or(u16::MAX, |i| i.block);
                    if b == id.fire || b == id.soul_fire || b == id.cactus || b == id.lava {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Cells touched along the movements of the tick (`sc.moves`), stored in `sc.hits`.
    fn collect_hits(&self, bbox: &Aabb, pos: [f64; 3], falling: bool, sc: &mut Scratch) {
        sc.hits.clear();
        let id = &self.bs.t.id;
        let box_at = |p: [f64; 3]| bbox.offset(p[0] - pos[0], p[1] - pos[1], p[2] - pos[2]);
        let eps = f64::from(1.0e-5f32);
        let mut visited = std::mem::take(&mut sc.visited);
        let mut seen = std::mem::take(&mut sc.seen);
        visited.clear();
        // One movement, from `from` to `to`, capped at `max` steps: returns the last step + 1.
        let mut check = |from: [f64; 3], to: [f64; 3], max: i32, hits: &mut Vec<Hit>| -> i32 {
            let target = box_at(to);
            let at = Aabb { min: target.min.map(|c| c + eps), max: target.max.map(|c| c - eps) };
            let travel = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
            let start = box_at(from);
            let mut last = 0;
            each_cell_stepped(from, to, &at, &mut seen, &mut |p, step| {
                if step >= max {
                    return false;
                }
                last = step;
                let st = self.st(p);
                // State record read once (air, block) without going through the view.
                let info = self.data.states.get(st as usize).copied().unwrap_or_default();
                if info.has(rc_data::flags::AIR) {
                    return true;
                }
                let b = info.block;
                let blk = if b == id.powder_snow && falling {
                    // Falling: low shape (0.9), touched only if the sliding box reaches it.
                    let o = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
                    let shape = Aabb { min: o, max: [o[0] + 1.0, o[1] + f64::from(0.9f32), o[2] + 1.0] };
                    crate::mobs::nether::ghast::collided_along(&start, travel, &[shape]).then_some(Blk::PowderSnow)
                } else if b == id.powder_snow {
                    Some(Blk::PowderSnow)
                } else if b == id.fire {
                    Some(Blk::Fire(1.0))
                } else if b == id.soul_fire {
                    Some(Blk::Fire(2.0))
                } else if b == id.cactus {
                    Some(Blk::Cactus)
                } else if (b == id.campfire || b == id.soul_campfire) && self.v().get(st, "lit") == Some("true") {
                    Some(Blk::Campfire(if b == id.soul_campfire { 2.0 } else { 1.0 }))
                } else {
                    Some(Blk::Plain)
                };
                let fluid = match self.phys.fluids.fluid(st) {
                    rc_physics::Fluid::None => Fl::None,
                    f => {
                        let above = self.st([p[0], p[1] + 1, p[2]]);
                        let h = f64::from(self.phys.fluids.height_at(st, above));
                        let o = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
                        let aabb = Aabb { min: o, max: [o[0] + 1.0, o[1] + h, o[2] + 1.0] };
                        if crate::mobs::nether::ghast::collided_along(&start, travel, &[aabb]) {
                            if f == rc_physics::Fluid::Lava { Fl::Lava } else { Fl::Water }
                        } else {
                            Fl::None
                        }
                    }
                };
                if (blk.is_some() || fluid != Fl::None) && !visited.contains(&p) {
                    visited.push(p);
                    hits.push(Hit { p, step, blk, fluid });
                }
                true
            });
            last + 1
        };
        for i in 0..sc.moves.len() {
            let mv = sc.moves[i];
            let travel = [mv.to[0] - mv.from[0], mv.to[1] - mv.from[1], mv.to[2] - mv.from[2]];
            let mut left = MAX_STEPS;
            match mv.wanted {
                Some(w) if travel.iter().any(|&c| c != 0.0) => {
                    // Vertical first, then the dominant horizontal axis last.
                    let order = if w[0].abs() < w[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
                    let mut p = mv.from;
                    for a in order {
                        if travel[a] != 0.0 {
                            let mut q = p;
                            q[a] += travel[a];
                            left -= check(p, q, left, &mut sc.hits);
                            p = q;
                        }
                    }
                }
                _ => left -= check(mv.from, mv.to, MAX_STEPS, &mut sc.hits),
            }
            if left <= 0 {
                check(mv.to, mv.to, 1, &mut sc.hits);
            }
        }
        drop(check);
        sc.seen = seen;
        sc.visited = visited;
    }

    fn late_effect(&mut self, who: &mut Who, e: Late, out: &mut Outcome) {
        match e {
            Late::Freeze => {
                out.in_powder_snow = true;
                if self.who_can_freeze(who) {
                    let f = (self.who_frozen(who) + 1).min(FREEZE_REQUIRED);
                    self.who_set_frozen(who, f);
                }
            }
            Late::ClearFreeze => self.who_set_frozen(who, 0),
            Late::FireIgnite => {
                if self.who_fire_immune(who) {
                    return;
                }
                let f = self.who_fire(who);
                if f < 0 {
                    self.who_set_fire(who, f + 1);
                } else if let Who::Player = who {
                    // Player already burning: 1 or 2 more ticks (world random draw).
                    let add = 1 + self.mobs.rng.next_int_bound(2);
                    self.who_set_fire(who, f + add);
                }
                if self.who_fire(who) >= 0 {
                    self.who_ignite(who, 160);
                }
            }
            Late::LavaIgnite => {
                if !self.who_fire_immune(who) {
                    self.who_ignite(who, 300);
                }
            }
            Late::Extinguish => self.who_clear_fire(who),
            Late::InFire(d) => {
                self.who_hurt(who, "minecraft:in_fire", d);
            }
            Late::LavaHurt => {
                if self.who_fire_immune(who) || !self.who_hurt(who, "minecraft:lava", 4.0) {
                    return;
                }
                match who {
                    Who::Player => {
                        let pitch = 2.0 + self.life.rng.next_float() * 0.4;
                        self.play_sound("minecraft:entity.generic.burn", 7, self.pos, 0.4, pitch);
                    }
                    // Silent: neither sound nor pitch draw (the mob's random stays intact).
                    Who::Mob(m) if !m.x.silent => {
                        let pitch = 2.0 + m.rng.next_float() * 0.4;
                        self.mob_sound_unmuted(m, "minecraft:entity.generic.burn", 0.4, pitch);
                    }
                    Who::Mob(_) => {}
                    // An item only hisses every 10 ticks, or when the lava destroys it.
                    Who::Item(d) if d.health <= 0 || d.ticks % 10 == 0 => {
                        let pitch = 2.0 + entity_rng(d.id, d.ticks, 1).next_float() * 0.4;
                        self.play_sound("minecraft:entity.generic.burn", 6, d.pos, 0.4, pitch);
                    }
                    Who::Item(_) => {}
                    Who::Orb(o) => {
                        let pitch = 2.0 + entity_rng(o.id, o.ticks, 1).next_float() * 0.4;
                        self.play_sound("minecraft:entity.generic.burn", 8, o.pos, 0.4, pitch);
                    }
                }
            }
            Late::SnowMelt(p) => {
                let player = matches!(who, Who::Player);
                if self.who_on_fire(who)
                    && (player || self.rule("minecraft:mob_griefing", true))
                    && (!player || Self::block_in_border(self.border_bounds(), p))
                {
                    self.destroy_block(p, false, 512);
                }
            }
        }
    }

    /// Extinguish sound (fire or freezing that stops) at the entity's position, even if silent.
    fn extinguish_sound(&mut self, who: &mut Who) {
        const NAME: &str = "minecraft:entity.generic.extinguish_fire";
        match who {
            Who::Player => {
                let r = &mut self.life.rng;
                let pitch = 1.6 + (r.next_float() - r.next_float()) * 0.4;
                self.play_sound(NAME, 7, self.pos, 0.7, pitch);
            }
            Who::Mob(m) => {
                let pitch = 1.6 + (m.rng.next_float() - m.rng.next_float()) * 0.4;
                self.mob_sound_unmuted(m, NAME, 0.7, pitch);
            }
            // Neutral category for an item, ambient for an orb.
            Who::Item(d) => {
                let mut r = entity_rng(d.id, d.ticks, 2);
                let pitch = 1.6 + (r.next_float() - r.next_float()) * 0.4;
                self.play_sound(NAME, 6, d.pos, 0.7, pitch);
            }
            Who::Orb(o) => {
                let mut r = entity_rng(o.id, o.ticks, 2);
                let pitch = 1.6 + (r.next_float() - r.next_float()) * 0.4;
                self.play_sound(NAME, 8, o.pos, 0.7, pitch);
            }
        }
    }

    /// Rain at the feet block or at the block at the top of the box (same column).
    pub(crate) fn in_rain(&self, pos: [f64; 3], top: f64) -> bool {
        let b = crate::mobs::mth::block_of(pos);
        self.is_raining_at(b) || self.is_raining_at([b[0], top.floor() as i32, b[2]])
    }

    fn who_in_rain(&self, who: &Who) -> bool {
        match who {
            Who::Player => self.in_rain(self.pos, self.player_aabb().max[1]),
            Who::Mob(m) => self.in_rain(m.pos, m.bbox().max[1]),
            Who::Item(d) => self.in_rain(d.pos, d.body.bounding_box().max[1]),
            Who::Orb(o) => self.in_rain(o.pos, o.body.bounding_box().max[1]),
        }
    }

    fn who_alive(&self, who: &Who) -> bool {
        match who {
            Who::Player => !self.life.vit.dead && self.life.vit.health > 0.0,
            Who::Mob(m) => m.alive(),
            Who::Item(d) => !d.is_removed(),
            Who::Orb(o) => !o.is_removed(),
        }
    }

    fn who_fire(&self, who: &Who) -> i32 {
        match who {
            Who::Player => self.life.vit.fire,
            Who::Mob(m) => m.fire,
            Who::Item(d) => d.fire,
            Who::Orb(o) => o.fire,
        }
    }

    fn who_set_fire(&mut self, who: &mut Who, v: i32) {
        match who {
            Who::Player => self.life.vit.fire = v,
            Who::Mob(m) => m.fire = v,
            Who::Item(d) => d.fire = v,
            Who::Orb(o) => o.fire = v,
        }
    }

    fn who_clear_fire(&mut self, who: &mut Who) {
        let f = self.who_fire(who).min(0);
        self.who_set_fire(who, f);
    }

    fn who_fire_immune(&self, who: &Who) -> bool {
        match who {
            Who::Player | Who::Orb(_) => false,
            Who::Mob(m) => mob_fire_immune(m),
            // A stack that resists "in fire" (netherite).
            Who::Item(d) => self.item_resists(&d.stack, "minecraft:in_fire"),
        }
    }

    fn who_on_fire(&self, who: &Who) -> bool {
        !self.who_fire_immune(who) && self.who_fire(who) > 0
    }

    /// Freezing counter (items and orbs: never counted, nothing perceptible).
    fn who_frozen(&self, who: &Who) -> i32 {
        match who {
            Who::Player => self.life.vit.ticks_frozen,
            Who::Mob(m) => m.ticks_frozen,
            Who::Item(_) | Who::Orb(_) => 0,
        }
    }

    fn who_set_frozen(&mut self, who: &mut Who, v: i32) {
        match who {
            Who::Player => self.life.vit.ticks_frozen = v,
            Who::Mob(m) => m.ticks_frozen = v,
            Who::Item(_) | Who::Orb(_) => {}
        }
    }

    fn who_can_freeze(&self, who: &Who) -> bool {
        match who {
            Who::Player => self.player_can_freeze(),
            Who::Mob(m) => self.mob_can_freeze(m),
            Who::Item(_) | Who::Orb(_) => false,
        }
    }

    fn who_hurt(&mut self, who: &mut Who, ty: &str, amount: f32) -> bool {
        match who {
            Who::Player => self.hurt(ty, amount),
            Who::Mob(m) => self.mob_hurt(m, ty, amount, None, None),
            // Fire damage on a fire-immune item: refused (resistance to "in fire").
            Who::Item(d) => {
                let fire = self.dmg_tag(ty, "minecraft:is_fire");
                !(fire && self.item_resists(&d.stack, "minecraft:in_fire")) && self.item_hurt_ent(d, ty, amount, false)
            }
            Who::Orb(o) => {
                crate::drops::orb_take_damage(o, amount);
                true
            }
        }
    }

    fn who_ignite(&mut self, who: &mut Who, ticks: i32) {
        match who {
            Who::Player => self.player_ignite(ticks),
            Who::Mob(m) => mob_ignite(m, ticks),
            // No burning-time attribute: the duration as is, kept if longer.
            Who::Item(d) => d.fire = d.fire.max(ticks),
            Who::Orb(o) => o.fire = o.fire.max(ticks),
        }
    }
}

/// Movements of an item's or orb's last physics tick into `out`; returns its position.
fn body_moves(b: &rc_physics::body::Body, out: &mut Vec<Move>) -> [f64; 3] {
    out.clear();
    out.extend(b.effect_moves().iter().map(|&(from, to, wanted)| Move { from, to, wanted }));
    b.pos
}

/// Own random of an item or orb for a sound pitch (`salt`: kind of draw), seeded like its
/// physics draws: the world random stays untouched.
pub(crate) fn entity_rng(id: i32, ticks: i32, salt: i64) -> crate::mobs::jrand::JRand {
    crate::mobs::jrand::JRand::new((i64::from(id) << 32 ^ i64::from(ticks)) ^ salt << 56)
}

/// Mob immune to fire (species, or creaking and the like).
pub(crate) fn mob_fire_immune(m: &Mob) -> bool {
    m.kind.info().fire_immune || crate::mobs::recent::fire_immune(m)
}

/// Ignition of a living mob: duration × `burning_time` (rounded up), kept if
/// longer, freezing cleared.
pub(crate) fn mob_ignite(m: &mut Mob, ticks: i32) {
    let k = m.attrs.by_name("minecraft:burning_time").map_or(1.0, |a| a.value());
    let t = (f64::from(ticks) * k).ceil() as i32;
    if m.fire < t {
        m.fire = t;
    }
    m.ticks_frozen = 0;
}
