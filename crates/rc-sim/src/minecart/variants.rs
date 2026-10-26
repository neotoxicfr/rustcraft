//! Behaviours specific to the kinds: activator rail, furnace (fuel,
//! push), TNT (fuse, explosion, fall, rails spared), hopper (suction), spawner
//! (the carried base mob spawner), detector rail (minecarts on it, comparator output), blast of an
//! explosion on the minecarts.

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::{CKind, Cart};
use crate::blocks::{P, flag};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

impl Server {
    /// Activator rail under the minecart.
    pub(crate) fn cart_activate(&mut self, c: &mut Cart, powered: bool) {
        match c.kind {
            CKind::Rideable => {
                if powered {
                    if c.is_vehicle() {
                        self.cart_eject(c);
                    }
                    if c.hurt_time == 0 {
                        c.hurt_dir = -c.hurt_dir;
                        c.hurt_time = 10;
                        c.damage = 50.0;
                        c.track.sync_velocity = true;
                        c.sync_hurt();
                    }
                }
            }
            CKind::Hopper => c.enabled = !powered,
            CKind::Tnt => {
                if powered && c.fuse < 0 {
                    self.tnt_cart_prime(c, None);
                }
            }
            CKind::Command => {
                if powered && c.tick_count - c.last_activated >= 4 {
                    c.last_activated = c.tick_count;
                    match self.carts.ticking_index {
                        // Command run on the spot: the minecart is put back in its slot for the duration
                        // of the command (`@s`, `data`, `tp` see it), then taken back with its
                        // changes; the rest of the tick starts from there, like the game.
                        Some(i) if self.carts.list.get(i).is_some_and(Option::is_none) => {
                            self.carts.list[i] = Some(Box::new(c.clone()));
                            self.command_cart_perform(c.id, c.pos, [c.yaw, c.pitch]);
                            if let Some(b) = self.carts.list[i].take() {
                                *c = *b;
                            }
                        }
                        // Outside the minecarts' tick: at the end of the minecart's tick.
                        _ => c.pending_command = Some((c.pos, [c.yaw, c.pitch])),
                    }
                }
            }
            CKind::Chest | CKind::Furnace | CKind::Spawner => {}
        }
    }

    /// End of the tick specific to the kind.
    pub(crate) fn cart_variant_tick(&mut self, c: &mut Cart) {
        match c.kind {
            CKind::Furnace => {
                if c.fuel > 0 {
                    c.fuel -= 1;
                }
                if c.fuel <= 0 {
                    c.push = [0.0; 3];
                }
                c.set_meta(13, Meta::Bool(c.fuel > 0));
                // Smoke: local particle (nothing sent), but the 1-in-4 draw is also made
                // on the server side and advances the minecart's random.
                if c.fuel > 0 {
                    let _ = c.rng.next_int_bound(4);
                }
            }
            CKind::Tnt => {
                if c.fuse > 0 {
                    c.fuse -= 1;
                } else if c.fuse == 0 {
                    let speed = c.vel[0] * c.vel[0] + c.vel[2] * c.vel[2];
                    let by = c.ignition.flatten();
                    self.tnt_cart_explode(c, by, speed);
                }
                // No "removed" guard: after an explosion (fuse, fall), a hit against a
                // wall causes a second one (quirk of game 26.3).
                if c.horizontal_collision {
                    let speed = c.vel[0] * c.vel[0] + c.vel[2] * c.vel[2];
                    if speed >= f64::from(0.01f32) {
                        let by = c.ignition.flatten();
                        self.tnt_cart_explode(c, by, speed);
                    }
                }
            }
            CKind::Hopper => {
                if !c.removed && c.enabled && !c.consumed && self.cart_hopper_suck(c) {
                    c.consumed = true;
                }
            }
            CKind::Spawner => {
                let players = self.alive_players();
                self.spawner_tick_at(crate::mobs::spawn::SpawnerAt::Cart(c), &players);
            }
            CKind::Rideable | CKind::Chest | CKind::Command => {}
        }
    }

    // ---------- Furnace ----------

    /// The furnace minecart's slowdown before the common slowdown: push projected
    /// onto the speed, × 0.8 + push (× 0.1 in water), otherwise × 0.98.
    pub(crate) fn furnace_slowdown(&mut self, c: &mut Cart, v: [f64; 3]) -> [f64; 3] {
        let p = c.push;
        if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] > 1.0e-7 {
            let hp = p[0] * p[0] + p[2] * p[2];
            let hv = v[0] * v[0] + v[2] * v[2];
            if hp > 1.0e-4 && hv > 0.001 {
                // Projection of the push onto `v`, normalised, then set back to the length of the push.
                let vl = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
                let dot = p[0] * v[0] + p[1] * v[1] + p[2] * v[2];
                let proj = [v[0] * dot / vl, v[1] * dot / vl, v[2] * dot / vl];
                let pl = (proj[0] * proj[0] + proj[1] * proj[1] + proj[2] * proj[2]).sqrt();
                let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                c.push = if pl < 1.0e-4 { [0.0; 3] } else { proj.map(|x| x / pl * len) };
            }
            let q = c.push;
            let mut n = [v[0] * 0.8 + q[0], v[1] * 0.0 + q[1], v[2] * 0.8 + q[2]];
            if c.in_water {
                n = n.map(|x| x * 0.1);
            }
            n
        } else {
            [v[0] * 0.98, 0.0, v[2] * 0.98]
        }
    }

    // ---------- TNT ----------

    /// Source that lights the TNT: a direct projectile only if it burns, otherwise a damage
    /// type of fire or explosion.
    pub(crate) fn tnt_ignited_by(&self, ty: &str) -> bool {
        match self.carts.direct {
            Some(burning) => burning,
            None => self.dmg_tag(ty, "minecraft:is_fire") || self.dmg_tag(ty, "minecraft:is_explosion"),
        }
    }

    /// `f` with the direct source of the damage set (burning projectile or not), then restored.
    pub(crate) fn with_direct<R>(&mut self, direct: Option<bool>, f: impl FnOnce(&mut Server) -> R) -> R {
        let saved = std::mem::replace(&mut self.carts.direct, direct);
        let r = f(self);
        self.carts.direct = saved;
        r
    }

    /// Fuse of 80 ticks (if the `tnt_explodes` rule allows it), author kept,
    /// event 70, priming sound.
    pub(crate) fn tnt_cart_prime(&mut self, c: &mut Cart, source: Option<Option<Ent>>) {
        if !self.rule("minecraft:tnt_explodes", true) {
            return;
        }
        c.fuse = 80;
        if let Some(by) = source
            && c.ignition.is_none()
        {
            c.ignition = Some(by);
        }
        self.cart_event(c, 70);
        if !c.silent {
            self.world_sound("minecraft:entity.tnt.primed", 4, c.pos, 1.0, 1.0);
        }
    }

    /// Power 4 + factor × random × 1.5 × min(√v², 5); removed.
    pub(crate) fn tnt_cart_explode(&mut self, c: &mut Cart, by: Option<Ent>, speed_sqr: f64) {
        if self.rule("minecraft:tnt_explodes", true) {
            let speed = speed_sqr.sqrt().min(5.0);
            let r = c.rng.next_double();
            let power = (f64::from(c.power_base) + f64::from(c.speed_factor) * r * 1.5 * speed) as f32;
            let ty = if by.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
            let primed = c.fuse > -1;
            c.removed = true;
            let src = Some(crate::mobs::recent::vibration::Src::Other);
            self.game_event_src("minecraft:explode", c.pos, src, None);
            self.explode_with(
                None,
                by,
                Some(c.id),
                c.pos,
                power,
                false,
                ty,
                crate::mobs::explode::Blast::TntCart { primed },
            );
        } else if c.fuse > -1 {
            self.cart_discard(c);
        }
    }

    // ---------- Hopper ----------

    /// Container above (one item per slot, first slot
    /// that passes), otherwise dropped items in the suction zone, then those that touch the
    /// box widened by 0.25.
    pub(crate) fn cart_hopper_suck(&mut self, c: &mut Cart) -> bool {
        let up = [c.pos[0].floor() as i32, (c.pos[1] + 0.5 + 1.0).floor() as i32, c.pos[2].floor() as i32];
        if let Some(src) = self.container_at(up) {
            self.unseal_all(&src);
            for q in src {
                let name = self.bname(q).to_owned();
                let furnace = matches!(name.as_str(), "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker");
                let mut slots = self.slots(q);
                let order: Vec<usize> = if furnace { vec![2, 1] } else { (0..slots.len()).collect() };
                for i in order {
                    let Some(it) = slots.get(i).cloned().flatten() else { continue };
                    let Some(st) = Stack::from_nbt(&it) else { continue };
                    let one = Stack { count: 1, ..st };
                    if self.cart_insert(c, one).is_none() {
                        let n = crate::nbtio::int(&it, "count").unwrap_or(1) - 1;
                        if n <= 0 {
                            slots[i] = None;
                        } else if let Some(x) = &mut slots[i] {
                            crate::nbtio::set(x, "count", Nbt::Int(n as i32));
                        }
                        self.store_slots(q, &slots);
                        return true;
                    }
                }
            }
            return false;
        }
        // Suction zone (column from 11/16 to 2 above the bottom centre lowered by 0.5).
        let (x, y, z) = (c.pos[0], c.pos[1] + 0.5, c.pos[2]);
        let zone = rc_physics::Aabb { min: [x - 0.5, y - 0.5 + 11.0 / 16.0, z - 0.5], max: [x + 0.5, y + 1.5, z + 0.5] };
        if self.cart_take_items(c, &zone) {
            return true;
        }
        let b = c.bbox();
        let grown = rc_physics::Aabb { min: [b.min[0] - 0.25, b.min[1], b.min[2] - 0.25], max: [b.max[0] + 0.25, b.max[1], b.max[2] + 0.25] };
        self.cart_take_items(c, &grown)
    }

    /// Dropped items touching `zone`: the first one taken whole stops the search.
    fn cart_take_items(&mut self, c: &mut Cart, zone: &rc_physics::Aabb) -> bool {
        let hit = |p: [f64; 3]| {
            let h = 0.125;
            p[0] + h > zone.min[0]
                && p[0] - h < zone.max[0]
                && p[1] + 0.25 > zone.min[1]
                && p[1] < zone.max[1]
                && p[2] + h > zone.min[2]
                && p[2] - h < zone.max[2]
        };
        let ids: Vec<i32> = self.life.drops.iter().filter(|d| !d.is_removed() && hit(d.pos)).map(|d| d.id).collect();
        for id in ids {
            let Some(d) = self.life.drops.iter().find(|d| d.id == id) else { continue };
            let stack = d.stack.clone();
            let rest = self.cart_insert(c, stack.clone());
            match rest {
                None => {
                    if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == id) {
                        d.stack.count = 0;
                        d.mark_removed();
                    }
                    return true;
                }
                Some(r) if r.count != stack.count => {
                    if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == id) {
                        d.stack.count = r.count;
                    }
                    let net = self.net(Some(&r));
                    let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Data {
                        id,
                        values: vec![(8, Meta::Item(net))],
                    }));
                }
                Some(_) => {}
            }
        }
        false
    }

    /// Insertion of an item into a container by a hopper towards the minecart: empty slot taken whole, otherwise merge;
    /// returns what did not fit.
    pub(crate) fn cart_insert(&mut self, c: &mut Cart, mut s: Stack) -> Option<Stack> {
        self.cart_unpack_loot(c, false);
        for i in 0..c.items.len() {
            if s.count <= 0 {
                break;
            }
            match &mut c.items[i] {
                slot @ None => {
                    let max = self.life.items.max_stack(&s);
                    let mut put = s.clone();
                    put.count = put.count.min(max);
                    *slot = Some(put);
                    s.count = 0;
                }
                Some(cur) if crate::items::same(cur, &s) => {
                    let space = self.life.items.max_stack(&s) - cur.count;
                    let n = s.count.min(space).max(0);
                    cur.count += n;
                    s.count -= n;
                }
                Some(_) => {}
            }
        }
        (s.count > 0).then_some(s)
    }

    // ---------- Hoppers (blocks) ----------

    /// Container minecart in cell `p` (draw among those present).
    pub(crate) fn cart_container_at(&mut self, p: P) -> Option<i32> {
        let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let b = rc_physics::Aabb { min: [x, y, z], max: [x + 1.0, y + 1.0, z + 1.0] };
        // Called every tick by any hopper without a neighbouring container block: two passes without
        // allocation (count, then take the nth).
        let hit = |c: &&Cart| !c.removed && matches!(c.kind, CKind::Chest | CKind::Hopper) && c.bbox().intersects(&b);
        let n = self.carts.list.iter().flatten().map(|c| &**c).filter(hit).count();
        if n == 0 {
            return None;
        }
        let k = self.bs.rng.next_int_bound(n as i32) as usize;
        self.carts.list.iter().flatten().map(|c| &**c).filter(hit).nth(k).map(|c| c.id)
    }

    /// Hopper `p` under a container minecart: one item from the first slot that passes.
    pub(crate) fn hopper_pull_from_cart(&mut self, p: P, cid: i32) -> bool {
        let Some((ci, mut c)) = self.cart_take(cid) else { return false };
        self.cart_unpack_loot(&mut c, false);
        let mut moved = false;
        for i in 0..c.items.len() {
            let Some(s) = c.items[i].clone() else { continue };
            if self.insert_one(&[p], &s.to_nbt(None), None) {
                if let Some(x) = c.items[i].as_mut() {
                    x.count -= 1;
                    if x.count <= 0 {
                        c.items[i] = None;
                    }
                }
                moved = true;
                break;
            }
        }
        self.cart_put(ci, c);
        moved
    }

    /// Hopper `p` that pours into a container minecart: one item, first slot that passes.
    pub(crate) fn hopper_push_into_cart(&mut self, p: P, cid: i32) -> bool {
        let Some((ci, mut c)) = self.cart_take(cid) else { return false };
        self.cart_unpack_loot(&mut c, false);
        let full = c.items.iter().all(|s| s.as_ref().is_some_and(|s| s.count >= self.life.items.max_stack(s)));
        let mut moved = false;
        if !full {
            let mut slots = self.slots(p);
            for i in 0..slots.len() {
                let Some(it) = slots[i].clone() else { continue };
                let Some(st) = Stack::from_nbt(&it) else { continue };
                if self.cart_insert(&mut c, Stack { count: 1, ..st }).is_none() {
                    let n = crate::nbtio::int(&it, "count").unwrap_or(1) - 1;
                    if n <= 0 {
                        slots[i] = None;
                    } else if let Some(x) = &mut slots[i] {
                        crate::nbtio::set(x, "count", Nbt::Int(n as i32));
                    }
                    self.store_slots(p, &slots);
                    moved = true;
                    break;
                }
            }
        }
        self.cart_put(ci, c);
        moved
    }

    // ---------- Detector rail ----------

    /// Minecarts on the detector rail `p` (box reduced by 0.2).
    fn carts_on_detector(&self, p: P) -> impl Iterator<Item = &Cart> {
        let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let b = rc_physics::Aabb { min: [x + 0.2, y, z + 0.2], max: [x + 0.8, y + 0.8, z + 0.8] };
        self.carts.list.iter().flatten().filter(move |c| !c.removed && c.bbox().intersects(&b)).map(|b| &**b)
    }

    /// Minecart entered an unpowered detector rail.
    pub(crate) fn detector_rail_inside(&mut self, p: P) {
        let s = self.get(p);
        if self.v().kind(s) == crate::blocks::Kind::DetectorRail && !self.v().get_bool(s, "powered") {
            self.detector_check(p, s);
        }
    }

    /// Scheduled tick of the powered detector rail.
    pub(crate) fn detector_rail_tick(&mut self, s: u32, p: P) {
        if self.v().get_bool(s, "powered") {
            self.detector_check(p, s);
        }
    }

    /// State updated (connected rails, neighbours, below), tick at 20 if pressed,
    /// comparator output notified.
    fn detector_check(&mut self, p: P, s: u32) {
        // The ticking minecart is out of the list: it counts too (caller in progress).
        let pressed = self.carts_on_detector(p).next().is_some() || self.detector_ticking_cart(p);
        let was = self.v().get_bool(s, "powered");
        let b = self.v().block(s);
        if pressed != was {
            let ns = self.v().set_bool(s, "powered", pressed);
            self.set_block(p, ns, flag::ALL);
            for q in self.rail_connections(p, ns) {
                let qs = self.get(q);
                let qb = self.v().block(qs);
                self.neighbor_changed(qs, q, qb, false);
            }
            self.update_neighbors_at(p, b);
            self.update_neighbors_at([p[0], p[1] - 1, p[2]], b);
        }
        if pressed {
            self.schedule_tick(p, b, 20);
        }
        self.update_neighbour_for_output_signal(p, b);
    }

    /// Comparator output of a powered detector rail: success of a command block minecart, otherwise
    /// fill of a container minecart, otherwise 0.
    pub(crate) fn detector_analog(&self, p: P, s: u32) -> i32 {
        if !self.v().get_bool(s, "powered") {
            return 0;
        }
        if let Some(c) = self.carts_on_detector(p).find(|c| c.kind == CKind::Command) {
            return crate::nbtio::int(&c.nbt, "SuccessCount").unwrap_or(0) as i32;
        }
        if let Some(c) = self.carts_on_detector(p).find(|c| matches!(c.kind, CKind::Chest | CKind::Hopper)) {
            return self.cart_signal(c);
        }
        0
    }

    /// Minecart being ticked (out of the list) placed on detector `p`.
    fn detector_ticking_cart(&self, p: P) -> bool {
        self.carts.ticking.is_some_and(|t| {
            let b = super::bbox_at(t);
            let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            let d = rc_physics::Aabb { min: [x + 0.2, y, z + 0.2], max: [x + 0.8, y + 0.8, z + 0.8] };
            b.intersects(&d)
        })
    }

    // ---------- Explosions ----------

    /// Blast on the minecarts: damage (the TNT minecart
    /// is primed), push according to the visible part; ignored if a mob is the author and
    /// `mob_griefing` is false.
    pub(crate) fn carts_explosion_hit(&mut self, center: [f64; 3], radius: f32, exclude: Option<i32>, ty: &str, by: Option<Ent>) {
        if matches!(by, Some(Ent::Mob(_))) && !self.rule("minecraft:mob_griefing", true) {
            return;
        }
        let double = f64::from(radius * 2.0);
        for i in 0..self.carts.list.len() {
            let Some(mut c) = self.carts.list[i].take() else { continue };
            if c.removed || Some(c.id) == exclude {
                self.carts.list[i] = Some(c);
                continue;
            }
            let d = [c.pos[0] - center[0], c.pos[1] - center[1], c.pos[2] - center[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / double;
            if dist > 1.0 {
                self.carts.list[i] = Some(c);
                continue;
            }
            let eye = c.pos[1] + f64::from(0.595f32);
            let mut dir = [c.pos[0] - center[0], eye - center[1], c.pos[2] - center[2]];
            let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            if l == 0.0 {
                self.carts.list[i] = Some(c);
                continue;
            }
            dir = dir.map(|v| v / l);
            let exposure = f64::from(self.seen_percent(center, &c.bbox()));
            let pow = (1.0 - dist) * exposure;
            let dmg = ((pow * pow + pow) / 2.0 * 7.0 * double + 1.0) as f32;
            self.cart_hurt(&mut c, ty, dmg, by);
            let kb = dir.map(|v| v * (1.0 - dist) * exposure);
            c.vel = [c.vel[0] + kb[0], c.vel[1] + kb[1], c.vel[2] + kb[2]];
            c.needs_sync = true;
            self.carts.list[i] = Some(c);
        }
    }

    // ---------- Command block ----------

    /// Pending command of a command block minecart (put back in the list).
    pub(crate) fn cart_run_pending_command(&mut self, id: i32) {
        let Some(c) = self.cart_mut(id) else { return };
        let Some((pos, rot)) = c.pending_command.take() else { return };
        self.command_cart_perform(id, pos, rot);
    }
}
