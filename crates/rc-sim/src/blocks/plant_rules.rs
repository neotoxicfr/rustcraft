//! G5-F09: blocks with their own rules in the integrated server — big dripleaf (tilt under
//! an entity, projectile, redstone, ticks 10/10/100, stem, bone meal), chorus (growth,
//! branches, death, connections, collapse), budding amethyst (buds), bell
//! (survival, single / double wall attachment).

use super::{Dir, Kind, P, above, below, flag, rel};
use crate::server::Server;

/// Delay until the next tilt stage (`None`: no tick).
fn tilt_delay(tilt: &str) -> Option<i32> {
    match tilt {
        "unstable" | "partial" => Some(10),
        "full" => Some(100),
        _ => None,
    }
}

impl Server {
    fn block_named(&self, name: &str) -> u16 {
        self.bs.t.block_id(name)
    }

    // --- Big dripleaf. ---

    /// Tilt placed (flag 2); "block changed" when it changes, except toward "unstable".
    fn dripleaf_set_tilt(&mut self, s: u32, p: P, tilt: &str) {
        let prev = self.v().get(s, "tilt") == Some(tilt);
        let ns = self.v().set(s, "tilt", tilt);
        self.set_block(p, ns, flag::CLIENTS);
        if tilt != "unstable" && !prev {
            self.game_event_block("minecraft:block_change", p, None, None);
        }
    }

    fn dripleaf_sound(&mut self, p: P, sound: &str) {
        // Difference computed in single-precision float like the game (1.2 − 0.8 is one ulp more than 0.4).
        let pitch = self.bs.rng.next_float() * (1.2f32 - 0.8f32) + 0.8f32;
        self.block_sound(p, sound, 1.0, pitch);
    }

    fn dripleaf_tilt_and_schedule(&mut self, s: u32, p: P, tilt: &str, sound: Option<&str>) {
        self.dripleaf_set_tilt(s, p, tilt);
        if let Some(snd) = sound {
            self.dripleaf_sound(p, snd);
        }
        if let Some(d) = tilt_delay(tilt) {
            let b = self.v().block(s);
            self.schedule_tick(p, b, d);
        }
    }

    fn dripleaf_reset(&mut self, s: u32, p: P) {
        self.dripleaf_set_tilt(s, p, "none");
        if self.v().get(s, "tilt") != Some("none") {
            self.dripleaf_sound(p, "minecraft:block.big_dripleaf.tilt_up");
        }
    }

    pub(crate) fn dripleaf_neighbor_changed(&mut self, s: u32, p: P) {
        if self.has_neighbor_signal(p) {
            self.dripleaf_reset(s, p);
        }
    }

    pub(crate) fn dripleaf_tick(&mut self, s: u32, p: P) {
        if self.has_neighbor_signal(p) {
            self.dripleaf_reset(s, p);
            return;
        }
        let down = Some("minecraft:block.big_dripleaf.tilt_down");
        match self.v().get(s, "tilt") {
            Some("unstable") => self.dripleaf_tilt_and_schedule(s, p, "partial", down),
            Some("partial") => self.dripleaf_tilt_and_schedule(s, p, "full", down),
            Some("full") => self.dripleaf_reset(s, p),
            _ => {}
        }
    }

    /// Projectile on the leaf: full tilt at once, whatever the state.
    pub(crate) fn dripleaf_projectile_hit(&mut self, s: u32, p: P) {
        self.dripleaf_tilt_and_schedule(s, p, "full", Some("minecraft:block.big_dripleaf.tilt_down"));
    }

    /// Entity in the leaf's cell (to be called by the pass over traversed blocks): standing on the
    /// ground and higher than 11/16 of the block, without signal, the flat leaf becomes unstable.
    pub(crate) fn dripleaf_entity_inside(&mut self, p: P, on_ground: bool, y: f64) {
        let s = self.get(p);
        if self.v().kind(s) != Kind::BigDripleaf || self.v().get(s, "tilt") != Some("none") {
            return;
        }
        // Threshold computed in single-precision float like the game (integer + 0.6875f).
        let threshold = f64::from(p[1] as f32 + 0.6875f32);
        if on_ground && y > threshold && !self.has_neighbor_signal(p) {
            self.dripleaf_tilt_and_schedule(s, p, "unstable", None);
        }
    }

    pub(crate) fn dripleaf_update_shape(&mut self, s: u32, kind: Kind, p: P, dir: Dir, ns: u32) -> u32 {
        let v = self.v();
        let leaf = self.block_named("big_dripleaf");
        if kind == Kind::BigDripleaf {
            if dir == Dir::Down && !self.plant_rule_survives(s, p) {
                return self.air();
            }
            if dir == Dir::Up && v.block(ns) == leaf {
                // A leaf placed on top: becomes stem (orientation and water kept).
                let stem = v.default_state(self.block_named("big_dripleaf_stem"));
                let stem = v.set_dir(stem, "facing", v.facing(s));
                return v.set_bool(stem, "waterlogged", v.get_bool(s, "waterlogged"));
            }
        } else if dir.axis() == 1 && !self.plant_rule_survives(s, p) {
            let b = v.block(s);
            self.schedule_tick(p, b, 1);
        }
        s
    }

    /// Cell where the leaf can grow: within world height, air, water or small leaf.
    fn dripleaf_can_grow_into(&self, q: P) -> bool {
        let s = self.get(q);
        let b = self.v().block(s);
        self.in_build_height(q[1])
            && (self.v().is_air(s) || b == self.bs.t.id.water || b == self.block_named("small_dripleaf"))
    }

    fn water_source_at(&self, q: P) -> bool {
        let f = self.fluid_at(q);
        f.kind == 1 && f.source
    }

    fn dripleaf_place(&mut self, q: P, stem: bool, water: bool, facing: Dir) {
        let b = self.block_named(if stem { "big_dripleaf_stem" } else { "big_dripleaf" });
        let v = self.v();
        let s = v.set_bool(v.set_dir(v.default_state(b), "facing", facing), "waterlogged", water);
        self.set_update(q, s);
    }

    /// Bone meal: leaf → stem + leaf above; stem → the top leaf goes up by one
    /// notch (orientation of the clicked stem). True if the target is valid.
    pub(crate) fn dripleaf_bone_meal(&mut self, s: u32, p: P, stem: bool) -> bool {
        let v = self.v();
        let facing = v.facing(s);
        if !stem {
            let up = above(p);
            if !self.dripleaf_can_grow_into(up) {
                return false;
            }
            let water = v.get_bool(s, "waterlogged");
            self.dripleaf_place(p, true, water, facing);
            let w = self.water_source_at(up);
            self.dripleaf_place(up, false, w, facing);
            return true;
        }
        let (stem_b, leaf_b) = (v.block(s), self.block_named("big_dripleaf"));
        let mut head = above(p);
        while v.block(self.get(head)) == stem_b {
            head = above(head);
        }
        if v.block(self.get(head)) != leaf_b || !self.dripleaf_can_grow_into(above(head)) {
            return false;
        }
        let w = self.water_source_at(head);
        self.dripleaf_place(head, true, w, facing);
        let w = self.water_source_at(above(head));
        self.dripleaf_place(above(head), false, w, facing);
        true
    }

    // --- Common survival (leaf, stem, chorus). ---

    pub(crate) fn plant_rule_survives(&self, s: u32, p: P) -> bool {
        let v = self.v();
        let bs = self.get(below(p));
        let bb = v.block(bs);
        match v.kind(s) {
            Kind::BigDripleaf => {
                v.block(s) == bb
                    || bb == self.block_named("big_dripleaf_stem")
                    || self.tag_id(crate::tag_bits::TagId::SupportsBigDripleaf, bb)
            }
            Kind::BigDripleafStem => {
                let ab = v.block(self.get(above(p)));
                (bb == v.block(s) || self.tag_id(crate::tag_bits::TagId::SupportsBigDripleaf, bb))
                    && (ab == v.block(s) || ab == self.block_named("big_dripleaf"))
            }
            Kind::ChorusFlower => {
                let plant = self.block_named("chorus_plant");
                if bb == plant || self.tag_id(crate::tag_bits::TagId::SupportsChorusFlower, bb) {
                    return true;
                }
                if !v.is_air(bs) {
                    return false;
                }
                let mut one = false;
                for d in Dir::HORIZONTAL {
                    let n = self.get(rel(p, d));
                    if v.block(n) == plant {
                        if one {
                            return false;
                        }
                        one = true;
                    } else if !v.is_air(n) {
                        return false;
                    }
                }
                one
            }
            Kind::ChorusPlant => {
                let me = v.block(s);
                let above_or_below = !v.is_air(self.get(above(p))) && !v.is_air(bs);
                for d in Dir::HORIZONTAL {
                    let q = rel(p, d);
                    if v.block(self.get(q)) == me {
                        if above_or_below {
                            return false;
                        }
                        let qb = v.block(self.get(below(q)));
                        if qb == me || self.tag_id(crate::tag_bits::TagId::SupportsChorusPlant, qb) {
                            return true;
                        }
                    }
                }
                bb == me || self.tag_id(crate::tag_bits::TagId::SupportsChorusPlant, bb)
            }
            _ => true,
        }
    }

    // --- Chorus. ---

    /// Chorus plant (default state) connected to its 6 neighbours: another plant or flower, and
    /// at the bottom also a block of the tag `supports_chorus_plant` (placement, growth).
    pub(crate) fn chorus_plant_state(&self, p: P) -> u32 {
        let v = self.v();
        let plant = self.block_named("chorus_plant");
        let flower = self.block_named("chorus_flower");
        let mut s = v.default_state(plant);
        for d in Dir::ALL {
            let b = v.block(self.get(rel(p, d)));
            let c = b == plant || b == flower || (d == Dir::Down && self.tag_id(crate::tag_bits::TagId::SupportsChorusPlant, b));
            s = v.set_bool(s, d.name(), c);
        }
        s
    }

    pub(crate) fn chorus_update_shape(&mut self, s: u32, kind: Kind, p: P, dir: Dir, ns: u32) -> u32 {
        let survives = self.plant_rule_survives(s, p);
        if kind == Kind::ChorusFlower {
            if dir != Dir::Up && !survives {
                let b = self.v().block(s);
                self.schedule_tick(p, b, 1);
            }
            return s;
        }
        if !survives {
            let b = self.v().block(s);
            self.schedule_tick(p, b, 1);
            return s;
        }
        let v = self.v();
        let nb = v.block(ns);
        let c = nb == v.block(s)
            || nb == self.block_named("chorus_flower")
            || (dir == Dir::Down && self.tag_id(crate::tag_bits::TagId::SupportsChorusPlant, nb));
        v.set_bool(s, dir.name(), c)
    }

    fn chorus_all_neighbors_empty(&self, q: P, ignore: Option<Dir>) -> bool {
        Dir::HORIZONTAL.iter().all(|&d| Some(d) == ignore || self.v().is_air(self.get(rel(q, d))))
    }

    fn chorus_grown_flower(&mut self, q: P, age: i32) {
        let f = self.v().set_int(self.v().default_state(self.block_named("chorus_flower")), "age", age);
        self.set_block(q, f, flag::CLIENTS);
        self.level_event(1033, q, 0);
    }

    fn chorus_dead_flower(&mut self, q: P) {
        let f = self.v().set_int(self.v().default_state(self.block_named("chorus_flower")), "age", 5);
        self.set_block(q, f, flag::CLIENTS);
        self.level_event(1034, q, 0);
    }

    /// Random tick of the chorus flower: grows up (short column), otherwise branches (age + 1),
    /// otherwise dies (age 5).
    pub(crate) fn chorus_flower_random_tick(&mut self, s: u32, p: P) {
        let up = above(p);
        if !self.v().is_air(self.get(up)) || up[1] >= self.top_y() {
            return;
        }
        let age = self.v().get_int(s, "age");
        if age >= 5 {
            return;
        }
        let plant = self.block_named("chorus_plant");
        let (mut grow_up, mut pillar) = (false, false);
        let below_s = self.get(below(p));
        let bb = self.v().block(below_s);
        if self.tag_id(crate::tag_bits::TagId::SupportsChorusFlower, bb) {
            grow_up = true;
        } else if bb == plant {
            let mut height = 1;
            for _ in 0..4 {
                let t = self.v().block(self.get([p[0], p[1] - (height + 1), p[2]]));
                if t != plant {
                    if self.tag_id(crate::tag_bits::TagId::SupportsChorusFlower, t) {
                        pillar = true;
                    }
                    break;
                }
                height += 1;
            }
            if height < 2 || height <= self.bs.rng.next_int_bound(if pillar { 5 } else { 4 }) {
                grow_up = true;
            }
        } else if self.v().is_air(below_s) {
            grow_up = true;
        }
        if grow_up && self.chorus_all_neighbors_empty(up, None) && self.v().is_air(self.get(above(up))) {
            let ps = self.chorus_plant_state(p);
            self.set_block(p, ps, flag::CLIENTS);
            self.chorus_grown_flower(up, age);
        } else if age < 4 {
            let mut tries = self.bs.rng.next_int_bound(4);
            if pillar {
                tries += 1;
            }
            let mut branched = false;
            for _ in 0..tries {
                let d = Dir::HORIZONTAL[self.bs.rng.next_int_bound(4) as usize];
                let t = rel(p, d);
                if self.v().is_air(self.get(t))
                    && self.v().is_air(self.get(below(t)))
                    && self.chorus_all_neighbors_empty(t, Some(d.opposite()))
                {
                    self.chorus_grown_flower(t, age + 1);
                    branched = true;
                }
            }
            if branched {
                let ps = self.chorus_plant_state(p);
                self.set_block(p, ps, flag::CLIENTS);
            } else {
                self.chorus_dead_flower(p);
            }
        } else {
            self.chorus_dead_flower(p);
        }
    }

    // --- Budding amethyst. ---

    /// Once in 5, a random face: bud placed in air or full water, otherwise the
    /// bud facing that face grows by one stage (up to the cluster).
    pub(crate) fn budding_amethyst_random_tick(&mut self, p: P) {
        if self.bs.rng.next_int_bound(5) != 0 {
            return;
        }
        let d = Dir::ALL[self.bs.rng.next_int_bound(6) as usize];
        let q = rel(p, d);
        let r = self.get(q);
        let v = self.v();
        let rb = v.block(r);
        let f = self.fluid_of(r);
        let stages = ["small_amethyst_bud", "medium_amethyst_bud", "large_amethyst_bud", "amethyst_cluster"];
        let next = if v.is_air(r) || (rb == self.bs.t.id.water && f.amount == 8) {
            Some(stages[0])
        } else {
            stages[..3]
                .iter()
                .position(|n| self.block_named(n) == rb)
                .filter(|_| v.get_dir(r, "facing") == Some(d))
                .map(|i| stages[i + 1])
        };
        let Some(next) = next else { return };
        let s = v.default_state(self.block_named(next));
        let s = v.set_bool(v.set_dir(s, "facing", d), "waterlogged", f.kind == 1 && f.source);
        self.set_update(q, s);
    }

    // --- Bell. ---

    /// Support direction: floor at the bottom, ceiling at the top, wall behind (orientation).
    fn bell_support_dir(&self, s: u32) -> Dir {
        match self.v().get(s, "attachment") {
            Some("ceiling") => Dir::Up,
            Some("single_wall" | "double_wall") => self.v().facing(s),
            _ => Dir::Down,
        }
    }

    pub(crate) fn bell_survives(&self, s: u32, p: P) -> bool {
        match self.bell_support_dir(s) {
            Dir::Up => self.v().center_down(self.get(above(p))),
            d => self.face_attach(p, d),
        }
    }

    pub(crate) fn bell_update_shape(&mut self, s: u32, p: P, dir: Dir, _np: P, ns: u32) -> u32 {
        let v = self.v();
        let att = v.get(s, "attachment").unwrap_or("floor");
        let support = self.bell_support_dir(s);
        if support == dir && att != "double_wall" && !self.bell_survives(s, p) {
            return self.air();
        }
        let facing = v.facing(s);
        if dir.axis() == facing.axis() {
            // The game tests the neighbour's face turned the same way as the direction (not the one that
            // faces the bell): reproduced as is.
            if att == "double_wall" && !v.sturdy(ns, dir) {
                return v.set_dir(v.set(s, "attachment", "single_wall"), "facing", dir.opposite());
            }
            if att == "single_wall" && support.opposite() == dir && v.sturdy(ns, facing) {
                return v.set(s, "attachment", "double_wall");
            }
        }
        s
    }
}
