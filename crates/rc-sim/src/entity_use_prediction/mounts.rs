//! Mounts' reactions to a right click, client side: horses and their kin, camels, the happy
//! ghast and the nautiluses.

use super::{CONSUME, Click, LocalEffect, Res, SUCCESS, slot};
use crate::mobs::species::Kind;

/// Species ridden through this file's rules.
pub(super) fn is_mount(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Horse
            | Kind::Donkey
            | Kind::Mule
            | Kind::Llama
            | Kind::TraderLlama
            | Kind::SkeletonHorse
            | Kind::ZombieHorse
            | Kind::Camel
            | Kind::CamelHusk
            | Kind::HappyGhast
            | Kind::Nautilus
            | Kind::ZombieNautilus
    )
}

/// A mount that carries a chest once given one.
fn takes_a_chest(kind: Kind) -> bool {
    matches!(kind, Kind::Donkey | Kind::Mule | Kind::Llama | Kind::TraderLlama)
}

impl Click<'_> {
    pub(super) fn mount(&mut self) -> Res {
        match self.kind {
            Some(Kind::Camel | Kind::CamelHusk) => self.camel(),
            Some(Kind::HappyGhast) => self.happy_ghast(),
            Some(Kind::Nautilus | Kind::ZombieNautilus) => self.nautilus(),
            Some(Kind::SkeletonHorse) if !self.horse_tamed() => Res::Pass,
            Some(Kind::SkeletonHorse) => self.horse_common(),
            _ => self.horse(),
        }
    }

    /// Horse, zombie horse, donkey, mule, llama: food, anger of an untamed one, a chest, then
    /// what every horse does.
    fn horse(&mut self) -> Res {
        let opens_inventory = !self.baby() && self.horse_tamed() && self.player.sneaking;
        let dandelion_on_young = self.kind != Some(Kind::ZombieHorse)
            && self.baby()
            && self.holding_in_either_hand("minecraft:golden_dandelion");
        let free = self.seen.passengers() == 0 && !opens_inventory && !dandelion_on_young;
        if free && !self.item.name.is_empty() {
            if self.is_food() {
                return self.fed_food();
            }
            if !self.horse_tamed() {
                return SUCCESS;
            }
            let has_chest = self.bool_at(19);
            if self.kind.is_some_and(takes_a_chest) && !has_chest && self.holds("minecraft:chest") {
                self.consume_held();
                return SUCCESS;
            }
        }
        self.horse_common()
    }

    /// Every horse: a ridden or young one is an animal; its owner sneaking opens its inventory;
    /// the held item is put on (saddle, then armour or carpet); otherwise the player mounts.
    fn horse_common(&mut self) -> Res {
        if self.seen.passengers() > 0 || self.baby() {
            return self.animal();
        }
        if self.horse_tamed() && self.player.sneaking {
            return SUCCESS;
        }
        if !self.item.name.is_empty() {
            let put_on = self.held_item_on_living();
            if put_on.consumes() {
                return put_on;
            }
            if self.equippable_in(slot::BODY) && self.seen.worn(slot::BODY).is_none() {
                self.consume_held();
                return SUCCESS;
            }
        }
        SUCCESS
    }

    /// A meal for a mount: on this client it heals a hurt one (the stack is then used) and a
    /// growing young one shows a spark (a camel's growing also uses the stack); the server
    /// decides the rest, without a swing here.
    fn fed_food(&mut self) -> Res {
        let Some(kind) = self.kind else { return CONSUME };
        let camel = crate::mobs::mounts::is_camel(kind);
        // The server's own table: what the client predicts cannot drift from what it does.
        let meal = crate::mobs::mounts::feed::mount_meal(kind, self.item.name);
        let healed = self.hurt() && meal.heal > 0.0;
        let sparks =
            if camel { self.can_age_up() } else { self.baby() && meal.grow_ticks > 0 && !self.age_locked() };
        if sparks {
            self.effects.push(LocalEffect::HappyVillagerParticle);
        }
        if healed || (camel && sparks) {
            self.consume_held();
        }
        CONSUME
    }

    /// Camel: sneaking opens a grown one's inventory, the saddle goes on, food, two seats, and
    /// the golden dandelion on a young one; anything else fails.
    fn camel(&mut self) -> Res {
        if self.player.sneaking && !self.baby() {
            return SUCCESS;
        }
        let put_on = self.held_item_on_living();
        if put_on.consumes() {
            return put_on;
        }
        if self.is_food() {
            return self.fed_food();
        }
        if self.seen.passengers() < 2 && !self.baby() {
            return CONSUME;
        }
        if self.baby() && self.holding_in_either_hand("minecraft:golden_dandelion") { self.animal() } else { Res::Fail }
    }

    /// Happy ghast: a grown one gets its harness and carries the player when harnessed.
    fn happy_ghast(&mut self) -> Res {
        if self.baby() {
            return self.animal();
        }
        if !self.item.name.is_empty() {
            let put_on = self.held_item_on_living();
            if put_on.consumes() {
                return put_on;
            }
        }
        if self.seen.worn(slot::BODY).is_some() && !self.player.sneaking { SUCCESS } else { self.animal() }
    }

    /// Nautilus: a tamed one's inventory, healing with its food, its gear, then a ride.
    fn nautilus(&mut self) -> Res {
        if self.baby() {
            return self.animal();
        }
        if self.tame() && self.player.sneaking {
            return SUCCESS;
        }
        if !self.item.name.is_empty() {
            if self.is_food() && self.hurt() {
                self.use_player_item();
                return SUCCESS;
            }
            let put_on = self.held_item_on_living();
            if put_on.consumes() {
                return put_on;
            }
        }
        if self.tame() && !self.player.sneaking && !self.is_food() { SUCCESS } else { self.animal() }
    }
}
