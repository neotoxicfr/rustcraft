//! Configuration and game-entry packets needed to decode the world.

use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader, Result, nbt, read_str};

const ID_MAX: usize = 32767;

#[derive(Debug, Clone, PartialEq)]
pub struct RegistryData {
    pub registry: String,
    /// Entries in identifier order; NBT absent if the client already knows the pack.
    pub entries: Vec<(String, Option<nbt::Nbt>)>,
}

pub fn read_registry_data(r: &mut Reader<'_>) -> Result<RegistryData> {
    let registry = read_str(r, ID_MAX)?.to_owned();
    // Minimal entry: empty identifier + boolean.
    let n = rc_proto::read_count(r, 2)?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let id = read_str(r, ID_MAX)?.to_owned();
        let data = if bool::decode(r)? { nbt::read(r)? } else { None };
        entries.push((id, data));
    }
    Ok(RegistryData { registry, entries })
}

/// Height of a dimension type read from the registries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DimensionHeight {
    pub min_y: i32,
    pub height: i32,
    /// "Nether" cardinal lighting (bottom and top at 0.9) instead of the default one.
    pub nether_light: bool,
}

impl DimensionHeight {
    pub fn from_nbt(v: &nbt::Nbt) -> Option<Self> {
        let min_y = v.get("min_y")?.as_i64()? as i32;
        let height = v.get("height")?.as_i64()? as i32;
        let nether_light = v.get("cardinal_light").and_then(nbt::Nbt::as_str) == Some("nether");
        (min_y % 16 == 0 && height > 0 && height % 16 == 0).then_some(DimensionHeight { min_y, height, nether_light })
    }

    pub fn sections(&self) -> usize {
        (self.height / 16) as usize
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpawnInfo {
    pub dimension_type: i32,
    pub dimension: String,
    pub hashed_seed: i64,
    pub game_mode: i32,
    /// Previous mode, absent if there is none.
    pub previous_game_mode: Option<i32>,
    pub debug: bool,
    pub flat: bool,
    pub last_death: Option<(String, BlockPos)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

pub fn read_spawn_info(r: &mut Reader<'_>) -> Result<SpawnInfo> {
    Ok(SpawnInfo {
        dimension_type: read_varint(r)?,
        dimension: read_str(r, ID_MAX)?.to_owned(),
        hashed_seed: i64::decode(r)?,
        game_mode: read_varint(r)?,
        // Coded + 1, 0 = absent.
        previous_game_mode: match read_varint(r)? {
            0 => None,
            v => Some(v - 1),
        },
        debug: bool::decode(r)?,
        flat: bool::decode(r)?,
        last_death: if bool::decode(r)? { Some((read_str(r, ID_MAX)?.to_owned(), BlockPos::decode(r)?)) } else { None },
        portal_cooldown: read_varint(r)?,
        sea_level: read_varint(r)?,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub player_id: i32,
    pub hardcore: bool,
    pub dimensions: Vec<String>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub show_death_screen: bool,
    pub limited_crafting: bool,
    pub spawn: SpawnInfo,
    /// The server authenticates players (online mode).
    pub online_mode: bool,
    pub enforces_secure_chat: bool,
}

pub fn read_login(r: &mut Reader<'_>) -> Result<Login> {
    let player_id = i32::decode(r)?;
    let hardcore = bool::decode(r)?;
    let n = rc_proto::read_count(r, 1)?;
    let dimensions = (0..n).map(|_| read_str(r, ID_MAX).map(str::to_owned)).collect::<Result<_>>()?;
    Ok(Login {
        player_id,
        hardcore,
        dimensions,
        max_players: read_varint(r)?,
        view_distance: read_varint(r)?,
        simulation_distance: read_varint(r)?,
        reduced_debug_info: bool::decode(r)?,
        show_death_screen: bool::decode(r)?,
        limited_crafting: bool::decode(r)?,
        spawn: read_spawn_info(r)?,
        online_mode: bool::decode(r)?,
        enforces_secure_chat: bool::decode(r)?,
    })
}
