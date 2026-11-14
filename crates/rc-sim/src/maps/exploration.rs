//! The `exploration_map` function of loot tables and trades. With "skip known structures" (the
//! default) the structure the origin stands in is designated first, so that the map leads to
//! another one; then the nearest structure of the destination not yet designated is searched for,
//! and the stack becomes an enlarged map of it, tracked without limit, its water outlined and a
//! "+" target on it. A trade runs it while its offer is drawn (an offer whose map finds nothing is
//! refused there, `Server::draw_trade_offer`); a loot table marks its stack, the map being made
//! where the loot is placed.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::items::{component, remove_component, set_component};
use crate::player::Stack;
use crate::progress::pred::{box_holds, referenced_start_chunks};
use crate::server::Server;

/// The `custom_data` entry a loot table's function leaves on its stack.
const MARKER: &str = "rustcraft:explore";

/// Settings of an exploration map function, with the game's defaults.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExplorationSettings {
    /// Structures searched for: tags (`#…`) and identifiers.
    destination: Vec<String>,
    /// Type of the "+" target marker.
    decoration: String,
    zoom: u8,
    /// In chunks, for the structures placed at random.
    search_radius: i32,
    skip_known_structures: bool,
}

impl ExplorationSettings {
    /// The settings of function `f` as written in the data.
    pub(crate) fn of_function(f: &Value) -> Self {
        let destination = match &f["destination"] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => vec!["#minecraft:on_treasure_maps".to_owned()],
        };
        ExplorationSettings {
            destination,
            decoration: f["decoration"].as_str().unwrap_or("minecraft:mansion").to_owned(),
            zoom: f["zoom"].as_i64().unwrap_or(2).clamp(0, 4) as u8,
            search_radius: f["search_radius"].as_i64().unwrap_or(50) as i32,
            skip_known_structures: f["skip_existing_chunks"].as_bool().unwrap_or(true),
        }
    }

    /// Leaves these settings on a loot table's stack, read back by `Server::resolve_exploration`.
    pub(crate) fn mark(&self, s: &mut Stack) {
        // One destination (every table of the game) written as its text, several as a list.
        let destination = match self.destination.as_slice() {
            [one] => Nbt::String(one.clone()),
            all => Nbt::List(all.iter().map(|d| Nbt::String(d.clone())).collect()),
        };
        let marker = Nbt::Compound(vec![
            ("destination".to_owned(), destination),
            ("decoration".to_owned(), Nbt::String(self.decoration.clone())),
            ("zoom".to_owned(), Nbt::Int(i32::from(self.zoom))),
            ("search_radius".to_owned(), Nbt::Int(self.search_radius)),
            ("skip_existing_chunks".to_owned(), Nbt::Byte(i8::from(self.skip_known_structures))),
        ]);
        let mut data = match component(s, "minecraft:custom_data") {
            Some(Nbt::Compound(c)) => c.clone(),
            _ => Vec::new(),
        };
        data.retain(|(k, _)| k != MARKER);
        data.push((MARKER.to_owned(), marker));
        set_component(s, "minecraft:custom_data", Nbt::Compound(data));
    }

    /// The settings `mark` left on `s`, taken off it (the stack's other custom data stays).
    fn take_marker(s: &mut Stack) -> Option<Self> {
        let Some(Nbt::Compound(data)) = component(s, "minecraft:custom_data") else { return None };
        let marker = data.iter().find(|(k, _)| k == MARKER).map(|(_, m)| m.clone())?;
        let rest: Vec<(String, Nbt)> = data.iter().filter(|(k, _)| k != MARKER).cloned().collect();
        if rest.is_empty() {
            remove_component(s, "minecraft:custom_data");
        } else {
            set_component(s, "minecraft:custom_data", Nbt::Compound(rest));
        }
        let text = |k: &str| marker.get(k).and_then(Nbt::as_str).map(str::to_owned);
        let destination = match marker.get("destination") {
            Some(Nbt::String(one)) => vec![one.clone()],
            list => crate::nbtio::list(list).iter().filter_map(Nbt::as_str).map(str::to_owned).collect(),
        };
        Some(ExplorationSettings {
            destination,
            decoration: text("decoration").unwrap_or_default(),
            zoom: crate::nbtio::int(&marker, "zoom").unwrap_or(2) as u8,
            search_radius: crate::nbtio::int(&marker, "search_radius").unwrap_or(50) as i32,
            skip_known_structures: crate::nbtio::int(&marker, "skip_existing_chunks").is_none_or(|v| v != 0),
        })
    }
}

impl Server {
    /// The exploration map function run on `s` at `origin`: true when a structure was found and
    /// `s` became its map; otherwise `s` is left as it was (the data's filter then discards it).
    pub(crate) fn make_exploration_map(&mut self, s: &mut Stack, settings: &ExplorationSettings, origin: [i32; 3]) -> bool {
        if s.count <= 0 {
            return false;
        }
        let wanted = self.destination_structures(&settings.destination);
        if settings.skip_known_structures {
            self.claim_structure_at(origin, &wanted);
        }
        let skip = settings.skip_known_structures;
        let Some((p, _)) = self.structure_search_ref(&wanted, origin, settings.search_radius, skip) else { return false };
        let id = self.map_create(f64::from(p[0]), f64::from(p[2]), settings.zoom, true, true);
        set_component(s, "minecraft:map_id", Nbt::Int(id));
        self.map_biome_preview(id);
        add_target_marker(s, p, &settings.decoration);
        true
    }

    /// The exploration map a loot table's function left on `s`, made where the loot is placed;
    /// a stack whose map finds no structure is emptied (the data's filter discards it).
    pub(crate) fn resolve_exploration(&mut self, s: &mut Stack, origin: [i32; 3]) {
        let Some(settings) = ExplorationSettings::take_marker(s) else { return };
        if !self.make_exploration_map(s, &settings, origin) {
            s.count = 0;
        }
    }

    /// Structure identifiers of a destination (tags expanded), each once, in order.
    fn destination_structures(&self, destination: &[String]) -> Vec<String> {
        let mut wanted: Vec<String> = Vec::new();
        for member in destination.iter().flat_map(|entry| self.structure_tag_members(entry)) {
            if !wanted.contains(&member) {
                wanted.push(member);
            }
        }
        wanted
    }

    /// The structure of `wanted` that `p` stands in (a cartographer's own village) takes a
    /// reference first, if it still takes one: the search then passes it over.
    fn claim_structure_at(&mut self, p: [i32; 3], wanted: &[String]) {
        let Some((cx, cz, id)) = self.start_holding(p, wanted) else { return };
        let Some(generator) = self.locate_gen() else { return };
        self.try_add_structure_reference(&generator, cx, cz, &id);
    }

    /// The structure start of `wanted` named by the references of `p`'s chunk whose whole box
    /// (all its pieces) holds `p`: its chunk and identifier.
    fn start_holding(&self, p: [i32; 3], wanted: &[String]) -> Option<(i32, i32, String)> {
        let Some(Some(chunk)) = self.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { return None };
        let Some(Nbt::Compound(references)) = chunk.root.get("structures").and_then(|s| s.get("References")) else {
            return None;
        };
        references
            .iter()
            .filter(|(name, _)| wanted.contains(name))
            .flat_map(|(name, starts)| referenced_start_chunks(starts).map(move |(x, z)| (x, z, name)))
            .find(|&(x, z, name)| self.start_box_holds(x, z, name, p))
            .map(|(x, z, name)| (x, z, name.clone()))
    }

    /// The start `name` of chunk (`cx`, `cz`), read from the loaded chunk or else from disk, has a
    /// box holding `p`.
    fn start_box_holds(&self, cx: i32, cz: i32, name: &str, p: [i32; 3]) -> bool {
        let holds = |root: &Nbt| {
            let start = root.get("structures").and_then(|s| s.get("starts")).and_then(|s| s.get(name));
            start.and_then(start_box).is_some_and(|bb| box_holds(&bb, p))
        };
        match self.chunks.get(&(cx, cz)) {
            Some(Some(chunk)) => holds(&chunk.root),
            _ => self.dim.load_raw("region", &[(cx, cz)]).pop().and_then(|(_, root)| root).is_some_and(|r| holds(&r)),
        }
    }
}

/// The whole box of a structure start: the union of its pieces' boxes (none without a piece).
fn start_box(start: &Nbt) -> Option<[i32; 6]> {
    let mut union: Option<[i32; 6]> = None;
    for piece in crate::nbtio::list(start.get("Children")) {
        let Some(Nbt::IntArray(bb)) = piece.get("BB") else { continue };
        let Ok(bb) = <[i32; 6]>::try_from(bb.as_slice()) else { continue };
        union = Some(match union {
            None => bb,
            Some(u) => std::array::from_fn(|k| if k < 3 { u[k].min(bb[k]) } else { u[k].max(bb[k]) }),
        });
    }
    union
}

#[cfg(test)]
#[path = "exploration_tests.rs"]
mod tests;

/// The "+" target marker of an exploration map, at the structure found.
fn add_target_marker(s: &mut Stack, p: [i32; 3], decoration: &str) {
    let entry = Nbt::Compound(vec![
        ("type".into(), Nbt::String(decoration.to_owned())),
        ("x".into(), Nbt::Double(f64::from(p[0]))),
        ("z".into(), Nbt::Double(f64::from(p[2]))),
        ("rotation".into(), Nbt::Float(180.0)),
    ]);
    let mut markers = match component(s, "minecraft:map_decorations") {
        Some(Nbt::Compound(f)) => f.clone(),
        _ => Vec::new(),
    };
    markers.retain(|(k, _)| k != "+");
    markers.push(("+".into(), entry));
    set_component(s, "minecraft:map_decorations", Nbt::Compound(markers));
}
