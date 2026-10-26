//! Portals (S6) like the official 26.3 server: lighting the Nether portal (flint and steel, fire
//! charge, placed fire), frame breaking, search and creation of the exit portal (radius 16
//! towards the Nether, 128 towards the surface, coordinate scale, obsidian platform),
//! passage of the player (delay, return), of items, falling blocks and mobs;
//! End portal frame (eyes), arrival platform, exit portal and credits,
//! End gateways. The dragon fight lives in `mobs/`; it creates the exit portal and
//! the gateways with the game's blocks, which this module knows how to cross.

use rc_client::{EntityEvent, Event};
use rc_physics::{Aabb, collide};
use rc_proto::nbt::Nbt;

use crate::blocks::{Dir, Kind, P, flag};
use crate::dims::{END, EntPortal, NETHER, OVERWORLD, PortalKind, Proc, Transition, rel};
use crate::portal_shape::{self as ps, Axis, Cell, Rect, Shape};
use crate::server::Server;

/// Arrival point in the End.
pub(crate) const END_SPAWN: P = [100, 50, 0];
/// Return delay of entities other than the player.
pub(crate) const ENTITY_COOLDOWN: i32 = 300;
pub(crate) const PLAYER_COOLDOWN: i32 = 10;
/// Return delay of a vehicle (minecart, boat).
const CART_COOLDOWN: i32 = 10;

fn floor3(p: [f64; 3]) -> P {
    [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32]
}

fn dist_sqr(a: P, b: P) -> i64 {
    let d = |i: usize| i64::from(a[i] - b[i]);
    d(0) * d(0) + d(1) * d(1) + d(2) * d(2)
}

/// Box of an entity of width `w` and height `h` placed at `p`.
fn ent_box(p: [f64; 3], w: f64, h: f64) -> Aabb {
    Aabb { min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0], max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0] }
}

fn hits(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.max[i] > b.min[i] && a.min[i] < b.max[i])
}

/// Portal touched by a box: (kind, position), in x, y, z order.
pub(crate) struct Touch {
    pub kind: PortalKind,
    pub pos: P,
}

impl Server {
    fn block_id_of(&self, name: &str) -> u16 {
        self.tb().block_id(name)
    }

    fn is_block(&self, s: u32, name: &str) -> bool {
        self.v().block(s) == self.block_id_of(name)
    }

    /// Cell seen by the search for the Nether portal frame.
    pub(crate) fn portal_cell(&self, p: P) -> Cell {
        let s = self.get(p);
        let v = self.v();
        let b = v.block(s);
        if b == self.block_id_of("nether_portal") {
            Cell::Portal
        } else if v.is_air(s) || self.tag_id(crate::tag_bits::TagId::Fire, b) {
            Cell::Empty
        } else if self.tag_id(crate::tag_bits::TagId::NetherPortalFrame, b) {
            Cell::Frame
        } else {
            Cell::Other
        }
    }

    fn portal_shape_at(&self, p: P, axis: Axis) -> Shape {
        ps::find_any_shape(&|q| self.portal_cell(q), self.dim.height.min_y, p, axis)
    }

    fn empty_portal_shape(&self, p: P, preferred: Axis) -> Option<Shape> {
        ps::find_empty_shape(&|q| self.portal_cell(q), self.dim.height.min_y, p, preferred)
    }

    fn in_portal_dimension(&self) -> bool {
        self.dim.name == OVERWORLD || self.dim.name == NETHER
    }

    fn nether_portal_state(&self, axis: Axis) -> u32 {
        let v = self.v();
        v.set(v.default_state(self.block_id_of("nether_portal")), "axis", axis.name())
    }

    fn create_portal_blocks(&mut self, s: &Shape) {
        let st = self.nether_portal_state(s.axis);
        for p in s.interior() {
            self.set_block(p, st, flag::CLIENTS | flag::KNOWN_SHAPE);
        }
    }

    /// Placed fire: an empty frame around it lights up; true if so.
    pub(crate) fn fire_lights_portal(&mut self, p: P) -> bool {
        if !self.in_portal_dimension() {
            return false;
        }
        let Some(shape) = self.empty_portal_shape(p, Axis::X) else { return false };
        self.create_portal_blocks(&shape);
        true
    }

    /// Shape update of a Nether portal block: air if the frame is no longer complete.
    pub(crate) fn portal_update_shape(&self, state: u32, p: P, dir: Dir, ns: u32) -> Option<u32> {
        let v = self.v();
        let pb = self.block_id_of("nether_portal");
        if v.block(state) != pb {
            return None;
        }
        let axis = v.get(state, "axis").and_then(Axis::parse).unwrap_or(Axis::X);
        let update_axis = match dir {
            Dir::West | Dir::East => Some(Axis::X),
            Dir::North | Dir::South => Some(Axis::Z),
            _ => None,
        };
        let wrong = update_axis.is_some_and(|a| a != axis);
        if !wrong && v.block(ns) != pb && !self.portal_shape_at(p, axis).is_complete() {
            Some(self.air())
        } else {
            Some(state)
        }
    }

    // --- Lighting by the player. ---

    /// Fire possible at `p`; `facing`: horizontal direction
    /// of the player.
    pub(crate) fn fire_can_be_placed(&self, p: P, facing: Dir) -> bool {
        let s = self.get(p);
        if !self.v().is_air(s) {
            return false;
        }
        let fire = self.fire_state_for(p);
        let survives = self.v().kind(fire) == Kind::SoulFire || self.can_survive(fire, p);
        survives || self.is_portal_spot(p, facing)
    }

    fn is_portal_spot(&self, p: P, facing: Dir) -> bool {
        if !self.in_portal_dimension() {
            return false;
        }
        for d in Dir::ALL {
            let q = ps::add(p, d.step(), 1);
            if self.portal_cell(q) == Cell::Frame {
                let preferred = match facing.counter_clockwise() {
                    Dir::West | Dir::East => Axis::X,
                    _ => Axis::Z,
                };
                return self.empty_portal_shape(p, preferred).is_some();
            }
        }
        false
    }

    /// Horizontal direction of the player.
    pub(crate) fn player_facing(&self) -> Dir {
        match ((f64::from(self.yaw) / 90.0 + 0.5).floor() as i32) & 3 {
            0 => Dir::South,
            1 => Dir::West,
            2 => Dir::North,
            _ => Dir::East,
        }
    }

    /// Flint and steel or fire charge on a block (/ use of a fire charge on a block);
    /// true if the item was used (or failed: the action is consumed).
    pub(crate) fn use_igniter(&mut self, item: &str, clicked: P, face: Dir) -> bool {
        let flint = item == "minecraft:flint_and_steel";
        if !flint && item != "minecraft:fire_charge" {
            return false;
        }
        let slot = 36 + self.held as usize;
        let s = self.get(clicked);
        let v = self.v();
        let name = self.data.block_of(s).map(|b| b.name.clone()).unwrap_or_default();
        let lightable = v.has(s, "lit")
            && !v.get_bool(s, "lit")
            && !v.get_bool(s, "waterlogged")
            && (name.ends_with("campfire") || name.ends_with("candle") || name.ends_with("candle_cake"));
        let mut used = false;
        if lightable {
            let lit = v.set_bool(s, "lit", true);
            self.igniter_sound(flint, clicked);
            self.set_block(clicked, lit, flag::ALL | 8);
            used = true;
        } else {
            let p = ps::add(clicked, face.step(), 1);
            if self.fire_can_be_placed(p, self.player_facing()) {
                self.igniter_sound(flint, p);
                let fire = self.fire_state_for(p);
                self.set_block(p, fire, if flint { flag::ALL | 8 } else { flag::ALL });
                used = true;
            }
        }
        if used {
            self.award_stat("minecraft:used", item, 1);
            if flint {
                self.hurt_item(slot, 1);
            } else if self.game_mode != 1
                && let Some(st) = &mut self.slots[slot]
            {
                st.count -= 1;
                if st.count <= 0 {
                    self.slots[slot] = None;
                }
            }
            self.broadcast_menus();
        }
        true
    }

    fn igniter_sound(&mut self, flint: bool, p: P) {
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        if flint {
            // Played by the acting client (sound excluded from the send); the world's random draw advances.
            let _ = self.bs.rng.next_float();
        } else {
            let a = self.bs.rng.next_float();
            let b = self.bs.rng.next_float();
            self.play_sound("minecraft:item.firecharge.use", 4, at, 1.0, (a - b) * 0.2 + 1.0);
        }
    }

    // --- Search and creation of the exit portal. ---

    /// `MOTION_BLOCKING` height of a column: first Y above the last blocking block.
    fn portal_surface_height(&self, x: i32, z: i32) -> i32 {
        let min = self.dim.height.min_y;
        let v = self.v();
        let cobweb = self.block_id_of("cobweb");
        let sapling = self.block_id_of("bamboo_sapling");
        for y in (min..self.top_y()).rev() {
            let s = self.get([x, y, z]);
            let b = v.block(s);
            let blocks = v.legacy_solid(s) && b != cobweb && b != sapling;
            if blocks || !self.fluid_of(s).is_empty() {
                return y + 1;
            }
        }
        min
    }

    /// Present chunks made complete at once (generation awaited).
    pub(crate) fn load_now(&mut self, list: &[(i32, i32)]) {
        self.ensure_loaded(list);
        let t = std::time::Instant::now();
        loop {
            let waiting = self.generator.as_ref().is_some_and(|g| list.iter().any(|p| g.pending.contains(p)));
            if !waiting || t.elapsed().as_secs() > 60 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.poll_gen();
        }
    }

    fn chunks_around(p: P, r: i32) -> Vec<(i32, i32)> {
        let (x0, x1, z0, z1) = ((p[0] - r) >> 4, (p[0] + r) >> 4, (p[2] - r) >> 4, (p[2] + r) >> 4);
        (x0..=x1).flat_map(|x| (z0..=z1).map(move |z| (x, z))).collect()
    }

    /// Nether portal blocks of a chunk (sections whose palette contains some).
    fn portal_blocks_in(&self, c: &crate::chunk::Chunk, out: &mut Vec<P>) {
        let pb = self.block_id_of("nether_portal");
        let v = self.v();
        let min_y = self.dim.height.min_y;
        for (si, sec) in c.sections.iter().enumerate() {
            let maybe = match &sec.blocks {
                rc_world::Container::Single(s) => v.block(*s) == pb,
                rc_world::Container::Packed { palette: Some(p), .. } => p.iter().any(|&s| v.block(s) == pb),
                rc_world::Container::Packed { palette: None, .. } => true,
            };
            if !maybe {
                continue;
            }
            for i in 0..4096usize {
                if v.block(sec.blocks.get(i)) == pb {
                    let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                    out.push([c.x * 16 + x as i32, min_y + si as i32 * 16 + y as i32, c.z * 16 + z as i32]);
                }
            }
        }
    }

    /// Nearest existing portal: square of radius 16 (towards
    /// the Nether) or 128, present chunks read without generation; the nearest then the lowest.
    pub(crate) fn find_closest_portal(&mut self, approx: P, to_nether: bool) -> Option<P> {
        let radius: i32 = if to_nether { 16 } else { 128 };
        let (cx, cz) = (approx[0] >> 4, approx[2] >> 4);
        let load_r = radius.div_euclid(16);
        let scan_r = load_r + 1;
        let mut found = Vec::new();
        let mut from_disk = Vec::new();
        for x in cx - scan_r..=cx + scan_r {
            for z in cz - scan_r..=cz + scan_r {
                match self.chunks.get(&(x, z)) {
                    Some(Some(c)) => self.portal_blocks_in(c, &mut found),
                    Some(None) => {}
                    None if (x - cx).abs() <= load_r && (z - cz).abs() <= load_r => from_disk.push((x, z)),
                    None => {}
                }
            }
        }
        if !from_disk.is_empty() {
            for (_, l) in self.dim.load(&from_disk, &self.data, &self.states) {
                if let crate::world::Load::Loaded(c) = l {
                    self.portal_blocks_in(&c, &mut found);
                }
            }
        }
        let bounds = self.border_bounds();
        let best = found
            .into_iter()
            .filter(|p| (p[0] - approx[0]).abs() <= radius && (p[2] - approx[2]).abs() <= radius)
            .filter(|p| Self::block_in_border(bounds, *p))
            .min_by_key(|p| (dist_sqr(*p, approx), p[1]))?;
        // Block re-read in the world (loaded chunk): it must still have an axis.
        self.load_now(&Self::chunks_around(best, 21));
        let s = self.get(best);
        self.v().has(s, "axis").then_some(best)
    }

    fn can_portal_replace(&self, p: P) -> bool {
        let s = self.get(p);
        (self.v().replaceable(s) || self.v().is_air(s)) && self.fluid_of(s).is_empty()
    }

    fn can_host_frame(&self, origin: P, dir: Dir, offset: i32) -> bool {
        let cw = dir.clockwise().step();
        let d = dir.step();
        for width in -1..3 {
            for height in -1..4 {
                let p = [
                    origin[0] + d[0] * width + cw[0] * offset,
                    origin[1] + height,
                    origin[2] + d[2] * width + cw[2] * offset,
                ];
                if height < 0 && !self.v().legacy_solid(self.get(p)) {
                    return false;
                }
                if height >= 0 && !self.can_portal_replace(p) {
                    return false;
                }
            }
        }
        true
    }

    /// Creation of the exit portal.
    pub(crate) fn create_portal(&mut self, origin: P, axis: Axis) -> Option<Rect> {
        let dir = match axis {
            Axis::X => Dir::East,
            Axis::Z => Dir::South,
        };
        let d = dir.step();
        let min_y = self.dim.height.min_y;
        let max_placeable = self.max_placeable_y();
        self.load_now(&Self::chunks_around(origin, 16 + 4));
        let (mut full_d, mut full_p): (f64, Option<P>) = (-1.0, None);
        let (mut part_d, mut part_p): (f64, Option<P>) = (-1.0, None);
        let bounds = self.border_bounds();
        let in_border = |p: P| Self::block_in_border(bounds, p);
        for col in ps::spiral(origin, 16, Dir::East, Dir::South) {
            let height = max_placeable.min(self.portal_surface_height(col[0], col[2]));
            if !(in_border(col) && in_border(ps::add(col, d, 1))) {
                continue;
            }
            let mut y = height;
            while y >= min_y {
                let mut p = [col[0], y, col[2]];
                if self.can_portal_replace(p) {
                    let first_empty = y;
                    while y > min_y {
                        p[1] -= 1;
                        if !self.can_portal_replace(p) {
                            break;
                        }
                        y -= 1;
                    }
                    if y + 4 <= max_placeable {
                        let dy = first_empty - y;
                        if dy <= 0 || dy >= 3 {
                            let at = [col[0], y, col[2]];
                            if self.can_host_frame(at, dir, 0) {
                                let dist = dist_sqr(origin, at) as f64;
                                if self.can_host_frame(at, dir, -1)
                                    && self.can_host_frame(at, dir, 1)
                                    && (full_d == -1.0 || full_d > dist)
                                {
                                    full_d = dist;
                                    full_p = Some(at);
                                }
                                if full_d == -1.0 && (part_d == -1.0 || part_d > dist) {
                                    part_d = dist;
                                    part_p = Some(at);
                                }
                            }
                        }
                    }
                }
                y -= 1;
            }
        }
        if full_d == -1.0 && part_d != -1.0 {
            full_p = part_p;
            full_d = part_d;
        }
        let obsidian = self.v().default_state(self.block_id_of("obsidian"));
        let air = self.air();
        let base = if full_d == -1.0 {
            let min_start = (min_y + 1).max(70);
            let max_start = max_placeable - 9;
            if max_start < min_start {
                return None;
            }
            let b = [origin[0] - d[0], origin[1].clamp(min_start, max_start), origin[2] - d[2]];
            let b = Self::clamp_to_border(bounds, f64::from(b[0]), f64::from(b[1]), f64::from(b[2]));
            // Brought back inside the border, the fallback can land far from the loaded chunks.
            self.load_now(&Self::chunks_around(b, 3));
            let cw = dir.clockwise().step();
            for bx in -1..2 {
                for w in 0..2 {
                    for h in -1..3 {
                        let p = [b[0] + w * d[0] + bx * cw[0], b[1] + h, b[2] + w * d[2] + bx * cw[2]];
                        self.set_block(p, if h < 0 { obsidian } else { air }, flag::ALL);
                    }
                }
            }
            b
        } else {
            full_p?
        };
        for w in -1..3 {
            for h in -1..4 {
                if w == -1 || w == 2 || h == -1 || h == 3 {
                    self.set_block([base[0] + w * d[0], base[1] + h, base[2] + w * d[2]], obsidian, flag::ALL);
                }
            }
        }
        let portal = self.nether_portal_state(axis);
        for w in 0..2 {
            for h in 0..3 {
                self.set_block(
                    [base[0] + w * d[0], base[1] + h, base[2] + w * d[2]],
                    portal,
                    flag::CLIENTS | flag::KNOWN_SHAPE,
                );
            }
        }
        Some(Rect { min: base, size1: 2, size2: 3 })
    }

    /// Rectangle of blocks identical to that of `p` (portal), axis of the state.
    fn portal_rect(&self, p: P) -> (Rect, Axis) {
        let s = self.get(p);
        let axis = self.v().get(s, "axis").and_then(Axis::parse).unwrap_or(Axis::X);
        let a1 = match axis {
            Axis::X => 0,
            Axis::Z => 2,
        };
        (ps::largest_rectangle(p, a1, 21, 1, 21, &|q| self.get(q) == s), axis)
    }

    /// Bounds of the world border (x min, x max, z min, z max), capped at the absolute size
    /// ±29,999,984 (the west bound of the border…).
    pub(crate) fn border_bounds(&self) -> [f64; 4] {
        let b = &self.cmd.border;
        let lim = 29_999_984.0;
        let half = b.size / 2.0;
        [
            (b.center[0] - half).clamp(-lim, lim),
            (b.center[0] + half).clamp(-lim, lim),
            (b.center[1] - half).clamp(-lim, lim),
            (b.center[1] + half).clamp(-lim, lim),
        ]
    }

    /// Block inside the border (low corner of the block).
    pub(crate) fn block_in_border(bounds: [f64; 4], p: P) -> bool {
        let (x, z) = (f64::from(p[0]), f64::from(p[2]));
        x >= bounds[0] && x < bounds[1] && z >= bounds[2] && z < bounds[3]
    }

    /// Low corner and high corner (minus 1e-5) inside the border.
    pub(crate) fn box_in_border(&self, b: &Aabb) -> bool {
        let w = self.border_bounds();
        let e = f64::from(1.0e-5f32);
        let ok = |x: f64, z: f64| x >= w[0] && x < w[1] && z >= w[2] && z < w[3];
        ok(b.min[0], b.min[2]) && ok(b.max[0] - e, b.max[2] - e)
    }

    /// The permitted-interaction test in the world for a player: the block must be inside the border (the
    /// spawn protection does not exist in singleplayer).
    pub(crate) fn may_interact(&self, p: P) -> bool {
        Self::block_in_border(self.border_bounds(), p)
    }

    /// Position brought back inside the border, block that contains it.
    fn clamp_to_border(bounds: [f64; 4], x: f64, y: f64, z: f64) -> P {
        let e = f64::from(1.0e-5f32);
        [
            x.clamp(bounds[0], bounds[1] - e).floor() as i32,
            y.floor() as i32,
            z.clamp(bounds[2], bounds[3] - e).floor() as i32,
        ]
    }

    /// Nearest free position for an entity:
    /// allowed centers = square of the width, 1 block high, grown by 1e-6; boxes of the neighbouring
    /// blocks inside the border, grown by half the size; free point nearest to the center.
    fn collision_free(&self, bottom: [f64; 3], w: f64, h: f64) -> [f64; 3] {
        if w > 4.0 || h > 4.0 {
            return bottom;
        }
        let center = [bottom[0], bottom[1] + h / 2.0, bottom[2]];
        let e = 1.0e-6;
        let allowed = (
            [center[0] - w / 2.0 - e, center[1] - e, center[2] - w / 2.0 - e],
            [center[0] + w / 2.0 + e, center[1] + 1.0 + e, center[2] + w / 2.0 + e],
        );
        self.free_position(allowed, center, w, h).map_or(bottom, |c| [c[0], c[1] - h / 2.0, c[2]])
    }

    /// Nearest free center to `center` among the allowed centers `allowed`, for a
    /// box w × h × w: collisions of the blocks around, inside the border,
    /// grown by half the size.
    pub(crate) fn free_position(&self, allowed: ps::Bounds, center: [f64; 3], w: f64, h: f64) -> Option<[f64; 3]> {
        let search = Aabb {
            min: [allowed.0[0] - w, allowed.0[1] - h, allowed.0[2] - w],
            max: [allowed.1[0] + w, allowed.1[1] + h, allowed.1[2] + w],
        };
        let mut boxes = Vec::new();
        collide::collect(&self.world(), &search, &mut boxes);
        let bounds = self.border_bounds();
        let eps = f64::from(1.0e-5f32);
        let inside = |x: f64, z: f64| x >= bounds[0] && x < bounds[1] && z >= bounds[2] && z < bounds[3];
        let grown: Vec<ps::Bounds> = boxes
            .iter()
            .filter(|b| inside(b.min[0], b.min[2]) && inside(b.max[0] - eps, b.max[2] - eps))
            .map(|b| {
                (
                    [b.min[0] - w / 2.0, b.min[1] - h / 2.0, b.min[2] - w / 2.0],
                    [b.max[0] + w / 2.0, b.max[1] + h / 2.0, b.max[2] + w / 2.0],
                )
            })
            .collect();
        ps::closest_free_point(allowed, &grown, center)
    }

    /// Destination through a Nether portal for an
    /// entity of size `w`×`h` at `pos`, entered at `entry`.
    pub(crate) fn nether_destination(
        &mut self,
        entry: P,
        pos: [f64; 3],
        w: f64,
        h: f64,
        spectator: bool,
    ) -> Option<Transition> {
        let from = self.dim.name.clone();
        let to = if from == NETHER { OVERWORLD } else { NETHER };
        if !self.world_dimensions().iter().any(|d| d == to) {
            return None;
        }
        let to_nether = to == NETHER;
        let scale = ps::teleport_scale(self.coordinate_scale(&from), self.coordinate_scale(to));
        // Border of the arrival dimension (a single simulated border for the whole world).
        let approx = Self::clamp_to_border(self.border_bounds(), pos[0] * scale, pos[1], pos[2] * scale);
        // Entry portal: axis, rectangle and relative position (departure world).
        let entry_state = self.get(entry);
        let (entry_axis, offset) = match self.v().get(entry_state, "axis").and_then(Axis::parse) {
            Some(axis) => {
                let (rect, _) = self.portal_rect(entry);
                (axis, ps::relative_position(rect, axis, pos, w, h))
            }
            None => (Axis::X, [0.5, 0.0, 0.0]),
        };
        let source_axis = entry_axis;
        self.in_world(to, |s| {
            let (rect, ticket_at) = match s.find_closest_portal(approx, to_nether) {
                Some(p) => (s.portal_rect(p).0, Some(p)),
                None => {
                    if spectator {
                        return None;
                    }
                    let r = s.create_portal(approx, source_axis);
                    if r.is_none() {
                        eprintln!("singleplayer: portal cannot be created (world border?)");
                    }
                    (r?, None)
                }
            };
            let _ = ticket_at;
            let exit_state = s.get(rect.min);
            let exit_axis = s.v().get(exit_state, "axis").and_then(Axis::parse).unwrap_or(Axis::X);
            let (target, rotation) = ps::exit_position(rect, exit_axis, entry_axis, offset, w, h);
            let free = s.collision_free(target, w, h);
            Some(Transition {
                dim: to.to_owned(),
                pos: free,
                yaw: rotation,
                pitch: 0.0,
                relative: rel::DELTA | rel::ROTATION,
                sound: true,
                ticket: true,
                missing_respawn: false,
            })
        })
    }

    /// Obsidian platform of the End.
    pub(crate) fn create_end_platform(&mut self, origin: P, drop_resources: bool) {
        let obsidian = self.block_id_of("obsidian");
        let air = self.v().default_state(self.tb().id.air);
        for dz in -2..=2 {
            for dx in -2..=2 {
                for dy in -1..3 {
                    let p = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
                    let want = if dy == -1 { obsidian } else { self.tb().id.air };
                    if self.v().block(self.get(p)) != want {
                        if drop_resources {
                            self.destroy_block(p, true, 512);
                        }
                        let st = if dy == -1 { self.v().default_state(obsidian) } else { air };
                        self.set_block(p, st, flag::ALL);
                    }
                }
            }
        }
    }

    /// Destination through an End portal.
    pub(crate) fn end_destination(&mut self, is_player: bool) -> Option<Transition> {
        let from_end = self.dim.name == END;
        if from_end {
            if is_player {
                return Some(self.respawn_transition(false));
            }
            let spawn_dim = self.level.spawn_dimension.clone();
            let sp = self.level.spawn_pos;
            let (yaw, pitch) = (self.level.spawn_yaw, self.level.spawn_pitch);
            return self.in_world(&spawn_dim, |s| {
                s.load_now(&Self::chunks_around(sp, 0));
                let top = s.heightmap_no_leaves(sp[0], sp[2]);
                Some(Transition {
                    dim: spawn_dim.clone(),
                    pos: [f64::from(sp[0]) + 0.5, f64::from(top), f64::from(sp[2]) + 0.5],
                    yaw,
                    pitch,
                    relative: rel::DELTA | rel::ROTATION,
                    sound: true,
                    ticket: true,
                    missing_respawn: false,
                })
            });
        }
        if !self.world_dimensions().iter().any(|d| d == END) {
            return None;
        }
        self.in_world(END, |s| {
            let base = [END_SPAWN[0], END_SPAWN[1] - 1, END_SPAWN[2]];
            s.load_now(&Self::chunks_around(base, 3));
            s.create_end_platform(base, true);
            let y = if is_player { f64::from(END_SPAWN[1]) - 1.0 } else { f64::from(END_SPAWN[1]) };
            Some(Transition {
                dim: END.to_owned(),
                pos: [f64::from(END_SPAWN[0]) + 0.5, y, f64::from(END_SPAWN[2]) + 0.5],
                yaw: 90.0,
                pitch: 0.0,
                relative: rel::DELTA | rel::X_ROT,
                sound: true,
                ticket: true,
                missing_respawn: false,
            })
        })
    }

    /// `MOTION_BLOCKING_NO_LEAVES` height (spawn point of entities).
    fn heightmap_no_leaves(&self, x: i32, z: i32) -> i32 {
        // Same rule as the game's map (tag no leaves or fluid), live map first.
        self.hm_no_leaves(x, z)
    }

    // --- Player. ---

    /// Portals touched by a box (box shrunk by 1e-5, like the check of blocks passed through).
    pub(crate) fn portals_touched(&self, b: &Aabb) -> Vec<Touch> {
        let e = 1.0e-5;
        let d =
            Aabb { min: [b.min[0] + e, b.min[1] + e, b.min[2] + e], max: [b.max[0] - e, b.max[1] - e, b.max[2] - e] };
        let lo = floor3(d.min);
        let hi = floor3(d.max);
        let (np, ep, gw) =
            (self.block_id_of("nether_portal"), self.block_id_of("end_portal"), self.block_id_of("end_gateway"));
        let mut out = Vec::new();
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let p = [x, y, z];
                    let b = self.v().block(self.get(p));
                    let kind = if b == np {
                        PortalKind::Nether
                    } else if b == ep {
                        let shape = Aabb {
                            min: [f64::from(x), f64::from(y) + 0.375, f64::from(z)],
                            max: [f64::from(x) + 1.0, f64::from(y) + 0.75, f64::from(z) + 1.0],
                        };
                        if !hits(&d, &shape) {
                            continue;
                        }
                        PortalKind::End
                    } else if b == gw {
                        PortalKind::Gateway
                    } else {
                        continue;
                    };
                    out.push(Touch { kind, pos: p });
                }
            }
        }
        out
    }

    fn player_delay(&self, kind: PortalKind) -> i32 {
        if kind != PortalKind::Nether {
            return 0;
        }
        let rule = if self.abilities().invulnerable {
            "minecraft:players_nether_portal_creative_delay"
        } else {
            "minecraft:players_nether_portal_default_delay"
        };
        let default = if self.abilities().invulnerable { 0 } else { 80 };
        self.rules.get(rule).and_then(Nbt::as_i64).map_or(default, |v| v.max(0) as i32)
    }

    pub(crate) fn player_portal_tick(&mut self) {
        if self.dims.won_game {
            return;
        }
        let alive = !self.life.vit.dead;
        // Spectator: no block effects (no portal).
        if alive && self.game_mode != 3 {
            let h = if self.sneaking { 1.5 } else { 1.8 };
            let b = ent_box(self.pos, 0.6, h);
            for t in self.portals_touched(&b) {
                match t.kind {
                    PortalKind::End if self.dim.name == END && !self.dims.seen_credits => {
                        self.show_end_credits();
                        return;
                    }
                    PortalKind::Gateway => {
                        if !self.gateway_cooling(t.pos) {
                            self.dims.player.set_inside(t.kind, t.pos, PLAYER_COOLDOWN);
                            self.gateway_trigger(t.pos);
                        }
                    }
                    k => self.dims.player.set_inside(k, t.pos, PLAYER_COOLDOWN),
                }
            }
        }
        // Portal handling.
        let mut ep = self.dims.player;
        if ep.cooldown > 0 {
            ep.cooldown -= 1;
        }
        let mut go = None;
        if let Some(mut pr) = ep.proc {
            let delay = self.player_delay(pr.kind);
            // F04: a passenger player does not change dimension.
            let can_use = alive && !self.player_riding_any();
            if pr.process(can_use, delay) {
                ep.cooldown = PLAYER_COOLDOWN;
                go = Some(pr);
                ep.proc = Some(pr);
            } else {
                ep.proc = if pr.expired() { None } else { Some(pr) };
            }
        }
        self.dims.player = ep;
        let Some(pr) = go else { return };
        let h = if self.sneaking { 1.5 } else { 1.8 };
        let t = match pr.kind {
            PortalKind::Nether => self.nether_destination(pr.entry, self.pos, 0.6, h, self.game_mode == 3),
            PortalKind::End => self.end_destination(true),
            PortalKind::Gateway => self.gateway_destination(pr.entry, false),
        };
        let Some(t) = t else { return };
        if t.dim == NETHER
            && self.dim.name != NETHER
            && !self.rule("minecraft:allow_entering_nether_using_portals", true)
        {
            return;
        }
        let ticket = t.ticket.then(|| t.dim.clone());
        self.teleport_player(t);
        if let Some(d) = ticket {
            self.place_portal_ticket(&d);
        }
    }

    /// Spectator who clicks a portal (Nether, End, gateway): immediate passage to its
    /// destination, otherwise red message "impossible to teleport". True if it was a portal.
    pub(crate) fn spectator_portal(&mut self, p: P) -> bool {
        let s = self.get(p);
        let kind = if self.is_block(s, "minecraft:nether_portal") {
            PortalKind::Nether
        } else if self.is_block(s, "minecraft:end_portal") {
            PortalKind::End
        } else if self.is_block(s, "minecraft:end_gateway") {
            PortalKind::Gateway
        } else {
            return false;
        };
        let t = match kind {
            PortalKind::Nether => self.nether_destination(p, self.pos, 0.6, 1.8, true),
            PortalKind::End => self.end_destination(true),
            PortalKind::Gateway => self.gateway_destination(p, false),
        };
        match t {
            Some(t) => {
                let ticket = t.ticket.then(|| t.dim.clone());
                self.teleport_player(t);
                if let Some(d) = ticket {
                    self.place_portal_ticket(&d);
                }
            }
            None => {
                let mut msg = rc_text::Component::translate("spectator.cannot_teleport", Vec::new());
                msg.style.color = Some(crate::commands::text::RED);
                let _ = self.send(rc_client::Event::ActionBar(msg));
            }
        }
        true
    }

    /// Entering the End exit portal without having seen the credits.
    fn show_end_credits(&mut self) {
        if !self.dims.won_game {
            self.dims.won_game = true;
            let _ = self.send(Event::GameEvent { kind: 4, param: 0.0 });
            self.dims.seen_credits = true;
        }
    }

    /// End of the credits (`client_command` 0 after the end screen): respawn, data kept.
    pub(crate) fn respawn_after_credits(&mut self) {
        self.dims.won_game = false;
        self.life.vit.crit = Default::default(); // new player: criteria rewritten
        let t = self.respawn_transition(false);
        self.change_dimension(t);
    }

    // --- End gateways. ---

    fn gateway_key(&self, p: P) -> (String, P) {
        (self.dim.name.clone(), p)
    }

    fn gateway_cooling(&self, p: P) -> bool {
        self.dims.gateways.get(&self.gateway_key(p)).is_some_and(|&c| c > 0)
    }

    /// 40 ticks, block event 1 (the client's beam).
    fn gateway_trigger(&mut self, p: P) {
        let k = self.gateway_key(p);
        self.dims.gateways.insert(k, 40);
        let b = self.block_id_of("end_gateway");
        self.block_event(p, b, 1, 0);
    }

    /// Ticker of the gateway at `p` (block entity created or re-read, G5-F10).
    pub(crate) fn gateway_ticker_register(&mut self, p: P) {
        if matches!(self.bs.be_ticks.get(&p), Some((_, crate::blocks::level::BeTick::Gateway))) {
            return;
        }
        let id = self.add_ticker(p);
        self.bs.be_ticks.insert(p, (id, crate::blocks::level::BeTick::Gateway));
    }

    /// Gateway tick: age + 1 (kept in `Age`); out of cooldown, every 2400 ticks
    /// of age, the purple beam (cooldown of 40 ticks, block event 1). The chunk is only
    /// marked to be saved when the gateway finishes appearing (age 200) or changes
    /// cooldown state, as in the game (the current age goes out with any other save); the end
    /// of the cooldown is marked by the cooldown countdown (`gateways_tick`).
    pub(crate) fn gateway_be_tick(&mut self, p: P) {
        let Some(age) = self.be_get(p).map(|b| match b.get("Age") {
            Some(Nbt::Long(a)) => *a,
            Some(n) => n.as_i64().unwrap_or(0),
            None => 0,
        }) else {
            return;
        };
        let (spawning, cooling) = (age < 200, self.gateway_cooling(p));
        let age = age + 1;
        if !cooling && age % 2400 == 0 {
            self.gateway_trigger(p);
        }
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "Age", Nbt::Long(age));
        }
        if (spawning != (age < 200) || cooling != self.gateway_cooling(p))
            && let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))
        {
            c.dirty = true;
        }
    }

    /// Cooldown countdown; the end of a cooldown marks the gateway's chunk to be
    /// saved (cooldown state change, which the game marks in the gateway's tick).
    pub(crate) fn gateways_tick(&mut self) {
        // G6-F12: the cooldown is counted down by the block entity tick, which does not run outside
        // the border nor outside tick range (empty, hence without allocation, usually).
        let frozen: Vec<P> = self
            .dims
            .gateways
            .keys()
            .filter(|(d, p)| *d == self.dim.name && !self.be_ticking(*p))
            .map(|(_, p)| *p)
            .collect();
        let (chunks, dim) = (&mut self.chunks, &self.dim.name);
        self.dims.gateways.retain(|(d, p), c| {
            if d == dim && frozen.contains(p) {
                return true;
            }
            *c -= 1;
            if *c > 0 {
                return true;
            }
            if d == dim
                && let Some(Some(ch)) = chunks.get_mut(&(p[0] >> 4, p[2] >> 4))
            {
                ch.dirty = true;
            }
            false
        });
    }

    fn be_exit(&self, p: P) -> (Option<P>, bool) {
        let Some(be) = self.be_get(p) else { return (None, false) };
        let exit = match be.get("exit_portal") {
            Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
            _ => None,
        };
        let exact = crate::nbtio::int(be, "ExactTeleport").is_some_and(|v| v != 0);
        (exit, exact)
    }

    fn be_set_exit(&mut self, p: P, exit: P, exact: bool) {
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "exit_portal", Nbt::IntArray(vec![exit[0], exit[1], exit[2]]));
            if exact {
                crate::nbtio::set(be, "ExactTeleport", Nbt::Byte(1));
            }
        }
        if let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4)) {
            c.dirty = true;
        }
    }

    fn full_block(&self, p: P) -> bool {
        self.v().full_collision(self.get(p))
    }

    /// The search for the highest block.
    fn tallest_block(&self, around: P, dist: i32, bedrock: bool) -> P {
        let bed = self.block_id_of("bedrock");
        let max_y = self.top_y() - 1;
        let mut tallest: Option<P> = None;
        for xd in -dist..=dist {
            for zd in -dist..=dist {
                if xd == 0 && zd == 0 && !bedrock {
                    continue;
                }
                let floor = tallest.map_or(self.dim.height.min_y, |t| t[1]);
                let mut y = max_y;
                while y > floor {
                    let p = [around[0] + xd, y, around[2] + zd];
                    let s = self.get(p);
                    if self.v().full_collision(s) && (bedrock || self.v().block(s) != bed) {
                        tallest = Some(p);
                        break;
                    }
                    y -= 1;
                }
            }
        }
        tallest.unwrap_or(around)
    }

    fn chunk_empty(&mut self, x: f64, z: f64) -> bool {
        let c = ((x / 16.0).floor() as i32, (z / 16.0).floor() as i32);
        self.load_now(&[c]);
        match self.chunks.get(&c) {
            Some(Some(ch)) => ch.sections.iter().all(|s| s.is_empty()),
            _ => true,
        }
    }

    /// The search or creation of a valid teleport position (gateway without known exit).
    fn gateway_find_or_create(&mut self, entry: P) -> P {
        let len = (f64::from(entry[0]).powi(2) + f64::from(entry[2]).powi(2)).sqrt();
        let dir = if len < 1.0e-5 { [0.0, 0.0] } else { [f64::from(entry[0]) / len, f64::from(entry[2]) / len] };
        let mut t = [dir[0] * 1024.0, dir[1] * 1024.0];
        let mut limit = 16;
        while !self.chunk_empty(t[0], t[1]) && {
            let go = limit > 0;
            limit -= 1;
            go
        } {
            t = [t[0] - dir[0] * 16.0, t[1] - dir[1] * 16.0];
        }
        let mut limit = 16;
        while self.chunk_empty(t[0], t[1]) && {
            let go = limit > 0;
            limit -= 1;
            go
        } {
            t = [t[0] + dir[0] * 16.0, t[1] + dir[1] * 16.0];
        }
        let c = ((t[0] / 16.0).floor() as i32, (t[1] / 16.0).floor() as i32);
        let spot = self.valid_spawn_in_chunk(c);
        let p = match spot {
            Some(p) => p,
            None => {
                let p = [(t[0] + 0.5).floor() as i32, 75, (t[1] + 0.5).floor() as i32];
                self.place_end_island(p);
                p
            }
        };
        self.tallest_block(p, 16, true)
    }

    /// Free End stone above, nearest to the origin.
    fn valid_spawn_in_chunk(&self, (cx, cz): (i32, i32)) -> Option<P> {
        let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { return None };
        let top_section = c.sections.iter().rposition(|s| !s.is_empty())?;
        let max_y = self.dim.height.min_y + top_section as i32 * 16 + 16 - 1;
        let stone = self.block_id_of("end_stone");
        let mut best: Option<(P, f64)> = None;
        for p in ps::between_closed([cx * 16, 30, cz * 16], [cx * 16 + 15, max_y, cz * 16 + 15]) {
            if self.v().block(self.get(p)) != stone {
                continue;
            }
            if self.full_block([p[0], p[1] + 1, p[2]]) || self.full_block([p[0], p[1] + 2, p[2]]) {
                continue;
            }
            let d = (0..3).map(|i| (f64::from(p[i]) + 0.5).powi(2)).sum::<f64>();
            if best.is_none_or(|b| d < b.1) {
                best = Some((p, d));
            }
        }
        best.map(|b| b.0)
    }

    /// End island, random draw created from the position packed into a 64-bit integer.
    fn place_end_island(&mut self, o: P) {
        let packed =
            ((i64::from(o[0]) & 0x3FF_FFFF) << 38) | ((i64::from(o[2]) & 0x3FF_FFFF) << 12) | (i64::from(o[1]) & 0xFFF);
        let mut rng = crate::mobs::jrand::JRand::new(packed);
        let stone = self.v().default_state(self.block_id_of("end_stone"));
        let mut size = rng.next_int_bound(3) as f32 + 4.0;
        let mut y = 0;
        while size > 0.5 {
            let (lo, hi) = ((-size).floor() as i32, size.ceil() as i32);
            for x in lo..=hi {
                for z in lo..=hi {
                    if ((x * x + z * z) as f32) <= (size + 1.0) * (size + 1.0) {
                        self.set_block([o[0] + x, o[1] + y, o[2] + z], stone, flag::ALL);
                    }
                }
            }
            size -= rng.next_int_bound(2) as f32 + 0.5;
            y -= 1;
        }
    }

    /// Gateway at `o`, known exit `exit` (not exact).
    fn place_gateway(&mut self, o: P, exit: P) {
        let gw = self.v().default_state(self.block_id_of("end_gateway"));
        let bedrock = self.v().default_state(self.block_id_of("bedrock"));
        let air = self.air();
        for p in ps::between_closed([o[0] - 1, o[1] - 2, o[2] - 1], [o[0] + 1, o[1] + 2, o[2] + 1]) {
            let (sx, sy, sz) = (p[0] == o[0], p[1] == o[1], p[2] == o[2]);
            let end = (p[1] - o[1]).abs() == 2;
            let s = if sx && sy && sz {
                gw
            } else if sy {
                air
            } else if (end && sx && sz) || ((sx || sz) && !end) {
                bedrock
            } else {
                air
            };
            self.set_block(p, s, flag::ALL);
            if s == gw {
                self.be_set_exit(p, exit, false);
            }
        }
    }

    /// Destination through a gateway.
    pub(crate) fn gateway_destination(&mut self, entry: P, pearl: bool) -> Option<Transition> {
        let (mut exit, exact) = self.be_exit(entry);
        if exit.is_none() && self.dim.name == END {
            let mut p = self.gateway_find_or_create(entry);
            p[1] += 10;
            self.load_now(&Self::chunks_around(p, 2));
            self.place_gateway(p, entry);
            self.be_set_exit(entry, p, exact);
            exit = Some(p);
        }
        let exit = exit?;
        let p = if exact {
            exit
        } else {
            let t = self.tallest_block([exit[0], exit[1] + 2, exit[2]], 5, false);
            [t[0], t[1] + 1, t[2]]
        };
        Some(Transition {
            dim: self.dim.name.clone(),
            pos: [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5],
            yaw: 0.0,
            pitch: 0.0,
            relative: if pearl { 0 } else { rel::DELTA | rel::ROTATION },
            sound: false,
            ticket: true,
            missing_respawn: false,
        })
    }

    // --- End portal frame. ---

    /// Eye of Ender on a frame; true if the eye is placed.
    pub(crate) fn use_ender_eye(&mut self, clicked: P) -> bool {
        let s = self.get(clicked);
        if !self.is_block(s, "end_portal_frame") || self.v().get_bool(s, "eye") {
            return false;
        }
        let ns = self.v().set_bool(s, "eye", true);
        // The eye adds a box 8/16 on a side, from 13/16 to 1: entities that touch it are lifted.
        self.push_entities_up_box(clicked, [0.25, 13.0 / 16.0, 0.25], [0.75, 1.0, 0.75]);
        self.set_block(clicked, ns, flag::CLIENTS);
        let frame_block = self.v().block(ns);
        self.update_neighbour_for_output_signal(clicked, frame_block);
        let slot = 36 + self.held as usize;
        if self.game_mode != 1
            && let Some(st) = &mut self.slots[slot]
        {
            st.count -= 1;
            if st.count <= 0 {
                self.slots[slot] = None;
            }
        }
        self.broadcast_menus();
        self.level_event(1503, clicked, 0);
        let frame = self.block_id_of("end_portal_frame");
        let found = {
            let eye = |p: P| {
                let st = self.get(p);
                (self.v().block(st) == frame && self.v().get_bool(st, "eye")).then(|| self.v().facing(st))
            };
            ps::find_end_frame(clicked, &eye)
        };
        if let Some(ftl) = found {
            let base = [ftl[0] - 3, ftl[1], ftl[2] - 3];
            let portal = self.v().default_state(self.block_id_of("end_portal"));
            for x in 0..3 {
                for z in 0..3 {
                    let p = [base[0] + x, base[1], base[2] + z];
                    self.destroy_block(p, true, 512);
                    self.set_block(p, portal, flag::CLIENTS);
                }
            }
            self.global_level_event(1038, [base[0] + 1, base[1], base[2] + 1], 0);
        }
        true
    }

    // --- Entities. ---

    /// Portals touched by an entity `w`×`h` going from `from` to `to` during this tick
    /// (the check of blocks passed through): cells swept along the path around the arrival box
    /// shrunk by 1e-5; the shape of the End portal must be touched by the sliding box.
    /// Each cell once, in sweep order. With no portal block in the box that
    /// encloses the path (common case), nothing is swept.
    pub(crate) fn portals_touched_swept(&self, from: [f64; 3], to: [f64; 3], w: f64, h: f64) -> Vec<Touch> {
        let (np, ep, gw) =
            (self.block_id_of("nether_portal"), self.block_id_of("end_portal"), self.block_id_of("end_gateway"));
        let e = f64::from(1.0e-5f32);
        let at = ent_box(to, w, h);
        let start = ent_box(from, w, h);
        let lo = floor3(std::array::from_fn(|i| at.min[i].min(start.min[i])));
        let hi = floor3(std::array::from_fn(|i| at.max[i].max(start.max[i])));
        if !self.portal_in_sections(lo, hi, [np, ep, gw]) {
            return Vec::new();
        }
        let d = Aabb { min: at.min.map(|v| v + e), max: at.max.map(|v| v - e) };
        let travel = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let mut out: Vec<Touch> = Vec::new();
        crate::mobs::nether::ghast::each_cell_between(from, to, &d, &mut |p| {
            let b = self.v().block(self.get(p));
            let kind = if b == np {
                PortalKind::Nether
            } else if b == gw {
                PortalKind::Gateway
            } else if b == ep {
                let shape = Aabb {
                    min: [f64::from(p[0]), f64::from(p[1]) + 0.375, f64::from(p[2])],
                    max: [f64::from(p[0]) + 1.0, f64::from(p[1]) + 0.75, f64::from(p[2]) + 1.0],
                };
                if !crate::mobs::nether::ghast::collided_along(&start, travel, &[shape]) {
                    return true;
                }
                PortalKind::End
            } else {
                return true;
            };
            if !out.iter().any(|t| t.pos == p) {
                out.push(Touch { kind, pos: p });
            }
            true
        });
        out
    }

    /// Can one of the sections covering cells `lo..=hi` contain a portal block?
    /// Read from the section's palette (single value or list of states), without reading its blocks;
    /// section without palette (direct identifiers): maybe. Absent chunk or section: no.
    fn portal_in_sections(&self, lo: P, hi: P, blocks: [u16; 3]) -> bool {
        let is_portal = |s: u32| blocks.contains(&self.v().block(s));
        let min_y = self.dim.height.min_y;
        let (sy0, sy1) = ((lo[1] - min_y).max(0) >> 4, (hi[1] - min_y) >> 4);
        for cx in lo[0] >> 4..=hi[0] >> 4 {
            for cz in lo[2] >> 4..=hi[2] >> 4 {
                let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { continue };
                for sy in sy0..=sy1 {
                    let Some(sec) = usize::try_from(sy).ok().and_then(|i| c.sections.get(i)) else { continue };
                    let maybe = match &sec.blocks {
                        rc_world::Container::Single(v) => is_portal(*v),
                        rc_world::Container::Packed { palette: Some(p), .. } => p.iter().any(|&v| is_portal(v)),
                        rc_world::Container::Packed { palette: None, .. } => true,
                    };
                    if maybe {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The "in a portal" mark for each portal touched (gateway: outside cooldown, which it
    /// triggers).
    pub(crate) fn entity_portal_mark(&mut self, id: i32, touched: &[Touch], cooldown: i32) {
        if touched.is_empty() {
            return;
        }
        let mut ep = self.dims.ents.get(&id).copied().unwrap_or_default();
        for t in touched {
            match t.kind {
                PortalKind::Gateway => {
                    if !self.gateway_cooling(t.pos) {
                        ep.set_inside(t.kind, t.pos, cooldown);
                        self.gateway_trigger(t.pos);
                    }
                }
                k => ep.set_inside(k, t.pos, cooldown),
            }
        }
        self.dims.ents.insert(id, ep);
    }

    /// The portal handling of an entity other than the player (delay 0): return delay counted down,
    /// process advanced; returns the passage due (return delay reset to `cooldown`: 300, vehicle 10).
    pub(crate) fn entity_portal_process(&mut self, id: i32, cooldown: i32) -> Option<Proc> {
        let mut ep = *self.dims.ents.get(&id)?;
        if ep.cooldown > 0 {
            ep.cooldown -= 1;
        }
        let mut go = None;
        if let Some(mut pr) = ep.proc {
            if pr.process(true, 0) {
                ep.cooldown = cooldown;
                go = Some(pr);
                ep.proc = Some(pr);
            } else {
                ep.proc = if pr.expired() { None } else { Some(pr) };
            }
        }
        if ep.cooldown == 0 && ep.proc.is_none() {
            self.dims.ents.remove(&id);
        } else {
            self.dims.ents.insert(id, ep);
        }
        go
    }

    /// Minecarts in a portal (G5-F06, return delay 10 like any vehicle): passage, the
    /// passenger player changes dimension with it (seat offset kept) and stays seated.
    /// A minecart carrying a mob does not pass (same difference as loaded mounts).
    pub(crate) fn carts_portal_tick(&mut self) {
        use crate::minecart::{HEIGHT, WIDTH};
        use crate::mobs::mob::Ent;
        let mut i = 0;
        while i < self.carts.list.len() {
            let Some(c) = self.carts.list[i].as_ref() else {
                i += 1;
                continue;
            };
            if c.removed || c.passengers.iter().any(|p| matches!(p, Ent::Mob(_))) {
                i += 1;
                continue;
            }
            let (id, pos) = (c.id, c.pos);
            let touched = self.portals_touched(&ent_box(pos, WIDTH, HEIGHT));
            if touched.is_empty() && !self.dims.ents.contains_key(&id) {
                i += 1;
                continue;
            }
            let Some(pr) = self.entity_portal_step(id, &touched, CART_COOLDOWN) else {
                i += 1;
                continue;
            };
            let Some(t) = self.entity_destination(pr, pos, WIDTH, HEIGHT, false) else {
                i += 1;
                continue;
            };
            let Some(mut c) = self.carts.list[i].take() else {
                i += 1;
                continue;
            };
            if std::mem::take(&mut c.track.seen) {
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![c.id])));
            }
            let (vel, yaw, pitch) = transition_motion(&t, c.vel, c.yaw, c.pitch);
            c.pos = t.pos;
            c.old_pos = t.pos;
            c.vel = vel;
            c.yaw = yaw;
            c.yaw_o = yaw;
            c.pitch = pitch;
            let same = t.dim == self.dim.name;
            if c.passengers.contains(&Ent::Player) {
                // Same offset, rotation of the passage.
                let mut pt = t.clone();
                pt.pos = std::array::from_fn(|k| t.pos[k] + (self.pos[k] - pos[k]));
                if same {
                    self.carts.list[i] = Some(c);
                    self.teleport_cart_passenger(pt);
                    i += 1;
                    continue;
                }
                // The active world becomes the destination: the minecart is put back there afterwards.
                self.carts.list.remove(i);
                self.teleport_cart_passenger(pt);
                self.carts.list.push(Some(c));
                return;
            }
            if same {
                self.carts.list[i] = Some(c);
                i += 1;
            } else {
                self.carts.list.remove(i);
                let dim = t.dim.clone();
                self.in_world(&dim, move |s| s.carts.list.push(Some(c)));
                if t.ticket {
                    self.place_portal_ticket(&dim);
                }
            }
        }
    }

    /// Passenger player of a minecart going through a portal: moved with it, they stay seated
    /// (a vehicle's passage takes its passengers along, without the vehicle dismount).
    fn teleport_cart_passenger(&mut self, t: Transition) {
        let seat = self.pcart.vehicle.take();
        self.teleport_player(t);
        self.pcart.vehicle = seat;
    }

    /// Marks then handles in the same tick (items, mobs, falling block).
    fn entity_portal_step(&mut self, id: i32, touched: &[Touch], cooldown: i32) -> Option<Proc> {
        self.entity_portal_mark(id, touched, cooldown);
        self.entity_portal_process(id, cooldown)
    }

    /// The entity's process was marked during this tick.
    pub(crate) fn entity_inside_portal_now(&self, id: i32) -> bool {
        self.dims.ents.get(&id).and_then(|e| e.proc).is_some_and(|p| p.inside)
    }

    /// Return delay of an entity in progress.
    pub(crate) fn entity_portal_cooling(&self, id: i32) -> bool {
        self.dims.ents.get(&id).is_some_and(|e| e.cooldown > 0)
    }

    /// Destination of an entity; `pearl`: Ender pearl (gateway without velocity kept).
    pub(crate) fn entity_destination(
        &mut self,
        pr: Proc,
        pos: [f64; 3],
        w: f64,
        h: f64,
        pearl: bool,
    ) -> Option<Transition> {
        let t = match pr.kind {
            PortalKind::Nether => self.nether_destination(pr.entry, pos, w, h, false),
            PortalKind::End => self.end_destination(false),
            PortalKind::Gateway => self.gateway_destination(pr.entry, pearl),
        }?;
        if t.dim == NETHER
            && self.dim.name != NETHER
            && !self.rule("minecraft:allow_entering_nether_using_portals", true)
        {
            return None;
        }
        Some(t)
    }

    /// Ground items in a portal (after their tick): passage, new dimension.
    pub(crate) fn drops_portal_tick(&mut self) {
        let mut i = 0;
        while i < self.life.drops.len() {
            let (id, pos) = (self.life.drops[i].id, self.life.drops[i].pos);
            let touched = self.portals_touched(&ent_box(pos, 0.25, 0.25));
            if touched.is_empty() && !self.dims.ents.contains_key(&id) {
                i += 1;
                continue;
            }
            let Some(pr) = self.entity_portal_step(id, &touched, ENTITY_COOLDOWN) else {
                i += 1;
                continue;
            };
            let Some(t) = self.entity_destination(pr, pos, 0.25, 0.25, false) else {
                i += 1;
                continue;
            };
            let mut d = self.life.drops.remove(i);
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![d.id])));
            d.pos = t.pos;
            // Item yaw (drawn at random at its creation): the velocity rotates by the difference.
            (d.vel, d.yaw, _) = transition_motion(&t, d.vel, d.yaw, 0.0);
            d.retrack();
            if t.dim == self.dim.name {
                self.reannounce_item(&d);
                self.life.drops.insert(i, d);
                i += 1;
            } else {
                let dim = t.dim.clone();
                self.in_world(&dim, move |s| s.life.drops.push(d));
                self.place_portal_ticket(&dim);
            }
        }
    }

    /// Mobs in a portal: passage (same rules as items).
    pub(crate) fn mobs_portal_tick(&mut self) {
        let mut i = 0;
        while i < self.mobs.list.len() {
            let Some(m) = self.mobs.list[i].as_ref() else {
                i += 1;
                continue;
            };
            // F04: a passenger does not pass alone; a loaded vehicle neither
            // here (passage of passengers with their mount is not simulated).
            if m.removed || m.dead || m.mount.vehicle.is_some() || !m.mount.passengers.is_empty() {
                i += 1;
                continue;
            }
            // The portal-use test false: wither, dragon, creaking tied to its heart, sleeping villager
            // (never marked in a portal).
            if !self.mob_can_use_portal(m) {
                i += 1;
                continue;
            }
            let (id, pos) = (m.id, m.pos);
            // Real box (babies, cube sizes, postures).
            let (w, h) = (f64::from(m.width()), f64::from(m.height()));
            let touched = self.portals_touched(&ent_box(pos, w, h));
            if touched.is_empty() && !self.dims.ents.contains_key(&id) {
                i += 1;
                continue;
            }
            let Some(pr) = self.entity_portal_step(id, &touched, ENTITY_COOLDOWN) else {
                i += 1;
                continue;
            };
            let Some(t) = self.entity_destination(pr, pos, w, h, false) else {
                i += 1;
                continue;
            };
            let Some(mut m) = self.mobs.list[i].take() else { continue };
            self.unpair(&mut m);
            m.pos = t.pos;
            m.old = t.pos;
            let (vel, yaw, pitch) = transition_motion(&t, m.vel, m.yaw, m.pitch);
            if t.relative & rel::Y_ROT != 0 {
                m.body_yaw += t.yaw;
            }
            (m.vel, m.yaw, m.head_yaw, m.pitch) = (vel, yaw, yaw, pitch);
            if t.dim == self.dim.name {
                self.mobs.list[i] = Some(m);
            } else {
                self.mobs.list.remove(i);
                self.mobs.index.mark_stale(); // shifted slots
                let dim = t.dim.clone();
                self.in_world(&dim, move |s| s.mobs.push(m));
                self.place_portal_ticket(&dim);
                continue;
            }
            i += 1;
        }
    }

    /// Falling block (after its movement, portal handling): `None` if it did not change
    /// dimension, otherwise `Some(duplicate)`: to or from the End, the old one keeps its last
    /// tick (duplication of game 26.3).
    pub(crate) fn falling_portal(&mut self, e: &mut crate::blocks::entities::SimEntity) -> Option<bool> {
        self.sim_entity_portal_mark(e);
        if !self.dims.ents.contains_key(&e.id) {
            return None;
        }
        let pr = self.entity_portal_process(e.id, ENTITY_COOLDOWN)?;
        self.sim_entity_portal_go(e, pr)
    }

    /// Primed TNT, at the start of the tick (before movement): true if it
    /// went to another dimension (the old one finishes its tick then disappears).
    pub(crate) fn tnt_portal(&mut self, e: &mut crate::blocks::entities::SimEntity) -> bool {
        if !self.dims.ents.contains_key(&e.id) {
            return false;
        }
        let Some(pr) = self.entity_portal_process(e.id, ENTITY_COOLDOWN) else { return false };
        self.sim_entity_portal_go(e, pr).is_some()
    }

    /// Portals touched by the movement of this tick of a falling block or a TNT
    /// (application of block effects).
    pub(crate) fn sim_entity_portal_mark(&mut self, e: &crate::blocks::entities::SimEntity) {
        let touched = self.portals_touched_swept(e.old, e.pos, 0.98, 0.98);
        self.entity_portal_mark(e.id, &touched, ENTITY_COOLDOWN);
    }

    /// Passage of a falling block or a TNT: same dimension (gateway), moved in place
    /// (teleport packet) and `None`; otherwise copy into the arrival dimension (new
    /// identifier, delay and process kept), the old one removed, and `Some(duplicate)`: to or
    /// from the End for the falling block. The passed TNT does not blow up the Nether portal.
    fn sim_entity_portal_go(&mut self, e: &mut crate::blocks::entities::SimEntity, pr: Proc) -> Option<bool> {
        use crate::blocks::entities::EntKind;
        let t = self.entity_destination(pr, e.pos, 0.98, 0.98, false)?;
        let from = self.dim.name.clone();
        let tnt = matches!(e.kind, EntKind::Tnt(_));
        // Falling block and TNT: zero yaw.
        let vel = transition_motion(&t, e.vel, 0.0, 0.0).0;
        if t.dim == from {
            e.pos = t.pos;
            e.old = t.pos;
            e.vel = vel;
            e.used_portal |= tnt;
            let _ = self.send(Event::Entity(EntityEvent::Teleport(rc_proto_v777::entity::TeleportEntity {
                id: e.id,
                pos: e.pos,
                velocity: e.vel,
                yaw: 0.0,
                pitch: 0.0,
                relative: 0,
                on_ground: e.on_ground,
            })));
            return None;
        }
        let dupe = !tnt && (from == END || t.dim == END);
        let mut copy = e.clone();
        copy.pos = t.pos;
        copy.old = t.pos;
        copy.vel = vel;
        copy.used_portal |= tnt;
        copy.track = Default::default();
        e.removed = true;
        let cooldown = self.dims.ents.remove(&e.id).unwrap_or_default();
        let dim = t.dim.clone();
        // Arrival in the player's dimension from a parked dimension: announced.
        let to_player = self.dims.home.as_deref() == Some(dim.as_str());
        self.in_world(&dim, |s| {
            copy.id = s.alloc_entity();
            s.dims.ents.insert(copy.id, EntPortal { cooldown: cooldown.cooldown, proc: cooldown.proc });
            if to_player {
                let muted = std::mem::replace(&mut s.muted, false);
                match copy.kind {
                    EntKind::Falling(st) => s.add_entity_packet(&copy, "minecraft:falling_block", st as i32),
                    EntKind::Tnt(fuse) => {
                        s.add_entity_packet(&copy, "minecraft:tnt", 0);
                        let _ = s.send(Event::Entity(EntityEvent::Data {
                            id: copy.id,
                            values: vec![(8, rc_proto_v777::entity::Meta::Int(fuse))],
                        }));
                    }
                }
                s.muted = muted;
            }
            s.bs.entities.list.push(copy);
        });
        self.place_portal_ticket(&dim);
        Some(dupe)
    }

    /// The portal-use test of a non-passenger mob: false for the wither, the dragon, the
    /// creaking tied to its heart and a sleeping mob.
    pub(crate) fn mob_can_use_portal(&self, m: &crate::mobs::mob::Mob) -> bool {
        use crate::mobs::species::Kind;
        !matches!(m.kind, Kind::Wither | Kind::EnderDragon)
            && !(m.kind == Kind::Creaking && m.rec.home.is_some())
            && self.npc_sleeping(m).is_none()
    }
}

/// The computation of the absolute position of a teleport for the velocity and rotation of an entity passing
/// a portal (transition position always absolute): rotation added or set, velocity
/// kept and rotated by the rotation difference (tabulated sine/cosine), or reset to zero.
pub(crate) fn transition_motion(t: &Transition, vel: [f64; 3], yaw: f32, pitch: f32) -> ([f64; 3], f32, f32) {
    const ROTATE_DELTA: i32 = 1 << 8;
    let ayaw = if t.relative & rel::Y_ROT != 0 { yaw } else { 0.0 } + t.yaw;
    let apitch = (if t.relative & rel::X_ROT != 0 { pitch } else { 0.0 } + t.pitch).clamp(-90.0, 90.0);
    let mut v = vel;
    if t.relative & ROTATE_DELTA != 0 {
        let rad = |d: f32| (f64::from(d) * 0.017_453_292_519_943_295) as f32;
        let (cx, sx) = (
            crate::commands::args::mcos(f64::from(rad(pitch - apitch))),
            crate::commands::args::msin(f64::from(rad(pitch - apitch))),
        );
        v = [v[0], v[1] * cx + v[2] * sx, v[2] * cx - v[1] * sx];
        let (cy, sy) = (
            crate::commands::args::mcos(f64::from(rad(yaw - ayaw))),
            crate::commands::args::msin(f64::from(rad(yaw - ayaw))),
        );
        v = [v[0] * cy + v[2] * sy, v[1], v[2] * cy - v[0] * sy];
    }
    let keep = |bit: i32, c: f64| if t.relative & (1 << bit) != 0 { c } else { 0.0 };
    ([keep(5, v[0]), keep(6, v[1]), keep(7, v[2])], ayaw, apitch)
}
