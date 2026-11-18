//! Boats and rafts of the integrated server, with or without a chest (mission L16-BATEAUX).
//!
//! A boat is a kind of the server's vehicle structure ([`crate::minecart::Cart`], `CKind::Boat`):
//! the game builds minecarts and boats on the same vehicle base (shake, damage, breaking into its
//! named item) and the chest boat on the same container base as the chest minecart (27 slots, loot
//! table, piglins, menu), so the tracking, the save, the commands, the explosions, the projectiles,
//! the portals and the passengers of the minecarts serve the boats too. This module holds what
//! is the boat's own: its tick ([`physics`]), its passengers and its pilot ([`riding`]), its
//! placement by the item and by a dispenser ([`placement`]).

pub(crate) mod physics;
pub(crate) mod placement;
pub(crate) mod riding;

#[cfg(test)]
mod tests;

use rc_physics::mounts::BoatFloat;

/// Box of every boat and raft: width, height (the eyes are at the top).
pub const WIDTH: f64 = 1.375f32 as f64;
pub const HEIGHT: f64 = 0.5625f32 as f64;
/// Tracking range of a boat, in chunks.
pub const TRACKING_RANGE: i32 = 10;
/// Ticks under water before the passengers are thrown out, and the bubble column's delay.
pub const TIME_TO_EJECT: f32 = 60.0;
pub const BUBBLE_TIME: i32 = 60;

/// Wood of a boat; the bamboo one is a raft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoatWood {
    Oak,
    Spruce,
    Birch,
    Jungle,
    Acacia,
    Cherry,
    DarkOak,
    PaleOak,
    Mangrove,
    Bamboo,
}

impl BoatWood {
    const ALL: [BoatWood; 10] = [
        BoatWood::Oak,
        BoatWood::Spruce,
        BoatWood::Birch,
        BoatWood::Jungle,
        BoatWood::Acacia,
        BoatWood::Cherry,
        BoatWood::DarkOak,
        BoatWood::PaleOak,
        BoatWood::Mangrove,
        BoatWood::Bamboo,
    ];
}

/// Entity type names (also the names of their items), two per wood in [`BoatWood`] order: without
/// then with a chest.
const TYPE_NAMES: [&str; 20] = [
    "minecraft:oak_boat",
    "minecraft:oak_chest_boat",
    "minecraft:spruce_boat",
    "minecraft:spruce_chest_boat",
    "minecraft:birch_boat",
    "minecraft:birch_chest_boat",
    "minecraft:jungle_boat",
    "minecraft:jungle_chest_boat",
    "minecraft:acacia_boat",
    "minecraft:acacia_chest_boat",
    "minecraft:cherry_boat",
    "minecraft:cherry_chest_boat",
    "minecraft:dark_oak_boat",
    "minecraft:dark_oak_chest_boat",
    "minecraft:pale_oak_boat",
    "minecraft:pale_oak_chest_boat",
    "minecraft:mangrove_boat",
    "minecraft:mangrove_chest_boat",
    "minecraft:bamboo_raft",
    "minecraft:bamboo_chest_raft",
];

/// One of the twenty boat and raft entity types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoatType {
    pub wood: BoatWood,
    pub chest: bool,
}

impl BoatType {
    /// Entity type name, which is also the name of the item that places it and that it drops.
    pub fn name(self) -> &'static str {
        TYPE_NAMES[self.wood as usize * 2 + usize::from(self.chest)]
    }

    pub fn from_name(name: &str) -> Option<BoatType> {
        let i = TYPE_NAMES.iter().position(|&n| n == name)?;
        Some(BoatType { wood: BoatWood::ALL[i / 2], chest: i % 2 == 1 })
    }

    /// Seats: two, one beside a chest.
    pub fn max_passengers(self) -> usize {
        if self.chest { 1 } else { 2 }
    }

    /// Height of the seats above the boat's feet: a third of its height, or 8/9 of it on a raft.
    pub fn ride_height(self) -> f64 {
        if self.wood == BoatWood::Bamboo {
            f64::from(HEIGHT as f32 * 0.888_888_9f32)
        } else {
            f64::from(HEIGHT as f32 / 3.0)
        }
    }

    /// Forward offset of a passenger riding alone (behind the chest of a chest boat).
    pub fn single_passenger_offset(self) -> f32 {
        if self.chest { 0.15 } else { 0.0 }
    }
}

/// The boat's own state, kept in its vehicle structure.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoatState {
    /// Water status, flotation, rotation per tick and ticks under water (shared with the client).
    pub float: BoatFloat,
    /// Angle of each paddle (left, right): it turns by π/8 a tick while that paddle rows, and a
    /// stroke sound plays when it passes the water.
    pub paddle_angles: [f32; 2],
    /// A bubble column was met under the boat this tick, and whether it pulls down.
    pub above_bubble_column: bool,
    pub bubble_column_pulls_down: bool,
}
