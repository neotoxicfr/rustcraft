//! Placement sound of a block this client did not place by itself.
//!
//! The game's client runs the whole placement of a block item and plays its sound at once (the
//! server leaves the placer out of that sound). This client only places full single-state
//! blocks by itself; for any other block (torch, stairs, slab, door…) it lacks the placement
//! rules and waits for the server's block updates, so it plays the sound when those show the
//! block placed: the block and its sound then appear together, one server answer after the
//! click, and a placement the server refuses stays silent like in the game.

/// A placement sent to the server and not made locally, until its sequence is acknowledged.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AwaitedPlacement {
    /// Sequence number of the use that sent it.
    pub seq: i32,
    /// Clicked block and the block against its clicked face: the two cells the server sends
    /// back after a use, and the only ones where it places a block item, scaffolding apart.
    pub clicked: [i32; 3],
    pub beside: [i32; 3],
    /// Block the item places.
    pub block: u16,
    /// Scaffolding: the server may place it at the end of the clicked column, far away.
    pub anywhere: bool,
    /// The placed block already stood there in another state (slab made double, one more
    /// candle…): a placement too, unless the block appears elsewhere before the
    /// acknowledgement (a fence clicked changes shape when one is placed beside it).
    merged: Option<([i32; 3], u32)>,
}

impl AwaitedPlacement {
    pub(crate) fn new(seq: i32, clicked: [i32; 3], beside: [i32; 3], block: u16, anywhere: bool) -> Self {
        AwaitedPlacement { seq, clicked, beside, block, anywhere, merged: None }
    }

    /// A block update from the server at `pos`, from `old` to `new` (`block_of` gives the block
    /// of a state): the cell and state where the awaited block was just placed, if it shows it.
    pub(crate) fn on_update(
        &mut self,
        pos: [i32; 3],
        old: u32,
        new: u32,
        block_of: impl Fn(u32) -> Option<u16>,
    ) -> Option<([i32; 3], u32)> {
        if old == new || block_of(new) != Some(self.block) {
            return None;
        }
        let watched = pos == self.clicked || pos == self.beside;
        if block_of(old) != Some(self.block) {
            // A new block of the item's kind: placed here.
            return (watched || self.anywhere).then_some((pos, new));
        }
        if watched && self.merged.is_none() {
            self.merged = Some((pos, new));
        }
        None
    }

    /// Acknowledgement up to `seq` received: `Some` once this placement is settled, with the
    /// merge it saw if any (the sound to play), `None` while it still waits.
    pub(crate) fn on_ack(&self, seq: i32) -> Option<Option<([i32; 3], u32)>> {
        (seq >= self.seq).then_some(self.merged)
    }
}

#[cfg(test)]
mod tests {
    use super::AwaitedPlacement;

    const AIR: u32 = 0;
    const STONE: u32 = 1;
    /// Two states of the placed block (bottom slab, double slab; or two fence shapes).
    const PLACED_A: u32 = 10;
    const PLACED_B: u32 = 11;
    const PLACED: u16 = 5;

    fn block_of(s: u32) -> Option<u16> {
        Some(match s {
            AIR => 0,
            STONE => 1,
            PLACED_A | PLACED_B => PLACED,
            _ => return None,
        })
    }

    fn awaited(anywhere: bool) -> AwaitedPlacement {
        AwaitedPlacement::new(7, [0, 64, 0], [0, 65, 0], PLACED, anywhere)
    }

    #[test]
    fn a_new_block_beside_or_in_place_of_the_clicked_one_is_placed_at_once() {
        let mut a = awaited(false);
        assert_eq!(a.on_update([0, 65, 0], AIR, PLACED_A, block_of), Some(([0, 65, 0], PLACED_A)));
        let mut a = awaited(false);
        assert_eq!(a.on_update([0, 64, 0], STONE, PLACED_A, block_of), Some(([0, 64, 0], PLACED_A)));
    }

    #[test]
    fn unchanged_cells_other_blocks_and_far_cells_are_not_a_placement() {
        let mut a = awaited(false);
        // The server always sends both cells back: unchanged after a refused placement.
        assert_eq!(a.on_update([0, 65, 0], AIR, AIR, block_of), None);
        assert_eq!(a.on_update([0, 64, 0], STONE, STONE, block_of), None);
        assert_eq!(a.on_update([0, 65, 0], AIR, STONE, block_of), None);
        // Same block appearing elsewhere (another player, the world).
        assert_eq!(a.on_update([5, 65, 0], AIR, PLACED_A, block_of), None);
        assert_eq!(a.on_ack(7), Some(None), "settled without a sound");
    }

    #[test]
    fn scaffolding_is_heard_wherever_the_server_puts_it() {
        let mut a = awaited(true);
        assert_eq!(a.on_update([0, 80, 0], AIR, PLACED_A, block_of), Some(([0, 80, 0], PLACED_A)));
    }

    #[test]
    fn a_merge_sounds_at_the_acknowledgement_unless_the_block_appears_beside() {
        // Slab made double in the clicked cell.
        let mut a = awaited(false);
        assert_eq!(a.on_update([0, 64, 0], PLACED_A, PLACED_B, block_of), None);
        assert_eq!(a.on_ack(6), None, "older acknowledgement: still waiting");
        assert_eq!(a.on_ack(7), Some(Some(([0, 64, 0], PLACED_B))));
        // Fence clicked: it changes shape, then the new fence shows beside it.
        let mut a = awaited(false);
        assert_eq!(a.on_update([0, 64, 0], PLACED_A, PLACED_B, block_of), None);
        assert_eq!(a.on_update([0, 65, 0], AIR, PLACED_A, block_of), Some(([0, 65, 0], PLACED_A)));
    }
}
