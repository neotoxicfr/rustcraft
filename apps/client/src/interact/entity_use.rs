//! Right click on an entity: the game client's local reaction, worked out by the integrated
//! server's rules (`rc_sim::entity_use_prediction`) on this client's copy of the entity, then
//! its effects applied here (sound for the actor, held stack used or filled, a mob gone into
//! its bucket). Only a success stops the click; anything else goes on to the item's use.

use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;
use rc_sim::entity_use_prediction::{
    self as prediction, ActingPlayer, HeldEquippable, HeldItem, LeashHolder, LocalEffect, Outcome, SeenEntity,
    SoundCategory,
};

use super::UseRes;
use crate::entities::Entity;
use crate::particles::{Kind as PKind, Options as POptions};

/// Ticks the game's client waits before a golden dandelion works again on the same entity.
const AGE_LOCK_WAIT_TICKS: u64 = 40;

/// Network equipment slot (`set_equipment` order) of an `equippable` slot code
/// (`ItemInfo::equip_slot`: the component's slot id + 1; 0 = none).
fn worn_slot_of_code(code: u8) -> Option<usize> {
    Some(match code {
        1 => 0,
        6 => 1,
        2..=5 => usize::from(code),
        7 => 6,
        8 => 7,
        _ => return None,
    })
}

/// A client entity as the prediction reads it.
struct ClientSeen<'a> {
    entity: &'a Entity,
    kind: String,
    worn: [Option<&'a str>; 8],
    worn_comes_off_with_shears: [bool; 8],
    max_health: f32,
    leash_holder: LeashHolder,
    holds_leashes: bool,
    distance_sqr: f64,
    age_lock_cooldown: i32,
    fed_a_stew_flower: bool,
}

impl SeenEntity for ClientSeen<'_> {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn data(&self, index: u8) -> Option<&Meta> {
        self.entity.meta.get(&index)
    }

    fn worn(&self, slot: usize) -> Option<&str> {
        self.worn.get(slot).copied().flatten()
    }

    fn worn_comes_off_with_shears(&self, slot: usize) -> bool {
        self.worn_comes_off_with_shears.get(slot).copied().unwrap_or(false)
    }

    fn max_health(&self) -> f32 {
        self.max_health
    }

    fn passengers(&self) -> usize {
        self.entity.passengers.len()
    }

    fn on_ground(&self) -> bool {
        self.entity.on_ground
    }

    fn leash_holder(&self) -> LeashHolder {
        self.leash_holder
    }

    fn holds_leashes(&self) -> bool {
        self.holds_leashes
    }

    fn distance_sqr(&self) -> f64 {
        self.distance_sqr
    }

    fn age_lock_cooldown(&self) -> i32 {
        self.age_lock_cooldown
    }

    fn fed_a_stew_flower(&self) -> bool {
        self.fed_a_stew_flower
    }
}

impl crate::App {
    /// The clicked entity's local reaction, its effects applied; the click's outcome.
    pub(super) fn entity_use(&mut self, hand: usize, id: i32) -> UseRes {
        let Some(predicted) = self.predict_entity_use(hand, id) else { return UseRes::Pass };
        for effect in predicted.effects {
            self.apply_entity_use_effect(hand, id, effect);
        }
        match predicted.outcome {
            Outcome::Pass => UseRes::Pass,
            Outcome::Success { swing } => UseRes::Success { swing },
        }
    }

    fn predict_entity_use(&self, hand: usize, id: i32) -> Option<prediction::Prediction> {
        let e = self.entities.map.get(&id)?;
        let seen = self.seen_entity(id, e);
        let item = self.held_item_facts(self.hand_stack(hand), e);
        let other_hand_item = self.hand_stack(1 - hand).map_or("", |s| self.item_name(s.item));
        let player = ActingPlayer {
            uuid: self.local_uuid,
            hand,
            sneaking: self.sneak_key(),
            infinite_materials: self.world_sounds.instabuild,
            other_hand_item,
            thundering: self.world_thundering(),
        };
        let tags = |item: &str, tag: &str| self.item_name_in_tag(item, tag);
        Some(prediction::predict(&seen, &item, &player, &tags))
    }

    fn seen_entity<'a>(&'a self, id: i32, e: &'a Entity) -> ClientSeen<'a> {
        let worn = std::array::from_fn(|s| e.equipment[s].as_ref().map(|st| self.item_name(st.item)));
        let worn_comes_off_with_shears = std::array::from_fn(|s| {
            let info = e.equipment[s]
                .as_ref()
                .and_then(|st| usize::try_from(st.item).ok())
                .and_then(|i| self.data.items.get(i));
            info.is_some_and(|i| i.equip_can_be_sheared)
        });
        let attributes = self.data.registry("minecraft:attribute").map_or(&[][..], |r| r.entries.as_slice());
        let health = match e.meta.get(&9) {
            Some(Meta::Float(h)) => *h,
            _ => 0.0,
        };
        // Without a synced maximum the entity counts as unhurt.
        let max_health = e.synced_attribute(attributes, "minecraft:max_health").map_or(health, |m| m as f32);
        let d: f64 = (0..3).map(|a| (e.pos[a] - self.player.pos[a]).powi(2)).sum();
        ClientSeen {
            entity: e,
            kind: format!("minecraft:{}", e.kind),
            worn,
            worn_comes_off_with_shears,
            max_health,
            leash_holder: self.leash_holder_of(id),
            holds_leashes: self.misc.leashes.values().any(|&holder| holder == id),
            distance_sqr: d,
            age_lock_cooldown: self.age_lock_cooldown(id, e.uuid),
            fed_a_stew_flower: e.stew_flower_fed,
        }
    }

    fn leash_holder_of(&self, id: i32) -> LeashHolder {
        match self.misc.leashes.get(&id) {
            None => LeashHolder::Nobody,
            Some(&holder) if holder == self.player_id => LeashHolder::ActingPlayer,
            Some(holder) if self.entities.map.get(holder).is_some_and(|h| h.kind == "player") => {
                LeashHolder::OtherPlayer
            }
            Some(_) => LeashHolder::OtherEntity,
        }
    }

    fn age_lock_cooldown(&self, id: i32, uuid: u128) -> i32 {
        match self.act.age_lock_waits.get(&id) {
            Some(&(waited, until)) if waited == uuid => i32::try_from(until.saturating_sub(self.act.tick)).unwrap_or(0),
            _ => 0,
        }
    }

    /// The stack in hand as the prediction reads it: name, custom name, dye, and its
    /// `equippable` component (patch first, then the item's default) towards `clicked`.
    fn held_item_facts<'a>(&'a self, stack: Option<&'a ItemStack>, clicked: &Entity) -> HeldItem<'a> {
        let Some(stack) = stack else {
            return HeldItem { name: "", custom_name: false, dye: None, equippable: None };
        };
        let ids = &self.component_ids;
        let patched = |name: &str| ids.of(name).and_then(|i| stack.component(i));
        let info = usize::try_from(stack.item).ok().and_then(|i| self.data.items.get(i));
        let dye = match patched("dye") {
            Some(b) => {
                rc_proto::varint::read_varint(&mut rc_proto::Reader::new(b)).ok().and_then(|c| u8::try_from(c).ok())
            }
            None => info.and_then(|i| i.dye.checked_sub(1)),
        };
        HeldItem {
            name: self.item_name(stack.item),
            custom_name: patched("custom_name").is_some(),
            dye,
            equippable: self.held_equippable(stack, info, clicked),
        }
    }

    fn held_equippable(
        &self,
        stack: &ItemStack,
        info: Option<&rc_data::ItemInfo>,
        clicked: &Entity,
    ) -> Option<HeldEquippable> {
        let component = self.component_ids.equippable;
        if let Some(b) = component.and_then(|i| stack.component(i)) {
            let rules = crate::stack::equippable_rules(b)?;
            let allows_clicked = match &rules.allowed {
                crate::stack::AllowedEntities::Any => true,
                crate::stack::AllowedEntities::Tag(tag) => self.entity_in_tag(clicked, tag),
                crate::stack::AllowedEntities::Types(types) => types.contains(&clicked.kind_id),
            };
            let slot = worn_slot_of_code(rules.slot_code)?;
            return Some(HeldEquippable { slot, on_interact: rules.on_interact, allows_clicked });
        }
        if component.is_some_and(|i| stack.removed.contains(&i)) {
            return None;
        }
        let info = info?;
        let slot = worn_slot_of_code(info.equip_slot)?;
        let allows_clicked = info.equip_allowed.is_empty()
            || info.equip_allowed.iter().any(|allowed| match allowed.strip_prefix('#') {
                Some(tag) => self.entity_in_tag(clicked, tag),
                None => allowed.strip_prefix("minecraft:").unwrap_or(allowed) == clicked.kind,
            });
        Some(HeldEquippable { slot, on_interact: info.equip_on_interact, allows_clicked })
    }

    /// The entity's type is in the received entity type tag `tag` (with or without namespace).
    fn entity_in_tag(&self, e: &Entity, tag: &str) -> bool {
        let short = tag.strip_prefix("minecraft:").unwrap_or(tag);
        let Some((_, list)) = self.debug.tags.iter().find(|(registry, _)| registry == "minecraft:entity_type") else {
            return false;
        };
        list.iter()
            .any(|(name, ids)| name.strip_prefix("minecraft:").unwrap_or(name) == short && ids.contains(&e.kind_id))
    }

    /// The item named `item` is in the received item tag `tag`.
    fn item_name_in_tag(&self, item: &str, tag: &str) -> bool {
        let indexed = crate::item_ids::indexed_name_lookups();
        let Some(id) = crate::item_ids::item_id(&self.data, &self.item_names, item, indexed) else { return false };
        crate::menu_special::in_tag(tag, id).unwrap_or(false)
    }

    /// Thunderstorm in this client's world (a world with weather, thunder above 0.9).
    fn world_thundering(&self) -> bool {
        self.env.has_sky && !self.dimension.ends_with("the_end") && self.env.env.weather.thunder() > 0.9
    }

    // ---------- Effects ----------

    fn apply_entity_use_effect(&mut self, hand: usize, id: i32, effect: LocalEffect) {
        match effect {
            LocalEffect::ConsumeHeld => self.use_one_held(hand),
            LocalEffect::FillHeld { result, creative_keeps } => self.fill_held(hand, result, creative_keeps),
            LocalEffect::PlayerSound { name, volume, pitch } => {
                if let Some(a) = self.audio.get() {
                    a.play(name, rc_audio::Category::Player, Some(self.player.pos), volume, pitch, None);
                }
            }
            LocalEffect::EntitySound { name, category, volume, pitch_low, pitch_spread } => {
                let pitch = pitch_low + self.act.rand() as f32 * pitch_spread;
                let at = self.entities.map.get(&id).map(|e| e.pos);
                if let (Some(a), Some(at)) = (self.audio.get(), at) {
                    a.play(name, audio_category(category), Some(at), volume, pitch, None);
                }
            }
            LocalEffect::HappyVillagerParticle => self.spark_on_entity(id),
            LocalEffect::RemoveEntity => remove_entity_locally(&mut self.entities, id),
            LocalEffect::StartAgeLockCooldown => {
                if let Some(uuid) = self.entities.map.get(&id).map(|e| e.uuid) {
                    self.act.age_lock_waits.insert(id, (uuid, self.act.tick + AGE_LOCK_WAIT_TICKS));
                }
            }
            LocalEffect::RememberStewFlower | LocalEffect::ForgetStewFlower => {
                if let Some(e) = self.entities.map.get_mut(&id) {
                    e.stew_flower_fed = effect == LocalEffect::RememberStewFlower;
                }
            }
        }
    }

    /// One "happy villager" spark at a random point of the entity's box, raised by half a
    /// block (the game's spot for a fed young mount).
    fn spark_on_entity(&mut self, id: i32) {
        let Some(e) = self.entities.map.get(&id) else { return };
        let size = e.size(self.dims_table());
        let (w, h) = (f64::from(size.w), f64::from(size.h));
        let pos = e.pos;
        let mut r = || self.act.rand();
        let at = [pos[0] + (2.0 * r() - 1.0) * w, pos[1] + r() * h + 0.5, pos[2] + (2.0 * r() - 1.0) * w];
        self.particles.spawn(PKind::HappyVillager, &POptions::default(), at, [0.0; 3]);
    }

    /// Held stack exchanged for a filled one (the game's filled-result rule): with infinite
    /// materials and `creative_keeps`, the hand stays and the result is added once; otherwise
    /// one unit used, then the result in the emptied hand or in the inventory.
    fn fill_held(&mut self, hand: usize, result: &str, creative_keeps: bool) {
        let indexed = crate::item_ids::indexed_name_lookups();
        let Some(item) = crate::item_ids::item_id(&self.data, &self.item_names, result, indexed) else { return };
        let filled = ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() };
        let infinite = self.world_sounds.instabuild;
        if creative_keeps && infinite {
            if !self.inventory.menu.iter().flatten().any(|s| s.item == item) {
                self.add_to_inventory(filled);
            }
            return;
        }
        self.use_one_held(hand);
        let slot = self.held_menu_slot(hand);
        match self.inventory.menu.get_mut(slot) {
            Some(held @ None) => *held = Some(filled),
            _ => self.add_to_inventory(filled),
        }
    }

    /// An unstackable stack into the first empty inventory slot (hot bar first), like the
    /// game's inventory; nothing when it is full (the server drops it).
    fn add_to_inventory(&mut self, stack: ItemStack) {
        let order = (36..45).chain(9..36);
        let free = order.into_iter().find(|&s| self.inventory.menu.get(s).is_some_and(Option::is_none));
        if let Some(s) = free {
            self.inventory.menu[s] = Some(stack);
        }
    }
}

/// The clicked entity leaves this client's world through the usual removal, so that a vehicle
/// forgets it as a passenger and its own passengers get off, as when the server removes it.
fn remove_entity_locally(entities: &mut crate::entities::Entities, id: i32) {
    entities.apply(rc_client::EntityEvent::Remove(vec![id]));
}

fn audio_category(category: SoundCategory) -> rc_audio::Category {
    match category {
        SoundCategory::Hostile => rc_audio::Category::Hostile,
        SoundCategory::Neutral => rc_audio::Category::Neutral,
        SoundCategory::Players => rc_audio::Category::Player,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client entity of `kind` read as the prediction reads it (nothing worn, unhurt).
    fn seen(entity: &Entity) -> ClientSeen<'_> {
        ClientSeen {
            entity,
            kind: format!("minecraft:{}", entity.kind),
            worn: [None; 8],
            worn_comes_off_with_shears: [false; 8],
            max_health: 10.0,
            leash_holder: LeashHolder::Nobody,
            holds_leashes: false,
            distance_sqr: 4.0,
            age_lock_cooldown: 0,
            fed_a_stew_flower: entity.stew_flower_fed,
        }
    }

    fn pig_food(item: &str, tag: &str) -> bool {
        tag == "minecraft:pig_food" && item == "minecraft:carrot"
    }

    /// The scenario of the gap: a carrot to a grown pig stops the click without a swing (before,
    /// this client went on to the carrot's own use, eating it); to a piglet (young flag synced at
    /// 16), with a swing and one carrot used. The client's entity is read with its namespace.
    #[test]
    fn carrot_to_a_pig_read_from_the_client_entity() {
        let carrot = HeldItem { name: "minecraft:carrot", custom_name: false, dye: None, equippable: None };
        let player = ActingPlayer {
            uuid: 1,
            hand: 0,
            sneaking: false,
            infinite_materials: false,
            other_hand_item: "",
            thundering: false,
        };
        let mut pig = crate::debug_render::tests::test_entity("pig");
        pig.meta.insert(9, Meta::Float(10.0));
        let grown = prediction::predict(&seen(&pig), &carrot, &player, &pig_food);
        assert_eq!(grown.outcome, Outcome::Success { swing: false });
        assert!(grown.effects.is_empty());
        pig.meta.insert(16, Meta::Bool(true));
        let young = prediction::predict(&seen(&pig), &carrot, &player, &pig_food);
        assert_eq!(young.outcome, Outcome::Success { swing: true });
        assert_eq!(young.effects, vec![LocalEffect::ConsumeHeld]);
    }

    /// An axolotl sitting in a boat taken into a bucket: the boat no longer carries it. Fails on
    /// the code before, which dropped the axolotl from the table and left the boat a ghost
    /// passenger until the next passengers packet.
    #[test]
    fn bucketed_passenger_leaves_its_boat() {
        let mut all = crate::entities::Entities {
            type_names: vec!["minecraft:oak_boat".into(), "minecraft:axolotl".into()],
            ..Default::default()
        };
        for (id, kind) in [(1, 0), (2, 1)] {
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid: 0,
                kind,
                pos: [0.0; 3],
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            all.apply(rc_client::EntityEvent::Add(Box::new(add)));
        }
        all.apply(rc_client::EntityEvent::Passengers { vehicle: 1, passengers: vec![2] });
        remove_entity_locally(&mut all, 2);
        assert!(!all.map.contains_key(&2), "the axolotl is still in the world");
        assert!(all.map[&1].passengers.is_empty(), "the boat still carries the bucketed axolotl");
    }

    /// Equipment slot codes of the component (id + 1) to the slots of `set_equipment`.
    #[test]
    fn equippable_slot_codes_to_worn_slots() {
        let worn: Vec<_> = (0..=9).map(worn_slot_of_code).collect();
        let want = [None, Some(0), Some(2), Some(3), Some(4), Some(5), Some(1), Some(6), Some(7), None];
        assert_eq!(worn, want);
    }
}
