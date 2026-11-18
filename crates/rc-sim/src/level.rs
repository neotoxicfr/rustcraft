//! `level.dat` (`anvil-world.md` § 2): world settings, compound kept for rewriting.

use std::path::Path;

use rc_proto::nbt::Nbt;

use crate::Error;
use crate::nbtio::{self, int};

#[derive(Debug, Clone)]
pub struct Level {
    /// Root read `{ Data: { … } }`, its `Data` converted to the current data version when it
    /// was older and the conversion covers it (`rc-datafix`).
    pub root: Nbt,
    /// Data version `level.dat` was written with (before any conversion; -1 when absent).
    pub data_version: i32,
    pub name: String,
    pub time: i64,
    pub game_type: i32,
    pub difficulty: String,
    pub hardcore: bool,
    pub difficulty_locked: bool,
    pub allow_commands: bool,
    pub spawn_pos: [i32; 3],
    /// The surface at the border's centre while the border leaves `spawn_pos` outside
    /// (`Server::update_effective_spawn`); `None` while the saved point holds.
    pub spawn_moved_by_border: Option<[i32; 3]>,
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

    pub fn from_root(mut root: Nbt) -> Result<Level, Error> {
        let data = nbtio::get_mut(&mut root, "Data").ok_or_else(|| Error::Format("level.dat without Data".into()))?;
        let data_version = rc_datafix::data_version(data, rc_datafix::FileKind::Level);
        // Read converted, like the game; one too old is kept as it is (the server refuses it).
        rc_datafix::upgrade(rc_datafix::FileKind::Level, data, &rc_datafix::Context::default());
        let d = root.get("Data").ok_or_else(|| Error::Format("level.dat without Data".into()))?;
        let diff = d.get("difficulty_settings");
        let spawn = d.get("spawn");
        let pos = match spawn.and_then(|s| s.get("pos")) {
            Some(Nbt::IntArray(a)) if a.len() == 3 => [a[0], a[1], a[2]],
            _ => [0, 64, 0],
        };
        Ok(Level {
            data_version,
            name: d.get("LevelName").and_then(Nbt::as_str).unwrap_or("").to_owned(),
            time: int(d, "Time").unwrap_or(0),
            game_type: int(d, "GameType").unwrap_or(0) as i32,
            difficulty: diff.and_then(|x| x.get("difficulty")).and_then(Nbt::as_str).unwrap_or("normal").to_owned(),
            hardcore: diff.and_then(|x| int(x, "hardcore")) == Some(1),
            difficulty_locked: diff.and_then(|x| int(x, "locked")) == Some(1),
            allow_commands: int(d, "allowCommands") == Some(1),
            spawn_pos: pos,
            spawn_moved_by_border: None,
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

    /// The spawn the server and the players use: the saved point, unless the border leaves it
    /// outside.
    pub fn effective_spawn_pos(&self) -> [i32; 3] {
        self.spawn_moved_by_border.unwrap_or(self.spawn_pos)
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
