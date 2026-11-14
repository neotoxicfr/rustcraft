//! The species' own reactions to a right click, client side (mounts: `mounts.rs`).

use super::{CONSUME, Click, LocalEffect, Res, SUCCESS, SoundCategory, slot};
use crate::mobs::species::Kind;

/// Mob that a water bucket (or, for the sulfur cube, an empty one) takes away, and its bucket.
fn bucket_of(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Cod => "minecraft:cod_bucket",
        Kind::Salmon => "minecraft:salmon_bucket",
        Kind::Pufferfish => "minecraft:pufferfish_bucket",
        Kind::TropicalFish => "minecraft:tropical_fish_bucket",
        Kind::Axolotl => "minecraft:axolotl_bucket",
        Kind::Tadpole => "minecraft:tadpole_bucket",
        Kind::SulfurCube => "minecraft:sulfur_cube_bucket",
        _ => return None,
    })
}

impl Click<'_> {
    /// The clicked mob's own reaction, by species.
    pub(super) fn species(&mut self) -> Res {
        let Some(kind) = self.kind else { return Res::Pass };
        match kind {
            Kind::Cow => self.milked("minecraft:entity.cow.milk"),
            Kind::Goat => {
                let sound = if self.bool_at(18) {
                    "minecraft:entity.goat.screaming.milk"
                } else {
                    "minecraft:entity.goat.milk"
                };
                self.milked(sound)
            }
            Kind::Mooshroom => self.mooshroom(),
            Kind::Sheep if self.holds("minecraft:shears") => CONSUME,
            Kind::Pig | Kind::Strider => self.saddled_walker(),
            Kind::Creeper => self.creeper(),
            Kind::Armadillo => self.armadillo(),
            Kind::Axolotl => self.bucket_pickup().unwrap_or_else(|| self.animal()),
            Kind::Cod | Kind::Salmon | Kind::Pufferfish | Kind::TropicalFish => {
                self.bucket_pickup().unwrap_or(Res::Pass)
            }
            Kind::Tadpole => self.tadpole(),
            Kind::Bee => self.bee(),
            Kind::Dolphin => self.dolphin(),
            Kind::IronGolem => self.iron_golem(),
            Kind::SnowGolem => self.sheared_if(self.byte_at(16) & 16 != 0),
            Kind::Bogged => self.sheared_if(!self.bool_at(16)),
            Kind::CopperGolem => self.copper_golem(),
            Kind::Allay => self.allay(),
            Kind::Wolf => self.wolf(),
            Kind::Cat => self.cat(),
            Kind::Parrot => self.parrot(),
            Kind::Ocelot => self.ocelot(),
            Kind::Villager => self.villager(),
            Kind::WanderingTrader => {
                if self.alive() && !self.baby() {
                    CONSUME
                } else {
                    self.ageable()
                }
            }
            Kind::ZombieVillager if self.holds("minecraft:golden_apple") => CONSUME,
            Kind::Piglin => self.piglin(),
            Kind::Panda => self.panda(),
            Kind::SulfurCube => self.sulfur_cube(),
            k if super::mounts::is_mount(k) => self.mount(),
            k if k.is_animal() => self.animal(),
            k if k.ageable() => self.ageable(),
            _ => Res::Pass,
        }
    }

    // ---------- Shared steps ----------

    /// An animal fed: a growing young one eats (swing), a grown or age-locked one takes the
    /// click without anything seen (the server decides on love); otherwise the golden dandelion.
    pub(super) fn animal(&mut self) -> Res {
        if self.is_food() {
            if self.can_age_up() {
                self.use_player_item();
                return SUCCESS;
            }
            return CONSUME;
        }
        self.ageable()
    }

    /// Golden dandelion on a young mob that can be age-locked, once its 40 ticks have passed.
    pub(super) fn ageable(&mut self) -> Res {
        let lockable = self.kind.is_some_and(|k| !k.cannot_be_age_locked());
        if self.holds("minecraft:golden_dandelion") && self.baby() && self.seen.age_lock_cooldown() == 0 && lockable {
            self.use_golden_dandelion();
            return SUCCESS;
        }
        Res::Pass
    }

    fn use_golden_dandelion(&mut self) {
        self.consume_held();
        self.effects.push(LocalEffect::StartAgeLockCooldown);
    }

    /// The mob's use of the player's item: one unit, except the axolotl's tropical fish bucket
    /// that leaves a water bucket.
    pub(super) fn use_player_item(&mut self) {
        if self.kind == Some(Kind::Axolotl) && self.holds("minecraft:tropical_fish_bucket") {
            self.fill_held("minecraft:water_bucket", true);
        } else {
            self.consume_held();
        }
    }

    /// Taken away in its bucket: the hand gets the filled bucket and the mob leaves at once.
    fn bucket_pickup(&mut self) -> Option<Res> {
        let kind = self.kind?;
        let picked_with = if kind == Kind::SulfurCube { "minecraft:bucket" } else { "minecraft:water_bucket" };
        if !self.holds(picked_with) || !self.alive() {
            return None;
        }
        self.fill_held(bucket_of(kind)?, false);
        self.effects.push(LocalEffect::RemoveEntity);
        Some(SUCCESS)
    }

    /// Shears on a mob ready for them (golems' pumpkin, bogged's mushrooms): the server shears.
    fn sheared_if(&self, ready: bool) -> Res {
        if self.holds("minecraft:shears") && self.alive() && ready { SUCCESS } else { Res::Pass }
    }

    // ---------- Farm animals ----------

    /// Bucket on a grown cow or goat: the actor hears the milking and holds the milk at once.
    fn milked(&mut self, sound: &'static str) -> Res {
        if self.holds("minecraft:bucket") && !self.baby() {
            self.effects.push(LocalEffect::PlayerSound { name: sound, volume: 1.0, pitch: 1.0 });
            self.fill_held("minecraft:milk_bucket", true);
            return SUCCESS;
        }
        self.animal()
    }

    /// Mooshroom: bowl (the client cannot know a suspicious stew, the server corrects it),
    /// shears, a flower fed to a brown one; otherwise a cow's reaction (milk included).
    fn mooshroom(&mut self) -> Res {
        let grown = !self.baby();
        if self.holds("minecraft:bowl") && grown {
            self.fill_held("minecraft:mushroom_stew", false);
            return SUCCESS;
        }
        if self.holds("minecraft:shears") && self.alive() && grown {
            return SUCCESS;
        }
        let brown = self.int_at(18) == 1;
        if brown && grown && crate::mobs::surface::mooshroom::flower_effect(self.item.name).is_some() {
            self.consume_held();
            return SUCCESS;
        }
        self.milked("minecraft:entity.cow.milk")
    }

    /// Pig and strider: mounted when saddled and free, otherwise food, then the saddle.
    fn saddled_walker(&mut self) -> Res {
        let free = self.seen.worn(slot::SADDLE).is_some() && self.seen.passengers() == 0;
        if !self.is_food() && free && !self.player.sneaking {
            return SUCCESS;
        }
        self.animal()
    }

    fn creeper(&mut self) -> Res {
        if !self.in_tag("minecraft:creeper_igniters") {
            return Res::Pass;
        }
        let sound = if self.holds("minecraft:fire_charge") {
            "minecraft:item.firecharge.use"
        } else {
            "minecraft:item.flintandsteel.use"
        };
        self.entity_sound(sound, SoundCategory::Hostile, 1.0, 0.8, 0.4);
        SUCCESS
    }

    /// Armadillo: the brush takes a scute off a grown one; rolled up, it refuses the rest.
    fn armadillo(&mut self) -> Res {
        if self.holds("minecraft:brush") && !self.baby() {
            return SUCCESS;
        }
        let rolled_up = self.int_at(18) != 0;
        if rolled_up { Res::Fail } else { self.animal() }
    }

    fn tadpole(&mut self) -> Res {
        if self.in_tag("minecraft:frog_food") && !self.age_locked() {
            self.consume_held();
            self.effects.push(LocalEffect::HappyVillagerParticle);
            return SUCCESS;
        }
        let lockable = self.kind.is_some_and(|k| !k.cannot_be_age_locked());
        if self.holds("minecraft:golden_dandelion") && self.seen.age_lock_cooldown() == 0 && lockable {
            self.use_golden_dandelion();
            return SUCCESS;
        }
        self.bucket_pickup().unwrap_or(Res::Pass)
    }

    /// Bee: a flower with an effect for it is eaten; the rest is animal food.
    fn bee(&mut self) -> Res {
        if self.is_food() && crate::mobs::wild::bee::flower_effect_for_bees(self.item.name).is_some() {
            self.use_player_item();
            return SUCCESS;
        }
        self.animal()
    }

    fn dolphin(&mut self) -> Res {
        if self.in_tag("minecraft:fishes") {
            self.consume_held();
            return SUCCESS;
        }
        self.ageable()
    }

    /// Iron ingot on a hurt iron golem: repaired (an unhurt one ignores it).
    fn iron_golem(&mut self) -> Res {
        if !self.holds("minecraft:iron_ingot") || !self.hurt() {
            return Res::Pass;
        }
        self.consume_held();
        SUCCESS
    }

    /// Copper golem: an empty hand takes what it holds, shears take its antenna flower; the
    /// rest is the server's (the client lets it pass).
    fn copper_golem(&self) -> Res {
        if self.item.name.is_empty() && self.seen.worn(slot::MAIN_HAND).is_some() {
            return SUCCESS;
        }
        let antenna = self.seen.worn(slot::SADDLE);
        let shearable = antenna.is_some_and(|a| (self.tags)(a, "minecraft:shearable_from_copper_golem"));
        if self.holds("minecraft:shears") && shearable { SUCCESS } else { Res::Pass }
    }

    /// Allay: duplicated while dancing, given an item when its hand is empty, gives it back to
    /// an empty main hand.
    fn allay(&mut self) -> Res {
        let holds_item = self.seen.worn(slot::MAIN_HAND).is_some();
        let dancing = self.bool_at(16);
        let can_duplicate = self.bool_at(17);
        if dancing && self.in_tag("minecraft:duplicates_allays") && can_duplicate {
            self.entity_sound("minecraft:block.amethyst_block.chime", SoundCategory::Neutral, 2.0, 1.0, 0.0);
            self.consume_held();
            return SUCCESS;
        }
        if !holds_item && !self.item.name.is_empty() {
            self.consume_held();
            self.entity_sound("minecraft:entity.allay.item_given", SoundCategory::Neutral, 2.0, 1.0, 0.0);
            return SUCCESS;
        }
        if holds_item && self.player.hand == 0 && self.item.name.is_empty() {
            self.entity_sound("minecraft:entity.allay.item_taken", SoundCategory::Neutral, 2.0, 1.0, 0.0);
            return SUCCESS;
        }
        Res::Pass
    }

    // ---------- Pets ----------

    /// Wolf: a tamed one is healed, gets its armour or collar, sits or stands for its owner;
    /// the taming bone is the server's.
    fn wolf(&mut self) -> Res {
        if !self.tame() {
            return self.animal();
        }
        if self.is_food() && self.hurt() {
            self.use_player_item();
            return SUCCESS;
        }
        let owned = self.owned_by_player();
        if !self.in_tag("minecraft:wolf_collar_dyes") || !owned {
            if self.equippable_in(slot::BODY) && self.seen.worn(slot::BODY).is_none() && owned && !self.baby() {
                self.consume_held();
                return SUCCESS;
            }
            let fed = self.animal();
            return if !fed.consumes() && owned { SUCCESS } else { fed };
        }
        let collar = self.int_at(21);
        if self.item.dye.is_some_and(|c| i32::from(c) != collar) {
            self.consume_held();
            return SUCCESS;
        }
        self.animal()
    }

    /// Cat: its owner dyes its collar, heals it, or makes it sit or stand; a wild one is fed to
    /// be tamed (server side).
    fn cat(&mut self) -> Res {
        if self.tame() && self.owned_by_player() {
            if self.in_tag("minecraft:cat_collar_dyes") {
                if self.item.dye.is_some_and(|c| i32::from(c) != self.int_at(23)) {
                    return SUCCESS;
                }
            } else if self.is_food() && self.hurt() {
                return SUCCESS;
            }
            let fed = self.animal();
            return if fed.consumes() { fed } else { SUCCESS };
        }
        if !self.tame() && self.is_food() {
            return SUCCESS;
        }
        self.animal()
    }

    /// Parrot: seeds offered to a wild one, its owner makes a perched one sit, a cookie kills it.
    fn parrot(&mut self) -> Res {
        if !self.tame() && self.in_tag("minecraft:parrot_food") {
            self.use_player_item();
            return SUCCESS;
        }
        if self.in_tag("minecraft:parrot_poisonous_food") {
            self.use_player_item();
            return SUCCESS;
        }
        let flying = !self.seen.on_ground();
        if !flying && self.tame() && self.owned_by_player() { SUCCESS } else { self.animal() }
    }

    /// Ocelot: food offered close by (under 3 blocks) while it does not trust the player yet.
    fn ocelot(&mut self) -> Res {
        let trusting = self.bool_at(18);
        if !trusting && self.is_food() && self.seen.distance_sqr() < 9.0 {
            self.use_player_item();
            return SUCCESS;
        }
        self.animal()
    }

    // ---------- Villagers and Nether ----------

    /// Villager: an awake, living one takes the click (trading, refusal, a young one's shake).
    fn villager(&mut self) -> Res {
        let sleeping = matches!(self.seen.data(6), Some(rc_proto_v777::entity::Meta::Pose(2)));
        if !self.alive() || sleeping {
            return self.ageable();
        }
        SUCCESS
    }

    /// Piglin: a grown one not admiring anything yet takes gold.
    fn piglin(&self) -> Res {
        let dancing = self.bool_at(19);
        let admiring =
            !dancing && self.seen.worn(slot::OFF_HAND).is_some_and(|o| (self.tags)(o, "minecraft:piglin_loved"));
        if !self.baby() && self.holds("minecraft:gold_ingot") && !admiring { SUCCESS } else { Res::Pass }
    }

    /// Panda: a worried one refuses everything in a thunderstorm; one on its back gets up;
    /// bamboo makes a young one grow (the rest of its food is the server's).
    fn panda(&mut self) -> Res {
        let worried = self.byte_at(21) == 2;
        if worried && self.player.thundering {
            return Res::Pass;
        }
        if self.byte_at(23) & 16 != 0 {
            return SUCCESS;
        }
        if self.is_food() {
            if !self.can_age_up() {
                return Res::Pass;
            }
            self.use_player_item();
            return CONSUME;
        }
        if self.baby() && self.holding_in_either_hand("minecraft:golden_dandelion") { self.animal() } else { Res::Pass }
    }

    /// Sulfur cube: a young one eats; a grown one is sheared of what it swallowed, swallows an
    /// item, or goes into an empty bucket. Its ignition depends on what it swallowed, which this
    /// client does not work out: that click falls to the item.
    fn sulfur_cube(&mut self) -> Res {
        if self.baby() {
            if self.in_tag("minecraft:sulfur_cube_food") && self.can_age_up() {
                self.use_player_item();
                return SUCCESS;
            }
            return self.ageable();
        }
        let swallowed = self.seen.worn(slot::BODY);
        if self.holds("minecraft:shears") && swallowed.is_some() {
            return SUCCESS;
        }
        if self.in_tag("minecraft:sulfur_cube_swallowable") {
            if swallowed == Some(self.item.name) {
                return Res::Pass;
            }
            self.consume_held();
            return CONSUME;
        }
        self.bucket_pickup().unwrap_or_else(|| self.ageable())
    }
}
