//! Gravity, fire, growth and "living" blocks (§ 2.2 to § 2.8): scheduled and random ticks of
//! plants, grass, leaves, ice, snow, farmland; G6-F08: eyeblossoms (open at night, wake their
//! neighbours), mushroom spread, pitcher crop growth, cauldrons fed by a stalactite.

use super::{Dir, Kind, P, above, add, below, flag, rel};
use crate::server::Server;

/// Rise of a box `bb` that touches the part gained by the shape of a block `p` (fractions
/// `lo` → `hi` on each axis): raised by a block then lowered onto the new top, by at most one
/// block; `None` if it does not touch that part.
fn push_up_dy(bb: &rc_physics::Aabb, p: P, lo: [f64; 3], hi: [f64; 3]) -> Option<f64> {
    let c = p.map(f64::from);
    let (min, max) = ([0, 1, 2].map(|i| c[i] + lo[i]), [0, 1, 2].map(|i| c[i] + hi[i]));
    if !(0..3).all(|i| bb.min[i] < max[i] && bb.max[i] > min[i]) {
        return None;
    }
    let (top, raised) = (max[1], bb.min[1] + 1.0);
    let offset = if top - 1.0e-7 <= raised { (top - raised).max(-1.0) } else { -1.0 };
    Some(1.0 + offset)
}

/// Precipitation of a biome at a point (none, rain, snow).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Precip {
    None,
    Rain,
    Snow,
}

/// Biome climate noises: altitude temperature (seed 1234), information noise
/// (2345) and the stack of the "frozen" modifier (3456, three layers).
struct ClimateNoise {
    temperature: rc_worldgen::noise::Simplex,
    info: rc_worldgen::noise::Simplex,
    frozen: [(rc_worldgen::noise::Simplex, f64, f32); 3],
}

fn climate_noise() -> &'static ClimateNoise {
    use rc_worldgen::noise::Simplex;
    use rc_worldgen::rng::Rng;
    static C: std::sync::OnceLock<ClimateNoise> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let info = Simplex::new_without_offset(&mut Rng::legacy(2345));
        let temperature = Simplex::new_without_offset(&mut Rng::legacy(1234));
        let mut fr = Rng::legacy(3456);
        let frozen = [(1.0, 0.142_857_15f32), (0.5, 0.285_714_3), (0.25, 0.571_428_6)]
            .map(|(freq, amp)| (Simplex::new_without_offset(&mut fr), freq, amp));
        ClimateNoise { temperature, info, frozen }
    })
}

/// Adjusted temperature of a biome ("frozen" modifier, altitude above sea + 17).
pub(crate) fn biome_temperature_at(base: f32, frozen: bool, p: P, sea_level: i32) -> f32 {
    let c = climate_noise();
    let mut t = base;
    if frozen {
        let (x, z) = (f64::from(p[0]), f64::from(p[2]));
        let mut large = 0.0f32;
        for (n, freq, amp) in &c.frozen {
            large += amp * n.get2(x * 0.05 * freq, z * 0.05 * freq);
        }
        let patches = f64::from(large * 7.0) + f64::from(c.info.get2(x * 0.2, z * 0.2));
        if patches < 0.3 && f64::from(c.info.get2(x * 0.09, z * 0.09)) < 0.8 {
            t = 0.2;
        }
    }
    let snow = sea_level + 17;
    if p[1] > snow {
        let v = c.temperature.get2(f64::from(p[0] as f32 / 8.0), f64::from(p[2] as f32 / 8.0)) * 8.0;
        t - (v + p[1] as f32 - snow as f32) * 0.05 / 40.0
    } else {
        t
    }
}

impl Server {
    /// The dimension has weather: sky, no ceiling, not the End.
    pub(crate) fn can_have_weather(&self) -> bool {
        let name = &self.dim.name;
        let sky = self.dim_type_field(name, "has_skylight").and_then(|v| v.as_bool()).unwrap_or(true);
        let ceiling = self.dim_type_field(name, "has_ceiling").and_then(|v| v.as_bool()).unwrap_or(false);
        sky && !ceiling && name != "minecraft:the_end"
    }

    /// Rain in the world (rain level > 0.2 where weather exists).
    pub(crate) fn bs_raining(&self) -> bool {
        self.can_have_weather() && self.life.weather.rain_level > 0.2
    }

    /// Rain falling on this block.
    pub(crate) fn bs_raining_at(&self, p: P) -> bool {
        self.bs_precipitation_at(p) == Precip::Rain
    }

    /// Rain, visible sky, above the "motion blocking" map,
    /// then the biome's precipitation at the point.
    pub(crate) fn bs_precipitation_at(&self, p: P) -> Precip {
        if !self.bs_raining() || !self.can_see_sky(p) || self.motion_blocking_height(p[0], p[2]) > p[1] {
            return Precip::None;
        }
        self.biome_precipitation(p, p)
    }

    /// Precipitation of the biome taken at `at_biome`, temperature evaluated at `p`.
    fn biome_precipitation(&self, at_biome: P, p: P) -> Precip {
        let Some(b) = self.biome_of(at_biome) else { return Precip::None };
        if !b.has_precipitation {
            return Precip::None;
        }
        let t = biome_temperature_at(b.temperature, b.frozen, p, crate::mobs::surface::sea_level(self));
        if t >= 0.15 { Precip::Rain } else { Precip::Snow }
    }

    /// Sky darkening (15 − sky light according to the time, the rain and
    /// the thunderstorm).
    pub(crate) fn sky_darken(&self) -> i32 {
        self.mob_sky_darken()
    }

    /// Angle of the sun (radians) from the world clock.
    fn sun_angle(&self) -> f32 {
        let day = self.clocks.first().map_or(self.time, |c| c.1);
        let f = ((day as f64 / 24000.0 - 0.25).rem_euclid(1.0)) as f32;
        let g = 0.5 - (f64::from(f) * std::f64::consts::PI).cos() as f32 / 2.0;
        (f * 2.0 + g) / 3.0 * std::f32::consts::TAU
    }

    /// Daylight detector (§ 3.14), every 20 ticks.
    pub(crate) fn daylight_update(&mut self, p: P) {
        if self.dim.name != "minecraft:overworld" {
            return;
        }
        self.daylight_signal(p);
    }

    /// Recomputed daylight detector signal (tick or click that inverts it); zero sky in a
    /// dimension without sky light.
    pub(crate) fn daylight_signal(&mut self, p: P) {
        let s = self.get(p);
        let name = self.dim.name.clone();
        let sky = self.dim_type_field(&name, "has_skylight").and_then(|v| v.as_bool()).unwrap_or(true);
        let raw = if sky { self.light_at(p, true) } else { 0 };
        let mut target = (raw - self.sky_darken()).max(0);
        let mut angle = self.sun_angle();
        if self.v().get_bool(s, "inverted") {
            target = 15 - target;
        } else if target > 0 {
            let offset = if angle < std::f32::consts::PI { 0.0 } else { std::f32::consts::TAU };
            angle += (offset - angle) * 0.2;
            target = (target as f32 * angle.cos()).round() as i32;
        }
        let target = target.clamp(0, 15);
        if self.v().get_int(s, "power") != target {
            let ns = self.v().set_int(s, "power", target);
            self.set_update(p, ns);
        }
    }

    /// Oxidation stage of an unwaxed copper block (0..3), `None` otherwise. The final stage
    /// (oxidized), which no longer ticks, still counts as an older neighbour.
    pub(crate) fn copper_stage(&self, s: u32) -> Option<u8> {
        let a = *self.bs.t.weather.get(self.v().block(s) as usize)?;
        (a != u8::MAX).then_some(a)
    }

    fn copper_next(&self, s: u32) -> Option<u32> {
        let n = self.data.block_of(s)?.name.strip_prefix("minecraft:")?.to_owned();
        let next = if let Some(r) = n.strip_prefix("exposed_") {
            format!("weathered_{r}")
        } else if let Some(r) = n.strip_prefix("weathered_") {
            format!("oxidized_{r}")
        } else if n.starts_with("oxidized_") {
            return None;
        } else if n == "copper_block" {
            "exposed_copper".to_owned()
        } else {
            format!("exposed_{n}")
        };
        let b = self.bs.t.block_id(&next);
        if b == u16::MAX {
            return None;
        }
        // Same properties: same offset in the state list.
        let first = self.data.blocks[self.v().block(s) as usize].first_state;
        Some(self.data.blocks[b as usize].first_state + (s - first))
    }

    pub(crate) fn copper_random_tick(&mut self, s: u32, p: P) {
        let Some(age) = self.copper_stage(s) else { return };
        // A door ages from its lower half only (the upper half copies it); a chest from its
        // single or left half, and never while someone has it open.
        let v = self.v();
        match v.kind(s) {
            Kind::Door { .. } if v.get(s, "half") == Some("upper") => return,
            Kind::Chest if v.get(s, "type") == Some("right") || self.life.openers.contains_key(&p) => return,
            _ => {}
        }
        if self.bs.rng.next_float() >= 0.056_888_89 {
            return;
        }
        let (mut older, mut same) = (0, 0);
        for dx in -4i32..=4 {
            for dy in -4i32..=4 {
                for dz in -4i32..=4 {
                    if dx.abs() + dy.abs() + dz.abs() > 4 || (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    if let Some(a) = self.copper_stage(self.get(add(p, dx, dy, dz))) {
                        if a < age {
                            return;
                        }
                        if a > age {
                            older += 1;
                        } else {
                            same += 1;
                        }
                    }
                }
            }
        }
        let chance = (older + 1) as f32 / (older + same + 1) as f32;
        let actual = chance * chance * if age == 0 { 0.75 } else { 1.0 };
        if self.bs.rng.next_float() < actual
            && let Some(ns) = self.copper_next(s)
        {
            self.set_update(p, ns);
        }
    }

    /// Local light (darkened sky, block), the maximum raw local brightness.
    pub(crate) fn local_brightness(&self, p: P) -> i32 {
        (self.light_at(p, true) - self.sky_darken()).max(self.light_at(p, false))
    }

    /// Rule `fire_spread_radius_around_player`: a player close enough.
    pub(crate) fn can_spread_fire_around(&self, p: P) -> bool {
        let r = self
            .rules
            .get("minecraft:fire_spread_radius_around_player")
            .and_then(rc_proto::nbt::Nbt::as_i64)
            .unwrap_or(128);
        if r < 0 {
            return true;
        }
        let r = r as f64;
        let (dx, dy, dz) = (
            self.pos[0] - (f64::from(p[0]) + 0.5),
            self.pos[1] - (f64::from(p[1]) + 0.5),
            self.pos[2] - (f64::from(p[2]) + 0.5),
        );
        dx * dx + dy * dy + dz * dz < r * r
    }

    // --- Gravity (§ 2.2). ---

    fn is_free(&self, s: u32) -> bool {
        let v = self.v();
        v.is_air(s)
            || matches!(v.kind(s), Kind::Fire | Kind::SoulFire)
            || v.info(s).has(rc_data::flags::LIQUID)
            || v.replaceable(s)
    }

    pub(crate) fn falling_tick(&mut self, s: u32, p: P) {
        if matches!(self.state_name(s), "minecraft:suspicious_sand" | "minecraft:suspicious_gravel") {
            self.brush_check_reset(p); // G5-F10
        }
        if self.is_free(self.get(below(p))) && p[1] >= self.dim.height.min_y {
            self.spawn_falling_block(p, s);
        }
    }

    // --- Fire (§ 2.3). ---

    fn ignite_odds(&self, s: u32) -> u8 {
        if self.v().get_bool(s, "waterlogged") {
            return 0;
        }
        self.bs.t.fire.get(self.v().block(s) as usize).map_or(0, |f| f.0)
    }

    fn burn_odds(&self, s: u32) -> u8 {
        if self.v().get_bool(s, "waterlogged") {
            return 0;
        }
        self.bs.t.fire.get(self.v().block(s) as usize).map_or(0, |f| f.1)
    }

    fn can_burn(&self, s: u32) -> bool {
        self.ignite_odds(s) > 0
    }

    pub(crate) fn fire_valid_location(&self, p: P) -> bool {
        Dir::ALL.iter().any(|&d| self.can_burn(self.get(rel(p, d))))
    }

    /// State of the fire placed at `p`.
    pub(crate) fn fire_state_for(&self, p: P) -> u32 {
        let v = self.v();
        let b = self.get(below(p));
        if self.tag_id(crate::tag_bits::TagId::SoulFireBaseBlocks, v.block(b)) {
            return v.default_state(self.bs.t.id.soul_fire);
        }
        let mut s = v.default_state(self.bs.t.id.fire);
        if !self.can_burn(b) && !v.sturdy(b, Dir::Up) {
            for d in Dir::ALL {
                if d != Dir::Down {
                    s = v.set_bool(s, d.name(), self.can_burn(self.get(rel(p, d))));
                }
            }
        }
        s
    }

    pub(crate) fn fire_on_place(&mut self, s: u32, p: P, old: u32) {
        // S6: an empty portal frame around lights up.
        if self.v().block(old) != self.v().block(s) && !self.fire_lights_portal(p) && !self.can_survive_fire(s, p) {
            self.remove_block(p, false);
        }
        if self.v().kind(s) == Kind::Fire {
            let delay = 30 + self.bs.rng.next_int_bound(10);
            let b = self.v().block(s);
            self.schedule_tick(p, b, delay);
        }
    }

    pub(crate) fn can_survive_fire(&self, s: u32, p: P) -> bool {
        if self.v().kind(s) == Kind::SoulFire {
            self.tag_id(crate::tag_bits::TagId::SoulFireBaseBlocks, self.v().block(self.get(below(p))))
        } else {
            self.can_survive(s, p)
        }
    }

    fn infiniburn(&self, s: u32) -> bool {
        let tag = match self.dim.name.as_str() {
            "minecraft:the_nether" => "minecraft:infiniburn_nether",
            "minecraft:the_end" => "minecraft:infiniburn_end",
            _ => "minecraft:infiniburn_overworld",
        };
        self.tag(tag, self.v().block(s))
    }

    fn near_rain(&self, p: P) -> bool {
        self.bs_raining_at(p) || Dir::HORIZONTAL.iter().any(|&d| self.bs_raining_at(rel(p, d)))
    }

    fn fire_tick(&mut self, s: u32, p: P) {
        let b = self.v().block(s);
        let delay = 30 + self.bs.rng.next_int_bound(10);
        self.schedule_tick(p, b, delay);
        if !self.can_spread_fire_around(p) {
            return;
        }
        let mut s = s;
        if !self.can_survive(s, p) {
            self.remove_block(p, false);
        }
        let infinite = self.infiniburn(self.get(below(p)));
        let age = self.v().get_int(s, "age");
        if !infinite && self.bs_raining() && self.near_rain(p) && self.bs.rng.next_float() < 0.2 + age as f32 * 0.03 {
            self.remove_block(p, false);
            return;
        }
        let new_age = (age + self.bs.rng.next_int_bound(3) / 2).min(15);
        if age != new_age {
            s = self.v().set_int(s, "age", new_age);
            self.set_block(p, s, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
        }
        if !infinite {
            if !self.fire_valid_location(p) {
                if !self.v().sturdy(self.get(below(p)), Dir::Up) || age > 3 {
                    self.remove_block(p, false);
                }
                return;
            }
            if age == 15 && self.bs.rng.next_int_bound(4) == 0 && !self.can_burn(self.get(below(p))) {
                self.remove_block(p, false);
                return;
            }
        }
        let humid = self.increased_fire_burnout(p);
        let extra = if humid { -50 } else { 0 };
        for (d, chance) in
            [(Dir::East, 300), (Dir::West, 300), (Dir::Down, 250), (Dir::Up, 250), (Dir::North, 300), (Dir::South, 300)]
        {
            self.check_burn_out(rel(p, d), chance + extra, age);
        }
        let diff = self.level.difficulty_id();
        for xx in -1..=1 {
            for zz in -1..=1 {
                for yy in -1..=4 {
                    if xx == 0 && yy == 0 && zz == 0 {
                        continue;
                    }
                    let mut rate = 100;
                    if yy > 1 {
                        rate += (yy - 1) * 100;
                    }
                    let t = add(p, xx, yy, zz);
                    let ignite = self.ignite_odds_at(t);
                    if ignite > 0 {
                        let mut odds = (ignite + 40 + diff * 7) / (age + 30);
                        if humid {
                            odds /= 2;
                        }
                        if odds > 0
                            && self.bs.rng.next_int_bound(rate) <= odds
                            && (!self.bs_raining() || !self.near_rain(t))
                        {
                            let spread = (age + self.bs.rng.next_int_bound(5) / 4).min(15);
                            let f = self.fire_with_age(t, spread);
                            self.set_update(t, f);
                        }
                    }
                }
            }
        }
    }

    fn ignite_odds_at(&self, p: P) -> i32 {
        if !self.v().is_air(self.get(p)) {
            return 0;
        }
        Dir::ALL.iter().map(|&d| i32::from(self.ignite_odds(self.get(rel(p, d))))).max().unwrap_or(0)
    }

    fn fire_with_age(&self, p: P, age: i32) -> u32 {
        let f = self.fire_state_for(p);
        if self.v().block(f) == self.bs.t.id.fire { self.v().set_int(f, "age", age) } else { f }
    }

    fn check_burn_out(&mut self, p: P, chance: i32, age: i32) {
        let odds = i32::from(self.burn_odds(self.get(p)));
        if self.bs.rng.next_int_bound(chance) < odds {
            let old = self.get(p);
            if self.bs.rng.next_int_bound(age + 10) < 5 && !self.bs_raining_at(p) {
                let na = (age + self.bs.rng.next_int_bound(5) / 4).min(15);
                let f = self.fire_with_age(p, na);
                self.set_update(p, f);
            } else {
                self.remove_block(p, false);
            }
            if self.v().kind(old) == Kind::Tnt {
                self.tnt_prime_block(p, None);
            }
        }
    }

    // --- Scheduled ticks of the other families. ---

    pub(crate) fn nature_tick(&mut self, s: u32, p: P) {
        let v = self.v();
        match v.kind(s) {
            Kind::Fire => self.fire_tick(s, p),
            Kind::Leaves => {
                let ns = self.leaf_update_distance(s, p);
                self.set_block(p, ns, flag::ALL);
            }
            Kind::SugarCane | Kind::Cactus => {
                if !self.can_survive(s, p) {
                    self.destroy_block(p, true, 512);
                }
            }
            Kind::Farmland => {
                if !self.can_survive(s, p) {
                    self.turn_to_dirt(p, None);
                }
            }
            // Dirt path: turns back into dirt unconditionally on its tick.
            _ if v.block(s) == self.bs.t.block_id("minecraft:dirt_path") => self.turn_to_dirt(p, None),
            Kind::FrostedIce => self.frosted_ice_tick(s, p),
            // G6-F08: the drop of a stalactite reaches the cauldron (the other two cauldrons
            // never take one).
            _ if v.block(s) == self.bs.t.id.cauldron || v.block(s) == self.bs.t.id.water_cauldron => {
                self.cauldron_drip_tick(s, p);
            }
            // G6-F08: eyeblossom woken by a neighbour that turned (short sound).
            _ if self.is_eyeblossom(v.block(s)) => {
                if self.eyeblossom_try_turn(s, p) {
                    let open = v.block(s) != self.bs.t.id.open_eyeblossom;
                    let sound = if open { "minecraft:block.eyeblossom.open" } else { "minecraft:block.eyeblossom.close" };
                    self.block_sound(p, sound, 1.0, 1.0);
                }
            }
            // F05: full composter (level 7 → 8, "ready" sound).
            _ if self.state_name(s) == "minecraft:composter" && v.get_int(s, "level") == 7 => {
                let ns = v.set_int(s, "level", 8);
                self.set_update(p, ns);
                let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
                self.world_sound("minecraft:block.composter.ready", 4, c, 1.0, 1.0);
            }
            _ => {}
        }
    }

    pub(crate) fn leaf_distance_of(&self, s: u32) -> i32 {
        let v = self.v();
        if self.tag_id(crate::tag_bits::TagId::PreventsNearbyLeafDecay, v.block(s)) {
            0
        } else if v.kind(s) == Kind::Leaves {
            v.get_int(s, "distance")
        } else {
            7
        }
    }

    pub(crate) fn leaf_update_distance(&self, s: u32, p: P) -> u32 {
        let mut d = 7;
        for dir in Dir::ALL {
            d = d.min(self.leaf_distance_of(self.get(rel(p, dir))) + 1);
            if d == 1 {
                break;
            }
        }
        self.v().set_int(s, "distance", d)
    }

    /// Farmland (or path) turned back into dirt: entities raised from 15/16 to 1, placement,
    /// "block changed" event attributed to `src` (the entity trampling it; none on the tick).
    pub(crate) fn turn_to_dirt(&mut self, p: P, src: Option<crate::mobs::mob::Ent>) {
        let dirt = self.v().default_state(self.bs.t.id.dirt);
        self.push_entities_up(p, 15.0 / 16.0, 1.0);
        self.set_update(p, dirt);
        self.game_event_block("minecraft:block_change", p, src, Some(dirt));
    }

    /// Rise of the mob being ticked, taken out of the list and therefore absent from
    /// `push_entities_up`: same rule as for the other entities (the one trampling the farmland
    /// is raised with them, before its fall damage).
    pub(crate) fn push_mob_up(m: &mut crate::mobs::mob::Mob, p: P, lo: f64, hi: f64) {
        if let Some(dy) = push_up_dy(&m.bbox(), p, [0.0, lo, 0.0], [1.0, hi, 1.0]) {
            m.pos[1] += dy;
            m.old = m.pos;
            m.needs_sync = true;
        }
    }

    /// Entities that touch the part gained by a block's collision shape (full width,
    /// height `lo` → `hi` as a fraction of a block): raised by a block then lowered onto the
    /// new top, by at most one block; spectators are never pushed.
    pub(crate) fn push_entities_up(&mut self, p: P, lo: f64, hi: f64) {
        self.push_entities_up_box(p, [0.0, lo, 0.0], [1.0, hi, 1.0]);
    }

    /// Same push for a gained part that occupies only a box of the block (fractions `lo` →
    /// `hi` on each axis), like the eye placed in the End portal frame.
    pub(crate) fn push_entities_up_box(&mut self, p: P, lo: [f64; 3], hi: [f64; 3]) {
        use crate::commands::ents::E;
        if (0..3).any(|i| hi[i] <= lo[i]) {
            return;
        }
        for e in self.all_ents() {
            if matches!(e, E::Player) && self.game_mode == 3 {
                continue;
            }
            let Some(dy) = push_up_dy(&self.e_bbox(e), p, lo, hi) else { continue };
            match e {
                E::Player => self.teleport_up(dy),
                E::Mob(i) => {
                    if let Some(m) = self.mobs.list.iter_mut().flatten().find(|m| m.id == i) {
                        m.pos[1] += dy;
                        m.old = m.pos;
                        m.needs_sync = true;
                    }
                }
                _ => {
                    let mut at = self.e_pos(e);
                    at[1] += dy;
                    self.ent_teleport(e, at, None);
                }
            }
        }
    }

    pub(crate) fn maintains_farmland(&self, p: P) -> bool {
        self.tag_id(crate::tag_bits::TagId::MaintainsFarmland, self.v().block(self.get(above(p))))
    }

    fn frosted_ice_tick(&mut self, s: u32, p: P) {
        let b = self.v().block(s);
        let fewer = Dir::ALL.iter().filter(|&&d| self.v().block(self.get(rel(p, d))) == b).count() < 4;
        if self.bs.rng.next_int_bound(3) == 0 || fewer {
            let bright =
                if self.dim.name == "minecraft:the_end" { self.light_at(p, false) } else { self.local_brightness(p) };
            let damp = i32::from(self.v().info(s).light_dampening);
            if bright > 11 - self.v().get_int(s, "age") - damp && self.frosted_melt(s, p) {
                for d in Dir::ALL {
                    let q = rel(p, d);
                    let qs = self.get(q);
                    if self.v().block(qs) == b && !self.frosted_melt(qs, q) {
                        let delay = 20 + self.bs.rng.next_int_bound(21);
                        self.schedule_tick(q, b, delay);
                    }
                }
                return;
            }
        }
        let delay = 20 + self.bs.rng.next_int_bound(21);
        self.schedule_tick(p, b, delay);
    }

    fn frosted_melt(&mut self, s: u32, p: P) -> bool {
        let age = self.v().get_int(s, "age");
        if age < 3 {
            let ns = self.v().set_int(s, "age", age + 1);
            self.set_block(p, ns, flag::CLIENTS);
            false
        } else {
            self.ice_melt(p, s);
            true
        }
    }

    /// Melting of frosted ice (`melt`) triggered by a neighbour.
    pub(crate) fn frosted_ice_melt(&mut self, p: P) {
        let s = self.get(p);
        self.ice_melt(p, s);
    }

    /// Melting of ice (frosted too): evaporated in the Nether, otherwise water, neighbour notified and
    /// "block destroyed" game event (ice state).
    fn ice_melt(&mut self, p: P, s: u32) {
        if self.dim.name == "minecraft:the_nether" {
            self.remove_block(p, false);
        } else {
            let water = self.v().default_state(self.bs.t.id.water);
            self.set_update(p, water);
            let wb = self.bs.t.id.water;
            self.neighbor_changed_at(p, wb);
            self.game_event_block("minecraft:block_destroy", p, None, Some(s));
        }
    }

    // --- Random ticks. ---

    pub(crate) fn nature_random_tick(&mut self, s: u32, p: P) {
        let v = self.v();
        let b = v.block(s);
        match v.kind(s) {
            Kind::Lava => self.lava_random_tick(p),
            Kind::Crop { max_age } => {
                let name = v.d.block_of(s).map_or("", |x| x.name.as_str());
                if (name == "minecraft:beetroots" || name == "minecraft:torchflower_crop")
                    && self.bs.rng.next_int_bound(3) == 0
                {
                    return;
                }
                if self.raw_brightness(p) >= 9 {
                    let age = v.get_int(s, "age");
                    if age < i32::from(max_age) {
                        let speed = self.growth_speed(b, p);
                        if self.bs.rng.next_int_bound((25.0 / speed) as i32 + 1) == 0 {
                            let ns = if name == "minecraft:torchflower_crop" && age + 1 > 1 {
                                self.v().default_state(self.bs.t.block_id("torchflower"))
                            } else {
                                self.v().set_int(s, "age", age + 1)
                            };
                            self.set_block(p, ns, flag::CLIENTS);
                        }
                    }
                }
            }
            Kind::Stem => self.stem_random_tick(s, p),
            Kind::NetherWart => {
                let age = v.get_int(s, "age");
                if age < 3 && self.bs.rng.next_int_bound(10) == 0 {
                    let ns = self.v().set_int(s, "age", age + 1);
                    self.set_block(p, ns, flag::CLIENTS);
                }
            }
            Kind::Cocoa => {
                if self.bs.rng.next_int_bound(5) == 0 {
                    let age = v.get_int(s, "age");
                    if age < 2 {
                        let ns = self.v().set_int(s, "age", age + 1);
                        self.set_block(p, ns, flag::CLIENTS);
                    }
                }
            }
            Kind::SweetBerryBush => {
                let age = v.get_int(s, "age");
                if age < 3 && self.bs.rng.next_int_bound(5) == 0 && self.raw_brightness(above(p)) >= 9 {
                    let ns = self.v().set_int(s, "age", age + 1);
                    self.set_block(p, ns, flag::CLIENTS);
                }
            }
            Kind::Sapling => {
                // Propagule: hanging, it matures (age); placed, it grows with no light
                // condition.
                let propagule = v.has(s, "hanging");
                if propagule && v.get_bool(s, "hanging") {
                    let age = v.get_int(s, "age");
                    if age < 4 {
                        let ns = self.v().set_int(s, "age", age + 1);
                        self.set_block(p, ns, flag::CLIENTS);
                    }
                } else if propagule {
                    if self.bs.rng.next_int_bound(7) == 0 {
                        self.advance_tree(p, s);
                    }
                } else if self.local_brightness(above(p)) >= 9 && self.bs.rng.next_int_bound(7) == 0 {
                    // Stage 0 → 1, then the tree.
                    self.advance_tree(p, s);
                }
            }
            Kind::Spreading => self.grass_random_tick(s, p),
            Kind::Nylium => {
                // Only the light through the top face counts (no snow or fluid rule).
                if self.light.dampening_into(s, self.get(above(p)), Dir::Up as usize) >= 15 {
                    let n = self.v().default_state(self.bs.t.id.netherrack);
                    self.set_update(p, n);
                }
            }
            Kind::Leaves => {
                if !v.get_bool(s, "persistent") && v.get_int(s, "distance") == 7 {
                    // Leaf that withers: its loot (saplings, sticks, apples) then removal.
                    self.drop_resources(p, s, None);
                    self.remove_block(p, false);
                }
            }
            Kind::Ice => {
                let damp = i32::from(v.info(s).light_dampening);
                if self.light_at(p, false) > 11 - damp {
                    self.ice_melt(p, s);
                }
            }
            Kind::Snow => {
                if self.light_at(p, false) > 11 {
                    self.remove_block(p, false);
                }
            }
            Kind::Farmland => {
                let moisture = v.get_int(s, "moisture");
                if !self.near_water(p) && !self.bs_raining_at(above(p)) {
                    if moisture > 0 {
                        let ns = self.v().set_int(s, "moisture", moisture - 1);
                        self.set_block(p, ns, flag::CLIENTS);
                    } else if !self.maintains_farmland(p) {
                        self.turn_to_dirt(p, None);
                    }
                } else if moisture < 7 {
                    let ns = self.v().set_int(s, "moisture", 7);
                    self.set_block(p, ns, flag::CLIENTS);
                }
            }
            Kind::SugarCane => {
                if self.v().is_air(self.get(above(p))) {
                    let mut h = 1;
                    while self.v().block(self.get(add(p, 0, -h, 0))) == b {
                        h += 1;
                    }
                    if h < 3 {
                        // The age is never sent to the clients (flags 260).
                        let age = v.get_int(s, "age");
                        if age == 15 {
                            let d = self.v().default_state(b);
                            self.set_update(above(p), d);
                            let ns = self.v().set_int(s, "age", 0);
                            self.set_block(p, ns, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
                        } else {
                            let ns = self.v().set_int(s, "age", age + 1);
                            self.set_block(p, ns, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
                        }
                    }
                }
            }
            // G6-F08: pitcher crop, lower half not ripe (only one that ticks): crop draw.
            Kind::PitcherCrop => {
                let speed = self.growth_speed(b, p);
                if self.bs.rng.next_int_bound((25.0 / speed) as i32 + 1) == 0 {
                    self.pitcher_grow(p, s, 1);
                }
            }
            Kind::Mushroom => self.mushroom_random_tick(s, p), // G6-F08
            Kind::Cactus => self.cactus_random_tick(s, p),
            Kind::Vine => self.vine_random_tick(s, p),
            Kind::Bamboo { sapling } => self.bamboo_random_tick(s, p, sapling),
            Kind::GrowingHead { up } => self.growing_random_tick(s, p, up),
            Kind::ChorusFlower => self.chorus_flower_random_tick(s, p),
            Kind::BuddingAmethyst => self.budding_amethyst_random_tick(p),
            Kind::Speleothem { dripstone } => self.speleothem_random_tick(s, p, dripstone), // G6-F09a
            _ => {
                let id = &self.bs.t.id;
                if b == id.redstone_ore || b == id.deepslate_redstone_ore {
                    if v.get_bool(s, "lit") {
                        let ns = v.set_bool(s, "lit", false);
                        self.set_block(p, ns, flag::ALL);
                    }
                } else if b == id.turtle_egg {
                    self.turtle_egg_random_tick(s, p); // G6-F08
                } else if self.is_eyeblossom(b) {
                    if self.eyeblossom_try_turn(s, p) {
                        let open = b != self.bs.t.id.open_eyeblossom;
                        let sound =
                            if open { "minecraft:block.eyeblossom.open_long" } else { "minecraft:block.eyeblossom.close_long" };
                        self.block_sound(p, sound, 1.0, 1.0);
                    }
                } else if b == id.potted_open_eyeblossom || b == id.potted_closed_eyeblossom {
                    self.potted_eyeblossom_random_tick(s, p);
                } else {
                    self.copper_random_tick(s, p);
                }
            }
        }
    }

    // --- Eyeblossoms (G6-F08): open at night in the overworld. ---

    pub(crate) fn is_eyeblossom(&self, b: u16) -> bool {
        b == self.bs.t.id.open_eyeblossom || b == self.bs.t.id.closed_eyeblossom
    }

    /// Environment attribute "eyeblossom open": the overworld day timeline opens them from
    /// 12,600 to 23,400; elsewhere no opinion (`None`: the flower keeps its state).
    fn eyeblossom_should_open(&self) -> Option<bool> {
        (self.dim.name == "minecraft:overworld").then(|| (12_600..23_401).contains(&self.day_time()))
    }

    /// Trail particle of an eyeblossom turned `open` (or closed): from the block's centre towards
    /// the point reached at a drawn speed (world random: lifetime 0.5 to 1.5 s, then the speed's
    /// three components, upwards), in as many ticks as 20 per second of lifetime, in the colour
    /// of the new state.
    fn eyeblossom_trail(&mut self, p: P, open: bool) {
        let r = &mut self.bs.rng;
        let life = 0.5 + r.next_double();
        let v = [r.next_double() - 0.5, r.next_double() + 1.0, r.next_double() - 0.5];
        let start = p.map(|c| f64::from(c) + 0.5);
        let target = [start[0] + v[0] * life, start[1] + v[1] * life, start[2] + v[2] * life];
        let color = if open { 0xFC_7812 } else { 0x5F_5F5F };
        self.trail_particle(start, target, color, (20.0 * life) as i32, false);
    }

    /// Eyeblossom that disagrees with the time of day: turned (flags 3, "block changed" with the
    /// old state), trail particle, then every identical flower within ±3, ±2, ±3 gets a tick in
    /// distance × 5 to distance × 10 ticks (one draw each, box scan order). True if it turned.
    pub(crate) fn eyeblossom_try_turn(&mut self, s: u32, p: P) -> bool {
        let open = self.v().block(s) == self.bs.t.id.open_eyeblossom;
        if self.eyeblossom_should_open().unwrap_or(open) == open {
            return false;
        }
        let to = if open { self.bs.t.id.closed_eyeblossom } else { self.bs.t.id.open_eyeblossom };
        let ns = self.v().default_state(to);
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, None, Some(s));
        self.eyeblossom_trail(p, !open);
        let mut same = Vec::new();
        self.scan_box(add(p, -3, -2, -3), add(p, 3, 2, 3), |q| q == s, |q, _| {
            same.push(q);
            true
        });
        let b = self.v().block(s);
        for q in same {
            let d2 = (0..3).map(|i| f64::from(q[i] - p[i]).powi(2)).sum::<f64>();
            let d = d2.sqrt();
            let (lo, hi) = ((d * 5.0) as i32, (d * 10.0) as i32);
            let delay = lo + self.bs.rng.next_int_bound(hi - lo + 1);
            self.schedule_tick(q, b, delay);
        }
        true
    }

    /// Potted eyeblossom: turned at its random tick like the flower, without event or neighbours
    /// (trail particle, then the long sound of the new state).
    fn potted_eyeblossom_random_tick(&mut self, s: u32, p: P) {
        let open = self.v().block(s) == self.bs.t.id.potted_open_eyeblossom;
        if self.eyeblossom_should_open().unwrap_or(open) == open {
            return;
        }
        let to = if open { self.bs.t.id.potted_closed_eyeblossom } else { self.bs.t.id.potted_open_eyeblossom };
        let ns = self.v().default_state(to);
        self.set_update(p, ns);
        self.eyeblossom_trail(p, !open);
        let sound = if open { "minecraft:block.eyeblossom.close_long" } else { "minecraft:block.eyeblossom.open_long" };
        self.block_sound(p, sound, 1.0, 1.0);
    }

    /// Mushroom (G6-F08): one chance in 25, if at most 4 of the same mushroom stand in the 9×3×9
    /// box; four random hops towards empty cells where it would hold, then a copy (flags 2) at
    /// the last drawn cell if it is empty and holds it.
    fn mushroom_random_tick(&mut self, s: u32, p: P) {
        if self.bs.rng.next_int_bound(25) != 0 || !self.at_most_in_box(p, [-4, -1, -4], [4, 1, 4], self.v().block(s), 4) {
            return;
        }
        let hop = |sv: &mut Self, from: P| {
            let dx = sv.bs.rng.next_int_bound(3) - 1;
            let dy = sv.bs.rng.next_int_bound(2) - sv.bs.rng.next_int_bound(2);
            let dz = sv.bs.rng.next_int_bound(3) - 1;
            add(from, dx, dy, dz)
        };
        let fits = |sv: &Self, q: P| sv.v().is_air(sv.get(q)) && sv.can_survive(s, q);
        let mut base = p;
        let mut q = hop(self, base);
        for _ in 0..4 {
            if fits(self, q) {
                base = q;
            }
            q = hop(self, base);
        }
        if fits(self, q) {
            self.set_block(q, s, flag::CLIENTS);
        }
    }

    fn growth_speed(&self, block: u16, p: P) -> f32 {
        let v = self.v();
        let mut speed = 1.0f32;
        let bp = below(p);
        for xx in -1..=1 {
            for zz in -1..=1 {
                let mut bsp = 0.0f32;
                let s = self.get(add(bp, xx, 0, zz));
                if self.tag_id(crate::tag_bits::TagId::GrowsCrops, v.block(s)) {
                    bsp = 1.0;
                    if v.get_int(s, "moisture") > 0 {
                        bsp = 3.0;
                    }
                }
                if xx != 0 || zz != 0 {
                    bsp /= 4.0;
                }
                speed += bsp;
            }
        }
        let is = |q: P| v.block(self.get(q)) == block;
        let (n, so, w, e) = (rel(p, Dir::North), rel(p, Dir::South), rel(p, Dir::West), rel(p, Dir::East));
        let horizontal = is(w) || is(e);
        let vertical = is(n) || is(so);
        let diagonal =
            || is(rel(w, Dir::North)) || is(rel(e, Dir::North)) || is(rel(e, Dir::South)) || is(rel(w, Dir::South));
        if (horizontal && vertical) || diagonal() {
            speed /= 2.0;
        }
        speed
    }

    pub(crate) fn stem_random_tick(&mut self, s: u32, p: P) {
        if self.raw_brightness(p) < 9 {
            return;
        }
        let b = self.v().block(s);
        let speed = self.growth_speed(b, p);
        if self.bs.rng.next_int_bound((25.0 / speed) as i32 + 1) != 0 {
            return;
        }
        let age = self.v().get_int(s, "age");
        if age < 7 {
            let ns = self.v().set_int(s, "age", age + 1);
            self.set_block(p, ns, flag::CLIENTS);
            return;
        }
        let d = Dir::HORIZONTAL[self.bs.rng.next_int_bound(4) as usize];
        let rp = rel(p, d);
        let below_s = self.get(below(rp));
        if self.v().is_air(self.get(rp)) && self.tag_id(crate::tag_bits::TagId::SupportsStemFruit, self.v().block(below_s)) {
            let name = self.data.block_of(s).map_or("", |x| x.name.as_str());
            let fruit = if name.contains("pumpkin") { "pumpkin" } else { "melon" };
            let fs = self.v().default_state(self.bs.t.block_id(fruit));
            self.set_update(rp, fs);
            let attached = self.bs.t.pair[b as usize];
            let a = self.v().set_dir(self.v().default_state(attached), "facing", d);
            self.set_update(p, a);
        }
    }

    /// Grass and mycelium stay alive: a single snow layer above always lets them, a full fluid
    /// never; otherwise the light entering through their top face must not be fully blocked,
    /// shape of the block above included (a bottom slab, stairs or thicker snow kill them).
    fn can_be_grass(&self, s: u32, p: P) -> bool {
        let a = self.get(above(p));
        let v = self.v();
        if v.block(a) == self.bs.t.id.snow && v.get_int(a, "layers") == 1 {
            return true;
        }
        if self.fluid_of(a).amount == 8 {
            return false;
        }
        self.light.dampening_into(s, a, Dir::Up as usize) < 15
    }

    fn grass_random_tick(&mut self, s: u32, p: P) {
        if !self.can_be_grass(s, p) {
            let dirt = self.v().default_state(self.bs.t.id.dirt);
            self.set_update(p, dirt);
            return;
        }
        if self.local_brightness(above(p)) < 9 {
            return;
        }
        let def = self.v().default_state(self.v().block(s));
        for _ in 0..4 {
            let dx = self.bs.rng.next_int_bound(3) - 1;
            let dy = self.bs.rng.next_int_bound(5) - 3;
            let dz = self.bs.rng.next_int_bound(3) - 1;
            let t = add(p, dx, dy, dz);
            if self.v().block(self.get(t)) == self.bs.t.id.dirt
                && self.can_be_grass(def, t)
                && self.fluid_at(above(t)).kind != 1
            {
                let snowy = self.tag_id(crate::tag_bits::TagId::Snow, self.v().block(self.get(above(t))));
                let ns = self.v().set_bool(def, "snowy", snowy);
                self.set_update(t, ns);
            }
        }
    }

    fn near_water(&self, p: P) -> bool {
        for x in -4..=4 {
            for y in 0..=1 {
                for z in -4..=4 {
                    if self.fluid_at(add(p, x, y, z)).kind == 1 {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn cactus_random_tick(&mut self, s: u32, p: P) {
        let b = self.v().block(s);
        let a = above(p);
        if !self.v().is_air(self.get(a)) {
            return;
        }
        let mut h = 1;
        let age = self.v().get_int(s, "age");
        while self.v().block(self.get(add(p, 0, -h, 0))) == b {
            h += 1;
            if h == 3 && age == 15 {
                return;
            }
        }
        let def = self.v().default_state(b);
        if age == 8 && self.can_survive(def, a) {
            let chance = if h >= 3 { 0.25 } else { 0.1 };
            if self.bs.rng.next_double() <= chance {
                let flower = self.v().default_state(self.bs.t.id.cactus_flower);
                self.set_update(a, flower);
            }
        } else if age == 15 && h < 3 {
            self.set_update(a, def);
            let ns = self.v().set_int(s, "age", 0);
            self.set_block(p, ns, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
            self.neighbor_changed_full(ns, a, b, false);
        }
        // The age is never sent to the clients (flags 260).
        if age < 15 {
            let ns = self.v().set_int(s, "age", age + 1);
            self.set_block(p, ns, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
        }
    }

    /// Precipitation of a column (§ 2.7): water freezing (biome of the top, temperature of the
    /// block below), then under rain: snow that accumulates (height rule), and block
    /// below the top that receives the precipitation (cauldrons).
    pub(crate) fn tick_precipitation(&mut self, p: P) {
        let top = self.motion_blocking_height(p[0], p[2]);
        let tp = [p[0], top, p[2]];
        let bp = [p[0], top - 1, p[2]];
        if !self.is_loaded(bp) {
            return;
        }
        if self.should_freeze_with(tp, bp) {
            let ice = self.v().default_state(self.bs.t.id.ice);
            self.set_update(bp, ice);
        }
        if !self.bs_raining() {
            return;
        }
        let max =
            self.rules.get("minecraft:max_snow_accumulation_height").and_then(rc_proto::nbt::Nbt::as_i64).unwrap_or(1)
                as i32;
        if max > 0 && self.should_snow(tp) {
            let s = self.get(tp);
            if self.v().block(s) == self.bs.t.id.snow {
                let layers = self.v().get_int(s, "layers");
                if layers < max.min(8) {
                    let ns = self.v().set_int(s, "layers", layers + 1);
                    // Snow collision: one layer less than the thickness.
                    self.push_entities_up(tp, f64::from(layers - 1) / 8.0, f64::from(layers) / 8.0);
                    self.set_update(tp, ns);
                }
            } else {
                let snow = self.v().default_state(self.bs.t.id.snow);
                self.set_update(tp, snow);
            }
        }
        let pr = self.biome_precipitation(tp, bp);
        if pr != Precip::None {
            let s = self.get(bp);
            self.handle_precipitation(s, bp, pr);
        }
    }

    /// Snow of the biome at `p`, block light < 10, air or snow, viable snow.
    fn should_snow(&self, p: P) -> bool {
        if self.biome_precipitation(p, p) != Precip::Snow {
            return false;
        }
        if !self.in_build_height(p[1]) || self.light_at(p, false) >= 10 {
            return false;
        }
        let s = self.get(p);
        let snow = self.v().default_state(self.bs.t.id.snow);
        (self.v().is_air(s) || self.v().block(s) == self.bs.t.id.snow) && self.can_survive(snow, p)
    }

    /// Cauldron under rain or snow: empty → one level of water or powder snow
    /// (5 % / 10 %), filled with the same type → one more level (draw made even if it is full).
    fn handle_precipitation(&mut self, s: u32, p: P, pr: Precip) {
        let name = self.state_name(s).to_owned();
        let layered = match name.as_str() {
            "minecraft:cauldron" => None,
            "minecraft:water_cauldron" => Some(Precip::Rain),
            "minecraft:powder_snow_cauldron" => Some(Precip::Snow),
            _ => return,
        };
        let chance = if pr == Precip::Rain { 0.05 } else { 0.1 };
        if self.bs.rng.next_float() >= chance {
            return;
        }
        match layered {
            None => {
                let to = if pr == Precip::Rain { "minecraft:water_cauldron" } else { "minecraft:powder_snow_cauldron" };
                let ns = self.default_state(to);
                self.set_update(p, ns);
                self.game_event_block("minecraft:block_change", p, None, None);
            }
            Some(kind) => {
                let level = self.v().get_int(s, "level");
                if level != 3 && kind == pr {
                    let ns = self.v().set_int(s, "level", level + 1);
                    self.set_update(p, ns);
                    self.game_event_block("minecraft:block_change", p, None, Some(ns));
                }
            }
        }
    }

    /// `MOTION_BLOCKING` heightmap (live map of the chunk; fallback scan).
    pub(crate) fn motion_blocking_height(&self, x: i32, z: i32) -> i32 {
        if let Some(Some(c)) = self.chunks.get(&(x >> 4, z >> 4))
            && c.heightmaps.iter().any(|(k, _)| *k == 4)
        {
            return self.surface_y(x, z, 4);
        }
        let min = self.dim.height.min_y;
        let mut y = self.top_y() - 1;
        while y >= min {
            let s = self.get([x, y, z]);
            if self.v().info(s).collision != 0 || !self.fluid_of(s).is_empty() {
                return y + 1;
            }
            y -= 1;
        }
        min
    }

    /// Biome attribute "fire goes out faster" (jungles, swamps, peaks…).
    fn increased_fire_burnout(&self, p: P) -> bool {
        self.biome_of(p)
            .and_then(|b| b.attributes.iter().find(|e| e.attr == "gameplay/increased_fire_burnout"))
            .is_some_and(|e| e.arg[0] != 0.0)
    }

    /// Biome of a block like the game: quart (a 4×4×4 biome cell) chosen by the fuzzy zoom
    /// (hashed seed), then its stored biome; quart outside the loaded chunks: that of the block without zoom.
    pub(crate) fn biome_of(&self, p: P) -> Option<&rc_data::Biome> {
        let (qx, qy, qz) = rc_worldgen::biome::zoom(self.wgs.zoom_seed, p[0], p[1], p[2]);
        self.biome_at_quart(qx, qy, qz).or_else(|| self.biome_at_quart(p[0] >> 2, p[1] >> 2, p[2] >> 2))
    }

    /// Stored biome of the quart `(qx, qy, qz)` (height clamped to the sections of the chunk).
    fn biome_at_quart(&self, qx: i32, qy: i32, qz: i32) -> Option<&rc_data::Biome> {
        let c = self.chunks.get(&(qx >> 2, qz >> 2))?.as_ref()?;
        let top = (c.sections.len() as i32 * 4 - 1).max(0);
        let lq = (qy - (self.dim.height.min_y >> 2)).clamp(0, top);
        let sec = c.sections.get(usize::try_from(lq >> 2).ok()?)?;
        let i = ((lq & 3) << 4) | ((qz & 3) << 2) | (qx & 3);
        let b = sec.biomes.get(i as usize);
        let k = *self.states.biome_data.get(b as usize)?;
        self.data.biomes.get(usize::from(k))
    }

    /// The freeze test of the biome taken at `at_biome`, evaluated at block `p` (adjusted
    /// temperature: "frozen" modifier and altitude).
    fn should_freeze_with(&self, at_biome: P, p: P) -> bool {
        let Some(b) = self.biome_of(at_biome) else { return false };
        if biome_temperature_at(b.temperature, b.frozen, p, crate::mobs::surface::sea_level(self)) >= 0.15 {
            return false;
        }
        if !self.in_build_height(p[1]) || self.light_at(p, false) >= 10 {
            return false;
        }
        let s = self.get(p);
        if self.v().kind(s) != Kind::Water || !self.fluid_of(s).source {
            return false;
        }
        let water = |q: P| self.fluid_at(q).kind == 1;
        !(water(rel(p, Dir::West))
            && water(rel(p, Dir::East))
            && water(rel(p, Dir::North))
            && water(rel(p, Dir::South)))
    }
}
