//! Blocks the goals compare a cell with: by identifier of the block table
//! (`RUSTCRAFT_OPT_BLOCKIDS`, default) or, old path, by name. A test checks that each identifier
//! is the block of that name.

use super::switches;
use crate::server::Server;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoalBlock {
    GrassBlock,
    Water,
    TurtleEgg,
    PowderSnow,
    Bedrock,
}

impl GoalBlock {
    #[cfg(test)]
    pub(crate) const ALL: [GoalBlock; 5] =
        [GoalBlock::GrassBlock, GoalBlock::Water, GoalBlock::TurtleEgg, GoalBlock::PowderSnow, GoalBlock::Bedrock];

    pub(crate) fn name(self) -> &'static str {
        match self {
            GoalBlock::GrassBlock => "minecraft:grass_block",
            GoalBlock::Water => "minecraft:water",
            GoalBlock::TurtleEgg => "minecraft:turtle_egg",
            GoalBlock::PowderSnow => "minecraft:powder_snow",
            GoalBlock::Bedrock => "minecraft:bedrock",
        }
    }

    /// Identifier in the block table of `s`.
    pub(crate) fn id(self, s: &Server) -> u16 {
        let ids = &s.bs.t.id;
        match self {
            GoalBlock::GrassBlock => ids.grass_block,
            GoalBlock::Water => ids.water,
            GoalBlock::TurtleEgg => ids.turtle_egg,
            GoalBlock::PowderSnow => ids.powder_snow,
            GoalBlock::Bedrock => ids.bedrock,
        }
    }
}

impl Server {
    /// The block at `p` is `block`.
    pub(crate) fn block_is(&self, p: [i32; 3], block: GoalBlock) -> bool {
        self.state_is(self.st(p), block, switches::block_ids())
    }

    /// The state `state` belongs to `block`: identifiers compared (`by_id`) or names. A state
    /// outside the table is no block at all (by name, it reads as air).
    pub(crate) fn state_is(&self, state: u32, block: GoalBlock, by_id: bool) -> bool {
        if by_id {
            self.data.state(state).is_some_and(|i| i.block == block.id(self))
        } else {
            self.state_name(state) == block.name()
        }
    }
}
