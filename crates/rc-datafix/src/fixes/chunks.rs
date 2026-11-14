//! Rules on a chunk's own fields (status, terrain blending).

use rc_proto::nbt::Nbt;

use super::{compound, field_mut, remove, set};
use crate::Context;
use crate::walk::namespaced_id;

/// Statuses before any terrain: a chunk there has nothing to blend with.
const BEFORE_TERRAIN: &[&str] =
    &["minecraft:empty", "minecraft:structure_starts", "minecraft:structure_references", "minecraft:biomes"];

/// A status text with its namespace, `default` when the field holds no text.
fn status_of(value: Option<&Nbt>, default: &str) -> String {
    namespaced_id(value.and_then(Nbt::as_str).unwrap_or(default)).into_owned()
}

fn blending_data(min_section: i32, max_section: i32) -> Nbt {
    compound([("min_section", Nbt::Int(min_section)), ("max_section", Nbt::Int(max_section))])
}

/// 4997: the chunk's terrain blending data is dropped, then recreated in the overworld for a
/// chunk past the biome step (blending the full height, sections -4 to 20), or one regenerated
/// under y = 0 towards such a step (the old height, sections 0 to 16).
pub(crate) fn recreate_blending_data(chunk: &mut Nbt, context: &Context<'_>) {
    remove(chunk, "blending_data");
    let Some(status) = chunk.get("Status") else { return };
    if context.dimension != Some("minecraft:overworld") {
        return;
    }
    if !BEFORE_TERRAIN.contains(&status_of(Some(status), "empty").as_str()) {
        set(chunk, "blending_data", blending_data(-4, 20));
    } else if let Some(retrogen) = chunk.get("below_zero_retrogen")
        && !BEFORE_TERRAIN.contains(&status_of(retrogen.get("target_status"), "empty").as_str())
    {
        set(chunk, "blending_data", blending_data(0, 16));
    }
}

/// 5009: the blending heights become floats; a height that is not a number (or the largest
/// double, "no value") becomes the largest float.
pub(crate) fn blending_heights_to_floats(chunk: &mut Nbt, _: &Context<'_>) {
    let Some(heights) = field_mut(chunk, "blending_data").and_then(|b| field_mut(b, "heights")) else { return };
    let values: Vec<Option<f64>> = match heights {
        Nbt::List(entries) => entries.iter().map(number).collect(),
        Nbt::ByteArray(a) => a.iter().map(|&b| Some(f64::from(b as i8))).collect(),
        Nbt::IntArray(a) => a.iter().map(|&v| Some(f64::from(v))).collect(),
        Nbt::LongArray(a) => a.iter().map(|&v| Some(v as f64)).collect(),
        // Not a list: read as an empty one.
        _ => Vec::new(),
    };
    *heights = Nbt::List(
        values
            .into_iter()
            .map(|v| match v {
                Some(v) if v != f64::MAX => Nbt::Float(v as f32),
                _ => Nbt::Float(f32::MAX),
            })
            .collect(),
    );
}

fn number(value: &Nbt) -> Option<f64> {
    match *value {
        Nbt::Float(v) => Some(f64::from(v)),
        Nbt::Double(v) => Some(v),
        ref v => v.as_i64().map(|i| i as f64),
    }
}

/// The steps between noise and carvers were merged into the terrain step.
const OLD_INTERMEDIATE_TERRAIN: &[&str] = &["minecraft:noise", "minecraft:surface"];

fn merged_status(status: &str) -> &str {
    match status {
        "minecraft:carvers" => "minecraft:terrain",
        s if OLD_INTERMEDIATE_TERRAIN.contains(&s) => "minecraft:biomes",
        s => s,
    }
}

/// 5013: the old terrain steps merge: `carvers` becomes `terrain`, `noise` and `surface` go back
/// to `biomes` (statuses and the under-zero regeneration's target). A chunk stopped in between
/// loses its blocks (only under y = 0 when only its regeneration target was past them), its
/// height maps and its blending data, to be made again.
pub(crate) fn merge_terrain_statuses(chunk: &mut Nbt, _: &Context<'_>) {
    let old_status = status_of(chunk.get("Status"), "minecraft:empty");
    set(chunk, "Status", Nbt::String(merged_status(&old_status).to_owned()));
    let mut old_target = None;
    if let Some(retrogen) = field_mut(chunk, "below_zero_retrogen") {
        let target = status_of(retrogen.get("target_status"), "minecraft:empty");
        set(retrogen, "target_status", Nbt::String(merged_status(&target).to_owned()));
        old_target = Some(target);
    }
    let status_between = OLD_INTERMEDIATE_TERRAIN.contains(&old_status.as_str());
    let target_between = old_target.as_deref().is_some_and(|t| OLD_INTERMEDIATE_TERRAIN.contains(&t));
    if status_between || target_between {
        let only_below_zero = old_target.is_some() && !target_between;
        remove_terrain_blocks(chunk, only_below_zero);
    }
}

fn remove_terrain_blocks(chunk: &mut Nbt, only_below_zero: bool) {
    remove(chunk, "Heightmaps");
    remove(chunk, "blending_data");
    let Some(Nbt::List(sections)) = field_mut(chunk, "sections") else { return };
    for section in sections {
        // The game reads the height as a byte, 0 when absent.
        let y = section.get("Y").and_then(Nbt::as_i64).map_or(0, |y| y as i8);
        if !(only_below_zero && y >= 0) {
            remove(section, "block_states");
        }
    }
}
