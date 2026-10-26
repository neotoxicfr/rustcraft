//! Items used on a block by the player and plant growth in singleplayer:
//! bone meal (crops, stems, berries, cocoa, saplings and trees, grasses, flowers, mushrooms,
//! moss, grass), data-driven block transformers (`block_transformer`:
//! axe, hoe, shovel), honeycomb wax; placing generator features in the live
//! world (trees, huge mushrooms, moss carpets, grasses).

use serde_json::Value;

use super::{Dir, P, above, flag, rel};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Trees of a sapling: simple trees (weight), 2×2 trees, "flowering" trees (weight).
struct Grower {
    trees: &'static [(&'static str, i32)],
    mega: &'static [(&'static str, i32)],
    flowers: &'static [(&'static str, i32)],
}

fn grower_of(block: &str) -> Option<Grower> {
    let g = |trees, mega, flowers| Some(Grower { trees, mega, flowers });
    match block.trim_start_matches("minecraft:") {
        "oak_sapling" => g(&[("oak", 9), ("fancy_oak", 1)], &[], &[("oak_bees_005", 9), ("fancy_oak_bees_005", 1)]),
        "spruce_sapling" => g(&[("spruce", 1)], &[("mega_spruce", 1), ("mega_pine", 1)], &[]),
        "mangrove_propagule" => g(&[("mangrove", 15), ("tall_mangrove", 85)], &[], &[]),
        "azalea" | "flowering_azalea" => g(&[("azalea_tree", 1)], &[], &[]),
        "birch_sapling" => g(&[("birch", 1)], &[], &[("birch_bees_005", 1)]),
        "jungle_sapling" => g(&[("jungle_tree_no_vine", 1)], &[("mega_jungle_tree", 1)], &[]),
        "acacia_sapling" => g(&[("acacia", 1)], &[], &[]),
        "cherry_sapling" => g(&[("cherry", 1)], &[], &[("cherry_bees_005", 1)]),
        "dark_oak_sapling" => g(&[], &[("dark_oak", 1)], &[]),
        "pale_oak_sapling" => g(&[], &[("pale_oak_bonemeal", 1)], &[]),
        "poplar_sapling" => g(&[("red_poplar", 1), ("orange_poplar", 1), ("yellow_poplar", 1)], &[], &[]),
        _ => None,
    }
}

/// Weighted random draw: nothing, and no draw, if the list is empty.
fn weighted(list: &'static [(&'static str, i32)], rng: &mut super::JavaRandom) -> Option<&'static str> {
    let total: i32 = list.iter().map(|e| e.1).sum();
    if total == 0 {
        return None;
    }
    let mut n = rng.next_int_bound(total);
    for &(name, w) in list {
        if n < w {
            return Some(name);
        }
        n -= w;
    }
    None
}

/// Simple plant that becomes a double plant under bone meal.
fn grown_double(block: &str) -> Option<&'static str> {
    Some(match block {
        "minecraft:short_grass" => "minecraft:tall_grass",
        "minecraft:fern" => "minecraft:large_fern",
        _ => return None,
    })
}

/// Configured feature placed by bone meal on this block (mushrooms, moss).
fn bonemeal_feature(block: &str) -> Option<&'static str> {
    Some(match block {
        "minecraft:red_mushroom" => "minecraft:huge_red_mushroom",
        "minecraft:brown_mushroom" => "minecraft:huge_brown_mushroom",
        "minecraft:moss_block" => "minecraft:moss_patch_bonemeal",
        "minecraft:pale_moss_block" => "minecraft:pale_moss_patch_bonemeal",
        _ => return None,
    })
}

impl Server {
    /// State `name` with these properties (unknown ones ignored, absent ones default).
    pub(crate) fn state_with(&self, name: &str, props: &[(String, String)]) -> Option<u32> {
        use rc_proto::nbt::Nbt;
        let props: Vec<(String, Nbt)> = props.iter().map(|(k, v)| (k.clone(), Nbt::String(v.clone()))).collect();
        let entry = Nbt::Compound(vec![
            ("id".into(), Nbt::String(name.to_owned())),
            ("properties".into(), Nbt::Compound(props)),
        ]);
        self.states.state_of(&self.data, &entry)
    }

    pub(crate) fn props_of(&self, s: u32) -> Vec<(String, String)> {
        self.data.state_properties(s).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
    }

    /// Use of the item in slot `slot` on the clicked block; true if the action is consumed
    /// ("used" statistic counted).
    pub(crate) fn item_use_on(&mut self, slot: usize, clicked: P, face: Dir) -> bool {
        let Some(stack) = self.slots[slot].clone() else { return false };
        let done = match stack.id.as_str() {
            "minecraft:bone_meal" => self.bone_meal_use(slot, clicked),
            "minecraft:honeycomb" => self.wax_use(slot, clicked, &stack),
            "minecraft:potion" => self.water_bottle_on_mud(slot, clicked, face), // G6-F01
            // G5-F10: item uses on a block, followed by the common trigger
            // `item_used_on_block` (stack from before the use) when the use succeeds.
            "minecraft:compass" | "minecraft:brush" | "minecraft:debug_stick" | "minecraft:shears" => {
                let ok = match stack.id.as_str() {
                    "minecraft:brush" => self.brush_use_on(slot),
                    "minecraft:debug_stick" => self.debug_stick_use(slot, clicked, true),
                    // Compass off lodestone, shears off plant head: common use
                    // (transformer carried by the stack, if any).
                    id => {
                        let own = if id == "minecraft:compass" {
                            self.compass_use_on(slot, clicked)
                        } else {
                            self.shears_trim_head(slot, clicked, &stack)
                        };
                        own || self
                            .transformer_of(&stack)
                            .is_some_and(|t| self.transform_use(slot, &stack, &t, clicked, face))
                    }
                };
                if ok {
                    self.trig_location("minecraft:item_used_on_block", clicked, Some(&stack));
                }
                ok
            }
            // Minecarts: placed on a rail (G5-F10), the item goes away except in creative.
            id if id.ends_with("minecart") => {
                let placed = self.place_minecart(&stack, clicked).is_some();
                if placed {
                    self.consume_held(slot);
                    self.trig_location("minecraft:item_used_on_block", clicked, Some(&stack));
                }
                placed
            }
            _ => match self.transformer_of(&stack) {
                Some(t) => self.transform_use(slot, &stack, &t, clicked, face),
                None => false,
            },
        };
        if done {
            self.award_stat("minecraft:used", &stack.id, 1);
        }
        done
    }

    /// Shears on the head of a climbing plant (kelp, weeping and
    /// twisting vines, cave vines) that is still growing: age raised to the maximum (25, it
    /// stops growing), `item_used_on_block` trigger, sound, `block_change` event,
    /// wear of 1.
    fn shears_trim_head(&mut self, slot: usize, p: P, stack: &crate::player::Stack) -> bool {
        let s = self.get(p);
        let head = matches!(
            self.state_name(s),
            "minecraft:kelp" | "minecraft:weeping_vines" | "minecraft:twisting_vines" | "minecraft:cave_vines"
        );
        if !head || self.v().get_int(s, "age") == 25 {
            return false;
        }
        self.trig_location("minecraft:item_used_on_block", p, Some(stack));
        // The game client plays this sound itself for the one who trims: also sent to the player.
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.play_sound("minecraft:block.growing_plant.crop", 4, at, 1.0, 1.0);
        let ns = self.v().set(s, "age", "25");
        self.set_block(p, ns, flag::ALL);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(ns));
        self.hurt_item(slot, 1);
        true
    }

    // --- Bone meal. ---

    /// Growable block → growth, bone meal consumed, vibration of
    /// the use, particles (event 1505, 15).
    fn bone_meal_use(&mut self, slot: usize, p: P) -> bool {
        if !self.grow_with_bone_meal(p) {
            return false;
        }
        // The vibration is drawn from the stack AFTER the withdrawal, which happens even in creative
        // (the count is only given back after the use): the last bone meal does not vibrate.
        let mut left = self.slots[slot].clone();
        if let Some(l) = &mut left {
            l.count -= 1;
        }
        self.consume_held(slot);
        self.use_vibration(left.as_ref(), "minecraft:item_interact_finish");
        self.level_event(1505, p, 15);
        true
    }

    /// True if the block accepts bone meal (even when the success draw
    /// fails).
    pub(crate) fn grow_with_bone_meal(&mut self, p: P) -> bool {
        let s = self.get(p);
        let name = self.state_name(s).to_owned();
        let v = self.v();
        match name.as_str() {
            "minecraft:wheat"
            | "minecraft:carrots"
            | "minecraft:potatoes"
            | "minecraft:beetroots"
            | "minecraft:torchflower_crop" => {
                // Maximum age: nothing; otherwise age + 2..5 (beetroot ÷ 3, torchflower + 1).
                self.grow_crop(p)
            }
            "minecraft:pumpkin_stem" | "minecraft:melon_stem" => {
                let age = v.get_int(s, "age");
                if age == 7 {
                    return false;
                }
                let age = (age + 2 + self.bs.rng.next_int_bound(4)).min(7);
                let ns = self.v().set_int(s, "age", age);
                self.set_block(p, ns, flag::CLIENTS);
                if age == 7 {
                    self.stem_random_tick(ns, p);
                }
                true
            }
            "minecraft:sweet_berry_bush" => {
                let age = v.get_int(s, "age");
                if age >= 3 {
                    return false;
                }
                let ns = self.v().set_int(s, "age", (age + 1).min(3));
                self.set_block(p, ns, flag::CLIENTS);
                true
            }
            "minecraft:cocoa" => {
                let age = v.get_int(s, "age");
                if age >= 2 {
                    return false;
                }
                let ns = self.v().set_int(s, "age", age + 1);
                self.set_block(p, ns, flag::CLIENTS);
                true
            }
            "minecraft:short_grass" | "minecraft:fern" => {
                let tall = grown_double(&name).unwrap_or_default();
                let lower = self.default_state(tall);
                let up = above(p);
                if !self.can_survive(lower, p) || !self.v().is_air(self.get(up)) || !self.in_build_height(up[1]) {
                    return false;
                }
                self.place_double_plant(lower, p, flag::CLIENTS);
                true
            }
            "minecraft:sunflower" | "minecraft:lilac" | "minecraft:rose_bush" | "minecraft:peony" => {
                self.pop_resource(p, Stack { id: name.clone(), count: 1, components: None });
                true
            }
            "minecraft:pink_petals" | "minecraft:wildflowers" => {
                let amount = v.get_int(s, "flower_amount");
                if amount < 4 {
                    let ns = self.v().set_int(s, "flower_amount", amount + 1);
                    self.set_block(p, ns, flag::CLIENTS);
                } else {
                    self.pop_resource(p, Stack { id: name.clone(), count: 1, components: None });
                }
                true
            }
            "minecraft:red_mushroom" | "minecraft:brown_mushroom" => {
                let feature = bonemeal_feature(&name).unwrap_or_default();
                let radius = self
                    .life
                    .pack
                    .get("worldgen/feature", feature.trim_start_matches("minecraft:"))
                    .and_then(|f| f["config"]["foliage_radius"].as_i64())
                    .unwrap_or(2) as i32;
                if !self.in_build_height(p[1] + 4 + radius) {
                    return false;
                }
                if self.bs.rng.next_float() < 0.4 {
                    // Block removed, feature placed, otherwise put back.
                    self.remove_block(p, false);
                    if !self.place_world_feature(feature, p) {
                        self.set_update(p, s);
                    }
                }
                true
            }
            "minecraft:moss_block" | "minecraft:pale_moss_block" => {
                if !self.v().is_air(self.get(above(p))) {
                    return false;
                }
                let feature = bonemeal_feature(&name).unwrap_or_default();
                self.place_world_feature(feature, above(p));
                true
            }
            "minecraft:big_dripleaf" => self.dripleaf_bone_meal(s, p, false),
            "minecraft:big_dripleaf_stem" => self.dripleaf_bone_meal(s, p, true),
            "minecraft:grass_block" => {
                let up = above(p);
                if !self.v().is_air(self.get(up)) || !self.in_build_height(up[1]) {
                    return false;
                }
                self.grass_bone_meal(p);
                true
            }
            _ => {
                if grower_of(&name).is_some() {
                    return self.sapling_bone_meal(p, s, &name);
                }
                false
            }
        }
    }

    /// Double plant placed (lower then upper half), given flags.
    fn place_double_plant(&mut self, lower: u32, p: P, flags: u32) {
        let lower = self.v().set(lower, "half", "lower");
        let upper = self.v().set(lower, "half", "upper");
        self.set_block(p, lower, flags);
        self.set_block(above(p), upper, flags);
    }

    /// Sapling (or azalea): height check for the azalea, 45% success, then
    /// the tree advance (stage 0 → 1, otherwise the tree).
    fn sapling_bone_meal(&mut self, p: P, s: u32, name: &str) -> bool {
        let azalea = name == "minecraft:azalea" || name == "minecraft:flowering_azalea";
        if azalea {
            // Base height of the azalea tree trunk + 2, no fluid above.
            let base = self
                .life
                .pack
                .get("worldgen/feature", "azalea_tree")
                .and_then(|f| f["config"]["trunk_placer"]["base_height"].as_i64())
                .unwrap_or(0) as i32;
            if !self.in_build_height(p[1] + base + 2) || !self.fluid_at(above(p)).is_empty() {
                return false;
            }
        } else if name == "minecraft:mangrove_propagule" {
            // Hanging propagule: age + 1 up to 4 (no tree).
            if self.v().get_bool(s, "hanging") {
                let age = self.v().get_int(s, "age");
                if age >= 4 {
                    return false;
                }
                let ns = self.v().set_int(s, "age", age + 1);
                self.set_block(p, ns, flag::CLIENTS);
                return true;
            }
        }
        if !azalea && !self.sapling_valid_target(p, s, name) {
            return false;
        }
        if self.bs.rng.next_float() < 0.45 {
            if azalea {
                self.grow_tree(p, s, name);
            } else {
                self.advance_tree(p, s);
            }
        }
        true
    }

    /// Valid bone meal target for a sapling: the growth test (draws on the world's randomness, simple
    /// tree then big tree; without a simple tree, a 2×2 square of saplings is required), then
    /// build height above the trunk of the shortest tree.
    fn sapling_valid_target(&mut self, p: P, s: u32, name: &str) -> bool {
        let Some(g) = grower_of(name) else { return false };
        let flowers = self.has_flowers_near(p);
        let list = if flowers && !g.flowers.is_empty() { g.flowers } else { g.trees };
        let tree = weighted(list, &mut self.bs.rng);
        let mega = weighted(g.mega, &mut self.bs.rng);
        if tree.is_none() && mega.is_some() {
            let block = self.v().block(s);
            let square = [0, -1].iter().any(|&dx| {
                [0, -1].iter().any(|&dz| {
                    [[0, 0], [1, 0], [0, 1], [1, 1]]
                        .iter()
                        .all(|&[a, b]| self.v().block(self.get([p[0] + dx + a, p[1], p[2] + dz + b])) == block)
                })
            });
            if !square {
                return false;
            }
        }
        let shortest = match name.trim_start_matches("minecraft:") {
            "oak_sapling" => Some("oak"),
            "spruce_sapling" => Some("spruce"),
            "mangrove_propagule" => Some("mangrove"),
            "birch_sapling" => Some("birch"),
            "jungle_sapling" => Some("jungle_tree_no_vine"),
            "acacia_sapling" => Some("acacia"),
            "cherry_sapling" => Some("cherry"),
            "poplar_sapling" => Some("red_poplar"),
            _ => None,
        };
        let base = shortest
            .and_then(|f| self.life.pack.get("worldgen/feature", f))
            .and_then(|f| f["config"]["trunk_placer"]["base_height"].as_i64())
            .unwrap_or(0) as i32;
        self.in_build_height(p[1] + base)
    }

    /// Stage 0 → 1, otherwise the tree grows.
    pub(crate) fn advance_tree(&mut self, p: P, s: u32) {
        if self.v().has(s, "stage") && self.v().get_int(s, "stage") == 0 {
            let ns = self.v().set_int(s, "stage", 1);
            self.set_block(p, ns, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
        } else {
            let name = self.state_name(s).to_owned();
            self.grow_tree(p, s, &name);
        }
    }

    /// 2×2 tree if four identical saplings in a square, otherwise simple
    /// tree ("flowering" with a flower at ±2/±1/±2); saplings removed then put back on
    /// failure.
    fn grow_tree(&mut self, p: P, s: u32, name: &str) -> bool {
        let Some(g) = grower_of(name) else { return false };
        let block = self.v().block(s);
        if let Some(mega) = weighted(g.mega, &mut self.bs.rng) {
            let id = format!("minecraft:{mega}");
            if self.feature_exists(&id) {
                for dx in [0, -1] {
                    for dz in [0, -1] {
                        let cells = [[0, 0], [1, 0], [0, 1], [1, 1]].map(|[a, b]| [p[0] + dx + a, p[1], p[2] + dz + b]);
                        if cells.iter().any(|&q| self.v().block(self.get(q)) != block) {
                            continue;
                        }
                        let olds: Vec<(P, u32)> = cells.iter().map(|&q| (q, self.get(q))).collect();
                        for &(q, _) in &olds {
                            self.remove_sapling(q);
                        }
                        if self.place_world_feature(&id, [p[0] + dx, p[1], p[2] + dz]) {
                            return true;
                        }
                        for &(q, old) in &olds {
                            self.set_block(q, old, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
                        }
                        return false;
                    }
                }
            }
        }
        let flowers = self.has_flowers_near(p);
        let list = if flowers && !g.flowers.is_empty() { g.flowers } else { g.trees };
        let Some(tree) = weighted(list, &mut self.bs.rng) else { return false };
        let id = format!("minecraft:{tree}");
        if !self.feature_exists(&id) {
            return false;
        }
        self.remove_sapling(p);
        if self.place_world_feature(&id, p) {
            return true;
        }
        self.set_block(p, s, flag::INVISIBLE | flag::SKIP_BE_SIDE_EFFECTS);
        false
    }

    /// Sapling removed without loot or update (remaining fluid; flags 818).
    fn remove_sapling(&mut self, q: P) {
        let f = self.fluid_at(q);
        let st = if f.is_empty() { self.air() } else { self.legacy_block(f) };
        self.set_block(q, st, 818);
    }

    fn has_flowers_near(&self, p: P) -> bool {
        (-2..=2).any(|dx| {
            (-1..=1).any(|dy| (-2..=2).any(|dz| self.btag_id([p[0] + dx, p[1] + dy, p[2] + dz], crate::tag_bits::TagId::Flowers)))
        })
    }

    /// Grass: 128 attempts above, moved at random by one notch per slice of 16 attempts as long
    /// as the ground remains grass and the cell is not full; tall grass enlarged (10%), otherwise
    /// on air: biome flower (12.5%) or grass (`grass_bonemeal`).
    fn grass_bone_meal(&mut self, p: P) {
        let grass = self.v().block(self.get(p));
        let up = above(p);
        'attempt: for attempt in 0..128 {
            let mut t = up;
            for _ in 0..attempt / 16 {
                let dx = self.bs.rng.next_int_bound(3) - 1;
                let dy = (self.bs.rng.next_int_bound(3) - 1) * self.bs.rng.next_int_bound(3) / 2;
                let dz = self.bs.rng.next_int_bound(3) - 1;
                t = [t[0] + dx, t[1] + dy, t[2] + dz];
                let below = self.get([t[0], t[1] - 1, t[2]]);
                if self.v().block(below) != grass || self.v().full_collision(self.get(t)) {
                    continue 'attempt;
                }
            }
            let ts = self.get(t);
            if self.state_name(ts) == "minecraft:short_grass" && self.bs.rng.next_float() < 0.1 {
                let lower = self.default_state("minecraft:tall_grass");
                if self.can_survive(lower, t) && self.v().is_air(self.get(above(t))) && self.in_build_height(t[1] + 1) {
                    self.place_double_plant(lower, t, flag::CLIENTS);
                }
            }
            if self.v().is_air(ts) && self.in_build_height(t[1]) {
                if self.bs.rng.next_float() < 0.125 {
                    let flowers = self.biome_bone_meal_features(t);
                    if flowers.is_empty() {
                        continue 'attempt; // only the current attempt stops
                    }
                    let i = self.bs.rng.next_int_bound(flowers.len() as i32) as usize;
                    self.place_world_feature(&flowers[i], t);
                } else {
                    self.place_world_placed("minecraft:grass_bonemeal", t);
                }
            }
        }
    }

    /// Biome features that can spawn from bone meal (tag
    /// `can_spawn_from_bone_meal`), in the order of the steps and of the placed features, nested
    /// features included.
    fn biome_bone_meal_features(&self, p: P) -> Vec<String> {
        let biome = self.biome_str(p);
        let Some(b) = self.life.pack.get("worldgen/biome", biome.trim_start_matches("minecraft:")) else {
            return Vec::new();
        };
        let tag: Vec<String> = self
            .life
            .pack
            .get("tags/worldgen/feature", "can_spawn_from_bone_meal")
            .and_then(|t| t["values"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let mut out = Vec::new();
        let steps = b["features"].as_array().cloned().unwrap_or_default();
        for step in &steps {
            let list: Vec<Value> = match step {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            for placed in list {
                let Some(pid) = placed.as_str() else { continue };
                let Some(pf) = self.life.pack.get("worldgen/placed_feature", pid.trim_start_matches("minecraft:"))
                else {
                    continue;
                };
                self.collect_features(&pf["feature"], &tag, &mut out, 0);
            }
        }
        out
    }

    fn collect_features(&self, f: &Value, tag: &[String], out: &mut Vec<String>, depth: u32) {
        if depth > 8 {
            return;
        }
        match f {
            Value::String(id) => {
                let id = if id.contains(':') { id.clone() } else { format!("minecraft:{id}") };
                if tag.contains(&id) {
                    out.push(id.clone());
                }
                if let Some(cf) = self.life.pack.get("worldgen/feature", id.trim_start_matches("minecraft:")) {
                    self.nested_features(&cf["config"], tag, out, depth + 1);
                }
            }
            Value::Object(_) => self.nested_features(&f["config"], tag, out, depth + 1),
            _ => {}
        }
    }

    /// Features referenced in a configuration (`feature` of nested placed features).
    fn nested_features(&self, v: &Value, tag: &[String], out: &mut Vec<String>, depth: u32) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if k == "feature" {
                        match x {
                            Value::String(pid) => {
                                if let Some(pf) =
                                    self.life.pack.get("worldgen/placed_feature", pid.trim_start_matches("minecraft:"))
                                {
                                    self.collect_features(&pf["feature"], tag, out, depth);
                                }
                            }
                            Value::Object(o) if o.contains_key("feature") => {
                                self.collect_features(&x["feature"], tag, out, depth);
                            }
                            _ => self.collect_features(x, tag, out, depth),
                        }
                    } else {
                        self.nested_features(x, tag, out, depth);
                    }
                }
            }
            Value::Array(a) => {
                for x in a {
                    self.nested_features(x, tag, out, depth);
                }
            }
            _ => {}
        }
    }

    // --- Generator features in the live world. ---

    fn feature_exists(&mut self, id: &str) -> bool {
        self.place_data().and_then(|(_, dg)| dg.deco.clone()).is_some_and(|d| d.reg.feature_id(id).is_some())
    }

    /// Configured feature placed at `pos` (3×3 chunks copied, changed blocks rewritten).
    pub(crate) fn place_world_feature(&mut self, id: &str, pos: P) -> bool {
        self.place_world(id, pos, false)
    }

    /// Placed feature (placement rules included) at `pos`.
    pub(crate) fn place_world_placed(&mut self, id: &str, pos: P) -> bool {
        self.place_world(id, pos, true)
    }

    fn place_world(&mut self, id: &str, pos: P, placed: bool) -> bool {
        use rc_worldgen::decor_region::{DecoChunk, Region};
        let Some((wd, dg)) = self.place_data() else { return false };
        let Some(deco) = dg.deco.clone() else { return false };
        let (cx, cz) = (pos[0] >> 4, pos[2] >> 4);
        let mut chunks: Vec<DecoChunk> = Vec::with_capacity(9);
        for dz in -1..=1 {
            for dx in -1..=1 {
                chunks.push(self.deco_chunk(cx + dx, cz + dz, &dg, &wd));
            }
        }
        // Seed drawn from the world's randomness (draw of a 64-bit integer).
        let hi = i64::from(self.bs.rng.next_int());
        let seed = (hi << 32).wrapping_add(i64::from(self.bs.rng.next_int()));
        let ok = {
            let mut it = chunks.iter_mut();
            let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().expect("9 chunks"));
            let mut r = Region::new(refs, &wd.blocks, dg.sea_level, dg.seed);
            let mut ctx = rc_worldgen::decor::Ctx::new(&mut r, &deco);
            let mut rng = rc_worldgen::decor_rng::DecoRng::legacy(seed);
            if placed {
                match deco.reg.placed_id(id) {
                    Some(pid) => rc_worldgen::placement::place(&mut ctx, pid, &mut rng, pos, None),
                    None => false,
                }
            } else {
                match deco.reg.feature_id(id) {
                    Some(fid) => rc_worldgen::feature::place_feature(&mut ctx, fid, &mut rng, pos),
                    None => false,
                }
            }
        };
        if ok {
            for dc in &chunks {
                self.apply_deco(dc, &wd.blocks);
            }
        }
        ok
    }

    // --- Block transformers (axe, hoe, shovel). ---

    /// Transformer carried by the item (component, otherwise the item's default one).
    fn transformer_of(&self, s: &Stack) -> Option<Value> {
        let id = crate::items::component(s, "minecraft:block_transformer")
            .and_then(|v| v.as_str().map(str::to_owned))
            .or_else(|| {
                self.life
                    .items
                    .default_component(s, "minecraft:block_transformer")
                    .and_then(|v| v.as_str().map(str::to_owned))
            })
            .or_else(|| {
                // Default components unavailable: base tools of the game.
                [
                    ("minecraft:axes", "minecraft:axe"),
                    ("minecraft:hoes", "minecraft:hoe"),
                    ("minecraft:shovels", "minecraft:shovel"),
                ]
                .iter()
                .find(|(tag, _)| self.life.items.has_tag(tag, &s.id))
                .map(|(_, t)| (*t).to_owned())
            })?;
        let v = self.life.pack.get("block_transformer", id.trim_start_matches("minecraft:"))?;
        Some((*v).clone())
    }

    /// First transformer whose face is allowed and
    /// whose rule gives a state; interaction loot, wear (or consumption of a
    /// stackable item), placement with flags 11, sound and particles (not to the acting player: their client
    /// predicts them), `block_change` event.
    fn transform_use(&mut self, slot: usize, stack: &Stack, t: &Value, p: P, face: Dir) -> bool {
        // Shield in the off hand without sneaking: the intent is to parry.
        if self.slots[45].as_ref().is_some_and(|o| self.stack_has(o, "minecraft:blocks_attacks")) && !self.sneaking
        {
            return false;
        }
        let Some(list) = t.as_array() else { return false };
        for data in list {
            let banned =
                data["disallowed_faces"].as_array().is_some_and(|a| a.iter().any(|f| f.as_str() == Some(face.name())));
            if banned {
                continue;
            }
            let Some(ns) = self.eval_state_provider(&data["block_state_provider"], p) else { continue };
            let ns = if data["update_from_neighbors"].as_bool().unwrap_or(true) {
                self.update_from_neighbour_shapes(ns, p)
            } else {
                ns
            };
            self.trig_location("minecraft:item_used_on_block", p, Some(stack));
            let old = self.get(p);
            if let Some(table) = data["loot"].as_str() {
                let drops = self.block_interact_drops(table, p, old, stack);
                let from_face = data["drop_strategy"].as_str() == Some("clicked_face");
                for d in drops {
                    if from_face {
                        self.pop_resource_from_face(p, face, d);
                    } else {
                        self.pop_resource(p, d);
                    }
                }
            }
            if self.life.items.max_stack(stack) > 1 {
                if data["consume_on_use"].as_bool().unwrap_or(true) {
                    self.consume_held(slot);
                }
            } else {
                let n = data["item_damage_per_use"].as_i64().unwrap_or(0) as i32;
                if n > 0 {
                    self.hurt_item(slot, n);
                }
            }
            self.set_block(p, ns, 11);
            // Sound and particles sent to everyone except the player (predicted by their client).
            self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(ns));
            if data["transform_type"].as_str() == Some("copper_chest")
                && self.v().get(old, "type").is_some_and(|t| t != "single")
            {
                let q = self.connected_chest(p, old);
                let qs = self.get(q);
                self.game_event_block("minecraft:block_change", q, Some(Ent::Player), Some(qs));
            }
            return true;
        }
        false
    }

    /// Neighbour of a double chest (left side: clockwise of the gaze, right: counter-clockwise).
    fn connected_chest(&self, p: P, s: u32) -> P {
        let facing = self.v().facing(s);
        let d = if self.v().get(s, "type") == Some("left") { facing.clockwise() } else { facing.counter_clockwise() };
        rel(p, d)
    }

    /// State provider of the transformers: `rule_based` (rules then fallback), simple state
    /// `{id, properties}` or `copy_properties` (properties of the old state copied).
    fn eval_state_provider(&self, v: &Value, p: P) -> Option<u32> {
        match v["type"].as_str() {
            Some("minecraft:rule_based") => {
                for r in v["rules"].as_array()? {
                    if self.eval_block_predicate(&r["if_true"], p) {
                        return self.eval_state_provider(&r["then"], p);
                    }
                }
                let fb = &v["fallback"];
                if fb.is_null() { None } else { self.eval_state_provider(fb, p) }
            }
            Some("minecraft:copy_properties") => {
                let src = &v["source"];
                let name = src["id"].as_str()?;
                let mut props: Vec<(String, String)> = src["properties"]
                    .as_object()
                    .map(|m| m.iter().filter_map(|(k, x)| Some((k.clone(), x.as_str()?.to_owned()))).collect())
                    .unwrap_or_default();
                for (k, x) in self.props_of(self.get(p)) {
                    match props.iter_mut().find(|(n, _)| *n == k) {
                        Some(e) => e.1 = x,
                        None => props.push((k, x)),
                    }
                }
                self.state_with(name, &props)
            }
            Some("minecraft:simple_state_provider") => self.eval_state_provider(&v["state"], p),
            _ => {
                let name = v["id"].as_str().or_else(|| v["Name"].as_str())?;
                let props: Vec<(String, String)> = v["properties"]
                    .as_object()
                    .or_else(|| v["Properties"].as_object())
                    .map(|m| m.iter().filter_map(|(k, x)| Some((k.clone(), x.as_str()?.to_owned()))).collect())
                    .unwrap_or_default();
                self.state_with(name, &props)
            }
        }
    }

    /// Block predicates of the transformers (`all_of`, `any_of`, `not`, `matching_blocks`,
    /// `matching_block_tag`, `offset` shift).
    fn eval_block_predicate(&self, v: &Value, p: P) -> bool {
        let off = v["offset"]
            .as_array()
            .map_or([0, 0, 0], |a| [0, 1, 2].map(|i| a.get(i).and_then(Value::as_i64).unwrap_or(0) as i32));
        let q = [p[0] + off[0], p[1] + off[1], p[2] + off[2]];
        match v["type"].as_str() {
            Some("minecraft:all_of") => {
                v["predicates"].as_array().is_some_and(|a| a.iter().all(|x| self.eval_block_predicate(x, p)))
            }
            Some("minecraft:any_of") => {
                v["predicates"].as_array().is_some_and(|a| a.iter().any(|x| self.eval_block_predicate(x, p)))
            }
            Some("minecraft:not") => !self.eval_block_predicate(&v["predicate"], p),
            Some("minecraft:true") => true,
            Some("minecraft:matching_blocks") => {
                let name = self.state_name(self.get(q)).to_owned();
                match &v["blocks"] {
                    Value::String(b) if b.starts_with('#') => self.btag(q, &b[1..]),
                    Value::String(b) => *b == name,
                    Value::Array(a) => a.iter().any(|x| x.as_str() == Some(name.as_str())),
                    _ => false,
                }
            }
            Some("minecraft:matching_block_tag") => v["tag"].as_str().is_some_and(|t| self.btag(q, t)),
            _ => false,
        }
    }

    /// Interaction loot table with a block (tool = held item), drawn in its sequence.
    fn block_interact_drops(&mut self, table: &str, p: P, s: u32, tool: &Stack) -> Vec<Stack> {
        let name = self.state_name(s).to_owned();
        let be = self.block_entity(p).cloned();
        let ctx = crate::loot::Ctx {
            block: Some((name.as_str(), self.props_of(s))),
            tool: Some(Some(tool)),
            block_entity: be.as_ref(),
            ..Default::default()
        };
        crate::mobs::surface::loot(self, table, &ctx)
    }

    /// Item pushed out of the clicked face.
    fn pop_resource_from_face(&mut self, p: P, face: Dir, stack: Stack) {
        if !self.rule("minecraft:block_drops", true) {
            return;
        }
        let st = face.step().map(f64::from);
        let half = 0.25 / 2.0; // half the width and height of an item
        let r = &mut self.life.rng;
        let mut between = |a: f64, b: f64| a + r.next_double() * (b - a);
        let x = f64::from(p[0]) + 0.5 + if st[0] == 0.0 { between(-0.25, 0.25) } else { st[0] * (0.5 + half) };
        let y = f64::from(p[1]) + 0.5 + if st[1] == 0.0 { between(-0.25, 0.25) } else { st[1] * (0.5 + half) } - half;
        let z = f64::from(p[2]) + 0.5 + if st[2] == 0.0 { between(-0.25, 0.25) } else { st[2] * (0.5 + half) };
        let vx = if st[0] == 0.0 { between(-0.1, 0.1) } else { st[0] * 0.1 };
        let vy = if st[1] == 0.0 { between(0.0, 0.1) } else { st[1] * 0.1 + 0.1 };
        let vz = if st[2] == 0.0 { between(-0.1, 0.1) } else { st[2] * 0.1 };
        self.spawn_item([x, y, z], [vx, vy, vz], stack, 10);
    }

    // --- Wax. ---

    /// Corresponding waxed state (inverse of axe "unwaxing"),
    /// honeycomb consumed, placement with flags 11, `block_change` event.
    fn wax_use(&mut self, slot: usize, p: P, stack: &Stack) -> bool {
        let s = self.get(p);
        let name = self.state_name(s).to_owned();
        let Some(waxed) = self.waxed_of(&name) else { return false };
        let Some(ns) = self.state_with(&waxed, &self.props_of(s)) else { return false };
        self.trig_location("minecraft:item_used_on_block", p, Some(stack));
        self.consume_held(slot);
        self.set_block(p, ns, 11);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(ns));
        if name.ends_with("chest") && self.v().get(s, "type").is_some_and(|t| t != "single") {
            let q = self.connected_chest(p, s);
            let qs = self.get(q);
            self.game_event_block("minecraft:block_change", q, Some(Ent::Player), Some(qs));
        }
        true
    }

    /// Waxed block of a copper block: re-read from the "unwaxing" rules of the axe
    /// transformer (`waxed_X` → `X`).
    pub(crate) fn waxed_of(&self, name: &str) -> Option<String> {
        let axe = self.life.pack.get("block_transformer", "axe")?;
        for data in axe.as_array()? {
            if data["particle"].as_str() != Some("wax_off") {
                continue;
            }
            for r in data["block_state_provider"]["rules"].as_array()? {
                if r["then"]["source"]["id"].as_str() == Some(name)
                    && let Some(b) = r["if_true"]["blocks"].as_str()
                {
                    return Some(b.to_owned());
                }
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "w8b_tests.rs"]
mod w8b_tests;
