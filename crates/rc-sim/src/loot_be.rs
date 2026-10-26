//! W8c: passage between the data of a block entity and item components — "collected"
//! components (contents, name, lock, sealed loot, banner patterns, bees, sherds,
//! head profile) read for `copy_components`, and applied to the block entity on placement from the
//! stack (the other components stay stored in `components`). Rules of game 26.3.

use rc_proto::nbt::Nbt;

use crate::nbtio::{list, set};

/// Families of block entities by identifier.
fn is_container(id: &str) -> bool {
    matches!(
        id,
        "minecraft:chest"
            | "minecraft:trapped_chest"
            | "minecraft:barrel"
            | "minecraft:shulker_box"
            | "minecraft:hopper"
            | "minecraft:dispenser"
            | "minecraft:dropper"
            | "minecraft:furnace"
            | "minecraft:blast_furnace"
            | "minecraft:smoker"
            | "minecraft:brewing_stand"
            | "minecraft:crafter"
    ) || id.ends_with("copper_chest")
}

/// Name carried by the `CustomName` field (the head keeps its own under `custom_name`).
fn nameable(id: &str) -> bool {
    is_container(id)
        || matches!(
            id,
            "minecraft:banner" | "minecraft:enchanting_table" | "minecraft:beacon" | "minecraft:command_block"
        )
}

/// Containers without lock or name whose `Items` become the `container` component.
fn holds_items(id: &str) -> bool {
    matches!(id, "minecraft:campfire" | "minecraft:chiseled_bookshelf" | "minecraft:shelf")
}

fn is_sign(id: &str) -> bool {
    matches!(id, "minecraft:sign" | "minecraft:hanging_sign")
}

/// Containers that also carry sealed loot (`LootTable`, `LootTableSeed`).
fn randomizable(id: &str) -> bool {
    is_container(id)
        && !matches!(id, "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" | "minecraft:brewing_stand")
}

/// Block entity fields that its collected components replace, specific to each type: they
/// are removed from the data copied onto the item (block pick with Ctrl).
pub fn carried(id: &str) -> &'static [&'static str] {
    match id {
        _ if randomizable(id) => &["CustomName", "lock", "Items", "LootTable", "LootTableSeed"],
        _ if is_container(id) => &["CustomName", "lock", "Items"],
        "minecraft:banner" => &["patterns", "CustomName"],
        "minecraft:beacon" => &["CustomName", "lock"],
        "minecraft:beehive" => &["bees"],
        _ if holds_items(id) => &["Items"],
        "minecraft:command_block" => &["CustomName", "conditionMet", "powered"],
        "minecraft:decorated_pot" => &["sherds", "item"],
        _ if is_sign(id) => &["front_text", "back_text", "is_waxed"],
        "minecraft:skull" => &["profile", "note_block_sound", "custom_name"],
        "minecraft:enchanting_table" => &["CustomName"],
        _ => &[],
    }
}

/// `Items` list (`Slot` slots) → `container` component (`slot`, `item`).
fn items_to_container(items: &[Nbt]) -> Nbt {
    let mut v: Vec<(i32, Nbt)> = items
        .iter()
        .filter_map(|it| {
            let slot = it.get("Slot").and_then(Nbt::as_i64)? as i32;
            let mut item = it.clone();
            crate::nbtio::remove(&mut item, "Slot");
            Some((slot, item))
        })
        .collect();
    v.sort_by_key(|x| x.0);
    Nbt::List(
        v.into_iter().map(|(s, i)| Nbt::Compound(vec![("slot".into(), Nbt::Int(s)), ("item".into(), i)])).collect(),
    )
}

/// `container` component → `Items` list.
fn container_to_items(c: &Nbt) -> Nbt {
    Nbt::List(
        list(Some(c))
            .iter()
            .filter_map(|e| {
                let slot = e.get("slot").and_then(Nbt::as_i64)?;
                let mut item = e.get("item")?.clone();
                set(&mut item, "Slot", Nbt::Byte(slot as i8));
                Some(item)
            })
            .collect(),
    )
}

/// Collected components of a block entity, then those it keeps as is
/// (`components`).
pub fn collect(be: &Nbt) -> Vec<(String, Nbt)> {
    let id = be.get("id").and_then(Nbt::as_str).unwrap_or("");
    let mut out: Vec<(String, Nbt)> = Vec::new();
    if let Some(Nbt::Compound(c)) = be.get("components") {
        out.extend(c.iter().cloned());
    }
    let mut put = |k: &str, v: Nbt| {
        out.retain(|(x, _)| x != k);
        out.push((k.to_owned(), v));
    };
    if nameable(id)
        && let Some(n) = be.get("CustomName")
    {
        put("minecraft:custom_name", n.clone());
    }
    if (is_container(id) || id == "minecraft:beacon")
        && let Some(l) = be.get("lock")
    {
        put("minecraft:lock", l.clone());
    }
    if holds_items(id) && !list(be.get("Items")).is_empty() {
        put("minecraft:container", items_to_container(list(be.get("Items"))));
    }
    if is_sign(id) {
        if let Some(t) = be.get("front_text") {
            put("minecraft:sign_text_front", t.clone());
        }
        if let Some(t) = be.get("back_text") {
            put("minecraft:sign_text_back", t.clone());
        }
        if be.get("is_waxed").and_then(Nbt::as_i64).unwrap_or(0) != 0 {
            put("minecraft:waxed", Nbt::Compound(Vec::new()));
        }
    }
    if is_container(id) {
        if let Some(t) = be.get("LootTable") {
            let mut f = vec![("loot_table".to_owned(), t.clone())];
            if let Some(seed) = be.get("LootTableSeed").filter(|s| s.as_i64() != Some(0)) {
                f.push(("seed".into(), seed.clone()));
            }
            put("minecraft:container_loot", Nbt::Compound(f));
        } else if !list(be.get("Items")).is_empty() {
            // Empty contents = default value of the item: nothing to set.
            put("minecraft:container", items_to_container(list(be.get("Items"))));
        }
    }
    match id {
        "minecraft:banner" => {
            if let Some(p) = be.get("patterns").filter(|p| !list(Some(p)).is_empty()) {
                put("minecraft:banner_patterns", p.clone());
            }
        }
        "minecraft:beehive" => {
            if let Some(b) = be.get("bees").filter(|b| !list(Some(b)).is_empty()) {
                put("minecraft:bees", b.clone());
            }
        }
        "minecraft:decorated_pot" => {
            if let Some(s) = be.get("sherds") {
                put("minecraft:pot_decorations", s.clone());
            }
            if let Some(it) = be.get("item") {
                let one =
                    Nbt::List(vec![Nbt::Compound(vec![("slot".into(), Nbt::Int(0)), ("item".into(), it.clone())])]);
                put("minecraft:container", one);
            }
        }
        "minecraft:skull" => {
            if let Some(p) = be.get("profile") {
                put("minecraft:profile", p.clone());
            }
            if let Some(s) = be.get("note_block_sound") {
                put("minecraft:note_block_sound", s.clone());
            }
            if let Some(n) = be.get("custom_name") {
                put("minecraft:custom_name", n.clone());
            }
        }
        _ => {}
    }
    out
}

/// Block entity types whose item data only applies for an operator in
/// creative.
const OP_ONLY: [&str; 6] = [
    "minecraft:command_block",
    "minecraft:lectern",
    "minecraft:sign",
    "minecraft:hanging_sign",
    "minecraft:mob_spawner",
    "minecraft:trial_spawner",
];

/// Components of a stack applied to the placed block entity;
/// `block_entity_data` merged first, only if it is of the type of the placed block entity and, for
/// a reserved type, if `op` (operator in creative; never for a dispenser).
pub fn apply(be: &mut Nbt, comps: &Nbt, op: bool) {
    let Nbt::Compound(fields) = comps else { return };
    let id = be.get("id").and_then(Nbt::as_str).unwrap_or("").to_owned();
    if let Some(Nbt::Compound(data)) = comps.get("minecraft:block_entity_data") {
        let ty = data.iter().find(|(k, _)| k == "id").and_then(|(_, v)| v.as_str()).unwrap_or("");
        let same = if ty.contains(':') { ty == id } else { id.strip_prefix("minecraft:") == Some(ty) };
        if same && (op || !OP_ONLY.contains(&id.as_str())) {
            for (k, v) in data {
                if k != "id" && k != "x" && k != "y" && k != "z" {
                    set(be, k, v.clone());
                }
            }
        }
    }
    let mut kept = Vec::new();
    for (k, v) in fields {
        if k.starts_with('!') || k == "minecraft:block_entity_data" {
            continue;
        }
        let used = match k.as_str() {
            "minecraft:custom_name" if nameable(&id) => {
                set(be, "CustomName", v.clone());
                true
            }
            "minecraft:custom_name" if id == "minecraft:skull" => {
                set(be, "custom_name", v.clone());
                true
            }
            "minecraft:lock" if is_container(&id) || id == "minecraft:beacon" => {
                set(be, "lock", v.clone());
                true
            }
            "minecraft:container" if holds_items(&id) => {
                set(be, "Items", container_to_items(v));
                true
            }
            "minecraft:sign_text_front" if is_sign(&id) => {
                set(be, "front_text", v.clone());
                true
            }
            "minecraft:sign_text_back" if is_sign(&id) => {
                set(be, "back_text", v.clone());
                true
            }
            "minecraft:waxed" if is_sign(&id) => {
                set(be, "is_waxed", Nbt::Byte(1));
                true
            }
            "minecraft:container_loot" if is_container(&id) => {
                if let Some(t) = v.get("loot_table") {
                    set(be, "LootTable", t.clone());
                }
                if let Some(s) = v.get("seed") {
                    set(be, "LootTableSeed", s.clone());
                }
                true
            }
            "minecraft:container" if is_container(&id) => {
                set(be, "Items", container_to_items(v));
                true
            }
            "minecraft:container" if id == "minecraft:decorated_pot" => {
                if let Some(it) = list(Some(v)).first().and_then(|e| e.get("item")) {
                    set(be, "item", it.clone());
                }
                true
            }
            "minecraft:banner_patterns" if id == "minecraft:banner" => {
                set(be, "patterns", v.clone());
                true
            }
            "minecraft:bees" if id == "minecraft:beehive" => {
                set(be, "bees", v.clone());
                true
            }
            "minecraft:pot_decorations" if id == "minecraft:decorated_pot" => {
                set(be, "sherds", v.clone());
                true
            }
            "minecraft:profile" if id == "minecraft:skull" => {
                set(be, "profile", v.clone());
                true
            }
            "minecraft:note_block_sound" if id == "minecraft:skull" => {
                set(be, "note_block_sound", v.clone());
                true
            }
            _ => false,
        };
        if !used {
            kept.push((k.clone(), v.clone()));
        }
    }
    if kept.is_empty() {
        crate::nbtio::remove(be, "components");
    } else {
        set(be, "components", Nbt::Compound(kept));
    }
}
