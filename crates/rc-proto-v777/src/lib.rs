//! Protocol 777 (Java Edition 26.3). The numeric packet identifiers are not
//! written here: they are resolved at startup from the official report (`rc-data`), by
//! name. This crate only contains the field layout.

pub mod advancements;
pub mod border;
pub mod chunk;
pub mod commands;
pub mod config;
pub mod container;
pub mod entity;
pub mod interact;
pub mod item;
pub mod map;
pub mod play;
pub mod players;
pub mod scores;
pub mod special;
pub mod waypoint;

pub const PROTOCOL: i32 = 777;
pub const GAME_VERSION: &str = "26.3";
