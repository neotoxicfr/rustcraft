//! Singleplayer player (`anvil-world.md` § 5, `integrated-server.md` § 2.3): file
//! `players/data/<uuid>.dat` read and kept (unknown fields copied), state useful to the server.

use std::path::{Path, PathBuf};

use rc_proto::nbt::Nbt;

use crate::nbtio::{self, f32s, f64s, int, list};

/// Stack from disk: identifier, quantity, components kept as is.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    pub id: String,
    pub count: i32,
    pub components: Option<Nbt>,
}

impl Stack {
    pub fn from_nbt(v: &Nbt) -> Option<Stack> {
        let id = v.get("id")?.as_str()?.to_owned();
        let count = int(v, "count").unwrap_or(1) as i32;
        // Patch reduced to the differences from the prototype, like the game on each application of a patch.
        (count > 0 && id != "minecraft:air").then(|| {
            let mut s = Stack { id, count, components: v.get("components").cloned() };
            crate::items::normalize_patch(&mut s);
            s
        })
    }

    pub fn to_nbt(&self, slot: Option<i8>) -> Nbt {
        let mut f = Vec::new();
        if let Some(s) = slot {
            f.push(("Slot".to_owned(), Nbt::Byte(s)));
        }
        f.push(("id".into(), Nbt::String(self.id.clone())));
        f.push(("count".into(), Nbt::Int(self.count)));
        if let Some(c) = &self.components {
            f.push(("components".into(), c.clone()));
        }
        Nbt::Compound(f)
    }
}

/// Slots of menu 0 (player inventory): 0 result, 1–4 grid, 5–8 armour (head → feet),
/// 9–35 bag, 36–44 hotbar, 45 off hand.
pub const MENU_SLOTS: usize = 46;

/// Disk equipment → slot of menu 0.
const EQUIPMENT: [(&str, usize); 5] = [("head", 5), ("chest", 6), ("legs", 7), ("feet", 8), ("offhand", 45)];

/// Disk bag slot (0–35) → slot of menu 0.
pub fn menu_slot(disk: i32) -> Option<usize> {
    match disk {
        0..=8 => Some(36 + disk as usize),
        9..=35 => Some(disk as usize),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct PlayerData {
    pub path: PathBuf,
    /// File read (empty for a new player), rewritten with the managed fields replaced.
    pub root: Nbt,
    pub exists: bool,
    pub dimension: Option<String>,
    pub pos: Option<[f64; 3]>,
    pub rotation: Option<[f32; 2]>,
    pub game_mode: Option<i32>,
    pub previous_game_mode: Option<i32>,
    pub selected: i32,
    pub slots: Vec<Option<Stack>>,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub xp_level: i32,
    pub xp_progress: f32,
    pub xp_total: i32,
    pub flying: bool,
    pub fly_speed: f32,
    pub walk_speed: f32,
}

impl PlayerData {
    pub fn read(world: &Path, uuid: u128) -> PlayerData {
        let dir = world.join("players").join("data");
        let name = nbtio::uuid_text(uuid);
        let path = dir.join(format!("{name}.dat"));
        let root = nbtio::read_dat(&path).or_else(|_| nbtio::read_dat(&dir.join(format!("{name}.dat_old")))).ok();
        let exists = root.is_some();
        let root = root.unwrap_or(Nbt::Compound(Vec::new()));
        let mut slots = vec![None; MENU_SLOTS];
        for s in list(root.get("Inventory")) {
            if let (Some(i), Some(st)) = (int(s, "Slot").and_then(|i| menu_slot(i as i32)), Stack::from_nbt(s)) {
                slots[i] = Some(st);
            }
        }
        if let Some(eq) = root.get("equipment") {
            for (k, i) in EQUIPMENT {
                slots[i] = eq.get(k).and_then(Stack::from_nbt);
            }
        }
        let ab = root.get("abilities");
        PlayerData {
            path,
            exists,
            dimension: root.get("Dimension").and_then(Nbt::as_str).map(str::to_owned),
            pos: f64s::<3>(root.get("Pos")),
            rotation: f32s::<2>(root.get("Rotation")),
            game_mode: int(&root, "playerGameType").map(|v| v as i32),
            previous_game_mode: int(&root, "previousPlayerGameType").map(|v| v as i32).filter(|&v| v >= 0),
            selected: int(&root, "SelectedItemSlot").unwrap_or(0).clamp(0, 8) as i32,
            slots,
            health: nbtio::float(&root, "Health").unwrap_or(20.0),
            food: int(&root, "foodLevel").unwrap_or(20) as i32,
            saturation: nbtio::float(&root, "foodSaturationLevel").unwrap_or(5.0),
            xp_level: int(&root, "XpLevel").unwrap_or(0) as i32,
            xp_progress: nbtio::float(&root, "XpP").unwrap_or(0.0),
            xp_total: int(&root, "XpTotal").unwrap_or(0) as i32,
            flying: ab.and_then(|a| int(a, "flying")) == Some(1),
            fly_speed: ab.and_then(|a| nbtio::float(a, "flySpeed")).unwrap_or(0.05),
            walk_speed: ab.and_then(|a| nbtio::float(a, "walkSpeed")).unwrap_or(0.1),
            root,
        }
    }
}
