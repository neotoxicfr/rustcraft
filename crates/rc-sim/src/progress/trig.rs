//! Advancement criteria triggers (`docs/specs/advancements-stats.md` § 7.3): one call per
//! game event; each active criterion (advancement not done, criterion not obtained) of the type is
//! tested ("player" condition then own conditions), the successful ones are then obtained.

use serde_json::Value;

use super::pred::{Ctx, Dmg, Who, doubles, ints, p_distance};
use crate::commands::ents::E;
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

#[cfg(test)]
#[path = "tests_trig.rs"]
mod tests;

impl Server {
    /// At least one active criterion for this trigger.
    pub(crate) fn trig_active(&self, name: &str) -> bool {
        self.prog
            .tree
            .by_trigger
            .get(name)
            .is_some_and(|l| l.iter().any(|&(a, c)| !self.prog.crit_done(a, c) && !self.prog.done(a)))
    }

    /// Tests the active criteria of trigger `name` then obtains those that succeed.
    pub(crate) fn trigger(&mut self, name: &str, test: &dyn Fn(&Server, &Value) -> bool) {
        let tree = self.prog.tree.clone();
        let Some(list) = tree.by_trigger.get(name) else { return };
        let me = Ctx { this: Some(Who::E(E::Player)), origin: Some(self.pos), ..Ctx::default() };
        let mut hits = Vec::new();
        for &(a, c) in list {
            if self.prog.crit_done(a, c) || self.prog.done(a) {
                continue;
            }
            let cond = &tree.list[a].criteria[c].cond;
            if let Some(p) = cond.get("player")
                && !self.p_context(p, &me)
            {
                continue;
            }
            if test(self, cond) {
                hits.push((a, c));
            }
        }
        for (a, c) in hits {
            self.adv_award(a, c);
        }
    }

    /// Contextual condition on an entity seen from the player.
    fn on_entity(&self, cond: Option<&Value>, w: Option<Who<'_>>) -> bool {
        match cond {
            None | Some(Value::Null) => true,
            Some(c) => w.is_some_and(|w| {
                let ctx = Ctx { this: Some(w), origin: Some(self.pos), ..Ctx::default() };
                self.p_context(c, &ctx)
            }),
        }
    }

    fn item_ok(&self, cond: Option<&Value>, s: Option<&Stack>) -> bool {
        cond.is_none_or(|p| p.is_null() || self.p_item(p, s))
    }

    // ---- Periodic ---------------------------------------------------------------------------

    /// Player tick: `tick`, `location` (every 20 ticks), fall, effects, item in use.
    pub(crate) fn trig_tick(&mut self) {
        if self.trig_active("minecraft:tick") {
            self.trigger("minecraft:tick", &|_, _| true);
        }
        if self.tick.is_multiple_of(20) && self.trig_active("minecraft:location") {
            self.trigger("minecraft:location", &|_, _| true);
        }
        // Fall start and landing.
        let fall = self.life.vit.fall;
        if fall > 0.0 && self.prog.fall_start.is_none() {
            self.prog.fall_start = Some(self.pos);
            // W8c: fall started above the impact point of an explosion
            // (`fall_after_explosion`); the impact is forgotten in all cases.
            if let Some((imp, cause)) = self.prog.expl_impact.take()
                && imp[1] <= self.pos[1]
                && self.trig_active("minecraft:fall_after_explosion")
            {
                let here = self.pos;
                self.trigger("minecraft:fall_after_explosion", &|s, c| {
                    distance_ok(s, c, imp, here) && typed_cause_ok(s, c.get("cause"), cause.as_deref())
                });
            }
        }
        if fall == 0.0
            && let Some(start) = self.prog.fall_start.take()
            && !self.life.vit.dead
            && self.trig_active("minecraft:fall_from_height")
        {
            let here = self.pos;
            self.trigger("minecraft:fall_from_height", &|s, c| distance_ok(s, c, start, here));
        }
        // W8c: levitation (`levitation`): start noted when the effect is added, forgotten at
        // removal; each tick, distance traveled and duration.
        if self.fx().has("minecraft:levitation") {
            if self.prog.lev_start.is_none() {
                self.prog.lev_start = Some((self.pos, self.tick));
            }
        } else {
            self.prog.lev_start = None;
        }
        if let Some((start, t0)) = self.prog.lev_start
            && self.trig_active("minecraft:levitation")
        {
            let here = self.pos;
            let dur = (self.tick - t0) as i64;
            self.trigger("minecraft:levitation", &|s, c| {
                distance_ok(s, c, start, here) && ints(c.get("duration"), dur)
            });
        }
        // W8c: vehicle in lava (`ride_entity_in_lava`): entry noted at the first tick,
        // trigger on the following ones, forgotten as soon as the vehicle leaves it or one dismounts.
        let veh_in_lava =
            self.mobs.mounts.riding.vehicle.and_then(|v| self.mobs.mob_ref_any(v)).is_some_and(|m| m.lava_height > 0.0);
        if veh_in_lava {
            match self.prog.lava_ride {
                None => self.prog.lava_ride = Some(self.pos),
                Some(start) => {
                    if self.trig_active("minecraft:ride_entity_in_lava") {
                        let here = self.pos;
                        self.trigger("minecraft:ride_entity_in_lava", &|s, c| distance_ok(s, c, start, here));
                    }
                }
            }
        } else {
            self.prog.lava_ride = None;
        }
        // W8c: mount reported by mounting an entity (mob or player, vehicle carrying the player).
        if std::mem::take(&mut self.prog.ride_started) && self.trig_active("minecraft:started_riding") {
            self.trigger("minecraft:started_riding", &|_, _| true);
        }
        // Item in use (`using_item`, each tick of use).
        if self.life.vit.using.is_some() && self.trig_active("minecraft:using_item") {
            let hand = self.life.vit.using.map_or(0, |u| u.0);
            let s = if hand == 1 { self.slots[45].clone() } else { self.slots[36 + self.held as usize].clone() };
            self.trigger("minecraft:using_item", &|sv, c| sv.item_ok(c.get("item"), s.as_ref()));
        }
    }

    /// Inventory slots changed since the last tick → `inventory_changed` per slot.
    pub(crate) fn trig_inventory_scan(&mut self) {
        if self.prog.inv_seen.len() != self.slots.len() {
            self.prog.inv_seen = self.slots.clone();
            return;
        }
        let mut changed = Vec::new();
        for i in 0..41 {
            let m = super::pred::inv_to_menu(i);
            if !crate::items::matches(self.slots[m].as_ref(), self.prog.inv_seen[m].as_ref())
                || self.slots[m].as_ref().map(|s| s.count) != self.prog.inv_seen[m].as_ref().map(|s| s.count)
            {
                changed.push(m);
            }
        }
        if changed.is_empty() {
            return;
        }
        self.prog.inv_seen = self.slots.clone();
        if !self.trig_active("minecraft:inventory_changed") {
            return;
        }
        for m in changed {
            let s = self.slots[m].clone();
            self.trig_inventory_changed(s.as_ref());
        }
    }

    /// `inventory_changed` (counts of full, empty, occupied slots; required items).
    pub(crate) fn trig_inventory_changed(&mut self, changed: Option<&Stack>) {
        let (mut full, mut empty, mut occupied) = (0i64, 0i64, 0i64);
        let inv: Vec<Option<Stack>> = (0..43)
            .map(|i| {
                let m = super::pred::inv_to_menu(i);
                self.slots.get(m).cloned().flatten()
            })
            .collect();
        for s in &inv {
            match s {
                None => empty += 1,
                Some(s) => {
                    occupied += 1;
                    if s.count >= self.life.items.max_stack(s) {
                        full += 1;
                    }
                }
            }
        }
        self.trigger("minecraft:inventory_changed", &|sv, c| {
            let slots = c.get("slots");
            if let Some(sl) = slots
                && !(ints(sl.get("full"), full) && ints(sl.get("empty"), empty) && ints(sl.get("occupied"), occupied))
            {
                return false;
            }
            let items: Vec<&Value> =
                c.get("items").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
            match items.len() {
                0 => true,
                1 => changed.is_some_and(|s| sv.p_item(items[0], Some(s))),
                _ => {
                    let mut left = items.clone();
                    for s in inv.iter().flatten() {
                        if left.is_empty() {
                            break;
                        }
                        left.retain(|p| !sv.p_item(p, Some(s)));
                    }
                    left.is_empty()
                }
            }
        });
    }

    /// `recipe_unlocked` for each learned recipe.
    pub(crate) fn trig_recipes_unlocked(&mut self, ids: &[String]) {
        if !self.trig_active("minecraft:recipe_unlocked") {
            return;
        }
        for id in ids {
            self.trigger("minecraft:recipe_unlocked", &|_, c| id_in(c.get("recipes"), id));
        }
    }

    /// After a movement: blocks passed through (`enter_block`) and sliding along a honey
    /// block (`slide_down_block`, only game ticks that are multiples of 20).
    pub(crate) fn trig_after_move(&mut self, before: [f64; 3], _was: bool) {
        let enter = self.trig_active("minecraft:enter_block");
        let slide = self.time % 20 == 0 && !self.on_ground && self.trig_active("minecraft:slide_down_block");
        if !enter && !slide {
            return;
        }
        let b = self.e_bbox(E::Player);
        let e = 1.0e-5;
        let (x0, y0, z0) =
            ((b.min[0] + e).floor() as i32, (b.min[1] + e).floor() as i32, (b.min[2] + e).floor() as i32);
        let (x1, y1, z1) =
            ((b.max[0] - e).floor() as i32, (b.max[1] - e).floor() as i32, (b.max[2] - e).floor() as i32);
        // Old vertical velocity seen by the honey rule: the official server recovers
        // exactly the tick's vertical movement, gravity removed then 0.08 added back.
        let old_dy = if slide { self.honey_old_dy(self.pos[1] - before[1]) } else { 0.0 };
        let overlap = 0.4375 + f64::from((b.max[0] - b.min[0]) as f32 / 2.0);
        let mut states = Vec::new();
        let mut slides = Vec::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    let s = self.get([x, y, z]);
                    if self.states.is_air(s) {
                        continue;
                    }
                    if enter && !states.contains(&s) {
                        states.push(s);
                    }
                    if slide
                        && old_dy < -0.08
                        && self.pos[1] <= f64::from(y) + 0.9375 - 1.0e-7
                        && self.state_name(s) == "minecraft:honey_block"
                    {
                        let dx = (f64::from(x) + 0.5 - self.pos[0]).abs();
                        let dz = (f64::from(z) + 0.5 - self.pos[2]).abs();
                        if dx + 1.0e-7 > overlap || dz + 1.0e-7 > overlap {
                            slides.push(s);
                        }
                    }
                }
            }
        }
        for s in states {
            self.trigger("minecraft:enter_block", &|sv, c| block_cond(sv, c, s));
        }
        for s in slides {
            self.trig_slide(s);
        }
    }

    /// The old vertical difference of the honey block applied to the velocity the player would have after their
    /// vertical movement `dy` of the tick (levitation, otherwise gravity, slow falling included).
    fn honey_old_dy(&self, dy: f64) -> f64 {
        let fx = self.fx();
        let after = match fx.get("minecraft:levitation") {
            Some(l) => dy + (0.05 * f64::from(l.amp + 1) - dy) * 0.2,
            None => {
                let g = self.player_attribute("minecraft:gravity");
                let g = if dy <= 0.0 && fx.has("minecraft:slow_falling") { g.min(0.01) } else { g };
                dy - g
            }
        };
        after + 0.08
    }

    // ---- Combat -----------------------------------------------------------------------------

    /// The player killed an entity (`player_killed_entity`, `kill_mob_near_sculk_catalyst`).
    /// `None`: victim that an entity predicate cannot yet describe (other player of the
    /// local network); only criteria without an entity condition pass.
    pub(crate) fn trig_killed(&mut self, victim: Option<Who<'_>>, dmg: &Dmg<'_>) {
        for name in ["minecraft:player_killed_entity", "minecraft:kill_mob_near_sculk_catalyst"] {
            if !self.trig_active(name) || (name.ends_with("catalyst") && !self.near_catalyst(victim)) {
                continue;
            }
            self.trigger(name, &|sv, c| {
                c.get("killing_blow").is_none_or(|k| sv.p_damage_source(k, dmg, Some(sv.pos)))
                    && sv.on_entity(c.get("entity"), victim)
            });
        }
    }

    fn near_catalyst(&self, _victim: Option<Who<'_>>) -> bool {
        false
    }

    /// An entity killed the player (`entity_killed_player`).
    pub(crate) fn trig_player_killed(&mut self, killer: Option<Who<'_>>, dmg: &Dmg<'_>) {
        if !self.trig_active("minecraft:entity_killed_player") {
            return;
        }
        self.trigger("minecraft:entity_killed_player", &|sv, c| {
            c.get("killing_blow").is_none_or(|k| sv.p_damage_source(k, dmg, Some(sv.pos)))
                && match c.get("entity") {
                    None | Some(Value::Null) => true,
                    Some(_) => killer.is_some() && sv.on_entity(c.get("entity"), killer),
                }
        });
    }

    /// `player_hurt_entity`: damage dealt by the player.
    pub(crate) fn trig_player_hurt_entity(
        &mut self,
        victim: Option<Who<'_>>,
        dmg: &Dmg<'_>,
        dealt: f32,
        taken: f32,
        blocked: bool,
    ) {
        if !self.trig_active("minecraft:player_hurt_entity") {
            return;
        }
        self.trigger("minecraft:player_hurt_entity", &|sv, c| {
            c.get("damage").is_none_or(|d| damage_ok(sv, d, dmg, dealt, taken, blocked))
                && sv.on_entity(c.get("entity"), victim)
        });
    }

    /// `entity_hurt_player`: damage received by the player.
    pub(crate) fn trig_entity_hurt_player(&mut self, dmg: &Dmg<'_>, dealt: f32, taken: f32, blocked: bool) {
        if !self.trig_active("minecraft:entity_hurt_player") {
            return;
        }
        self.trigger("minecraft:entity_hurt_player", &|sv, c| {
            c.get("damage").is_none_or(|d| damage_ok(sv, d, dmg, dealt, taken, blocked))
        });
    }

    // ---- Items and blocks -------------------------------------------------------------------

    /// `consume_item`: item eaten or drunk (stack before consumption).
    pub(crate) fn trig_consume(&mut self, s: &Stack) {
        if self.trig_active("minecraft:consume_item") {
            self.trigger("minecraft:consume_item", &|sv, c| sv.item_ok(c.get("item"), Some(s)));
        }
    }

    /// `recipe_crafted`: recipe and ingredients of the grid.
    pub(crate) fn trig_recipe_crafted(&mut self, kind: &str, id: &str, used: &[Stack]) {
        if !self.trig_active(kind) {
            return;
        }
        self.trigger(kind, &|sv, c| {
            if !id_in(c.get("recipes"), id) {
                return false;
            }
            let mut left: Vec<&Stack> = used.iter().collect();
            for p in c.get("ingredients").and_then(Value::as_array).into_iter().flatten() {
                match left.iter().position(|s| sv.p_item(p, Some(s))) {
                    Some(i) => {
                        left.remove(i);
                    }
                    None => return false,
                }
            }
            true
        });
    }

    /// Location triggers (`placed_block`, `item_used_on_block`, `any_block_use`,
    /// `allay_drop_item_on_block`): context = block center, state, tool.
    pub(crate) fn trig_location(&mut self, kind: &str, pos: [i32; 3], tool: Option<&Stack>) {
        if !self.trig_active(kind) {
            return;
        }
        let state = self.get(pos);
        let origin = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
        let tool_ref = tool;
        self.trigger(kind, &|sv, c| match c.get("location") {
            None | Some(Value::Null) => true,
            Some(l) => {
                let ctx = Ctx {
                    this: Some(Who::E(E::Player)),
                    origin: Some(origin),
                    block: Some(state),
                    tool: Some(tool_ref),
                    ..Ctx::default()
                };
                sv.p_context(l, &ctx)
            }
        });
    }

    /// `default_block_use`: interaction without an effective item.
    pub(crate) fn trig_default_block_use(&mut self, pos: [i32; 3]) {
        self.trig_location("minecraft:default_block_use", pos, None);
    }

    /// `player_interacted_with_entity`: item in hand and entity.
    pub(crate) fn trig_entity_interact(&mut self, item: Option<&Stack>, ent: Who<'_>) {
        if self.trig_active("minecraft:player_interacted_with_entity") {
            self.trigger("minecraft:player_interacted_with_entity", &|sv, c| {
                sv.item_ok(c.get("item"), item) && sv.on_entity(c.get("entity"), Some(ent))
            });
        }
    }

    /// W8c: `player_sheared_equipment`: equipment sheared on a mob.
    pub(crate) fn trig_sheared_equipment(&mut self, item: &Stack, ent: Who<'_>) {
        if self.trig_active("minecraft:player_sheared_equipment") {
            self.trigger("minecraft:player_sheared_equipment", &|sv, c| {
                sv.item_ok(c.get("item"), Some(item)) && sv.on_entity(c.get("entity"), Some(ent))
            });
        }
    }

    /// `filled_bucket`: bucket filled (stack obtained).
    pub(crate) fn trig_filled_bucket(&mut self, s: &Stack) {
        if self.trig_active("minecraft:filled_bucket") {
            self.trigger("minecraft:filled_bucket", &|sv, c| sv.item_ok(c.get("item"), Some(s)));
        }
    }

    /// `player_generates_container_loot`: loot table of an opened chest.
    pub(crate) fn trig_container_loot(&mut self, table: &str) {
        if self.trig_active("minecraft:player_generates_container_loot") {
            self.trigger("minecraft:player_generates_container_loot", &|_, c| id_in(c.get("loot_tables"), table));
        }
    }

    /// `enchanted_item`: item enchanted, levels spent.
    pub(crate) fn trig_enchanted_item(&mut self, s: &Stack, levels: i32) {
        if self.trig_active("minecraft:enchanted_item") {
            self.trigger("minecraft:enchanted_item", &|sv, c| {
                sv.item_ok(c.get("item"), Some(s)) && ints(c.get("levels"), i64::from(levels))
            });
        }
    }

    /// `brewed_potion`: potion taken from a brewing stand.
    pub(crate) fn trig_brewed_potion(&mut self, potion: Option<&str>) {
        if self.trig_active("minecraft:brewed_potion") {
            self.trigger("minecraft:brewed_potion", &|_, c| match c.get("potion") {
                None | Some(Value::Null) => true,
                Some(p) => potion.is_some_and(|x| p.as_str().is_some_and(|w| crate::commands::args::id(w) == x)),
            });
        }
    }

    /// `construct_beacon`: level of the pyramid.
    pub(crate) fn trig_construct_beacon(&mut self, level: i32) {
        if self.trig_active("minecraft:construct_beacon") {
            self.trigger("minecraft:construct_beacon", &|_, c| ints(c.get("level"), i64::from(level)));
        }
    }

    /// `used_totem`.
    pub(crate) fn trig_used_totem(&mut self, s: &Stack) {
        if self.trig_active("minecraft:used_totem") {
            self.trigger("minecraft:used_totem", &|sv, c| sv.item_ok(c.get("item"), Some(s)));
        }
    }

    /// `villager_trade`: merchant and item obtained.
    pub(crate) fn trig_trade(&mut self, villager: Who<'_>, s: &Stack) {
        if self.trig_active("minecraft:villager_trade") {
            self.trigger("minecraft:villager_trade", &|sv, c| {
                sv.on_entity(c.get("villager"), Some(villager)) && sv.item_ok(c.get("item"), Some(s))
            });
        }
    }

    /// `item_durability_changed`: new wear (damage) of a stack.
    pub(crate) fn trig_durability(&mut self, s: &Stack, new_damage: i32) {
        if !self.trig_active("minecraft:item_durability_changed") {
            return;
        }
        let max = self.life.items.max_damage(s);
        let cur = self.life.items.damage(s);
        self.trigger("minecraft:item_durability_changed", &|sv, c| {
            sv.item_ok(c.get("item"), Some(s))
                && ints(c.get("durability"), i64::from(max - new_damage))
                && ints(c.get("delta"), i64::from(cur - new_damage))
        });
    }

    /// `effects_changed`: current effects of the player, possible source.
    pub(crate) fn trig_effects_changed(&mut self, source: Option<Who<'_>>) {
        if !self.trig_active("minecraft:effects_changed") {
            return;
        }
        self.trigger("minecraft:effects_changed", &|sv, c| {
            c.get("effects").is_none_or(|e| {
                let mut p = serde_json::Map::new();
                p.insert("minecraft:effects".into(), e.clone());
                sv.p_entity(&Value::Object(p), Some(Who::E(E::Player)), Some(sv.pos))
            }) && match c.get("source") {
                None | Some(Value::Null) => true,
                Some(_) => source.is_some() && sv.on_entity(c.get("source"), source),
            }
        });
    }

    /// `changed_dimension` then `nether_travel` (return from the Nether to the surface, start =
    /// Nether entry saved with the player; the caller then clears it).
    pub(crate) fn trig_changed_dimension(&mut self, from: &str, to: &str) {
        if self.trig_active("minecraft:changed_dimension") {
            self.trigger("minecraft:changed_dimension", &|_, c| {
                c.get("from").and_then(Value::as_str).is_none_or(|f| crate::commands::args::id(f) == from)
                    && c.get("to").and_then(Value::as_str).is_none_or(|t| crate::commands::args::id(t) == to)
            });
        }
        if from == "minecraft:the_nether"
            && to == "minecraft:overworld"
            && let Some(start) = self.dims.entered_nether
            && self.trig_active("minecraft:nether_travel")
        {
            let here = self.pos;
            self.trigger("minecraft:nether_travel", &|s, c| distance_ok(s, c, start, here));
        }
    }

    /// `bred_animals`: parents (in either order) and baby.
    pub(crate) fn trig_bred(&mut self, parent: Who<'_>, partner: Who<'_>, child: Option<Who<'_>>) {
        if !self.trig_active("minecraft:bred_animals") {
            return;
        }
        self.trigger("minecraft:bred_animals", &|sv, c| {
            let child_ok = match c.get("child") {
                None | Some(Value::Null) => true,
                Some(_) => child.is_some() && sv.on_entity(c.get("child"), child),
            };
            child_ok
                && ((sv.on_entity(c.get("parent"), Some(parent)) && sv.on_entity(c.get("partner"), Some(partner)))
                    || (sv.on_entity(c.get("parent"), Some(partner)) && sv.on_entity(c.get("partner"), Some(parent))))
        });
    }

    /// `tame_animal`.
    pub(crate) fn trig_tame(&mut self, animal: Who<'_>) {
        if self.trig_active("minecraft:tame_animal") {
            self.trigger("minecraft:tame_animal", &|sv, c| sv.on_entity(c.get("entity"), Some(animal)));
        }
    }

    /// `summoned_entity` (wither, golems, respawned dragon).
    pub(crate) fn trig_summoned(&mut self, e: Who<'_>) {
        if self.trig_active("minecraft:summoned_entity") {
            self.trigger("minecraft:summoned_entity", &|sv, c| sv.on_entity(c.get("entity"), Some(e)));
        }
    }

    /// Triggers without conditions of their own (`slept_in_bed`, `started_riding`, `hero_of_the_village`,
    /// `avoid_vibration`, `raid_win`).
    pub(crate) fn trig_simple(&mut self, name: &str) {
        if self.trig_active(name) {
            self.trigger(name, &|_, _| true);
        }
    }

    /// W8c: `bee_nest_destroyed`: hive or nest broken by the player (bees left inside).
    pub(crate) fn trig_bee_nest(&mut self, state: u32, tool: Option<&Stack>, bees: i32) {
        if self.trig_active("minecraft:bee_nest_destroyed") {
            self.trigger("minecraft:bee_nest_destroyed", &|sv, c| {
                block_cond(sv, c, state)
                    && sv.item_ok(c.get("item"), tool)
                    && ints(c.get("num_bees_inside"), i64::from(bees))
            });
        }
    }

    /// W8c: `cured_zombie_villager`: zombie villager cured by the player.
    pub(crate) fn trig_cured(&mut self, zombie: Who<'_>, villager: Who<'_>) {
        if self.trig_active("minecraft:cured_zombie_villager") {
            self.trigger("minecraft:cured_zombie_villager", &|sv, c| {
                sv.on_entity(c.get("zombie"), Some(zombie)) && sv.on_entity(c.get("villager"), Some(villager))
            });
        }
    }

    /// `thrown_item_picked_up_by_player` / `_by_entity`.
    pub(crate) fn trig_picked_up(&mut self, kind: &str, s: &Stack, ent: Option<Who<'_>>) {
        if self.trig_active(kind) {
            self.trigger(kind, &|sv, c| {
                sv.item_ok(c.get("item"), Some(s))
                    && match c.get("entity") {
                        None | Some(Value::Null) => true,
                        Some(_) => ent.is_some() && sv.on_entity(c.get("entity"), ent),
                    }
            });
        }
    }

    /// `shot_crossbow`.
    #[allow(dead_code)] // mechanic not yet simulated on the server side
    pub(crate) fn trig_shot_crossbow(&mut self, s: &Stack) {
        if self.trig_active("minecraft:shot_crossbow") {
            self.trigger("minecraft:shot_crossbow", &|sv, c| sv.item_ok(c.get("item"), Some(s)));
        }
    }

    /// `killed_by_arrow`: victims of the same projectile, originating weapon.
    pub(crate) fn trig_killed_by_arrow(&mut self, victims: &[Who<'_>], weapon: Option<&Stack>) {
        if !self.trig_active("minecraft:killed_by_arrow") {
            return;
        }
        let mut types: Vec<String> = victims.iter().map(|v| self.w_type(*v)).collect();
        types.sort();
        types.dedup();
        let unique = types.len() as i64;
        self.trigger("minecraft:killed_by_arrow", &|sv, c| {
            if let Some(w) = c.get("fired_from_weapon")
                && !weapon.is_some_and(|s| sv.p_item(w, Some(s)))
            {
                return false;
            }
            let mut left: Vec<Who<'_>> = victims.to_vec();
            for p in c.get("victims").and_then(Value::as_array).into_iter().flatten() {
                match left.iter().position(|v| sv.on_entity(Some(p), Some(*v))) {
                    Some(i) => {
                        left.remove(i);
                    }
                    None => return false,
                }
            }
            ints(c.get("unique_entity_types"), unique)
        });
    }

    /// `slide_down_block` (honey).
    pub(crate) fn trig_slide(&mut self, state: u32) {
        if self.trig_active("minecraft:slide_down_block") {
            self.trigger("minecraft:slide_down_block", &|sv, c| block_cond(sv, c, state));
        }
    }

    /// `target_hit`: signal strength, projectile.
    pub(crate) fn trig_target_hit(&mut self, projectile: Who<'_>, signal: i32) {
        if self.trig_active("minecraft:target_hit") {
            self.trigger("minecraft:target_hit", &|sv, c| {
                ints(c.get("signal_strength"), i64::from(signal)) && sv.on_entity(c.get("projectile"), Some(projectile))
            });
        }
    }

    /// `used_ender_eye`: horizontal distance to the targeted structure.
    #[allow(dead_code)] // mechanic not yet simulated on the server side
    pub(crate) fn trig_used_ender_eye(&mut self, feature: [i32; 3]) {
        if self.trig_active("minecraft:used_ender_eye") {
            let xd = self.pos[0] - f64::from(feature[0]);
            let zd = self.pos[2] - f64::from(feature[2]);
            let d2 = xd * xd + zd * zd;
            self.trigger("minecraft:used_ender_eye", &|_, c| super::pred::doubles_sqr(c.get("distance"), d2));
        }
    }
}

/// Set of identifiers of a criterion (`recipes`, `loot_tables`): identifier or list
/// (a tag of recipes or tables does not exist in the base game: never true).
fn id_in(v: Option<&Value>, id: &str) -> bool {
    match v {
        Some(Value::String(s)) => crate::commands::args::id(s) == id,
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str().is_some_and(|s| crate::commands::args::id(s) == id)),
        _ => false,
    }
}

/// `block` / `state` conditions (block triggers).
fn block_cond(sv: &Server, c: &Value, s: u32) -> bool {
    let mut b = serde_json::Map::new();
    if let Some(x) = c.get("block").or_else(|| c.get("blocks")) {
        b.insert("blocks".into(), x.clone());
    }
    if let Some(x) = c.get("state") {
        b.insert("state".into(), x.clone());
    }
    sv.p_block_state(&Value::Object(b), s)
}

/// `start_position` and `distance`.
fn distance_ok(sv: &Server, c: &Value, start: [f64; 3], here: [f64; 3]) -> bool {
    if let Some(p) = c.get("start_position")
        && !sv.p_location(p, start[0], start[1], start[2])
    {
        return false;
    }
    c.get("distance").is_none_or(|d| p_distance(d, start, here))
}

/// W8c: condition on a vanished entity of which only the type remains known (cause of an
/// explosion: wind charge, TNT…): properties of "this" limited to the type; list =
/// all true; any other form fails.
fn typed_cause_ok(sv: &Server, cond: Option<&Value>, ty: Option<&str>) -> bool {
    let Some(c) = cond.filter(|c| !c.is_null()) else { return true };
    let Some(ty) = ty else { return false };
    let one = |c: &Value| {
        c["type"].as_str().map(|t| t.trim_start_matches("minecraft:")) == Some("entity_properties")
            && c["entity"].as_str().unwrap_or("this") == "this"
            && c["predicate"].as_object().is_some_and(|o| {
                o.iter().all(|(k, v)| {
                    matches!(k.as_str(), "minecraft:entity_type" | "minecraft:type")
                        && sv.p_set("minecraft:entity_type", v, ty)
                })
            })
    };
    match c {
        Value::Array(a) => a.iter().all(one),
        o => one(o),
    }
}

/// Damage predicate.
fn damage_ok(sv: &Server, p: &Value, d: &Dmg<'_>, dealt: f32, taken: f32, blocked: bool) -> bool {
    doubles(p.get("dealt"), f64::from(dealt))
        && doubles(p.get("taken"), f64::from(taken))
        && p.get("source_entity").is_none_or(|e| sv.p_entity(e, d.source, Some(sv.pos)))
        && p.get("blocked").and_then(Value::as_bool).is_none_or(|b| b == blocked)
        && p.get("type").is_none_or(|t| sv.p_damage_source(t, d, Some(sv.pos)))
}

impl Server {
    /// `lightning_strike`: lightning that disappears within 256 blocks of the player. `pos`: its
    /// position, `fires`: cells set on fire, `hit`: entities struck, `viewers`: living
    /// entities of the ±15 box not struck (witnesses).
    pub(crate) fn trig_lightning_strike(&mut self, pos: [f64; 3], fires: i32, hit: &[Ent], viewers: &[Ent]) {
        let d = [self.pos[0] - pos[0], self.pos[1] - pos[1], self.pos[2] - pos[2]];
        if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32 >= 256.0
            || !self.trig_active("minecraft:lightning_strike")
        {
            return;
        }
        self.trigger("minecraft:lightning_strike", &|sv, c| {
            let bolt_ok = c.get("lightning").is_none_or(|l| sv.bolt_cond(l, pos, fires, hit));
            bolt_ok
                && c.get("bystander").is_none_or(|b| {
                    viewers.iter().any(|&e| {
                        let who = match e {
                            Ent::Player => Who::E(E::Player),
                            Ent::Mob(id) => Who::E(E::Mob(id)),
                        };
                        let ctx = Ctx { this: Some(who), origin: Some(sv.pos), ..Ctx::default() };
                        sv.p_context(b, &ctx)
                    })
                })
        });
    }

    /// Contextual condition on the lightning itself (entity predicate: type, location, distance
    /// to the player, `lightning` sub-predicate).
    fn bolt_cond(&self, v: &Value, pos: [f64; 3], fires: i32, hit: &[Ent]) -> bool {
        match v {
            Value::Null => true,
            Value::Array(l) => l.iter().all(|c| self.bolt_cond(c, pos, fires, hit)),
            Value::Object(o) if o.contains_key("type") || o.contains_key("condition") => {
                match crate::commands::args::id(v["type"].as_str().or_else(|| v["condition"].as_str()).unwrap_or(""))
                    .as_str()
                {
                    "minecraft:entity_properties" => {
                        v.get("predicate").is_none_or(|p| self.bolt_pred(p, pos, fires, hit))
                    }
                    "minecraft:inverted" => !self.bolt_cond(&v["term"], pos, fires, hit),
                    "minecraft:all_of" => {
                        v["terms"].as_array().is_none_or(|t| t.iter().all(|c| self.bolt_cond(c, pos, fires, hit)))
                    }
                    "minecraft:any_of" => {
                        v["terms"].as_array().is_some_and(|t| t.iter().any(|c| self.bolt_cond(c, pos, fires, hit)))
                    }
                    _ => false,
                }
            }
            p => self.bolt_pred(p, pos, fires, hit),
        }
    }

    fn bolt_pred(&self, p: &Value, pos: [f64; 3], fires: i32, hit: &[Ent]) -> bool {
        let Value::Object(parts) = p else { return true };
        parts.iter().all(|(k, v)| match crate::commands::args::id(k).as_str() {
            "minecraft:entity_type" | "minecraft:type" => self.p_set("minecraft:entity_type", v, "minecraft:lightning_bolt"),
            "minecraft:location" => self.p_location(v, pos[0], pos[1], pos[2]),
            "minecraft:distance" => p_distance(v, self.pos, pos),
            "minecraft:type_specific/lightning" => {
                ints(v.get("blocks_set_on_fire"), i64::from(fires))
                    && v.get("entity_struck").is_none_or(|e| {
                        hit.iter().any(|&h| {
                            let who = match h {
                                Ent::Player => Who::E(E::Player),
                                Ent::Mob(id) => Who::E(E::Mob(id)),
                            };
                            // Context origin: the player, not the lightning.
                            self.p_entity(e, Some(who), Some(self.pos))
                        })
                    })
            }
            _ => false,
        })
    }
}
