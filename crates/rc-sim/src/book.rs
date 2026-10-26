//! Recipe book sent to the client (`creative-inventory.md` § 9): displays of known
//! recipes (identifier = rank of the recipe on our side), categories, groups, and sets
//! of items accepted by furnaces (`update_recipes`).

use rc_client::Event;
use rc_client::recipes::{HolderSet, RecipeDisplay, RecipeEntry, SlotDisplay};
use serde_json::Value;

use crate::recipes::{Body, CookKind};
use crate::server::Server;

impl Server {
    /// Display of an ingredient: the set of accepted items as is (tag or list
    /// of items), like the game, even for a single item.
    fn slot_display(&self, v: &Value) -> SlotDisplay {
        self.holder_set(v).map_or(SlotDisplay::Empty, SlotDisplay::Tag)
    }

    /// Optional ingredient (smithing): nothing displayed if missing.
    fn opt_slot_display(&self, v: &Value) -> SlotDisplay {
        if v.is_null() { SlotDisplay::Empty } else { self.slot_display(v) }
    }

    fn holder_set(&self, v: &Value) -> Option<HolderSet> {
        match v {
            Value::String(s) => match s.strip_prefix('#') {
                Some(tag) => Some(HolderSet::Tag(tag.to_owned())),
                None => self.life.items.id(s).map(|i| HolderSet::Ids(vec![i])),
            },
            Value::Array(a) => {
                Some(HolderSet::Ids(a.iter().filter_map(Value::as_str).filter_map(|s| self.life.items.id(s)).collect()))
            }
            _ => None,
        }
    }

    fn result_display(&self, v: &Value) -> SlotDisplay {
        let Some(t) = v["id"].as_str() else { return SlotDisplay::Empty };
        let s = crate::player::Stack {
            id: t.to_owned(),
            count: v["count"].as_i64().unwrap_or(1) as i32,
            components: v.get("components").filter(|c| c.is_object()).map(crate::recipes::json_to_nbt),
        };
        self.net(Some(&s)).map_or(SlotDisplay::Empty, SlotDisplay::Stack)
    }

    fn station(&self, name: &str) -> SlotDisplay {
        self.life.items.id(name).map_or(SlotDisplay::Empty, SlotDisplay::Item)
    }

    fn category(&self, name: &str) -> i32 {
        self.data
            .registry("minecraft:recipe_book_category")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .map_or(0, |i| i as i32)
    }

    /// Result stack displayed: item `id`, count, components of the recipe result.
    fn book_stack(&self, id: &str, count: i32, result: &Value) -> SlotDisplay {
        let s = crate::player::Stack {
            id: id.to_owned(),
            count,
            components: result.get("components").filter(|c| c.is_object()).map(crate::recipes::json_to_nbt),
        };
        self.net(Some(&s)).map_or(SlotDisplay::Empty, SlotDisplay::Stack)
    }

    /// Entries of a recipe's book, one per display (flags: 1 notification,
    /// 2 highlight); none for a special recipe.
    pub(crate) fn book_entries(&self, id: &str, flags: u8) -> Vec<(RecipeEntry, u8)> {
        let Some(index) = self.life.recipes.index(id) else { return Vec::new() };
        let group = self.life.recipes.group_id(index);
        self.life
            .recipes
            .displays(index)
            .zip(0..)
            .filter_map(|(d, sub)| {
                let (display, category, requirements) = self.book_display(index, sub)?;
                let category = rc_data::ids::with_key(&["minecraft:", &category], |k| self.category(k));
                Some((RecipeEntry { id: d, display, group, category, requirements }, flags))
            })
            .collect()
    }

    /// Display `sub` of recipe `index` (the number of materials of a transmutation),
    /// its book category and the sets of items to place.
    #[allow(clippy::type_complexity)]
    fn book_display(&self, index: usize, sub: usize) -> Option<(RecipeDisplay, String, Option<Vec<HolderSet>>)> {
        use crate::recipes::Craft;
        let r = &self.life.recipes.list[index];
        let v = self.life.pack.get("recipe", &r.id)?;
        let cat = r.category.as_deref().unwrap_or("misc");
        let table = || self.station("minecraft:crafting_table");
        let crafting = || format!("crafting_{}", crafting_cat(cat));
        let (display, category, reqs) = match &r.body {
            Body::Craft(Craft::Transmute { min, max, add_count, result, .. }) => {
                let k = min + sub as i32;
                let material = self.slot_display(&v["material"]);
                let mut ingredients = vec![self.slot_display(&v["input"])];
                ingredients.extend((0..k).map(|_| material.clone()));
                let count = if *add_count { k + result.count } else { result.count };
                // Without a result item: one stack per item accepted as input.
                let shown = if result.id.is_empty() {
                    let Body::Craft(Craft::Transmute { input, .. }) = &r.body else { return None };
                    SlotDisplay::Composite(
                        input
                            .iter()
                            .filter_map(|&i| self.life.items.name(i))
                            .map(|n| self.book_stack(n, count, &v["result"]))
                            .collect(),
                    )
                } else {
                    self.book_stack(&result.id, count, &v["result"])
                };
                let d = RecipeDisplay::Shapeless { ingredients, result: shown, station: table() };
                let mut reqs: Vec<HolderSet> = self.holder_set(&v["input"]).into_iter().collect();
                reqs.extend((0..*max).filter_map(|_| self.holder_set(&v["material"])));
                (d, crafting(), Some(reqs))
            }
            Body::Craft(Craft::Dye { .. }) => {
                let dye_comp = self
                    .data
                    .registry("minecraft:data_component_type")
                    .and_then(|r| r.entries.iter().position(|e| e == "minecraft:dye"))
                    .map_or(0, |i| i as i32);
                let dyes = SlotDisplay::OnlyWithComponent(Box::new(self.slot_display(&v["dye"])), dye_comp);
                let target = self.slot_display(&v["target"]);
                let d = RecipeDisplay::Shapeless {
                    ingredients: vec![target.clone(), dyes.clone()],
                    result: SlotDisplay::Dyed(Box::new(dyes), Box::new(target)),
                    station: table(),
                };
                let reqs = [&v["target"], &v["dye"]].iter().filter_map(|x| self.holder_set(x)).collect();
                (d, crafting(), Some(reqs))
            }
            Body::Craft(Craft::Imbue { result, .. }) => {
                let m = self.slot_display(&v["material"]);
                let source = SlotDisplay::WithAnyPotion(Box::new(self.slot_display(&v["source"])));
                let mut ingredients = vec![m.clone(); 9];
                ingredients[4] = source;
                let d = RecipeDisplay::Shaped {
                    width: 3,
                    height: 3,
                    ingredients,
                    result: SlotDisplay::WithAnyPotion(Box::new(self.book_stack(
                        &result.id,
                        result.count,
                        &v["result"],
                    ))),
                    station: table(),
                };
                let (mh, sh) = (self.holder_set(&v["material"])?, self.holder_set(&v["source"])?);
                let mut reqs = vec![mh; 9];
                reqs[4] = sh;
                (d, crafting(), Some(reqs))
            }
            Body::Stonecut { .. } => {
                let d = RecipeDisplay::Stonecutter {
                    input: self.slot_display(&v["ingredient"]),
                    result: self.result_display(&v["result"]),
                    station: self.station("minecraft:stonecutter"),
                };
                (d, "stonecutter".to_owned(), self.holder_set(&v["ingredient"]).map(|h| vec![h]))
            }
            Body::Other(t) if t == "smithing_transform" || t == "smithing_trim" => {
                let base = self.slot_display(&v["base"]);
                let result = if t == "smithing_trim" {
                    SlotDisplay::SmithingTrim(Box::new(base.clone()), Box::new(self.slot_display(&v["addition"])))
                } else {
                    self.result_display(&v["result"])
                };
                let d = RecipeDisplay::Smithing {
                    template: self.opt_slot_display(&v["template"]),
                    base,
                    addition: self.opt_slot_display(&v["addition"]),
                    result,
                    station: self.station("minecraft:smithing_table"),
                };
                let reqs = ["template", "base", "addition"].iter().filter_map(|k| self.holder_set(&v[*k])).collect();
                (d, "smithing".to_owned(), Some(reqs))
            }
            Body::Craft(crate::recipes::Craft::Shaped { w, h, .. }) => {
                let rows: Vec<&str> = v["pattern"].as_array()?.iter().filter_map(Value::as_str).collect();
                // Pattern trimmed as on reading: we take the non-empty cells again.
                let w0 = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
                let cell = |x: usize, y: usize| rows[y].chars().nth(x).unwrap_or(' ');
                let ys: Vec<usize> = (0..rows.len()).filter(|&y| (0..w0).any(|x| cell(x, y) != ' ')).collect();
                let xs: Vec<usize> = (0..w0).filter(|&x| (0..rows.len()).any(|y| cell(x, y) != ' ')).collect();
                let (x0, y0) = (*xs.first()?, *ys.first()?);
                let mut ings = Vec::new();
                let mut reqs = Vec::new();
                for y in y0..y0 + h {
                    for x in x0..x0 + w {
                        let c = cell(x, y);
                        if c == ' ' {
                            ings.push(SlotDisplay::Empty);
                        } else {
                            let k = &v["key"][c.to_string()];
                            ings.push(self.slot_display(k));
                            reqs.extend(self.holder_set(k));
                        }
                    }
                }
                let d = RecipeDisplay::Shaped {
                    width: *w as i32,
                    height: *h as i32,
                    ingredients: ings,
                    result: self.result_display(&v["result"]),
                    station: self.station("minecraft:crafting_table"),
                };
                (d, format!("crafting_{}", crafting_cat(cat)), Some(reqs))
            }
            Body::Craft(crate::recipes::Craft::Shapeless { .. }) => {
                let list = v["ingredients"].as_array()?;
                let d = RecipeDisplay::Shapeless {
                    ingredients: list.iter().map(|i| self.slot_display(i)).collect(),
                    result: self.result_display(&v["result"]),
                    station: self.station("minecraft:crafting_table"),
                };
                (
                    d,
                    format!("crafting_{}", crafting_cat(cat)),
                    Some(list.iter().filter_map(|i| self.holder_set(i)).collect()),
                )
            }
            Body::Cook(c) => {
                let (station, category) = match c.kind {
                    CookKind::Smelting => ("minecraft:furnace", format!("furnace_{}", cook_cat(cat))),
                    CookKind::Blasting => (
                        "minecraft:blast_furnace",
                        format!("blast_furnace_{}", if cat == "blocks" { "blocks" } else { "misc" }),
                    ),
                    CookKind::Smoking => ("minecraft:smoker", "smoker_food".to_owned()),
                    CookKind::Campfire => ("minecraft:campfire", "campfire".to_owned()),
                };
                let d = RecipeDisplay::Furnace {
                    ingredient: self.slot_display(&v["ingredient"]),
                    fuel: SlotDisplay::AnyFuel,
                    result: self.result_display(&v["result"]),
                    station: self.station(station),
                    time: c.time,
                    xp: c.xp,
                };
                (d, category, self.holder_set(&v["ingredient"]).map(|h| vec![h]))
            }
            _ => return None,
        };
        Some((display, category, reqs))
    }

    /// `place_recipe` (click in the book): the grid is emptied into the inventory then filled
    /// with the ingredients found (once, or as many times as possible with Shift); failing that, the
    /// ghost recipe is sent back.
    pub(crate) fn place_recipe(&mut self, window: i32, id: i32, max: bool) {
        use crate::menus::Kind;
        if self.game_mode == 3 || window != self.current_window() {
            return;
        }
        // Book display number → recipe (and display rank, for the ghost recipe).
        let Some((ridx, sub)) = self.life.recipes.of_display(id) else { return };
        let r = self.life.recipes.list[ridx].clone();
        if !self.life.known.contains(&r.id) {
            return;
        }
        let open = window != 0;
        let kind = if open { self.life.menu.as_ref().map(|m| m.kind) } else { Some(Kind::Inventory) };
        // Menu grid: first slot, side (furnaces: slot 0, side 1).
        let (first, side) = match kind {
            Some(Kind::Inventory) => (1usize, 2usize),
            Some(Kind::Crafting) => (1, 3),
            Some(Kind::Furnace(_)) => (0, 1),
            _ => return,
        };
        // W8c: placement ingredients (non-empty slots in order), input per slot. Shapeless
        // recipes (transmutation, dyeing, impregnation) are placed like a shapeless
        // recipe, over the whole grid, in the order of their ingredients.
        use crate::recipes::{Craft, Ingredient};
        let unshaped = |ings: Vec<Ingredient>| {
            let n = ings.len() as i32;
            (ings.len() <= side * side).then(|| (ings, (0..n).collect::<Vec<i32>>(), side, side))
        };
        let placed = match &r.body {
            Body::Craft(Craft::Shaped { w, h, cells, .. }) if *w <= side && *h <= side => {
                let mut ings = Vec::new();
                let entries = cells
                    .iter()
                    .map(|c| match c {
                        Some(i) => {
                            ings.push(i.clone());
                            ings.len() as i32 - 1
                        }
                        None => -1,
                    })
                    .collect();
                Some((ings, entries, *w, *h))
            }
            Body::Craft(Craft::Shapeless { ings, .. }) => unshaped(ings.clone()),
            Body::Craft(Craft::Transmute { input, material, max, .. }) => {
                let mut ings = vec![input.clone()];
                ings.extend((0..*max).map(|_| material.clone()));
                unshaped(ings)
            }
            Body::Craft(Craft::Dye { target, dye, .. }) => unshaped(vec![target.clone(), dye.clone()]),
            Body::Craft(Craft::Imbue { source, material, .. }) => {
                let mut ings = vec![material.clone(); 9];
                ings[4] = source.clone();
                unshaped(ings)
            }
            Body::Cook(c) if side == 1 => Some((vec![c.ingredient.clone()], vec![0], 1, 1)),
            _ => None,
        };
        let Some((ings, entries, rw, rh)) = placed else { return };
        let furnace = side == 1;
        let creative = self.game_mode == 1;
        let outcome = self.with_menu(open, |m, inv, env| {
            let grid: Vec<usize> = (first..first + side * side).collect();
            let matches = if furnace {
                let input = m.get(inv, 0).cloned();
                match (&r.body, input) {
                    (Body::Cook(c), Some(s)) => env.recipes.cooking(env.items, c.kind, &s).is_some_and(|x| x.0 == ridx),
                    _ => false,
                }
            } else {
                let cells: Vec<Option<crate::player::Stack>> = grid.iter().map(|&i| m.get(inv, i).cloned()).collect();
                let maps = env.maps;
                env.recipes.craft(env.items, &cells, side, &|id| maps.scale_of(id)).is_some_and(|x| x.0 == ridx)
            };
            let plan = crate::book::place::Plan {
                ings,
                entries,
                rw,
                rh,
                clear: if furnace { vec![0, 2] } else { grid.clone() },
                counted: if furnace { vec![0, 1, 2] } else { grid.clone() },
                counted_simple: !furnace,
                grid,
                side,
                matches,
                creative,
            };
            m.own_dirty = true;
            m.book_place(env, inv, &plan, max)
        });
        if outcome == Some(crate::book::place::Outcome::Ghost)
            && let Some((display, _, _)) = self.book_display(ridx, sub)
        {
            let ghost = rc_client::recipes::RecipeEvent::Ghost { window, display: Box::new(display) };
            let _ = self.send(Event::Recipes(ghost));
        }
        // Result recomputed and differences sent.
        self.with_menu_result(open);
        self.after_menu_action();
    }

    /// `recipe_book_seen_recipe`: the recipe (rank) is no longer "new".
    pub(crate) fn book_seen(&mut self, id: i32) {
        if let Some(r) = self.life.recipes.of_display(id).map(|(i, _)| &self.life.recipes.list[i]) {
            let rid = r.id.clone();
            self.life.highlight.remove(&rid);
        }
    }

    /// `recipe_book_change_settings`: book 0 crafting table, 1 furnace, 2 blast furnace, 3 smoker.
    pub(crate) fn book_settings(&mut self, book: i32, open: bool, filter: bool) {
        let (k_open, k_filter) = match book {
            0 => ("isGuiOpen", "isFilteringCraftable"),
            1 => ("isFurnaceGuiOpen", "isFurnaceFilteringCraftable"),
            2 => ("isBlastingFurnaceGuiOpen", "isBlastingFurnaceFilteringCraftable"),
            3 => ("isSmokerGuiOpen", "isSmokerFilteringCraftable"),
            _ => return,
        };
        use rc_proto::nbt::Nbt;
        let root = &mut self.pdata.root;
        let mut b = root.get("recipeBook").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut b, k_open, Nbt::Byte(i8::from(open)));
        crate::nbtio::set(&mut b, k_filter, Nbt::Byte(i8::from(filter)));
        crate::nbtio::set(root, "recipeBook", b);
    }

    /// `update_recipes`: items accepted by each furnace.
    pub(crate) fn send_recipe_sets(&self) {
        let mut sets = Vec::new();
        for (key, kind) in [
            ("minecraft:furnace_input", CookKind::Smelting),
            ("minecraft:blast_furnace_input", CookKind::Blasting),
            ("minecraft:smoker_input", CookKind::Smoking),
            ("minecraft:campfire_input", CookKind::Campfire),
        ] {
            let mut ids: Vec<i32> = self
                .life
                .recipes
                .list
                .iter()
                .filter_map(|r| match &r.body {
                    Body::Cook(c) if c.kind == kind => Some(c.ingredient.clone()),
                    _ => None,
                })
                .flatten()
                .collect();
            ids.sort_unstable();
            ids.dedup();
            sets.push((key.to_owned(), ids));
        }
        let mut rec = rc_proto_v777::special::Recipes { sets, stonecutter: Vec::new() };
        self.station_recipe_sets(&mut rec); // S7
        let _ = self.send(Event::UpdateRecipes(Box::new(rec)));
    }
}

fn crafting_cat(c: &str) -> &str {
    match c {
        "building" => "building_blocks",
        "redstone" => "redstone",
        "equipment" => "equipment",
        _ => "misc",
    }
}

fn cook_cat(c: &str) -> &str {
    match c {
        "food" => "food",
        "blocks" => "blocks",
        _ => "misc",
    }
}

#[path = "book_place.rs"]
pub(crate) mod place;
