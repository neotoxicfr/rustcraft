//! Block placement predicted by the client at the click.
//!
//! The game's client places a block item by itself, on its own copy of the world and with the
//! server's placement rules: the block and its sound come at the click, the server's answer then
//! confirms or undoes it. The rules are the server's own (`blocks::rules`), read here through the
//! client's world: one source for both, no copy of the rules.

use std::sync::Arc;

use rc_data::VersionData;

use crate::blocks::Table;
use crate::blocks::placement::{Placer, SecondBlock};
use crate::blocks::rules::{BlockReader, PlacementRules};
use crate::blocks::{Dir, P};
use crate::tag_bits::TagId;

/// The client's world as the placement rules read it.
pub trait ClientWorld {
    /// Block state at `p` in the world's height (air in a chunk the client does not hold).
    fn block_state(&self, p: [i32; 3]) -> u32;
    /// Block (index) in the block tag `name` (`minecraft:` included), as the server sent it.
    fn block_tag(&self, name: &str, block: u16) -> bool;
    /// The stronger of the sky light and block light stored at `p`.
    fn raw_brightness(&self, p: [i32; 3]) -> i32;
    /// Lowest cell of the world.
    fn min_y(&self) -> i32;
    /// Number of cells of the world's height.
    fn height(&self) -> i32;
    /// World border: min x, max x, min z, max z.
    fn border_bounds(&self) -> [f64; 4];
    /// An entity (the player included) stands in the collision shape of `state` placed at `p`.
    fn obstructed(&self, state: u32, p: [i32; 3]) -> bool;
}

/// The player who clicks, as the placement rules read him.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacingPlayer {
    pub yaw: f32,
    pub pitch: f32,
    /// The sneak key is held.
    pub sneaking: bool,
    /// The aiming ray started inside the clicked block.
    pub hit_inside: bool,
    /// Creative operator (permission 2 or more).
    pub game_master: bool,
}

/// A block item used on a block face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockClick<'a> {
    /// Item identifier (`minecraft:oak_stairs`).
    pub item: &'a str,
    pub clicked: [i32; 3],
    /// Clicked face, game order (down, up, north, south, west, east).
    pub face: u8,
    /// Clicked point, absolute.
    pub at: [f64; 3],
}

/// Cell and state of a predicted placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictedPlacement {
    pub pos: [i32; 3],
    pub state: u32,
}

/// The block tables the placement rules read: the integrated server's own when it shares the
/// version data (singleplayer), built once otherwise.
pub struct PlacementPredictor {
    table: Arc<Table>,
}

impl PlacementPredictor {
    pub fn new(data: &Arc<VersionData>) -> Self {
        PlacementPredictor { table: Table::shared(data) }
    }

    #[cfg(test)]
    pub(crate) fn table(&self) -> &Arc<Table> {
        &self.table
    }

    /// What block item `click.item` places when `player` uses it on `world` (`None`: no cell, no
    /// placement state, or the state does not hold there or meets an entity), as the game's
    /// client works it out before sending the use to the server.
    pub fn predict(
        &self,
        data: &VersionData,
        world: &(impl ClientWorld + ?Sized),
        player: &PlacingPlayer,
        click: &BlockClick,
    ) -> Option<PredictedPlacement> {
        let reader = ClientReader { world, table: &self.table, data };
        let block = self.table.block_id(rc_data::rules::placed_block(click.item));
        if block == u16::MAX {
            return None;
        }
        let placer = Placer {
            yaw: player.yaw,
            pitch: player.pitch,
            sneaking: player.sneaking,
            hit_inside: player.hit_inside,
            game_master: player.game_master,
        };
        let face = Dir::from_index(usize::from(click.face));
        let ctx = reader.placement_target(block, click.item, click.clicked, face, click.at, &placer).ok()?;
        let pos = ctx.pos;
        let state = reader.placement_state(block, &ctx)?;
        if !reader.placement_holds(block, state, pos) || world.obstructed(state, pos) {
            return None;
        }
        Some(PredictedPlacement { pos, state })
    }

    /// Block the game's client places right after `state` at `pos` (door's upper half, bed's
    /// head, tall plant's top), from the placed state once its item's `block_state` is applied.
    pub fn second_block(
        &self,
        data: &VersionData,
        world: &(impl ClientWorld + ?Sized),
        pos: [i32; 3],
        state: u32,
    ) -> Option<([i32; 3], u32)> {
        let reader = ClientReader { world, table: &self.table, data };
        reader.placed_second_block(pos, state).filter(|s| s.on_client).map(|SecondBlock { pos, state, .. }| (pos, state))
    }
}

/// Own placement sound of a bucket of a solid block (the powder snow bucket), played instead of
/// the block's; such a bucket leaves an empty bucket in hand outside creative.
pub fn solid_bucket_sound(item: &str) -> Option<&'static str> {
    (item == "minecraft:powder_snow_bucket").then_some(crate::blocks::placement::POWDER_SNOW_BUCKET_EMPTY_SOUND)
}

/// The client world read by the server's rules.
struct ClientReader<'a, W: ?Sized> {
    world: &'a W,
    table: &'a Table,
    data: &'a VersionData,
}

impl<W: ClientWorld + ?Sized> BlockReader for ClientReader<'_, W> {
    /// Void air outside the world's height, like the game's level.
    fn get(&self, p: P) -> u32 {
        if (self.min_y()..self.top_y()).contains(&p[1]) {
            self.world.block_state(p)
        } else {
            self.view().default_state(self.table.id.void_air)
        }
    }

    fn table(&self) -> &Table {
        self.table
    }

    fn data(&self) -> &VersionData {
        self.data
    }

    fn tag(&self, name: &str, block: u16) -> bool {
        self.world.block_tag(name, block)
    }

    fn tag_id(&self, t: TagId, block: u16) -> bool {
        self.world.block_tag(t.name(), block)
    }

    fn raw_brightness(&self, p: P) -> i32 {
        self.world.raw_brightness(p)
    }

    fn top_y(&self) -> i32 {
        self.world.min_y() + self.world.height()
    }

    fn min_y(&self) -> i32 {
        self.world.min_y()
    }

    fn border_bounds(&self) -> [f64; 4] {
        self.world.border_bounds()
    }

    /// The client draws a moving block apart from the world: the cell itself is what supports.
    fn support_state(&self, q: P) -> u32 {
        self.get(q)
    }

    fn wire_signal(&self) -> bool {
        true
    }

    /// The client holds no block entity field the signal reads use (a comparator's output, a
    /// jukebox's playback, a test block's power are the server's): 0, as the game's client sees them.
    fn be_int(&self, _p: P, _key: &str) -> i32 {
        0
    }

    fn jukebox_signal(&self, _p: P) -> i32 {
        0
    }

    fn test_block_signal(&self, _s: u32, _p: P) -> i32 {
        0
    }

    /// The game's client never counts a chest's openers (its lid follows a block event).
    fn chest_openers(&self, _p: P) -> i32 {
        0
    }
}
