//! Right click on an entity, as the game's client works it out before the server answers.
//!
//! The game's client runs the clicked entity's own reaction on its copy of the entity, then the
//! held item's: the outcome decides whether the click stops there (and whether the arm swings)
//! or goes on to the plain use of the held item, and the reaction's client side already changes
//! a few things (a sound only the actor hears, the held stack used or filled, a fish gone into
//! its bucket). This module is that client side, read from what the client knows of the entity
//! (its synced data, worn items, leash) with the server's own tables: species names and food
//! tags (`mobs::species`), flowers of the stews and of the bees.

mod mobs;
mod mounts;
#[cfg(test)]
mod tests;

use rc_proto_v777::entity::Meta;

use crate::mobs::species::Kind;

/// Network equipment slots (`set_equipment`): main hand, off hand, feet, legs, chest, head,
/// body, saddle.
pub mod slot {
    pub const MAIN_HAND: usize = 0;
    pub const OFF_HAND: usize = 1;
    pub const HEAD: usize = 5;
    pub const BODY: usize = 6;
    pub const SADDLE: usize = 7;
}

/// What a right click on the entity does on the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing done: the held item's plain use comes next.
    Pass,
    /// The click stops here; `swing`: the client swings the arm itself (otherwise the server
    /// may, or nobody).
    Success { swing: bool },
}

/// A change the game's client makes at once, before the server's answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LocalEffect {
    /// One unit of the held stack used (none for a player with infinite materials).
    ConsumeHeld,
    /// The held stack exchanged for a filled one (milk, stew, a mob in its bucket): one unit
    /// used and the result in the hand when it was the last one, otherwise in the inventory.
    /// With `creative_keeps`, infinite materials keep the hand as it is and add the result
    /// to the inventory once.
    FillHeld { result: &'static str, creative_keeps: bool },
    /// A sound the acting player hears from their own client (the server leaves them out),
    /// at the player.
    PlayerSound { name: &'static str, volume: f32, pitch: f32 },
    /// A sound at the clicked entity that only the actor's client plays, with a pitch drawn in
    /// `[pitch_low, pitch_low + pitch_spread)`.
    EntitySound { name: &'static str, category: SoundCategory, volume: f32, pitch_low: f32, pitch_spread: f32 },
    /// One "happy villager" particle in the clicked entity's box (a young mount or tadpole fed).
    HappyVillagerParticle,
    /// The clicked entity leaves this client's world at once (picked up in a bucket).
    RemoveEntity,
    /// The golden dandelion's 40-tick wait starts for this entity on this client.
    StartAgeLockCooldown,
    /// This client fed the clicked brown mooshroom a stew flower: kept on its copy of the entity.
    RememberStewFlower,
    /// The clicked mooshroom's stew was milked: the flower fed by this client is used up.
    ForgetStewFlower,
}

/// Sound categories of the sounds a click plays on the actor's client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCategory {
    Hostile,
    Neutral,
    Players,
}

/// The client's verdict on a right click on an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    pub outcome: Outcome,
    pub effects: Vec<LocalEffect>,
}

/// Holder of the clicked entity's leash, as the client knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeashHolder {
    Nobody,
    ActingPlayer,
    OtherPlayer,
    OtherEntity,
}

/// The clicked entity as the client holds it.
pub trait SeenEntity {
    /// Type name, `minecraft:` included.
    fn kind(&self) -> &str;
    /// Last synced value at `index`.
    fn data(&self, index: u8) -> Option<&Meta>;
    /// Item worn in network slot `slot` (see [`slot`]), `None` when empty.
    fn worn(&self, slot: usize) -> Option<&str>;
    /// The item worn in `slot` comes off with shears (`equippable.can_be_sheared`).
    fn worn_comes_off_with_shears(&self, slot: usize) -> bool;
    /// Maximum health (synced attribute).
    fn max_health(&self) -> f32;
    fn passengers(&self) -> usize;
    fn on_ground(&self) -> bool;
    fn leash_holder(&self) -> LeashHolder;
    /// Some entity's leash is tied to this one.
    fn holds_leashes(&self) -> bool;
    /// Squared distance between the acting player and the entity.
    fn distance_sqr(&self) -> f64;
    /// Ticks this client still waits before a golden dandelion works again on it.
    fn age_lock_cooldown(&self) -> i32;
    /// This client fed it a stew flower its stew has not used yet (a brown mooshroom). The game's
    /// client keeps that on its own copy of the entity: a flower fed by another player, or before
    /// the entity was last sent to this client, is unknown to it.
    fn fed_a_stew_flower(&self) -> bool;
}

/// The `equippable` component of the held stack, as it applies to the clicked entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldEquippable {
    /// Network slot it goes to.
    pub slot: usize,
    /// A right click puts it on the entity.
    pub on_interact: bool,
    /// The clicked entity's type is among the allowed ones.
    pub allows_clicked: bool,
}

/// The stack in the clicking hand.
pub struct HeldItem<'a> {
    /// Item name, empty for a bare hand.
    pub name: &'a str,
    /// It carries a custom name (a name tag that names).
    pub custom_name: bool,
    /// Its dye colour (game order), when it has one.
    pub dye: Option<u8>,
    pub equippable: Option<HeldEquippable>,
}

/// The player who clicks.
pub struct ActingPlayer<'a> {
    pub uuid: u128,
    /// 0 main hand, 1 off hand.
    pub hand: usize,
    pub sneaking: bool,
    /// Creative or spectator mode with infinite materials (`instabuild`).
    pub infinite_materials: bool,
    /// Item in the other hand (a golden dandelion counts in either hand).
    pub other_hand_item: &'a str,
    /// It thunders in the client's world (a worried panda then refuses everything).
    pub thundering: bool,
}

/// Item tags as the client received them: `in_tag(item, tag)`, names with namespace.
pub type ItemTags<'a> = dyn Fn(&str, &str) -> bool + 'a;

/// Right click with `item` on `seen`: what the game's client does before the server answers.
pub fn predict(seen: &dyn SeenEntity, item: &HeldItem, player: &ActingPlayer, tags: &ItemTags) -> Prediction {
    let mut click = Click { seen, item, player, tags, kind: Kind::from_name(seen.kind()), effects: Vec::new() };
    let own = click.entity_reaction();
    let outcome = match own {
        Res::Success { swing } => Outcome::Success { swing },
        Res::Fail | Res::Pass if !item.name.is_empty() && click.living() => click.held_item_on_living().outcome(),
        _ => Outcome::Pass,
    };
    Prediction { outcome, effects: click.effects }
}

/// Result of one step of a reaction, the game's three kinds: only a success stops the click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Res {
    Success { swing: bool },
    Fail,
    Pass,
}

/// The game's swinging success.
const SUCCESS: Res = Res::Success { swing: true };
/// The game's successes the client does not swing for (server swing, or none).
const CONSUME: Res = Res::Success { swing: false };

impl Res {
    fn consumes(self) -> bool {
        matches!(self, Res::Success { .. })
    }

    fn outcome(self) -> Outcome {
        match self {
            Res::Success { swing } => Outcome::Success { swing },
            _ => Outcome::Pass,
        }
    }
}

/// One right click being worked out.
struct Click<'a> {
    seen: &'a dyn SeenEntity,
    item: &'a HeldItem<'a>,
    player: &'a ActingPlayer<'a>,
    tags: &'a ItemTags<'a>,
    /// Species, for a simulated mob.
    kind: Option<Kind>,
    effects: Vec<LocalEffect>,
}

impl Click<'_> {
    /// The entity's own reaction, by family.
    fn entity_reaction(&mut self) -> Res {
        match self.short_kind() {
            "item_frame" | "glow_item_frame" => self.item_frame(),
            "armor_stand" => self.armor_stand(),
            "cushion" => self.cushion(),
            "interaction" => {
                let responds = self.bool_at(10); // synced index 10: its `response` field
                if responds {
                    SUCCESS
                } else {
                    CONSUME
                }
            }
            "leash_knot" => SUCCESS,
            k if k.ends_with("_boat") || k.ends_with("_raft") => self.boat(k.contains("chest")),
            _ if self.kind.is_some() => self.mob(),
            _ => self.base(),
        }
    }

    fn short_kind(&self) -> &str {
        let k = self.seen.kind();
        k.strip_prefix("minecraft:").unwrap_or(k)
    }

    // ---------- What every entity does ----------

    /// The reaction common to every entity: shears cut its leashes, take a mob's shearable
    /// equipment off, the leash held by the player is untied, a lead ties it (server side).
    fn base(&mut self) -> Res {
        let leashable =
            self.kind.is_some() || self.short_kind().ends_with("_boat") || self.short_kind().ends_with("_raft");
        let leashed = leashable && self.seen.leash_holder() != LeashHolder::Nobody;
        if self.holds("minecraft:shears") && (leashed || self.seen.holds_leashes()) {
            return SUCCESS;
        }
        if self.kind.is_some()
            && self.holds("minecraft:shears")
            && !self.player.sneaking
            && self.shears_take_equipment()
        {
            return SUCCESS;
        }
        if !leashable || !self.alive() {
            return Res::Pass;
        }
        match self.seen.leash_holder() {
            LeashHolder::ActingPlayer => SUCCESS,
            LeashHolder::OtherPlayer => Res::Pass,
            _ if self.holds("minecraft:lead") => CONSUME,
            _ => Res::Pass,
        }
    }

    /// Shears on a mob: something it wears comes off (its owner only for a wolf, never while
    /// it carries a passenger).
    fn shears_take_equipment(&self) -> bool {
        let may = match self.kind {
            Some(Kind::Wolf) => self.owned_by_player(),
            _ => self.seen.passengers() == 0,
        };
        may && (0..8).any(|s| self.seen.worn(s).is_some() && self.seen.worn_comes_off_with_shears(s))
    }

    /// A simulated mob: dead does nothing; a name tag that names, then a spawn egg come first;
    /// then the common reaction, then the species' own.
    fn mob(&mut self) -> Res {
        if !self.alive() {
            return Res::Pass;
        }
        if self.holds("minecraft:name_tag") && self.item.custom_name {
            return SUCCESS;
        }
        if self.item.name.ends_with("_spawn_egg") {
            return CONSUME;
        }
        let base = self.base();
        if base != Res::Pass {
            return base;
        }
        let own = self.species();
        if own.consumes() { own } else { Res::Pass }
    }

    /// The held item's own reaction on a living entity, after the entity did nothing: put on
    /// (saddle, harness), a dye on a sheep, a name tag.
    fn held_item_on_living(&mut self) -> Res {
        if let Some(e) = self.item.equippable
            && e.on_interact
            && self.equippable_in(e.slot)
            && self.seen.worn(e.slot).is_none()
            && self.alive()
        {
            return SUCCESS;
        }
        if self.kind == Some(Kind::Sheep)
            && self.alive()
            && !self.sheep_sheared()
            && self.item.dye.is_some_and(|c| c != self.sheep_colour())
        {
            self.entity_sound("minecraft:item.dye.use", SoundCategory::Players, 1.0, 1.0, 0.0);
            return SUCCESS;
        }
        let nameable = self.short_kind() != "player";
        if self.holds("minecraft:name_tag") && self.item.custom_name && nameable {
            return SUCCESS;
        }
        Res::Pass
    }

    /// The held stack may go in `slot` of the clicked entity: its component allows the type,
    /// and the entity may use that slot now.
    fn equippable_in(&self, slot: usize) -> bool {
        self.item.equippable.is_some_and(|e| e.slot == slot && e.allows_clicked) && self.can_use_slot(slot)
    }

    /// Slots a mob may use only in some states: saddles on grown (and tamed) mounts, bodies
    /// of grown ghasts and cubes, nautilus gear on a tamed grown one.
    fn can_use_slot(&self, slot: usize) -> bool {
        let grown = self.alive() && !self.baby();
        match (self.kind, slot) {
            (Some(Kind::Pig | Kind::Strider), slot::SADDLE) => grown,
            (Some(Kind::Donkey | Kind::Mule | Kind::Camel | Kind::CamelHusk), slot::SADDLE) => {
                grown && self.horse_tamed()
            }
            (Some(Kind::HappyGhast | Kind::SulfurCube), slot::BODY) => grown,
            (Some(Kind::Nautilus | Kind::ZombieNautilus), slot::SADDLE | slot::BODY) => grown && self.tame(),
            _ if self.short_kind() == "armor_stand" => slot != slot::BODY && slot != slot::SADDLE,
            _ => true,
        }
    }

    // ---------- Entities that are not mobs ----------

    /// Item frame: an empty frame with an empty hand does nothing, otherwise the server places
    /// or turns the item.
    fn item_frame(&self) -> Res {
        let framed = matches!(self.seen.data(8), Some(Meta::Item(Some(_))));
        if !framed && self.item.name.is_empty() { Res::Pass } else { SUCCESS }
    }

    /// Armor stand: a marker and a name tag get the common reaction, the rest is the server's.
    fn armor_stand(&mut self) -> Res {
        let marker = self.byte_at(15) & 16 != 0;
        if marker || self.holds("minecraft:name_tag") {
            return self.base();
        }
        CONSUME
    }

    /// Cushion: sat on when nobody sits there and the player does not sneak.
    fn cushion(&self) -> Res {
        if self.player.sneaking || self.seen.passengers() > 0 { Res::Pass } else { CONSUME }
    }

    /// Boat or raft: the common reaction, then boarding unless sneaking (a chest boat opens
    /// its chest instead).
    fn boat(&mut self, with_chest: bool) -> Res {
        let base = self.base();
        if base != Res::Pass {
            return base;
        }
        if !self.player.sneaking || with_chest { SUCCESS } else { Res::Pass }
    }

    // ---------- Effects ----------

    fn consume_held(&mut self) {
        self.effects.push(LocalEffect::ConsumeHeld);
    }

    fn fill_held(&mut self, result: &'static str, creative_keeps: bool) {
        self.effects.push(LocalEffect::FillHeld { result, creative_keeps });
    }

    fn entity_sound(
        &mut self,
        name: &'static str,
        category: SoundCategory,
        volume: f32,
        pitch_low: f32,
        pitch_spread: f32,
    ) {
        self.effects.push(LocalEffect::EntitySound { name, category, volume, pitch_low, pitch_spread });
    }

    // ---------- What the client knows ----------

    fn holds(&self, item: &str) -> bool {
        self.item.name == item
    }

    fn holding_in_either_hand(&self, item: &str) -> bool {
        self.item.name == item || self.player.other_hand_item == item
    }

    fn in_tag(&self, tag: &str) -> bool {
        !self.item.name.is_empty() && (self.tags)(self.item.name, tag)
    }

    /// The held item is this species' food (the server's table of food tags; a wild grown
    /// nautilus only takes its taming items).
    fn is_food(&self) -> bool {
        if matches!(self.kind, Some(Kind::Nautilus | Kind::ZombieNautilus)) {
            let taming = !self.tame() && !self.baby();
            return self.in_tag(if taming { "minecraft:nautilus_taming_items" } else { "minecraft:nautilus_food" });
        }
        self.kind.and_then(Kind::food_tag).is_some_and(|t| self.in_tag(t))
    }

    fn living(&self) -> bool {
        self.kind.is_some() || matches!(self.short_kind(), "armor_stand" | "player" | "mannequin")
    }

    /// Alive on this client: synced health above zero.
    fn alive(&self) -> bool {
        !matches!(self.seen.data(9), Some(Meta::Float(h)) if *h <= 0.0)
    }

    fn health(&self) -> f32 {
        match self.seen.data(9) {
            Some(Meta::Float(h)) => *h,
            _ => self.seen.max_health(),
        }
    }

    fn hurt(&self) -> bool {
        self.health() < self.seen.max_health()
    }

    fn bool_at(&self, index: u8) -> bool {
        matches!(self.seen.data(index), Some(Meta::Bool(true)))
    }

    fn byte_at(&self, index: u8) -> i8 {
        match self.seen.data(index) {
            Some(Meta::Byte(b)) => *b,
            _ => 0,
        }
    }

    fn int_at(&self, index: u8) -> i32 {
        match self.seen.data(index) {
            Some(Meta::Int(v) | Meta::Variant(v, _)) => *v,
            _ => 0,
        }
    }

    /// Young: the age flag of the ageing mobs (index 16; 17 for a piglin; a tadpole never).
    fn baby(&self) -> bool {
        match self.kind {
            Some(Kind::Piglin) => self.bool_at(17),
            Some(Kind::Tadpole) => false,
            _ => self.bool_at(16),
        }
    }

    fn age_locked(&self) -> bool {
        self.bool_at(17)
    }

    /// Young and still growing: food speeds it up.
    fn can_age_up(&self) -> bool {
        self.baby() && !self.age_locked() && self.kind != Some(Kind::SkeletonHorse)
    }

    /// Tamed flag of a wolf, cat, parrot or nautilus.
    fn tame(&self) -> bool {
        self.byte_at(18) & 4 != 0
    }

    fn owned_by_player(&self) -> bool {
        matches!(self.seen.data(19), Some(Meta::Entity(Some(u))) if *u == self.player.uuid)
    }

    /// Tamed flag of a horse, donkey, mule, llama or camel.
    fn horse_tamed(&self) -> bool {
        self.byte_at(18) & 2 != 0
    }

    fn sheep_sheared(&self) -> bool {
        self.byte_at(18) & 16 != 0
    }

    fn sheep_colour(&self) -> u8 {
        (self.byte_at(18) & 15) as u8
    }
}
