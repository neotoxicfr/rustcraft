//! Correspondence between disk palettes (names and properties as text) and the
//! global state identifiers of the version data, in both directions.

use hashbrown::HashMap;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;

/// Conversion tables built once per world.
pub struct States {
    blocks: HashMap<String, u16>,
    /// Per block: step of each property (in the order of `BlockInfo::properties`).
    strides: Vec<Vec<u32>>,
    /// Per state: true if it is air (empty for the count of non-empty blocks).
    pub air: Vec<bool>,
    pub air_state: u32,
    pub biomes: HashMap<String, u32>,
    pub biome_names: Vec<String>,
    /// Per palette biome: index into `VersionData::biomes` (`u16::MAX`: unknown),
    /// resolved once instead of a name lookup at each block.
    pub biome_data: Vec<u16>,
    /// Default biome of an absent section.
    pub default_biome: u32,
    /// Heightmap rules per state (`chunk::heightmap_rules`); empty: maps not
    /// kept up to date.
    pub heightmap_rules: Vec<u8>,
}

impl States {
    pub fn new(data: &VersionData, biome_names: Vec<String>) -> States {
        let blocks: HashMap<String, u16> =
            data.blocks.iter().enumerate().map(|(i, b)| (b.name.clone(), i as u16)).collect();
        let strides = data
            .blocks
            .iter()
            .map(|b| {
                let first = b.first_state;
                let vals = |s: u32, p: usize| data.state_values[data.states[s as usize].values as usize + p];
                (0..b.properties.len())
                    .map(|p| (1..b.state_count).find(|&o| vals(first + o, p) != vals(first, p)).unwrap_or(1))
                    .collect()
            })
            .collect();
        let air_blocks: Vec<u16> = ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"]
            .iter()
            .filter_map(|n| blocks.get(*n).copied())
            .collect();
        let air = data.states.iter().map(|s| air_blocks.contains(&s.block)).collect();
        let air_state = blocks.get("minecraft:air").map_or(0, |&b| data.blocks[b as usize].default_state);
        let biomes: HashMap<String, u32> = biome_names.iter().enumerate().map(|(i, n)| (n.clone(), i as u32)).collect();
        let default_biome = biomes.get("minecraft:plains").copied().unwrap_or(0);
        let biome_data = biome_names
            .iter()
            .map(|n| data.biomes.iter().position(|b| &b.name == n).map_or(u16::MAX, |i| i as u16))
            .collect();
        States {
            blocks,
            strides,
            air,
            air_state,
            biomes,
            biome_names,
            biome_data,
            default_biome,
            heightmap_rules: Vec::new(),
        }
    }

    /// With the heightmap rules (block tags resolved).
    pub fn with_heightmap_rules(mut self, data: &VersionData, block_tags: &HashMap<String, Vec<u32>>) -> States {
        self.heightmap_rules = crate::chunk::heightmap_rules(data, block_tags);
        self
    }

    /// State of a block palette entry: string (default state) or
    /// `{ Name/id, Properties/properties }`. `None`: unknown block.
    pub fn state_of(&self, data: &VersionData, entry: &Nbt) -> Option<u32> {
        let entry = crate::nbtio::unwrap_hetero(entry);
        let (name, props) = match entry {
            Nbt::String(s) => (s.as_str(), None),
            Nbt::Compound(_) => (
                entry.get("id").or_else(|| entry.get("Name")).and_then(Nbt::as_str)?,
                entry.get("properties").or_else(|| entry.get("Properties")),
            ),
            _ => return None,
        };
        let b = *self.blocks.get(name)?;
        let info = &data.blocks[b as usize];
        let Some(Nbt::Compound(props)) = props else { return Some(info.default_state) };
        let default = &data.states[info.default_state as usize];
        let mut state = info.first_state;
        for (p, &prop) in info.properties.iter().enumerate() {
            let def = data.state_values[default.values as usize + p];
            let property = &data.properties[prop as usize];
            // Property absent (or unknown value): default value.
            let v = props
                .iter()
                .find(|(k, _)| *k == property.name)
                .and_then(|(_, v)| v.as_str())
                .and_then(|v| property.values.iter().position(|x| x == v))
                .map_or(def, |i| i as u8);
            state += u32::from(v) * self.strides[b as usize][p];
        }
        Some(state)
    }

    /// Disk palette entry of a state: string if it is the default state of its block,
    /// otherwise `{ id, properties }` with all properties as text (26.3 keys).
    pub fn palette_entry(&self, data: &VersionData, state: u32) -> Nbt {
        let Some(info) = data.block_of(state) else { return Nbt::String("minecraft:air".into()) };
        if state == info.default_state || info.properties.is_empty() {
            return Nbt::String(info.name.clone());
        }
        let props =
            data.state_properties(state).map(|(k, v)| (k.to_owned(), Nbt::String(v.to_owned()))).collect::<Vec<_>>();
        Nbt::Compound(vec![("id".into(), Nbt::String(info.name.clone())), ("properties".into(), Nbt::Compound(props))])
    }

    pub fn is_air(&self, state: u32) -> bool {
        self.air.get(state as usize).copied().unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state is found again from its properties (the steps are right for all blocks).
    #[test]
    fn every_state_round_trips() {
        let Some(data) = rc_testdata::version_data() else { return };
        let s = States::new(&data, Vec::new());
        for state in 0..data.states.len() as u32 {
            let e = s.palette_entry(&data, state);
            assert_eq!(s.state_of(&data, &e), Some(state), "{e:?}");
        }
        assert!(s.is_air(s.air_state));
    }
}
