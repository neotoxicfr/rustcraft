//! Use of minecarts: right click (mounting, chest, hopper, furnace fuel), hits and
//! breaking, contents dropped, menus, loot, passengers
//! (player and mobs), dismounting, placing on a rail.

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::{CKind, Cart, SEAT};
use crate::menus::Menu;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::recent::vibration::Src;
use crate::player::Stack;
use crate::server::Server;

/// Horizontal direction (south, west, north, east) of a yaw.
fn direction_of(yaw: f32) -> [i32; 2] {
    match ((f64::from(yaw) / 90.0 + 0.5).floor() as i32) & 3 {
        0 => [0, 1],
        1 => [-1, 0],
        2 => [0, -1],
        _ => [1, 0],
    }
}

/// Clockwise rotation of a horizontal direction.
fn clockwise(d: [i32; 2]) -> [i32; 2] {
    [-d[1], d[0]]
}

/// The dismount offsets according to the direction.
fn offsets_for(fwd: [i32; 2]) -> [[i32; 2]; 8] {
    let right = clockwise(fwd);
    let left = [-right[0], -right[1]];
    let back = [-fwd[0], -fwd[1]];
    [
        right,
        left,
        [back[0] + right[0], back[1] + right[1]],
        [back[0] + left[0], back[1] + left[1]],
        [fwd[0] + right[0], fwd[1] + right[1]],
        [fwd[0] + left[0], fwd[1] + left[1]],
        back,
        fwd,
    ]
}

impl Server {
    // ---------- Right click ----------

    /// Player interaction with entity `id`; true if it is a minecart (handled here).
    pub(crate) fn cart_interact(&mut self, id: i32, hand: i32) -> bool {
        let Some((i, mut c)) = self.cart_take(id) else { return false };
        if self.cart_in_reach(&c) {
            self.cart_use(&mut c, hand);
        }
        self.cart_put(i, c);
        self.broadcast_menus();
        true
    }

    /// Interaction range (range test on the box, margin 3).
    fn cart_in_reach(&self, c: &Cart) -> bool {
        let b = c.bbox();
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let p = [eye[0].clamp(b.min[0], b.max[0]), eye[1].clamp(b.min[1], b.max[1]), eye[2].clamp(b.min[2], b.max[2])];
        let r = self.player_attribute("minecraft:entity_interaction_range") + 3.0;
        crate::mobs::mth::dist2(eye, p) < r * r
    }

    /// `interact` of the kind (spectator: only the menu of a container opens).
    fn cart_use(&mut self, c: &mut Cart, hand: i32) {
        if self.game_mode == 3 {
            if matches!(c.kind, CKind::Chest | CKind::Hopper) {
                self.cart_open_menu(c);
            }
            return;
        }
        match c.kind {
            CKind::Rideable => {
                if !self.sneaking && !c.is_vehicle() {
                    self.player_start_riding_cart(c);
                }
            }
            CKind::Chest | CKind::Hopper => {
                let opened = self.cart_open_menu(c);
                if opened && c.kind == CKind::Chest {
                    self.game_event_src("minecraft:container_open", c.pos, Some(Src::Living(Ent::Player)), None);
                    self.anger_nearby_piglins(true);
                }
            }
            CKind::Furnace => {
                let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
                let fuel = self.slots[slot].as_ref().is_some_and(|s| self.item_tag(&s.id, "minecraft:furnace_minecart_fuel"));
                if fuel && c.fuel + 3600 <= 32000 {
                    c.fuel += 3600;
                    if c.fuel > 0 {
                        c.push = [c.pos[0] - self.pos[0], 0.0, c.pos[2] - self.pos[2]];
                    }
                    if self.game_mode != 1
                        && let Some(s) = self.slots[slot].as_mut()
                    {
                        s.count -= 1;
                        if s.count <= 0 {
                            self.slots[slot] = None;
                        }
                    }
                }
            }
            // TNT, spawner: nothing; command block: screen on the client side only.
            CKind::Tnt | CKind::Spawner | CKind::Command => {}
        }
    }

    // ---------- Hits, damage, breaking ----------

    /// Player hit on entity `id`; true if it is a minecart (the player's attack against a
    /// non-living entity: no critical hit, no weapon damage, knockback by push).
    pub(crate) fn cart_attacked(&mut self, id: i32) -> bool {
        let Some((i, mut c)) = self.cart_take(id) else { return false };
        if self.game_mode != 3 && !self.life.vit.dead && self.cart_in_reach(&c) {
            self.player_attack_cart(&mut c);
        }
        self.cart_put(i, c);
        true
    }

    fn player_attack_cart(&mut self, c: &mut Cart) {
        let weapon = self.slots[36 + self.held as usize].clone();
        let mut base = self.player_attribute("minecraft:attack_damage") as f32;
        let speed = self.player_attribute("minecraft:attack_speed");
        let delay = (1.0 / speed * 20.0) as f32;
        let scale = ((self.mobs.attack_ticker as f32 + 0.5) / delay).clamp(0.0, 1.0);
        // Sharpness: the only weapon bonus valid against any entity.
        let sharp = weapon.as_ref().map_or(0, |w| self.life.items.enchantment(w, "minecraft:sharpness"));
        let ench = if sharp > 0 { 0.5 * sharp as f32 + 0.5 } else { 0.0 };
        let magic = scale * ench;
        base *= 0.2 + scale * scale * 0.8;
        self.mobs.attack_ticker = 0;
        if base <= 0.0 && magic <= 0.0 {
            return;
        }
        let full = scale > 0.9;
        let sprint_kb = self.life.vit.sprinting && full;
        if sprint_kb {
            self.world_sound("minecraft:entity.player.attack.knockback", 7, self.pos, 1.0, 1.0);
        }
        let total = base + magic;
        // Never a critical hit against a non-living entity.
        let sweep = self.sweep_allowed(full, false, sprint_kb, weapon.as_ref());
        let hurt = self.cart_hurt(c, "minecraft:player_attack", total, Some(Ent::Player));
        if hurt {
            let kb_ench = weapon.as_ref().map_or(0, |w| self.life.items.enchantment(w, "minecraft:knockback")) as f32;
            let kb = kb_ench + if sprint_kb { 0.5 } else { 0.0 };
            if kb > 0.0 {
                let r = self.yaw * crate::mobs::mth::DEG;
                let (s, co) = (crate::mobs::mth::sin(r), crate::mobs::mth::cos(r));
                c.vel[0] += f64::from(-s * kb);
                c.vel[1] += 0.1;
                c.vel[2] += f64::from(co * kb);
                c.needs_sync = true;
                self.life.vit.sprinting = false;
            }
            if sweep {
                self.world_sound("minecraft:entity.player.attack.sweep", 7, self.pos, 1.0, 1.0);
                self.sweep_attack(&c.bbox(), None, base, scale);
            } else {
                let s = if full { "minecraft:entity.player.attack.strong" } else { "minecraft:entity.player.attack.weak" };
                self.world_sound(s, 7, self.pos, 1.0, 1.0);
            }
            if magic > 0.0 {
                let _ = self.send(Event::Entity(rc_client::EntityEvent::Animate { id: c.id, action: 2 }));
            }
            // Non-living target: the player forgets their last victim (wolf that helps them).
            self.set_last_hurt_mob(Some(Ent::Player), None);
            let invul = self.abilities().invulnerable;
            self.life.vit.exhaust(0.1, invul);
        } else {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
        }
    }

    /// Damage received (TNT included); true if the hit lands.
    pub(crate) fn cart_hurt(&mut self, c: &mut Cart, ty: &str, amount: f32, by: Option<Ent>) -> bool {
        if c.removed {
            return true;
        }
        // TNT minecart hit by a flaming arrow: explosion (handled by the projectile).
        if c.invulnerable && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability") {
            let creative = by == Some(Ent::Player) && self.game_mode == 1;
            if !creative {
                return false;
            }
        }
        c.hurt_dir = -c.hurt_dir;
        c.hurt_time = 10;
        c.track.sync_velocity = true;
        c.damage += amount * 10.0;
        c.sync_hurt();
        let src = by.map(Src::Living).or(Some(Src::Other));
        self.game_event_src("minecraft:entity_damage", c.pos, src, None);
        let creative = by == Some(Ent::Player) && self.game_mode == 1;
        let source_destroys = c.kind == CKind::Tnt && self.tnt_ignited_by(ty);
        if (creative || c.damage <= 40.0) && !source_destroys {
            if creative {
                self.cart_discard(c);
            }
        } else {
            self.cart_destroy(c, ty, by);
        }
        true
    }

    /// Break caused by a damage source: TNT primed or dropped, otherwise item dropped; contents afterwards.
    pub(crate) fn cart_destroy(&mut self, c: &mut Cart, ty: &str, by: Option<Ent>) {
        if c.kind == CKind::Tnt {
            let speed = c.vel[0] * c.vel[0] + c.vel[2] * c.vel[2];
            if !self.tnt_ignited_by(ty) && speed < f64::from(0.01f32) {
                self.cart_destroy_drop(c);
            } else if c.fuse < 0 {
                self.tnt_cart_prime(c, Some(by));
                c.fuse = c.rng.next_int_bound(20) + c.rng.next_int_bound(20);
            }
            return;
        }
        self.cart_destroy_drop(c);
        if matches!(c.kind, CKind::Chest | CKind::Hopper) && self.rule("minecraft:entity_drops", true) {
            // Contents already dropped by the death; piglins angered if the player hit themselves (neither
            // projectile, nor explosion of which they are the author).
            let direct_player =
                by == Some(Ent::Player) && self.carts.direct.is_none() && !self.dmg_tag(ty, "minecraft:is_explosion");
            if direct_player {
                self.anger_nearby_piglins(true);
            }
        }
    }

    /// Break that drops the item: killed (contents dropped), then the minecart's item with its name.
    pub(crate) fn cart_destroy_drop(&mut self, c: &mut Cart) {
        self.cart_kill(c);
        if self.rule("minecraft:entity_drops", true) {
            let mut s = Stack { id: c.kind.drop_item().to_owned(), count: 1, components: None };
            if let Some(n) = c.nbt.get("CustomName").cloned() {
                crate::items::set_component(&mut s, "minecraft:custom_name", n);
            }
            self.cart_spawn_at_location(c, s);
        }
    }

    /// Item dropped at the minecart's position (no offset), default
    /// delay.
    fn cart_spawn_at_location(&mut self, c: &Cart, s: Stack) {
        self.spawn_item_random(c.pos, s, 10);
    }

    /// `kill`: removed as killed (the contents of a container fall), then `entity_die` event.
    pub(crate) fn cart_kill(&mut self, c: &mut Cart) {
        self.cart_discard(c);
        self.game_event_src("minecraft:entity_die", c.pos, Some(Src::Other), None);
    }

    /// Removal that destroys (killed or discarded): contents dropped by a container.
    pub(crate) fn cart_discard(&mut self, c: &mut Cart) {
        if c.removed {
            return;
        }
        c.removed = true;
        if matches!(c.kind, CKind::Chest | CKind::Hopper) {
            self.cart_unpack_loot(c, false);
            let items: Vec<Stack> = c.items.iter_mut().filter_map(Option::take).collect();
            for s in items {
                self.drop_contents(c.pos, s);
            }
        }
    }

    // ---------- Menus and loot ----------

    /// Opening of the menu (spectator: refused while the loot is sealed); true if opened.
    fn cart_open_menu(&mut self, c: &mut Cart) -> bool {
        if c.loot.is_some() && self.game_mode == 3 {
            return false;
        }
        self.cart_unpack_loot(c, true);
        let window = {
            self.life.counter = self.life.counter % 100 + 1;
            self.life.counter
        };
        let menu_type = if c.kind == CKind::Chest { 2 } else { 16 };
        let mut m = Menu::container(window, menu_type, c.items.clone(), false);
        m.cart = Some(c.id);
        let title = self.cart_display(c);
        self.open_menu(m, Some(title));
        self.life.menu.is_some()
    }

    /// Displayed name of the minecart (custom name, otherwise type name), entity hover.
    fn cart_display(&self, c: &Cart) -> rc_text::Component {
        let base = match c.nbt.get("CustomName").and_then(rc_text::from_nbt) {
            Some(n) => n,
            None => rc_text::Component::translate(format!("entity.{}", c.kind.name().replace(':', ".")), Vec::new()),
        };
        let mut out = base.clone();
        out.style.hover = Some(std::sync::Arc::new(rc_text::HoverEvent::Entity {
            id: c.kind.name().to_owned(),
            uuid: c.uuid,
            name: Some(base),
        }));
        out.style.insertion = Some(std::sync::Arc::from(crate::nbtio::uuid_text(c.uuid)));
        out
    }

    /// Loot drawn: minecart position, player's luck.
    pub(crate) fn cart_unpack_loot(&mut self, c: &mut Cart, by_player: bool) {
        let Some(table) = c.loot.take() else { return };
        if by_player {
            self.s9_container_loot(&table); // S9: `player_generates_container_loot`
        }
        let mut items = std::mem::take(&mut c.items);
        self.fill_loot(&table, c.loot_seed, by_player, &mut items, c.block_pos());
        c.items = items;
    }

    /// Menu still valid: minecart present, within range + 4.
    pub(crate) fn cart_menu_valid(&self, id: i32) -> bool {
        let Some(c) = self.cart_ref(id) else { return false };
        let b = c.bbox();
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let p = [eye[0].clamp(b.min[0], b.max[0]), eye[1].clamp(b.min[1], b.max[1]), eye[2].clamp(b.min[2], b.max[2])];
        let r = self.player_attribute("minecraft:entity_interaction_range") + 4.0;
        crate::mobs::mth::dist2(eye, p) < r * r
    }

    /// Closing of the menu of a chest minecart: `container_close` event.
    pub(crate) fn cart_menu_closed(&mut self, id: i32) {
        if let Some(c) = self.cart_ref(id)
            && c.kind == CKind::Chest
        {
            let pos = c.pos;
            self.game_event_src("minecraft:container_close", pos, Some(Src::Living(Ent::Player)), None);
        }
    }

    // ---------- Passengers ----------

    /// Is the player a passenger (mob, minecart or cushion)?
    pub(crate) fn player_riding_any(&self) -> bool {
        self.mobs.mounts.riding.vehicle.is_some() || self.pcart.vehicle.is_some() || self.deco.seat.is_some()
    }

    /// Seat of a passenger (feet): position + 0.1875 (0 for a villager or a wandering
    /// trader) − attachment of the passenger (player: 0.6).
    pub(crate) fn cart_seat(c: &Cart, rider: Option<&Mob>) -> [f64; 3] {
        use crate::mobs::species::Kind;
        let lowered = rider.is_some_and(|r| matches!(r.kind, Kind::Villager | Kind::WanderingTrader));
        let point = if lowered { 0.0 } else { SEAT };
        let a = crate::mobs::mounts::riding::vehicle_attach(rider);
        [c.pos[0], c.pos[1] + point - a, c.pos[2]]
    }

    /// The player gets into the minecart.
    pub(crate) fn player_start_riding_cart(&mut self, c: &mut Cart) -> bool {
        if self.pcart.vehicle == Some(c.id) || self.sneaking || self.mobs.mounts.riding.boarding_cooldown > 0 {
            return false;
        }
        if c.is_vehicle() {
            return false;
        }
        if self.mobs.mounts.riding.vehicle.is_some() {
            self.player_stop_riding();
        }
        if self.pcart.vehicle.is_some() {
            self.player_stop_riding_cart();
        }
        self.pcart.vehicle = Some(c.id);
        self.prog.ride_started = true; // `started_riding`
        c.passengers.push(Ent::Player);
        self.game_event_at("minecraft:entity_mount", c.pos, Some(Ent::Player));
        let seat = Self::cart_seat(c, None);
        self.teleport_to(seat);
        self.last_good = seat;
        true
    }

    /// Dismounting the vehicle (teleport, death, dimension change): the player leaves their
    /// minecart without a dismount spot (position set by the caller), mount delay reset to 0.
    pub(crate) fn cart_remove_player(&mut self) {
        let Some(id) = self.pcart.vehicle.take() else { return };
        self.mobs.mounts.riding.boarding_cooldown = 0;
        if let Some((i, mut c)) = self.cart_take(id) {
            c.passengers.retain(|p| *p != Ent::Player);
            self.game_event_at("minecraft:entity_dismount", c.pos, Some(Ent::Player));
            self.cart_put(i, c);
        }
    }

    /// The player gets off their minecart (→ dismount spot).
    pub(crate) fn player_stop_riding_cart(&mut self) {
        let Some(id) = self.pcart.vehicle.take() else { return };
        self.mobs.mounts.riding.boarding_cooldown = 60;
        let target = match self.cart_take(id) {
            Some((i, mut c)) => {
                c.passengers.retain(|p| *p != Ent::Player);
                let poses: [(f32, f32, &[i32]); 3] = [(0.6, 1.8, &[0, 1, -1]), (0.6, 1.5, &[0, 1, -1]), (0.6, 0.6, &[0, 1])];
                let t = if self.cart_gone_or_portal(&c) { self.cart_dismount_free(&c, self.pos, 0.6, 1.8) } else { self.cart_dismount_location(&c, &poses) };
                self.game_event_at("minecraft:entity_dismount", c.pos, Some(Ent::Player));
                self.cart_put(i, c);
                t
            }
            None => self.pos,
        };
        self.teleport_to(target);
        self.last_good = target;
    }

    /// A mob gets into the minecart.
    pub(crate) fn cart_mob_start_riding(&mut self, c: &mut Cart, m: &mut Mob) -> bool {
        if m.mount.vehicle == Some(c.id) || m.mount.boarding_cooldown > 0 || c.is_vehicle() {
            return false;
        }
        m.mount.vehicle = Some(c.id);
        c.passengers.push(Ent::Mob(m.id));
        m.pos = Self::cart_seat(c, Some(m));
        m.old = m.pos;
        self.game_event_src("minecraft:entity_mount", c.pos, Some(Src::Living(Ent::Mob(m.id))), None);
        true
    }

    /// `/ride … mount <minecart>`: forced mounting (seat not checked) of the player or a mob.
    pub(crate) fn cart_ride_forced(&mut self, cid: i32, target: crate::commands::ents::E) -> bool {
        use crate::commands::ents::E;
        let Some((ci, mut c)) = self.cart_take(cid) else { return false };
        let ok = match target {
            E::Player => {
                if self.mobs.mounts.riding.vehicle.is_some() {
                    self.player_stop_riding();
                }
                self.pcart.vehicle = Some(c.id);
                self.prog.ride_started = true;
                c.passengers.push(Ent::Player);
                let seat = Self::cart_seat(&c, None);
                self.teleport_to(seat);
                self.last_good = seat;
                true
            }
            E::Mob(r) => match self.mob_index(r).and_then(|i| self.mobs.list[i].take().map(|m| (i, m))) {
                Some((i, mut m)) => {
                    m.mount.vehicle = Some(c.id);
                    c.passengers.push(Ent::Mob(m.id));
                    m.pos = Self::cart_seat(&c, Some(&m));
                    m.old = m.pos;
                    m.needs_sync = true;
                    self.mobs.list[i] = Some(m);
                    true
                }
                None => false,
            },
            _ => false,
        };
        self.cart_put(ci, c);
        ok
    }

    /// `/ride <mob> dismount` of a minecart.
    pub(crate) fn cart_dismount_mob(&mut self, cid: i32, mob: i32) {
        let Some((ci, mut c)) = self.cart_take(cid) else { return };
        if let Some(i) = self.mob_index(mob)
            && let Some(mut m) = self.mobs.list[i].take()
        {
            self.cart_mob_dismount(&mut c, &mut m);
            self.mobs.list[i] = Some(m);
        }
        self.cart_put(ci, c);
    }

    /// A mob gets off the minecart.
    fn cart_mob_dismount(&mut self, c: &mut Cart, m: &mut Mob) {
        c.passengers.retain(|p| *p != Ent::Mob(m.id));
        m.mount.vehicle = None;
        m.mount.boarding_cooldown = 60;
        let (w, h) = (m.width(), m.height());
        let poses: [(f32, f32, &[i32]); 1] = [(w, h, &[0, 1, -1])];
        let t = if m.removed {
            m.pos
        } else if self.cart_gone_or_portal(c) {
            self.cart_dismount_free(c, m.pos, f64::from(w), f64::from(h))
        } else {
            self.cart_dismount_location(c, &poses)
        };
        m.pos = t;
        m.old = t;
        m.needs_sync = true;
    }

    /// Everyone gets off.
    pub(crate) fn cart_eject(&mut self, c: &mut Cart) {
        for p in c.passengers.clone() {
            match p {
                Ent::Player => {
                    // The minecart is out of the list: dismount done here.
                    self.pcart.vehicle = None;
                    self.mobs.mounts.riding.boarding_cooldown = 60;
                    c.passengers.retain(|q| *q != Ent::Player);
                    let poses: [(f32, f32, &[i32]); 3] =
                        [(0.6, 1.8, &[0, 1, -1]), (0.6, 1.5, &[0, 1, -1]), (0.6, 0.6, &[0, 1])];
                    let t = if self.cart_gone_or_portal(c) { self.cart_dismount_free(c, self.pos, 0.6, 1.8) } else { self.cart_dismount_location(c, &poses) };
                    self.game_event_at("minecraft:entity_dismount", c.pos, Some(Ent::Player));
                    self.teleport_to(t);
                    self.last_good = t;
                }
                Ent::Mob(id) => {
                    if let Some(i) = self.mob_index(id)
                        && let Some(mut m) = self.mobs.list[i].take()
                    {
                        self.cart_mob_dismount(c, &mut m);
                        self.mobs.list[i] = Some(m);
                    } else {
                        c.passengers.retain(|q| *q != p);
                    }
                }
            }
        }
    }

    /// Minecart removed: its passengers get off.
    pub(crate) fn cart_release_passengers(&mut self, c: &Cart) {
        let mut c = c.clone();
        self.cart_eject(&mut c);
    }

    /// Passengers of the minecart ticked after it, then seated.
    pub(crate) fn cart_tick_passengers(&mut self, id: i32) {
        let Some(list) = self.cart_ref(id).map(|c| c.passengers.clone()) else { return };
        for p in list {
            match p {
                // The seated player (host or guest) is ticked in their own state.
                Ent::Player => {
                    if let Some(who) = self.cart_rider(id) {
                        self.as_player(who, |sv| sv.player_cart_ride_tick(id));
                    }
                }
                Ent::Mob(pid) => {
                    let Some(i) = self.mob_index(pid) else { continue };
                    let Some(mut r) = self.mobs.list[i].take() else { continue };
                    if r.removed || r.mount.vehicle != Some(id) {
                        if let Some((ci, mut c)) = self.cart_take(id) {
                            self.cart_mob_dismount(&mut c, &mut r);
                            self.cart_put(ci, c);
                        }
                        self.mobs.list[i] = Some(r);
                        continue;
                    }
                    r.vel = [0.0; 3];
                    self.tick_mob(&mut r);
                    if r.mount.vehicle == Some(id)
                        && let Some(c) = self.cart_ref(id)
                    {
                        r.pos = Self::cart_seat(c, Some(&r));
                    }
                    r.fall_distance = 0.0;
                    self.mobs.list[i] = Some(r);
                    self.tick_passengers(pid);
                }
            }
        }
    }

    /// Tick of the player riding a minecart: sneaking → gets off; otherwise seated.
    fn player_cart_ride_tick(&mut self, id: i32) {
        if self.sneaking {
            self.player_stop_riding_cart();
            return;
        }
        let Some(c) = self.cart_ref(id) else {
            self.pcart.vehicle = None;
            return;
        };
        let seat = Self::cart_seat(c, None);
        let before = self.pos;
        if self.teleport.is_none() {
            self.pos = seat;
            self.last_good = seat;
        }
        // Travel statistic.
        self.riding_stats(before, Some("minecart_one_cm"));
    }

    /// Player and minecarts after the tick: minecart gone → dismount; pushes of the player.
    pub(crate) fn cart_player_tick(&mut self) {
        if let Some(id) = self.pcart.vehicle
            && self.cart_ref(id).is_none()
        {
            self.pcart.vehicle = None;
        }
        if self.cmd.ticking.runs() {
            self.player_pushes_carts();
        }
    }

    /// Dismount spot: poses (width,
    /// height, vertical offsets) in order, cells around according to the direction of movement,
    /// otherwise the top of the box.
    pub(crate) fn cart_dismount_location(&self, c: &Cart, poses: &[(f32, f32, &[i32])]) -> [f64; 3] {
        let d = direction_of(c.yaw);
        let fwd = clockwise(if c.flipped { [-d[0], -d[1]] } else { d });
        let offsets = offsets_for(fwd);
        let v = c.block_pos();
        let w = self.world();
        for &(width, height, heights) in poses {
            let reach = f64::from(width.min(1.0) / 2.0);
            for &oy in heights {
                for o in offsets {
                    let t = [v[0] + o[0], v[1] + oy, v[2] + o[1]];
                    let floor = self.dismount_floor_height(t);
                    if !floor.is_finite() || floor >= 1.0 {
                        continue;
                    }
                    let loc = [f64::from(t[0]) + 0.5, f64::from(t[1]) + floor, f64::from(t[2]) + 0.5];
                    let b = rc_physics::Aabb {
                        min: [loc[0] - reach, loc[1], loc[2] - reach],
                        max: [loc[0] + reach, loc[1] + f64::from(height), loc[2] + reach],
                    };
                    if rc_physics::is_free(&w, &b) {
                        return loc;
                    }
                }
            }
        }
        [c.pos[0], c.bbox().max[1], c.pos[2]]
    }

    /// Minecart removed, or placed in a portal block: the dismount does not look for a cell
    /// around it.
    fn cart_gone_or_portal(&self, c: &Cart) -> bool {
        c.removed || self.btag_id(c.block_pos(), crate::tag_bits::TagId::Portals)
    }

    /// Dismount in that case: passenger raised to the height of the minecart, then nearest free centre
    /// in their own w × h × w box (boxes of more than 4 blocks: kept as is).
    fn cart_dismount_free(&self, c: &Cart, at: [f64; 3], w: f64, h: f64) -> [f64; 3] {
        let t = [at[0], at[1].max(c.pos[1]), at[2]];
        if w > 4.0 || h > 4.0 {
            return t;
        }
        let center = [t[0], t[1] + h / 2.0, t[2]];
        let allowed = (
            [center[0] - w / 2.0, center[1] - h / 2.0, center[2] - w / 2.0],
            [center[0] + w / 2.0, center[1] + h / 2.0, center[2] + w / 2.0],
        );
        self.free_position(allowed, center, w, h).map_or(t, |p| [p[0], p[1] - h / 2.0, p[2]])
    }

    /// Ground height of the block over non-climbable shapes (ladder, vine, open trapdoor
    /// ignored): top of the shape, otherwise that of the one below − 1, otherwise −∞.
    fn dismount_floor_height(&self, p: [i32; 3]) -> f64 {
        let top = |q: [i32; 3]| -> Option<f64> {
            let st = self.st(q);
            let open_trapdoor = self.bname(q).ends_with("_trapdoor") && self.v().get(st, "open") == Some("true");
            if self.phys.climbable(st) || open_trapdoor {
                return None;
            }
            self.collision_top(q)
        };
        if let Some(t) = top(p) {
            return t;
        }
        match top([p[0], p[1] - 1, p[2]]) {
            Some(b) if b >= 1.0 => b - 1.0,
            _ => f64::NEG_INFINITY,
        }
    }

    // ---------- Placement ----------

    /// Placement of a minecart by its item on the block `clicked`: a rail
    /// is required; centre of the rail, + 0.5 on a slope. Returns the identifier of the placed minecart
    /// (the caller removes the item, except in creative), `None` if refused.
    // Called by the use of an item on a block (`item_use_on`, blocks/growth.rs).
    pub(crate) fn place_minecart(&mut self, item: &Stack, clicked: [i32; 3]) -> Option<i32> {
        let kind = match item.id.as_str() {
            "minecraft:minecart" => CKind::Rideable,
            "minecraft:chest_minecart" => CKind::Chest,
            "minecraft:furnace_minecart" => CKind::Furnace,
            "minecraft:tnt_minecart" => CKind::Tnt,
            "minecraft:hopper_minecart" => CKind::Hopper,
            "minecraft:command_block_minecart" => CKind::Command,
            _ => return None,
        };
        if !self.btag_id(clicked, crate::tag_bits::TagId::Rails) {
            return None;
        }
        let slope = self.rail_shape_at(clicked).is_some_and(|s| s.is_slope());
        let offset = if slope { 0.5 } else { 0.0 };
        let pos = [f64::from(clicked[0]) + 0.5, f64::from(clicked[1]) + 0.0625 + offset, f64::from(clicked[2]) + 0.5];
        self.next_entity += 1;
        let mut c = Cart::new(kind, self.next_entity, pos);
        // Stack components: custom name, entity data.
        if let Some(n) = crate::items::component(item, "minecraft:custom_name").cloned() {
            crate::nbtio::set(&mut c.nbt, "CustomName", n);
        }
        if let Some(d) = crate::items::component(item, "minecraft:entity_data").cloned() {
            let mut full = self.cart_to_nbt(&c);
            if let Nbt::Compound(f) = &d {
                for (k, v) in f {
                    if k != "id" {
                        crate::nbtio::set(&mut full, k, v.clone());
                    }
                }
            }
            let (id, first) = (c.id, c.first_tick);
            c.load(self, &full);
            c.id = id;
            c.first_tick = first;
        }
        let id = c.id;
        let below = self.st([clicked[0], clicked[1] - 1, clicked[2]]);
        self.cart_add(c);
        let center = clicked.map(|v| f64::from(v) + 0.5);
        self.game_event_src("minecraft:entity_place", center, Some(Src::Living(Ent::Player)), Some(below));
        Some(id)
    }
}
