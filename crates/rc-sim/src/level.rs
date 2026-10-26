//! `level.dat` (`anvil-world.md` § 2): world settings, compound kept for rewriting.

use std::path::Path;

use rc_proto::nbt::Nbt;

use crate::Error;
use crate::nbtio::{self, int};

#[derive(Debug, Clone)]
pub struct Level {
    /// Root read `{ Data: { … } }`.
    pub root: Nbt,
    pub data_version: i32,
    pub name: String,
    pub time: i64,
    pub game_type: i32,
    pub difficulty: String,
    pub hardcore: bool,
    pub difficulty_locked: bool,
    pub allow_commands: bool,
    pub spawn_pos: [i32; 3],
    pub spawn_yaw: f32,
    pub spawn_pitch: f32,
    pub spawn_dimension: String,
    pub singleplayer_uuid: Option<u128>,
}

impl Level {
    /// `level.dat`, otherwise `level.dat_old` if it is unreadable.
    pub fn read(dir: &Path) -> Result<Level, Error> {
        match nbtio::read_dat(&dir.join("level.dat")) {
            Ok(root) => Level::from_root(root),
            Err(e) => {
                let old = dir.join("level.dat_old");
                if old.is_file() { Level::from_root(nbtio::read_dat(&old)?) } else { Err(e) }
            }
        }
    }

    pub fn from_root(root: Nbt) -> Result<Level, Error> {
        let d = root.get("Data").ok_or_else(|| Error::Format("level.dat without Data".into()))?;
        let diff = d.get("difficulty_settings");
        let spawn = d.get("spawn");
        let pos = match spawn.and_then(|s| s.get("pos")) {
            Some(Nbt::IntArray(a)) if a.len() == 3 => [a[0], a[1], a[2]],
            _ => [0, 64, 0],
        };
        Ok(Level {
            data_version: int(d, "DataVersion").unwrap_or(0) as i32,
            name: d.get("LevelName").and_then(Nbt::as_str).unwrap_or("").to_owned(),
            time: int(d, "Time").unwrap_or(0),
            game_type: int(d, "GameType").unwrap_or(0) as i32,
            difficulty: diff.and_then(|x| x.get("difficulty")).and_then(Nbt::as_str).unwrap_or("normal").to_owned(),
            hardcore: diff.and_then(|x| int(x, "hardcore")) == Some(1),
            difficulty_locked: diff.and_then(|x| int(x, "locked")) == Some(1),
            allow_commands: int(d, "allowCommands") == Some(1),
            spawn_pos: pos,
            spawn_yaw: spawn.and_then(|s| nbtio::float(s, "yaw")).unwrap_or(0.0),
            spawn_pitch: spawn.and_then(|s| nbtio::float(s, "pitch")).unwrap_or(0.0),
            spawn_dimension: spawn
                .and_then(|s| s.get("dimension"))
                .and_then(Nbt::as_str)
                .unwrap_or("minecraft:overworld")
                .to_owned(),
            singleplayer_uuid: nbtio::uuid(d.get("singleplayer_uuid")),
            root,
        })
    }

    /// Network difficulty identifier (0 peaceful … 3 hard).
    pub fn difficulty_id(&self) -> i32 {
        match self.difficulty.as_str() {
            "peaceful" => 0,
            "easy" => 1,
            "hard" => 3,
            _ => 2,
        }
    }
}
