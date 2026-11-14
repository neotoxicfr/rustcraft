//! What the clients learn of a block entity when its block changes. Like the game, a change is
//! only marked; the broadcast of the tick then sends each changed block's state and, right
//! after it, the update tag of its block entity for the kinds that have one. A client that has
//! just been given the block holds the block entity the data is loaded into: data sent before
//! the block is dropped (blank sign, white banner, default head until the chunk reloads).

use rc_proto::nbt::Nbt;

use super::P;
use crate::server::Server;

/// The update tag a block entity kind sends with its block; most kinds send none. One byte: the
/// block table holds one per block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateTag {
    /// Nothing is sent (chests, furnaces, hoppers, command blocks…).
    None,
    /// Every saved field, components included (banner).
    FieldsAndComponents,
    /// Every saved field (signs, heads, beacon, End gateway, creaking heart, game master blocks).
    Fields,
    /// The saved fields without the spawn candidates (mob spawner).
    SpawnerFields,
    /// The campfire's items (an empty list without any).
    CampfireItems,
    /// The shelf's items and their alignment (empty and false without them).
    ShelfItems,
    /// The decorated pot's sherds and item.
    PotContents,
    /// The conduit's target.
    Conduit,
    /// The trial spawner's next spawn time (active state) and spawn data.
    TrialSpawner,
    /// The vault's shared data.
    Vault,
    /// The brushed block's face and item.
    Brushable,
    /// An empty compound (copper golem statue).
    Empty,
}

/// Fields of a block entity never sent: its kind and position travel apart.
const PLACE_FIELDS: [&str; 5] = ["id", "x", "y", "z", "keepPacked"];
/// [`PLACE_FIELDS`] and the components: what [`UpdateTag::Fields`] leaves out.
const PLACE_AND_COMPONENTS: [&str; 6] = ["id", "x", "y", "z", "keepPacked", "components"];
/// [`PLACE_AND_COMPONENTS`] and the spawn candidates: what [`UpdateTag::SpawnerFields`] leaves out.
const SPAWNER_LEFT_OUT: [&str; 7] = ["id", "x", "y", "z", "keepPacked", "components", "SpawnPotentials"];

impl UpdateTag {
    /// The update tag of block entity kind `id`.
    pub(crate) fn of_kind(id: &str) -> UpdateTag {
        match id.strip_prefix("minecraft:").unwrap_or(id) {
            "banner" => UpdateTag::FieldsAndComponents,
            "sign" | "hanging_sign" | "skull" | "beacon" | "end_gateway" | "creaking_heart" | "jigsaw"
            | "structure_block" | "test_block" | "test_instance_block" => UpdateTag::Fields,
            "mob_spawner" => UpdateTag::SpawnerFields,
            "campfire" => UpdateTag::CampfireItems,
            "shelf" => UpdateTag::ShelfItems,
            "decorated_pot" => UpdateTag::PotContents,
            "conduit" => UpdateTag::Conduit,
            "trial_spawner" => UpdateTag::TrialSpawner,
            "vault" => UpdateTag::Vault,
            "brushable_block" => UpdateTag::Brushable,
            "copper_golem_statue" => UpdateTag::Empty,
            _ => UpdateTag::None,
        }
    }

    /// The update tag of each block of `blocks` (by block index), from its block entity kind.
    pub(crate) fn per_block<'a>(blocks: impl Iterator<Item = &'a str>) -> Vec<UpdateTag> {
        blocks.map(|name| super::entities::block_entity_id(name).map_or(UpdateTag::None, UpdateTag::of_kind)).collect()
    }
}

/// `be` without the fields in `left_out`.
fn fields_without(be: &Nbt, left_out: &[&str]) -> Nbt {
    let mut out = be.clone();
    for k in left_out {
        crate::nbtio::remove(&mut out, k);
    }
    out
}

/// Fields `keys` of `be`; an absent item list or alignment flag is written empty, like the game.
fn only_fields(be: &Nbt, keys: &[&str]) -> Nbt {
    let mut out = Vec::with_capacity(keys.len());
    for &k in keys {
        match be.get(k) {
            Some(v) => out.push((k.to_owned(), v.clone())),
            None if k == "Items" => out.push((k.to_owned(), Nbt::List(Vec::new()))),
            None if k == "align_items_to_bottom" => out.push((k.to_owned(), Nbt::Byte(0))),
            None => {}
        }
    }
    Nbt::Compound(out)
}

impl Server {
    /// The game's "block changed" mark for a change that only touched the block entity: the next
    /// broadcast sends the block's state, then its block entity's update tag.
    pub(crate) fn mark_block_changed(&mut self, p: P) {
        self.changed.push(p);
    }

    /// Index of the block entity type at `p` (registry `block_entity_type`).
    pub(crate) fn be_type_index(&self, p: P) -> Option<i32> {
        let id = self.be_get(p)?.get("id").and_then(Nbt::as_str)?;
        let r = self.data.registry("minecraft:block_entity_type")?;
        r.entries.iter().position(|e| e == id).map(|i| i as i32)
    }

    /// The update tag of the block entity of state `s` at `p`, sent right after the block's
    /// state; `None` for a kind that sends none. The kind comes from the block table, so a
    /// changed block without one costs no block entity lookup.
    pub(crate) fn be_update_tag(&self, p: P, s: u32) -> Option<Nbt> {
        let v = self.view();
        if !v.has_block_entity(s) {
            return None;
        }
        let tag = self.bs.t.update_tag.get(usize::from(v.block(s))).copied().unwrap_or(UpdateTag::None);
        match tag {
            UpdateTag::None => None,
            UpdateTag::FieldsAndComponents => self.be_get(p).map(|be| fields_without(be, &PLACE_FIELDS)),
            UpdateTag::Fields => self.be_get(p).map(|be| fields_without(be, &PLACE_AND_COMPONENTS)),
            UpdateTag::SpawnerFields => self.be_get(p).map(|be| fields_without(be, &SPAWNER_LEFT_OUT)),
            UpdateTag::CampfireItems => self.be_get(p).map(|be| only_fields(be, &["Items"])),
            UpdateTag::ShelfItems => self.be_get(p).map(|be| only_fields(be, &["Items", "align_items_to_bottom"])),
            UpdateTag::PotContents => self.be_get(p).map(|be| only_fields(be, &["sherds", "item"])),
            UpdateTag::Conduit => self.conduit_update_tag(p),
            UpdateTag::TrialSpawner => self.trial_update_tag(p),
            UpdateTag::Vault => self.vault_update_tag(p),
            UpdateTag::Brushable => self.be_get(p).is_some().then(|| self.brush_update_tag(p)),
            UpdateTag::Empty => self.be_get(p).is_some().then(|| Nbt::Compound(Vec::new())),
        }
    }
}
