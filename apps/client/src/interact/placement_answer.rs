//! Placement sound of a block this client did not place by itself.
//!
//! The game's client runs the whole placement of a block item and plays its sound at once (the
//! server leaves the placer out of that sound). This client only places full single-state
//! blocks by itself; for any other block (torch, stairs, slab, door…) it lacks the placement
//! rules and waits for the server's block updates, so it plays the sound when those show the
//! block placed: the block and its sound then appear together, one server answer after the
//! click, and a placement the server refuses stays silent like in the game.

use std::collections::VecDeque;

use rc_data::VersionData;

/// Cell and state where an awaited block shows placed: where its sound plays.
pub(crate) type Placed = ([i32; 3], u32);

/// Most placements kept waiting at once. Holding the use key repeats a placement every 4 ticks,
/// so even a 1.6 s answer keeps about 8 in flight; the bound only stops a server that never
/// acknowledges from growing the list.
const MAX_AWAITED: usize = 32;

/// Number properties a block item raises when used on a block of its own kind (one more candle,
/// sea pickle, turtle egg, snow layer, petal or leaf segment).
const AMOUNT_PROPERTIES: [&str; 6] = ["candles", "pickles", "eggs", "layers", "flower_amount", "segment_amount"];

/// Blocks an item of their own kind grows by one face.
const FACE_BLOCKS: [&str; 4] =
    ["minecraft:glow_lichen", "minecraft:sculk_vein", "minecraft:resin_clump", "minecraft:vine"];
const FACE_PROPERTIES: [&str; 6] = ["north", "east", "south", "west", "up", "down"];

/// A placement sent to the server and not made locally, until its sequence is acknowledged.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AwaitedPlacement {
    /// Sequence number of the use that sent it.
    seq: i32,
    /// Clicked block and the block against its clicked face: the two cells the server sends
    /// back after a use, and the only ones where it places a block item, scaffolding apart.
    clicked: [i32; 3],
    beside: [i32; 3],
    /// Blocks the item may place: its own and its wall variant (torch, sign, banner, head, coral
    /// fan), or its own twice.
    blocks: [u16; 2],
    /// Scaffolding: the server may place it at the end of the clicked column, far away.
    anywhere: bool,
    /// The placed block already stood there in another state (slab made double, one more
    /// candle…): a placement too, unless the block appears elsewhere before the
    /// acknowledgement (a fence clicked changes shape when one is placed beside it).
    merged: Option<Placed>,
}

impl AwaitedPlacement {
    /// Placement of `block` (default block of the item) sent with sequence `seq`, the cell
    /// `clicked` and the cell `beside` against its clicked face.
    pub(crate) fn new(d: &VersionData, block: &str, seq: i32, clicked: [i32; 3], beside: [i32; 3]) -> Option<Self> {
        let own = block_index(d, block)?;
        let wall = wall_variant(d, block).unwrap_or(own);
        let anywhere = block == "minecraft:scaffolding";
        Some(AwaitedPlacement { seq, clicked, beside, blocks: [own, wall], anywhere, merged: None })
    }

    fn places(&self, block: u16) -> bool {
        self.blocks.contains(&block)
    }

    fn watches(&self, pos: [i32; 3]) -> bool {
        pos == self.clicked || pos == self.beside
    }
}

/// Placements sent and not made locally, oldest first, each until its sequence is acknowledged.
/// Several stay in flight when the server answers slower than the use key repeats.
#[derive(Debug, Default)]
pub(crate) struct AwaitedPlacements {
    queue: VecDeque<AwaitedPlacement>,
}

impl AwaitedPlacements {
    pub(crate) fn push(&mut self, placement: AwaitedPlacement) {
        if self.queue.len() == MAX_AWAITED {
            self.queue.pop_front();
        }
        self.queue.push_back(placement);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// A block update from the server at `pos`, from `old` to `new`: the cell and state where an
    /// awaited block was just placed, if it shows one (that placement is then over). A merge is
    /// kept for its acknowledgement. Each update answers at most one placement, the oldest.
    pub(crate) fn on_update(&mut self, d: &VersionData, pos: [i32; 3], old: u32, new: u32) -> Option<Placed> {
        if old == new {
            return None;
        }
        let new_block = d.state(new)?.block;
        let old_block = d.state(old).map(|s| s.block);
        if old_block != Some(new_block) {
            // A new block of an item's kind: placed here.
            let k = self.queue.iter().position(|a| a.places(new_block) && (a.anywhere || a.watches(pos)))?;
            self.queue.remove(k);
            return Some((pos, new));
        }
        let merging = |a: &AwaitedPlacement| a.merged.is_none() && a.places(new_block) && a.watches(pos);
        if self.queue.iter().any(merging) && shows_merge(d, old, new) {
            let a = self.queue.iter_mut().find(|a| merging(a))?;
            a.merged = Some((pos, new));
        }
        None
    }

    /// Acknowledgement up to `seq` received: takes out the oldest placement it settles, `None`
    /// when none is left. `Some(Some(_))` is a merge seen before it, whose sound plays now.
    pub(crate) fn pop_settled(&mut self, seq: i32) -> Option<Option<Placed>> {
        if self.queue.front()?.seq > seq {
            return None;
        }
        self.queue.pop_front().map(|a| a.merged)
    }
}

/// Index of block `name`, as a state's block.
fn block_index(d: &VersionData, name: &str) -> Option<u16> {
    d.block_id(name).and_then(|i| u16::try_from(i).ok())
}

/// Wall variant of block `name`, named with `wall_` before one of its words (torch →
/// wall_torch, oak_hanging_sign → oak_wall_hanging_sign, skeleton_skull → skeleton_wall_skull,
/// tube_coral_fan → tube_coral_wall_fan): the server places it when the item goes on a side.
fn wall_variant(d: &VersionData, name: &str) -> Option<u16> {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    let mut word_starts = std::iter::once(0).chain(bare.match_indices('_').map(|(i, _)| i + 1));
    word_starts.find_map(|i| block_index(d, &format!("minecraft:{}wall_{}", &bare[..i], &bare[i..])))
}

fn property<'a>(d: &'a VersionData, state: u32, key: &str) -> Option<&'a str> {
    d.state_properties(state).find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// The change from `old` to `new`, two states of one block, shows one more of it placed: a slab
/// made double, one more candle, pickle, egg, snow layer, petal or leaf segment, one more face of
/// a lichen-like block. A change of power or shape only (a wire's power, a fence's sides) is not.
fn shows_merge(d: &VersionData, old: u32, new: u32) -> bool {
    if property(d, new, "type") == Some("double") && property(d, old, "type") != Some("double") {
        return true;
    }
    let amount = |s: u32, key: &str| property(d, s, key).and_then(|v| v.parse::<u8>().ok());
    if AMOUNT_PROPERTIES.iter().any(|&k| amount(new, k) > amount(old, k)) {
        return true;
    }
    let faces = |s: u32| FACE_PROPERTIES.iter().filter(|&&k| property(d, s, k) == Some("true")).count();
    d.block_of(new).is_some_and(|b| FACE_BLOCKS.contains(&b.name.as_str())) && faces(new) > faces(old)
}

#[cfg(test)]
mod tests {
    use super::{AwaitedPlacement, AwaitedPlacements};
    use crate::block_anim::tests::state_of;
    use rc_data::VersionData;

    const CLICKED: [i32; 3] = [0, 64, 0];
    const BESIDE: [i32; 3] = [1, 64, 0];

    fn awaiting(d: &VersionData, block: &str, seqs: &[i32]) -> AwaitedPlacements {
        let mut q = AwaitedPlacements::default();
        for &seq in seqs {
            q.push(AwaitedPlacement::new(d, block, seq, CLICKED, BESIDE).expect(block));
        }
        q
    }

    fn air(d: &VersionData) -> u32 {
        state_of(d, "minecraft:air", &[])
    }

    /// A torch clicked on a wall's side: the server places a wall torch beside, heard at once.
    /// Before, only the item's own block was awaited and the wall torch stayed silent.
    #[test]
    fn a_wall_variant_placed_beside_is_heard() {
        let Some(d) = rc_testdata::version_data() else { return };
        for (item_block, wall_block) in [
            ("minecraft:torch", "minecraft:wall_torch"),
            ("minecraft:oak_sign", "minecraft:oak_wall_sign"),
            ("minecraft:oak_hanging_sign", "minecraft:oak_wall_hanging_sign"),
            ("minecraft:white_banner", "minecraft:white_wall_banner"),
            ("minecraft:skeleton_skull", "minecraft:skeleton_wall_skull"),
            ("minecraft:dead_tube_coral_fan", "minecraft:dead_tube_coral_wall_fan"),
        ] {
            let mut q = awaiting(&d, item_block, &[7]);
            let wall = state_of(&d, wall_block, &[]);
            assert_eq!(q.on_update(&d, BESIDE, air(&d), wall), Some((BESIDE, wall)), "{item_block}");
            assert!(q.is_empty(), "{item_block}: settled");
        }
        // A block with no wall variant waits for itself only.
        let mut q = awaiting(&d, "minecraft:oak_stairs", &[7]);
        let wall_torch = state_of(&d, "minecraft:wall_torch", &[]);
        assert_eq!(q.on_update(&d, BESIDE, air(&d), wall_torch), None);
    }

    #[test]
    fn unchanged_cells_other_blocks_and_far_cells_are_not_a_placement() {
        let Some(d) = rc_testdata::version_data() else { return };
        let (air, stone) = (air(&d), state_of(&d, "minecraft:stone", &[]));
        let torch = state_of(&d, "minecraft:torch", &[]);
        let mut q = awaiting(&d, "minecraft:torch", &[7]);
        // The server always sends both cells back: unchanged after a refused placement.
        assert_eq!(q.on_update(&d, BESIDE, air, air), None);
        assert_eq!(q.on_update(&d, CLICKED, stone, stone), None);
        assert_eq!(q.on_update(&d, BESIDE, air, stone), None);
        // Same block appearing elsewhere (another player, the world).
        assert_eq!(q.on_update(&d, [5, 65, 0], air, torch), None);
        assert_eq!(q.pop_settled(7), Some(None), "settled without a sound");
        assert_eq!(q.pop_settled(7), None);
    }

    #[test]
    fn scaffolding_is_heard_wherever_the_server_puts_it() {
        let Some(d) = rc_testdata::version_data() else { return };
        let scaffolding = state_of(&d, "minecraft:scaffolding", &[]);
        let mut q = awaiting(&d, "minecraft:scaffolding", &[7]);
        assert_eq!(q.on_update(&d, [0, 80, 0], air(&d), scaffolding), Some(([0, 80, 0], scaffolding)));
    }

    #[test]
    fn a_merge_sounds_at_the_acknowledgement_unless_the_block_appears_beside() {
        let Some(d) = rc_testdata::version_data() else { return };
        let slab = |t: &str| state_of(&d, "minecraft:oak_slab", &[("type", t), ("waterlogged", "false")]);
        let mut q = awaiting(&d, "minecraft:oak_slab", &[7]);
        assert_eq!(q.on_update(&d, CLICKED, slab("bottom"), slab("double")), None);
        assert_eq!(q.pop_settled(6), None, "older acknowledgement: still waiting");
        assert_eq!(q.pop_settled(7), Some(Some((CLICKED, slab("double")))));
        // One more candle in the clicked cell.
        let candles = |n: &str| state_of(&d, "minecraft:candle", &[("candles", n)]);
        let mut q = awaiting(&d, "minecraft:candle", &[7]);
        q.on_update(&d, CLICKED, candles("1"), candles("2"));
        assert_eq!(q.pop_settled(7), Some(Some((CLICKED, candles("2")))));
        // Fence clicked: it changes shape (no merge), then the new fence shows beside it.
        let fence = |east: &str| state_of(&d, "minecraft:oak_fence", &[("east", east)]);
        let mut q = awaiting(&d, "minecraft:oak_fence", &[7]);
        assert_eq!(q.on_update(&d, CLICKED, fence("false"), fence("true")), None);
        assert_eq!(q.on_update(&d, BESIDE, air(&d), fence("false")), Some((BESIDE, fence("false"))));
        assert!(q.is_empty());
    }

    /// Redstone dust on a running wire whose power changes before the acknowledgement of a
    /// refused placement: no sound. Before, any change of the clicked block counted as a merge.
    #[test]
    fn a_power_or_shape_change_is_not_a_merge() {
        let Some(d) = rc_testdata::version_data() else { return };
        let wire = |power: &str| {
            let sides = [("north", "side"), ("east", "none"), ("south", "side"), ("west", "none")];
            state_of(&d, "minecraft:redstone_wire", &[sides[0], sides[1], sides[2], sides[3], ("power", power)])
        };
        let mut q = awaiting(&d, "minecraft:redstone_wire", &[7]);
        assert_eq!(q.on_update(&d, CLICKED, wire("3"), wire("4")), None);
        assert_eq!(q.pop_settled(7), Some(None), "refused placement stays silent");
    }

    /// Torches placed every 4 ticks with a slow server: each answer finds its own placement.
    /// Before, each click replaced the one before and only the last placement could be heard.
    #[test]
    fn placements_in_flight_are_each_heard() {
        let Some(d) = rc_testdata::version_data() else { return };
        let torch = state_of(&d, "minecraft:torch", &[]);
        let mut q = AwaitedPlacements::default();
        for (seq, x) in [(7, 0), (8, 1), (9, 2)] {
            q.push(AwaitedPlacement::new(&d, "minecraft:torch", seq, [x, 63, 0], [x, 64, 0]).expect("torch"));
        }
        for x in 0..3 {
            assert_eq!(q.on_update(&d, [x, 64, 0], air(&d), torch), Some(([x, 64, 0], torch)), "torch {x}");
        }
        assert!(q.is_empty());
        // Two candles added to the same cell: two merges, two sounds at the acknowledgement.
        let candles = |n: &str| state_of(&d, "minecraft:candle", &[("candles", n)]);
        let mut q = awaiting(&d, "minecraft:candle", &[7, 8]);
        q.on_update(&d, CLICKED, candles("1"), candles("2"));
        q.on_update(&d, CLICKED, candles("2"), candles("3"));
        assert_eq!(q.pop_settled(8), Some(Some((CLICKED, candles("2")))));
        assert_eq!(q.pop_settled(8), Some(Some((CLICKED, candles("3")))));
        assert_eq!(q.pop_settled(8), None);
    }
}
