//! Maps (`docs/specs/maps.md`): client table of the map data (§ 2), colors (§ 3),
//! decoration types (§ 4) and 128 × 128 textures sent per modified rectangle (§ 5).
//!
//! The table belongs to the client world: only one at a time, hence a global state read by the
//! entity engine (frames, hand) and the tooltip. A new connection clears it; a dimension
//! change keeps it.

use hashbrown::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

use rc_assets::texture::Image;
use rc_proto_v777::item::ItemStack;
use rc_proto_v777::map::{MapDecoration, MapItemData};

/// Side of a map in pixels.
pub const SIZE: usize = 128;

/// Base colors (RGB), 64 slots of which 62 defined; 0 = transparent.
const BASE: [u32; 64] = [
    0x000000, 0x7FB238, 0xF7E9A3, 0xC7C7C7, 0xFF0000, 0xA0A0FF, 0xA7A7A7, 0x007C00, 0xFFFFFF, 0xA4A8B8, 0x976D4D,
    0x707070, 0x4040FF, 0x8F7748, 0xFFFCF5, 0xD87F33, 0xB24CD8, 0x6699D8, 0xE5E533, 0x7FCC19, 0xF27FA5, 0x4C4C4C,
    0x999999, 0x4C7F99, 0x7F3FB2, 0x334CB2, 0x664C33, 0x667F33, 0x993333, 0x191919, 0xFAEE4D, 0x5CDBD5, 0x4A80FF,
    0x00D93A, 0x815631, 0x700200, 0xD1B1A1, 0x9F5224, 0x95576C, 0x706C8A, 0xBA8524, 0x677535, 0xA04D4E, 0x392923,
    0x876B62, 0x575C5C, 0x7A4958, 0x4C3E5C, 0x4C3223, 0x4C522A, 0x8E3C2E, 0x251610, 0xBD3031, 0x943F61, 0x5C191D,
    0x167E86, 0x3A8E8C, 0x562C3E, 0x14B485, 0x646464, 0xD8AF93, 0x7FA796, 0x000000, 0x000000,
];

/// Multiplier of each shade (out of 255).
const SHADES: [u32; 4] = [180, 220, 255, 135];

/// Pixel byte → RGBA (256 entries computed once).
pub fn palette() -> &'static [[u8; 4]; 256] {
    static P: LazyLock<[[u8; 4]; 256]> = LazyLock::new(|| {
        std::array::from_fn(|p| {
            let base = p >> 2;
            if base == 0 || base >= 62 {
                return [0; 4];
            }
            let m = SHADES[p & 3];
            let c = BASE[base];
            let ch = |s: u32| ((c >> s & 0xff) * m / 255).min(255) as u8;
            [ch(16), ch(8), ch(0), 255]
        })
    });
    &P
}

/// Decoration types in registry order: (sprite, visible in a frame).
pub const DECORATIONS: [(&str, bool); 40] = [
    ("player", false),
    ("frame", true),
    ("red_marker", false),
    ("blue_marker", false),
    ("target_x", true),
    ("target_point", true),
    ("player_off_map", false),
    ("player_off_limits", false),
    ("woodland_mansion", true),
    ("ocean_monument", true),
    ("white_banner", true),
    ("orange_banner", true),
    ("magenta_banner", true),
    ("light_blue_banner", true),
    ("yellow_banner", true),
    ("lime_banner", true),
    ("pink_banner", true),
    ("gray_banner", true),
    ("light_gray_banner", true),
    ("cyan_banner", true),
    ("purple_banner", true),
    ("blue_banner", true),
    ("brown_banner", true),
    ("green_banner", true),
    ("red_banner", true),
    ("black_banner", true),
    ("red_x", true),
    ("desert_village", true),
    ("plains_village", true),
    ("savanna_village", true),
    ("snowy_village", true),
    ("taiga_village", true),
    ("jungle_temple", true),
    ("swamp_hut", true),
    ("trial_chambers", true),
    ("abandoned_camp", true),
    ("ancient_city", true),
    ("desert_pyramid", true),
    ("mineshaft", true),
    ("warm_ocean_ruins", true),
];

/// Data of a map known to the client.
pub struct MapData {
    /// Frozen at creation of the entry.
    pub scale: i8,
    pub locked: bool,
    /// 128 × 128 color bytes (index x + 128·y).
    pub colors: Box<[u8]>,
    pub decorations: Vec<MapDecoration>,
    /// Rectangle modified since the last upload to the GPU: x0, y0, x1, y1 (inclusive).
    dirty: Option<[u8; 4]>,
}

#[derive(Default)]
pub struct MapStore {
    maps: HashMap<i32, MapData>,
    /// Changes on every reset (the GPU textures of the old maps are no longer valid).
    pub generation: u32,
}

impl MapStore {
    /// Reception of a `map_item_data` packet (§ 2).
    pub fn apply(&mut self, p: MapItemData) {
        if rc_data::env_flag!("RUSTCRAFT_MAPLOG") {
            let patch = p.patch.as_ref().map(|q| (q.x0, q.y0, q.width, q.height, q.colors.len()));
            eprintln!(
                "map {}: scale {}, lock {}, decorations {:?}, rectangle {patch:?}",
                p.id,
                p.scale,
                p.locked,
                p.decorations.as_ref().map(Vec::len)
            );
        }
        let m = self.maps.entry(p.id).or_insert_with(|| MapData {
            scale: p.scale,
            locked: p.locked,
            colors: vec![0; SIZE * SIZE].into_boxed_slice(),
            decorations: Vec::new(),
            dirty: None,
        });
        if let Some(d) = p.decorations {
            m.decorations = d;
        }
        let Some(patch) = p.patch else { return };
        let w = usize::from(patch.width);
        let mut rect: Option<[u8; 4]> = None;
        for (i, &c) in patch.colors.iter().enumerate().take(w * usize::from(patch.height)) {
            let (x, y) = (usize::from(patch.x0) + i % w, usize::from(patch.y0) + i / w);
            // Outside the map: ignored (the game would throw an exception).
            if x >= SIZE || y >= SIZE {
                continue;
            }
            m.colors[x + SIZE * y] = c;
            let (x, y) = (x as u8, y as u8);
            rect = Some(match rect {
                None => [x, y, x, y],
                Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
            });
        }
        if let Some(r) = rect {
            m.dirty = Some(match m.dirty {
                None => r,
                Some(d) => [d[0].min(r[0]), d[1].min(r[1]), d[2].max(r[2]), d[3].max(r[3])],
            });
        }
    }

    /// New connection: empty table.
    pub fn clear(&mut self) {
        self.maps.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn get(&self, id: i32) -> Option<&MapData> {
        self.maps.get(&id)
    }
}

impl MapData {
    /// RGBA pixels of the rectangle (x0, y0, x1, y1 inclusive).
    fn image(&self, r: [u8; 4]) -> Image {
        let pal = palette();
        let (w, h) = (u32::from(r[2] - r[0]) + 1, u32::from(r[3] - r[1]) + 1);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in r[1]..=r[3] {
            let row = usize::from(y) * SIZE;
            for x in r[0]..=r[2] {
                rgba.extend_from_slice(&pal[usize::from(self.colors[row + usize::from(x)])]);
            }
        }
        Image { width: w, height: h, rgba }
    }
}

static STORE: LazyLock<Mutex<MapStore>> = LazyLock::new(Mutex::default);

/// Table of the client world's maps.
pub fn store() -> MutexGuard<'static, MapStore> {
    STORE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Map id of a stack (`map_id` component of the patch).
pub fn map_id(ids: &crate::stack::ComponentIds, stack: &ItemStack) -> Option<i32> {
    let b = stack.component(ids.map_id?)?;
    rc_proto::varint::read_varint(&mut rc_proto::Reader::new(b)).ok()
}

/// Tooltip lines of a map (§ 9), gray: `post` = post-processing (0 lock,
/// 1 enlarge), `named` = custom name present, `advanced` = advanced tooltips (F3 + H).
pub fn tooltip(id: i32, post: Option<i32>, named: bool, advanced: bool) -> Vec<rc_text::Component> {
    use rc_text::{Arg, Component};
    let gray = |key: &str, args: Vec<Arg>| {
        let mut c = Component::translate(key, args);
        c.style.color = Some(0xaa_aaaa);
        c
    };
    let store = store();
    let Some(m) = store.get(id) else { return vec![gray("filled_map.unknown", Vec::new())] };
    let mut out = Vec::new();
    if !named && post.is_none() {
        out.push(gray("filled_map.id", vec![Arg::Text(id.to_string())]));
    }
    if m.locked || post == Some(0) {
        out.push(gray("filled_map.locked", Vec::new()));
    }
    if advanced {
        let s = (i32::from(m.scale) + i32::from(post == Some(1))).min(4);
        out.push(gray("filled_map.scale", vec![Arg::Text((1i64 << s.clamp(0, 30)).to_string())]));
        out.push(gray("filled_map.level", vec![Arg::Text(s.to_string()), Arg::Text("4".into())]));
    }
    out
}

/// Map textures on the render side: a 128 × 128 slot per map in the entity
/// sheet, created at first draw, then only the modified rectangle is sent again.
pub struct MapTextures {
    rects: HashMap<i32, [f32; 4]>,
    /// Slots freed (table reset), reused.
    free: Vec<[f32; 4]>,
    generation: u32,
    next: u32,
    /// First vertex of the unit square in the item mesh buffer.
    pub quad_first: u32,
}

impl MapTextures {
    pub fn new(quad_first: u32) -> Self {
        MapTextures { rects: HashMap::new(), free: Vec::new(), generation: 0, next: 0, quad_first }
    }

    /// Rectangle (sheet pixels) of the texture of map `id`, up to date; `None` if the
    /// map is unknown.
    pub fn texture(
        &mut self,
        atlas: &mut crate::entity_atlas::EntityAtlas,
        store: &mut MapStore,
        id: i32,
    ) -> Option<[f32; 4]> {
        if self.generation != store.generation {
            self.generation = store.generation;
            self.free.extend(self.rects.drain().map(|(_, r)| r));
        }
        let m = store.maps.get_mut(&id)?;
        if let Some(&r) = self.rects.get(&id) {
            if let Some(d) = m.dirty.take() {
                atlas.write(r[0] as u32 + u32::from(d[0]), r[1] as u32 + u32::from(d[1]), m.image(d));
            }
            return Some(r);
        }
        let full = m.image([0, 0, SIZE as u8 - 1, SIZE as u8 - 1]);
        m.dirty = None;
        let r = match self.free.pop() {
            Some(r) => {
                atlas.write(r[0] as u32, r[1] as u32, full);
                r
            }
            None => {
                let name = format!("#map/{}", self.next);
                self.next += 1;
                atlas.insert(&name, full);
                atlas.rect(&name)?
            }
        };
        self.rects.insert(id, r);
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_proto_v777::map::MapPatch;

    fn packet(id: i32, scale: i8, locked: bool, patch: Option<MapPatch>) -> MapItemData {
        MapItemData { id, scale, locked, decorations: None, patch }
    }

    #[test]
    fn palette_values() {
        let p = palette();
        assert_eq!(p[0], [0; 4]);
        assert_eq!(p[3], [0; 4]);
        // Grass (1), shade 2 (255): exact color; shade 0 (180): ⌊c × 180 / 255⌋.
        assert_eq!(p[1 << 2 | 2], [0x7f, 0xb2, 0x38, 255]);
        assert_eq!(p[1 << 2], [89, 125, 39, 255]);
        assert_eq!(p[62 << 2 | 1], [0; 4]);
    }

    #[test]
    fn frozen_scale_and_patch() {
        let mut s = MapStore::default();
        s.apply(packet(1, 2, false, None));
        s.apply(packet(
            1,
            4,
            true,
            Some(MapPatch { x0: 126, y0: 3, width: 4, height: 1, colors: vec![9, 10, 11, 12] }),
        ));
        let m = s.get(1).unwrap();
        assert_eq!((m.scale, m.locked), (2, false));
        assert_eq!(&m.colors[126 + 3 * SIZE..128 + 3 * SIZE], &[9, 10]);
        assert_eq!(m.dirty, Some([126, 3, 127, 3]));
    }

    #[test]
    fn decorations_replaced_or_kept() {
        let mut s = MapStore::default();
        let d = MapDecoration { kind: 1, x: 0, y: 0, rot: 0, name: None };
        let mut p = packet(5, 0, false, None);
        p.decorations = Some(vec![d.clone(), d]);
        s.apply(p);
        s.apply(packet(5, 0, false, None));
        assert_eq!(s.get(5).unwrap().decorations.len(), 2);
        let mut p = packet(5, 0, false, None);
        p.decorations = Some(Vec::new());
        s.apply(p);
        assert!(s.get(5).unwrap().decorations.is_empty());
    }
}
