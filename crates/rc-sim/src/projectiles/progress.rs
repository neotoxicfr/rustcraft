//! Statistics and advancements (S9) of projectiles, fishing and maps: item used,
//! fish caught, target hit, damage dealt at range; triggers `shot_crossbow`,
//! `fishing_rod_hooked`, `target_hit`, `channeled_lightning`, `killed_by_arrow`,
//! `player_hurt_entity`.

use serde_json::Value;

use crate::commands::ents::E;
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::progress::pred::{Ctx, Dmg, Who};
use crate::projectiles::Proj;
use crate::server::Server;

impl Server {
    fn s8_who(&self, e: Ent) -> Who<'static> {
        match e {
            Ent::Player => Who::E(E::Player),
            Ent::Mob(id) => Who::E(E::Mob(id)),
        }
    }

    fn s8_entity_ok(&self, cond: Option<&Value>, w: Option<Who<'_>>) -> bool {
        match cond {
            None | Some(Value::Null) => true,
            Some(c) => w.is_some_and(|w| {
                let ctx = Ctx { this: Some(w), origin: Some(self.pos), ..Ctx::default() };
                self.p_context(c, &ctx)
            }),
        }
    }

    fn s8_item_ok(&self, cond: Option<&Value>, s: Option<&Stack>) -> bool {
        cond.is_none_or(|p| p.is_null() || self.p_item(p, s))
    }

    /// "Used" statistic of an item (throwing weapon, thrown item, rod, map).
    pub(crate) fn s8_used(&mut self, id: &str) {
        self.award_stat("minecraft:used", id, 1);
    }

    /// `shot_crossbow`.
    pub(crate) fn s8_shot_crossbow(&mut self, w: &Stack) {
        if self.trig_active("minecraft:shot_crossbow") {
            self.trigger("minecraft:shot_crossbow", &|sv, c| sv.s8_item_ok(c.get("item"), Some(w)));
        }
    }

    /// `fishing_rod_hooked`: rod, hooked entity, fished items (only one must match).
    pub(crate) fn s8_rod_hooked(&mut self, rod: &Stack, hooked: Option<Ent>, items: &[Stack]) {
        for s in items {
            if self.life.items.has_tag("minecraft:fishes", &s.id) {
                self.award_custom("fish_caught", 1);
            }
        }
        if !self.trig_active("minecraft:fishing_rod_hooked") {
            return;
        }
        self.trigger("minecraft:fishing_rod_hooked", &|sv, c| {
            let who = hooked.map(|e| sv.s8_who(e));
            sv.s8_item_ok(c.get("rod"), Some(rod))
                && sv.s8_entity_ok(c.get("entity"), who)
                && match c.get("item") {
                    None | Some(Value::Null) => true,
                    Some(p) => items.iter().any(|s| sv.p_item(p, Some(s))),
                }
        });
    }

    /// Target hit by a player's projectile: statistic and `target_hit` (signal, projectile
    /// seen as an entity, origin = the player).
    pub(crate) fn s8_target_hit(&mut self, signal: i32, p: &Proj) {
        self.award_custom("target_hit", 1);
        self.trig_target_hit(Who::Proj(p), signal);
    }

    /// `channeled_lightning`: each victim condition must target one of the victims.
    pub(crate) fn s8_channeled(&mut self, victims: &[Ent]) {
        if !self.trig_active("minecraft:channeled_lightning") {
            return;
        }
        let v = victims.to_vec();
        self.trigger("minecraft:channeled_lightning", &|sv, c| {
            c.get("victims").and_then(Value::as_array).is_none_or(|conds| {
                conds.iter().all(|p| v.iter().any(|&e| sv.s8_entity_ok(Some(p), Some(sv.s8_who(e)))))
            })
        });
    }

    /// `killed_by_arrow`: victims killed by a player's arrow (originating weapon). The list
    /// may be empty (piercing arrow that hurts without killing): the trigger fires anyway.
    /// A victim already removed from the world (more than 20 ticks after its death) is no longer seen.
    pub(crate) fn s8_killed_by_arrow(&mut self, victims: &[Ent], weapon: Option<&Stack>) {
        if !self.trig_active("minecraft:killed_by_arrow") {
            return;
        }
        let who: Vec<Who<'_>> = victims
            .iter()
            .filter(|&&e| match e {
                Ent::Player => true,
                Ent::Mob(id) => self.mobs.mob_ref_any(id).is_some(),
            })
            .map(|&e| self.s8_who(e))
            .collect();
        self.trig_killed_by_arrow(&who, weapon);
    }

    /// Ranged damage dealt by the player: statistic and `player_hurt_entity`.
    pub(crate) fn s8_player_hurt_by_projectile(&mut self, victim: Ent, ty: &str, original: f32, dealt: f32) {
        if dealt > 0.0 {
            self.award_custom("damage_dealt", (dealt * 10.0).round() as i32);
        }
        let Ent::Mob(id) = victim else { return };
        let Some(m) = self.mob_ref(id).map(|m| (*m).clone()) else { return };
        let dmg = Dmg { ty: ty.to_owned(), source: Some(Who::E(E::Player)), direct: None, pos: Some(self.pos) };
        self.trig_player_hurt_entity(Some(Who::Mob(&m)), &dmg, original, dealt, false);
    }
}
