//! Armor stand: placement by the item, equipment swapped on right click, broken in two hits.

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::{DKind, Use, base_fields, flag, kind_of, named, put_after};
use crate::blocks::{Dir, P, rel};
use crate::mobs::mob::Ent;
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Equipment slots: NBT name, network index, filter bit, hand.
const SLOTS: [(&str, u8, u32, bool); 6] = [
    ("mainhand", 0, 0, true),
    ("offhand", 1, 5, true),
    ("feet", 2, 1, false),
    ("legs", 3, 2, false),
    ("chest", 4, 3, false),
    ("head", 5, 4, false),
];
const MAINHAND: usize = 0;
const OFFHAND: usize = 1;
const FEET: usize = 2;
const LEGS: usize = 3;
const CHEST: usize = 4;
const HEAD: usize = 5;

/// Default poses (head, body, left arm, right arm, left leg, right leg).
pub(super) const POSES: [(&str, [f32; 3]); 6] = [
    ("Head", [0.0, 0.0, 0.0]),
    ("Body", [0.0, 0.0, 0.0]),
    ("LeftArm", [-10.0, 0.0, -10.0]),
    ("RightArm", [-15.0, 0.0, 10.0]),
    ("LeftLeg", [-1.0, 0.0, -1.0]),
    ("RightLeg", [1.0, 0.0, 1.0]),
];

fn small(e: &Nbt) -> bool {
    flag(e, "Small")
}

fn marker(e: &Nbt) -> bool {
    flag(e, "Marker")
}

fn show_arms(e: &Nbt) -> bool {
    flag(e, "ShowArms")
}

fn disabled_slots(e: &Nbt) -> i64 {
    int(e, "DisabledSlots").unwrap_or(0)
}

/// Width, height and eye height of an armor stand (`stand`) or a mannequin kept as compound `e`,
/// in single precision like the game: a marker stand has none (a fixed size, never scaled); a
/// small stand is half as large with its eyes at 0.9875; a mannequin takes the size of its pose,
/// asleep a fixed 0.2. The others are multiplied by `scale` (the attribute).
pub(crate) fn living_dims(e: &Nbt, stand: bool, scale: f32) -> (f32, f32, f32) {
    let (w, h, eye) = if stand {
        if marker(e) {
            return (0.0, 0.0, 0.0);
        } else if small(e) {
            (0.5 * 0.5, 1.975 * 0.5, 0.9875)
        } else {
            (0.5, 1.975, 1.7775)
        }
    } else {
        match e.get("pose").and_then(Nbt::as_str) {
            Some("sleeping") => return (0.2, 0.2, 0.2),
            Some("crouching") => (0.6, 1.5, 1.27),
            Some("swimming" | "fall_flying") => (0.6, 0.6, 0.4),
            _ => (0.6, 1.8, 1.62),
        }
    };
    if scale == 1.0 { (w, h, eye) } else { (w * scale, h * scale, eye * scale) }
}

/// Can attribute `name` of living decoration `e` stray from its type's default? Only through a
/// saved entry of its own, or a worn stack carrying attribute modifiers or enchantments of its own
/// (no item moves the scale or raises the waypoint range by default): the usual case is answered
/// without building anything.
pub(crate) fn attribute_may_differ(e: &Nbt, name: &str) -> bool {
    attribute_may_differ_with(e, name, rc_data::opt_switch!("RUSTCRAFT_OPT_LIVING_DECO_ATTRIBUTE_ONE_WALK"))
}

/// [`attribute_may_differ`] by one walk of the compound's fields (`one_walk`: asked every tick of
/// every living decoration, a lookup by name walks the fields anyway, so one walk finds both
/// places instead of two lookups), or by the two lookups
/// (`RUSTCRAFT_OPT_LIVING_DECO_ATTRIBUTE_ONE_WALK=0`).
pub(crate) fn attribute_may_differ_with(e: &Nbt, name: &str, one_walk: bool) -> bool {
    let saved = |attributes: Option<&Nbt>| {
        nbtio::list(attributes).iter().any(|a| a.get("id").and_then(Nbt::as_str) == Some(name))
    };
    if one_walk {
        let Nbt::Compound(fields) = e else { return false };
        return fields.iter().any(|(key, value)| match key.as_str() {
            "attributes" => saved(Some(value)),
            "equipment" => worn_stack_with_own_modifiers(value),
            _ => false,
        });
    }
    saved(e.get("attributes")) || e.get("equipment").is_some_and(worn_stack_with_own_modifiers)
}

/// A worn stack carries attribute modifiers or enchantments of its own (its components).
fn worn_stack_with_own_modifiers(equipment: &Nbt) -> bool {
    let Nbt::Compound(slots) = equipment else { return false };
    slots.iter().any(|(_, s)| {
        s.get("components").is_some_and(|c| {
            c.get("minecraft:attribute_modifiers").is_some() || c.get("minecraft:enchantments").is_some()
        })
    })
}

impl Server {
    /// The `scale` attribute of a living decoration: 1 when nothing can move it
    /// ([`attribute_may_differ`]), the usual case read every tick.
    pub(crate) fn living_scale(&self, e: &Nbt, ty: &str) -> f32 {
        if !attribute_may_differ(e, "minecraft:scale") {
            return 1.0;
        }
        self.nbt_attribute(e, ty, "minecraft:scale").map_or(1.0, |v| v as f32)
    }

    /// [`living_dims`] of a living decoration with its own scale, widened: (width, height, eyes).
    pub(crate) fn living_size(&self, e: &Nbt, stand: bool) -> (f64, f64, f64) {
        let ty = if stand { "minecraft:armor_stand" } else { "minecraft:mannequin" };
        let (w, h, eye) = living_dims(e, stand, self.living_scale(e, ty));
        (f64::from(w), f64::from(h), f64::from(eye))
    }
}

/// Angles of a part (`Rotations`: each angle modulo 360, non-finite read as 0).
pub(super) fn rotations(v: Option<&Nbt>, d: [f32; 3]) -> [f32; 3] {
    match nbtio::f32s::<3>(v) {
        Some(r) => r.map(|a| if a.is_finite() { a % 360.0 } else { 0.0 }),
        None => d,
    }
}

/// Network data: flags (small, arms, no base plate, marker), non-default poses.
pub(super) fn meta(e: &Nbt, values: &mut Vec<(u8, Meta)>) {
    let mut f = 0i8;
    for (k, b) in [("Small", 1), ("ShowArms", 4), ("NoBasePlate", 8), ("Marker", 16)] {
        if flag(e, k) {
            f |= b;
        }
    }
    if f != 0 {
        values.push((15, Meta::Byte(f)));
    }
    let pose = e.get("Pose");
    for (i, (k, d)) in POSES.iter().enumerate() {
        let r = rotations(pose.and_then(|p| p.get(k)), *d);
        if r != *d {
            values.push((16 + i as u8, Meta::Rotations(r)));
        }
    }
}

fn slot_item(e: &Nbt, i: usize) -> Option<Stack> {
    e.get("equipment").and_then(|q| q.get(SLOTS[i].0)).and_then(Stack::from_nbt)
}

/// Slot forbidden by `DisabledSlots`, or hand without arms.
pub(crate) fn is_disabled(e: &Nbt, i: usize) -> bool {
    disabled_slots(e) & (1 << SLOTS[i].2) != 0 || (SLOTS[i].3 && !show_arms(e))
}

/// Slot targeted by the click from the relative height of the point touched.
fn clicked_slot(e: &Nbt, at_y: f64) -> usize {
    let sm = small(e);
    let y = at_y / if sm { 0.5 } else { 1.0 };
    let has = |i| slot_item(e, i).is_some();
    if y >= 0.1 && y < 0.1 + if sm { 0.8 } else { 0.45 } && has(FEET) {
        FEET
    } else if y >= 0.9 + if sm { 0.3 } else { 0.0 } && y < 0.9 + if sm { 1.0 } else { 0.7 } && has(CHEST) {
        CHEST
    } else if y >= 0.4 && y < 0.4 + if sm { 1.0 } else { 0.8 } && has(LEGS) {
        LEGS
    } else if y >= 1.6 && has(HEAD) {
        HEAD
    } else if !has(MAINHAND) && has(OFFHAND) {
        OFFHAND
    } else {
        MAINHAND
    }
}

impl Server {
    /// Equipment slot of an item for the stand: that of its
    /// `equippable` component if the stand accepts it, otherwise the main hand.
    fn stand_slot_for(&self, e: &Nbt, st: Option<&Stack>) -> usize {
        let Some(st) = st else { return MAINHAND };
        let slot = self.comp_json(st, "minecraft:equippable").and_then(|v| v["slot"].as_str().map(str::to_owned));
        match slot.and_then(|s| SLOTS.iter().position(|x| x.0 == s)) {
            Some(i) if !is_disabled(e, i) => i,
            _ => MAINHAND,
        }
    }

    /// The placement of an armor stand; `None` if the item is not an armor stand.
    pub(crate) fn stand_use_on(&mut self, slot: usize, st: &Stack, clicked: P, face: Dir) -> Option<Use> {
        if st.id != "minecraft:armor_stand" {
            return None;
        }
        if face == Dir::Down {
            return Some(Use::Fail);
        }
        let pos = if self.replaceable_by(self.get(clicked), &st.id, true, face, false) {
            clicked
        } else {
            rel(clicked, face)
        };
        let (hw, h) = (f64::from(0.5f32 / 2.0), f64::from(1.975f32));
        let c = [f64::from(pos[0]) + 0.5, f64::from(pos[1]), f64::from(pos[2]) + 0.5];
        let b = Aabb { min: [c[0] - hw, c[1], c[2] - hw], max: [c[0] + hw, c[1] + h, c[2] + hw] };
        if !self.no_collision(&b) || self.any_entity_in(&b) {
            return Some(Use::Fail);
        }
        // Placed above the block then lowered by up to 2 blocks.
        let y = f64::from(pos[1]) + self.box_y_offset(pos, true, 0.5, 1.975);
        // Orientation: the player's, reversed, rounded to the eighth of a turn.
        let wrapped = wrap_degrees(self.yaw - 180.0);
        let yaw = ((wrapped + 22.5) / 45.0).floor() * 45.0;
        let u = self.deco_uuid();
        let mut f = base_fields(DKind::Stand, [c[0], y, c[2]], yaw, 0.0, u);
        f.extend(living_fields());
        f.extend([
            ("Invisible".into(), Nbt::Byte(0)),
            ("Small".into(), Nbt::Byte(0)),
            ("ShowArms".into(), Nbt::Byte(0)),
            ("DisabledSlots".into(), Nbt::Int(0)),
            ("NoBasePlate".into(), Nbt::Byte(0)),
            ("Pose".into(), Nbt::Compound(Vec::new())),
        ]);
        let mut e = Nbt::Compound(f);
        self.apply_stack_config(&mut e, st);
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.deco_spawn(e);
        self.world_sound("minecraft:entity.armor_stand.place", 4, at, 0.75, 0.8);
        self.game_event_at("minecraft:entity_place", at, Some(Ent::Player));
        self.consume_held(slot);
        Some(Use::Success)
    }

    /// Blocks and solid entities: a free-room test made for no entity (the border does not count).
    pub(super) fn no_collision(&self, b: &Aabb) -> bool {
        self.room_free(crate::solid_entities::RoomFor::NoEntity, b)
    }

    /// An entity (spectator excluded) whose box overlaps `b`.
    pub(super) fn any_entity_in(&self, b: &Aabb) -> bool {
        use crate::commands::ents::E;
        self.all_ents()
            .into_iter()
            .filter(|e| !(*e == E::Player && self.game_mode == 3))
            .any(|e| self.e_bbox(e).intersects(b))
    }

    /// Right click on a stand; true if the equipment changed.
    pub(super) fn stand_interact(&mut self, id: i32, slot: usize, at: [f64; 3]) -> bool {
        let Some(e) = self.deco_nbt(id).cloned() else { return false };
        let held = self.slots[slot].clone();
        if marker(&e) || held.as_ref().is_some_and(|s| s.id == "minecraft:name_tag") {
            return false;
        }
        let item_slot = self.stand_slot_for(&e, held.as_ref());
        match held {
            None => {
                let clicked = clicked_slot(&e, at[1]);
                let target = if is_disabled(&e, clicked) { item_slot } else { clicked };
                slot_item(&e, target).is_some() && self.stand_swap(id, &e, target, slot)
            }
            Some(_) => {
                if is_disabled(&e, item_slot) || (SLOTS[item_slot].3 && !show_arms(&e)) {
                    return false;
                }
                self.stand_swap(id, &e, item_slot, slot)
            }
        }
    }

    /// Exchange between the stand's slot and the player's hand.
    fn stand_swap(&mut self, id: i32, e: &Nbt, i: usize, hand: usize) -> bool {
        let cur = slot_item(e, i);
        let held = self.slots[hand].clone();
        let ds = disabled_slots(e);
        let bit = SLOTS[i].2;
        if cur.is_some() && ds & (1 << (bit + 8)) != 0 {
            return false;
        }
        if cur.is_none() && ds & (1 << (bit + 16)) != 0 {
            return false;
        }
        let new = if self.game_mode == 1 && cur.is_none() && held.is_some() {
            held.clone().map(|s| Stack { count: 1, ..s })
        } else if held.as_ref().is_none_or(|s| s.count <= 1) {
            self.slots[hand] = cur.clone();
            self.send_slot(hand);
            held.clone()
        } else if cur.is_some() {
            return false;
        } else {
            let mut h = held.clone().expect("held stack");
            h.count -= 1;
            self.slots[hand] = Some(h);
            self.send_slot(hand);
            held.clone().map(|s| Stack { count: 1, ..s })
        };
        self.stand_set_slot(id, e, i, cur.as_ref(), new);
        true
    }

    /// Compound, client, equipment sound and game event.
    fn stand_set_slot(&mut self, id: i32, e: &Nbt, i: usize, old: Option<&Stack>, new: Option<Stack>) {
        let new_nbt = new.as_ref().map(|s| s.to_nbt(None));
        self.deco_edit(id, |n| {
            let mut eq = n.get("equipment").cloned().unwrap_or(Nbt::Compound(Vec::new()));
            match new_nbt {
                Some(v) => {
                    // Game slot order in the compound.
                    let after = SLOTS[..i].iter().rev().find(|s| eq.get(s.0).is_some()).map(|s| s.0);
                    match after {
                        Some(a) => put_after(&mut eq, a, SLOTS[i].0, v),
                        None => {
                            if let Nbt::Compound(f) = &mut eq {
                                f.retain(|(k, _)| k != SLOTS[i].0);
                                f.insert(0, (SLOTS[i].0.to_owned(), v));
                            }
                        }
                    }
                }
                None => nbtio::remove(&mut eq, SLOTS[i].0),
            }
            if matches!(&eq, Nbt::Compound(f) if f.is_empty()) {
                nbtio::remove(n, "equipment");
            } else {
                put_after(n, "Brain", "equipment", eq);
            }
        });
        let net = self.net(new.as_ref());
        let _ = self.send(Event::Entity(EntityEvent::Equipment { id, slots: vec![(SLOTS[i].1, net)] }));
        let same = match (old, new.as_ref()) {
            (Some(a), Some(b)) => crate::items::same(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let eqp = new.as_ref().and_then(|s| self.comp_json(s, "minecraft:equippable"));
        if let Some(q) = &eqp
            && q["slot"].as_str() == Some(SLOTS[i].0)
        {
            let sound = q["equip_sound"].as_str().unwrap_or("minecraft:item.armor.equip_generic").to_owned();
            self.deco_sound(e, &sound, 6, 1.0, 1.0);
        }
        let at = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let ev = if eqp.is_some() { "minecraft:equip" } else { "minecraft:unequip" };
        self.game_event_at(ev, at, None);
    }

    /// Player hit on a stand (the player attack then the damage taken by an armor stand on the server side).
    pub(super) fn stand_attacked(&mut self, id: i32) {
        let Some(e) = self.deco_nbt(id).cloned() else { return };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if !self.may_interact(pos.map(|v| v.floor() as i32)) {
            return;
        }
        let weapon = self.player_weapon();
        let speed = self.player_attribute("minecraft:attack_speed");
        let delay = (1.0 / speed * 20.0) as f32;
        let scale = ((self.mobs.attack_ticker as f32 + 0.5) / delay).clamp(0.0, 1.0);
        let base = self.player_base_damage() * (0.2 + scale * scale * 0.8);
        self.mobs.attack_ticker = 0;
        if base <= 0.0 {
            return;
        }
        let full = scale > 0.9;
        let crit = full
            && self.life.vit.fall > 0.0
            && !self.on_ground
            && !self.life.vit.sprinting
            && !self.phys.climbable(self.st(pos.map(|v| v.floor() as i32)));
        // G5-F04: damage source of the weapon (its type, the mace's smash), like on a mob; the
        // damage itself does not matter to a stand.
        let smash = weapon.as_ref().is_some_and(|w| w.id == crate::mace::MACE)
            && crate::mace::can_smash(self.life.vit.fall, self.proj_player.fall_flying);
        let ty = self.weapon_damage_type(weapon.as_ref(), smash, "minecraft:player_attack");
        if !self.stand_hurt(id, &e, &ty) {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
            return;
        }
        if crit {
            self.world_sound("minecraft:entity.player.attack.crit", 7, self.pos, 1.0, 1.0);
            let _ = self.send(Event::Entity(EntityEvent::Animate { id, action: 1 }));
        } else {
            let s = if full { "minecraft:entity.player.attack.strong" } else { "minecraft:entity.player.attack.weak" };
            self.world_sound(s, 7, self.pos, 1.0, 1.0);
        }
        // The stand is a living entity: the mace's smash (fall stopped, sound, knockback around
        // it), the weapon's after-hit effects (Wind Burst; the stand's own equipment is not
        // read), then the fall reset.
        if smash {
            let bbox = self.deco_box(&e).unwrap_or(Aabb { min: pos, max: pos });
            let st = crate::mace::Struck {
                ent: Ent::Mob(id),
                uuid: nbtio::uuid(e.get("UUID")).unwrap_or(0),
                pos,
                bbox,
                on_ground: flag(&e, "OnGround"),
                on_pos: self.on_pos(pos, None, 1.0e-5),
            };
            self.player_smash(&st);
        }
        self.player_weapon_post_attack(crate::commands::ents::E::Other(id), pos, &ty, weapon.as_ref());
        if smash && crate::mace::can_smash(self.life.vit.fall, self.proj_player.fall_flying) {
            self.life.vit.fall = 0.0;
        }
        // Weapon wear (the stand is a living entity).
        if let Some(w) = &weapon {
            let per = self.weapon_wear(w);
            if per > 0 {
                self.hurt_item(36 + self.held as usize, per);
            }
        }
        let invul = self.abilities().invulnerable;
        self.life.vit.exhaust(0.1, invul);
    }

    /// The damage taken by an armor stand on the server side from a player source; true if the hit landed.
    pub(crate) fn stand_hurt(&mut self, id: i32, e: &Nbt, ty: &str) -> bool {
        let creative = self.game_mode == 1;
        if (flag(e, "Invulnerable") && !creative) || flag(e, "Invisible") || marker(e) {
            return false;
        }
        let incremental = self.dmg_tag(ty, "minecraft:can_break_armor_stand");
        let kill = self.dmg_tag(ty, "minecraft:always_kills_armor_stands");
        if (!incremental && !kill) || self.game_mode == 2 {
            return false;
        }
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if creative {
            self.deco_sound(e, "minecraft:entity.armor_stand.break", 6, 1.0, 1.0);
            self.stand_particles(e);
            self.stand_kill(id, pos);
            return true;
        }
        let now = self.time;
        let last = self.deco.live.iter().find(|l| l.id == id).map_or(0, |l| l.last_hit);
        if now - last > 5 && !kill {
            let _ = self.send(Event::Entity(EntityEvent::Event { id, event: 32 }));
            self.game_event_at("minecraft:entity_damage", pos, Some(Ent::Player));
            if let Some(l) = self.deco_live(id) {
                l.last_hit = now;
            }
        } else {
            // The stand itself to the block, then all the rest.
            let at = pos.map(|v| v.floor() as i32);
            self.pop_resource(at, named("minecraft:armor_stand", e));
            self.stand_broken(e, at);
            self.stand_particles(e);
            self.stand_kill(id, pos);
        }
        true
    }

    /// Break sound, equipment dropped above the block.
    pub(super) fn stand_broken(&mut self, e: &Nbt, at: P) {
        self.deco_sound(e, "minecraft:entity.armor_stand.break", 6, 1.0, 1.0);
        let up = [at[0], at[1] + 1, at[2]];
        for i in [MAINHAND, OFFHAND, FEET, LEGS, CHEST, HEAD] {
            // Curse of vanishing (enchantment effect "prevent equipment drop"): nothing falls.
            if let Some(s) = slot_item(e, i)
                && !self.ench_has(&s, "minecraft:prevent_equipment_drop")
            {
                self.pop_resource(up, s);
            }
        }
    }

    fn stand_kill(&mut self, id: i32, pos: [f64; 3]) {
        self.deco_remove(id);
        self.game_event_at("minecraft:entity_die", pos, Some(Ent::Player));
    }

    /// Oak plank fragments.
    fn stand_particles(&self, e: &Nbt) {
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let (w, h, _) = self.living_size(e, true);
        let Some(state) = self.states.state_of(&self.data, &Nbt::String("minecraft:oak_planks".into())) else { return };
        let at = [pos[0], pos[1] + h * 0.666_666_666_666_666_6, pos[2]];
        let d = [(w / 4.0) as f32, (h / 4.0) as f32, (w / 4.0) as f32];
        self.block_particles(state, at, d, 0.05, 10);
    }
}

impl Server {
    /// Tick of an armor stand or a mannequin (no input): falling
    /// in the air, landing (fall sound), network tracking. Without physics (marker, no
    /// gravity, immobile mannequin): nothing moves, but the tracking still runs (the game tracks
    /// every entity each tick: a push is sent by it, compared with the last velocity sent).
    pub(super) fn living_tick(&mut self, i: usize, pos: [f64; 3]) {
        let id = self.deco.live[i].id;
        let stand = self.deco.live[i].kind == DKind::Stand;
        // Mannequin: hurt and death countdowns (removed at the end of its death).
        if !stand && self.mannequin_tick_counts(i) {
            return;
        }
        let interval = if stand { 3 } else { 2 };
        // Fields read without cloning the compound (equipment included): called every tick.
        let Some(e) = self.live_nbt(i) else { return };
        let motion = nbtio::motion(e.get("Motion"));
        let on_ground = flag(e, "OnGround");
        let rot = nbtio::f32s::<2>(e.get("Rotation")).unwrap_or_default();
        // Tracking alone, the entity left where it is; its small speeds are still cancelled (the
        // game does it each tick for every living entity, physics or not).
        let still = |s: &mut Server| {
            let saved = motion.unwrap_or_default();
            let v = saved.map(|c| if c.abs() < 0.003 { 0.0 } else { c });
            // Compared bit for bit: a saved -0.0 becomes 0.0 like the game's.
            if v.map(f64::to_bits) != saved.map(f64::to_bits) {
                s.deco_edit_at(i, |n| nbtio::set(n, "Motion", super::dbl(v)));
            }
            let mut t = std::mem::take(&mut s.deco.live[i].track);
            s.track_turned(&mut t, id, pos, v, on_ground, interval, rot);
            s.deco.live[i].track = t;
        };
        if flag(e, "NoGravity") || if stand { marker(e) } else { flag(e, "immovable") } {
            still(self);
            return;
        }
        let (w, h, _) = self.living_size(e, stand);
        let mut fall = match e.get("fall_distance") {
            Some(Nbt::Double(d)) => *d,
            _ => 0.0,
        };
        let silent = flag(e, "Silent");
        let b = Aabb { min: [pos[0] - w / 2.0, pos[1], pos[2] - w / 2.0], max: [pos[0] + w / 2.0, pos[1] + h, pos[2] + w / 2.0] };
        // In a fluid: fluid displacement not simulated for them, the entity stays in place (its
        // tracking still runs).
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.floor() as i32);
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    if !self.fluid_at([x, y, z]).is_empty() {
                        still(self);
                        return;
                    }
                }
            }
        }
        let mut vel = motion.unwrap_or_default();
        // Small speeds cancelled.
        for v in &mut vel {
            if v.abs() < 0.003 {
                *v = 0.0;
            }
        }
        // Friction of the block under the feet on the ground, movement, gravity.
        let fb = if on_ground {
            let below = [pos[0].floor() as i32, (pos[1] - 0.500_001).floor() as i32, pos[2].floor() as i32];
            self.phys.friction.get(self.st(below) as usize).copied().unwrap_or(0.6)
        } else {
            1.0
        };
        let step = if stand { 0.0 } else { f64::from(0.6f32) };
        let world = self.world_for_room(Self::room_for_object(id, pos));
        let mv = rc_physics::collide::resolve(&world, &b, vel, on_ground, step);
        let l2 = |v: [f64; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let mut np = pos;
        if l2(mv) > 1.0e-7 || l2(vel) - l2(mv) < 1.0e-7 {
            np = [pos[0] + mv[0], pos[1] + mv[1], pos[2] + mv[2]];
        }
        let vertical = vel[1] != mv[1];
        let landed = vertical && vel[1] < 0.0;
        if (vel[0] - mv[0]).abs() >= 1.0e-5 {
            vel[0] = 0.0;
        }
        if (vel[2] - mv[2]).abs() >= 1.0e-5 {
            vel[2] = 0.0;
        }
        if vertical {
            vel[1] = 0.0;
        }
        // Fall: accumulated distance, then sound on landing beyond 3 blocks (the stand
        // takes no fall damage).
        if landed {
            if fall > 0.0 && stand && (fall - 3.0).ceil() > 0.0 && !silent {
                self.world_sound("minecraft:entity.armor_stand.fall", 6, np, 1.0, 1.0);
                let under = [np[0].floor() as i32, (np[1] - 0.2).floor() as i32, np[2].floor() as i32];
                let s = self.st(under);
                if !self.view().is_air(s)
                    && let Some(t) = self.data.state(s).and_then(|i| self.data.sound_types.get(usize::from(i.sound)))
                {
                    let (snd, vol, pit) = (t.fall.clone(), t.volume * 0.5, t.pitch * 0.75);
                    self.world_sound(&snd, 6, np, vol, pit);
                }
            }
            fall = 0.0;
        } else if mv[1] < 0.0 {
            fall -= mv[1];
        }
        let fr = f64::from(fb * 0.91f32);
        let vel = [vel[0] * fr, (vel[1] - 0.08) * f64::from(0.98f32), vel[2] * fr];
        let changed = np != pos || landed != on_ground || motion != Some(vel);
        if changed {
            let fd = fall;
            self.deco_edit_at(i, |n| {
                nbtio::set(n, "Pos", super::dbl(np));
                nbtio::set(n, "Motion", super::dbl(vel));
                nbtio::set(n, "OnGround", Nbt::Byte(i8::from(landed)));
                nbtio::set(n, "fall_distance", Nbt::Double(fd));
            });
        }
        let mut t = std::mem::take(&mut self.deco.live[i].track);
        self.track_turned(&mut t, id, np, vel, landed, interval, rot);
        self.deco.live[i].track = t;
    }
}

/// Armor stand or mannequin that a piston pushes: its box, position, ground flag and, on the
/// ground, its main supporting block (a marker is ignored by pistons: `None`).
pub(crate) struct PistonBody {
    pub bbox: Aabb,
    pub pos: [f64; 3],
    pub on_ground: bool,
    pub support: Option<[i32; 3]>,
}

impl Server {
    /// [`PistonBody`] of the decoration `id`, if it is a pushable armor stand or mannequin.
    pub(crate) fn deco_piston_body(&self, id: i32) -> Option<PistonBody> {
        let e = self.deco_nbt(id)?;
        let k = kind_of(e)?;
        if !matches!(k, DKind::Stand | DKind::Mannequin) || (k == DKind::Stand && marker(e)) {
            return None;
        }
        let bbox = self.deco_box(e)?;
        let pos = nbtio::f64s::<3>(e.get("Pos"))?;
        let on_ground = flag(e, "OnGround");
        let support = if on_ground { rc_physics::support_of(&self.world(), &bbox, pos) } else { None };
        Some(PistonBody { bbox, pos, on_ground, support })
    }

    /// Piston move of an armor stand or a mannequin: a weightless stand has no physics and is
    /// placed as is (no cap, no collision); the others are capped at 0.51 block per axis and per
    /// tick (their own total), then moved with their collisions. Its tick's tracking sends it.
    pub(crate) fn deco_piston_move(&mut self, id: i32, d: [f64; 3]) {
        let Some(i) = self.deco.live.iter().position(|l| l.id == id && !l.removed) else { return };
        let Some(e) = self.live_nbt(i) else { return };
        if self.deco.live[i].kind == DKind::Stand && flag(e, "NoGravity") {
            let Some(p) = nbtio::f64s::<3>(e.get("Pos")) else { return };
            let np = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            self.deco_edit(id, |n| nbtio::set(n, "Pos", super::dbl(np)));
            return;
        }
        let d = self.deco.live[i].piston.limit(self.time, d);
        if d != [0.0; 3] {
            self.living_shove(id, d);
        }
    }

    /// Velocity of an armor stand or a mannequin set on one axis (slime block in motion: the
    /// server simulates them).
    pub(crate) fn deco_launch(&mut self, id: i32, axis: usize, v: f64) {
        let Some(e) = self.deco_nbt(id) else { return };
        let mut m = nbtio::motion(e.get("Motion")).unwrap_or_default();
        m[axis] = v;
        self.deco_edit(id, |n| nbtio::set(n, "Motion", super::dbl(m)));
    }

    /// An ordinary move of an armor stand or a mannequin by `d` with its collisions (opening
    /// shulker, capped piston push): position, ground flag, horizontal speed stopped against a
    /// wall. A marker or a weightless stand has no physics and is not moved. The tracking of its
    /// tick sends it.
    pub(super) fn living_shove(&mut self, id: i32, d: [f64; 3]) {
        let Some(e) = self.deco_nbt(id) else { return };
        let stand = kind_of(e) == Some(DKind::Stand);
        if stand && (marker(e) || flag(e, "NoGravity")) {
            return;
        }
        let Some(pos) = nbtio::f64s::<3>(e.get("Pos")) else { return };
        let (w, h, _) = self.living_size(e, stand);
        let on_ground = flag(e, "OnGround");
        let mut vel = nbtio::motion(e.get("Motion")).unwrap_or_default();
        let b = Aabb { min: [pos[0] - w / 2.0, pos[1], pos[2] - w / 2.0], max: [pos[0] + w / 2.0, pos[1] + h, pos[2] + w / 2.0] };
        let step = if stand { 0.0 } else { f64::from(0.6f32) };
        let world = self.world_for_room(Self::room_for_object(id, pos));
        let mv = rc_physics::collide::resolve(&world, &b, d, on_ground, step);
        let np = [pos[0] + mv[0], pos[1] + mv[1], pos[2] + mv[2]];
        let vertical = d[1] != mv[1];
        let ground = vertical && d[1] < 0.0;
        for a in [0, 2] {
            if (d[a] - mv[a]).abs() >= 1.0e-5 {
                vel[a] = 0.0;
            }
        }
        if vertical {
            vel[1] = 0.0;
        }
        self.deco_edit(id, |n| {
            nbtio::set(n, "Pos", super::dbl(np));
            nbtio::set(n, "Motion", super::dbl(vel));
            nbtio::set(n, "OnGround", Nbt::Byte(i8::from(ground)));
        });
    }
}

/// Fields of the living entity of a new stand.
fn living_fields() -> Vec<(String, Nbt)> {
    vec![
        ("Health".into(), Nbt::Float(20.0)),
        ("HurtTime".into(), Nbt::Short(0)),
        ("DeathTime".into(), Nbt::Short(0)),
        ("AbsorptionAmount".into(), Nbt::Float(0.0)),
        ("current_impulse_context_reset_grace_time".into(), Nbt::Int(0)),
        ("attributes".into(), Nbt::List(Vec::new())),
        ("FallFlying".into(), Nbt::Byte(0)),
        ("Brain".into(), Nbt::Compound(vec![("memories".into(), Nbt::Compound(Vec::new()))])),
    ]
}

/// Wrapping an angle into [−180, 180) (float): angle wrapped into [−180, 180).
fn wrap_degrees(a: f32) -> f32 {
    let mut w = a % 360.0;
    if w >= 180.0 {
        w -= 360.0;
    }
    if w < -180.0 {
        w += 360.0;
    }
    w
}
