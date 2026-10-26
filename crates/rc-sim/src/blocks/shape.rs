//! Shape updates by family (§ 1.8, § 2.13): connections (fences, walls, panes, stairs,
//! double chests), linked halves (doors, beds, double plants), blocks that no longer
//! hold (torches, levers, plants, carpets, snow…), scheduled ticks (fluids, sand,
//! leaves, sugar cane, cactus, observers).

use super::{Dir, Kind, P, above, below, rel};
use crate::server::Server;

impl Server {
    pub(crate) fn shape_of(&mut self, s: u32, kind: Kind, p: P, dir: Dir, np: P, ns: u32) -> u32 {
        let v = self.v();
        match kind {
            Kind::Wire => self.wire_update_shape(s, p, dir, ns),
            Kind::Repeater => {
                if dir == Dir::Down && !v.rigid_up(ns) {
                    self.air()
                } else if dir.axis() != v.facing(s).axis() {
                    let locked = self.repeater_locked(p, s);
                    self.v().set_bool(s, "locked", locked)
                } else {
                    s
                }
            }
            Kind::Comparator => {
                if dir == Dir::Down && !v.rigid_up(ns) {
                    self.air()
                } else {
                    s
                }
            }
            Kind::Observer => self.observer_update_shape(s, p, dir),
            Kind::RedstoneTorch | Kind::Torch => {
                if dir == Dir::Down && !v.center_up(self.support_state(below(p))) {
                    self.air()
                } else {
                    s
                }
            }
            Kind::RedstoneWallTorch | Kind::WallTorch => {
                let f = v.facing(s);
                if dir.opposite() == f && !self.wall_attach(p, f) { self.air() } else { s }
            }
            Kind::Lever | Kind::Button { .. } | Kind::Grindstone => {
                let c = super::redstone::connected_dir(&v, s);
                if c.opposite() == dir && !self.face_attach(p, c.opposite()) { self.air() } else { s }
            }
            Kind::Plate { .. } | Kind::WeightedPlate { .. } => {
                let b = self.support_state(below(p));
                if dir == Dir::Down && !(v.rigid_up(b) || v.center_up(b)) { self.air() } else { s }
            }
            Kind::PistonHead => {
                if dir.opposite() == v.facing(s) && !self.piston_head_survives(s, p) {
                    self.air()
                } else {
                    s
                }
            }
            Kind::Water | Kind::Lava => self.liquid_update_shape(s, p, ns),
            Kind::Falling { .. } | Kind::Anvil => {
                let b = v.block(s);
                self.schedule_tick(p, b, 2);
                s
            }
            Kind::DragonEgg => {
                let b = v.block(s);
                self.schedule_tick(p, b, 5);
                s
            }
            Kind::ConcretePowder => {
                if self.touches_liquid(p) {
                    let c = self.bs.t.pair[v.block(s) as usize];
                    self.v().default_state(c)
                } else {
                    let b = v.block(s);
                    self.schedule_tick(p, b, 2);
                    s
                }
            }
            Kind::Leaves => {
                let d = self.leaf_distance_of(ns) + 1;
                if d != 1 || v.get_int(s, "distance") != d {
                    let b = v.block(s);
                    self.schedule_tick(p, b, 1);
                }
                s
            }
            Kind::Fence => {
                if dir.horizontal() {
                    let c = self.fence_connects(s, ns, v.sturdy(ns, dir.opposite()), dir.opposite());
                    self.v().set_bool(s, dir.name(), c)
                } else {
                    s
                }
            }
            Kind::Pane => {
                if dir.horizontal() {
                    let c = self.pane_attaches(ns, v.sturdy(ns, dir.opposite()));
                    self.v().set_bool(s, dir.name(), c)
                } else {
                    s
                }
            }
            Kind::Wall => match dir {
                Dir::Down => s,
                Dir::Up => {
                    let c = [Dir::North, Dir::East, Dir::South, Dir::West].map(|d| v.get(s, d.name()) != Some("none"));
                    self.wall_shape(s, np, ns, c)
                }
                _ => {
                    let mut c =
                        [Dir::North, Dir::East, Dir::South, Dir::West].map(|d| v.get(s, d.name()) != Some("none"));
                    let i = [Dir::North, Dir::East, Dir::South, Dir::West].iter().position(|&d| d == dir).unwrap_or(0);
                    c[i] = self.wall_connects(ns, v.sturdy(ns, dir.opposite()), dir.opposite());
                    let a = above(p);
                    let as_ = self.get(a);
                    self.wall_shape(s, a, as_, c)
                }
            },
            Kind::Stairs => {
                if dir.horizontal() {
                    let sh = self.stairs_shape(s, p);
                    self.v().set(s, "shape", sh)
                } else {
                    s
                }
            }
            Kind::FenceGate => {
                if v.facing(s).clockwise().axis() != dir.axis() {
                    s
                } else {
                    let o = self.get(rel(p, dir.opposite()));
                    let in_wall = self.is_wall(ns) || self.is_wall(o);
                    self.v().set_bool(s, "in_wall", in_wall)
                }
            }
            Kind::Door { .. } => {
                let half = v.get(s, "half").unwrap_or("lower");
                let lower = half == "lower";
                if dir.axis() != 1 || lower != (dir == Dir::Up) {
                    if lower && dir == Dir::Down && !v.sturdy(self.support_state(below(p)), Dir::Up) {
                        self.air()
                    } else {
                        s
                    }
                } else if matches!(v.kind(ns), Kind::Door { .. }) && v.get(ns, "half") != Some(half) {
                    self.v().set(ns, "half", half)
                } else {
                    self.air()
                }
            }
            Kind::Bed => {
                let part = v.get(s, "part").unwrap_or("foot");
                let f = v.facing(s);
                let nd = if part == "foot" { f } else { f.opposite() };
                if dir == nd {
                    if v.block(ns) == v.block(s) && v.get(ns, "part") != Some(part) {
                        let occ = v.get_bool(ns, "occupied");
                        self.v().set_bool(s, "occupied", occ)
                    } else {
                        self.air()
                    }
                } else {
                    s
                }
            }
            Kind::DoublePlant => self.double_plant_shape(s, p, dir, ns),
            // Pitcher crop: double plant rule once tall (age 3+),
            // otherwise a simple survival test.
            Kind::PitcherCrop => {
                if v.get_int(s, "age") >= 3 {
                    self.double_plant_shape(s, p, dir, ns)
                } else if self.can_survive(s, p) {
                    s
                } else {
                    self.air()
                }
            }
            Kind::Bush
            | Kind::DeadBush
            | Kind::Sapling
            | Kind::Crop { .. }
            | Kind::Stem
            | Kind::NetherWart
            | Kind::SweetBerryBush
            | Kind::Mushroom
            | Kind::Carpet
            | Kind::Snow => {
                if self.can_survive(s, p) {
                    s
                } else {
                    self.air()
                }
            }
            Kind::AttachedStem => {
                let fruit_ok = {
                    let n = v.d.block_of(ns).map_or("", |b| b.name.as_str());
                    let mine = v.d.block_of(s).map_or("", |b| b.name.as_str());
                    (mine.contains("pumpkin") && n == "minecraft:pumpkin")
                        || (mine.contains("melon") && n == "minecraft:melon")
                };
                if !fruit_ok && dir == v.facing(s) {
                    let stem = self.bs.t.pair[v.block(s) as usize];
                    self.v().set_int(self.v().default_state(stem), "age", 7)
                } else if self.can_survive(s, p) {
                    s
                } else {
                    self.air()
                }
            }
            Kind::Cocoa => {
                if dir == v.facing(s) && !self.tag_id(crate::tag_bits::TagId::SupportsCocoa, v.block(ns)) {
                    self.air()
                } else {
                    s
                }
            }
            Kind::Ladder => {
                let f = v.facing(s);
                if dir.opposite() == f && !self.wall_attach(p, f) { self.air() } else { s }
            }
            Kind::Lantern => {
                let hanging = v.get_bool(s, "hanging");
                let c = if hanging { Dir::Down } else { Dir::Up };
                if c.opposite() == dir {
                    let support = self.get(rel(p, dir));
                    let ok = if hanging { v.center_down(support) } else { v.center_up(support) };
                    if !ok {
                        return self.air();
                    }
                }
                s
            }
            Kind::StandingSign => {
                if dir == Dir::Down && !self.v().info(self.support_state(below(p))).has(rc_data::flags::LEGACY_SOLID) {
                    self.air()
                } else {
                    s
                }
            }
            Kind::WallSign => {
                let f = v.facing(s);
                if dir.opposite() == f
                    && !self.v().info(self.support_state(rel(p, f.opposite()))).has(rc_data::flags::LEGACY_SOLID)
                {
                    self.air()
                } else {
                    s
                }
            }
            Kind::SugarCane | Kind::Cactus => {
                if !self.can_survive(s, p) {
                    let b = v.block(s);
                    self.schedule_tick(p, b, 1);
                }
                s
            }
            Kind::Farmland => {
                if dir == Dir::Up && !self.can_survive(s, p) {
                    let b = v.block(s);
                    self.schedule_tick(p, b, 1);
                }
                s
            }
            Kind::Fire => {
                if self.can_survive(s, p) {
                    let age = v.get_int(s, "age");
                    let f = self.fire_state_for(p);
                    if self.v().block(f) == self.bs.t.id.fire { self.v().set_int(f, "age", age) } else { f }
                } else {
                    self.air()
                }
            }
            Kind::SoulFire => {
                if self.tag_id(crate::tag_bits::TagId::SoulFireBaseBlocks, v.block(self.support_state(below(p)))) {
                    v.default_state(v.block(s))
                } else {
                    self.air()
                }
            }
            Kind::Chest => self.chest_shape(s, dir, ns),
            Kind::Rail | Kind::PoweredRail | Kind::DetectorRail => s,
            Kind::TripwireHook => {
                if dir.opposite() == v.facing(s) && !self.hook_survives(s, p) {
                    self.air()
                } else {
                    s
                }
            }
            Kind::Tripwire => {
                if dir.horizontal() {
                    let c = self.tripwire_connects(ns, dir);
                    self.v().set_bool(s, dir.name(), c)
                } else {
                    s
                }
            }
            Kind::NoteBlock if dir.axis() == 1 => self.note_with_instrument(s, p),
            Kind::Vine => self.vine_update_shape(s, p, dir),
            Kind::Multiface => self.multiface_update_shape(s, p, dir),
            Kind::GrowingHead { up } => self.growing_update_shape(s, p, dir, ns, true, up),
            Kind::GrowingBody { up } => self.growing_update_shape(s, p, dir, ns, false, up),
            Kind::Support(sup) => self.support_update_shape(sup, s, p, dir),
            Kind::Bamboo { sapling } => self.bamboo_update_shape(s, p, dir, ns, sapling),
            Kind::Scaffolding => {
                let b = v.block(s);
                self.schedule_tick(p, b, 1);
                s
            }
            Kind::BigDripleaf | Kind::BigDripleafStem => self.dripleaf_update_shape(s, kind, p, dir, ns),
            Kind::ChorusFlower | Kind::ChorusPlant => self.chorus_update_shape(s, kind, p, dir, ns),
            Kind::Bell => self.bell_update_shape(s, p, dir, np, ns),
            // Huge mushroom: the face against the same block goes out (never relit).
            Kind::HugeMushroom if v.block(ns) == v.block(s) => v.set_bool(s, dir.name(), false),
            Kind::HangingMoss => self.hanging_moss_update_shape(s, p),
            Kind::PaleMossCarpet => self.moss_carpet_update_shape(s, p),
            // Campfire: signal (high smoke) depending on the hay bale below.
            Kind::Campfire if dir == Dir::Down => {
                let hay = v.block(ns) == self.bs.t.block_id("minecraft:hay_block");
                v.set_bool(s, "signal_fire", hay)
            }
            // Beehive: a neighbouring fire makes the bees come out.
            Kind::Beehive => {
                if v.kind(self.get(np)) == Kind::Fire {
                    self.hive_fire_near(p);
                }
                s
            }
            // Covered dirt path: turns back into dirt on the next tick (like
            // farmland).
            _ if dir == Dir::Up && v.block(s) == self.bs.t.block_id("minecraft:dirt_path") => {
                if !self.can_survive(s, p) {
                    let b = v.block(s);
                    self.schedule_tick(p, b, 1);
                }
                s
            }
            _ => s,
        }
    }

    /// Linked halves of a double plant: the half that loses its sister → air; the lower one also tests
    /// its survival when the block below changes.
    fn double_plant_shape(&self, s: u32, p: P, dir: Dir, ns: u32) -> u32 {
        let v = self.v();
        let half = v.get(s, "half").unwrap_or("lower");
        let lower = half == "lower";
        if dir.axis() != 1 || lower != (dir == Dir::Up) || (v.block(ns) == v.block(s) && v.get(ns, "half") != Some(half))
        {
            if lower && dir == Dir::Down && !self.can_survive(s, p) { self.air() } else { s }
        } else {
            self.air()
        }
    }

    /// Does the block hold in place?
    pub(crate) fn can_survive(&self, s: u32, p: P) -> bool {
        let v = self.v();
        let bp = below(p);
        let b = self.support_state(bp);
        let bb = v.block(b);
        let veg = |tag: &str| self.tag(tag, bb);
        match v.kind(s) {
            // Mangrove propagule: hanging, holds to the block above (dedicated tag);
            // placed, to the block below (other tag).
            Kind::Sapling if v.block(s) == self.bs.t.id.mangrove_propagule => {
                if v.get_bool(s, "hanging") {
                    self.tag("minecraft:supports_hanging_mangrove_propagule", v.block(self.get(above(p))))
                } else {
                    veg("minecraft:supports_mangrove_propagule")
                }
            }
            Kind::Bush | Kind::Sapling => veg("minecraft:supports_vegetation"),
            Kind::DeadBush => veg("minecraft:supports_dry_vegetation"),
            Kind::Crop { .. } => self.raw_brightness(p) >= 8 && veg("minecraft:supports_crops"),
            Kind::Stem => veg("minecraft:supports_stem_crops") || veg("minecraft:supports_crops"),
            Kind::AttachedStem => veg("minecraft:supports_stem_crops") || veg("minecraft:supports_crops"),
            Kind::NetherWart => veg("minecraft:supports_nether_wart"),
            Kind::SweetBerryBush => veg("minecraft:supports_vegetation"),
            Kind::Mushroom => {
                veg("minecraft:overrides_mushroom_light_requirement")
                    || (self.raw_brightness(p) < 13 && v.info(b).has(rc_data::flags::SOLID_RENDER))
            }
            Kind::DoublePlant => {
                if v.get(s, "half") == Some("upper") {
                    bb == v.block(s) && v.get(b, "half") == Some("lower")
                } else if v.block(s) == self.bs.t.id.tall_seagrass {
                    let f = self.fluid_at(p);
                    v.sturdy(b, Dir::Up) && !veg("minecraft:cannot_support_seagrass") && f.kind == 1 && f.source
                } else if v.has(s, "facing") {
                    // Small dripleaf: block of its tag, or vegetation ground under a
                    // source water.
                    veg("minecraft:supports_small_dripleaf") || {
                        let f = self.fluid_at(p);
                        f.kind == 1 && f.source && veg("minecraft:supports_vegetation")
                    }
                } else {
                    veg("minecraft:supports_vegetation")
                }
            }
            Kind::PitcherCrop => {
                if v.get(s, "half") == Some("upper") {
                    bb == v.block(s) && v.get(b, "half") == Some("lower")
                } else {
                    self.raw_brightness(p) >= 8 && veg("minecraft:supports_crops")
                }
            }
            Kind::Carpet => !v.is_air(b),
            Kind::Snow => {
                if veg("minecraft:cannot_support_snow_layer") {
                    false
                } else if veg("minecraft:support_override_snow_layer") {
                    true
                } else {
                    let full_top = self.face_full_up(b);
                    full_top || (bb == self.bs.t.id.snow && v.get_int(b, "layers") == 8)
                }
            }
            Kind::SugarCane => {
                if bb == self.bs.t.id.sugar_cane {
                    return true;
                }
                if veg("minecraft:supports_sugar_cane") {
                    for d in Dir::HORIZONTAL {
                        let q = rel(bp, d);
                        let qs = self.get(q);
                        let f = self.fluid_of(qs);
                        if f.kind == 1 || self.tag_id(crate::tag_bits::TagId::SupportsSugarCaneAdjacently, v.block(qs)) {
                            return true;
                        }
                    }
                }
                false
            }
            Kind::Cactus => {
                for d in Dir::HORIZONTAL {
                    let q = rel(p, d);
                    let qs = self.get(q);
                    if v.legacy_solid(qs) || self.fluid_of(qs).kind == 2 {
                        return false;
                    }
                }
                (bb == self.bs.t.id.cactus || veg("minecraft:supports_cactus"))
                    && !v.info(self.get(above(p))).has(rc_data::flags::LIQUID)
            }
            Kind::Farmland => !v.legacy_solid(self.get(above(p))) || self.maintains_farmland(p),
            Kind::Fire => v.sturdy(b, Dir::Up) || self.fire_valid_location(p),
            Kind::TripwireHook => self.hook_survives(s, p),
            Kind::Vine => self.vine_survives(s, p),
            Kind::Multiface => self.multiface_survives(s, p),
            Kind::GrowingHead { up } | Kind::GrowingBody { up } => self.growing_survives(s, p, up),
            Kind::Support(sup) => self.support_survives(sup, s, p),
            Kind::Bamboo { .. } => self.bamboo_survives(p),
            Kind::Scaffolding => self.scaffold_distance(p) < 7,
            Kind::BigDripleaf | Kind::BigDripleafStem | Kind::ChorusFlower | Kind::ChorusPlant => {
                self.plant_rule_survives(s, p)
            }
            Kind::Bell => self.bell_survives(s, p),
            Kind::HangingMoss => self.hanging_moss_survives(s, p),
            Kind::PaleMossCarpet => self.moss_carpet_survives(s, p),
            // Dirt path: nothing solid above, except a fence gate.
            _ if v.block(s) == self.bs.t.block_id("minecraft:dirt_path") => {
                let up = self.get(above(p));
                !v.legacy_solid(up) || v.kind(up) == Kind::FenceGate
            }
            _ => true,
        }
    }

    pub(crate) fn face_full_up(&self, s: u32) -> bool {
        let info = self.v().info(s);
        let m = self.bs.t.face_masks.get(info.collision as usize).map_or([0; 4], |m| m[Dir::Up as usize]);
        m.iter().all(|&w| w == u64::MAX)
    }

    pub(crate) fn wall_attach(&self, p: P, facing: Dir) -> bool {
        let q = rel(p, facing.opposite());
        self.v().sturdy(self.support_state(q), facing)
    }

    pub(crate) fn face_attach(&self, p: P, toward: Dir) -> bool {
        let q = rel(p, toward);
        self.v().sturdy(self.support_state(q), toward.opposite())
    }

    fn connection_exception(&self, s: u32) -> bool {
        let v = self.v();
        let n = v.d.block_of(s).map_or("", |b| b.name.as_str());
        v.kind(s) == Kind::Leaves
            || matches!(
                n,
                "minecraft:barrier"
                    | "minecraft:carved_pumpkin"
                    | "minecraft:jack_o_lantern"
                    | "minecraft:melon"
                    | "minecraft:pumpkin"
            )
            || self.tag_id(crate::tag_bits::TagId::ShulkerBoxes, v.block(s))
    }

    pub(crate) fn fence_connects(&self, me: u32, s: u32, solid: bool, d: Dir) -> bool {
        let v = self.v();
        let same = self.tag_id(crate::tag_bits::TagId::Fences, v.block(s))
            && self.tag_id(crate::tag_bits::TagId::WoodenFences, v.block(s)) == self.tag_id(crate::tag_bits::TagId::WoodenFences, v.block(me));
        let gate = v.kind(s) == Kind::FenceGate && v.facing(s).axis() == d.clockwise().axis();
        (!self.connection_exception(s) && solid) || same || gate
    }

    pub(crate) fn pane_attaches(&self, s: u32, solid: bool) -> bool {
        (!self.connection_exception(s) && solid) || self.v().kind(s) == Kind::Pane || self.is_wall(s)
    }

    pub(crate) fn is_wall(&self, s: u32) -> bool {
        self.tag_id(crate::tag_bits::TagId::Walls, self.v().block(s))
    }

    pub(crate) fn wall_connects(&self, s: u32, solid: bool, d: Dir) -> bool {
        let v = self.v();
        let gate = v.kind(s) == Kind::FenceGate && v.facing(s).axis() == d.clockwise().axis();
        self.is_wall(s) || (!self.connection_exception(s) && solid) || v.kind(s) == Kind::Pane || gate
    }

    /// Sides and post of a wall from the connections (north, east, south, west) and the block
    /// above.
    pub(crate) fn wall_shape(&self, s: u32, _top: P, top_s: u32, c: [bool; 4]) -> u32 {
        let v = self.v();
        let info = v.info(top_s);
        let above_mask = self.bs.t.face_masks.get(info.collision as usize).map_or([0; 4], |m| m[Dir::Down as usize]);
        let covered = |x0: usize, x1: usize, z0: usize, z1: usize| {
            (x0..x1).all(|x| {
                (z0..z1).all(|z| {
                    let bit = x * 16 + z;
                    above_mask[bit / 64] & (1 << (bit % 64)) != 0
                })
            })
        };
        // Test zones (sixteenths): sides towards the north, east, south, west; central post.
        let tests = [(7, 9, 0, 9), (7, 16, 7, 9), (7, 9, 7, 16), (0, 9, 7, 9)];
        let mut ns = s;
        for (i, d) in [Dir::North, Dir::East, Dir::South, Dir::West].iter().enumerate() {
            let side = if c[i] {
                let (x0, x1, z0, z1) = tests[i];
                if covered(x0, x1, z0, z1) { "tall" } else { "low" }
            } else {
                "none"
            };
            ns = v.set(ns, d.name(), side);
        }
        let post = {
            let top_post = v.kind(top_s) == Kind::Wall && v.get_bool(top_s, "up");
            if top_post {
                true
            } else {
                let g = |d: Dir| v.get(ns, d.name()).unwrap_or("none");
                let (n, so, e, w) = (g(Dir::North), g(Dir::South), g(Dir::East), g(Dir::West));
                let (nn, sn, en, wn) = (n == "none", so == "none", e == "none", w == "none");
                let corner = (nn && sn && wn && en) || nn != sn || wn != en;
                if corner {
                    true
                } else {
                    let high = (n == "tall" && so == "tall") || (e == "tall" && w == "tall");
                    if high {
                        false
                    } else {
                        self.tag_id(crate::tag_bits::TagId::WallPostOverride, v.block(top_s)) || covered(7, 9, 7, 9)
                    }
                }
            }
        };
        v.set_bool(ns, "up", post)
    }

    pub(crate) fn stairs_shape(&self, s: u32, p: P) -> &'static str {
        let v = self.v();
        let facing = v.facing(s);
        let half = v.get(s, "half");
        let behind = self.get(rel(p, facing));
        if v.kind(behind) == Kind::Stairs && v.get(behind, "half") == half {
            let bf = v.facing(behind);
            if bf.axis() != facing.axis() && self.stairs_can_take(s, p, bf.opposite()) {
                return if bf == facing.counter_clockwise() { "outer_left" } else { "outer_right" };
            }
        }
        let front = self.get(rel(p, facing.opposite()));
        if v.kind(front) == Kind::Stairs && v.get(front, "half") == half {
            let ff = v.facing(front);
            if ff.axis() != facing.axis() && self.stairs_can_take(s, p, ff) {
                return if ff == facing.counter_clockwise() { "inner_left" } else { "inner_right" };
            }
        }
        "straight"
    }

    fn stairs_can_take(&self, s: u32, p: P, d: Dir) -> bool {
        let v = self.v();
        let n = self.get(rel(p, d));
        v.kind(n) != Kind::Stairs || v.facing(n) != v.facing(s) || v.get(n, "half") != v.get(s, "half")
    }

    fn chest_connected_dir(&self, s: u32) -> Dir {
        let v = self.v();
        let f = v.facing(s);
        if v.get(s, "type") == Some("left") { f.clockwise() } else { f.counter_clockwise() }
    }

    /// Can chest `me` join state `other`? Same block for wooden chests;
    /// for copper, any copper chest (oxidation and wax mixed).
    pub(crate) fn chest_connects(&self, me: u32, other: u32) -> bool {
        let v = self.v();
        let (a, b) = (v.block(me), v.block(other));
        if self.copper_chest(a) { self.copper_chest(b) && v.has(other, "type") } else { a == b }
    }

    fn copper_chest(&self, b: u16) -> bool {
        self.tag("minecraft:copper_chests", b)
    }

    fn chest_shape(&self, s: u32, dir: Dir, ns: u32) -> u32 {
        let v = self.v();
        let connects = self.chest_connects(s, ns);
        let mut out = s;
        if connects && dir.horizontal() {
            let nt = v.get(ns, "type").unwrap_or("single");
            if v.get(s, "type") == Some("single")
                && nt != "single"
                && v.facing(s) == v.facing(ns)
                && self.chest_connected_dir(ns) == dir.opposite()
            {
                out = v.set(s, "type", if nt == "left" { "right" } else { "left" });
            }
        } else if !connects && v.get(s, "type") != Some("single") && self.chest_connected_dir(s) == dir {
            return v.set(s, "type", "single");
        }
        // Copper: the half takes the block of its joined neighbour (oxidation, wax), properties
        // kept; a half that ages therefore drags the other along instead of detaching.
        if connects
            && v.block(ns) != v.block(out)
            && self.copper_chest(v.block(out))
            && v.get(out, "type") != Some("single")
            && self.chest_connected_dir(out) == dir
        {
            return self.with_props_of(v.default_state(v.block(ns)), out);
        }
        out
    }

    /// Default state `target` that takes the values of the common properties of `from`.
    pub(crate) fn with_props_of(&self, target: u32, from: u32) -> u32 {
        let v = self.v();
        let mut out = target;
        for (name, values, i) in v.props_sorted(from) {
            out = v.set(out, name, &values[i]);
        }
        out
    }

    pub(crate) fn chest_other_half(&self, s: u32, p: P) -> Option<P> {
        let v = self.v();
        if v.kind(s) != Kind::Chest || v.get(s, "type").is_none_or(|t| t == "single") {
            return None;
        }
        Some(rel(p, self.chest_connected_dir(s)))
    }

    /// Concrete powder in contact with a liquid (§ 2.2).
    pub(crate) fn touches_liquid(&self, p: P) -> bool {
        // The cell tested before each direction is the last one visited (game quirk).
        let mut t = p;
        for d in Dir::ALL {
            if d != Dir::Down || self.can_solidify(self.get(t)) {
                t = rel(p, d);
                let s = self.get(t);
                if self.can_solidify(s) && !self.v().sturdy(s, d.opposite()) {
                    return true;
                }
            }
        }
        false
    }

    fn can_solidify(&self, s: u32) -> bool {
        self.fluid_of(s).kind == 1
    }

    /// Raw light (max of sky, block) at `p`, without sky darkening.
    pub(crate) fn raw_brightness(&self, p: P) -> i32 {
        self.light_at(p, true).max(self.light_at(p, false))
    }

    /// Sky light (`sky`) or block light stored at `p`.
    pub(crate) fn light_at(&self, p: P, sky: bool) -> i32 {
        let Some(Some(c)) = self.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { return if sky { 15 } else { 0 } };
        let ly = p[1] - self.dim.height.min_y;
        let li = (ly >> 4) + 1;
        let layers = if sky { &c.sky } else { &c.block };
        let Some(Some(l)) = layers.get(usize::try_from(li).unwrap_or(usize::MAX)) else {
            return if sky && li >= layers.len() as i32 { 15 } else { 0 };
        };
        let i = rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize);
        i32::from((l[i >> 1] >> ((i & 1) * 4)) & 15)
    }
}
