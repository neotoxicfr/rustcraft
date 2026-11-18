//! Placing a boat or raft: by its item, aimed with the item ray (fluids included), and by a
//! dispenser just outside its face.

use rc_physics::Aabb;
use rc_physics::ray::Fluids;
use rc_proto::nbt::Nbt;

use crate::minecart::{CKind, Cart};
use crate::mobs::mob::Ent;
use crate::mobs::recent::vibration::Src;
use crate::player::Stack;
use crate::server::Server;

/// The vehicle kind of a boat or raft item.
pub(crate) fn boat_kind_of_item(id: &str) -> Option<CKind> {
    super::BoatType::from_name(id).map(CKind::Boat)
}

impl Server {
    /// `use_item` of the boat or raft held in `slot`: placed where the player's ray (fluids
    /// included) meets a block, facing the player's yaw, unless the player's eyes are inside an
    /// entity one can aim at or the boat has no room. The item goes (not in creative), the
    /// `entity_place` event and the "used" statistic follow.
    pub(crate) fn boat_item_use(&mut self, slot: usize, item: &Stack, kind: CKind) {
        let hit = self.item_ray(Fluids::Any);
        if hit.miss || self.eyes_inside_pickable_entity() {
            return;
        }
        self.next_entity += 1;
        let mut c = Cart::new(kind, self.next_entity, hit.at);
        self.apply_stack_to_vehicle(&mut c, item);
        (c.yaw, c.yaw_o) = (self.yaw, self.yaw);
        if !self.vehicle_has_room(&c) {
            return;
        }
        self.cart_add(c);
        self.game_event_src("minecraft:entity_place", hit.at, Some(Src::Living(Ent::Player)), None);
        if self.game_mode != 1
            && let Some(s) = self.slots[slot].as_mut()
        {
            s.count -= 1;
            if s.count <= 0 {
                self.slots[slot] = None;
            }
        }
        self.broadcast_menus();
        self.award_stat("minecraft:used", &item.id, 1);
    }

    /// A dispenser's boat or raft placed at `pos` facing `yaw`, with the stack's components (no
    /// user: no operator).
    pub(crate) fn place_boat_from_dispenser(&mut self, kind: CKind, pos: [f64; 3], yaw: f32, stack: &Stack) {
        self.next_entity += 1;
        let mut c = Cart::new(kind, self.next_entity, pos);
        self.apply_stack_to_vehicle(&mut c, stack);
        (c.yaw, c.yaw_o) = (yaw, yaw);
        self.cart_add(c);
    }

    /// What a stack hands to the vehicle it places: its custom name and custom data, then its
    /// entity data read over the vehicle's own save (identifier and "first tick" flag kept).
    pub(crate) fn apply_stack_to_vehicle(&self, c: &mut Cart, stack: &Stack) {
        if let Some(n) = crate::items::component(stack, "minecraft:custom_name").cloned() {
            crate::nbtio::set(&mut c.nbt, "CustomName", n);
        }
        if let Some(d) = crate::items::component(stack, "minecraft:custom_data").cloned() {
            crate::nbtio::set(&mut c.nbt, "data", d);
        }
        let Some(Nbt::Compound(fields)) = crate::items::component(stack, "minecraft:entity_data") else { return };
        let mut full = self.cart_to_nbt(c);
        for (k, v) in fields {
            if k != "id" {
                crate::nbtio::set(&mut full, k, v.clone());
            }
        }
        let (id, first) = (c.id, c.first_tick);
        c.load(self, &full);
        (c.id, c.first_tick) = (id, first);
    }

    /// The player's eyes are inside an entity one can aim at (a mob, a vehicle): no boat is
    /// placed from there.
    fn eyes_inside_pickable_entity(&self) -> bool {
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let inside = |b: Aabb| (0..3).all(|k| eye[k] >= b.min[k] && eye[k] < b.max[k]);
        self.mobs.list.iter().any(|m| !m.removed && inside(m.bbox()))
            || self.carts.list.iter().flatten().any(|c| !c.removed && inside(c.bbox()))
    }

    /// Room for a new vehicle: no block nor solid entity, and none of the entities a vehicle
    /// collides with (mobs one can push, the player, the other vehicles).
    fn vehicle_has_room(&self, c: &Cart) -> bool {
        let b = c.bbox();
        if !self.room_free(Self::room_for_object(c.id, c.pos), &b) {
            return false;
        }
        let touches = |o: Aabb| o.intersects(&b);
        let mob = self.mobs.list.iter().any(|m| m.alive() && self.mob_pushable(m) && touches(m.bbox()));
        let player = self.game_mode != 3 && !self.life.vit.dead && touches(self.player_aabb());
        let vehicle = self.carts.list.iter().flatten().any(|o| !o.removed && touches(o.bbox()));
        !(mob || player || vehicle)
    }
}
