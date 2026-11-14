//! Rules on item stacks, their identifiers and their components.

use rc_proto::nbt::Nbt;

use super::{compound, field_mut, plain_translation_key, remove, set};
use crate::Context;
use crate::walk::namespaced_id;

/// A list of four item identifiers (back, left, right, front) as four item stacks, one per side:
/// a missing or non-text entry is a brick; an empty identifier keeps the whole list as it was
/// (`None`). Pot decorations (4996) and decorated pot sherds (4996.1) share it.
pub(crate) fn id_list_to_sides(list: &Nbt) -> Option<Nbt> {
    let ids: Vec<Option<&str>> = match list {
        Nbt::List(entries) => entries.iter().map(Nbt::as_str).collect(),
        // Number arrays read as lists of numbers: no entry is an identifier.
        Nbt::ByteArray(a) => vec![None; a.len()],
        Nbt::IntArray(a) => vec![None; a.len()],
        Nbt::LongArray(a) => vec![None; a.len()],
        _ => return None,
    };
    let mut sides = Vec::with_capacity(4);
    for (i, side) in ["back", "left", "right", "front"].into_iter().enumerate() {
        let id = ids.get(i).copied().flatten().unwrap_or("minecraft:brick");
        if id.is_empty() {
            return None;
        }
        sides.push((side.to_owned(), compound([("id", Nbt::String(id.to_owned()))])));
    }
    Some(Nbt::Compound(sides))
}

/// 4996: the `pot_decorations` component, a list of ids, becomes four sides.
pub(crate) fn pot_decorations_to_sides(components: &mut Nbt, _: &Context<'_>) {
    if let Some(decorations) = field_mut(components, "minecraft:pot_decorations")
        && let Some(sides) = id_list_to_sides(decorations)
    {
        *decorations = sides;
    }
}

/// 5007: the `swing_animation` component becomes both `attack_animation` and `interact_animation`.
pub(crate) fn split_swing_animation(components: &mut Nbt, _: &Context<'_>) {
    if let Some(animation) = remove(components, "minecraft:swing_animation") {
        set(components, "minecraft:attack_animation", animation.clone());
        set(components, "minecraft:interact_animation", animation);
    }
}

/// 5008: the `map_color` component is dropped.
pub(crate) fn remove_map_color(components: &mut Nbt, _: &Context<'_>) {
    remove(components, "minecraft:map_color");
}

/// 5014: the inline `block_transformer` component is dropped.
pub(crate) fn remove_block_transformer(components: &mut Nbt, _: &Context<'_>) {
    remove(components, "minecraft:block_transformer");
}

/// Per explorer decoration type (the `+` decoration of a filled map): the map's own item, and
/// the name key the map carried until then.
const EXPLORER_MAPS: &[(&str, &str, &str)] = &[
    ("minecraft:mansion", "minecraft:woodland_explorer_map", "filled_map.mansion"),
    ("minecraft:monument", "minecraft:ocean_explorer_map", "filled_map.monument"),
    ("minecraft:trial_chambers", "minecraft:trial_explorer_map", "filled_map.trial_chambers"),
    ("minecraft:jungle_temple", "minecraft:jungle_explorer_map", "filled_map.explorer_jungle"),
    ("minecraft:swamp_hut", "minecraft:swamp_explorer_map", "filled_map.explorer_swamp"),
    ("minecraft:village_desert", "minecraft:desert_village_map", "filled_map.village_desert"),
    ("minecraft:village_plains", "minecraft:plains_village_map", "filled_map.village_plains"),
    ("minecraft:village_savanna", "minecraft:savanna_village_map", "filled_map.village_savanna"),
    ("minecraft:village_snowy", "minecraft:snowy_village_map", "filled_map.village_snowy"),
    ("minecraft:village_taiga", "minecraft:taiga_village_map", "filled_map.village_taiga"),
    ("minecraft:red_x", "minecraft:buried_treasure_map", "filled_map.buried_treasure"),
];

/// 5008: a filled map with an explorer decoration becomes that explorer map's item; its name
/// component goes when it was the old explorer name.
pub(crate) fn explorer_map_items(stack: &mut Nbt, _: &Context<'_>) {
    if stack.get("id").and_then(Nbt::as_str) != Some("minecraft:filled_map") {
        return;
    }
    let Some(components) = stack.get("components") else { return };
    let Some(explorer) = components.get("minecraft:map_decorations").and_then(|d| d.get("+")) else { return };
    let decoration = namespaced_id(explorer.get("type").and_then(Nbt::as_str).unwrap_or(""));
    let Some(&(_, item, old_name_key)) = EXPLORER_MAPS.iter().find(|(kind, _, _)| *kind == decoration) else {
        return;
    };
    set(stack, "id", Nbt::String(item.to_owned()));
    if let Some(components) = field_mut(stack, "components")
        && components.get("minecraft:item_name").and_then(plain_translation_key) == Some(old_name_key)
    {
        remove(components, "minecraft:item_name");
    }
}

/// 5012: explorer map items take the name of what they lead to.
const EXPLORER_MAP_RENAMES: &[(&str, &str)] = &[
    ("minecraft:ocean_explorer_map", "minecraft:ocean_monument_map"),
    ("minecraft:swamp_explorer_map", "minecraft:swamp_hut_map"),
    ("minecraft:trial_explorer_map", "minecraft:buried_trial_chambers_map"),
    ("minecraft:woodland_explorer_map", "minecraft:woodland_mansion_map"),
    ("minecraft:jungle_explorer_map", "minecraft:jungle_pyramid_map"),
];

/// 5012: renames an item identifier from [`EXPLORER_MAP_RENAMES`].
pub(crate) fn rename_explorer_maps(name: &mut Nbt, _: &Context<'_>) {
    let Nbt::String(id) = name else { return };
    let full = namespaced_id(id);
    if let Some(&(_, new)) = EXPLORER_MAP_RENAMES.iter().find(|(old, _)| *old == full) {
        *id = new.to_owned();
    }
}

/// 5016: an empty map named with the old buried treasure key takes the new item key.
pub(crate) fn unfilled_buried_treasure_map(stack: &mut Nbt, _: &Context<'_>) {
    if stack.get("id").and_then(Nbt::as_str) != Some("minecraft:map") {
        return;
    }
    if let Some(name) = field_mut(stack, "components").and_then(|c| field_mut(c, "minecraft:item_name"))
        && plain_translation_key(name) == Some("filled_map.buried_treasure")
    {
        set(name, "translate", Nbt::String("item.minecraft.buried_treasure_map".to_owned()));
    }
}
