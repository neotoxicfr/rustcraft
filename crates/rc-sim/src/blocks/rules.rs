//! The world as the block rules read it, and the rule traits built on it.
//!
//! The game's client places a block item by itself, with the same placement rules as the server,
//! on its own copy of the world. The rules here are written once: the integrated server reads its
//! own world through [`BlockReader`], and the client's placement prediction reads the client world
//! through the same trait (`crate::placement_prediction`). Every rule trait (`AttachRules`,
//! `ShapeRules`, `SignalRules`, …) is implemented for every reader, so the server's calls are
//! unchanged and compiled for the server alone (no dynamic call on its path).

use rc_data::VersionData;

use super::level::{Fluid, fluid_of_state};
use super::table::{Table, View};
use super::P;
use crate::server::Server;
use crate::tag_bits::TagId;

pub(crate) use super::attach::AttachRules;
pub(crate) use super::moss_rules::MossRules;
pub(crate) use super::nature::NatureRules;
pub(crate) use super::placement::PlacementRules;
pub(crate) use super::plant_rules::PlantRules;
pub(crate) use super::potent_sulfur::SulfurRules;
pub(crate) use super::redstone::SignalRules;
pub(crate) use super::shape::ShapeRules;
pub(crate) use super::speleothem::SpeleothemRules;

/// What the block rules read of a world: block states, the block table, tags, light, the world's
/// height and border, and the few block entity facts the signal reads need.
pub(crate) trait BlockReader {
    /// State at `p`; void air outside the height or in a chunk not loaded.
    fn get(&self, p: P) -> u32;
    fn table(&self) -> &Table;
    fn data(&self) -> &VersionData;
    /// Block (index) in the block tag named `name`.
    fn tag(&self, name: &str, block: u16) -> bool;
    /// Block (index) in the block tag `t`.
    fn tag_id(&self, t: TagId, block: u16) -> bool;
    /// The stronger of the stored sky light and block light at `p`.
    fn raw_brightness(&self, p: P) -> i32;
    /// First cell above the world's height.
    fn top_y(&self) -> i32;
    /// Lowest cell of the world.
    fn min_y(&self) -> i32;
    /// World border: min x, max x, min z, max z.
    fn border_bounds(&self) -> [f64; 4];
    /// State seen by the support tests (a block moved by a piston that has reached the end of its
    /// stroke shows the carried block).
    fn support_state(&self, q: P) -> u32;
    /// False while the server computes the power of a dust (dust then gives no signal).
    fn wire_signal(&self) -> bool;
    /// Integer field `key` of the block entity at `p`, 0 without one.
    fn be_int(&self, p: P, key: &str) -> i32;
    /// Weak signal of a jukebox at `p`: 15 during playback.
    fn jukebox_signal(&self, p: P) -> i32;
    /// Signal of the test block `s` at `p`: 15 for a powered start block.
    fn test_block_signal(&self, s: u32, p: P) -> i32;
    /// Number of openers of the chest half at `p` (the trapped chest's signal), 0 without one.
    fn chest_openers(&self, p: P) -> i32;

    /// Block view: reading and writing the properties of a state.
    #[inline]
    fn view(&self) -> View<'_> {
        View { table: self.table(), data: self.data() }
    }

    #[inline]
    fn fluid_of(&self, s: u32) -> Fluid {
        fluid_of_state(self.table(), self.data(), s)
    }

    #[inline]
    fn fluid_at(&self, p: P) -> Fluid {
        self.fluid_of(self.get(p))
    }

    fn in_build_height(&self, y: i32) -> bool {
        (self.min_y()..self.top_y()).contains(&y)
    }

    fn state_name(&self, s: u32) -> &str {
        self.data().block_of(s).map_or("minecraft:air", |b| b.name.as_str())
    }
}

/// The server reads its own world: each method is the server's own reading.
impl BlockReader for Server {
    #[inline]
    fn get(&self, p: P) -> u32 {
        Server::get(self, p)
    }

    #[inline]
    fn table(&self) -> &Table {
        Server::table(self)
    }

    #[inline]
    fn data(&self) -> &VersionData {
        &self.data
    }

    #[inline]
    fn tag(&self, name: &str, block: u16) -> bool {
        Server::tag(self, name, block)
    }

    #[inline]
    fn tag_id(&self, t: TagId, block: u16) -> bool {
        Server::tag_id(self, t, block)
    }

    fn raw_brightness(&self, p: P) -> i32 {
        Server::raw_brightness(self, p)
    }

    #[inline]
    fn top_y(&self) -> i32 {
        Server::top_y(self)
    }

    #[inline]
    fn min_y(&self) -> i32 {
        self.dim.height.min_y
    }

    fn border_bounds(&self) -> [f64; 4] {
        Server::border_bounds(self)
    }

    #[inline]
    fn support_state(&self, q: P) -> u32 {
        Server::support_state(self, q)
    }

    #[inline]
    fn wire_signal(&self) -> bool {
        self.bs.wire_signal
    }

    fn be_int(&self, p: P, key: &str) -> i32 {
        Server::be_int(self, p, key)
    }

    fn jukebox_signal(&self, p: P) -> i32 {
        Server::jukebox_signal(self, p)
    }

    fn test_block_signal(&self, s: u32, p: P) -> i32 {
        Server::test_block_signal(self, s, p)
    }

    fn chest_openers(&self, p: P) -> i32 {
        self.life.openers.get(&p).copied().unwrap_or(0)
    }

    /// The server's own view (counted per tick by the tick benches in tests).
    #[inline]
    fn view(&self) -> View<'_> {
        Server::view(self)
    }
}
