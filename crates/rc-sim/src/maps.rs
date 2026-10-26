//! Maps (S8, `docs/specs/maps.md`): map data (`data/minecraft/maps/<n>.dat`,
//! last number `maps/last_id.dat`), blank map used, colours sampled around the
//! player holding it (average of the colours per scale cell, relief,
//! water, Nether ceiling), markers (player, off-map, banners, targets of exploration
//! maps), `map_item_data` packets (modified rectangle, markers every 5
//! updates), enlarging and locking post-processing, exploration maps.

use hashbrown::HashMap;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::map::{MapDecoration, MapItemData, MapPatch};

use crate::items::{component, remove_component, set_component};
use crate::player::Stack;
use crate::server::Server;

/// Useful map colours.
const NONE: u8 = 0;
const WATER: u8 = 12;
const DIRT: u8 = 10;
const STONE: u8 = 11;
const COLOR_ORANGE: u8 = 15;
const COLOR_BROWN: u8 = 26;
/// Brightnesses.
const LOW: u8 = 0;
const NORMAL: u8 = 1;
const HIGH: u8 = 2;
const LOWEST: u8 = 3;

/// Map item: the filled map and the 16 exploration maps (treasure, mansion…) are
/// all the same kind of item in game 26.3; these are exactly the identifiers ending in
/// `_map` (the blank map, `minecraft:map`, is another item).
pub(crate) fn is_map_item(id: &str) -> bool {
    id.ends_with("_map")
}

fn packed(color: u8, b: u8) -> u8 {
    color << 2 | (b & 3)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deco {
    pub kind: String,
    pub x: i8,
    pub y: i8,
    pub rot: u8,
    pub name: Option<Nbt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    pub pos: [i32; 3],
    pub color: String,
    pub name: Option<Nbt>,
}

impl Banner {
    fn id(&self) -> String {
        format!("banner-{},{},{}", self.pos[0], self.pos[1], self.pos[2])
    }
}

/// Tracking of the player holding the map.
#[derive(Debug, Clone)]
pub struct Holding {
    dirty_data: bool,
    min: (i32, i32),
    max: (i32, i32),
    dirty_deco: bool,
    tick: i32,
    pub step: i32,
}

impl Default for Holding {
    fn default() -> Holding {
        Holding { dirty_data: true, min: (0, 0), max: (127, 127), dirty_deco: true, tick: 0, step: 0 }
    }
}

#[derive(Debug, Clone)]
pub struct MapData {
    pub cx: i32,
    pub cz: i32,
    pub dim: String,
    pub scale: u8,
    pub colors: Vec<u8>,
    pub tracking: bool,
    pub unlimited: bool,
    pub locked: bool,
    pub banners: Vec<Banner>,
    /// Frames (read back and rewritten as is: frames are not simulated).
    pub frames: Vec<Nbt>,
    pub decorations: Vec<(String, Deco)>,
    pub tracked: i32,
    /// Players carrying it: UUID, sending state specific to each.
    pub holding: Vec<(u128, Holding)>,
    pub dirty: bool,
}

impl MapData {
    /// Centre aligned to the grid of the map size.
    pub fn fresh(x: f64, z: f64, scale: u8, tracking: bool, unlimited: bool, dim: &str) -> MapData {
        let size = 128 * (1i32 << scale);
        let ax = ((x + 64.0) / f64::from(size)).floor() as i32;
        let az = ((z + 64.0) / f64::from(size)).floor() as i32;
        MapData {
            cx: ax * size + size / 2 - 64,
            cz: az * size + size / 2 - 64,
            dim: dim.to_owned(),
            scale,
            colors: vec![0; 16384],
            tracking,
            unlimited,
            locked: false,
            banners: Vec::new(),
            frames: Vec::new(),
            decorations: Vec::new(),
            tracked: 0,
            holding: Vec::new(),
            dirty: true,
        }
    }

    fn deco_tracks(kind: &str) -> bool {
        !matches!(
            kind,
            "minecraft:target_x"
                | "minecraft:target_point"
                | "minecraft:mansion"
                | "minecraft:monument"
                | "minecraft:red_x"
                | "minecraft:village_desert"
                | "minecraft:village_plains"
                | "minecraft:village_savanna"
                | "minecraft:village_snowy"
                | "minecraft:village_taiga"
                | "minecraft:jungle_temple"
                | "minecraft:swamp_hut"
                | "minecraft:trial_chambers"
        )
    }

    /// Sending state of the carrier `who` (created on first carry).
    fn holding_of(&mut self, who: u128) -> &mut Holding {
        let i = match self.holding.iter().position(|x| x.0 == who) {
            Some(i) => i,
            None => {
                self.holding.push((who, Holding::default()));
                self.holding.len() - 1
            }
        };
        &mut self.holding[i].1
    }

    fn remove_deco(&mut self, key: &str) {
        if let Some(i) = self.decorations.iter().position(|d| d.0 == key) {
            let (_, d) = self.decorations.remove(i);
            if Self::deco_tracks(&d.kind) {
                self.tracked -= 1;
            }
        }
        for (_, h) in &mut self.holding {
            h.dirty_deco = true;
        }
    }

    fn clamp_coord(d: f32) -> i8 {
        if d <= -63.0 {
            -128
        } else if d >= 63.0 {
            127
        } else {
            (f64::from(d * 2.0) + 0.5) as i32 as i8
        }
    }

    fn inside(xd: f32, yd: f32) -> bool {
        (-63.0..=63.0).contains(&xd) && (-63.0..=63.0).contains(&yd)
    }

    fn rotation(&self, nether_time: Option<i64>, yaw: f64) -> u8 {
        if let Some(t) = nether_time {
            let s = (t / 10) as i32;
            return ((s.wrapping_mul(s).wrapping_mul(34_187_121).wrapping_add(s.wrapping_mul(121)) >> 15) & 15) as u8;
        }
        let a = if yaw < 0.0 { yaw - 8.0 } else { yaw + 8.0 };
        (a * 16.0 / 360.0) as i32 as i8 as u8
    }

    /// Adding a map marker.
    #[allow(clippy::too_many_arguments)]
    fn add_deco(
        &mut self,
        kind: &str,
        nether_time: Option<i64>,
        key: &str,
        x: f64,
        z: f64,
        yaw: f64,
        name: Option<Nbt>,
    ) {
        let s = (1i32 << self.scale) as f32;
        let xd = (x - f64::from(self.cx)) as f32 / s;
        let yd = (z - f64::from(self.cz)) as f32 / s;
        let (cxd, cyd) = (Self::clamp_coord(xd), Self::clamp_coord(yd));
        let ty = if kind == "minecraft:player" {
            if Self::inside(xd, yd) {
                Some(kind.to_owned())
            } else if xd.abs() < 320.0 && yd.abs() < 320.0 {
                Some("minecraft:player_off_map".to_owned())
            } else if self.unlimited {
                Some("minecraft:player_off_limits".to_owned())
            } else {
                None
            }
        } else if !Self::inside(xd, yd) && !self.unlimited {
            None
        } else {
            Some(kind.to_owned())
        };
        let Some(ty) = ty else {
            self.remove_deco(key);
            return;
        };
        let rot = self.rotation(nether_time, yaw);
        let d = Deco { kind: ty, x: cxd, y: cyd, rot, name };
        let prev = match self.decorations.iter().position(|e| e.0 == key) {
            Some(i) => Some(std::mem::replace(&mut self.decorations[i].1, d.clone())),
            None => {
                self.decorations.push((key.to_owned(), d.clone()));
                None
            }
        };
        if prev.as_ref() != Some(&d) {
            if prev.as_ref().is_some_and(|p| Self::deco_tracks(&p.kind)) {
                self.tracked -= 1;
            }
            if Self::deco_tracks(&d.kind) {
                self.tracked += 1;
            }
            for (_, h) in &mut self.holding {
                h.dirty_deco = true;
            }
        }
    }

    fn set_color(&mut self, x: i32, y: i32, c: u8) {
        self.colors[(x + y * 128) as usize] = c;
        self.dirty = true;
        for (_, h) in &mut self.holding {
            if h.dirty_data {
                h.min = (h.min.0.min(x), h.min.1.min(y));
                h.max = (h.max.0.max(x), h.max.1.max(y));
            } else {
                h.dirty_data = true;
                h.min = (x, y);
                h.max = (x, y);
            }
        }
    }

    fn update_color(&mut self, x: i32, y: i32, c: u8) -> bool {
        if self.colors[(x + y * 128) as usize] != c {
            self.set_color(x, y, c);
            true
        } else {
            false
        }
    }

    pub fn to_nbt(&self) -> Nbt {
        let banners: Vec<Nbt> = self
            .banners
            .iter()
            .map(|b| {
                let mut f =
                    vec![("pos".into(), Nbt::IntArray(b.pos.to_vec())), ("color".into(), Nbt::String(b.color.clone()))];
                if let Some(n) = &b.name {
                    f.push(("name".into(), n.clone()));
                }
                Nbt::Compound(f)
            })
            .collect();
        let data = Nbt::Compound(vec![
            ("dimension".into(), Nbt::String(self.dim.clone())),
            ("xCenter".into(), Nbt::Int(self.cx)),
            ("zCenter".into(), Nbt::Int(self.cz)),
            ("scale".into(), Nbt::Byte(self.scale as i8)),
            ("colors".into(), Nbt::ByteArray(self.colors.clone())),
            ("trackingPosition".into(), Nbt::Byte(i8::from(self.tracking))),
            ("unlimitedTracking".into(), Nbt::Byte(i8::from(self.unlimited))),
            ("locked".into(), Nbt::Byte(i8::from(self.locked))),
            ("banners".into(), Nbt::List(banners)),
            ("frames".into(), Nbt::List(self.frames.clone())),
        ]);
        Nbt::Compound(vec![("data".into(), data), ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION))])
    }

    pub fn from_nbt(root: &Nbt) -> Option<MapData> {
        let d = root.get("data")?;
        let i = |k: &str| crate::nbtio::int(d, k);
        let flag = |k: &str, def: bool| i(k).map_or(def, |v| v != 0);
        let mut m = MapData {
            cx: i("xCenter")? as i32,
            cz: i("zCenter")? as i32,
            dim: d.get("dimension").and_then(Nbt::as_str).unwrap_or("minecraft:overworld").to_owned(),
            scale: i("scale").unwrap_or(0).clamp(0, 4) as u8,
            colors: vec![0; 16384],
            tracking: flag("trackingPosition", true),
            unlimited: flag("unlimitedTracking", false),
            locked: flag("locked", false),
            banners: Vec::new(),
            frames: crate::nbtio::list(d.get("frames")).to_vec(),
            decorations: Vec::new(),
            tracked: 0,
            holding: Vec::new(),
            dirty: false,
        };
        if let Some(Nbt::ByteArray(c)) = d.get("colors")
            && c.len() == 16384
        {
            m.colors = c.clone();
        }
        for b in crate::nbtio::list(d.get("banners")) {
            let pos = match b.get("pos") {
                Some(Nbt::IntArray(a)) if a.len() == 3 => [a[0], a[1], a[2]],
                _ => continue,
            };
            let banner = Banner {
                pos,
                color: b.get("color").and_then(Nbt::as_str).unwrap_or("white").to_owned(),
                name: b.get("name").cloned(),
            };
            let kind = format!("minecraft:banner_{}", banner.color);
            let (x, z) = (f64::from(pos[0]), f64::from(pos[2]));
            m.add_deco(&kind, None, &banner.id(), x, z, 180.0, banner.name.clone());
            m.banners.push(banner);
        }
        Some(m)
    }
}

/// All maps of the world (independent of dimensions).
#[derive(Debug, Default)]
pub struct Maps {
    pub maps: HashMap<i32, MapData>,
    pub absent: hashbrown::HashSet<i32>,
    pub last_id: Option<i32>,
    pub last_id_dirty: bool,
}

impl Maps {
    /// Scale of a loaded map (`None`: unknown).
    pub fn scale_of(&self, id: i32) -> Option<u8> {
        self.maps.get(&id).map(|m| m.scale)
    }
}

impl Server {
    fn maps_dir(&self) -> std::path::PathBuf {
        self.world.join("data").join("minecraft").join("maps")
    }

    /// Loaded map (read on demand).
    pub(crate) fn map_get(&mut self, id: i32) -> Option<&mut MapData> {
        if !self.maps.maps.contains_key(&id) && !self.maps.absent.contains(&id) {
            let path = self.maps_dir().join(format!("{id}.dat"));
            match crate::nbtio::read_dat(&path).ok().as_ref().and_then(MapData::from_nbt) {
                Some(m) => {
                    self.maps.maps.insert(id, m);
                }
                None => {
                    self.maps.absent.insert(id);
                }
            }
        }
        self.maps.maps.get_mut(&id)
    }

    /// Next number.
    fn map_next_id(&mut self) -> i32 {
        if self.maps.last_id.is_none() {
            let path = self.maps_dir().join("last_id.dat");
            let v = crate::nbtio::read_dat(&path)
                .ok()
                .and_then(|r| r.get("data").and_then(|d| crate::nbtio::int(d, "map")))
                .unwrap_or(-1) as i32;
            self.maps.last_id = Some(v);
        }
        let id = self.maps.last_id.unwrap_or(-1) + 1;
        self.maps.last_id = Some(id);
        self.maps.last_id_dirty = true;
        self.maps.absent.remove(&id);
        id
    }

    /// New map; returns its number.
    pub(crate) fn map_create(&mut self, x: f64, z: f64, scale: u8, tracking: bool, unlimited: bool) -> i32 {
        let m = MapData::fresh(x, z, scale, tracking, unlimited, &self.dim.name.clone());
        let id = self.map_next_id();
        self.maps.maps.insert(id, m);
        id
    }

    /// Maps to write (modified) and last number.
    pub(crate) fn maps_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        let dir = self.maps_dir();
        let dirty: Vec<i32> = self.maps.maps.iter().filter(|(_, m)| m.dirty).map(|(k, _)| *k).collect();
        if (!dirty.is_empty() || self.maps.last_id_dirty) && std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        for id in dirty {
            if let Some(m) = self.maps.maps.get_mut(&id) {
                m.dirty = false;
                let nbt = crate::nbtio::encode_root(&m.to_nbt());
                jobs.push(crate::save::Job::Dat { path: dir.join(format!("{id}.dat")), nbt, backup: false });
            }
        }
        if self.maps.last_id_dirty {
            self.maps.last_id_dirty = false;
            let root = Nbt::Compound(vec![
                ("data".into(), Nbt::Compound(vec![("map".into(), Nbt::Int(self.maps.last_id.unwrap_or(-1)))])),
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ]);
            jobs.push(crate::save::Job::Dat {
                path: dir.join("last_id.dat"),
                nbt: crate::nbtio::encode_root(&root),
                backup: false,
            });
        }
    }

    // ---------- Use ----------

    /// Filled map centred on the player (scale 0, tracking).
    pub(crate) fn empty_map_use(&mut self, slot: usize) -> bool {
        let Some(s) = self.slots[slot].clone() else { return false };
        if s.id != "minecraft:map" {
            return false;
        }
        let creative = self.game_mode == 1;
        if !creative && let Some(st) = &mut self.slots[slot] {
            st.count -= 1;
            if st.count <= 0 {
                self.slots[slot] = None;
            }
        }
        // Sound attached to the player (it follows them).
        self.entity_sound(self.player_id, "minecraft:ui.cartography_table.take_result", 7, 1.0, 1.0);
        let (bx, bz) = (self.pos[0].floor(), self.pos[2].floor());
        let id = self.map_create(bx, bz, 0, true, false);
        let mut map = Stack { id: "minecraft:filled_map".into(), count: 1, components: None };
        set_component(&mut map, "minecraft:map_id", Nbt::Int(id));
        if self.slots[slot].is_none() {
            self.slots[slot] = Some(map);
        } else if !self.proj_give(map.clone()) {
            self.drop_from_player_with(map, false);
        }
        self.broadcast_menus();
        true
    }

    /// Map clicked on a banner; true if handled.
    pub(crate) fn map_use_on_banner(&mut self, slot: usize, p: [i32; 3]) -> bool {
        let Some(s) = self.slots[slot].clone() else { return false };
        let Some(id) = component(&s, "minecraft:map_id").and_then(Nbt::as_i64) else { return false };
        let name = self.state_name(self.st(p)).to_owned();
        if !name.ends_with("_banner") {
            return false;
        }
        let be = self.block_entity(p).cloned();
        let color = name
            .trim_start_matches("minecraft:")
            .trim_end_matches("_wall_banner")
            .trim_end_matches("_banner")
            .to_owned();
        let cname = be.as_ref().and_then(|b| b.get("CustomName").cloned());
        let Some(m) = self.map_get(id as i32) else { return true };
        let (x, z) = (f64::from(p[0]) + 0.5, f64::from(p[2]) + 0.5);
        let s = f64::from(1i32 << m.scale);
        let (xd, yd) = ((x - f64::from(m.cx)) / s, (z - f64::from(m.cz)) / s);
        if (-63.0..=63.0).contains(&xd) && (-63.0..=63.0).contains(&yd) {
            let b = Banner { pos: p, color: color.clone(), name: cname };
            if let Some(i) = m.banners.iter().position(|o| *o == b) {
                m.banners.remove(i);
                m.remove_deco(&b.id());
                m.dirty = true;
            } else if m.tracked <= 256 {
                m.add_deco(&format!("minecraft:banner_{color}"), None, &b.id(), x, z, 180.0, b.name.clone());
                m.banners.push(b);
                m.dirty = true;
            }
        }
        true
    }

    // ---------- Tick ----------

    /// Maps of the inventory then packets.
    pub(crate) fn maps_tick(&mut self) {
        if self.life.vit.dead || !self.player_ready() {
            return;
        }
        self.map_post_process();
        // Game inventory: hotbar (0-8), main storage (9-35), armour, off hand.
        let order: Vec<usize> = (36..45).chain(9..36).chain([8, 7, 6, 5, 45]).collect();
        let main = 36 + self.held as usize;
        let mut seen = Vec::new();
        for &slot in &order {
            let Some(id) = self.slots[slot]
                .as_ref()
                .filter(|s| is_map_item(&s.id))
                .and_then(|s| component(s, "minecraft:map_id").and_then(Nbt::as_i64))
            else {
                continue;
            };
            let id = id as i32;
            let stack =
                self.slots[slot].clone().unwrap_or_else(|| Stack { id: String::new(), count: 0, components: None });
            self.map_carried(id, &stack);
            let hand = slot == main || slot == 45;
            if hand && self.map_get(id).is_some_and(|m| !m.locked) {
                self.map_update(id);
            }
            if !seen.contains(&id) {
                seen.push(id);
            }
        }
        for &slot in &order {
            let Some(id) =
                self.slots[slot].as_ref().and_then(|s| component(s, "minecraft:map_id").and_then(Nbt::as_i64))
            else {
                continue;
            };
            self.map_send(id as i32);
        }
    }

    /// Tick of the held item (player only, no frame).
    fn map_carried(&mut self, id: i32, stack: &Stack) {
        let nether = (self.dim.name == "minecraft:the_nether").then_some(self.time);
        let (x, z, yaw, dim) = (self.pos[0], self.pos[2], f64::from(self.yaw), self.dim.name.clone());
        let (name, uuid) = (self.name.clone(), self.uuid);
        let statics: Vec<(String, Nbt)> = match component(stack, "minecraft:map_decorations") {
            Some(Nbt::Compound(f)) => f.clone(),
            _ => Vec::new(),
        };
        let Some(m) = self.map_get(id) else { return };
        m.holding_of(uuid);
        if m.dim == dim && m.tracking {
            let nt = if m.dim == "minecraft:the_nether" { nether } else { None };
            m.add_deco("minecraft:player", nt, &name, x, z, yaw, None);
        }
        for (key, e) in statics {
            if m.decorations.iter().any(|d| d.0 == key) {
                continue;
            }
            let ty = e.get("type").and_then(Nbt::as_str).unwrap_or("minecraft:target_x").to_owned();
            let ex = match e.get("x") {
                Some(Nbt::Double(v)) => *v,
                Some(v) => v.as_i64().unwrap_or(0) as f64,
                None => 0.0,
            };
            let ez = match e.get("z") {
                Some(Nbt::Double(v)) => *v,
                Some(v) => v.as_i64().unwrap_or(0) as f64,
                None => 0.0,
            };
            let rot = match e.get("rotation") {
                Some(Nbt::Float(v)) => f64::from(*v),
                _ => 0.0,
            };
            m.add_deco(&ty, None, &key, ex, ez, rot, None);
        }
    }

    /// Update packet for the player.
    fn map_send(&mut self, id: i32) {
        let kinds = self.data.registry("minecraft:map_decoration_type").map(|r| r.entries.clone()).unwrap_or_default();
        let uuid = self.uuid;
        let Some(m) = self.maps.maps.get_mut(&id) else { return };
        let Some(h) = m.holding.iter_mut().find(|x| x.0 == uuid).map(|x| &mut x.1) else { return };
        let patch = if h.dirty_data {
            h.dirty_data = false;
            let (x0, y0) = h.min;
            let (w, hh) = (h.max.0 + 1 - x0, h.max.1 + 1 - y0);
            let mut colors = vec![0u8; (w * hh) as usize];
            for x in 0..w {
                for y in 0..hh {
                    colors[(x + y * w) as usize] = m.colors[(x0 + x + (y0 + y) * 128) as usize];
                }
            }
            Some(MapPatch { x0: x0 as u8, y0: y0 as u8, width: w as u8, height: hh as u8, colors })
        } else {
            None
        };
        let decorations = if h.dirty_deco && {
            let t = h.tick;
            h.tick += 1;
            t % 5 == 0
        } {
            h.dirty_deco = false;
            Some(
                m.decorations
                    .iter()
                    .map(|(_, d)| MapDecoration {
                        kind: kinds.iter().position(|k| *k == d.kind).unwrap_or(0) as i32,
                        x: d.x,
                        y: d.y,
                        rot: d.rot & 15,
                        name: d.name.clone(),
                    })
                    .collect(),
            )
        } else {
            None
        };
        if patch.is_none() && decorations.is_none() {
            return;
        }
        let p = MapItemData { id, scale: m.scale as i8, locked: m.locked, decorations, patch };
        let _ = self.send(Event::MapData(p));
    }

    /// `WORLD_SURFACE` height (first free block above the highest non-empty block),
    /// recomputed: the heightmaps of modified chunks are not kept up to date.
    fn map_surface(&self, x: i32, z: i32) -> i32 {
        let min_y = self.dim.height.min_y;
        let Some(Some(c)) = self.chunks.get(&(x >> 4, z >> 4)) else { return min_y };
        for (i, sec) in c.sections.iter().enumerate().rev() {
            if sec.non_empty == 0 {
                continue;
            }
            for ly in (0..16).rev() {
                let y = min_y + i as i32 * 16 + ly;
                if !self.states.is_air(self.st([x, y, z])) {
                    return y + 1;
                }
            }
        }
        min_y
    }

    /// Map colour of a state (index).
    fn map_color_of(&self, s: u32) -> u8 {
        self.data.states.get(s as usize).map_or(NONE, |i| i.map_color)
    }

    /// Sampling around the player (one column in 16 per tick, plus the
    /// columns neighbouring a change).
    fn map_update(&mut self, id: i32) {
        let min_y = self.dim.height.min_y;
        let ceiling =
            self.dim_type_field(&self.dim.name.clone(), "has_ceiling").and_then(|v| v.as_bool()).unwrap_or(false);
        let (px, pz) = (self.pos[0], self.pos[2]);
        let Some(m) = self.maps.maps.get(&id) else { return };
        if m.dim != self.dim.name {
            return;
        }
        let scale = 1i32 << m.scale;
        let (cx, cz) = (m.cx, m.cz);
        let player_x = ((px - f64::from(cx)).floor() as i32) / scale + 64;
        let player_y = ((pz - f64::from(cz)).floor() as i32) / scale + 64;
        let mut radius = 128 / scale;
        if ceiling {
            radius /= 2;
        }
        let step = {
            let uuid = self.uuid;
            let m = self.maps.maps.get_mut(&id).expect("map");
            let h = m.holding_of(uuid);
            h.step += 1;
            h.step
        };
        let water_state = self.default_state_of("minecraft:water");
        let lava_state = self.default_state_of("minecraft:lava");
        let mut consecutive = false;
        for img_x in (player_x - radius + 1)..(player_x + radius) {
            if (img_x & 15) != (step & 15) && !consecutive {
                continue;
            }
            consecutive = false;
            let mut prev_height = 0.0f64;
            for img_y in (player_y - radius - 1)..(player_y + radius) {
                if !(img_x >= 0 && img_y >= -1 && img_x < 128 && img_y < 128) {
                    continue;
                }
                let dist2 = (img_x - player_x).pow(2) + (img_y - player_y).pow(2);
                let dither_black = dist2 > (radius - 2) * (radius - 2);
                let ax = (cx / scale + img_x - 64) * scale;
                let az = (cz / scale + img_y - 64) * scale;
                if !matches!(self.chunks.get(&(ax >> 4, az >> 4)), Some(Some(_))) {
                    continue;
                }
                let mut counts: Vec<(u8, i32)> = Vec::new();
                let mut add = |c: u8, n: i32| match counts.iter_mut().find(|e| e.0 == c) {
                    Some(e) => e.1 += n,
                    None => counts.push((c, n)),
                };
                let mut water_depth = 0;
                let mut avg_height = 0.0f64;
                if ceiling {
                    let mut noise = ax.wrapping_add(az.wrapping_mul(231_871));
                    noise = noise.wrapping_mul(noise).wrapping_mul(31_287_121).wrapping_add(noise.wrapping_mul(11));
                    if (noise >> 20) & 1 == 0 {
                        add(DIRT, 10);
                    } else {
                        add(STONE, 100);
                    }
                    avg_height = 100.0;
                } else {
                    for dx in 0..scale {
                        for dz in 0..scale {
                            let (bx, bz) = (ax + dx, az + dz);
                            // Height of the world surface + 1: first free block.
                            let mut col_y = self.map_surface(bx, bz);
                            let mut state;
                            if col_y <= min_y {
                                state = self.default_state_of("minecraft:bedrock").unwrap_or(0);
                            } else {
                                loop {
                                    col_y -= 1;
                                    state = self.st([bx, col_y, bz]);
                                    if !(self.map_color_of(state) == NONE && col_y > min_y) {
                                        break;
                                    }
                                }
                                let info = self.data.state(state);
                                let fluid = info.map_or(0, |i| i.fluid);
                                if col_y > min_y && fluid != 0 {
                                    let mut solid = col_y - 1;
                                    loop {
                                        let below = self.st([bx, solid, bz]);
                                        solid -= 1;
                                        water_depth += 1;
                                        let bf = self.data.state(below).map_or(0, |i| i.fluid);
                                        if !(solid > min_y && bf != 0) {
                                            break;
                                        }
                                    }
                                    // Fluid without a sturdy top face: fluid colour.
                                    let sturdy_up = info.is_some_and(|i| i.sturdy & 2 != 0);
                                    if !sturdy_up {
                                        let fname =
                                            self.data.fluids.get(fluid as usize).map(String::as_str).unwrap_or("");
                                        let legacy = if fname.contains("lava") { lava_state } else { water_state };
                                        if let Some(l) = legacy {
                                            state = l;
                                        }
                                    }
                                }
                            }
                            self.map_check_banners(id, bx, bz);
                            avg_height += f64::from(col_y) / f64::from(scale * scale);
                            add(self.map_color_of(state), 1);
                        }
                    }
                }
                water_depth /= scale * scale;
                // Most frequent colour (first encountered on a tie).
                let mut color = NONE;
                let mut best = 0;
                for (c, n) in &counts {
                    if *n > best {
                        best = *n;
                        color = *c;
                    }
                }
                let parity = f64::from((img_x + img_y) & 1);
                let bright = if color == WATER {
                    let diff = f64::from(water_depth) * 0.1 + parity * 0.2;
                    if diff < 0.5 {
                        HIGH
                    } else if diff > 0.9 {
                        LOW
                    } else {
                        NORMAL
                    }
                } else {
                    let diff = (avg_height - prev_height) * 4.0 / f64::from(scale + 4) + (parity - 0.5) * 0.4;
                    if diff > 0.6 {
                        HIGH
                    } else if diff < -0.6 {
                        LOW
                    } else {
                        NORMAL
                    }
                };
                prev_height = avg_height;
                if img_y >= 0 && dist2 < radius * radius && (!dither_black || (img_x + img_y) & 1 != 0) {
                    let m = self.maps.maps.get_mut(&id).expect("map");
                    consecutive |= m.update_color(img_x, img_y, packed(color, bright));
                }
            }
        }
    }

    /// Banner gone or changed → marker removed.
    fn map_check_banners(&mut self, id: i32, x: i32, z: i32) {
        let Some(m) = self.maps.maps.get(&id) else { return };
        let here: Vec<Banner> = m.banners.iter().filter(|b| b.pos[0] == x && b.pos[2] == z).cloned().collect();
        for b in here {
            let name = self.state_name(self.st(b.pos)).to_owned();
            let color = name
                .trim_start_matches("minecraft:")
                .trim_end_matches("_wall_banner")
                .trim_end_matches("_banner")
                .to_owned();
            let cname = self.block_entity(b.pos).and_then(|e| e.get("CustomName").cloned());
            let same = name.ends_with("_banner") && color == b.color && cname == b.name;
            if !same {
                let m = self.maps.maps.get_mut(&id).expect("map");
                m.banners.retain(|o| *o != b);
                m.remove_deco(&b.id());
                m.dirty = true;
            }
        }
    }

    /// Post-processing pending: enlarging, locking.
    fn map_post_process(&mut self) {
        for slot in 0..self.slots.len() {
            let Some(s) = self.slots[slot].clone() else { continue };
            let Some(kind) = component(&s, "minecraft:map_post_processing").and_then(Nbt::as_i64) else { continue };
            let mut s = s;
            remove_component(&mut s, "minecraft:map_post_processing");
            if let Some(id) = component(&s, "minecraft:map_id").and_then(Nbt::as_i64)
                && let Some(new) = self.map_post(id as i32, kind == 1)
            {
                set_component(&mut s, "minecraft:map_id", Nbt::Int(new));
            }
            self.slots[slot] = Some(s);
            self.broadcast_menus();
        }
    }

    /// Map enlarging / map locking: new map, number returned.
    pub(crate) fn map_post(&mut self, id: i32, scale: bool) -> Option<i32> {
        let m = self.map_get(id)?.clone();
        let new = if scale {
            MapData::fresh(f64::from(m.cx), f64::from(m.cz), (m.scale + 1).min(4), m.tracking, m.unlimited, &m.dim)
        } else {
            let mut n = MapData::fresh(0.0, 0.0, m.scale, m.tracking, m.unlimited, &m.dim);
            n.cx = m.cx;
            n.cz = m.cz;
            n.locked = true;
            n.banners = m.banners.clone();
            n.decorations = m.decorations.clone();
            n.tracked = m.tracked;
            n.colors = m.colors.clone();
            n
        };
        let nid = self.map_next_id();
        self.maps.maps.insert(nid, new);
        Some(nid)
    }

    // ---------- Exploration maps ----------

    /// The exploration map function (marker placed by loot or trading): nearest
    /// structure, enlarged map tracked without limit, biome preview, "+" target. Without a
    /// structure: the stack is emptied (the game's filter discards it).
    pub(crate) fn resolve_exploration(&mut self, s: &mut Stack, origin: [i32; 3]) {
        let Some(Nbt::Compound(cd)) = component(s, "minecraft:custom_data").cloned() else { return };
        let Some((_, params)) = cd.iter().find(|(k, _)| k == "rustcraft:explore").cloned() else { return };
        let rest: Vec<(String, Nbt)> = cd.into_iter().filter(|(k, _)| k != "rustcraft:explore").collect();
        if rest.is_empty() {
            remove_component(s, "minecraft:custom_data");
        } else {
            set_component(s, "minecraft:custom_data", Nbt::Compound(rest));
        }
        let dest = params.get("destination").and_then(Nbt::as_str).unwrap_or("#minecraft:on_treasure_maps").to_owned();
        let deco = params.get("decoration").and_then(Nbt::as_str).unwrap_or("minecraft:mansion").to_owned();
        let zoom = crate::nbtio::int(&params, "zoom").unwrap_or(2).clamp(0, 4) as u8;
        let radius = crate::nbtio::int(&params, "search_radius").unwrap_or(50) as i32;
        let wanted = self.structure_tag_members(&dest);
        // `skip_existing_chunks` (default true): structure already designated by a skipped map.
        let skip = crate::nbtio::int(&params, "skip_existing_chunks").is_none_or(|v| v != 0);
        let Some((p, _)) = self.structure_search_ref(&wanted, origin, radius, skip) else {
            s.count = 0;
            return;
        };
        let id = self.map_create(f64::from(p[0]), f64::from(p[2]), zoom, true, true);
        set_component(s, "minecraft:map_id", Nbt::Int(id));
        self.map_biome_preview(id);
        let entry = Nbt::Compound(vec![
            ("type".into(), Nbt::String(deco)),
            ("x".into(), Nbt::Double(f64::from(p[0]))),
            ("z".into(), Nbt::Double(f64::from(p[2]))),
            ("rotation".into(), Nbt::Float(180.0)),
        ]);
        let mut decos = match component(s, "minecraft:map_decorations") {
            Some(Nbt::Compound(f)) => f.clone(),
            _ => Vec::new(),
        };
        decos.retain(|(k, _)| k != "+");
        decos.push(("+".into(), entry));
        set_component(s, "minecraft:map_decorations", Nbt::Compound(decos));
    }

    /// Trade offers carrying an exploration map to resolve (origin: the villager);
    /// an offer without a structure found is removed.
    pub(crate) fn resolve_offer_maps(&mut self, offers: &mut Vec<crate::mobs::trades::Offer>, origin: [i32; 3]) {
        for o in offers.iter_mut() {
            self.resolve_exploration(&mut o.result, origin);
        }
        offers.retain(|o| o.result.count > 0);
    }

    /// Water outlines (biomes `water_on_map_outlines`).
    fn map_biome_preview(&mut self, id: i32) {
        let Some(m) = self.maps.maps.get(&id) else { return };
        let scale = 1i32 << m.scale;
        let (sx, sz) = (m.cx / scale - 64, m.cz / scale - 64);
        let sea = self.map_sea_level();
        let biomes = self.regs.entries("minecraft:worldgen/biome").to_vec();
        let watery_tag: Vec<String> = self.biome_tag_members("minecraft:water_on_map_outlines");
        let points: Vec<[i32; 3]> =
            (0..128).flat_map(|row| (0..128).map(move |col| [(sx + col) * scale, sea, (sz + row) * scale])).collect();
        let found = self.generated_biomes(&points);
        let watery: Vec<bool> =
            found.iter().map(|b| b.and_then(|i| biomes.get(i)).is_some_and(|n| watery_tag.contains(n))).collect();
        let at = |x: i32, z: i32| watery[(z * 128 + x) as usize];
        let mut out = Vec::new();
        for mx in 1..127 {
            for mz in 1..127 {
                let mut n = 0;
                for dx in -1..2 {
                    for dz in -1..2 {
                        if (dx != 0 || dz != 0) && at(mx + dx, mz + dz) {
                            n += 1;
                        }
                    }
                }
                let mut b = LOWEST;
                let mut c = NONE;
                if at(mx, mz) {
                    c = COLOR_ORANGE;
                    if n > 7 && mz % 2 == 0 {
                        let k = (mx + (rc_physics::sin_t(mz as f32) * 7.0) as i32) / 8 % 5;
                        b = match k {
                            0 | 4 => LOW,
                            1 | 3 => NORMAL,
                            2 => HIGH,
                            _ => LOWEST,
                        };
                    } else if n > 7 {
                        c = NONE;
                    } else if n > 5 {
                        b = NORMAL;
                    } else if n > 1 {
                        b = LOW;
                    }
                } else if n > 0 {
                    c = COLOR_BROWN;
                    b = if n > 3 { NORMAL } else { LOWEST };
                }
                if c != NONE {
                    out.push((mx, mz, packed(c, b)));
                }
            }
        }
        if let Some(m) = self.maps.maps.get_mut(&id) {
            for (x, z, v) in out {
                m.set_color(x, z, v);
            }
        }
    }

    /// Sea level of the dimension (generator settings; 63 by default).
    fn map_sea_level(&self) -> i32 {
        63
    }

    fn biome_tag_members(&self, tag: &str) -> Vec<String> {
        let names = self.regs.entries("minecraft:worldgen/biome");
        let short = tag.trim_start_matches("minecraft:");
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:worldgen/biome")
            .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag || n.trim_start_matches("minecraft:") == short))
            .map(|(_, ids)| ids.iter().filter_map(|&i| names.get(i as usize).cloned()).collect())
            .unwrap_or_default()
    }
}
