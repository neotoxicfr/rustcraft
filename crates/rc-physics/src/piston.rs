//! Piston moving blocks, shared by the server and the client: collision shape
//! (depending on progress, and empty in the direction of the push for the pushed entity), area swept
//! in one tick and push distance of an entity, resetting out of the retracting base, sticky
//! zone of the honey block, cap of 0.51 block per axis and per tick.

use rc_data::VersionData;

use crate::{Aabb, Tables, World};

/// Step per direction (0 down, 1 up, 2 north, 3 south, 4 west, 5 east).
pub const STEP: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Opposite direction (the pairs follow each other).
#[inline]
pub fn opposite(d: u8) -> u8 {
    d ^ 1
}

/// Axis of a direction: 0 x, 1 y, 2 z.
#[inline]
pub fn axis(d: u8) -> usize {
    match d {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 0,
    }
}

/// Moving block (block entity `minecraft:piston`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moving {
    pub moved: u32,
    /// Direction of the piston.
    pub dir: u8,
    pub extending: bool,
    pub source: bool,
    pub progress: f32,
}

impl Moving {
    /// Direction of the block's movement.
    pub fn movement(&self) -> u8 {
        if self.extending { self.dir } else { opposite(self.dir) }
    }

    fn extended_progress(&self, p: f32) -> f32 {
        if self.extending { p - 1.0 } else { 1.0 - p }
    }

    /// Offset of the block at the current progress.
    fn offset(&self) -> [f64; 3] {
        let k = f64::from(self.extended_progress(self.progress));
        let s = STEP[usize::from(self.dir)];
        [k * f64::from(s[0]), k * f64::from(s[1]), k * f64::from(s[2])]
    }
}

/// Piston states useful to the physics.
#[derive(Debug, Clone)]
pub struct PistonStates {
    moving_piston: u16,
    slime: u16,
    honey: u16,
    /// Bases: state, direction, sticky, same extended state.
    bases: Vec<(u32, u8, bool, u32)>,
    /// Heads: direction × 4 + sticky × 2 + short.
    heads: [u32; 24],
}

impl Default for PistonStates {
    /// No known piston (test tables).
    fn default() -> Self {
        PistonStates { moving_piston: u16::MAX, slime: u16::MAX, honey: u16::MAX, bases: Vec::new(), heads: [0; 24] }
    }
}

fn dir_of(name: &str) -> Option<u8> {
    Some(match name {
        "down" => 0,
        "up" => 1,
        "north" => 2,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => return None,
    })
}

impl PistonStates {
    pub fn new(d: &VersionData) -> Self {
        let id = |n: &str| d.blocks.iter().position(|b| b.name == n).map_or(u16::MAX, |i| i as u16);
        let mut p = PistonStates {
            moving_piston: id("minecraft:moving_piston"),
            slime: id("minecraft:slime_block"),
            honey: id("minecraft:honey_block"),
            bases: Vec::new(),
            heads: [0; 24],
        };
        let prop = |s: u32, k: &str| d.state_properties(s).find(|e| e.0 == k).map(|e| e.1);
        for (name, sticky) in [("minecraft:piston", false), ("minecraft:sticky_piston", true)] {
            let Some(b) = d.blocks.iter().find(|b| b.name == name) else { continue };
            let states: Vec<u32> = (b.first_state..b.first_state + b.state_count).collect();
            for &s in &states {
                let Some(f) = prop(s, "facing").and_then(dir_of) else { continue };
                let ext = states
                    .iter()
                    .copied()
                    .find(|&e| prop(e, "facing") == prop(s, "facing") && prop(e, "extended") == Some("true"))
                    .unwrap_or(s);
                p.bases.push((s, f, sticky, ext));
            }
        }
        if let Some(b) = d.blocks.iter().find(|b| b.name == "minecraft:piston_head") {
            for s in b.first_state..b.first_state + b.state_count {
                let Some(f) = prop(s, "facing").and_then(dir_of) else { continue };
                let sticky = prop(s, "type") == Some("sticky");
                let short = prop(s, "short") == Some("true");
                p.heads[usize::from(f) * 4 + usize::from(sticky) * 2 + usize::from(short)] = s;
            }
        }
        p
    }

    /// Piston head state.
    pub fn head(&self, facing: u8, sticky: bool, short: bool) -> u32 {
        self.heads[usize::from(facing % 6) * 4 + usize::from(sticky) * 2 + usize::from(short)]
    }

    /// Piston base: direction, sticky, same extended state.
    pub fn base(&self, s: u32) -> Option<(u8, bool, u32)> {
        self.bases.iter().find(|b| b.0 == s).map(|b| (b.1, b.2, b.3))
    }

    /// Piston head: direction and sticky.
    pub fn head_of(&self, s: u32) -> Option<(u8, bool)> {
        let i = self.heads.iter().position(|&h| h == s && s != 0)?;
        Some(((i / 4) as u8, i % 4 >= 2))
    }
}

/// Area swept by a moving block during a tick.
#[derive(Debug, Clone)]
pub struct Sweep {
    /// Direction of the movement.
    pub movement: u8,
    /// Progress advance this tick.
    pub delta: f64,
    /// Entity search zone.
    pub area: Aabb,
    /// Boxes of the shape, each stretched over the advance.
    boxes: Vec<Aabb>,
    /// Slime block: the entity is launched in the direction of the movement.
    pub bounce: bool,
    /// Retracting base: the entity is then reset out of the base block.
    pub fix_base: bool,
}

/// Strip of width `amount` glued to the face of `b` in direction `d`.
pub fn movement_area(b: &Aabb, d: u8, amount: f64) -> Aabb {
    let sign = if matches!(d, 1 | 3 | 5) { 1.0 } else { -1.0 };
    let delta = amount * sign;
    let (lo, hi) = (delta.min(0.0), delta.max(0.0));
    let a = axis(d);
    let mut out = *b;
    let face = if sign > 0.0 { b.max[a] } else { b.min[a] };
    out.min[a] = face + lo;
    out.max[a] = face + hi;
    out
}

/// Distance that takes `b` out of `outside` in direction `d`.
fn movement_to_leave(outside: &Aabb, d: u8, b: &Aabb) -> f64 {
    match d {
        5 => outside.max[0] - b.min[0],
        4 => b.max[0] - outside.min[0],
        0 => b.max[1] - outside.min[1],
        3 => outside.max[2] - b.min[2],
        2 => b.max[2] - outside.min[2],
        _ => outside.max[1] - b.min[1],
    }
}

fn union(a: &Aabb, b: &Aabb) -> Aabb {
    Aabb { min: std::array::from_fn(|i| a.min[i].min(b.min[i])), max: std::array::from_fn(|i| a.max[i].max(b.max[i])) }
}

fn shift(b: &Aabb, o: [f64; 3]) -> Aabb {
    b.offset(o[0], o[1], o[2])
}

impl Sweep {
    /// Push distance of the entity of box `e` (already sorted by the zone), 0.01 included;
    /// `None` if no box touches it.
    pub fn push_of(&self, e: &Aabb) -> Option<f64> {
        let mut delta: f64 = 0.0;
        for b in &self.boxes {
            if b.intersects(e) {
                delta = delta.max(movement_to_leave(b, self.movement, e));
                if delta >= self.delta {
                    break;
                }
            }
        }
        (delta > 0.0).then(|| delta.min(self.delta) + 0.01)
    }
}

/// Reset out of the retracting base: distance to travel in the
/// opposite direction of `movement`, if the entity only exceeds the base in that direction.
pub fn base_fix(pos: [i32; 3], e: &Aabb, movement: u8, delta: f64) -> Option<f64> {
    let p = pos.map(f64::from);
    let cell = Aabb { min: p, max: [p[0] + 1.0, p[1] + 1.0, p[2] + 1.0] };
    if !e.intersects(&cell) {
        return None;
    }
    let back = opposite(movement);
    let inter = Aabb {
        min: std::array::from_fn(|i| e.min[i].max(cell.min[i])),
        max: std::array::from_fn(|i| e.max[i].min(cell.max[i])),
    };
    let whole = movement_to_leave(&cell, back, e) + 0.01;
    let part = movement_to_leave(&cell, back, &inter) + 0.01;
    ((whole - part).abs() < 0.01).then(|| whole.min(delta) + 0.01)
}

/// Cap of the piston pushes: at most 0.51 block per axis and per
/// game tick, total reset at each new tick; a single axis at a time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PistonDeltas {
    time: i64,
    d: [f64; 3],
}

impl PistonDeltas {
    pub fn limit(&mut self, time: i64, v: [f64; 3]) -> [f64; 3] {
        if v[0] * v[0] + v[1] * v[1] + v[2] * v[2] <= 1.0e-7 {
            return v;
        }
        if time != self.time {
            self.d = [0.0; 3];
            self.time = time;
        }
        let Some(a) = (0..3).find(|&a| v[a] != 0.0) else { return [0.0; 3] };
        let total = (v[a] + self.d[a]).clamp(-0.51, 0.51);
        let amount = total - self.d[a];
        self.d[a] = total;
        let mut out = [0.0; 3];
        if amount.abs() > f64::from(1.0e-5f32) {
            out[a] = amount;
        }
        out
    }
}

impl Tables {
    /// The state is a piston moving block.
    #[inline]
    pub fn is_moving_piston(&self, s: u32) -> bool {
        self.block.get(s as usize).is_some_and(|&b| b == self.piston.moving_piston)
    }

    /// Collision shape of a moving block (local to its cell, may overflow it);
    /// `noclip`: direction of the push in progress, in which the block does not block.
    pub fn moving_collision(&self, m: &Moving, noclip: Option<u8>, out: &mut Vec<Aabb>) {
        if !m.extending
            && m.source
            && let Some((_, _, ext)) = self.piston.base(m.moved)
        {
            self.collision(ext, out);
        }
        if m.progress < 1.0 && noclip == Some(m.movement()) {
            return;
        }
        // Head: always of the normal type; short if `extending != (1 - p < 0.25)`.
        let state =
            if m.source { self.piston.head(m.dir, false, m.extending != (1.0 - m.progress < 0.25)) } else { m.moved };
        let o = m.offset();
        let from = out.len();
        self.collision(state, out);
        for b in &mut out[from..] {
            *b = shift(b, o);
        }
    }

    /// State whose shape pushes the entities: the head (short past 0.25) for the retracting
    /// base, otherwise the moved state.
    fn pushing_state(&self, m: &Moving) -> u32 {
        match self.piston.base(m.moved) {
            Some((facing, sticky, _)) if !m.extending && m.source => {
                self.piston.head(facing, sticky, m.progress > 0.25)
            }
            _ => m.moved,
        }
    }

    /// Area swept by the moving block at `pos` going to `new_progress`.
    pub fn piston_sweep(&self, pos: [i32; 3], m: &Moving, new_progress: f32) -> Option<Sweep> {
        let mut shape = Vec::new();
        self.collision(self.pushing_state(m), &mut shape);
        let first = shape.first()?;
        let movement = m.movement();
        let delta = f64::from(new_progress - m.progress);
        let o = m.offset();
        let o = [f64::from(pos[0]) + o[0], f64::from(pos[1]) + o[1], f64::from(pos[2]) + o[2]];
        let bounds = shape.iter().fold(*first, |a, b| union(&a, b));
        let at = shift(&bounds, o);
        let area = union(&movement_area(&at, movement, delta), &at);
        let boxes = shape.iter().map(|b| movement_area(&shift(b, o), movement, delta)).collect();
        let block = self.block.get(m.moved as usize).copied();
        Some(Sweep {
            movement,
            delta,
            area,
            boxes,
            bounce: block == Some(self.piston.slime),
            fix_base: !m.extending && m.source,
        })
    }

    /// Honey block moved horizontally: zone above it where the
    /// entities on the ground are dragged along, direction and distance (without margin).
    pub fn honey_carry(&self, pos: [i32; 3], m: &Moving, new_progress: f32) -> Option<(Aabb, u8, f64)> {
        if self.block.get(m.moved as usize).copied() != Some(self.piston.honey) {
            return None;
        }
        let movement = m.movement();
        if axis(movement) == 1 {
            return None;
        }
        let mut shape = Vec::new();
        self.collision(m.moved, &mut shape);
        let top = shape.iter().map(|b| b.max[1]).fold(f64::NEG_INFINITY, f64::max);
        let o = m.offset();
        let o = [f64::from(pos[0]) + o[0], f64::from(pos[1]) + o[1], f64::from(pos[2]) + o[2]];
        let b = shift(&Aabb { min: [0.0, top, 0.0], max: [1.0, 1.500_001_000_000_000_1, 1.0] }, o);
        Some((b, movement, f64::from(new_progress - m.progress)))
    }
}

/// Movement of `d` in direction `dir`.
pub fn along(dir: u8, d: f64) -> [f64; 3] {
    let s = STEP[usize::from(dir % 6)];
    [d * f64::from(s[0]), d * f64::from(s[1]), d * f64::from(s[2])]
}

/// Entity that the pistons push: its box, its supports and its piston movement (which
/// ignores the pushing block in its direction, `noclip`).
pub trait PistonTarget {
    fn bbox(&self) -> Aabb;
    fn position(&self) -> [f64; 3];
    fn grounded(&self) -> bool;
    fn supported_by(&self, p: [i32; 3]) -> bool;
    /// Launched by the slime: speed on axis `axis` set to `v` (entity simulated on this side).
    fn launch(&mut self, axis: usize, v: f64);
    fn piston_push(&mut self, d: [f64; 3]);
}

/// Push by the swept zone (for one entity): launched by the slime,
/// pushed out of the zone then reset out of the retracting base.
pub fn push_collided(sw: &Sweep, pos: [i32; 3], e: &mut dyn PistonTarget) {
    let b = e.bbox();
    if !sw.area.intersects(&b) {
        return;
    }
    let mv = sw.movement;
    if sw.bounce {
        let a = axis(mv);
        e.launch(a, f64::from(STEP[usize::from(mv)][a]));
    }
    if let Some(d) = sw.push_of(&b) {
        e.piston_push(along(mv, d));
        if sw.fix_base
            && let Some(f) = base_fix(pos, &e.bbox(), mv, sw.delta)
        {
            e.piston_push(along(opposite(mv), f));
        }
    }
}

/// Dragging by the honey (for one entity): on the ground, resting on the
/// block or the centre above the zone.
pub fn carry_stuck(carry: &(Aabb, u8, f64), pos: [i32; 3], e: &mut dyn PistonTarget) {
    let (area, mv, d) = carry;
    let p = e.position();
    let over = (area.min[0]..=area.max[0]).contains(&p[0]) && (area.min[2]..=area.max[2]).contains(&p[2]);
    if e.bbox().intersects(area) && e.grounded() && (e.supported_by(pos) || over) {
        e.piston_push(along(*mv, *d));
    }
}

/// The local player pushed, at game tick `time`, in world `w` (with `noclip`).
struct PlayerTarget<'a> {
    p: &'a mut crate::Player,
    time: i64,
    w: &'a dyn World,
    t: &'a Tables,
}

impl PistonTarget for PlayerTarget<'_> {
    fn bbox(&self) -> Aabb {
        self.p.bounding_box()
    }
    fn position(&self) -> [f64; 3] {
        self.p.pos
    }
    fn grounded(&self) -> bool {
        self.p.on_ground
    }
    fn supported_by(&self, p: [i32; 3]) -> bool {
        self.p.supporting_block() == Some(p)
    }
    fn launch(&mut self, axis: usize, v: f64) {
        self.p.vel[axis] = v;
    }
    fn piston_push(&mut self, d: [f64; 3]) {
        self.p.piston_move(d, self.time, self.w, self.t);
    }
}

/// Item, orb, primed TNT or falling block pushed (shared `body` physics), at game
/// tick `time`, in world `w` (with `noclip`); `g`: gravity of the entity (0.04; orb 0.03).
pub struct BodyTarget<'a> {
    pub b: &'a mut crate::body::Body,
    pub w: &'a dyn World,
    pub t: &'a Tables,
    pub time: i64,
    pub g: f64,
}

impl PistonTarget for BodyTarget<'_> {
    fn bbox(&self) -> Aabb {
        self.b.bounding_box()
    }
    fn position(&self) -> [f64; 3] {
        self.b.pos
    }
    fn grounded(&self) -> bool {
        self.b.on_ground
    }
    fn supported_by(&self, p: [i32; 3]) -> bool {
        self.b.support() == Some(p)
    }
    fn launch(&mut self, axis: usize, v: f64) {
        self.b.vel[axis] = v;
    }
    fn piston_push(&mut self, d: [f64; 3]) {
        self.b.piston_move(d, self.time, self.w, self.t, self.g);
    }
}

impl crate::Player {
    /// Tick of the moving block at `pos` (progress going to `new_progress`) applied to the
    /// player. `w` ignores the blocks that push in the direction of the movement (`noclip`).
    pub fn piston_tick(&mut self, pos: [i32; 3], m: &Moving, new_progress: f32, time: i64, w: &dyn World, t: &Tables) {
        let sweep = t.piston_sweep(pos, m, new_progress);
        let carry = t.honey_carry(pos, m, new_progress);
        let mut e = PlayerTarget { p: self, time, w, t };
        if let Some(sw) = &sweep {
            push_collided(sw, pos, &mut e);
        }
        if let Some(c) = &carry {
            carry_stuck(c, pos, &mut e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stone ground under y = 64, nothing else.
    struct Floor<'a> {
        t: &'a Tables,
        stone: u32,
    }

    impl World for Floor<'_> {
        fn state(&self, _x: i32, y: i32, _z: i32) -> Option<u32> {
            Some(if y < 64 { self.stone } else { 0 })
        }
        fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
            let s = self.state(x, y, z).unwrap_or(0);
            self.t.collision(s, out)
        }
    }

    /// Stone pushed east from (0,64,0) to (1,64,0); player standing right in front (west
    /// edge at 1.2): pushed by 0.31 at the first tick (0.3 + 0.01), then by 0.5 (0.49 capped
    /// at the advance, + 0.01) at the second; he ends at 0.01 from the arrived block.
    #[test]
    fn player_pushed_by_moving_stone() {
        let d = rc_testdata::version_data().expect("version data absent (extraction)");
        let t = Tables::new(&d);
        let stone = d.blocks.iter().find(|b| b.name == "minecraft:stone").expect("stone").default_state;
        let w = Floor { t: &t, stone };
        let mut p = crate::Player::new([1.5, 64.0, 0.5], 0.0, 0.0);
        p.on_ground = true;
        let mut m = Moving { moved: stone, dir: 5, extending: true, source: false, progress: 0.0 };
        p.piston_tick([1, 64, 0], &m, 0.5, 10, &w, &t);
        assert!((p.pos[0] - 1.81).abs() < 1e-6, "{:?}", p.pos);
        m.progress = 0.5;
        p.piston_tick([1, 64, 0], &m, 1.0, 11, &w, &t);
        assert!((p.pos[0] - 2.31).abs() < 1e-6, "{:?}", p.pos);
        assert_eq!(p.pos[1], 64.0);
        // Shape: at mid-course the stone occupies x 0.5..1.5; empty in the direction of the push.
        let mut out = Vec::new();
        t.moving_collision(&m, None, &mut out);
        assert_eq!(out.len(), 1);
        assert!((out[0].min[0] + 0.5).abs() < 1e-9 && (out[0].max[0] - 0.5).abs() < 1e-9, "{out:?}");
        out.clear();
        t.moving_collision(&m, Some(5), &mut out);
        assert!(out.is_empty());
        // Without a block entity, the moving block has no shape: the collision search
        // can skip the "moving block?" test when none moves.
        let moving = d.blocks.iter().find(|b| b.name == "minecraft:moving_piston").expect("moving block");
        for s in moving.first_state..moving.first_state + moving.state_count {
            out.clear();
            assert!(!t.collision(s, &mut out) && out.is_empty(), "state {s}: {out:?}");
        }
    }

    /// Bench world: stone ground; `check` adds the "moving block?" test to each
    /// cell (cost added to the collision searches of the client and the server), `mover` places
    /// a moving block at mid-course at (1, 64, 0).
    struct Bench<'a> {
        t: &'a Tables,
        stone: u32,
        moving: u32,
        check: bool,
        /// Test skipped when no block moves (client counter, empty server table).
        guard: bool,
        mover: Option<Moving>,
    }

    impl Bench<'_> {
        fn at(&self, x: i32, y: i32, z: i32) -> u32 {
            if self.mover.is_some() && [x, y, z] == [1, 64, 0] {
                self.moving
            } else if y < 64 {
                self.stone
            } else {
                0
            }
        }
        fn shape(&self, s: u32, out: &mut Vec<Aabb>) -> bool {
            if !self.t.is_moving_piston(s) {
                return false;
            }
            if let Some(m) = &self.mover {
                self.t.moving_collision(m, None, out);
            }
            true
        }
    }

    impl World for Bench<'_> {
        fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
            Some(self.at(x, y, z))
        }
        fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
            let s = self.at(x, y, z);
            let any = !self.guard || self.mover.is_some();
            (self.check && any && self.shape(s, out)) || self.t.collision(s, out)
        }
        fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
            self.check && self.mover.is_some() && self.shape(self.at(x, y, z), out)
        }
    }

    /// Bench: `collide::resolve` of a player walking on the ground (0.2 in x, −0.08 in y), without the
    /// moving-block test (code from before), with it (no moving block), and with a moving
    /// block next to it.
    #[test]
    #[ignore]
    fn bench_collision_moving_check() {
        let d = rc_testdata::version_data().expect("version data absent (extraction)");
        let t = Tables::new(&d);
        let stone = d.blocks.iter().find(|b| b.name == "minecraft:stone").expect("stone").default_state;
        let moving = d.blocks.iter().find(|b| b.name == "minecraft:moving_piston").expect("moving block").default_state;
        let m = Moving { moved: stone, dir: 5, extending: true, source: false, progress: 0.5 };
        let start = Aabb { min: [-0.8, 64.0, 0.2], max: [-0.2, 65.8, 0.8] };
        let n = 1_000_000;
        let cases = [
            ("before", false, false, None),
            ("after without guard, none", true, false, None),
            ("after with guard, none", true, true, None),
            ("after with guard, block next to it", true, true, Some(m)),
        ];
        for (name, check, guard, mover) in cases {
            let w = Bench { t: &t, stone, moving, check, guard, mover };
            let mut best = std::time::Duration::MAX;
            for _ in 0..5 {
                let t0 = std::time::Instant::now();
                for _ in 0..n {
                    std::hint::black_box(crate::collide::resolve(&w, &start, std::hint::black_box([0.2, -0.08, 0.0]), true, 0.6));
                }
                best = best.min(t0.elapsed());
            }
            println!("{name}: {:.1} ns per resolution", best.as_nanos() as f64 / f64::from(n));
        }
    }

    #[test]
    fn deltas_cap_per_tick_and_axis() {
        let mut d = PistonDeltas::default();
        assert_eq!(d.limit(5, [0.3, 0.0, 0.0]), [0.3, 0.0, 0.0]);
        let v = d.limit(5, [0.3, 0.0, 0.0]);
        assert!((v[0] - 0.21).abs() < 1e-12, "{v:?}");
        assert_eq!(d.limit(5, [0.3, 0.0, 0.0]), [0.0; 3]);
        // Other axis free, new tick reset to zero; a single axis at a time.
        assert_eq!(d.limit(5, [0.0, 0.2, 0.0]), [0.0, 0.2, 0.0]);
        assert_eq!(d.limit(6, [0.3, 0.0, 0.0]), [0.3, 0.0, 0.0]);
        assert_eq!(d.limit(7, [0.2, 0.4, 0.0]), [0.2, 0.0, 0.0]);
    }

    #[test]
    fn movement_area_and_leave() {
        let b = Aabb { min: [0.0, 0.0, 0.0], max: [1.0, 1.0, 1.0] };
        let east = movement_area(&b, 5, 0.5);
        assert_eq!((east.min, east.max), ([1.0, 0.0, 0.0], [1.5, 1.0, 1.0]));
        let west = movement_area(&b, 4, 0.5);
        assert_eq!((west.min, west.max), ([-0.5, 0.0, 0.0], [0.0, 1.0, 1.0]));
        // Entity whose west edge is at 1.2: the strip is (1..1.5) pushes it by 0.3.
        let e = Aabb { min: [1.2, 0.0, 0.2], max: [1.8, 1.8, 0.8] };
        assert!((movement_to_leave(&east, 5, &e) - 0.3).abs() < 1e-12);
    }
}
