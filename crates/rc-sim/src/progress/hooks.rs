//! Game events seen by the statistics and advancements (S9): a one-line call from
//! the game code (S2, S4, S6, S7) to these functions, which count like the official game and
//! trigger the corresponding criteria.

use super::pred::{Dmg, Who};
use crate::commands::ents::E;
use crate::mobs::mob::{Ent, Mob};
use crate::player::Stack;
use crate::server::Server;

/// "Interaction" statistic of an opened block (blocks with a menu).
fn interact_stat(block: &str) -> Option<&'static str> {
    let b = block.trim_start_matches("minecraft:");
    Some(match b {
        "chest" => "open_chest",
        "trapped_chest" => "trigger_trapped_chest",
        "ender_chest" => "open_enderchest",
        "barrel" => "open_barrel",
        "furnace" => "interact_with_furnace",
        "blast_furnace" => "interact_with_blast_furnace",
        "smoker" => "interact_with_smoker",
        "crafting_table" => "interact_with_crafting_table",
        "brewing_stand" => "interact_with_brewingstand",
        "beacon" => "interact_with_beacon",
        "dispenser" => "inspect_dispenser",
        "dropper" => "inspect_dropper",
        "hopper" => "inspect_hopper",
        "anvil" | "chipped_anvil" | "damaged_anvil" => "interact_with_anvil",
        "grindstone" => "interact_with_grindstone",
        "smithing_table" => "interact_with_smithing_table",
        "stonecutter" => "interact_with_stonecutter",
        "loom" => "interact_with_loom",
        "cartography_table" => "interact_with_cartography_table",
        "lectern" => "interact_with_lectern",
        "enchanting_table" => return None,
        s if s.ends_with("shulker_box") => "open_shulker_box",
        _ => return None,
    })
}

impl Server {
    /// Tool used to break: "used" statistic if the item is a tool.
    pub(crate) fn s9_tool_used(&mut self, tool: Option<&Stack>) {
        let Some(t) = tool else { return };
        let is_tool =
            self.life.items.id(&t.id).and_then(|i| self.data.items.get(i as usize)).is_some_and(|i| i.tool.is_some());
        if is_tool {
            self.award_stat("minecraft:used", &t.id, 1);
        }
    }

    /// Block broken with the right tool.
    pub(crate) fn s9_block_mined(&mut self, block: &str) {
        self.award_stat("minecraft:mined", block, 1);
    }

    /// Item wear: trigger, "broken" statistic.
    pub(crate) fn s9_item_damaged(&mut self, s: &Stack, new_damage: i32, broke: bool) {
        if new_damage != 0 {
            self.trig_durability(s, new_damage);
        }
        if broke {
            self.award_stat("minecraft:broken", &s.id, 1);
        }
    }

    /// Item picked up: count of the stack on the ground before pickup (even partial, like the game).
    pub(crate) fn s9_picked_up(&mut self, id: &str, count: i32) {
        self.award_stat("minecraft:picked_up", id, count);
    }

    /// Item thrown from the hand or the menu.
    pub(crate) fn s9_dropped(&mut self, s: &Stack) {
        if s.count > 0 {
            self.award_stat("minecraft:dropped", &s.id, s.count);
            self.award_custom("drop", 1);
        }
    }

    /// W8c: components of the stack applied to the placed block entity (contents of the
    /// Shulker box, name, banner patterns, bees, sherds, head profile…), before the reaction
    /// to the block placement.
    pub(crate) fn placed_components(&mut self, pos: [i32; 3], s: &Stack) {
        let op = self.can_use_game_master_blocks();
        if let Some(comps) = &s.components
            && let Some(id) =
                self.block_entity(pos).and_then(|b| b.get("id")).and_then(rc_proto::nbt::Nbt::as_str).map(str::to_owned)
            && let Some(be) = self.block_entity_mut(pos, &id)
        {
            crate::loot::be::apply(be, comps, op);
            self.reload_live_be(pos);
            self.send_block_entity(pos);
        }
    }

    /// Block item placed: `placed_block` (placed cell) and `item_used_on_block` (clicked cell),
    /// stack from before the placement.
    pub(crate) fn s9_block_placed(&mut self, clicked: [i32; 3], pos: [i32; 3], s: &Stack) {
        self.trig_location("minecraft:placed_block", pos, Some(s));
        self.trig_location("minecraft:item_used_on_block", clicked, Some(s));
    }

    /// Block used without an item (menu, bed): `default_block_use` and interaction statistic
    /// if a menu has just opened.
    pub(crate) fn s9_block_used(&mut self, pos: [i32; 3], block: &str, opened: bool) {
        if opened && let Some(st) = interact_stat(block) {
            self.award_custom(st, 1);
        }
        self.trig_default_block_use(pos);
    }

    /// Items crafted (results taken), grid used, recipes, station events.
    pub(crate) fn s9_menu_made(
        &mut self,
        made: &[(Stack, i32)],
        grid: &[Stack],
        crafted: &[usize],
        station: &[crate::stations::Ev],
    ) {
        for (s, n) in made {
            self.award_stat("minecraft:crafted", &s.id, *n);
        }
        let ids: Vec<String> =
            crafted.iter().filter_map(|&i| self.life.recipes.list.get(i).map(|r| r.id.clone())).collect();
        for id in ids {
            self.trig_recipe_crafted("minecraft:recipe_crafted", &id, grid);
        }
        for ev in station {
            if let crate::stations::Ev::Smithed(Some(r)) = ev {
                self.trig_recipe_crafted("minecraft:recipe_crafted", r, grid);
            }
        }
    }

    /// Item consumed: "used" statistic, `consume_item`.
    pub(crate) fn s9_consumed(&mut self, s: &Stack) {
        self.award_stat("minecraft:used", &s.id, 1);
        self.trig_consume(s);
    }

    /// Item used (thrown, bucket, spawn egg…): "used" statistic.
    /// Bottles taken from a brewing stand: contained potion (`potion_contents.potion`).
    pub(crate) fn s9_brewed(&mut self, taken: &[Stack]) {
        for s in taken {
            let potion = crate::potions::potion_of(s);
            self.trig_brewed_potion(potion.as_deref());
        }
    }

    #[allow(dead_code)] // generic item use: no entry point yet
    pub(crate) fn s9_item_used(&mut self, s: &Stack) {
        self.award_stat("minecraft:used", &s.id, 1);
    }

    /// Enchantment at the table: statistic and `enchanted_item`.
    pub(crate) fn s9_enchanted(&mut self, s: &Stack, cost: i32) {
        self.award_custom("enchant_item", 1);
        self.trig_enchanted_item(s, cost);
    }

    /// Player hit on a mob: damage dealt (tenths), weapon used, trigger.
    pub(crate) fn s9_player_hit_mob(&mut self, m: &Mob, original: f32, dealt: f32, weapon: Option<&Stack>) {
        self.s9_player_hit(Some(Who::Mob(m)), original, dealt, weapon);
    }

    /// Current player's hit on a living entity (`None`: another LAN player,
    /// whom the entity predicates do not describe yet).
    pub(crate) fn s9_player_hit(&mut self, victim: Option<Who<'_>>, original: f32, dealt: f32, weapon: Option<&Stack>) {
        if dealt > 0.0 {
            self.award_custom("damage_dealt", (dealt * 10.0).round() as i32);
        }
        if let Some(w) = weapon
            && self.life.items.default_component(w, "minecraft:weapon").is_some()
        {
            self.award_stat("minecraft:used", &w.id, 1);
        }
        let dmg = Dmg {
            ty: "minecraft:player_attack".into(),
            source: Some(Who::E(E::Player)),
            direct: Some(Who::E(E::Player)),
            pos: Some(self.pos),
        };
        self.trig_player_hurt_entity(victim, &dmg, original, dealt, false);
    }

    /// The current player killed another player (`victim`: their score name): statistic,
    /// criteria `totalKillCount`, `playerKillCount`, `teamkill.<colour>` (victim's team),
    /// `killedByTeam.<colour>` (of the victim, killer's team), `player_killed_entity`.
    pub(crate) fn s9_player_killed_player(&mut self, victim: &str) {
        self.award_custom("player_kills", 1);
        self.score_criteria_add("totalKillCount", 1);
        self.score_criteria_add("playerKillCount", 1);
        if !self.cmd.scores.objectives.is_empty() {
            if let Some(c) = self.cmd.scores.team_of(victim).and_then(|t| t.color.clone()) {
                self.score_criteria_add(&format!("teamkill.{c}"), 1);
            }
            if let Some(c) = self.cmd.scores.team_of(&self.name).and_then(|t| t.color.clone()) {
                self.criteria_update(&format!("killedByTeam.{c}"), victim, |v| v.wrapping_add(1));
            }
        }
        let dmg = Dmg {
            ty: "minecraft:player_attack".into(),
            source: Some(Who::E(E::Player)),
            direct: Some(Who::E(E::Player)),
            pos: Some(self.pos),
        };
        self.trig_killed(None, &dmg);
    }

    /// Death of a mob: credit to the player (the reaction to a kill).
    pub(crate) fn s9_mob_died(&mut self, m: &Mob, ty: &str, by: Option<Ent>) {
        if m.last_hurt_by_player <= 0 {
            return;
        }
        let kind = m.kind.info().name.to_owned();
        self.award_stat("minecraft:killed", &kind, 1);
        self.award_custom("mob_kills", 1);
        self.score_criteria_add("totalKillCount", 1);
        if !self.cmd.scores.objectives.is_empty() {
            let victim = crate::nbtio::uuid_text(m.uuid);
            let team = self.cmd.scores.team_of(&victim).and_then(|t| t.color.clone());
            if let Some(c) = team {
                self.score_criteria_add(&format!("teamkill.{c}"), 1);
            }
            // The victim scores `killedByTeam.<colour of the player's team>`.
            let mine = self.cmd.scores.team_of(&self.name).and_then(|t| t.color.clone());
            // Display name taken from `m`: a mob that dies during its tick is out of the
            // list, not findable by its UUID.
            if let Some(c) = mine {
                let shown = self.mob_display(m);
                self.criteria_update_shown(&format!("killedByTeam.{c}"), &victim, Some(&shown), |v| {
                    v.wrapping_add(1)
                });
            }
        }
        let direct = match by {
            Some(Ent::Player) => Some(Who::E(E::Player)),
            Some(Ent::Mob(i)) => Some(Who::E(E::Mob(i))),
            None => None,
        };
        let dmg = Dmg { ty: ty.to_owned(), source: direct, direct, pos: Some(m.pos) };
        self.trig_killed(Some(Who::Mob(m)), &dmg);
    }

    /// Damage received by the player: statistics in tenths, `entity_hurt_player`.
    pub(crate) fn s9_player_hurt(&mut self, ty: &str, original: f32, taken: f32, absorbed: f32, parried: f32) {
        // W8c: `damage_blocked_by_shield` (tenths of a point).
        if parried > 0.0 && parried < 3.402_823_5e37 {
            self.award_custom("damage_blocked_by_shield", (parried * 10.0).round() as i32);
        }
        if absorbed > 0.0 && absorbed < 3.402_823_5e37 {
            self.award_custom("damage_absorbed", (absorbed * 10.0).round() as i32);
        }
        if taken > 0.0 && taken < 3.402_823_5e37 {
            self.award_custom("damage_taken", (taken * 10.0).round() as i32);
        }
        let attacker = self.prog.attacker.take();
        let src = attacker.as_deref().map(Who::Mob);
        let dmg = Dmg { ty: ty.to_owned(), source: src, direct: src, pos: attacker.as_ref().map(|m| m.pos) };
        self.trig_entity_hurt_player(&dmg, original, taken, parried > 0.0);
        self.prog.attacker = attacker;
    }

    /// Death of the player: statistics, scores, `entity_killed_player`.
    pub(crate) fn s9_player_died(&mut self, ty: &str) {
        let killer = self.prog.attacker.clone().or_else(|| {
            let (e, t) = self.mobs.player_hurt_by?;
            if self.mobs.player_ticks - t > 100 {
                return None;
            }
            match e {
                Ent::Mob(i) => self.mobs.mob_ref_any(i).map(|m| Box::new(m.clone())),
                Ent::Player => None,
            }
        });
        if let Some(k) = &killer {
            self.award_stat("minecraft:killed_by", k.kind.info().name, 1);
        } else if self.prog.attacker_player.is_some() {
            // Killed by the hit of another LAN player.
            self.award_stat("minecraft:killed_by", "minecraft:player", 1);
        }
        self.award_custom("deaths", 1);
        self.reset_stat("minecraft:custom", "minecraft:time_since_death");
        self.reset_stat("minecraft:custom", "minecraft:time_since_rest");
        self.score_criteria_add("deathCount", 1);
        let src = killer.as_deref().map(Who::Mob);
        let dmg = Dmg { ty: ty.to_owned(), source: src, direct: src, pos: killer.as_ref().map(|m| m.pos) };
        self.trig_player_killed(src, &dmg);
    }

    /// Dimension change.
    pub(crate) fn s9_dimension(&mut self, from: &str, to: &str) {
        self.trig_changed_dimension(from, to);
    }

    /// The player falls asleep (successfully): statistic specific to the bed type; trigger only if
    /// the bed sets the respawn point (never the straw bed in the normal world).
    pub(crate) fn s9_slept(&mut self, straw: bool, sets_spawn: bool) {
        self.award_custom(if straw { "sleep_in_straw_bed" } else { "sleep_in_bed" }, 1);
        if sets_spawn {
            self.trig_simple("minecraft:slept_in_bed");
        }
    }

    /// Breeding caused by the player: statistic and `bred_animals`.
    pub(crate) fn s9_bred(&mut self, parent: &Mob, partner_id: i32, child: Option<&Mob>) {
        let partner_cause = self.mobs.mob_ref_any(partner_id).is_some_and(|p| p.love_cause);
        if !parent.love_cause && !partner_cause {
            return;
        }
        self.award_custom("animals_bred", 1);
        let partner = Who::E(E::Mob(partner_id));
        self.trig_bred(Who::Mob(parent), partner, child.map(Who::Mob));
    }

    /// Trade completed with a merchant (result slot).
    pub(crate) fn s9_traded(&mut self, m: &Mob, result: &Stack) {
        self.award_custom("traded_with_villager", 1);
        self.trig_trade(Who::Mob(m), result);
    }

    /// Taming by the player.
    pub(crate) fn s9_tamed(&mut self, m: &Mob) {
        self.trig_tame(Who::Mob(m));
    }

    /// Interaction with an entity (`player_interacted_with_entity`), item before use.
    pub(crate) fn s9_entity_interact(&mut self, item: Option<&Stack>, m: &Mob) {
        self.trig_entity_interact(item, Who::Mob(m));
    }

    /// Container loot drawn by the player.
    pub(crate) fn s9_container_loot(&mut self, table: &str) {
        self.trig_container_loot(table);
    }
}
