//! Common player interactions with a mob: name tag and spawn egg
//! (handled before the species' own interaction), sheep dyeing (after it), tamed cat and
//! wolf (collar, food, wolf armour), animal food, forced baby,
//! mixed colour of a baby.

use super::combat::DYES;
use super::mob::Mob;
use super::mounts::BODY;
use super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

/// Makes the mob a baby if its species allows it; true if it became one.
pub(crate) fn set_baby(m: &mut Mob) -> bool {
    match m.kind {
        Kind::Zombie | Kind::Husk | Kind::ZombieVillager | Kind::Drowned | Kind::ZombifiedPiglin => {
            m.baby = true;
            m.attrs.speed.remove_mod("minecraft:baby");
            m.attrs.speed.set_mod("minecraft:baby", 0.5, 1);
        }
        Kind::Piglin => super::piglins::set_piglin_baby(m, true),
        Kind::Zoglin => {
            m.baby = true;
            m.attrs.attack.base = 0.5;
        }
        Kind::SulfurCube => m.set_baby(true),
        // Species that cannot be babies.
        Kind::CamelHusk | Kind::Frog | Kind::ZombieNautilus | Kind::Parrot => {}
        k if k.ageable() => {
            m.age = -24000;
            super::piglins::age_boundary(m);
        }
        _ => {}
    }
    m.is_baby()
}

/// Common data of a bucketed mob: flags set (no AI, silent, no gravity,
/// glowing, invulnerable, persistent) then health.
pub(crate) fn bucket_default_data(m: &Mob) -> rc_proto::nbt::Nbt {
    use rc_proto::nbt::Nbt;
    let mut data = Nbt::Compound(Vec::new());
    for (k, on) in [
        ("NoAI", m.no_ai),
        ("Silent", m.x.silent),
        ("NoGravity", m.surf.no_gravity),
        ("Glowing", m.x.glowing),
        ("Invulnerable", m.invulnerable),
        ("PersistenceRequired", m.persistent),
    ] {
        if on {
            crate::nbtio::set(&mut data, k, Nbt::Byte(1));
        }
    }
    crate::nbtio::set(&mut data, "Health", Nbt::Float(m.health));
    data
}

/// Re-reading of the common data of a bucket: each present flag is set as is (persistence
/// only if true), then health.
pub(crate) fn load_bucket_default_data(m: &mut Mob, data: &rc_proto::nbt::Nbt) {
    let flag = |k: &str| crate::nbtio::int(data, k).map(|v| v != 0);
    if let Some(v) = flag("NoAI") {
        m.no_ai = v;
    }
    if let Some(v) = flag("Silent") {
        m.x.silent = v;
    }
    if let Some(v) = flag("NoGravity") {
        m.surf.no_gravity = v;
    }
    if let Some(v) = flag("Glowing") {
        m.x.glowing = v;
    }
    if let Some(v) = flag("Invulnerable") {
        m.invulnerable = v;
    }
    if flag("PersistenceRequired") == Some(true) {
        m.persistent = true;
    }
    if let Some(h) = crate::nbtio::float(data, "Health") {
        m.health = h.clamp(0.0, m.max_health());
    }
}

/// Crack level of the wolf armour according to the remaining durability fraction.
fn wolf_armor_crack(damage: i32, max: i32) -> u8 {
    if max <= 0 {
        return 0;
    }
    let f = (max - damage) as f32 / max as f32;
    if f < 0.32 {
        3
    } else if f < 0.69 {
        2
    } else if f < 0.95 {
        1
    } else {
        0
    }
}

impl Server {
    /// Dye colour of a stack, as an index into `DYES`.
    pub(crate) fn dye_index(&self, s: &Stack) -> Option<u8> {
        let c = self.life.items.dye(s)?;
        DYES.iter().position(|d| *d == c).map(|i| i as u8)
    }

    /// Colour of a baby from those of the parents: result of the recipe that mixes the two
    /// dyes, otherwise one of the two at random (world random).
    pub(crate) fn mixed_dye(&mut self, a: u8, b: u8) -> u8 {
        if let Some(c) = self.dye_mix_recipe(a, b) {
            return c;
        }
        if self.mobs.rng.next_bool() { a } else { b }
    }

    fn dye_mix_recipe(&self, a: u8, b: u8) -> Option<u8> {
        // Dye of each colour: the item `<colour>_dye`, checked by its component.
        let item = |c: u8| {
            let name = DYES.get(usize::from(c))?;
            let s = Stack { id: format!("minecraft:{name}_dye"), count: 1, components: None };
            (self.life.items.dye(&s).as_deref() == Some(*name)).then_some(s)
        };
        let grid = [Some(item(a)?), Some(item(b)?), None, None];
        let maps = &self.maps;
        let (_, out) = self.life.recipes.craft(&self.life.items, &grid, 2, &|id| maps.scale_of(id))?;
        self.dye_index(&out)
    }

    /// Priority interactions: name tag, then spawn egg of the same species.
    pub(crate) fn important_interact(&mut self, m: &mut Mob, slot: usize) -> bool {
        let Some(held) = self.slots[slot].clone() else { return false };
        if held.id == "minecraft:name_tag"
            && let Some(name) = crate::items::component(&held, "minecraft:custom_name")
        {
            m.custom_name = Some(name.clone());
            m.persistent = true;
            self.consume_held(slot);
            return true;
        }
        // Egg of another species, or baby impossible: the interaction continues.
        held.id.ends_with("_spawn_egg") && self.egg_offspring(m, slot, &held)
    }

    /// Species of the egg: entity data of the stack, otherwise that of the item.
    pub(crate) fn egg_kind(&self, egg: &Stack) -> Option<Kind> {
        let id = crate::items::component(egg, "minecraft:entity_data")
            .and_then(|d| d.get("id"))
            .and_then(rc_proto::nbt::Nbt::as_str)
            .map(str::to_owned)
            .or_else(|| self.life.items.defaults(&egg.id)["minecraft:entity_data"]["id"].as_str().map(str::to_owned))?;
        Kind::from_name(&id)
    }

    /// Baby born from an egg placed on a mob of its species: breeding baby (the
    /// mob stands in for both parents) or new mob, at its position, null look, name
    /// of the egg, egg consumed outside creative. False if the species differs or cannot be a baby.
    fn egg_offspring(&mut self, m: &mut Mob, slot: usize, egg: &Stack) -> bool {
        if self.egg_kind(egg) != Some(m.kind) {
            return false;
        }
        let mut baby = if bred_here(m.kind) { self.breed_offspring(m, None) } else { self.new_mob(m.kind, m.pos, 0.0) };
        if !set_baby(&mut baby) {
            return false;
        }
        baby.pos = m.pos;
        baby.old = m.pos;
        baby.yaw = 0.0;
        baby.pitch = 0.0;
        baby.head_yaw = 0.0;
        baby.body_yaw = 0.0;
        if let Some(n) = crate::items::component(egg, "minecraft:custom_name") {
            baby.custom_name = Some(n.clone());
        }
        match m.kind {
            // Zombie family: pickup drawn on the parent's random.
            Kind::Zombie | Kind::Husk | Kind::ZombieVillager | Kind::Drowned | Kind::ZombifiedPiglin => {
                let (_, special) = self.local_difficulty(baby.block_pos());
                baby.can_pickup = m.rng.next_float() < 0.55 * special;
            }
            Kind::Fox => super::wild::fox::add_trusted(&mut baby, self.uuid),
            _ => {}
        }
        self.add_mob(baby);
        self.consume_held(slot);
        true
    }

    /// Breeding baby of the species bred here (cow, pig, chicken, cat, wolf,
    /// sheep); `partner`: (variant, wool, collar) of the second parent, `None` = the same.
    pub(crate) fn breed_offspring(&mut self, m: &mut Mob, partner: Option<(i32, u8, i32)>) -> Mob {
        let (pv, pw, pc) = partner.unwrap_or((m.variant, m.wool, m.collar));
        let mut baby = self.new_mob(m.kind, m.pos, 0.0);
        baby.age = -24000;
        match m.kind {
            Kind::Cow | Kind::Pig | Kind::Chicken => baby.variant = if m.rng.next_bool() { m.variant } else { pv },
            Kind::Cat | Kind::Wolf => {
                baby.variant = if m.rng.next_bool() { m.variant } else { pv };
                if m.tame {
                    baby.owner = m.owner;
                    baby.tame = true;
                    if m.kind == Kind::Wolf {
                        baby.attrs.max_health.base = 40.0;
                        baby.health = 40.0;
                    }
                    baby.collar = i32::from(self.mixed_dye(m.collar as u8 & 15, pc as u8 & 15));
                }
                // Wolf: sound variant drawn at random; cat: the default one.
                if m.kind == Kind::Wolf {
                    let n = self.regs.entries("minecraft:wolf_sound_variant").len() as i32;
                    baby.sound_variant = m.rng.next_int_bound(n.max(1));
                }
            }
            Kind::Sheep => baby.wool = self.mixed_dye(m.wool & 15, pw & 15),
            _ => {}
        }
        baby
    }

    /// Item used on the mob when it did nothing: sheep dye.
    pub(crate) fn item_on_mob(&mut self, m: &mut Mob, slot: usize) -> bool {
        let Some(held) = self.slots[slot].clone() else { return false };
        if m.kind != Kind::Sheep || !m.alive() || m.sheared || !held.id.ends_with("_dye") {
            return false;
        }
        let Some(c) = self.dye_index(&held) else { return false };
        if c == m.wool & 15 {
            return false;
        }
        self.entity_sound(m.id, "minecraft:item.dye.use", 7, 1.0, 1.0);
        m.wool = c;
        self.consume_held(slot);
        true
    }

    /// Animal food (love, baby that grows up and keeps its progress), then golden dandelion.
    pub(crate) fn animal_food_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> bool {
        if let Some(tag) = m.kind.food_tag()
            && self.item_tag(item, tag)
        {
            if m.age == 0 && !m.is_in_love() {
                self.consume_held(slot);
                m.in_love = 600;
                m.love_cause = true; // S9: breeding credited to the player
                self.mob_event(m, 18);
                self.eating_sound(m);
                return true;
            }
            if m.is_baby() && !m.mount.age_locked {
                self.consume_held(slot);
                super::mounts::age_up(m, super::mounts::feeding_secs(-m.age), true);
                self.eating_sound(m);
                return true;
            }
        }
        self.golden_dandelion(m, slot, item)
    }

    /// Eating sound specific to the species, that of its sound variant: cat (volume and pitch 1),
    /// pig (the mob's voice, pitch random drawn).
    fn eating_sound(&mut self, m: &mut Mob) {
        if m.kind == Kind::Cat {
            let s = self.variant_sound(m, "eat_sound").unwrap_or_else(|| "minecraft:entity.cat.eat".into());
            self.mob_sound(m, &s, 1.0, 1.0);
        } else if m.kind == Kind::Pig {
            let s = self.variant_sound(m, "eat_sound").unwrap_or_else(|| "minecraft:entity.pig.eat".into());
            self.make_sound(m, &s);
        }
    }

    /// Meal of a tamed animal: healing (factor × nutrition, otherwise the default value).
    fn tame_feed(&mut self, m: &mut Mob, slot: usize, factor: f32, default: f32) {
        let held = self.slots[slot].clone();
        let heal = held.as_ref().and_then(|h| self.life.items.food(h)).map_or(default, |f| factor * f.0 as f32);
        self.consume_held(slot);
        if m.health > 0.0 {
            m.health = (m.health + heal).min(m.max_health());
        }
        self.eating_sound(m);
    }

    /// Taming attempt (one chance in three, animal's random).
    fn try_tame(&mut self, m: &mut Mob) {
        if m.rng.next_int_bound(3) == 0 {
            m.tame = true;
            m.owner = Some(self.uuid);
            if m.kind == Kind::Wolf {
                m.attrs.max_health.base = 40.0;
                m.health = 40.0;
                m.nav.stop();
                m.target = None;
            }
            m.sitting = true;
            self.s9_tamed(m); // S9
            self.mob_event(m, 7);
        } else {
            self.mob_event(m, 6);
        }
    }

    /// Cat and wolf: taming, collar, healing, wolf armour, sitting/standing.
    pub(crate) fn tame_interact(&mut self, m: &mut Mob, slot: usize) -> bool {
        let held = self.slots[slot].clone();
        let item = held.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        let owned = m.tame && m.owner == Some(self.uuid);
        if m.kind == Kind::Cat {
            if m.tame {
                if owned {
                    if self.item_tag(&item, "minecraft:cat_collar_dyes") {
                        if let Some(c) = held.as_ref().and_then(|h| self.dye_index(h))
                            && i32::from(c) != m.collar
                        {
                            m.collar = i32::from(c);
                            self.consume_held(slot);
                            m.persistent = true;
                            return true;
                        }
                    } else if self.item_tag(&item, "minecraft:cat_food") && m.health < m.max_health() {
                        self.tame_feed(m, slot, 1.0, 1.0);
                        return true;
                    }
                    if !self.animal_food_interact(m, slot, &item) {
                        m.sitting = !m.sitting;
                    }
                    return true;
                }
            } else if self.item_tag(&item, "minecraft:cat_food") {
                self.consume_held(slot);
                self.try_tame(m);
                m.persistent = true;
                self.eating_sound(m);
                return true;
            }
            let r = self.animal_food_interact(m, slot, &item);
            if r {
                m.persistent = true;
            }
            return r;
        }
        if m.tame {
            if self.item_tag(&item, "minecraft:wolf_food") && m.health < m.max_health() {
                self.tame_feed(m, slot, 2.0, 2.0);
                return true;
            }
            if !self.item_tag(&item, "minecraft:wolf_collar_dyes") || !owned {
                if let Some(h) = &held
                    && owned
                    && !m.is_baby()
                    && m.equipment[BODY].is_none()
                    && self.mount_equippable_in(m, h, BODY)
                {
                    m.drop_chance[BODY] = 2.0;
                    self.mount_set_slot(m, BODY, Some(Stack { count: 1, ..h.clone() }));
                    self.consume_held(slot);
                    return true;
                }
                if let (Some(h), Some(armor)) = (&held, m.equipment[BODY].clone())
                    && m.sitting
                    && owned
                    && self.life.items.damageable(&armor)
                    && self.life.items.damage(&armor) > 0
                    && crate::stations::valid_repair(&self.life.items, &armor, h)
                {
                    // The game shortens the stack, then returns it whole in creative: nothing in creative.
                    self.consume_held(slot);
                    self.mob_sound(m, "minecraft:item.wolf_armor.repair", 1.0, 1.0);
                    let max = self.life.items.max_damage(&armor);
                    let unit = (max as f32 * 0.125) as i32;
                    let d = (self.life.items.damage(&armor) - unit).max(0);
                    let mut armor = armor;
                    crate::lootfn::set_damage(&mut armor, d);
                    self.set_body_armor(m, Some(armor));
                    return true;
                }
                let r = self.animal_food_interact(m, slot, &item);
                if !r && owned {
                    m.sitting = !m.sitting;
                    m.jumping = false;
                    m.nav.stop();
                    m.target = None;
                    return true;
                }
                return r;
            }
            if let Some(c) = held.as_ref().and_then(|h| self.dye_index(h))
                && i32::from(c) != m.collar
            {
                m.collar = i32::from(c);
                self.consume_held(slot);
                return true;
            }
        } else if item == "minecraft:bone" && m.anger_end < 0 {
            self.consume_held(slot);
            self.try_tame(m);
            return true;
        }
        self.animal_food_interact(m, slot, &item)
    }

    /// Wolf armour replaced without a placement sound (wear, repair): equipment sent.
    fn set_body_armor(&self, m: &mut Mob, s: Option<Stack>) {
        m.equipment[BODY] = s;
        if m.track.seen {
            let slots = vec![(BODY as u8, self.net(m.equipment[BODY].as_ref()))];
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Equipment { id: m.id, slots }));
        }
    }

    /// The wolf armour takes the damage (except `#bypasses_wolf_armor`): wear rounded up
    /// (Unbreaking included), breaking (event 65), sound and scale crumbs on crack change.
    /// True if it took them (health does not move).
    pub(crate) fn wolf_armor_absorb(&mut self, m: &mut Mob, ty: &str, dmg: f32) -> bool {
        if !self.wolf_armor_absorbs(m, ty) {
            return false;
        }
        let Some(mut armor) = m.equipment[BODY].clone() else { return false };
        let items = &self.life.items;
        if !items.damageable(&armor) {
            return true;
        }
        let (before, max) = (items.damage(&armor), items.max_damage(&armor));
        let armor_tag = items.has_tag("minecraft:enchantable/armor", &armor.id);
        let n = self.unbreaking_kept(&armor, dmg.ceil() as i32, armor_tag);
        if n <= 0 {
            return true;
        }
        let after = before + n;
        let level_after = if after >= max {
            // Event first: the client plays the item's break with it still in its body slot.
            self.mob_event(m, 65);
            self.set_body_armor(m, None);
            0
        } else {
            crate::lootfn::set_damage(&mut armor, after);
            self.set_body_armor(m, Some(armor));
            wolf_armor_crack(after, max)
        };
        if wolf_armor_crack(before, max) != level_after {
            self.mob_sound(m, "minecraft:item.wolf_armor.crack", 1.0, 1.0);
            self.item_particles("minecraft:armadillo_scute", [m.pos[0], m.pos[1] + 1.0, m.pos[2]], [0.2, 0.1, 0.2], 0.1, 20);
        }
        true
    }

    /// Wolf armour able to absorb this damage type.
    pub(crate) fn wolf_armor_absorbs(&self, m: &Mob, ty: &str) -> bool {
        m.kind == Kind::Wolf
            && m.equipment[BODY].as_ref().is_some_and(|s| s.id == "minecraft:wolf_armor")
            && !self.dmg_tag(ty, "minecraft:bypasses_wolf_armor")
    }
}

/// Species whose breeding baby is built by `breed_offspring`.
fn bred_here(k: Kind) -> bool {
    matches!(k, Kind::Cow | Kind::Pig | Kind::Chicken | Kind::Cat | Kind::Wolf | Kind::Sheep)
}
