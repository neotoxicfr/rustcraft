//! Cushion: placed on top of a block (anchored below it), removed when the anchor disappears,
//! destroyed by fire and lava, broken by the player.

use rc_physics::Aabb;
use rc_proto::nbt::Nbt;

use super::{DKind, DYES, Use, base_fields, flag, named};
use crate::blocks::{Dir, P};
use crate::mobs::mob::Ent;
use crate::nbtio;
use crate::player::Stack;
use crate::server::Server;

/// Height of the anchor slice (1/64 of a block).
const SLICE: f64 = 0.015625;

/// Colour (dye index) of a cushion, white by default.
pub(super) fn color(e: &Nbt) -> u8 {
    e.get("color").and_then(Nbt::as_str).and_then(|c| DYES.iter().position(|d| *d == c)).unwrap_or(0) as u8
}

/// Seat of the sitting player: attachment point of the passenger (top of the cushion) minus that of the
/// player as a passenger (0.6).
fn cushion_seat(p: [f64; 3]) -> [f64; 3] {
    [p[0], p[1] + 0.25 - 0.6, p[2]]
}

/// Box of a cushion placed at `p` (1 × 0.25).
pub(super) fn cushion_box(p: [f64; 3]) -> Aabb {
    Aabb { min: [p[0] - 0.5, p[1], p[2] - 0.5], max: [p[0] + 0.5, p[1] + 0.25, p[2] + 0.5] }
}

fn next_down(x: f64) -> f64 {
    if x.is_nan() || x == f64::NEG_INFINITY {
        x
    } else if x == 0.0 {
        -f64::from_bits(1)
    } else if x > 0.0 {
        f64::from_bits(x.to_bits() - 1)
    } else {
        f64::from_bits(x.to_bits() + 1)
    }
}

fn next_up(x: f64) -> f64 {
    -next_down(-x)
}

/// The next shrunk box.
fn next_deflated(b: &Aabb) -> Aabb {
    Aabb { min: b.min.map(next_up), max: b.max.map(next_down) }
}

/// Cells of a box (floors of the bounds, inclusive).
fn cells(b: &Aabb) -> impl Iterator<Item = P> {
    let lo = b.min.map(|v| v.floor() as i32);
    let hi = b.max.map(|v| v.floor() as i32);
    (lo[1]..=hi[1]).flat_map(move |y| (lo[2]..=hi[2]).flat_map(move |z| (lo[0]..=hi[0]).map(move |x| [x, y, z])))
}

/// `b` entirely covered by the union of `boxes` (split at the box bounds).
fn covered(b: &Aabb, boxes: &[Aabb]) -> bool {
    let mut cuts: [Vec<f64>; 3] = [vec![b.min[0], b.max[0]], vec![b.min[1], b.max[1]], vec![b.min[2], b.max[2]]];
    for o in boxes {
        for (a, cut) in cuts.iter_mut().enumerate() {
            for v in [o.min[a], o.max[a]] {
                if v > b.min[a] && v < b.max[a] {
                    cut.push(v);
                }
            }
        }
    }
    for c in &mut cuts {
        c.sort_by(f64::total_cmp);
        c.dedup();
    }
    for x in cuts[0].windows(2) {
        for y in cuts[1].windows(2) {
            for z in cuts[2].windows(2) {
                let m = [(x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0, (z[0] + z[1]) / 2.0];
                if !boxes.iter().any(|o| (0..3).all(|a| o.min[a] <= m[a] && m[a] <= o.max[a])) {
                    return false;
                }
            }
        }
    }
    true
}

impl Server {
    /// A block with a non-empty outline whose envelope touches the 1/64 slice
    /// under the cushion (search down to 1/8 lower).
    fn anchor_below(&self, b: &Aabb) -> bool {
        let anchor = Aabb {
            min: [b.min[0], b.min[1] - SLICE, b.min[2]],
            max: [next_down(b.max[0]), b.min[1], next_down(b.max[2])],
        };
        let search = anchor.expand_towards([0.0, -0.125, 0.0]);
        let v = self.v();
        for p in cells(&search) {
            let shape = self.data.shape(v.info(self.get(p)).outline);
            if shape.is_empty() {
                continue;
            }
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for s in shape {
                for a in 0..3 {
                    lo[a] = lo[a].min(s.min[a]);
                    hi[a] = hi[a].max(s.max[a]);
                }
            }
            let env = Aabb { min: lo, max: hi }.offset(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            if env.intersects(&anchor) {
                return true;
            }
        }
        false
    }

    /// All cells of the box suffocating.
    fn covered_by_suffocating(&self, b: &Aabb) -> bool {
        cells(&next_deflated(b)).all(|p| self.suffocating(self.get(p)))
    }

    /// The low slice of the cushion entirely taken up by collisions.
    fn anchor_buried(&self, b: &Aabb) -> bool {
        let slice =
            next_deflated(&Aabb { min: b.min, max: [b.max[0], b.min[1] + SLICE, b.max[2]] });
        let mut boxes = Vec::new();
        rc_physics::collide::collect(&self.world(), &slice, &mut boxes);
        let clipped: Vec<Aabb> = boxes
            .iter()
            .filter(|o| o.intersects(&slice))
            .map(|o| Aabb {
                min: std::array::from_fn(|a| o.min[a].max(slice.min[a])),
                max: std::array::from_fn(|a| o.max[a].min(slice.max[a])),
            })
            .collect();
        !clipped.is_empty() && covered(&slice, &clipped)
    }

    /// The survival test at a spot.
    fn cushion_would_survive(&self, b: &Aabb) -> bool {
        self.anchor_below(b) && !self.covered_by_suffocating(b)
    }

    pub(super) fn cushion_survives(&self, e: &Nbt) -> bool {
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.cushion_would_survive(&cushion_box(p))
    }

    /// Colour carried by a cushion stack (`cushion/color`), `None` if it is not one.
    fn cushion_color_of(&self, st: &Stack) -> Option<u8> {
        let c = crate::items::component(st, "minecraft:cushion/color")
            .and_then(Nbt::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.life
                    .items
                    .default_component(st, "minecraft:cushion/color")
                    .and_then(|v| v.as_str().map(str::to_owned))
            })?;
        DYES.iter().position(|d| *d == c).map(|i| i as u8)
    }

    /// Touched point recomputed on the collision shape for blocks of the tag
    /// `cushion_uses_collision_shape` (cauldrons, hopper, composter): (block, face, point).
    fn cushion_hit(&self, clicked: P, face: Dir, at: [f64; 3]) -> (P, Dir, [f64; 3]) {
        if !self.btag_id(clicked, crate::tag_bits::TagId::CushionUsesCollisionShape) {
            return (clicked, face, at);
        }
        let from = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let ray = [at[0] - from[0], at[1] - from[1], at[2] - from[2]];
        let len = (ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2]).sqrt();
        if len < 1.0e-4 {
            return (clicked, face, at);
        }
        let to = std::array::from_fn(|i| at[i] + ray[i] / len * 0.001);
        let shape = self.data.shape(self.v().info(self.get(clicked)).collision);
        let mut best: Option<(f64, Dir)> = None;
        for s in shape {
            let b = Aabb { min: s.min, max: s.max }.offset(
                f64::from(clicked[0]),
                f64::from(clicked[1]),
                f64::from(clicked[2]),
            );
            if let Some(h) = clip_box(&b, from, to)
                && best.is_none_or(|x| h.0 < x.0)
            {
                best = Some(h);
            }
        }
        match best {
            Some((t, d)) => (clicked, d, std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t)),
            None => (clicked, face, at),
        }
    }

    /// Placing a cushion; `None` if the item is not a cushion. `at`: absolute touched point.
    pub(crate) fn cushion_use_on(&mut self, slot: usize, st: &Stack, clicked: P, face: Dir, at: [f64; 3]) -> Option<Use> {
        let col = self.cushion_color_of(st)?;
        let (hit_pos, hit_face, hit) = self.cushion_hit(clicked, face, at);
        if hit_face != Dir::Up {
            return Some(Use::Fail);
        }
        let pos = if self.replaceable_by(self.get(hit_pos), &st.id, true, hit_face, false) {
            hit_pos
        } else {
            [hit_pos[0], hit_pos[1] + 1, hit_pos[2]]
        };
        let ep = [f64::from(pos[0]) + 0.5, hit[1], f64::from(pos[2]) + 0.5];
        let b = cushion_box(ep);
        if !self.cushion_would_survive(&b) || self.anchor_buried(&b) {
            return Some(Use::Fail);
        }
        if self.cushion_in(&b) {
            return Some(Use::Fail);
        }
        // Turned toward the player, in quarter turns.
        let quarter = ((f64::from(self.yaw) / 90.0 + 0.5).floor() as i32) & 3;
        let yaw = quarter as f32 * 90.0;
        let u = self.deco_uuid();
        let mut f = base_fields(DKind::Cushion, ep, yaw, 0.0, u);
        let bp = ep.map(|v| v.floor() as i32);
        f.push(("block_pos".into(), Nbt::IntArray(bp.to_vec())));
        f.push(("color".into(), Nbt::String(DYES[col as usize].into())));
        let mut e = Nbt::Compound(f);
        self.apply_stack_config(&mut e, st);
        let id = self.deco_spawn(e);
        self.cushion_fire_check(id, false);
        self.world_sound("minecraft:entity.cushion.place", 4, ep, 0.75, 0.8);
        self.game_event_at("minecraft:entity_place", ep, Some(Ent::Player));
        self.consume_held(slot);
        Some(Use::Success)
    }

    /// A cushion whose box touches `b`.
    fn cushion_in(&self, b: &Aabb) -> bool {
        self.deco
            .live
            .iter()
            .filter(|l| l.kind == DKind::Cushion && !l.removed)
            .filter_map(|l| self.deco_nbt(l.id))
            .any(|e| self.deco_box(e).is_some_and(|o| o.intersects(b)))
    }

    /// Fire in its box (on placement and at each check); `lava`:
    /// also the lava of its block. Cushion destroyed, item dropped.
    pub(super) fn cushion_fire_check(&mut self, id: i32, lava: bool) {
        let Some(e) = self.deco_nbt(id).cloned() else { return };
        if flag(&e, "Invulnerable") {
            return;
        }
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let b = cushion_box(p);
        let cell = p.map(|v| v.floor() as i32);
        let f = self.fluid_at(cell);
        let in_lava = lava && f.kind == 2 && {
            let full = self.fluid_at([cell[0], cell[1] + 1, cell[2]]).kind == 2;
            let top = f64::from(cell[1]) + if full { 1.0 } else { f64::from(f.own_height()) };
            b.min[1] < top && b.max[1] > f64::from(cell[1])
        };
        if in_lava {
            // Hit of 4, then burn sound (0.4; 2 + random × 0.4).
            self.deco_remove(id);
            self.game_event_at("minecraft:entity_die", p, None);
            self.cushion_dropped(&e, None);
            let pitch = 2.0 + self.life.rng.next_float() * 0.4;
            self.deco_sound(&e, "minecraft:entity.generic.burn", 6, 0.4, pitch);
            return;
        }
        if cells(&next_deflated(&b)).any(|q| self.btag_id(q, crate::tag_bits::TagId::Fire)) {
            self.deco_remove(id);
            self.game_event_at("minecraft:entity_die", p, None);
            self.cushion_dropped(&e, None);
        }
    }

    /// Sound, wool shards, item (except creative player or rule).
    pub(super) fn cushion_dropped(&mut self, e: &Nbt, by: Option<Ent>) {
        let creative = by == Some(Ent::Player) && self.game_mode == 1;
        self.deco_sound(e, "minecraft:entity.cushion.break", 6, 1.0, 1.0);
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let wool = format!("minecraft:{}_wool", DYES[color(e) as usize]);
        if let Some(s) = self.states.state_of(&self.data, &Nbt::String(wool)) {
            self.block_particles(s, [p[0], p[1] + 0.25 * 0.666_666_666_666_666_6, p[2]], [0.25, 0.0625, 0.25], 0.05, 10);
        }
        if self.rule("minecraft:entity_drops", true) && !creative {
            let item = format!("minecraft:{}_cushion", DYES[color(e) as usize]);
            self.deco_drop_at(e, None, named(&item, e));
        }
    }

    /// Right click on a cushion: the player sits on it, unless sneaking,
    /// cushion free and remount delay elapsed.
    pub(super) fn cushion_interact(&mut self, id: i32, secondary: bool) -> bool {
        if secondary || self.deco.seat == Some(id) || self.mobs.mounts.riding.boarding_cooldown > 0 {
            return false;
        }
        let Some(e) = self.deco_nbt(id).cloned() else { return false };
        // The previous mount is left first.
        self.player_stop_riding();
        self.cushion_stand_up();
        self.deco.seat = Some(id);
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.game_event_at("minecraft:entity_mount", p, Some(Ent::Player));
        let seat = cushion_seat(p);
        self.teleport_to(seat);
        self.last_good = seat;
        let player = self.player_id;
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Passengers {
            vehicle: id,
            passengers: vec![player],
        }));
        self.deco_sound(&e, "minecraft:entity.cushion.sit", 6, 1.0, 1.0);
        true
    }

    /// Tick of the sitting player: sneaking, they stand up; otherwise they stay on the cushion.
    pub(crate) fn cushion_seat_tick(&mut self) {
        let Some(id) = self.deco.seat else { return };
        let Some(p) = self.deco_nbt(id).and_then(|e| nbtio::f64s::<3>(e.get("Pos"))) else {
            self.cushion_ejected(id, None);
            return;
        };
        if self.sneaking {
            self.cushion_stand_up();
        } else if self.teleport.is_none() {
            let seat = cushion_seat(p);
            self.pos = seat;
            self.last_good = seat;
        }
    }

    /// The player leaves their cushion: placed above, standing-up sound.
    pub(crate) fn cushion_stand_up(&mut self) {
        let Some(p) = self.cushion_unseat() else { return };
        // On top of the cushion's box.
        let target = [p[0], p[1] + 0.25, p[2]];
        self.teleport_to(target);
        self.last_good = target;
    }

    /// The player leaves their cushion without being put back: passengers emptied, standing-up
    /// sound, event; returns the cushion position.
    pub(crate) fn cushion_unseat(&mut self) -> Option<[f64; 3]> {
        let id = self.deco.seat.take()?;
        self.mobs.mounts.riding.boarding_cooldown = 0; // The player's forced dismount from their vehicle resets the delay to 0
        let e = self.deco_nbt(id).cloned()?;
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Passengers {
            vehicle: id,
            passengers: Vec::new(),
        }));
        self.deco_sound(&e, "minecraft:entity.cushion.get_up", 6, 1.0, 1.0);
        self.game_event_at("minecraft:entity_dismount", p, Some(Ent::Player));
        Some(p)
    }

    /// Cushion removed under the player: they get down without sound, in its place (at least at the height
    /// of the cushion).
    pub(super) fn cushion_ejected(&mut self, id: i32, at: Option<[f64; 3]>) {
        if self.deco.seat != Some(id) {
            return;
        }
        self.deco.seat = None;
        self.mobs.mounts.riding.boarding_cooldown = 0; // The player's forced dismount from their vehicle resets the delay to 0
        let y = at.map_or(self.pos[1], |p| self.pos[1].max(p[1]));
        let target = [self.pos[0], y, self.pos[2]];
        self.teleport_to(target);
        self.last_good = target;
    }
}

/// Entry of a segment into a box (slab method): fraction and face touched.
fn clip_box(b: &Aabb, from: [f64; 3], to: [f64; 3]) -> Option<(f64, Dir)> {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let mut face = None;
    let faces = [(Dir::West, Dir::East), (Dir::Down, Dir::Up), (Dir::North, Dir::South)];
    for a in 0..3 {
        if d[a].abs() < 1.0e-12 {
            if from[a] < b.min[a] || from[a] > b.max[a] {
                return None;
            }
            continue;
        }
        let (n, f) = ((b.min[a] - from[a]) / d[a], (b.max[a] - from[a]) / d[a]);
        let (near, far, side) = if n < f { (n, f, faces[a].0) } else { (f, n, faces[a].1) };
        if near > t0 {
            t0 = near;
            face = Some(side);
        }
        t1 = t1.min(far);
        if t0 > t1 {
            return None;
        }
    }
    face.map(|f| (t0, f))
}
