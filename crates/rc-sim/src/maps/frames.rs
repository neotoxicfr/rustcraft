//! Maps held in item frames, like the game: every 10 ticks of a frame's life, the map it holds
//! is carried by each player of the world (its data sent to them), with a green `frame` marker
//! at the frame's block, turned by the frame's direction; the map's file lists those frames
//! (`frames`), the list is kept up to date and read back into markers when the map loads; the
//! marker and the entry go when the map leaves its frame.

use rc_proto::nbt::Nbt;

use super::{MapData, is_map_item};
use crate::items::component;
use crate::player::Stack;
use crate::server::Server;

/// A frame holding a map, as the map's file lists it: the frame's block, its turn in degrees
/// (its 2D direction × 90, −90 on a floor or a ceiling) and the frame entity's number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapFrame {
    pub pos: [i32; 3],
    pub rotation: i32,
    pub entity_id: i32,
}

/// Key of the marker of frame entity `entity_id` among the map's markers.
fn marker_key(entity_id: i32) -> String {
    format!("frame-{entity_id}")
}

impl MapFrame {
    /// The frame list's entry read from the file (all three fields required).
    pub(super) fn from_nbt(n: &Nbt) -> Option<MapFrame> {
        let pos = match n.get("pos") {
            Some(Nbt::IntArray(a)) if a.len() == 3 => [a[0], a[1], a[2]],
            _ => return None,
        };
        let int = |k: &str| crate::nbtio::int(n, k).map(|v| v as i32);
        Some(MapFrame { pos, rotation: int("rotation")?, entity_id: int("entity_id")? })
    }

    pub(super) fn to_nbt(self) -> Nbt {
        Nbt::Compound(vec![
            ("pos".into(), Nbt::IntArray(self.pos.to_vec())),
            ("rotation".into(), Nbt::Int(self.rotation)),
            ("entity_id".into(), Nbt::Int(self.entity_id)),
        ])
    }
}

impl MapData {
    /// The frame list read from the file, each frame's marker placed again (outside the game's
    /// time: no Nether spin). A later entry at the same block replaces an earlier one.
    pub(super) fn read_frames(&mut self, list: &[Nbt]) {
        for frame in list.iter().filter_map(MapFrame::from_nbt) {
            self.put_frame(frame);
            self.add_deco("minecraft:frame", None, &marker_key(frame.entity_id), x_of(frame), z_of(frame), f64::from(frame.rotation), None);
        }
    }

    /// The frame entry for its block, replaced or added; true when the list changed.
    fn put_frame(&mut self, frame: MapFrame) -> bool {
        match self.frames.iter().position(|f| f.pos == frame.pos) {
            Some(i) if self.frames[i] == frame => false,
            Some(i) => {
                self.frames[i] = frame;
                true
            }
            None => {
                self.frames.push(frame);
                true
            }
        }
    }

    /// Frame `frame` seen holding this map while the game runs: the marker of another frame
    /// entity at the same block goes, its own marker is placed, the list kept (the file saved
    /// again when it changes).
    fn frame_seen(&mut self, frame: MapFrame, game_time: i64) {
        if let Some(old) = self.frames.iter().find(|f| f.pos == frame.pos)
            && old.entity_id != frame.entity_id
        {
            let old = marker_key(old.entity_id);
            self.remove_deco(&old);
        }
        let key = marker_key(frame.entity_id);
        self.add_deco("minecraft:frame", Some(game_time), &key, x_of(frame), z_of(frame), f64::from(frame.rotation), None);
        if self.put_frame(frame) {
            self.dirty = true;
        }
    }

    /// The map left the frame `entity_id` at block `pos`: its marker and its entry go.
    fn left_frame(&mut self, pos: [i32; 3], entity_id: i32) {
        self.remove_deco(&marker_key(entity_id));
        self.frames.retain(|f| f.pos != pos);
        self.dirty = true;
    }
}

fn x_of(frame: MapFrame) -> f64 {
    f64::from(frame.pos[0])
}

fn z_of(frame: MapFrame) -> f64 {
    f64::from(frame.pos[2])
}

/// A frame due to show its map this tick: the map's number, the stack it holds, the frame.
#[derive(Debug, Clone)]
pub(crate) struct FramedMap {
    pub map: i32,
    pub stack: Stack,
    pub frame: MapFrame,
}

impl FramedMap {
    /// The map held by a frame whose compound is `e`, at block `pos`, its turn `rotation`.
    pub(crate) fn of_frame(e: &Nbt, entity_id: i32, pos: [i32; 3], rotation: i32) -> Option<FramedMap> {
        let stack = e.get("Item").and_then(Stack::from_nbt).filter(|s| is_map_item(&s.id))?;
        let map = component(&stack, "minecraft:map_id").and_then(Nbt::as_i64)? as i32;
        Some(FramedMap { map, stack, frame: MapFrame { pos, rotation, entity_id } })
    }
}

impl Server {
    /// The host's world tick: its frames' clocks move on, the maps due are shown to the host (the
    /// LAN guests of that world see them in their own tick).
    pub(crate) fn frame_maps_tick(&mut self) {
        self.frames_of_world_tick();
        self.frame_maps_for_player();
    }

    /// Once per server tick of the world the server works in (the host's, or one simulated away
    /// from it): its frames' clocks move on and the maps due are kept with the world, for the
    /// players standing there, like the game where each world shows its frames to its own players.
    pub(crate) fn frames_of_world_tick(&mut self) {
        // A world the host reached during the tick is simulated twice: its frames tick once.
        if self.deco.frame_maps_due_tick == Some(self.tick) {
            return;
        }
        self.deco.frame_maps_due = self.frames_with_maps_due();
        self.deco.frame_maps_due_tick = Some(self.tick);
    }

    /// The maps the frames of the current player's world show this tick: once per world tick for
    /// the host, then for each LAN guest in its own tick, in its own world.
    pub(crate) fn frame_maps_for_player(&mut self) {
        // A world that did not tick its frames this tick shows nothing (its list is an old one).
        if self.deco.frame_maps_due_tick != Some(self.tick) || !self.player_stands_here() {
            return;
        }
        let due = std::mem::take(&mut self.deco.frame_maps_due);
        for framed in &due {
            self.map_in_frame(framed);
        }
        self.deco.frame_maps_due = due;
    }

    /// One frame's map carried by the current player, like the game: the player's own marker
    /// goes unless it holds that map, the frame's marker and entry are kept, the stack's static
    /// markers added, then the map's changes sent.
    fn map_in_frame(&mut self, framed: &FramedMap) {
        let carries = self.slots.iter().flatten().any(|s| {
            is_map_item(&s.id) && component(s, "minecraft:map_id").and_then(Nbt::as_i64) == Some(i64::from(framed.map))
        });
        let (name, uuid, time) = (self.name.clone(), self.uuid, self.time);
        let Some(m) = self.map_get(framed.map) else { return };
        m.holding_of(uuid);
        if !carries {
            m.remove_deco(&name);
        }
        if m.tracking {
            m.frame_seen(framed.frame, time);
        }
        m.add_static_decorations(&framed.stack, Some(time));
        self.map_send(framed.map);
    }

    /// The map `item` left the frame `entity_id` whose compound is `frame`: the map forgets it.
    pub(crate) fn map_left_frame(&mut self, item: &Stack, entity_id: i32, frame_pos: [i32; 3]) {
        if !is_map_item(&item.id) {
            return;
        }
        let Some(id) = component(item, "minecraft:map_id").and_then(Nbt::as_i64) else { return };
        if let Some(m) = self.map_get(id as i32) {
            m.left_frame(frame_pos, entity_id);
        }
    }
}

#[cfg(test)]
#[path = "frames_tests.rs"]
mod tests;
