//! Projectile movement, tick by tick like the game: trace against collision shapes
//! (colliding block, fluids ignored), entity boxes (margin:
//! the margin computation), arrows (piercing), thrown items
//! (the tick of a thrown projectile), rockets (`move` with collisions), fireballs.

use rc_physics::Aabb;

use super::{DEG, PKind, Proj, lerp_rotation};
use crate::mobs::mob::Ent;
use crate::mobs::mth::atan2;
use crate::server::Server;

/// Faces: down, up, north, south, west, east.
pub const FACES: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Result of a trace against blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHit {
    pub loc: [f64; 3],
    pub pos: [i32; 3],
    /// Face touched (index into `FACES`).
    pub face: usize,
    pub inside: bool,
    /// Hit on the world border, point brought back inside.
    pub border: bool,
}

/// Result of a trace against an entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityHit {
    pub ent: Target,
    pub loc: [f64; 3],
}

/// Touchable target: mob, player, or item on the ground (fishing bobber).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Living(Ent),
    Item(i32),
    /// F15: dragon part (dragon identifier, part index); the damage goes to the
    /// dragon through this part.
    Part(i32, u8),
    /// F15: End crystal (always "catchable").
    Crystal(i32),
    /// Other touchable projectile: shulker bullet, or deflectable projectile (tag
    /// `redirectable_projectile`).
    Proj(i32),
    /// G5-F06: minecart.
    Cart(i32),
}

impl Target {
    /// Living entity hit (the dragon for one of its parts).
    pub fn living(self) -> Option<Ent> {
        match self {
            Target::Living(e) => Some(e),
            Target::Part(id, _) => Some(Ent::Mob(id)),
            Target::Item(_) | Target::Crystal(_) | Target::Proj(_) | Target::Cart(_) => None,
        }
    }

    /// Non-living target that takes damage: crystal, projectile, minecart.
    pub fn other(self) -> bool {
        matches!(self, Target::Crystal(_) | Target::Proj(_) | Target::Cart(_))
    }
}

/// Projectile of the tag `redirectable_projectile` (large fireball, wind charges):
/// touchable by another projectile, which sends it back.
pub fn redirectable(k: PKind) -> bool {
    matches!(k, PKind::LargeFireball | PKind::WindCharge | PKind::BreezeWindCharge)
}

/// What a projectile touches during its movement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Miss([f64; 3]),
    Block(BlockHit),
    Entity(EntityHit),
}

impl Hit {
    pub fn loc(&self) -> [f64; 3] {
        match self {
            Hit::Miss(p) => *p,
            Hit::Block(b) => b.loc,
            Hit::Entity(e) => e.loc,
        }
    }
}

/// The most aligned face (first in case of a tie).
pub fn nearest_face(d: [f64; 3]) -> usize {
    let mut best = 0;
    let mut dot = f64::MIN;
    for (i, f) in FACES.iter().enumerate() {
        let v = f64::from(f[0]) * d[0] + f64::from(f[1]) * d[1] + f64::from(f[2]) * d[2];
        if v > dot {
            dot = v;
            best = i;
        }
    }
    best
}

/// Opposite face.
pub fn opposite(face: usize) -> usize {
    face ^ 1
}

/// Entry fraction into the box (entry faces only, tolerance 1e-7),
/// with the face touched.
pub fn aabb_clip(b: &Aabb, from: [f64; 3], to: [f64; 3]) -> Option<(f64, usize)> {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut scale = 1.0f64;
    let mut face = None;
    // Axis x (west/east), y (down/up), z (north/south); b1, b2 the two other axes.
    let axes: [(usize, usize, usize, usize, usize); 3] = [(0, 1, 2, 4, 5), (1, 2, 0, 0, 1), (2, 0, 1, 2, 3)];
    for (a, b1, b2, neg_face, pos_face) in axes {
        let (plane, f) = if d[a] > 1.0e-7 {
            (b.min[a], neg_face)
        } else if d[a] < -1.0e-7 {
            (b.max[a], pos_face)
        } else {
            continue;
        };
        let s = (plane - from[a]) / d[a];
        let pb = from[b1] + s * d[b1];
        let pc = from[b2] + s * d[b2];
        if 0.0 < s
            && s < scale
            && b.min[b1] - 1.0e-7 < pb
            && pb < b.max[b1] + 1.0e-7
            && b.min[b2] - 1.0e-7 < pc
            && pc < b.max[b2] + 1.0e-7
        {
            scale = s;
            face = Some(f);
        }
    }
    face.map(|f| (scale, f))
}

fn contains(b: &Aabb, p: [f64; 3]) -> bool {
    (0..3).all(|k| p[k] >= b.min[k] && p[k] < b.max[k])
}

fn inflate(b: &Aabb, m: f64) -> Aabb {
    Aabb { min: b.min.map(|v| v - m), max: b.max.map(|v| v + m) }
}

fn lerp3(t: f64, a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + t * (b[i] - a[i]))
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The computation of a projectile's collision margin.
pub fn margin(tick_count: i32) -> f64 {
    f64::from(((tick_count - 2) as f32 / 20.0).clamp(0.0, 0.3))
}

impl Server {
    /// Trace of the segment against collision shapes (colliding blocks, without fluid).
    pub(crate) fn proj_clip(&self, from: [f64; 3], to: [f64; 3]) -> Option<BlockHit> {
        if from == to {
            return None;
        }
        let te = lerp3(-1.0e-7, to, from);
        let fe = lerp3(-1.0e-7, from, to);
        let mut cell = fe.map(|v| v.floor() as i32);
        if let Some(h) = self.clip_cell_hit(cell, from, to) {
            return Some(h);
        }
        let d: [f64; 3] = std::array::from_fn(|i| te[i] - fe[i]);
        let sign = d.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
        let frac = |v: f64| v - v.floor();
        let mut t: [f64; 3] =
            std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(fe[i]) } else { frac(fe[i]) });
        while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
            let a = if t[0] < t[1] {
                if t[0] < t[2] { 0 } else { 2 }
            } else if t[1] < t[2] {
                1
            } else {
                2
            };
            cell[a] += sign[a];
            t[a] += delta[a];
            if let Some(h) = self.clip_cell_hit(cell, from, to) {
                return Some(h);
            }
        }
        None
    }

    /// Starting from inside the world border, a path that would end outside (at the block hit or at the end)
    /// touches the border at the point brought back inside,
    /// face most aligned with the path.
    pub(crate) fn proj_clip_border(&self, from: [f64; 3], to: [f64; 3]) -> Option<BlockHit> {
        let hit = self.proj_clip(from, to);
        let end = hit.map_or(to, |h| h.loc);
        let b = self.border_bounds();
        let inside = |p: [f64; 3]| p[0] >= b[0] && p[0] < b[1] && p[2] >= b[2] && p[2] < b[3];
        if !inside(from) || inside(end) {
            return hit;
        }
        let e = f64::from(1.0e-5f32);
        let c = [end[0].clamp(b[0], b[1] - e), end[1], end[2].clamp(b[2], b[3] - e)];
        let d = [end[0] - from[0], end[1] - from[1], end[2] - from[2]];
        Some(BlockHit { loc: c, pos: c.map(|v| v.floor() as i32), face: nearest_face(d), inside: false, border: true })
    }

    /// The intersection of a ray with a one-cell shape on the original segment.
    fn clip_cell_hit(&self, cell: [i32; 3], from: [f64; 3], to: [f64; 3]) -> Option<BlockHit> {
        let mut boxes = Vec::new();
        self.phys.collision(self.st(cell), &mut boxes);
        if boxes.is_empty() {
            return None;
        }
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1.0e-7 {
            return None;
        }
        let test = [from[0] + d[0] * 0.001, from[1] + d[1] * 0.001, from[2] + d[2] * 0.001];
        let c = cell.map(f64::from);
        if boxes.iter().any(|b| contains(&b.offset(c[0], c[1], c[2]), test)) {
            return Some(BlockHit {
                loc: test,
                pos: cell,
                face: opposite(nearest_face(d)),
                inside: true,
                border: false,
            });
        }
        let mut best: Option<(f64, usize)> = None;
        for b in &boxes {
            let bb = b.offset(c[0], c[1], c[2]);
            // Same sweep as the intersection of a ray with a list of boxes: the scale shrinks from one box to the next.
            let mut sub = aabb_clip(&bb, from, to);
            if let (Some((s, _)), Some((bs, _))) = (sub, best)
                && s >= bs
            {
                sub = None;
            }
            if sub.is_some() {
                best = sub;
            }
        }
        best.map(|(s, face)| BlockHit { loc: lerp3(s, from, to), pos: cell, face, inside: false, border: false })
    }

    /// Possible targets of a projectile in `area`, with box and position.
    pub(crate) fn proj_targets(&self, p: &Proj, area: &Aabb, items: bool) -> Vec<(Target, Aabb, [f64; 3])> {
        let mut out = Vec::new();
        let owner = self.proj_owner(p);
        let blocks_owner = |e: Ent| !p.left_owner && owner == Some(e);
        for o in self.mobs.list.iter().flatten() {
            if !o.alive() || o.removed {
                continue;
            }
            let e = Ent::Mob(o.id);
            if blocks_owner(e) || p.pierced.contains(&o.id) {
                continue;
            }
            // F15: the dragon is not targetable, its parts are (same entity targeted).
            if let Some(d) = o.end.dragon.as_ref() {
                for i in 0..8 {
                    let b = crate::mobs::end::dragon::part_box(d, i);
                    if b.intersects(area) {
                        out.push((Target::Part(o.id, i as u8), b, d.parts[i]));
                    }
                }
                continue;
            }
            // Fireballs and shulker bullet: never an entity without physics (vex).
            if (p.kind.hurting() || p.kind == PKind::ShulkerBullet) && crate::mobs::illagers::no_physics(o) {
                continue;
            }
            let b = o.bbox();
            if b.intersects(area) {
                out.push((Target::Living(e), b, o.pos));
            }
        }
        if self.game_mode != 3 && self.ent_alive(Ent::Player) && !blocks_owner(Ent::Player) {
            let b = self.player_aabb();
            if b.intersects(area) && !p.pierced.contains(&self.player_id) {
                out.push((Target::Living(Ent::Player), b, self.pos));
            }
        }
        // "Catchable" non-living entities (except fishing hook): End crystals, shulker bullets
        // (without physics: never for a "hurting" projectile nor another bullet),
        // deflectable projectiles. The current projectile (its pre-tick copy) is excluded. A
        // wind charge targets neither a crystal nor another wind charge.
        if !items {
            let wind = p.kind.wind();
            for c in self.mobs.end.crystals.iter().filter(|c| !c.removed && !wind) {
                let b = c.bbox();
                if b.intersects(area) && !p.pierced.contains(&c.id) {
                    out.push((Target::Crystal(c.id), b, c.pos));
                }
            }
            // G5-F06: minecarts (always touchable as long as they exist).
            for c in self.carts.list.iter().flatten().filter(|c| !c.removed) {
                let b = c.bbox();
                if b.intersects(area) && !p.pierced.contains(&c.id) {
                    out.push((Target::Cart(c.id), b, c.pos));
                }
            }
            for q in self.proj.list.iter().filter(|q| !q.removed && q.id != p.id && !(wind && q.kind.wind())) {
                let ok = match q.kind {
                    PKind::ShulkerBullet => !p.kind.hurting() && p.kind != PKind::ShulkerBullet,
                    k => redirectable(k),
                };
                let b = q.bbox();
                if ok && b.intersects(area) && !p.pierced.contains(&q.id) {
                    out.push((Target::Proj(q.id), b, q.pos));
                }
            }
        }
        if items {
            for d in self.life.drops.iter().filter(|d| !d.is_removed()) {
                let b = Aabb {
                    min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                    max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
                };
                if b.intersects(area) {
                    out.push((Target::Item(d.id), b, d.pos));
                }
            }
        }
        out
    }

    /// The nearest of the boxes inflated by the margin.
    pub(crate) fn entity_hit_nearest(
        &self,
        p: &Proj,
        from: [f64; 3],
        to: [f64; 3],
        area: &Aabb,
        m: f64,
        items: bool,
    ) -> Option<EntityHit> {
        let mut best: Option<(f64, EntityHit)> = None;
        for (t, b, _) in self.proj_targets(p, area, items) {
            if let Some((s, _)) = aabb_clip(&inflate(&b, m), from, to) {
                let loc = lerp3(s, from, to);
                let dd = dist2(from, loc);
                if best.is_none_or(|x| dd < x.0) {
                    best = Some((dd, EntityHit { ent: t, loc }));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// The search for all entities touched (without the starting entity, point not projected).
    fn entity_hits_many(
        &self,
        p: &Proj,
        from: [f64; 3],
        to: [f64; 3],
        area: &Aabb,
        m: f64,
    ) -> Vec<(EntityHit, [f64; 3])> {
        let mut out = Vec::new();
        for (t, b, pos) in self.proj_targets(p, area, false) {
            if let Some((s, _)) = aabb_clip(&b, from, to) {
                out.push((EntityHit { ent: t, loc: lerp3(s, from, to) }, pos));
            } else if m > 0.0
                && let Some((s, _)) = aabb_clip(&inflate(&b, m), from, to)
            {
                let outside = lerp3(s, from, to);
                let center = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0, (b.min[2] + b.max[2]) / 2.0];
                let towards = self.proj_clip_border(outside, center).map_or(center, |h| h.loc);
                if aabb_clip(&b, outside, towards).is_some() {
                    out.push((EntityHit { ent: t, loc: outside }, pos));
                }
            }
        }
        out
    }

    /// Blocks then entities on the movement of this tick.
    pub(crate) fn hit_on_move(&self, p: &Proj, items: bool) -> Hit {
        let from = p.pos;
        let to = [from[0] + p.vel[0], from[1] + p.vel[1], from[2] + p.vel[2]];
        let block = self.proj_clip_border(from, to);
        let end = block.map_or(to, |b| b.loc);
        let area = inflate(&p.bbox().expand_towards(p.vel), 1.0);
        if let Some(e) = self.entity_hit_nearest(p, from, end, &area, margin(p.tick_count), items) {
            return Hit::Entity(e);
        }
        match block {
            Some(b) => Hit::Block(b),
            None => Hit::Miss(to),
        }
    }

    /// No box of the owner (and of their mount) touches the box
    /// stretched by the speed and inflated by 1.
    pub(crate) fn check_left_owner(&self, p: &mut Proj) {
        if p.left_owner {
            return;
        }
        let area = inflate(&p.bbox().expand_towards(p.vel), 1.0);
        p.left_owner = match self.proj_owner(p) {
            None => true,
            Some(e) => self.ent_bbox(e).is_none_or(|b| !b.intersects(&area)),
        };
    }

    /// Base tick of the entity: water (current), fire, under the world.
    pub(crate) fn proj_base_tick(&mut self, p: &mut Proj, pushed: bool) {
        let w = self.world();
        let b = p.bbox();
        let mut v = p.vel;
        let (water, lava, _) =
            rc_physics::scan_fluids_entity(&w, &self.phys, &b, p.pos, p.pos[1], pushed, self.fast_lava(), &mut v);
        p.vel = v;
        p.in_water = water > 0.0;
        if p.fire > 0 {
            p.fire -= 1;
        }
        if lava > 0.0 {
            // Lava (lava damage → setting on fire in seconds).
            p.fire = p.fire.max(300);
        }
        if p.in_water {
            p.fire = 0;
        }
        if p.pos[1] < f64::from(self.dim.height.min_y - 64) {
            p.removed = true;
        }
        p.first_tick = false;
        p.sync_fire_flag();
    }

    /// The tick of a projectile (before the base tick), then the base tick which starts with the
    /// portal. The fishing bobber, which passes no portal, calls the
    /// base tick alone.
    pub(crate) fn proj_super_tick(&mut self, p: &mut Proj, pushed: bool) {
        self.proj_mark_shot(p);
        self.check_left_owner(p);
        self.proj_portal_tick(p);
        self.proj_base_tick(p, pushed);
    }

    /// Portal handling of a projectile, on the passage marked by its movements: same
    /// dimension (gateway), moved in place with a teleport packet; elsewhere,
    /// copy in the arrival dimension (new identifier, same UUID, delay and process
    /// kept), the old one removed (it no longer touches anything). Ender pearl through a gateway:
    /// stopped dead; from the End to the surface, refused as long as its player has not seen the credits.
    pub(crate) fn proj_portal_tick(&mut self, p: &mut Proj) {
        if p.removed || !self.dims.ents.contains_key(&p.id) {
            return;
        }
        let Some(pr) = self.entity_portal_process(p.id, crate::portal::ENTITY_COOLDOWN) else { return };
        let (w, h) = p.kind.size();
        let pearl = p.kind == PKind::EnderPearl;
        let Some(t) = self.entity_destination(pr, p.pos, w, h, pearl) else { return };
        if pearl
            && p.owner == Some(self.uuid)
            && self.dim.name == crate::dims::END
            && t.dim == crate::dims::OVERWORLD
            && !self.dims.seen_credits
        {
            return;
        }
        let (vel, yaw, pitch) = crate::portal::transition_motion(&t, p.vel, p.yaw, p.pitch);
        if t.dim == self.dim.name {
            (p.pos, p.old_pos, p.vel, p.yaw, p.pitch) = (t.pos, t.pos, vel, yaw, pitch);
            if p.track.seen {
                let tp = rc_proto_v777::entity::TeleportEntity {
                    id: p.id,
                    pos: p.pos,
                    velocity: p.vel,
                    yaw: p.yaw,
                    pitch: p.pitch,
                    relative: 0,
                    on_ground: p.on_ground,
                };
                let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Teleport(tp)));
            }
            return;
        }
        let mut copy = p.clone();
        p.removed = true;
        (copy.pos, copy.old_pos, copy.vel, copy.yaw, copy.pitch) = (t.pos, t.pos, vel, yaw, pitch);
        copy.track = Default::default();
        let ep = self.dims.ents.remove(&p.id).unwrap_or_default();
        let dim = t.dim.clone();
        self.in_world(&dim, |s| {
            copy.id = s.proj_new_id();
            s.dims.ents.insert(copy.id, ep);
            s.proj.list.push(copy);
        });
        self.place_portal_ticket(&dim);
    }

    /// First tick of a projectile: `projectile_shoot` event at its position, emitted by
    /// its shooter, then "shot" flag.
    pub(crate) fn proj_mark_shot(&mut self, p: &mut Proj) {
        if !p.has_been_shot {
            let owner = self.proj_owner(p);
            self.game_event_at("minecraft:projectile_shoot", p.pos, owner);
            p.has_been_shot = true;
        }
    }

    /// Blocks crossed since the start of the tick: fire and lava ignite, water extinguishes, portals
    /// marked.
    pub(crate) fn proj_blocks_effects(&mut self, p: &mut Proj) {
        let from = p.old_pos;
        self.proj_blocks_effects_from(p, from);
    }

    /// Application of block effects (one step of the arrow).
    pub(crate) fn proj_blocks_effects_from(&mut self, p: &mut Proj, from: [f64; 3]) {
        let (w, h) = p.kind.size();
        let b = p.bbox();
        // Lowered box of the wind charge: same offset at the start.
        let dy = b.min[1] - p.pos[1];
        let to = [p.pos[0], b.min[1], p.pos[2]];
        let touched = self.portals_touched_swept([from[0], from[1] + dy, from[2]], to, w, h);
        self.entity_portal_mark(p.id, &touched, crate::portal::ENTITY_COOLDOWN);
        let b = b.deflate(1.0e-5);
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.floor() as i32);
        let arrow = p.kind.arrow_like();
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let st = self.st([x, y, z]);
                    // Arrows: wooden buttons pressed (shape re-cut tested by the button).
                    if arrow && matches!(self.v().kind(st), crate::blocks::Kind::Button { arrows: true, .. }) {
                        self.button_arrow_inside([x, y, z], p);
                        continue;
                    }
                    let n = self.state_name(st);
                    match n {
                        "minecraft:fire" | "minecraft:soul_fire" => {
                            if p.fire <= 0 {
                                p.fire = 160;
                            }
                        }
                        "minecraft:lava" => p.fire = p.fire.max(300),
                        _ => {}
                    }
                }
            }
        }
    }

    // ---------- Arrows and trident ----------

    /// The tick of an arrow (and the tick of a thrown trident for the loyalty return).
    pub(crate) fn arrow_tick(&mut self, p: &mut Proj) {
        if p.kind == PKind::Trident {
            self.trident_pre_tick(p);
            if p.removed {
                return;
            }
        }
        let physics = !p.no_physics;
        let movement = p.vel;
        let bp = p.block_pos();
        let state = self.st(bp);
        if physics {
            let mut boxes = Vec::new();
            self.phys.collision(state, &mut boxes);
            let c = bp.map(f64::from);
            if boxes.iter().any(|b| contains(&b.offset(c[0], c[1], c[2]), p.pos)) {
                p.vel = [0.0; 3];
                p.in_ground = true;
            }
        }
        if p.shake > 0 {
            p.shake -= 1;
        }
        if p.in_water || self.raining_at(bp) {
            p.fire = 0;
        }
        if p.in_ground && physics {
            if p.last_state != Some(state) && self.arrow_should_fall(p) {
                // The start of the fall.
                p.in_ground = false;
                let f = [p.rng.next_float(), p.rng.next_float(), p.rng.next_float()];
                p.vel = [
                    p.vel[0] * f64::from(f[0] * 0.2),
                    p.vel[1] * f64::from(f[1] * 0.2),
                    p.vel[2] * f64::from(f[2] * 0.2),
                ];
                p.life = 0;
            } else if p.kind != PKind::Trident || p.pickup != super::Pickup::Allowed || p.loyalty == 0 {
                p.life += 1;
                if p.life >= 1200 {
                    p.removed = true;
                }
            }
            p.in_ground_time += 1;
            // Stuck and still there: effects of the blocks at its position (held wooden button…).
            if !p.removed {
                self.proj_blocks_effects(p);
            }
            self.arrow_ground_tick(p);
            p.sync_fire_flag();
            p.sync_arrow_flags();
            return;
        }
        p.in_ground_time = 0;
        let orig = p.pos;
        if p.in_water {
            let inertia = if p.kind == PKind::Trident { 0.99f32 } else { 0.6f32 };
            p.vel = p.vel.map(|v| v * f64::from(inertia));
        }
        let yr = if physics {
            (atan2(movement[0], movement[2]) * DEG) as f32
        } else {
            (atan2(-movement[0], -movement[2]) * DEG) as f32
        };
        let xr = (atan2(movement[1], (movement[0] * movement[0] + movement[2] * movement[2]).sqrt()) * DEG) as f32;
        p.pitch = lerp_rotation(p.pitch, xr);
        p.yaw = lerp_rotation(p.yaw, yr);
        self.check_left_owner(p);
        if physics {
            let to = [orig[0] + movement[0], orig[1] + movement[1], orig[2] + movement[2]];
            let hit = self.proj_clip_border(orig, to);
            self.step_move_and_hit(p, hit, to);
        } else {
            p.pos = [orig[0] + movement[0], orig[1] + movement[1], orig[2] + movement[2]];
            self.proj_blocks_effects(p);
        }
        if !p.in_water {
            p.vel = p.vel.map(|v| v * f64::from(0.99f32));
        }
        if physics && !p.in_ground && !p.no_gravity {
            p.vel[1] -= PKind::Arrow.gravity();
        }
        let pushed = !p.in_ground;
        self.proj_super_tick(p, pushed);
        p.sync_arrow_flags();
        if p.kind == PKind::Arrow {
            self.tipped_arrow_tick(p);
        }
    }

    /// No more collision in a ±0.06 cube around the tip.
    fn arrow_should_fall(&self, p: &Proj) -> bool {
        let b = Aabb { min: p.pos.map(|v| v - 0.06), max: p.pos.map(|v| v + 0.06) };
        rc_physics::collide::is_free(&self.world(), &b)
    }

    /// Entities on the segment (piercing), then the block.
    fn step_move_and_hit(&mut self, p: &mut Proj, block: Option<BlockHit>, to: [f64; 3]) {
        let target = block.map_or(to, |b| b.loc);
        while !p.removed {
            let start = p.pos;
            let area = inflate(&p.bbox().expand_towards(p.vel), 1.0);
            // Trident already hurting: no more entities.
            let mut hits = if p.kind == PKind::Trident {
                if p.dealt_damage {
                    Vec::new()
                } else {
                    self.entity_hit_nearest(p, start, target, &area, margin(p.tick_count), false)
                        .map(|h| {
                            let pos = self.target_pos(h.ent);
                            vec![(h, pos)]
                        })
                        .unwrap_or_default()
                }
            } else {
                self.entity_hits_many(p, start, target, &area, margin(p.tick_count))
            };
            hits.sort_by(|a, b| dist2(start, a.1).total_cmp(&dist2(start, b.1)));
            let next = hits.first().map_or(target, |h| h.0.loc);
            p.pos = next;
            self.proj_blocks_effects_from(p, start);
            // Portal marked by this step: passage right away (gone elsewhere, nothing more).
            if self.entity_inside_portal_now(p.id) {
                self.proj_portal_tick(p);
                if p.removed {
                    break;
                }
            }
            if hits.is_empty() {
                if let Some(b) = block
                    && !p.removed
                {
                    // World border: the arrow bounces.
                    if b.border {
                        self.proj_deflect_reverse(p, [0.2; 3]);
                    } else {
                        self.proj_hit_block(p, b);
                        self.proj_land(p, Hit::Block(b));
                    }
                    p.needs_sync = true;
                }
                break;
            } else if !p.removed && !p.no_physics {
                let mut deflected = false;
                for (h, _) in &hits {
                    // F15: dragon part targeted.
                    self.mobs.end.part = self.dragon_part_hit(*h);
                    let d = self.proj_hit_entity(p, *h);
                    self.mobs.end.part = None;
                    if d {
                        deflected = true;
                    } else {
                        self.proj_land(p, Hit::Entity(*h));
                    }
                    if p.removed || deflected {
                        break;
                    }
                }
                p.needs_sync = true;
                if p.pierce > 0 && !deflected {
                    continue;
                }
                break;
            } else {
                break;
            }
        }
    }

    /// Position (feet) of a target.
    pub(crate) fn target_pos(&self, t: Target) -> [f64; 3] {
        match t {
            Target::Living(e) => self.ent_pos(e).unwrap_or([0.0; 3]),
            Target::Item(id) => self.life.drops.iter().find(|d| d.id == id).map_or([0.0; 3], |d| d.pos),
            Target::Part(id, i) => {
                self.mob_ref(id).and_then(|m| m.end.dragon.as_ref()).map_or([0.0; 3], |d| d.parts[usize::from(i)])
            }
            Target::Crystal(id) => self.mobs.end.crystals.iter().find(|c| c.id == id).map_or([0.0; 3], |c| c.pos),
            Target::Proj(id) => self.proj.list.iter().find(|q| q.id == id).map_or([0.0; 3], |q| q.pos),
            Target::Cart(id) => self.cart_ref(id).map_or([0.0; 3], |c| c.pos), // G5-F06
        }
    }

    // ---------- Thrown items ----------

    /// The tick of a thrown projectile.
    pub(crate) fn throwable_tick(&mut self, p: &mut Proj) {
        if !p.no_gravity {
            p.vel[1] -= p.kind.gravity();
        }
        let inertia = if p.in_water { 0.8f32 } else { 0.99f32 };
        p.vel = p.vel.map(|v| v * f64::from(inertia));
        let hit = self.hit_on_move(p, false);
        p.pos = hit.loc();
        // Rotation update (smoothed from the previous rotation).
        let v = p.vel;
        let xr = (atan2(v[1], (v[0] * v[0] + v[2] * v[2]).sqrt()) * DEG) as f32;
        let yr = (atan2(v[0], v[2]) * DEG) as f32;
        p.pitch = lerp_rotation(p.pitch, xr);
        p.yaw = lerp_rotation(p.yaw, yr);
        self.proj_blocks_effects(p);
        self.proj_super_tick(p, true);
        if !p.removed && !matches!(hit, Hit::Miss(_)) {
            self.proj_hit(p, hit);
        }
    }

    /// Disappears if its player is dead (rule), otherwise thrown item.
    pub(crate) fn pearl_tick(&mut self, p: &mut Proj) {
        if p.owner == Some(self.uuid) && self.life.vit.dead && self.rule("minecraft:ender_pearls_vanish_on_death", true)
        {
            p.removed = true;
            return;
        }
        self.throwable_tick(p);
    }

    // ---------- Rockets ----------

    /// The tick of a firework rocket.
    pub(crate) fn firework_tick(&mut self, p: &mut Proj) {
        self.proj_super_tick(p, true);
        let hit;
        if let Some(e) = p.attached {
            if let Some(pos) = self.ent_pos(e) {
                // The gliding player's client handles the push; the server follows its position.
                p.pos = pos;
                p.vel = if e == Ent::Player { self.proj_player.known_move } else { [0.0; 3] };
            }
            hit = self.hit_on_move(p, false);
        } else {
            if !p.at_angle {
                let h = if p.horizontal_collision { 1.0 } else { 1.15 };
                p.vel = [p.vel[0] * h, p.vel[1] + 0.04, p.vel[2] * h];
            }
            let movement = p.vel;
            hit = self.hit_on_move(p, false);
            self.proj_move_collide(p, movement);
            self.proj_blocks_effects(p);
            p.vel = movement;
        }
        if !p.no_physics && !p.removed && !matches!(hit, Hit::Miss(_)) {
            self.proj_hit(p, hit);
            p.needs_sync = true;
        }
        // The rotation update.
        let v = p.vel;
        let xr = (atan2(v[1], (v[0] * v[0] + v[2] * v[2]).sqrt()) * DEG) as f32;
        let yr = (atan2(v[0], v[2]) * DEG) as f32;
        p.pitch = lerp_rotation(p.pitch, xr);
        p.yaw = lerp_rotation(p.yaw, yr);
        if p.fw_life == 0 && !p.silent {
            self.world_sound("minecraft:entity.firework_rocket.launch", 8, p.pos, 3.0, 1.0);
        }
        p.fw_life += 1;
        if p.fw_life > p.fw_lifetime && !p.removed {
            self.firework_explode(p);
        }
    }

    /// The movement of an entity with collisions without stepping (block collisions), contact flags.
    pub(crate) fn proj_move_collide(&mut self, p: &mut Proj, d: [f64; 3]) {
        let w = self.world();
        let b = p.bbox();
        let m = rc_physics::collide::resolve(&w, &b, d, p.on_ground, 0.0);
        p.horizontal_collision = (m[0] - d[0]).abs() > 1.0e-5 || (m[2] - d[2]).abs() > 1.0e-5;
        let vertical = (m[1] - d[1]).abs() > 1.0e-5;
        p.on_ground = vertical && d[1] < 0.0;
        p.pos = [p.pos[0] + m[0], p.pos[1] + m[1], p.pos[2] + m[2]];
        // Blocked components cancelled (block's reaction to the fall, collisions).
        if (m[0] - d[0]).abs() > 1.0e-5 {
            p.vel[0] = 0.0;
        }
        if (m[2] - d[2]).abs() > 1.0e-5 {
            p.vel[2] = 0.0;
        }
        if vertical {
            p.vel[1] = 0.0;
        }
    }

    // ---------- Fireball ----------

    /// The tick of a destructive projectile (dispenser, blaze, ghast). A removed author is no longer
    /// resolved (author absent): the ball continues without them, the removal test never plays.
    pub(crate) fn fireball_tick(&mut self, p: &mut Proj) {
        // F14: wind charge risen to 30 above the world: burst.
        if p.kind.wind() && self.wind_above_world(p) {
            return;
        }
        let v = p.vel;
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let n = if l < 1.0e-4 { [0.0; 3] } else { v.map(|c| c / l) };
        // F14: inertia 1 (air and liquid) for the wind charge; F16: wither blue skull
        // less inert (0.73) out of water.
        let inertia = if p.kind.wind() {
            1.0f32
        } else if p.in_water {
            0.8f32
        } else if p.kind == PKind::WitherSkull && p.dangerous {
            0.73f32
        } else {
            0.95f32
        };
        p.vel = std::array::from_fn(|i| (v[i] + n[i] * p.accel) * f64::from(inertia));
        let c = ((p.pos[0].floor() as i32) >> 4, (p.pos[2].floor() as i32) >> 4);
        if !matches!(self.chunks.get(&c), Some(Some(_))) {
            p.removed = true;
            return;
        }
        let hit = self.hit_on_move(p, false);
        // The rotation towards the movement.
        let mv = p.vel;
        if mv != [0.0; 3] {
            let sd = (mv[0] * mv[0] + mv[2] * mv[2]).sqrt();
            let yr = (atan2(mv[2], mv[0]) * DEG) as f32 + 90.0;
            let xr = (atan2(sd, mv[1]) * DEG) as f32 - 90.0;
            p.pitch = lerp_rotation_raw(p.pitch, xr, 0.2);
            p.yaw = lerp_rotation_raw(p.yaw, yr, 0.2);
        }
        p.pos = hit.loc();
        self.proj_blocks_effects(p);
        self.proj_super_tick(p, true);
        // False for the wind charge (F14, without fire flag), the dragon's ball (F15)
        // and the wither skull (F16, without fire flag).
        if !p.kind.wind() && p.kind != PKind::WitherSkull {
            if p.kind != PKind::DragonFireball {
                p.fire = p.fire.max(20);
            }
            p.sync_fire_flag();
        }
        if !matches!(hit, Hit::Miss(_)) && !p.removed {
            self.proj_hit(p, hit);
        }
    }

    /// Rain falling on this block (visible sky, rainy weather).
    pub(crate) fn raining_at(&self, p: [i32; 3]) -> bool {
        self.is_raining_at(p)
    }
}

/// Rotation smoothing with a factor.
fn lerp_rotation_raw(old: f32, new: f32, k: f32) -> f32 {
    let mut o = old;
    while new - o < -180.0 {
        o -= 360.0;
    }
    while new - o >= 180.0 {
        o += 360.0;
    }
    o + k * (new - o)
}
