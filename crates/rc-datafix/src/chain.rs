//! The ordered chain of conversion rules, from the oldest version covered to the current one.
//!
//! The order is the game's: by version, then by step, then in the order the game lists rules of
//! the same step. A file is converted by applying, one after the other, every rule newer than the
//! version it was written with; each rule reads the data with the schema in force just before it.

use rc_proto::nbt::Nbt;

use crate::Context;
use crate::fixes::{blocks, chunks, items, profiles};
use crate::kind::Kind;
use crate::map::{self, Schema};
use crate::walk;

/// `RUSTCRAFT_OPT_DATAFIX_SKIP_WALKS_WITHOUT_TARGET` (default on; `0`: every rule walks the
/// whole file): no walk for a rule whose kind the file cannot hold.
fn skip_walks_without_target() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_DATAFIX_SKIP_WALKS_WITHOUT_TARGET")
}

/// Oldest data version every rule up to the current one is written for. Data of an older
/// version is never converted (left as it is, see [`crate::Outcome::TooOld`]).
pub const OLDEST_CONVERTIBLE: i32 = 4899;

/// One conversion rule: on which kind of data, from which version (what it changes: its
/// function's documentation).
pub(crate) struct Rule {
    pub(crate) version: i32,
    /// Step within the version (the game numbers some rules `version.step`).
    pub(crate) step: i32,
    pub(crate) target: Kind,
    pub(crate) fix: fn(&mut Nbt, &Context<'_>),
}

impl Rule {
    /// The rule's version key (`version * 10 + step`), the game's ordering.
    pub(crate) fn key(&self) -> i32 {
        self.version * 10 + self.step
    }
}

const fn rule(version: i32, step: i32, target: Kind, fix: fn(&mut Nbt, &Context<'_>)) -> Rule {
    Rule { version, step, target, fix }
}

/// Every rule above [`OLDEST_CONVERTIBLE`], in the game's order.
pub(crate) const CHAIN: &[Rule] = &[
    rule(4996, 0, Kind::DataComponents, items::pot_decorations_to_sides),
    rule(4996, 1, Kind::BlockEntity, blocks::pot_sherds_to_sides),
    rule(4997, 0, Kind::Chunk, chunks::recreate_blending_data),
    rule(5001, 0, Kind::DebugProfile, profiles::rename_post_effect_entry),
    rule(5002, 0, Kind::BlockEntity, blocks::signs_allow_op_features),
    rule(5006, 0, Kind::BlockState, blocks::block_state_field_names),
    rule(5007, 0, Kind::DataComponents, items::split_swing_animation),
    rule(5008, 0, Kind::ItemStack, items::explorer_map_items),
    rule(5008, 0, Kind::DataComponents, items::remove_map_color),
    rule(5009, 0, Kind::Chunk, chunks::blending_heights_to_floats),
    rule(5012, 0, Kind::ItemName, items::rename_explorer_maps),
    rule(5013, 0, Kind::Chunk, chunks::merge_terrain_statuses),
    rule(5014, 0, Kind::DataComponents, items::remove_block_transformer),
    rule(5016, 0, Kind::ItemStack, items::unfilled_buried_treasure_map),
];

/// Applies to `root` (of `kind`, written at data version `from`) every rule newer than `from`.
pub(crate) fn apply(kind: Kind, root: &mut Nbt, from: i32, context: &Context<'_>) {
    apply_with(kind, root, from, context, skip_walks_without_target());
}

/// [`apply`], with the walks of rules whose kind the file cannot hold skipped or not (`skip`).
pub(crate) fn apply_with(kind: Kind, root: &mut Nbt, from: i32, context: &Context<'_>, skip: bool) {
    let hidden = walk::read_as_schema(kind, root, Schema::of_version(from));
    let from_key = Schema::of_version(from).0;
    for rule in CHAIN.iter().filter(|r| r.key() > from_key) {
        let schema = Schema::before(rule.key());
        // A rule on the file's own root kind, which the file cannot hold again inside, is the
        // root alone; a rule on a kind the file cannot hold has nothing to do: no walk.
        if skip && !map::can_hold(kind, rule.target, schema) {
            if rule.target == kind {
                (rule.fix)(root, context);
            }
            continue;
        }
        walk::visit(kind, root, schema, &mut |k, node| {
            if k == rule.target {
                (rule.fix)(node, context);
            }
        });
    }
    if hidden > 0 {
        walk::restore_hidden(root);
    }
}
