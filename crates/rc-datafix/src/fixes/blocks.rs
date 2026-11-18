//! Rules on block entities and block states.

use rc_proto::nbt::Nbt;

use super::{rename, set};
use crate::Context;
use crate::fixes::items::id_list_to_sides;

fn id(block_entity: &Nbt) -> Option<&str> {
    block_entity.get("id").and_then(Nbt::as_str)
}

/// 4996.1: a decorated pot's sherds, a list of ids, become four sides; a pot without sherds gets
/// four bricks.
pub(crate) fn pot_sherds_to_sides(block_entity: &mut Nbt, _: &Context<'_>) {
    if id(block_entity) != Some("minecraft:decorated_pot") {
        return;
    }
    let sherds = block_entity.get("sherds").cloned().unwrap_or(Nbt::List(Vec::new()));
    let sides = id_list_to_sides(&sherds).unwrap_or(sherds);
    set(block_entity, "sherds", sides);
}

/// 5002: signs and hanging signs saved before allow their text's operator-only features.
pub(crate) fn signs_allow_op_features(block_entity: &mut Nbt, _: &Context<'_>) {
    if matches!(id(block_entity), Some("minecraft:sign" | "minecraft:hanging_sign")) {
        set(block_entity, "allow_op_features", Nbt::Byte(1));
    }
}

/// 5006: a block state's `Name` and `Properties` become `id` and `properties`.
pub(crate) fn block_state_field_names(state: &mut Nbt, _: &Context<'_>) {
    rename(state, "Name", "id");
    rename(state, "Properties", "properties");
}
