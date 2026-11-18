//! Network form of recipe packets (book, item sets, stonecutter).

use rc_client::Event;
use rc_client::ids::Ids;
use rc_client::recipes::{HolderSet, RecipeDisplay, RecipeEntry, RecipeEvent, SlotDisplay};
use rc_proto::Encode;
use rc_proto::varint::write_varint;

use super::encode::write_template;
use crate::server::Server;

fn holder_set(w: &mut Vec<u8>, h: &HolderSet) {
    match h {
        HolderSet::Tag(t) => {
            write_varint(w, 0);
            rc_proto::write_str(w, t);
        }
        HolderSet::Ids(v) => {
            write_varint(w, v.len() as i32 + 1);
            for x in v {
                write_varint(w, *x);
            }
        }
    }
}

fn slot(_sv: &Server, w: &mut Vec<u8>, s: &SlotDisplay, trim_pattern: i32) {
    match s {
        SlotDisplay::Empty => write_varint(w, 0),
        SlotDisplay::AnyFuel => write_varint(w, 1),
        SlotDisplay::WithAnyPotion(x) => {
            write_varint(w, 2);
            slot(_sv, w, x, trim_pattern);
        }
        SlotDisplay::OnlyWithComponent(x, c) => {
            write_varint(w, 3);
            slot(_sv, w, x, trim_pattern);
            write_varint(w, *c);
        }
        SlotDisplay::Item(i) => {
            write_varint(w, 4);
            write_varint(w, *i);
        }
        SlotDisplay::Stack(st) => {
            write_varint(w, 5);
            write_template(w, st);
        }
        SlotDisplay::Tag(h) => {
            write_varint(w, 6);
            holder_set(w, h);
        }
        SlotDisplay::Dyed(a, b) => {
            write_varint(w, 7);
            slot(_sv, w, a, trim_pattern);
            slot(_sv, w, b, trim_pattern);
        }
        SlotDisplay::SmithingTrim(base, material) => {
            write_varint(w, 8);
            slot(_sv, w, base, trim_pattern);
            slot(_sv, w, material, trim_pattern);
            write_varint(w, trim_pattern + 1);
        }
        SlotDisplay::WithRemainder(x) => {
            write_varint(w, 9);
            slot(_sv, w, x, trim_pattern);
            write_varint(w, 0);
        }
        SlotDisplay::Composite(list) => {
            write_varint(w, 10);
            write_varint(w, list.len() as i32);
            for x in list {
                slot(_sv, w, x, trim_pattern);
            }
        }
    }
}

fn display(sv: &Server, w: &mut Vec<u8>, d: &RecipeDisplay, tp: i32) {
    match d {
        RecipeDisplay::Shapeless { ingredients, result, station } => {
            write_varint(w, 0);
            write_varint(w, ingredients.len() as i32);
            for i in ingredients {
                slot(sv, w, i, tp);
            }
            slot(sv, w, result, tp);
            slot(sv, w, station, tp);
        }
        RecipeDisplay::Shaped { width, height, ingredients, result, station } => {
            write_varint(w, 1);
            write_varint(w, *width);
            write_varint(w, *height);
            write_varint(w, ingredients.len() as i32);
            for i in ingredients {
                slot(sv, w, i, tp);
            }
            slot(sv, w, result, tp);
            slot(sv, w, station, tp);
        }
        RecipeDisplay::Furnace { ingredient, fuel, result, station, time, xp } => {
            write_varint(w, 2);
            slot(sv, w, ingredient, tp);
            slot(sv, w, fuel, tp);
            slot(sv, w, result, tp);
            slot(sv, w, station, tp);
            write_varint(w, *time);
            xp.encode(w);
        }
        RecipeDisplay::Stonecutter { input, result, station } => {
            write_varint(w, 3);
            slot(sv, w, input, tp);
            slot(sv, w, result, tp);
            slot(sv, w, station, tp);
        }
        RecipeDisplay::Smithing { template, base, addition, result, station } => {
            write_varint(w, 4);
            slot(sv, w, template, tp);
            slot(sv, w, base, tp);
            slot(sv, w, addition, tp);
            slot(sv, w, result, tp);
            slot(sv, w, station, tp);
        }
    }
}

/// Trim pattern of the smithing recipe of a book display (registry index), 0 otherwise.
fn trim_pattern(sv: &Server, display: i32) -> i32 {
    let Some(r) = sv.life.recipes.of_display(display).map(|(i, _)| &sv.life.recipes.list[i]) else { return 0 };
    let Some(v) = sv.life.pack.get("recipe", &r.id) else { return 0 };
    let Some(p) = v["pattern"].as_str() else { return 0 };
    let p = crate::commands::args::id(p);
    sv.regs.entries("minecraft:trim_pattern").iter().position(|x| *x == p).map_or(0, |i| i as i32)
}

fn entry(sv: &Server, w: &mut Vec<u8>, e: &RecipeEntry) {
    write_varint(w, e.id);
    display(sv, w, &e.display, trim_pattern(sv, e.id));
    write_varint(w, e.group.map_or(0, |g| g + 1));
    write_varint(w, e.category);
    e.requirements.is_some().encode(w);
    if let Some(r) = &e.requirements {
        write_varint(w, r.len() as i32);
        for h in r {
            holder_set(w, h);
        }
    }
}

pub(crate) fn recipe_event(sv: &Server, ids: &Ids, r: &RecipeEvent, out: &mut Vec<Vec<u8>>) {
    let mut w = Vec::new();
    match r {
        RecipeEvent::Add(list, replace) => {
            write_varint(&mut w, ids.recipe_book_add);
            write_varint(&mut w, list.len() as i32);
            for (e, flags) in list {
                entry(sv, &mut w, e);
                w.push(*flags);
            }
            replace.encode(&mut w);
        }
        RecipeEvent::Remove(list) => {
            write_varint(&mut w, ids.recipe_book_remove);
            write_varint(&mut w, list.len() as i32);
            for x in list {
                write_varint(&mut w, *x);
            }
        }
        RecipeEvent::Settings(s) => {
            write_varint(&mut w, ids.recipe_book_settings);
            for (open, filter) in s {
                open.encode(&mut w);
                filter.encode(&mut w);
            }
        }
        RecipeEvent::Ghost { window, display: d } => {
            write_varint(&mut w, ids.place_ghost_recipe);
            write_varint(&mut w, *window);
            display(sv, &mut w, d, 0);
        }
    }
    out.push(w);
}

pub(crate) fn update_recipes(_sv: &Server, w: &mut Vec<u8>, r: &rc_proto_v777::special::Recipes) {
    use rc_proto_v777::special::Holders;
    write_varint(w, r.sets.len() as i32);
    for (k, ids) in &r.sets {
        rc_proto::write_str(w, k);
        write_varint(w, ids.len() as i32);
        for i in ids {
            write_varint(w, *i);
        }
    }
    write_varint(w, r.stonecutter.len() as i32);
    for (input, shown) in &r.stonecutter {
        match input {
            Holders::Tag(t) => {
                write_varint(w, 0);
                rc_proto::write_str(w, t);
            }
            Holders::Items(v) => {
                write_varint(w, v.len() as i32 + 1);
                for x in v {
                    write_varint(w, *x);
                }
            }
        }
        match shown {
            Some(s) => {
                write_varint(w, 5);
                write_template(w, s);
            }
            None => write_varint(w, 0),
        }
    }
}

/// Avoids the "unused" warning when `Event` is referenced only by the types.
#[allow(dead_code)]
fn _uses(_: &Event) {}
