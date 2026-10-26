//! Dispenser behaviours by item: each known item
//! follows its rule, the others are thrown out in front (common fallback). The "optional" rules
//! play click 1000 when they succeed, 1001 otherwise; all play smoke 2000
//! (direction). Each rule returns the stack left in the slot.
//!
//! Not simulated server side (fallback: item thrown): boats, minecarts, armor stand, chest
//! on an equine, brush on an armadillo, sulfur cube; equipment put on a mob (the
//! player, however, is equipped).

use rc_proto::nbt::Nbt;
use serde_json::Value;

use super::{Dir, Kind, P, flag, rel};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Stack minus one item (empty: none).
pub(super) fn shrink(mut s: Stack) -> Option<Stack> {
    s.count -= 1;
    (s.count > 0).then_some(s)
}

/// Full buckets emptied in front of the dispenser.
fn filled_bucket(id: &str) -> bool {
    matches!(id, "minecraft:water_bucket" | "minecraft:lava_bucket" | "minecraft:powder_snow_bucket")
        || crate::mobs::sealife::fish::bucket_kind(id).is_some()
}

impl Server {
    /// Dispenses the stack `stack` from slot `i` of the dispenser at `p` facing `f`; returns
    /// the stack left in the slot.
    pub(crate) fn dispense_item(&mut self, p: P, f: Dir, i: usize, stack: Stack) -> Option<Stack> {
        let front = rel(p, f);
        let id = stack.id.clone();
        match id.as_str() {
            pid if crate::projectiles::dispense::dispensable(pid) => {
                let ev = self.dispense_projectile(p, f.step(), &stack);
                self.dispense_sounds(p, f, ev);
                shrink(stack)
            }
            b if filled_bucket(b) => self.dispense_filled_bucket(p, f, i, stack),
            "minecraft:bucket" => self.dispense_empty_bucket(p, f, i, stack),
            "minecraft:flint_and_steel" => {
                let (rest, ok) = self.dispense_flint(front, f, stack);
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                rest
            }
            "minecraft:bone_meal" => {
                let ok = self.grow_with_bone_meal(front);
                if ok {
                    self.level_event(1505, front, 15);
                }
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                if ok { shrink(stack) } else { Some(stack) }
            }
            "minecraft:tnt" => {
                let ok = self.rule("minecraft:tnt_explodes", true);
                if ok {
                    self.tnt_prime(front);
                    let at = [f64::from(front[0]) + 0.5, f64::from(front[1]), f64::from(front[2]) + 0.5];
                    self.world_sound("minecraft:entity.tnt.primed", 4, at, 1.0, 1.0);
                    self.game_event_block("minecraft:entity_place", front, None, None);
                }
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                if ok { shrink(stack) } else { Some(stack) }
            }
            "minecraft:wither_skeleton_skull" | "minecraft:carved_pumpkin" => {
                let skull = id == "minecraft:wither_skeleton_skull";
                let placed = self.v().is_air(self.get(front))
                    && if skull { self.wither_can_spawn_mob(front) } else { self.can_spawn_golem(front) };
                let ok = if placed {
                    if skull {
                        self.wither_dispense_skull(front, f.step());
                    } else {
                        let st = self.default_state("minecraft:carved_pumpkin");
                        self.set_update(front, st);
                        self.game_event_block("minecraft:block_place", front, None, None);
                    }
                    true
                } else {
                    self.dispense_equip(front, &stack)
                };
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                if ok { shrink(stack) } else { Some(stack) }
            }
            s if s.ends_with("shulker_box") => {
                let ok = self.dispense_shulker_box(front, f, &stack);
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                if ok { shrink(stack) } else { Some(stack) }
            }
            "minecraft:glass_bottle" => self.dispense_glass_bottle(p, f, i, stack),
            "minecraft:glowstone" => {
                let s = self.get(front);
                if self.state_name(s) != "minecraft:respawn_anchor" {
                    return self.dispense_default(p, f, stack);
                }
                let charges = self.v().get_int(s, "charges");
                let ok = charges != 4;
                if ok {
                    // The charge of a respawn anchor without a player.
                    let ns = self.v().set_int(s, "charges", charges + 1);
                    self.set_block(front, ns, flag::ALL);
                    self.game_event_block("minecraft:block_change", front, None, Some(ns));
                    self.block_sound(front, "minecraft:block.respawn_anchor.charge", 1.0, 1.0);
                }
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                if ok { shrink(stack) } else { Some(stack) }
            }
            "minecraft:honeycomb" => {
                let s = self.get(front);
                let waxed = self.waxed_of(self.state_name(s)).and_then(|w| self.state_with(&w, &self.props_of(s)));
                let Some(ns) = waxed else { return self.dispense_default(p, f, stack) };
                self.set_update(front, ns);
                self.level_event(3003, front, 0);
                self.block_sound(front, "minecraft:item.honeycomb.wax_on", 1.0, 1.0);
                self.dispense_sounds(p, f, 1000);
                shrink(stack)
            }
            "minecraft:shears" => {
                let ok = self.dispense_shears(front);
                let rest = if ok { self.hurt_dispensed(stack) } else { Some(stack) };
                self.dispense_sounds(p, f, if ok { 1000 } else { 1001 });
                rest
            }
            "minecraft:potion" => self.dispense_water_potion(p, f, i, stack),
            _ if self.stack_has(&stack, "minecraft:equippable") => {
                if self.dispense_equip(front, &stack) {
                    self.dispense_sounds(p, f, 1000);
                    shrink(stack)
                } else {
                    self.dispense_default(p, f, stack)
                }
            }
            _ if self.stack_has(&stack, "minecraft:entity_data") && id.ends_with("_spawn_egg") => {
                let rest = if self.dispense_spawn_egg(p, f, &stack) { shrink(stack) } else { Some(stack) };
                self.dispense_sounds(p, f, 1000);
                rest
            }
            _ => self.dispense_default(p, f, stack),
        }
    }

    /// Sound (`ev`) then oriented smoke (2000).
    fn dispense_sounds(&mut self, p: P, f: Dir, ev: i32) {
        self.level_event(ev, p, 0);
        self.level_event(2000, p, f as i32);
    }

    /// Throwing one item in front of the dispenser:
    /// item on the ground, click 1000 and smoke 2000. Returns the stack minus that item.
    pub(crate) fn dispense_default(&mut self, p: P, f: Dir, stack: Stack) -> Option<Stack> {
        let one = Stack { count: 1, ..stack.clone() };
        self.dispense_spawn_item(p, f, one);
        self.dispense_sounds(p, f, 1000);
        shrink(stack)
    }

    /// The output of an item (precision 6): item on the ground 0.7 block from the centre towards `f`, lowered by 0.125
    /// on the vertical axis and by 0.15625 otherwise; velocity drawn around the direction (0.2 to 0.3)
    /// and upwards (0.2), spread ±0.0172275 × 6 as a triangle. World random: first the
    /// two draws of the default velocity of the item's constructor, replaced afterwards.
    fn dispense_spawn_item(&mut self, p: P, f: Dir, it: Stack) {
        let d = f.step().map(f64::from);
        let mut pos: [f64; 3] = std::array::from_fn(|k| f64::from(p[k]) + 0.5 + 0.7 * d[k]);
        pos[1] -= if f.axis() == 1 { 0.125 } else { 0.15625 };
        let r = &mut self.bs.rng;
        let _ = (r.next_double(), r.next_double());
        let pow = r.next_double() * 0.1 + 0.2;
        let spread = 0.017_227_5 * 6.0;
        let mut tri = |mean: f64| mean + spread * (r.next_double() - r.next_double());
        let v = [tri(d[0] * pow), tri(0.2), tri(d[2] * pow)];
        self.spawn_item(pos, v, it, 0);
    }

    /// One item consumed; the remainder (bucket, bottle) takes its place
    /// if the stack is empty, otherwise goes into the dispenser (empty slot or identical stack),
    /// otherwise it is thrown in front (1000, 2000).
    fn consume_with_remainder(&mut self, p: P, f: Dir, i: usize, stack: Stack, remainder: Stack) -> Option<Stack> {
        let Some(rest) = shrink(stack) else { return Some(remainder) };
        let mut slots = self.slots(p);
        if let Some(s) = slots.get_mut(i) {
            *s = Some(rest.to_nbt(None));
        }
        let max = self.life.items.max_stack(&remainder);
        let mut left = remainder;
        for slot in slots.iter_mut() {
            if left.count <= 0 {
                break;
            }
            let cur = slot.as_ref().and_then(Stack::from_nbt);
            let room = match &cur {
                None => max,
                Some(c) if crate::items::same(c, &left) => max - c.count,
                Some(_) => 0,
            };
            let n = left.count.min(room);
            if n > 0 {
                let mut put = cur.unwrap_or_else(|| Stack { count: 0, ..left.clone() });
                put.count += n;
                left.count -= n;
                *slot = Some(put.to_nbt(None));
            }
        }
        // The dispenser slot keeps the partly used stack (rewritten by the caller).
        self.store_slots(p, &slots);
        if left.count > 0 {
            self.dispense_spawn_item(p, f, left);
            self.dispense_sounds(p, f, 1000);
        }
        Some(rest)
    }

    /// Full bucket emptied in front (then the bucket's mob); refusal: thrown by
    /// the nested default rule, so click and smoke are played twice (26.3).
    fn dispense_filled_bucket(&mut self, p: P, f: Dir, i: usize, stack: Stack) -> Option<Stack> {
        let front = rel(p, f);
        let done = match stack.id.as_str() {
            "minecraft:water_bucket" => self.bucket_empty_at(front, true, None, None),
            "minecraft:lava_bucket" => self.bucket_empty_at(front, false, None, None),
            "minecraft:powder_snow_bucket" => {
                let ok = self.in_build_height(front[1]) && self.v().is_air(self.get(front));
                if ok {
                    let snow = self.default_state("minecraft:powder_snow");
                    self.set_update(front, snow);
                    self.game_event_block("minecraft:fluid_place", front, None, None);
                    self.block_sound(front, "minecraft:block.powder_snow.place", 1.0, 1.0);
                }
                ok
            }
            id => {
                let Some(kind) = crate::mobs::sealife::fish::bucket_kind(id) else { return Some(stack) };
                let sound = crate::mobs::brainy::bucket_empty_sound(kind).unwrap_or("minecraft:item.bucket.empty_fish");
                let ok = self.empty_water(front, None, sound);
                if ok {
                    self.game_event_block("minecraft:fluid_place", front, None, None);
                    self.spawn_from_bucket(kind, &stack, front);
                    self.game_event_block("minecraft:entity_place", front, None, None);
                }
                ok
            }
        };
        if !done {
            let rest = self.dispense_default(p, f, stack);
            self.dispense_sounds(p, f, 1000);
            return rest;
        }
        let rest = self.consume_with_remainder(p, f, i, stack, stack_of("minecraft:bucket"));
        self.dispense_sounds(p, f, 1000);
        rest
    }

    /// Empty bucket: filling from the block in front (source, waterlogged block, powder snow,
    /// bubble column), `fluid_pickup` event; otherwise thrown.
    fn dispense_empty_bucket(&mut self, p: P, f: Dir, i: usize, stack: Stack) -> Option<Stack> {
        let front = rel(p, f);
        let Some(filled) = self.bucket_pickup_at(front) else { return self.dispense_default(p, f, stack) };
        self.game_event_block("minecraft:fluid_pickup", front, None, None);
        let rest = self.consume_with_remainder(p, f, i, stack, stack_of(filled));
        self.dispense_sounds(p, f, 1000);
        rest
    }

    /// Flint and steel: fire in the air, otherwise campfire or
    /// candle lit, otherwise primed TNT; one point of wear when it was used.
    fn dispense_flint(&mut self, front: P, f: Dir, stack: Stack) -> (Option<Stack>, bool) {
        let s = self.get(front);
        let v = self.v();
        let name = self.state_name(s).to_owned();
        let lightable = v.has(s, "lit")
            && !v.get_bool(s, "lit")
            && !v.get_bool(s, "waterlogged")
            && (name.ends_with("campfire") || name.ends_with("candle") || name.ends_with("candle_cake"));
        let ok = if self.fire_can_be_placed(front, f) {
            let fire = self.fire_state_for(front);
            self.set_update(front, fire);
            self.game_event_block("minecraft:block_place", front, None, None);
            true
        } else if lightable {
            let lit = self.v().set_bool(s, "lit", true);
            self.set_update(front, lit);
            self.game_event_block("minecraft:block_change", front, None, None);
            true
        } else if v.kind(s) == Kind::Tnt {
            let primed = self.tnt_prime_block(front, None);
            if primed {
                self.remove_block(front, false);
            }
            primed
        } else {
            // Nothing to light: success with no effect (the game does not put the failure back here).
            true
        };
        if ok { (self.hurt_dispensed(stack), true) } else { (Some(stack), false) }
    }

    /// One point of wear without a holder: durability drawn on the world random;
    /// worn down to the end, the item disappears.
    fn hurt_dispensed(&mut self, mut s: Stack) -> Option<Stack> {
        let items = &self.life.items;
        if !items.damageable(&s) {
            return Some(s);
        }
        let unbreaking = items.enchantment(&s, "minecraft:unbreaking");
        if unbreaking > 0 && self.life.rng.next_float() < unbreaking as f32 / (unbreaking as f32 + 1.0) {
            return Some(s);
        }
        let (max, dmg) = (items.max_damage(&s), items.damage(&s) + 1);
        if dmg >= max {
            return shrink(s);
        }
        crate::items::set_component(&mut s, "minecraft:damage", Nbt::Int(dmg));
        Some(s)
    }

    /// Shulker box placed in front: facing `f` if the
    /// cell below is empty, otherwise upwards; components copied into the block entity.
    fn dispense_shulker_box(&mut self, front: P, f: Dir, it: &Stack) -> bool {
        let block = self.tb().block_id(&it.id);
        if block == u16::MAX || !self.in_build_height(front[1]) {
            return false;
        }
        let cur = self.get(front);
        if !(self.v().is_air(cur) || self.v().replaceable(cur)) || self.v().block(cur) == block {
            return false;
        }
        let face = if self.v().is_air(self.get(super::below(front))) { f } else { Dir::Up };
        let st = self.v().set(self.v().default_state(block), "facing", face.name());
        if !self.can_place_at(st, front) || self.obstructed(st, front) {
            return false;
        }
        if !self.set_block(front, st, flag::ALL | 8) {
            return false;
        }
        if let Some(comps) = &it.components
            && let Some(be) = self.be_get_mut(front)
        {
            crate::loot::be::apply(be, comps, false);
            self.send_block_entity(front);
        }
        let placed = self.get(front);
        if let Some(t) = self.data.states.get(placed as usize).and_then(|i| self.data.sound_types.get(usize::from(i.sound))).cloned() {
            self.block_sound(front, &t.place, (t.volume + 1.0) / 2.0, t.pitch * 0.8);
        }
        self.game_event_block("minecraft:block_place", front, None, Some(placed));
        true
    }

    /// Empty bottle: honey from a full beehive (bees released), otherwise water; otherwise thrown with
    /// the failure click (the rule stays "failed" while throwing, like the game).
    fn dispense_glass_bottle(&mut self, p: P, f: Dir, i: usize, stack: Stack) -> Option<Stack> {
        let front = rel(p, f);
        let filled = if self.hive_honey(front).is_some_and(|h| h >= 5) {
            self.hive_release_by_dispenser(front);
            Some(stack_of("minecraft:honey_bottle"))
        } else if self.fluid_at(front).kind == 1 {
            Some(water_bottle())
        } else {
            None
        };
        let Some(filled) = filled else {
            let one = Stack { count: 1, ..stack.clone() };
            self.dispense_spawn_item(p, f, one);
            self.dispense_sounds(p, f, 1001);
            return shrink(stack);
        };
        self.game_event_block("minecraft:fluid_pickup", p, None, None);
        let rest = self.consume_with_remainder(p, f, i, stack, filled);
        self.dispense_sounds(p, f, 1000);
        rest
    }

    /// Water bottle on a block that turns into mud: splashes, sound, mud placed; otherwise thrown
    /// (nested default rule: click and smoke doubled, like the full bucket).
    fn dispense_water_potion(&mut self, p: P, f: Dir, i: usize, stack: Stack) -> Option<Stack> {
        let front = rel(p, f);
        let water = crate::items::component(&stack, "minecraft:potion_contents")
            .and_then(|c| c.get("potion"))
            .and_then(Nbt::as_str)
            == Some("minecraft:water");
        let b = self.v().block(self.get(front));
        if !water || !self.tag_id(crate::tag_bits::TagId::ConvertibleToMud, b) {
            let rest = self.dispense_default(p, f, stack);
            self.dispense_sounds(p, f, 1000);
            return rest;
        }
        for _ in 0..5 {
            let r = &mut self.bs.rng;
            let (dx, dz) = (r.next_double(), r.next_double());
            let at = [f64::from(p[0]) + dx, f64::from(p[1] + 1), f64::from(p[2]) + dz];
            self.particles("minecraft:splash", at, [0.0; 3], 1.0, 1);
        }
        self.block_sound(p, "minecraft:item.bottle.empty", 1.0, 1.0);
        self.game_event_block("minecraft:fluid_place", p, None, None);
        let mud = self.default_state("minecraft:mud");
        self.set_update(front, mud);
        let rest = self.consume_with_remainder(p, f, i, stack, stack_of("minecraft:glass_bottle"));
        self.dispense_sounds(p, f, 1000);
        rest
    }

    /// Shears: full beehive harvested (honeycombs, bees released), otherwise sheep
    /// sheared in front. True if they were used.
    fn dispense_shears(&mut self, front: P) -> bool {
        if self.hive_honey(front).is_some_and(|h| h >= 5) {
            self.block_sound(front, "minecraft:block.beehive.shear", 1.0, 1.0);
            self.hive_drop_honeycomb(front);
            self.hive_release_by_dispenser(front);
            self.game_event_block("minecraft:shear", front, None, None);
            return true;
        }
        let cell = cell_box(front);
        let sheep: Option<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .find(|m| {
                m.kind == crate::mobs::species::Kind::Sheep
                    && m.alive()
                    && !m.sheared
                    && !m.is_baby()
                    && m.bbox().intersects(&cell)
            })
            .map(|m| m.id);
        let Some(id) = sheep else { return false };
        let Some(i) = self.mob_index(id) else { return false };
        let Some(mut m) = self.mobs.list[i].take() else { return false };
        self.sheep_shear(&mut m, 4);
        self.mobs.list[i] = Some(m);
        self.game_event_block("minecraft:shear", front, None, None);
        true
    }

    /// Spawn egg: mob placed in front (shifted upwards
    /// except when shooting upwards), `entity_place` event at the dispenser. True if it spawned.
    fn dispense_spawn_egg(&mut self, p: P, f: Dir, egg: &Stack) -> bool {
        let front = rel(p, f);
        let ty = crate::items::component(egg, "minecraft:entity_data")
            .and_then(|d| d.get("id"))
            .and_then(Nbt::as_str)
            .map(str::to_owned)
            .or_else(|| self.life.items.defaults(&egg.id)["minecraft:entity_data"]["id"].as_str().map(str::to_owned));
        let Some(kind) = ty.as_deref().and_then(crate::mobs::species::Kind::from_name) else { return false };
        let y_off = if f == Dir::Up { 0.0 } else { self.spawn_y_offset(front, false, kind) };
        let pos = [f64::from(front[0]) + 0.5, f64::from(front[1]) + y_off, f64::from(front[2]) + 0.5];
        let yaw = crate::mobs::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let mut m = self.new_mob(kind, pos, yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let mut g = crate::mobs::spawn::Group::default();
        self.finalize_spawn(&mut m, crate::mobs::spawn::Reason::SpawnEgg, &mut g);
        if let Some(n) = crate::items::component(egg, "minecraft:custom_name") {
            m.custom_name = Some(n.clone());
        }
        if let Some(a) = kind.info().ambient {
            let a = a.to_owned();
            self.make_sound(&mut m, &a);
        }
        self.add_mob(m);
        self.game_event_block("minecraft:entity_place", p, None, None);
        true
    }

    /// Equipment put on the player in front: item equippable by
    /// dispenser, empty slot, living player who is not a spectator.
    fn dispense_equip(&mut self, front: P, it: &Stack) -> bool {
        if self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        let Some(eq) = self.comp_json(it, "minecraft:equippable") else { return false };
        if eq.get("dispensable").and_then(Value::as_bool) == Some(false) {
            return false;
        }
        let target = match eq["slot"].as_str().unwrap_or("") {
            "head" => 5,
            "chest" => 6,
            "legs" => 7,
            "feet" => 8,
            "offhand" => 45,
            "mainhand" => 36 + self.held as usize,
            _ => return false,
        };
        if let Some(allowed) = eq.get("allowed_entities")
            && !self.entity_set_has(allowed, "minecraft:player")
        {
            return false;
        }
        let touches = self.ent_bbox(Ent::Player).is_some_and(|b| b.intersects(&cell_box(front)));
        if !touches || self.slots[target].is_some() {
            return false;
        }
        self.set_equipment(target, Some(Stack { count: 1, ..it.clone() }), None);
        self.broadcast_menus();
        true
    }
}

fn stack_of(id: &str) -> Stack {
    Stack { id: id.to_owned(), count: 1, components: None }
}

/// Water bottle.
fn water_bottle() -> Stack {
    Stack {
        id: "minecraft:potion".into(),
        count: 1,
        components: Some(Nbt::Compound(vec![(
            "minecraft:potion_contents".into(),
            Nbt::Compound(vec![("potion".into(), Nbt::String("minecraft:water".into()))]),
        )])),
    }
}

/// Unit box of cell `p` (the whole cell).
fn cell_box(p: P) -> rc_physics::Aabb {
    let c = p.map(f64::from);
    rc_physics::Aabb { min: c, max: [c[0] + 1.0, c[1] + 1.0, c[2] + 1.0] }
}

#[cfg(test)]
#[path = "dispense_tests.rs"]
mod tests;
