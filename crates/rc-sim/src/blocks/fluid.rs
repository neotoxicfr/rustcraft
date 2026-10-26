//! Server-side fluids (§ 2.1): scheduling by liquid blocks, water-lava meeting,
//! new computed fluid, flow downward and sideways (slope search),
//! writing into a cell (waterlogged blocks, destruction), random lava tick.

use hashbrown::HashMap;

use rc_data::{VersionData, sim_bits};

use super::level::Fluid;
use super::{Dir, Kind, P, above, below, flag, rel};
use crate::server::Server;

/// Faces of each shape covered (16 × 16 square per direction), for fluids to pass through.
pub(crate) fn face_masks(d: &VersionData) -> Vec<[[u64; 4]; 6]> {
    (0..d.shapes.len())
        .map(|i| {
            let (a, b) = d.shapes[i];
            masks(&d.aabbs[a as usize..b as usize])
        })
        .collect()
}

fn masks(boxes: &[rc_data::Aabb]) -> [[u64; 4]; 6] {
    let mut out = [[0u64; 4]; 6];
    const E: f64 = 1.0e-6;
    for b in boxes {
        for (dir, mask) in out.iter_mut().enumerate() {
            let (axis, at_max) = match dir {
                0 => (1, false),
                1 => (1, true),
                2 => (2, false),
                3 => (2, true),
                4 => (0, false),
                _ => (0, true),
            };
            let touches = if at_max { b.max[axis] >= 1.0 - E } else { b.min[axis] <= E };
            if !touches {
                continue;
            }
            let (u, v) = match axis {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let lo = |x: f64| ((x * 16.0) + E).floor().clamp(0.0, 16.0) as usize;
            let hi = |x: f64| ((x * 16.0) - E).ceil().clamp(0.0, 16.0) as usize;
            for iu in lo(b.min[u])..hi(b.max[u]) {
                for iv in lo(b.min[v])..hi(b.max[v]) {
                    let bit = iu * 16 + iv;
                    mask[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
    }
    out
}

const FULL: [u64; 4] = [u64::MAX; 4];

impl Server {
    pub(crate) fn tag(&self, name: &str, block: u16) -> bool {
        self.tag_u32(name, u32::from(block), crate::tag_bits::mode())
    }

    pub(crate) fn fast_lava(&self) -> bool {
        self.dim.name == "minecraft:the_nether"
    }

    fn drop_off(&self, kind: u8) -> u8 {
        if kind == 2 && !self.fast_lava() { 2 } else { 1 }
    }

    pub(crate) fn fluid_delay(&self, kind: u8) -> i32 {
        match kind {
            2 => {
                if self.fast_lava() {
                    10
                } else {
                    30
                }
            }
            _ => 5,
        }
    }

    fn slope_distance_max(&self, kind: u8) -> i32 {
        if kind == 2 && !self.fast_lava() { 2 } else { 4 }
    }

    /// Height of a fluid at `p`: 1 if the same fluid is above.
    fn fluid_height(&self, f: Fluid, p: P) -> f32 {
        if f.is_empty() {
            return 0.0;
        }
        if self.fluid_at(above(p)).kind == f.kind { 1.0 } else { f.own_height() }
    }

    /// Passage between two cells (§ 2.1).
    fn pass_wall(&self, dir: Dir, src: u32, dst: u32) -> bool {
        let (si, di) = (self.v().info(src), self.v().info(dst));
        if self.v().full_collision(dst) || self.v().full_collision(src) {
            return false;
        }
        if si.collision == 0 && di.collision == 0 {
            return true;
        }
        let t = &self.bs.t;
        let a = t.face_masks.get(si.collision as usize).map_or([0; 4], |m| m[dir as usize]);
        let b = t.face_masks.get(di.collision as usize).map_or([0; 4], |m| m[dir.opposite() as usize]);
        !(0..4).all(|i| a[i] | b[i] == FULL[i])
    }

    fn can_hold_any(&self, s: u32) -> bool {
        self.v().sim(s) & sim_bits::LIQUID_CONTAINER != 0
            || self.tag_id(crate::tag_bits::TagId::WashedAwayByFluids, self.v().block(s))
    }

    /// `fluid_type` = exact type (`Fluid::type_id`).
    fn can_hold_specific(&self, s: u32, fluid_type: u8) -> bool {
        if self.v().sim(s) & sim_bits::LIQUID_CONTAINER != 0 {
            fluid_type == 1 && self.v().sim(s) & sim_bits::PLACE_WATER != 0
        } else {
            true
        }
    }

    fn is_source_of(&self, f: Fluid, kind: u8) -> bool {
        f.kind == kind && f.source
    }

    fn maybe_pass(&self, kind: u8, src: u32, dir: Dir, ts: u32, tf: Fluid) -> bool {
        !self.is_source_of(tf, kind) && self.can_hold_any(ts) && self.pass_wall(dir, src, ts)
    }

    fn can_pass(&self, kind: u8, src: u32, dir: Dir, ts: u32, tf: Fluid) -> bool {
        self.maybe_pass(kind, src, dir, ts, tf) && self.can_hold_specific(ts, kind * 2)
    }

    fn is_hole(&self, kind: u8, top: u32, bottom: u32) -> bool {
        if !self.pass_wall(Dir::Down, top, bottom) {
            return false;
        }
        let bf = self.fluid_of(bottom);
        bf.kind == kind || (self.can_hold_any(bottom) && self.can_hold_specific(bottom, kind * 2))
    }

    fn convert_to_source(&self, kind: u8) -> bool {
        let rule = if kind == 1 { "minecraft:water_source_conversion" } else { "minecraft:lava_source_conversion" };
        self.rule(rule, kind == 1)
    }

    /// New fluid computed at `p` for fluid `kind`.
    fn new_liquid(&self, kind: u8, p: P, s: u32) -> Fluid {
        let mut highest = 0u8;
        let mut sources = 0;
        for d in Dir::HORIZONTAL {
            let rp = rel(p, d);
            let rs = self.get(rp);
            let f = self.fluid_of(rs);
            if f.kind == kind && self.pass_wall(d, s, rs) {
                if f.source {
                    sources += 1;
                }
                highest = highest.max(f.amount);
            }
        }
        if sources >= 2 && self.convert_to_source(kind) {
            let bs = self.get(below(p));
            let bf = self.fluid_of(bs);
            if self.v().legacy_solid(bs) || self.is_source_of(bf, kind) {
                return Fluid { kind, source: true, amount: 8, falling: false };
            }
        }
        let ap = above(p);
        let a = self.get(ap);
        let af = self.fluid_of(a);
        if af.kind == kind && self.pass_wall(Dir::Up, s, a) {
            return Fluid { kind, source: false, amount: 8, falling: true };
        }
        let amount = i32::from(highest) - i32::from(self.drop_off(kind));
        if amount <= 0 { Fluid::EMPTY } else { Fluid { kind, source: false, amount: amount as u8, falling: false } }
    }

    fn can_be_replaced_with(&self, existing: Fluid, p: P, other_kind: u8, dir: Dir) -> bool {
        match existing.kind {
            0 => true,
            1 => dir == Dir::Down && other_kind != 1,
            _ => self.fluid_height(existing, p) >= 0.444_444_45 && other_kind == 1,
        }
    }

    /// Fluid tick at `p` (§ 2.1).
    pub(crate) fn fluid_tick(&mut self, s: u32, p: P) {
        let f = self.fluid_of(s);
        let (mut bs, mut fs) = (s, f);
        if !f.source {
            let cur = self.get(p);
            let nf = self.new_liquid(f.kind, p, cur);
            let mut delay = self.fluid_delay(f.kind);
            if f.kind == 2
                && !nf.is_empty()
                && !f.falling
                && !nf.falling
                && self.fluid_height(nf, p) > self.fluid_height(f, p)
                && self.bs.rng.next_int_bound(4) != 0
            {
                delay *= 4;
            }
            if nf.is_empty() {
                fs = nf;
                bs = self.air();
                self.set_update(p, bs);
            } else if nf != f {
                fs = nf;
                bs = self.legacy_block(nf);
                self.set_update(p, bs);
                self.schedule_fluid_tick(p, nf.type_id(), delay);
            }
        }
        self.spread(p, bs, fs);
    }

    fn spread(&mut self, p: P, s: u32, f: Fluid) {
        if f.is_empty() {
            return;
        }
        let bp = below(p);
        let bs = self.get(bp);
        let bf = self.fluid_of(bs);
        if self.maybe_pass(f.kind, s, Dir::Down, bs, bf) {
            let nb = self.new_liquid(f.kind, bp, bs);
            let nt = nb.type_id();
            if self.can_be_replaced_with(bf, bp, nb.kind, Dir::Down) && self.can_hold_specific(bs, nt) {
                self.spread_to(f.kind, bp, bs, Dir::Down, nb);
                if self.source_neighbors(f.kind, p) >= 3 {
                    self.spread_sides(p, f, s);
                }
                return;
            }
        }
        if f.source || !self.is_hole(f.kind, s, bs) {
            self.spread_sides(p, f, s);
        }
    }

    fn source_neighbors(&self, kind: u8, p: P) -> usize {
        Dir::HORIZONTAL.iter().filter(|&&d| self.is_source_of(self.fluid_at(rel(p, d)), kind)).count()
    }

    fn spread_sides(&mut self, p: P, f: Fluid, s: u32) {
        let mut n = i32::from(f.amount) - i32::from(self.drop_off(f.kind));
        if f.falling {
            n = 7;
        }
        if n <= 0 {
            return;
        }
        let spreads = self.get_spread(f.kind, p, s);
        // Order of the game's directions: north, south, west, east.
        for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
            if let Some(nf) = spreads.iter().find(|e| e.0 == d).map(|e| e.1) {
                let np = rel(p, d);
                let ns = self.get(np);
                self.spread_to(f.kind, np, ns, d, nf);
            }
        }
    }

    fn get_spread(&self, kind: u8, p: P, s: u32) -> Vec<(Dir, Fluid)> {
        let mut lowest = 1000;
        let mut result: Vec<(Dir, Fluid)> = Vec::new();
        let mut ctx = SpreadContext { origin: p, states: HashMap::new(), holes: HashMap::new() };
        for d in Dir::HORIZONTAL {
            let tp = rel(p, d);
            let ts = self.get(tp);
            let tf = self.fluid_of(ts);
            if self.maybe_pass(kind, s, d, ts, tf) {
                let nf = self.new_liquid(kind, tp, ts);
                if self.can_hold_specific(ts, nf.type_id()) {
                    let distance = if self.ctx_hole(&mut ctx, kind, tp) {
                        0
                    } else {
                        self.slope(&mut ctx, kind, tp, 1, d.opposite(), ts)
                    };
                    if distance < lowest {
                        result.clear();
                    }
                    if distance <= lowest {
                        if self.can_be_replaced_with(tf, tp, nf.kind, d) {
                            result.retain(|e| e.0 != d);
                            result.push((d, nf));
                        }
                        lowest = distance;
                    }
                }
            }
        }
        result
    }

    fn ctx_state(&self, ctx: &mut SpreadContext, q: P) -> u32 {
        let k = ctx.key(q);
        *ctx.states.entry(k).or_insert_with(|| self.get(q))
    }

    fn ctx_hole(&self, ctx: &mut SpreadContext, kind: u8, q: P) -> bool {
        let k = ctx.key(q);
        if let Some(&h) = ctx.holes.get(&k) {
            return h;
        }
        let s = self.ctx_state(ctx, q);
        let h = self.is_hole(kind, s, self.get(below(q)));
        ctx.holes.insert(k, h);
        h
    }

    fn slope(&self, ctx: &mut SpreadContext, kind: u8, p: P, pass: i32, from: Dir, s: u32) -> i32 {
        let mut lowest = 1000;
        for d in Dir::HORIZONTAL {
            if d == from {
                continue;
            }
            let tp = rel(p, d);
            let ts = self.ctx_state(ctx, tp);
            let tf = self.fluid_of(ts);
            if self.can_pass(kind, s, d, ts, tf) {
                if self.ctx_hole(ctx, kind, tp) {
                    return pass;
                }
                if pass < self.slope_distance_max(kind) {
                    let v = self.slope(ctx, kind, tp, pass + 1, d.opposite(), ts);
                    lowest = lowest.min(v);
                }
            }
        }
        lowest
    }

    fn spread_to(&mut self, kind: u8, p: P, s: u32, dir: Dir, target: Fluid) {
        if kind == 2 && dir == Dir::Down && self.fluid_of(s).kind == 1 {
            if self.v().kind(s) == Kind::Water {
                let stone = self.v().default_state(self.bs.t.id.stone);
                self.set_update(p, stone);
            }
            self.level_event(1501, p, 0);
            return;
        }
        if self.v().sim(s) & sim_bits::LIQUID_CONTAINER != 0 {
            if target.kind == 1
                && target.source
                && self.v().has(s, "waterlogged")
                && !self.v().get_bool(s, "waterlogged")
                && self.accepts_water(s, false)
            {
                self.place_water_in(p, s);
            }
            return;
        }
        if !self.v().is_air(s) {
            if kind == 2 {
                self.level_event(1501, p, 0);
            } else {
                // Water: the carried-away block drops its loot (no tool, block entity in place).
                self.drop_resources(p, s, None);
            }
        }
        let ns = self.legacy_block(target);
        self.set_update(p, ns);
    }

    // --- Liquid blocks. ---

    /// Water-lava meeting (in the lava block); false if the lava was transformed.
    fn should_spread_liquid(&mut self, s: u32, p: P) -> bool {
        if self.v().kind(s) != Kind::Lava {
            return true;
        }
        let soul = self.v().block(self.get(below(p))) == self.bs.t.id.soul_soil;
        for d in [Dir::Down, Dir::South, Dir::North, Dir::East, Dir::West] {
            let np = rel(p, d.opposite());
            if self.fluid_at(np).kind == 1 {
                let t = &self.bs.t;
                let b = if self.fluid_at(p).source { t.id.obsidian } else { t.id.cobblestone };
                let ns = self.v().default_state(b);
                self.set_update(p, ns);
                self.level_event(1501, p, 0);
                return false;
            }
            if soul && self.v().block(self.get(np)) == self.bs.t.id.blue_ice {
                let ns = self.v().default_state(self.bs.t.id.basalt);
                self.set_update(p, ns);
                self.level_event(1501, p, 0);
                return false;
            }
        }
        true
    }

    pub(crate) fn liquid_on_place(&mut self, s: u32, p: P) {
        if self.should_spread_liquid(s, p) {
            let f = self.fluid_of(s);
            let delay = self.fluid_delay(f.kind);
            self.schedule_fluid_tick(p, f.type_id(), delay);
        }
    }

    pub(crate) fn liquid_neighbor_changed(&mut self, s: u32, p: P) {
        self.liquid_on_place(s, p);
    }

    pub(crate) fn liquid_update_shape(&mut self, s: u32, p: P, ns: u32) -> u32 {
        let f = self.fluid_of(s);
        if f.source || self.fluid_of(ns).source {
            let delay = self.fluid_delay(f.kind);
            self.schedule_fluid_tick(p, f.type_id(), delay);
        }
        s
    }

    /// Random lava tick (§ 2.1): fire spread.
    pub(crate) fn lava_random_tick(&mut self, p: P) {
        if !self.can_spread_fire_around(p) {
            return;
        }
        let passes = self.bs.rng.next_int_bound(3);
        if passes > 0 {
            let mut t = p;
            for _ in 0..passes {
                let dx = self.bs.rng.next_int_bound(3) - 1;
                let dz = self.bs.rng.next_int_bound(3) - 1;
                t = [t[0] + dx, t[1] + 1, t[2] + dz];
                if !self.is_loaded(t) {
                    return;
                }
                let s = self.get(t);
                if self.v().is_air(s) {
                    if Dir::ALL.iter().any(|&d| self.lava_flammable(rel(t, d))) {
                        let fire = self.fire_state_for(t);
                        self.set_update(t, fire);
                        return;
                    }
                } else if self.tag_id(crate::tag_bits::TagId::BlocksLavaFireSpread, self.v().block(s)) {
                    return;
                }
            }
        } else {
            for _ in 0..3 {
                let dx = self.bs.rng.next_int_bound(3) - 1;
                let dz = self.bs.rng.next_int_bound(3) - 1;
                let t = [p[0] + dx, p[1], p[2] + dz];
                if !self.is_loaded(t) {
                    return;
                }
                if self.v().is_air(self.get(above(t))) && self.lava_flammable(t) {
                    let fire = self.fire_state_for(t);
                    self.set_update(above(t), fire);
                }
            }
        }
    }

    fn lava_flammable(&self, q: P) -> bool {
        if self.in_build_height(q[1]) && !self.is_loaded(q) {
            return false;
        }
        self.v().sim(self.get(q)) & sim_bits::IGNITED_BY_LAVA != 0
    }

    /// Placing a liquid in a waterloggable block that is not waterlogged (source water): waterlogged (flags 3), tick
    /// of water in 5. Lit campfire: put out (sound, event); dried ghast: sound. Single
    /// point for buckets, dispensers and flowing water (G6-F01).
    pub(crate) fn place_water_in(&mut self, p: P, s: u32) {
        let wet = self.v().set_bool(s, "waterlogged", true);
        match self.v().kind(s) {
            Kind::Campfire => {
                if self.v().get_bool(s, "lit") {
                    self.block_sound(p, "minecraft:entity.generic.extinguish_fire", 1.0, 1.0);
                    self.game_event_block("minecraft:block_change", p, None, None);
                }
                let ns = self.v().set_bool(wet, "lit", false);
                self.set_update(p, ns);
                self.schedule_fluid_tick(p, 1, 5);
            }
            _ => {
                self.set_update(p, wet);
                self.schedule_fluid_tick(p, 1, 5);
                if self.state_name(s) == "minecraft:dried_ghast" {
                    self.block_sound(p, "minecraft:block.dried_ghast.place_in_water", 1.0, 1.0);
                }
            }
        }
    }
}

struct SpreadContext {
    origin: P,
    states: HashMap<i16, u32>,
    holes: HashMap<i16, bool>,
}

impl SpreadContext {
    fn key(&self, q: P) -> i16 {
        let rx = q[0] - self.origin[0];
        let rz = q[2] - self.origin[2];
        ((((rx + 128) & 0xff) << 8) | ((rz + 128) & 0xff)) as i16
    }
}

impl Server {
    /// Sponge: breadth-first walk (depth 6, 65 cells counted, neighbours
    /// in the order down, up, north, south, west, east) of the water cells: source picked up or waterlogged
    /// block emptied, flowing water removed, kelp and seagrass broken with their loot; if at
    /// least one cell was taken, wet sponge and absorption sound.
    pub(crate) fn sponge_absorb(&mut self, start: P) {
        let mut queue = std::collections::VecDeque::from([(start, 0)]);
        let mut seen = hashbrown::HashSet::new();
        let mut count = 0;
        while let Some((q, depth)) = queue.pop_front() {
            if !seen.insert(q) {
                continue;
            }
            if q != start && !self.sponge_take(q) {
                continue;
            }
            count += 1;
            if count >= 65 {
                break;
            }
            if depth < 6 {
                for d in Dir::ALL {
                    queue.push_back((rel(q, d), depth + 1));
                }
            }
        }
        if count > 1 {
            let wet = self.default_state("minecraft:wet_sponge");
            self.set_block(start, wet, flag::CLIENTS);
            self.block_sound(start, "minecraft:block.sponge.absorb", 1.0, 1.0);
        }
    }

    /// One cell of the sponge search: true if it is taken (and the walk continues).
    fn sponge_take(&mut self, q: P) -> bool {
        let s = self.get(q);
        let f = self.fluid_of(s);
        if f.kind != 1 {
            return false;
        }
        let v = self.v();
        // Bucket pickup: source of a liquid block (flowing water: removed), or waterlogged
        // block emptied.
        if v.kind(s) == Kind::Water {
            let air = self.air();
            if f.source {
                self.set_block(q, air, 11);
            } else {
                self.set_update(q, air);
            }
            return true;
        }
        if v.has(s, "waterlogged") && v.get_bool(s, "waterlogged") {
            let ns = v.set_bool(s, "waterlogged", false);
            self.set_update(q, ns);
            return true;
        }
        match self.state_name(s) {
            "minecraft:bubble_column" => {
                let air = self.air();
                self.set_block(q, air, 11);
                true
            }
            "minecraft:kelp" | "minecraft:kelp_plant" | "minecraft:seagrass" | "minecraft:tall_seagrass" => {
                self.drop_resources(q, s, None);
                let air = self.air();
                self.set_update(q, air);
                true
            }
            _ => false,
        }
    }
}
