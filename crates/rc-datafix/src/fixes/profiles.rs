//! Rules on the client's debug screen profile.

use rc_proto::nbt::Nbt;

use super::{field_mut, rename};
use crate::Context;

/// 5001: the custom profile's `post_effect` entry is now `post_effects`.
pub(crate) fn rename_post_effect_entry(profile: &mut Nbt, _: &Context<'_>) {
    if let Some(custom) = field_mut(profile, "custom") {
        rename(custom, "minecraft:post_effect", "minecraft:post_effects");
    }
}
