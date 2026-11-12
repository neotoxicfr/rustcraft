//! G6-F07: bone meal on the plants that `grow_with_bone_meal` (growth.rs) did not handle —
//! bushes and dry grasses (spread to a neighbour), netherrack and nylium, nether fungi (planted
//! variant), bamboo, climbing plants (kelp, weeping, twisting and cave vines), glow lichen
//! (multiface spread), hanging moss, mangrove leaves, rooted dirt, pale moss carpet, pitcher
//! plant, seagrass, sea pickles, shelf mushroom — and bone meal in water (seagrass and corals).
//! 26.3 game rules; every random draw on the world's random source, in the game's order.

use rc_data::PropKey;

use super::{Dir, Kind, P, above, below, flag, rel, rel_n};
use crate::server::Server;
use crate::tag_bits::TagId;

/// Probability that a nether vine gains one more cell under bone meal.
const NETHER_VINE_KEEP: f64 = 0.826;

impl Server {
    fn bm_default(&self, name: &str) -> u32 {
        self.view().default_state(self.bs.t.block_id(name))
    }

    /// The four horizontals shuffled (the game's shuffle, from the last to the first).
    fn bm_shuffled_horizontal(&mut self) -> [Dir; 4] {
        let mut d = Dir::HORIZONTAL;
        for i in (2..=4).rev() {
            let j = self.bs.rng.next_int_bound(i as i32) as usize;
            d.swap(i - 1, j);
        }
        d
    }

    /// The six directions shuffled.
    fn bm_shuffled_all(&mut self) -> [Dir; 6] {
        let mut d = Dir::ALL;
        for i in (2..=6).rev() {
            let j = self.bs.rng.next_int_bound(i as i32) as usize;
            d.swap(i - 1, j);
        }
        d
    }

    /// First empty horizontal neighbour (in the given order) where `place` would survive.
    fn bm_spread_target(&self, p: P, place: u32, order: [Dir; 4]) -> Option<P> {
        order.into_iter().map(|d| rel(p, d)).find(|&q| self.view().is_air(self.get(q)) && self.can_survive(place, q))
    }

    /// Spread to a neighbour: valid if a free neighbour exists where `test` survives (plan
    /// order), then `place` is set on a neighbour drawn at random among the suitable ones.
    fn bm_spread(&mut self, p: P, test: u32, place: u32) -> bool {
        if self.bm_spread_target(p, test, Dir::HORIZONTAL).is_none() {
            return false;
        }
        let order = self.bm_shuffled_horizontal();
        if let Some(q) = self.bm_spread_target(p, test, order) {
            self.set_update(q, place);
        }
        true
    }

    /// Targets added by G6-F07; `None`: block unknown here (no bone meal effect).
    pub(crate) fn bone_meal_more(&mut self, s: u32, p: P, name: &str) -> Option<bool> {
        let v = self.view();
        Some(match name {
            "minecraft:bush" | "minecraft:red_shrub" | "minecraft:firefly_bush" => {
                let place = v.default_state(v.block(s));
                self.bm_spread(p, s, place)
            }
            "minecraft:short_dry_grass" => {
                let tall = self.bm_default("minecraft:tall_dry_grass");
                self.set_update(p, tall);
                true
            }
            "minecraft:tall_dry_grass" => {
                let short = self.bm_default("minecraft:short_dry_grass");
                self.bm_spread(p, short, short)
            }
            "minecraft:netherrack" => self.bm_netherrack(p),
            "minecraft:crimson_nylium" | "minecraft:warped_nylium" => {
                let up = above(p);
                if !v.is_air(self.get(up)) || !self.in_build_height(up[1]) {
                    return Some(false);
                }
                self.place_world_feature("minecraft:nylium_bonemeal", up);
                true
            }
            "minecraft:crimson_fungus" | "minecraft:warped_fungus" => {
                let kind = name.trim_start_matches("minecraft:").trim_end_matches("_fungus");
                let base = self.bs.t.block_id(&format!("minecraft:{kind}_nylium"));
                if v.block(self.get(below(p))) != base || !self.in_build_height(p[1] + 1) {
                    return Some(false);
                }
                if self.bs.rng.next_float() < 0.4 {
                    self.place_world_feature(&format!("minecraft:{kind}_fungus_planted"), p);
                }
                true
            }
            "minecraft:bamboo_sapling" => {
                let up = above(p);
                if !v.is_air(self.get(up)) || !self.in_build_height(up[1]) {
                    return Some(false);
                }
                let st = v.set(self.bm_default("minecraft:bamboo"), PropKey::Leaves, "small");
                self.set_update(up, st);
                true
            }
            "minecraft:bamboo" => self.bm_bamboo(p),
            "minecraft:cave_vines" | "minecraft:cave_vines_plant" => {
                if v.get_bool(s, PropKey::Berries) {
                    return Some(false);
                }
                self.set_block(p, v.set_bool(s, PropKey::Berries, true), flag::CLIENTS);
                true
            }
            "minecraft:kelp" | "minecraft:weeping_vines" | "minecraft:twisting_vines" => self.bm_vine_head(s, p),
            "minecraft:kelp_plant" | "minecraft:weeping_vines_plant" | "minecraft:twisting_vines_plant" => {
                self.bm_vine_body(s, p)
            }
            "minecraft:glow_lichen" => self.bm_lichen(s, p),
            "minecraft:pale_hanging_moss" => {
                let me = v.block(s);
                let mut tip = p;
                while v.block(self.get(below(tip))) == me {
                    tip = below(tip);
                }
                let grow = below(tip);
                if !v.is_air(self.get(grow)) || !self.in_build_height(grow[1]) {
                    return Some(false);
                }
                self.set_update(grow, v.set_bool(s, PropKey::Tip, true));
                true
            }
            "minecraft:mangrove_leaves" => {
                if !v.is_air(self.get(below(p))) {
                    return Some(false);
                }
                let prop = v.set_bool(self.bm_default("minecraft:mangrove_propagule"), PropKey::Hanging, true);
                self.set_block(below(p), v.set_int(prop, PropKey::Age, 0), flag::CLIENTS);
                true
            }
            "minecraft:rooted_dirt" => {
                let down = below(p);
                if !v.is_air(self.get(down)) || !self.in_build_height(down[1]) {
                    return Some(false);
                }
                let roots = self.bm_default("minecraft:hanging_roots");
                self.set_update(down, roots);
                true
            }
            "minecraft:pale_moss_carpet" => {
                if !v.get_bool(s, PropKey::Bottom) {
                    return Some(false);
                }
                let top = self.moss_carpet_topper(s, p, |_| true);
                if self.view().is_air(top) {
                    return Some(false);
                }
                self.set_update(above(p), top);
                true
            }
            "minecraft:pitcher_crop" => self.bm_pitcher(s, p),
            "minecraft:seagrass" => {
                if v.block(self.get(above(p))) != self.bs.t.id.water {
                    return Some(false);
                }
                self.bm_tall_seagrass(p);
                true
            }
            "minecraft:sea_pickle" => self.bm_sea_pickle(s, p),
            "minecraft:shelf_mushroom" => {
                let age = v.get_int(s, PropKey::Age);
                if age >= 1 {
                    return Some(false);
                }
                self.set_block(p, v.set_int(s, PropKey::Age, age + 1), flag::CLIENTS);
                true
            }
            "minecraft:small_dripleaf" => self.small_dripleaf_bone_meal(s, p),
            _ => return None,
        })
    }

    /// Netherrack: valid if the block above lets skylight through and a nylium touches the block
    /// (3×3×3 cube); becomes the neighbouring nylium, chosen by a coin flip if both are there.
    fn bm_netherrack(&mut self, p: P) -> bool {
        let v = self.view();
        if !v.info(self.get(above(p))).has(rc_data::flags::SKYLIGHT_THROUGH) {
            return false;
        }
        let (crimson, warped) =
            (self.bs.t.block_id("minecraft:crimson_nylium"), self.bs.t.block_id("minecraft:warped_nylium"));
        let (mut red, mut blue, mut any) = (false, false, false);
        'scan: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let b = v.block(self.get([p[0] + dx, p[1] + dy, p[2] + dz]));
                    if self.tag_id(TagId::Nylium, b) {
                        any = true;
                        red |= b == crimson;
                        blue |= b == warped;
                        if red && blue {
                            break 'scan;
                        }
                    }
                }
            }
        }
        if !any {
            return false;
        }
        let to = if red && blue {
            if self.bs.rng.next_bool() { warped } else { crimson }
        } else if blue {
            warped
        } else if red {
            crimson
        } else {
            return true;
        };
        self.set_update(p, self.view().default_state(to));
        true
    }

    /// Bamboo: the column (16 at most) has not finished growing and the cell above the top is
    /// free; 1 or 2 stalks added at the top.
    fn bm_bamboo(&mut self, p: P) -> bool {
        let v = self.view();
        let up = self.bamboo_run(p, true);
        let down = self.bamboo_run(p, false);
        let grow = rel_n(p, Dir::Up, up + 1);
        if up + down + 1 >= 16
            || v.get_int(self.get(rel_n(p, Dir::Up, up)), PropKey::Stage) == 1
            || !self.in_build_height(grow[1])
            || !v.is_air(self.get(grow))
        {
            return false;
        }
        let n = 1 + self.bs.rng.next_int_bound(2);
        for i in 0..n {
            let (top, total) = (rel_n(p, Dir::Up, up + i), up + down + 1 + i);
            let ts = self.get(top);
            let g = above(top);
            let v = self.view();
            if total >= 16 || v.get_int(ts, PropKey::Stage) == 1 || !v.is_air(self.get(g)) || !self.in_build_height(g[1]) {
                break;
            }
            self.bamboo_grow(ts, top, total);
        }
        true
    }

    /// Growth direction and the cell the head accepts (kelp: water; vines: air).
    fn bm_vine_dir(&self, s: u32) -> Dir {
        match self.view().kind(s) {
            Kind::GrowingHead { up: true } | Kind::GrowingBody { up: true } => Dir::Up,
            _ => Dir::Down,
        }
    }

    fn bm_vine_can_grow_into(&self, head: u32, q: P) -> bool {
        let into = self.get(q);
        if self.is_kelp(head) { self.view().block(into) == self.bs.t.id.water } else { self.view().is_air(into) }
    }

    /// Head of a climbing plant: valid if the next cell accepts it; grows by 1 cell (kelp) or by
    /// a drawn number (nether vines: each extra cell at 82.6%), age + 1 per cell (25 at most).
    fn bm_vine_head(&mut self, s: u32, p: P) -> bool {
        let d = self.bm_vine_dir(s);
        let mut fwd = rel(p, d);
        if !self.bm_vine_can_grow_into(s, fwd) || !self.in_build_height(fwd[1]) {
            return false;
        }
        let v = self.view();
        let mut age = (v.get_int(s, PropKey::Age) + 1).min(25);
        let n = if self.is_kelp(s) {
            1
        } else {
            let (mut keep, mut n) = (1.0, 0);
            while self.bs.rng.next_double() < keep {
                keep *= NETHER_VINE_KEEP;
                n += 1;
            }
            n
        };
        for _ in 0..n {
            if !self.bm_vine_can_grow_into(s, fwd) || !self.in_build_height(fwd[1]) {
                break;
            }
            self.set_update(fwd, self.view().set_int(s, PropKey::Age, age));
            fwd = rel(fwd, d);
            age = (age + 1).min(25);
        }
        true
    }

    /// Body of a climbing plant: defers to the head at the end of the column.
    fn bm_vine_body(&mut self, s: u32, p: P) -> bool {
        let d = self.bm_vine_dir(s);
        let (head, body) = self.growing_parts(s);
        let v = self.view();
        let mut q = rel(p, d);
        while v.block(self.get(q)) == body {
            q = rel(q, d);
        }
        let hs = self.get(q);
        if v.block(hs) != head {
            return false;
        }
        self.bm_vine_head(hs, q)
    }

    // --- Glow lichen: spread from one face to another. ---

    /// Cell where the lichen can take face `face`: air, lichen or source water, without that
    /// face yet, attachable face.
    fn bm_lichen_can_spread_into(&self, me: u16, q: P, face: Dir) -> bool {
        let v = self.view();
        let e = self.get(q);
        let replaceable =
            v.is_air(e) || v.block(e) == me || (v.block(e) == self.bs.t.id.water && self.fluid_of(e).source);
        replaceable && (v.block(e) != me || !v.get_bool(e, face.key())) && self.multiface_can_attach(q, face)
    }

    /// Spread target from face `from` towards `towards`: same cell, same plane, or around the
    /// edge (in that order).
    fn bm_lichen_target(&self, s: u32, p: P, from: Dir, towards: Dir) -> Option<(P, Dir)> {
        let v = self.view();
        if towards.axis() == from.axis() || !v.get_bool(s, from.key()) || v.get_bool(s, towards.key()) {
            return None;
        }
        let me = v.block(s);
        [(p, towards), (rel(p, towards), from), (rel(rel(p, towards), from), towards.opposite())]
            .into_iter()
            .find(|&(q, f)| self.bm_lichen_can_spread_into(me, q, f))
    }

    fn bm_lichen(&mut self, s: u32, p: P) -> bool {
        let valid = Dir::ALL
            .iter()
            .any(|&f| Dir::ALL.iter().any(|&t| self.bm_lichen_target(s, p, f.opposite(), t).is_some()));
        if !valid {
            return false;
        }
        let me = self.view().block(s);
        for from in self.bm_shuffled_all() {
            if !self.view().get_bool(s, from.key()) {
                continue;
            }
            for towards in self.bm_shuffled_all() {
                let Some((q, face)) = self.bm_lichen_target(s, p, from, towards) else { continue };
                let old = self.get(q);
                let v = self.view();
                let base = if v.block(old) == me {
                    old
                } else if self.fluid_of(old).kind == 1 && self.fluid_of(old).source {
                    v.set_bool(v.default_state(me), PropKey::Waterlogged, true)
                } else {
                    v.default_state(me)
                };
                // Placement refused (cell wrapped outside the world): next direction, like the game.
                if self.set_block(q, v.set_bool(base, face.key(), true), flag::CLIENTS) {
                    return true;
                }
            }
        }
        true
    }

    // --- Pitcher plant (double crop). ---

    /// The crop can move to age `age`: not ripe, enough light, upper half inside the world and,
    /// from age 3, the cell above free (air or the plant itself).
    fn bm_pitcher_can_grow(&self, lp: P, ls: u32, age: i32) -> bool {
        let up = self.get(above(lp));
        self.view().get_int(ls, PropKey::Age) < 4
            && self.raw_brightness(lp) >= 8
            && self.in_build_height(lp[1] + 1)
            && (age < 3 || self.view().is_air(up) || self.view().kind(up) == Kind::PitcherCrop)
    }

    /// Growth of the pitcher plant (lower half): age + `inc` (4 at most); upper half placed
    /// from age 3.
    pub(crate) fn pitcher_grow(&mut self, lp: P, ls: u32, inc: i32) {
        let v = self.view();
        let age = (v.get_int(ls, PropKey::Age) + inc).min(4);
        if !self.bm_pitcher_can_grow(lp, ls, age) {
            return;
        }
        let low = v.set_int(ls, PropKey::Age, age);
        self.set_block(lp, low, flag::CLIENTS);
        if age >= 3 {
            self.set_update(above(lp), self.view().set(low, PropKey::Half, "upper"));
        }
    }

    fn bm_pitcher(&mut self, s: u32, p: P) -> bool {
        let v = self.view();
        let lower = |st: u32| v.kind(st) == Kind::PitcherCrop && v.get(st, PropKey::Half) == Some("lower");
        let (lp, ls) = if lower(s) {
            (p, s)
        } else if lower(self.get(below(p))) {
            (below(p), self.get(below(p)))
        } else {
            return false;
        };
        if !self.bm_pitcher_can_grow(lp, ls, v.get_int(ls, PropKey::Age) + 1) {
            return false;
        }
        self.pitcher_grow(lp, ls, 1);
        true
    }

    // --- Water plants. ---

    fn bm_tall_seagrass(&mut self, p: P) {
        let v = self.view();
        let low = v.set(v.default_state(self.bs.t.id.tall_seagrass), PropKey::Half, "lower");
        self.set_block(p, low, flag::CLIENTS);
        self.set_block(above(p), self.view().set(low, PropKey::Half, "upper"), flag::CLIENTS);
    }

    /// Live sea pickles on a coral block: more pickles on the corals of a diamond 5 wide around,
    /// then 4 pickles here.
    fn bm_sea_pickle(&mut self, s: u32, p: P) -> bool {
        let v = self.view();
        if !v.get_bool(s, PropKey::Waterlogged) || !self.tag_id(TagId::CoralBlocks, v.block(self.get(below(p)))) {
            return false;
        }
        let water = self.bs.t.id.water;
        let (mut span, mut off) = (1, 0);
        for (count, x) in (0..5).enumerate() {
            for z in 0..span {
                for y in p[1] - 1..p[1] + 1 {
                    let q = [p[0] - 2 + x, y, p[2] - off + z];
                    if q != p && self.bs.rng.next_int_bound(6) == 0 && self.view().block(self.get(q)) == water {
                        let b = self.view().block(self.get(below(q)));
                        if self.tag_id(TagId::CoralBlocks, b) {
                            let n = self.bs.rng.next_int_bound(4) + 1;
                            let v = self.view();
                            let st = v.set_int(v.default_state(v.block(s)), PropKey::Pickles, n);
                            self.set_update(q, st);
                        }
                    }
                }
            }
            if count < 2 {
                span += 2;
                off += 1;
            } else {
                span -= 2;
                off -= 1;
            }
        }
        self.set_block(p, self.view().set_int(s, PropKey::Pickles, 4), flag::CLIENTS);
        true
    }

    /// Members of a block tag, in the tag's order.
    fn bm_tag_members(&self, tag: &str) -> &[i32] {
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:block")
            .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag))
            .map_or(&[][..], |(_, ids)| ids.as_slice())
    }

    /// Element drawn at random from a tag (nothing, and no draw, if it is empty).
    fn bm_random_in_tag(&mut self, tag: &str) -> Option<u32> {
        let n = self.bm_tag_members(tag).len();
        if n == 0 {
            return None;
        }
        let i = self.bs.rng.next_int_bound(n as i32) as usize;
        let b = self.bm_tag_members(tag)[i];
        Some(self.view().default_state(b as u16))
    }

    /// Bone meal in full water `p` (clicked face of a solid block, or a dispenser with no
    /// face): 128 seagrass attempts, displaced like the grass ones; in a coral biome, a wall fan
    /// on the clicked face at the first attempt, otherwise one in four a random underwater
    /// plant; existing seagrass grown one time in ten. True if `p` is full water.
    pub(crate) fn grow_water_plant(&mut self, p: P, face: Option<Dir>) -> bool {
        let water = self.bs.t.id.water;
        let f = self.fluid_at(p);
        if self.view().block(self.get(p)) != water || f.amount != 8 {
            return false;
        }
        let seagrass = self.bm_default("minecraft:seagrass");
        'attempt: for j in 0..128 {
            let mut t = p;
            let mut grow = seagrass;
            for _ in 0..j / 16 {
                let dx = self.bs.rng.next_int_bound(3) - 1;
                let dy = (self.bs.rng.next_int_bound(3) - 1) * self.bs.rng.next_int_bound(3) / 2;
                let dz = self.bs.rng.next_int_bound(3) - 1;
                t = [t[0] + dx, t[1] + dy, t[2] + dz];
                if self.view().full_collision(self.get(t)) {
                    continue 'attempt;
                }
            }
            if self.biome_tag(t,"minecraft:produces_corals_from_bonemeal") {
                if let Some(face) = face.filter(|d| j == 0 && d.axis() != 1) {
                    grow = self.bm_random_in_tag("minecraft:wall_corals").unwrap_or(grow);
                    if self.view().has(grow, PropKey::Facing) {
                        grow = self.view().set_dir(grow, PropKey::Facing, face);
                    }
                } else if self.bs.rng.next_int_bound(4) == 0 {
                    grow = self.bm_random_in_tag("minecraft:underwater_bonemeals").unwrap_or(grow);
                }
            }
            if self.tag_id(TagId::WallCorals, self.view().block(grow)) && self.view().has(grow, PropKey::Facing) {
                let mut d = 0;
                while !self.can_survive(grow, t) && d < 4 {
                    let h = Dir::HORIZONTAL[self.bs.rng.next_int_bound(4) as usize];
                    grow = self.view().set_dir(grow, PropKey::Facing, h);
                    d += 1;
                }
            }
            if !self.can_survive(grow, t) {
                continue;
            }
            let ts = self.get(t);
            if self.view().block(ts) == water && self.fluid_of(ts).amount == 8 {
                self.set_update(t, grow);
            } else if self.view().block(ts) == self.bs.t.block_id("minecraft:seagrass")
                && self.view().block(self.get(above(t))) == water
                && self.bs.rng.next_int_bound(10) == 0
            {
                self.bm_tall_seagrass(t);
            }
        }
        true
    }
}
